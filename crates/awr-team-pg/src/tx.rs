use crate::error::{PgError, PgResult};
use serde::Deserialize;
use serde_json::{Value, json};

/// i64 values that travel as decimal strings at response/receipt boundaries
/// (fence, lease_version, revisions). Deserialization accepts legacy numeric
/// receipts too. Shared by claim/execution records.
pub(crate) fn ser_i64_string<S: serde::Serializer>(
    value: &i64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

pub(crate) fn de_i64_flex<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<i64, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::String(s) => s.parse().map_err(serde::de::Error::custom),
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("invalid i64")),
        other => Err(serde::de::Error::custom(format!(
            "expected decimal string or number, got {other}"
        ))),
    }
}

/// Bump the project revision and append an event in the same transaction
/// (the project row must already be locked by the caller). Replays return
/// before mutations, so they never duplicate events.
pub(crate) async fn emit_event(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    actor_id: &str,
    work_id: &str,
    event_type: &str,
    payload: Value,
) -> PgResult<i64> {
    let revision: i64 = tx
        .query_one(
            "SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant_id, &project_id],
        )
        .await?
        .get(0);
    let next = revision + 1;
    tx.execute(
        "UPDATE awr_team.projects SET project_revision=$1
         WHERE tenant_id=$2 AND id=$3 AND project_revision=$4",
        &[&next, &tenant_id, &project_id, &revision],
    )
    .await?;
    let event_id = new_id();
    tx.execute(
        "INSERT INTO awr_team.events(
            tenant_id, project_id, id, project_revision, event_index,
            event_type, actor_id, work_id, payload_json)
         VALUES ($1,$2,$3,$4,0,$5,$6,$7,$8)",
        &[
            &tenant_id,
            &project_id,
            &event_id,
            &next,
            &event_type,
            &actor_id,
            &work_id,
            &payload,
        ],
    )
    .await?;
    Ok(next)
}

/// Attempts of work that PostgreSQL may roll back with a serialization failure or a deadlock.
const ROLLED_BACK_ATTEMPTS: u32 = 8;

/// Run `attempt` again, with a short exponential back-off, while PostgreSQL reports that it rolled
/// the transaction back (SQLSTATE 40001 or 40P01, see `PgError::is_retryable`). At most eight
/// attempts and 635 ms of waiting, the bounds the workstream read store already uses.
///
/// Only for work that is safe to repeat: a read-only transaction, or a request keyed by an
/// idempotency receipt. A repeatable-read transaction that locks a row another transaction updated
/// after its snapshot started fails this way, and the project row is updated by every command.
pub async fn retry_rolled_back<T, F, Fut>(mut attempt: F) -> PgResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = PgResult<T>>,
{
    for n in 0..ROLLED_BACK_ATTEMPTS {
        match attempt().await {
            Err(error) if n + 1 < ROLLED_BACK_ATTEMPTS && error.is_retryable() => {
                tokio::time::sleep(std::time::Duration::from_millis(5 << n)).await;
            }
            other => return other,
        }
    }
    unreachable!("the final attempt always returns")
}

