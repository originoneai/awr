//! Durable source-write gaps fence effects and dependency metadata separately.
use super::*;

pub(crate) async fn require_effect_clear(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<()> {
    require_work_clear(tx, tenant, project, work, false).await
}

pub(crate) async fn require_dependency_clear(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<()> {
    require_work_clear(tx, tenant, project, work, true).await
}

async fn require_work_clear(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    dependency: bool,
) -> PgResult<()> {
    let row = tx.query_opt(
        "SELECT request_id FROM awr_team.planning_writeback_journals
         WHERE tenant_id=$1 AND project_id=$2 AND phase IN ('validated','source_written','pg_activating')
           AND (intent_json IS NULL OR CASE WHEN $4 THEN dependency_work_ids ELSE affected_work_ids END ? $3)
         ORDER BY request_id LIMIT 1",
        &[&tenant,&project,&work,&dependency],
    ).await?;
    if let Some(row) = row {
        return Err(PgError::ActionBlockedByInvalidation(format!(
            "source_writeback:{}",
            row.get::<_, String>(0)
        )));
    }
    Ok(())
}

/// Source replacement itself cannot commute with another unfinished file intent.
/// Only the internally verified original request may install its own snapshot.
pub(crate) async fn require_source_available(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    own_request: Option<&str>,
) -> PgResult<()> {
    let pending: bool = tx.query_one(
        "SELECT EXISTS(SELECT 1 FROM awr_team.planning_writeback_journals
         WHERE tenant_id=$1 AND project_id=$2 AND phase IN ('validated','source_written','pg_activating')
           AND ($3::text IS NULL OR request_id<>$3))",
        &[&tenant,&project,&own_request],
    ).await?.get(0);
    if pending {
        return Err(PgError::WritebackRefused(
            "an original source writeback must be queried and recovered first".into(),
        ));
    }
    Ok(())
}
