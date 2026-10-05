//! A controlled runner may clear only its precisely attributed report barrier.
//! Aggregate, historical and subsequently overwritten barriers stay operator-only.
use super::*;

pub(super) fn grant_matches(run: &Row, auth: &ReaderAuthority, stream: &Id) -> bool {
    run.get::<_, Option<i64>>("attestation_grant_version")
        .zip(auth.grant_versions.get(stream))
        .is_some_and(|(admitted, current)| admitted == *current)
}

fn controlled(run: &Row, auth: &ReaderAuthority, stream: &Id, contract: &str) -> bool {
    run.get::<_, Option<String>>("admission_mode").as_deref() == Some("reference_write_v1")
        && auth.execution_access.get(stream).is_some_and(|a| a.attest)
        && grant_matches(run, auth, stream)
        && run.get::<_, String>("executor_actor_id") == auth.actor_id
        && run
            .get::<_, Option<String>>("executor_client_id")
            .as_deref()
            == Some(&auth.client_id)
        && run.get::<_, Option<String>>("coordinator_epoch").as_deref() == Some(&auth.epoch)
        && run.get::<_, String>("contract_hash") == contract
        && run.get::<_, i64>("work_last_fence") == run.get::<_, i64>("fence")
}

async fn exact_resources(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    run: &Row,
) -> PgResult<bool> {
    let declared: Vec<String> = serde_json::from_value(run.get("declared_scope_json"))
        .map_err(|_| PgError::SourceDivergence)?;
    let Ok(paths) = settlement::normalized_scope(&declared) else {
        return Ok(false);
    };
    let Some(generation) = run.get::<_, Option<i64>>("admission_lease_version") else {
        return Ok(false);
    };
    let rows = tx
        .query(
            "SELECT execution_id,resource_kind,canonical_key,worktree_id,lease_generation,fence
        FROM awr_team.resource_reservations WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
          AND state IN ('reserved','unknown') ORDER BY id",
            &[&tenant, &project, &run.get::<_, String>("work_id")],
        )
        .await?;
    let mut seen = std::collections::BTreeSet::new();
    Ok(rows.len() == paths.len()
        && rows.iter().all(|r| {
            let key: String = r.get(2);
            r.get::<_, Option<String>>(0).as_deref() == Some(run.get::<_, String>("id").as_str())
                && r.get::<_, String>(1) == "dir"
                && paths.contains(&key)
                && seen.insert(key)
                && r.get::<_, String>(3).is_empty()
                && r.get::<_, i64>(4) == generation
                && r.get::<_, i64>(5) == run.get::<_, i64>("fence")
        }))
}

async fn unexposed(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    run: &Row,
) -> PgResult<bool> {
    Ok(tx.query_one("SELECT
        NOT EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
          AND id<>$4 AND state NOT IN ('succeeded','failed','cancelled'))
        AND NOT EXISTS(SELECT 1 FROM awr_team.outbox o JOIN awr_team.executions e
          ON e.tenant_id=o.tenant_id AND e.project_id=o.project_id AND e.id=o.aggregate_id
          WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.work_id=$3)
        AND EXISTS(SELECT 1 FROM awr_team.work_contracts c JOIN awr_team.work_scopes s
          ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.scope_id
          WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$3 AND c.snapshot_id=$5
            AND c.scope_id='main' AND c.definition_state='enabled' AND s.status='active')",
        &[&tenant,&project,&run.get::<_,String>("work_id"),&run.get::<_,String>("id"),&auth.snapshot]).await?.get(0))
}

pub(super) async fn available(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    stream: &Id,
    contract: &str,
    run: &Row,
    latest: Option<&Value>,
) -> PgResult<bool> {
    let Some(latest) = latest else {
        return Ok(false);
    };
    let payload = &latest["payload"];
    if !controlled(run, auth, stream, contract)
        || !run.get::<_, bool>("recovery_blocked")
        || run
            .get::<_, Option<String>>("recovery_execution_id")
            .as_deref()
            != Some(run.get::<_, String>("id").as_str())
        || run
            .get::<_, Option<String>>("recovery_receipt_id")
            .as_deref()
            != latest["receipt_id"].as_str()
        || latest["receipt_kind"] != "caller_asserted"
        || awr_team::request_hash(payload).ok().as_deref() != latest["digest"].as_str()
        || payload["scope_violation"] != false
        || payload["client_id"] != auth.client_id
        || payload["session_id"]
            != run
                .get::<_, Option<String>>("session_id")
                .unwrap_or_default()
        || payload["coordinator_epoch"] != auth.epoch
        || payload["contract_hash"] != contract
        || payload["workstream_id"] != stream.to_string()
        || payload["ownership_version"]
            .as_str()
            .and_then(|v| version(v).ok())
            != run.get::<_, Option<i64>>("ownership_version")
        || payload["execution_version"]
            .as_str()
            .and_then(|v| version(v).ok())
            .and_then(|v| v.checked_add(1))
            != Some(run.get::<_, i64>("execution_version"))
    {
        return Ok(false);
    }
    Ok(exact_resources(tx, tenant, project, run).await?
        && unexposed(tx, tenant, project, auth, run).await?)
}

pub(super) async fn may_attribute(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    stream: &Id,
    contract: &str,
    run: &Row,
    exceeded: bool,
) -> PgResult<bool> {
    if exceeded || !run.get::<_, bool>("lease_live") || !controlled(run, auth, stream, contract) {
        return Ok(false);
    }
    if run.get::<_, bool>("recovery_blocked") {
        let latest =
            recovery::latest_receipt(tx, tenant, project, &run.get::<_, String>("id")).await?;
        return available(
            tx,
            tenant,
            project,
            auth,
            stream,
            contract,
            run,
            latest.as_ref(),
        )
        .await;
    }
    Ok(exact_resources(tx, tenant, project, run).await?
        && unexposed(tx, tenant, project, auth, run).await?)
}

pub(super) async fn attribute(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    run: &Row,
    receipt: &str,
) -> PgResult<()> {
    let changed = tx.execute("UPDATE awr_team.work_runtime SET recovery_execution_id=$4,recovery_receipt_id=$5
        WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3 AND recovery_blocked",
        &[&tenant,&project,&run.get::<_,String>("work_id"),&run.get::<_,String>("id"),&receipt]).await?;
    if changed != 1 {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}