/// Ordinary writes serialize with freeze/import/restore and require active state.
pub(crate) async fn lock_active_project(
    tx: &tokio_postgres::Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<()> {
    let row = tx
        .query_opt(
            "SELECT status FROM awr_team.projects WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
            &[&tenant, &project],
        )
        .await?
        .ok_or(PgError::ProjectNotAvailable)?;
    if row.get::<_, String>(0) != "active" {
        return Err(PgError::ProjectNotAvailable);
    }
    Ok(())
}

pub struct TeamStore {
    pool: crate::PgPool,
}

#[derive(Clone, Debug)]
pub struct CommandRequest {
    pub tenant_id: String,
    pub project_id: String,
    pub actor_id: String,
    pub client_id: String,
    pub request_id: String,
    pub op: String,
    pub args: Value,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct CommandOutcome {
    pub replayed: bool,
    pub committed_project_revision: String,
    pub result: Value,
}

impl TeamStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: crate::PgPool::new(url),
        }
    }

    /// Build from a validated `tokio_postgres::Config` (see PgPool::from_config).
    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: crate::PgPool::from_config(config),
        }
    }

    pub(crate) async fn connect(&self) -> PgResult<crate::PgClient> {
        self.pool.get().await
    }

    pub async fn execute(&self, request: CommandRequest) -> PgResult<CommandOutcome> {
        let mut client = self.connect().await?;
        // The command entry must refuse an incompatible or half-migrated
        // database BEFORE any state change; `awr-server check` alone does not
        // protect this path (CR #36 P2-2). Fails before the transaction opens,
        // so business state, revisions, events and receipts stay untouched.
        crate::migrate::check_schema(&client).await?;
        let request_hash = hash_request(&request)?;
        let tx = client.transaction().await?;
        bind_scope(&tx, &request.tenant_id, &request.project_id).await?;
        lock_active_project(&tx, &request.tenant_id, &request.project_id).await?;
        let locked = tx
            .query_opt(
                "SELECT project_revision FROM awr_team.projects
                 WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
                &[&request.tenant_id, &request.project_id],
            )
            .await?;
        let Some(row) = locked else {
            return Err(PgError::ProjectNotAvailable);
        };
        let revision: i64 = row.get(0);
        if let Some(existing) = load_operation(&tx, &request).await? {
            // Validate the request identity FIRST; legacy receipts from other
            // stores may carry a NULL committed revision and must still reach
            // this comparison instead of panicking during parsing (CR #56 P2-1).
            if existing.0 != request_hash {
                return Err(PgError::IdempotencyConflict);
            }
            let committed = existing.1.ok_or_else(|| {
                PgError::Protocol("legacy receipt has no committed revision".into())
            })?;
            return Ok(CommandOutcome {
                replayed: true,
                committed_project_revision: committed.to_string(),
                result: existing.2,
            });
        }
        let next = revision + 1;
        let result = apply_op(&tx, &request, next).await?;
        tx.execute(
            "UPDATE awr_team.projects SET project_revision=$1
             WHERE tenant_id=$2 AND id=$3 AND project_revision=$4",
            &[&next, &request.tenant_id, &request.project_id, &revision],
        )
        .await?;
        let event_id = new_id();
        let op_id = new_id();
        let event_type = format!("command.{}", request.op);
        let work_id: Option<String> = request
            .args
            .get("work_id")
            .and_then(|v| v.as_str().map(str::to_owned));
        tx.execute(
            "INSERT INTO awr_team.events(
                tenant_id, project_id, id, project_revision, event_index,
                event_type, actor_id, work_id, payload_json)
             VALUES ($1,$2,$3,$4,0,$5,$6,$7,$8)",
            &[
                &request.tenant_id,
                &request.project_id,
                &event_id,
                &next,
                &event_type,
                &request.actor_id,
                &work_id,
                &result,
            ],
        )
        .await?;
        tx.execute(
            "INSERT INTO awr_team.operations(
                tenant_id, project_id, id, actor_id, client_id, request_id, op,
                request_hash, state, committed_project_revision, result_json)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'committed',$9,$10)",
            &[
                &request.tenant_id,
                &request.project_id,
                &op_id,
                &request.actor_id,
                &request.client_id,
                &request.request_id,
                &request.op,
                &request_hash,
                &next,
                &result,
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(CommandOutcome {
            replayed: false,
            committed_project_revision: next.to_string(),
            result,
        })
    }

    pub async fn abort_after_partial_write(&self, request: CommandRequest) -> PgResult<()> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_scope(&tx, &request.tenant_id, &request.project_id).await?;
        let event_id = new_id();
        let payload = json!({});
        tx.execute(
            "INSERT INTO awr_team.events(
                tenant_id, project_id, id, project_revision, event_index,
                event_type, actor_id, work_id, payload_json)
             VALUES ($1,$2,$3,1,0,'test.partial',$4,NULL,$5)",
            &[
                &request.tenant_id,
                &request.project_id,
                &event_id,
                &request.actor_id,
                &payload,
            ],
        )
        .await?;
        tx.rollback().await?;
        Ok(())
    }
}

