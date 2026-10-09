//! Fixed lock order for Team PG writers (WS-023 / team architecture §9.3).
//!
//! Every writer that needs multiple row locks must follow this order so two
//! command shapes cannot deadlock across lines:
//!
//! 1. **Project barrier** — `workstream_modes` (admission) then `projects`
//! 2. **Auth / mainline** — tenants, actors, credentials, memberships, grants
//! 3. **Responsibility locks** — advisory locks for the request keys (sorted), then
//!    the per-task lock (`responsibility::lock_transition_keys`); only with the
//!    barrier held and before any `task_responsibilities` row is locked or written
//! 4. **Graph coordination** — dependency / graph snapshot rows when needed
//! 5. **Sorted tasks** — `work_runtime` rows ordered by `(scope_id, work_id)`
//! 6. **Sorted resources** — `resource_reservations` by `(kind, key, worktree_id)`
//! 7. **Receipts / events** — operations and events (append under held locks)
//!
//! Every store that writes responsibility or handoff rows takes the barrier first
//! (`agent_authorization::lock_project`, or the identical `FOR UPDATE` of the project
//! row in `team_handoff`), including the direct store APIs (`ResponsibilityStore`,
//! `HandoffStore`), not only the authenticated command path. A store call that took the task lock first and reached the barrier later,
//! through the foreign key of `task_responsibilities` (a KEY SHARE on the project
//! row), deadlocked against a command that already held the barrier and was waiting
//! for the same task lock. PostgreSQL reports such a conflict as SQLSTATE 40P01;
//! `PgError::is_retryable` recognizes it (and 40001) because the transaction was
//! rolled back and the request can be repeated with its original request ID.
//!
//! Freeze, import and restore take the project barrier exclusively and keep
//! it for the whole transition. Ordinary writers use the same barrier (or a
//! shared project lock when task interleaving is enabled) so barriers win
//! without torn state. Long-running work must run **outside** these locks.
//!
//! Claim rows are task-scoped leases: lock the owning `work_runtime` **before**
//! the claim whenever both are required. Discover `work_id` with an unlocked
//! read when the caller only has a claim id.

use crate::error::PgResult;
use tokio_postgres::Transaction;

/// Stable sort key for resource reservation rows.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ResourceLockKey {
    pub kind: String,
    pub key: String,
    /// Empty string means a shared (non-worktree) binding, matching stored rows.
    pub worktree_id: String,
}

impl ResourceLockKey {
    pub fn new(
        kind: impl Into<String>,
        key: impl Into<String>,
        worktree_id: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            key: key.into(),
            worktree_id: worktree_id.into(),
        }
    }
}

/// Sort work ids in place (lexicographic). Callers pass the scope separately.
pub fn sort_work_ids(work_ids: &mut [String]) {
    work_ids.sort();
}

/// Sort resource lock keys in place.
pub fn sort_resource_keys(keys: &mut [ResourceLockKey]) {
    keys.sort();
}

/// Lock `work_runtime` rows for `scope_id` in sorted `work_id` order.
/// Missing rows are skipped (caller may insert later under the same project barrier).
pub async fn lock_works_sorted(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    scope_id: &str,
    work_ids: &[String],
) -> PgResult<()> {
    let mut ordered = work_ids.to_vec();
    sort_work_ids(&mut ordered);
    ordered.dedup();
    for work_id in ordered {
        tx.query_opt(
            "SELECT work_id FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id=$3 AND work_id=$4
             FOR UPDATE",
            &[&tenant_id, &project_id, &scope_id, &work_id],
        )
        .await?;
    }
    Ok(())
}

/// Lock matching `resource_reservations` in sorted key order.
/// Only rows in `reserved` or `unknown` state are locked (active contention set).
pub async fn lock_resources_sorted(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    keys: &[ResourceLockKey],
) -> PgResult<()> {
    let mut ordered = keys.to_vec();
    sort_resource_keys(&mut ordered);
    ordered.dedup();
    for key in ordered {
        tx.query(
            "SELECT id FROM awr_team.resource_reservations
             WHERE tenant_id=$1 AND project_id=$2
               AND resource_kind=$3 AND canonical_key=$4
               AND worktree_id=$5
               AND state IN ('reserved', 'unknown')
             FOR UPDATE",
            &[
                &tenant_id,
                &project_id,
                &key.kind,
                &key.key,
                &key.worktree_id,
            ],
        )
        .await?;
    }
    Ok(())
}

/// Resolve the work identity for a claim without taking row locks, then lock
/// task → claim in the fixed order.
pub async fn lock_claim_after_work(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    claim_id: &str,
) -> PgResult<Option<(String, String)>> {
    let row = tx
        .query_opt(
            "SELECT scope_id, work_id FROM awr_team.claims
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &claim_id],
        )
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let scope_id: String = row.get(0);
    let work_id: String = row.get(1);
    lock_works_sorted(tx, tenant_id, project_id, &scope_id, &[work_id.clone()]).await?;
    let locked = tx
        .query_opt(
            "SELECT scope_id, work_id FROM awr_team.claims
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3
             FOR UPDATE",
            &[&tenant_id, &project_id, &claim_id],
        )
        .await?;
    Ok(locked.map(|r| (r.get(0), r.get(1))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_ids_sort_and_dedup_order_is_stable() {
        let mut ids = vec!["work-b".into(), "work-a".into(), "work-b".into()];
        sort_work_ids(&mut ids);
        ids.dedup();
        assert_eq!(ids, vec!["work-a".to_string(), "work-b".to_string()]);
    }

    #[test]
    fn resource_keys_sort_by_kind_key_worktree() {
        let mut keys = vec![
            ResourceLockKey::new("file", "src/b.rs", ""),
            ResourceLockKey::new("file", "src/a.rs", "wt-2"),
            ResourceLockKey::new("file", "src/a.rs", "wt-1"),
            ResourceLockKey::new("dir", "src", ""),
        ];
        sort_resource_keys(&mut keys);
        assert_eq!(keys[0].kind, "dir");
        assert_eq!(keys[1].key, "src/a.rs");
        assert_eq!(keys[1].worktree_id, "wt-1");
        assert_eq!(keys[2].worktree_id, "wt-2");
        assert_eq!(keys[3].key, "src/b.rs");
    }
}
