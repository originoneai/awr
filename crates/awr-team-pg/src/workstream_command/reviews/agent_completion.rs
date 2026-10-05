//! Explicit caller-managed completion keeps exact workspace settlement and
//! operator reconciliation distinct. Neither upgrades caller evidence to trust.
use super::*;

pub(super) async fn verify_execution(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    exec: &tokio_postgres::Row,
    artifact_digest: Option<&str>,
) -> PgResult<Value> {
    let id: String = exec.get("id");
    let client: String = exec
        .get::<_, Option<String>>("executor_client_id")
        .ok_or(PgError::EvidenceInvalid)?;
    let session: String = exec
        .get::<_, Option<String>>("session_id")
        .ok_or(PgError::EvidenceInvalid)?;
    if exec.get::<_, String>("work_id") != command.work_id
        || exec.get::<_, Option<String>>("workstream_id").as_deref()
            != Some(command.workstream_id.to_string().as_str())
        || exec.get::<_, Option<i64>>("ownership_version")
            != Some(version(&command.expected_ownership_version)?)
    {
        return Err(PgError::EvidenceInvalid);
    }
    let unresolved: bool = tx.query_one(
        "SELECT EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state NOT IN ('succeeded','failed','cancelled'))
         OR EXISTS(SELECT 1 FROM awr_team.resource_reservations WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state IN ('reserved','unknown'))
         OR EXISTS(SELECT 1 FROM awr_team.wait_items WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='open')",
        &[&tenant,&project,&command.work_id]).await?.get(0);
    if unresolved {
        return Err(PgError::RecoveryBlocked);
    }
    let row = tx.query_opt(
        "SELECT id,receipt_kind,digest,payload_json,reporter_actor_id FROM awr_team.execution_receipts
         WHERE tenant_id=$1 AND project_id=$2 AND execution_id=$3 ORDER BY created_at DESC,id DESC LIMIT 1",
        &[&tenant,&project,&id]).await?.ok_or(PgError::EvidenceInvalid)?;
    let receipt_id: String = row.get(0);
    let payload: Value = row.get(3);
    if row.get::<_, String>(1) == "caller_asserted"
        && exec.get::<_, bool>("workspace_effects_settled")
    {
        if row.get::<_, String>(4) != exec.get::<_, String>("executor_actor_id")
            || awr_team::request_hash(&payload).map_err(|_| PgError::EvidenceInvalid)?
                != row.get::<_, String>(2)
        {
            return Err(PgError::EvidenceInvalid);
        }
        let mut binding = executions::settlement::verify_receipt(
            tx,
            tenant,
            project,
            auth,
            command,
            exec,
            &payload,
            artifact_digest,
        )
        .await?;
        binding["caller_receipt_id"] = json!(receipt_id);
        binding["executor_client_id"] = json!(client);
        return Ok(binding);
    }
    if row.get::<_, String>(1) != "reconcile"
        || awr_team::request_hash(&payload).map_err(|_| PgError::EvidenceInvalid)?
            != row.get::<_, String>(2)
        || payload["outcome"] != "succeeded"
        || payload["effects_settled"] != true
        || payload["scope_violation"] != false
        || payload["input_digest"].as_str()
            != exec.get::<_, Option<String>>("input_digest").as_deref()
        || payload["output_digest"].as_str()
            != exec.get::<_, Option<String>>("result_digest").as_deref()
        || payload["environment_digest"].as_str()
            != exec
                .get::<_, Option<String>>("environment_digest")
                .as_deref()
        || payload["observed_paths"]
            != exec
                .get::<_, Option<Value>>("observed_paths_json")
                .unwrap_or(Value::Null)
        || payload["contract_hash"] != command.expected_contract_hash
        || payload["workstream_id"] != command.workstream_id.to_string()
        || payload["ownership_version"] != command.expected_ownership_version
        || payload["coordinator_epoch"] != auth.epoch
        || payload["execution_session_id"] != session
        || payload["execution_version"]
            .as_str()
            .and_then(|v| v.parse::<i64>().ok())
            .and_then(|v| v.checked_add(1))
            != Some(exec.get::<_, i64>("execution_version"))
    {
        return Err(PgError::EvidenceInvalid);
    }
    let caller_id = payload["reviewed_receipt_id"]
        .as_str()
        .ok_or(PgError::EvidenceInvalid)?;
    let caller = tx.query_opt(
        "SELECT receipt_kind,digest,payload_json,reporter_actor_id FROM awr_team.execution_receipts
         WHERE tenant_id=$1 AND project_id=$2 AND execution_id=$3 AND id=$4",
        &[&tenant,&project,&id,&caller_id]).await?.ok_or(PgError::EvidenceInvalid)?;
    let report: Value = caller.get(2);
    if caller.get::<_, String>(0) != "caller_asserted"
        || caller.get::<_, String>(3) != exec.get::<_, String>("executor_actor_id")
        || awr_team::request_hash(&report).map_err(|_| PgError::EvidenceInvalid)?
            != caller.get::<_, String>(1)
        || report["outcome"] != "succeeded"
        || report["scope_violation"] != false
        || report["output_digest"] != payload["output_digest"]
        || report["observed_paths"] != payload["observed_paths"]
        || report["contract_hash"] != payload["contract_hash"]
        || report["workstream_id"] != payload["workstream_id"]
        || report["ownership_version"] != payload["ownership_version"]
        || report["coordinator_epoch"].as_str()
            != exec
                .get::<_, Option<String>>("coordinator_epoch")
                .as_deref()
        || report["client_id"] != client
        || report["session_id"] != session
    {
        return Err(PgError::EvidenceInvalid);
    }
    Ok(
        json!({"caller_receipt_id":caller_id,"reconcile_receipt_id":receipt_id,
        "executor_client_id":client,"execution_basis":"caller_asserted_reconciled"}),
    )
}