pub(crate) async fn bind_scope(
    tx: &tokio_postgres::Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<()> {
    bind_workstream_scope(tx, tenant, project).await?;
    // Hold admission through the action, including repeatable-read queries.
    // Source enablement locks this separate row before the project row, so a
    // legacy caller cannot pass the check and write after scopes become active.
    let enabled: bool = tx
        .query_opt(
            "SELECT enabled FROM awr_team.workstream_modes
         WHERE tenant_id=$1 AND project_id=$2 FOR SHARE",
            &[&tenant, &project],
        )
        .await?
        .ok_or(PgError::ProjectNotAvailable)?
        .get(0);
    if enabled {
        return Err(PgError::Unsupported(
            "enabled workstreams require authenticated scoped operations".into(),
        ));
    }
    Ok(())
}

/// Internal binding for source coordination and explicitly authenticated APIs.
/// This sets only the tenant/project RLS boundary; it does NOT authorize a caller.
pub(crate) async fn bind_workstream_scope(
    tx: &tokio_postgres::Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<()> {
    tx.execute("SELECT set_config('awr.tenant_id', $1, true)", &[&tenant])
        .await?;
    tx.execute("SELECT set_config('awr.project_id', $1, true)", &[&project])
        .await?;
    Ok(())
}

async fn load_operation(
    tx: &tokio_postgres::Transaction<'_>,
    request: &CommandRequest,
) -> PgResult<Option<(String, Option<i64>, Value)>> {
    let row = tx
        .query_opt(
            "SELECT request_hash, committed_project_revision, result_json
             FROM awr_team.operations
             WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND request_id=$5",
            &[
                &request.tenant_id,
                &request.project_id,
                &request.actor_id,
                &request.client_id,
                &request.request_id,
            ],
        )
        .await?;
    Ok(row.map(|row| (row.get(0), row.get(1), row.get(2))))
}

async fn apply_op(
    tx: &tokio_postgres::Transaction<'_>,
    request: &CommandRequest,
    revision: i64,
) -> PgResult<Value> {
    match request.op.as_str() {
        "work.touch" => {
            let work_id = request
                .args
                .get("work_id")
                .and_then(Value::as_str)
                .ok_or_else(|| PgError::Protocol("work_id required".into()))?
                .to_owned();
            let scope_id = request
                .args
                .get("scope_id")
                .and_then(Value::as_str)
                .ok_or_else(|| PgError::Protocol("scope_id required".into()))?
                .to_owned();
            tx.execute(
                "INSERT INTO awr_team.work_runtime(
                    tenant_id, project_id, scope_id, work_id, state, work_version, last_fence)
                 VALUES ($1,$2,$3,$4,'pending',1,0)
                 ON CONFLICT (tenant_id, project_id, scope_id, work_id)
                 DO UPDATE SET work_version = awr_team.work_runtime.work_version + 1",
                &[&request.tenant_id, &request.project_id, &scope_id, &work_id],
            )
            .await?;
            // Version values are decimal strings end to end; a JSON number
            // would lose precision past 2^53 for JavaScript consumers, and
            // this payload is persisted and replayed verbatim (CR #36 P2-4).
            Ok(json!({"op":"work.touch","work_id":work_id,"revision":revision.to_string()}))
        }
        other => Err(PgError::Protocol(format!("unsupported op {other}"))),
    }
}

fn hash_request(request: &CommandRequest) -> PgResult<String> {
    let value = json!({
        "op": request.op,
        "args": request.args,
        "tenant_id": request.tenant_id,
        "project_id": request.project_id,
        "actor_id": request.actor_id,
        "client_id": request.client_id,
        "request_id": request.request_id,
    });
    awr_team::request_hash(&value).map_err(|e| PgError::Protocol(e.to_string()))
}

pub(crate) fn new_id() -> String {
    ulid::Ulid::new().to_string()
}

/// Shared reviewer/approver validation: the account must exist, be active,
/// and hold an approval-capable membership (admin/reviewer) in the project
/// (CR #42 P2-5; shared with the source approval path).
pub(crate) async fn validate_reviewer(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    reviewer_actor_id: &str,
) -> PgResult<()> {
    let reviewer = tx
        .query_opt(
            "SELECT a.status, m.role
             FROM awr_team.actors a
             LEFT JOIN awr_team.project_memberships m
               ON m.tenant_id=a.tenant_id AND m.actor_id=a.id
              AND m.project_id=$2
             WHERE a.tenant_id=$1 AND a.id=$3",
            &[&tenant_id, &project_id, &reviewer_actor_id],
        )
        .await?;
    let Some((status, role)) =
        reviewer.map(|r| (r.get::<_, String>(0), r.get::<_, Option<String>>(1)))
    else {
        return Err(PgError::Forbidden);
    };
    if status != "active" {
        return Err(PgError::Forbidden);
    }
    // Source-approval / membership capability (not TMCP review.decide).
    // Independent review.decide is gated separately via membership.independent_review.
    match role.as_deref() {
        Some("admin")
        | Some("project_admin")
        | Some("reviewer")
        | Some("maintainer")
        | Some("developer")
        | Some("worker") => Ok(()),
        _ => Err(PgError::Forbidden),
    }
}

/// Explicit Agent review membership authority, rechecked in the decision transaction.
pub(crate) async fn require_agent_review_grant(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    reviewer_actor_id: &str,
) -> PgResult<()> {
    let allowed = tx
        .query_opt(
            "SELECT a.kind='agent' AND a.status='active' AND m.agent_review
         FROM awr_team.actors a JOIN awr_team.project_memberships m
           ON m.tenant_id=a.tenant_id AND m.actor_id=a.id
         WHERE m.tenant_id=$1 AND m.project_id=$2 AND m.actor_id=$3",
            &[&tenant_id, &project_id, &reviewer_actor_id],
        )
        .await?
        .is_some_and(|r| r.get::<_, bool>(0));
    if allowed {
        Ok(())
    } else {
        Err(PgError::Forbidden)
    }
}

/// TMCP-031: explicit independent review.decide grant on an eligible template.
pub(crate) async fn require_independent_review_grant(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    reviewer_actor_id: &str,
) -> PgResult<()> {
    let row = tx
        .query_opt(
            "SELECT m.role, COALESCE(m.independent_review, false)
             FROM awr_team.project_memberships m
             WHERE m.tenant_id=$1 AND m.project_id=$2 AND m.actor_id=$3",
            &[&tenant_id, &project_id, &reviewer_actor_id],
        )
        .await?
        .ok_or(PgError::Forbidden)?;
    let role: String = row.get(0);
    let independent_review: bool = row.get(1);
    let template = crate::workstream_auth::map_membership_role(&role).ok_or(PgError::Forbidden)?;
    if independent_review && awr_team::independent_review_eligible(template) {
        Ok(())
    } else {
        Err(PgError::Forbidden)
    }
}
