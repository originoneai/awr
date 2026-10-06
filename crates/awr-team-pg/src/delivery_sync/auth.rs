use super::*;
use crate::workstream_auth::{
    CommandAuthPhase, ReaderAuthority, authenticate_writer, authorize_command,
};
use awr_core::WorkstreamAction;
use awr_team::{Action, delivery::CandidateBinding};
use serde_json::json;
use tokio_postgres::Transaction;

pub(super) async fn admit(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    bearer: &str,
    set: &DeliveryReadSet,
    action: Action,
    session: Option<&str>,
) -> PgResult<ReaderAuthority> {
    bounded(set)?;
    identity(&set.work_id)?;
    identity(&set.coordinator_epoch)?;
    identity(&set.source_snapshot_id)?;
    digest(&set.contract_hash)?;
    let mut auth = authenticate_writer(tx, tenant, project, bearer).await?;
    if action == Action::AccessManageProject {
        if !matches!(auth.actor_kind.as_str(), "human" | "system") {
            return Err(PgError::Forbidden);
        }
        crate::delegation_auth::authorize_project_action(tx, &mut auth, project, action).await?;
        auth.access
            .authorize(&auth.catalog, set.workstream_id, WorkstreamAction::Manage)?;
    } else {
        crate::delegation_auth::resolve_agent_delegation(
            tx,
            &mut auth,
            project,
            Some(&set.work_id),
            session,
            Some(action),
            now_ms(),
        )
        .await?;
        authorize_command(
            &auth,
            set.workstream_id,
            &set.work_id,
            "delivery.register_pr",
            CommandAuthPhase::Effect,
        )?;
    }
    let (binding, ownership) =
        crate::workstream_read::work_binding(tx, tenant, project, &auth, &set.work_id).await?;
    if binding.workstream_id != set.workstream_id {
        return Err(PgError::Forbidden);
    }
    if auth.project_status != "active" {
        return Err(PgError::ProjectNotAvailable);
    }
    if auth.epoch != set.coordinator_epoch {
        return Err(PgError::EpochChanged);
    }
    if auth.snapshot != set.source_snapshot_id
        || ownership != version(&set.ownership_version)?
        || auth.catalog.get(set.workstream_id)?.authority_version
            != version(&set.authority_version)? as u64
    {
        return Err(PgError::PreconditionsChanged);
    }
    let row = tx
        .query_opt(
            "SELECT contract_json,contract_hash FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &set.work_id],
        )
        .await?
        .ok_or(PgError::SourceDivergence)?;
    let contract: awr_team::WorkContract =
        serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
    let actual: String = row.get(1);
    if contract.work_id.as_str() != set.work_id
        || contract.hash().map_err(|_| PgError::SourceDivergence)? != actual
    {
        return Err(PgError::SourceDivergence);
    }
    if actual != set.contract_hash {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(auth)
}

pub(super) fn bind_candidate(
    tenant: &str,
    project: &str,
    set: &DeliveryReadSet,
    binding: &CandidateBinding,
) -> PgResult<()> {
    binding.validate().map_err(|_| invalid())?;
    if binding.tenant_id.as_str() != tenant
        || binding.project_id.as_str() != project
        || binding.scope_id.as_str() != "main"
        || binding.workstream_id != set.workstream_id.to_string()
        || binding.work_id.as_str() != set.work_id
        || binding.contract_hash != set.contract_hash
    {
        return Err(PgError::BindingInvalid);
    }
    Ok(())
}

pub(super) fn authority_binding(auth: &ReaderAuthority, set: &DeliveryReadSet) -> PgResult<String> {
    hash(&json!({"subject":auth.binding,"grants":auth.grant_versions,
        "delegation":auth.delegation_id,"actions":auth.delegated_actions,
        "authority_version":set.authority_version}))
}

pub(super) async fn require_executor(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    request: &SelectDeliveryCandidate,
) -> PgResult<()> {
    for value in [&request.session_id, &request.claim_id] {
        identity(value)?;
    }
    version(&request.fence)?;
    version(&request.lease_version)?;
    crate::responsibility::lock_transition_keys(
        tx,
        tenant,
        project,
        &request.read_set.work_id,
        &[],
    )
    .await?;
    crate::lock_works_sorted(
        tx,
        tenant,
        project,
        "main",
        &[request.read_set.work_id.clone()],
    )
    .await?;
    crate::workstream_command::task_intake::require_enabled(
        tx,
        tenant,
        project,
        auth,
        &request.read_set.work_id,
    )
    .await?;
    let set = &request.read_set;
    let command = crate::WorkstreamCommand {
        protocol_version: 1,
        request_id: request.request_id.clone(),
        op: "delivery.register_pr".into(),
        work_id: set.work_id.clone(),
        workstream_id: set.workstream_id,
        coordinator_epoch: set.coordinator_epoch.clone(),
        expected_project_revision: auth.revision.to_string(),
        expected_authority_version: set.authority_version.clone(),
        expected_ownership_version: set.ownership_version.clone(),
        expected_contract_hash: set.contract_hash.clone(),
        args: json!({}),
    };
    crate::workstream_command::task_intake::require_executor(tx, tenant, project, auth, &command)
        .await?;
    let actual =
        crate::workstream_command::task_intake::actor_instance(tx, tenant, project, auth).await?;
    let responsibility = crate::responsibility::current(tx, tenant, project, &set.work_id).await?;
    if responsibility.current_executor.as_ref() != Some(&actual) {
        return Err(PgError::ClaimHeld);
    }
    let claim = crate::workstream_command::claims::inspect(
        tx,
        tenant,
        project,
        auth,
        &set.work_id,
        &set.workstream_id.to_string(),
        version(&set.ownership_version)?,
        &request.claim_id,
        Some(&request.session_id),
    )
    .await?;
    if claim["owned_by_client"] != true {
        return Err(PgError::Forbidden);
    }
    if claim["fence"] != request.fence {
        return Err(PgError::StaleFence);
    }
    if claim["lease_version"] != request.lease_version {
        return Err(PgError::PreconditionsChanged);
    }
    if claim["lease_live"] != true {
        return Err(PgError::LeaseExpired);
    }
    Ok(())
}

pub(super) async fn replay<T: Serialize>(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    op: &str,
    request_id: &str,
    request: &T,
) -> PgResult<(String, Option<Value>)> {
    identity(request_id)?;
    bounded(request)?;
    let request_hash = hash(
        &json!({"protocol":"awr-delivery-sync-v1","op":op,"request":request,
        "tenant":tenant,"project":project,"actor":auth.actor_id,"client":auth.client_id}),
    )?;
    let row = tx
        .query_opt(
            "SELECT request_hash,result_json FROM awr_team.delivery_sync_requests
        WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND request_id=$5",
            &[
                &tenant,
                &project,
                &auth.actor_id,
                &auth.client_id,
                &request_id,
            ],
        )
        .await?;
    if let Some(row) = row {
        if row.get::<_, String>(0) != request_hash {
            return Err(PgError::IdempotencyConflict);
        }
        let mut result: Value = row.get(1);
        result["replayed"] = json!(true);
        return Ok((request_hash, Some(result)));
    }
    Ok((request_hash, None))
}

pub(super) async fn finish(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    set: &DeliveryReadSet,
    op: &str,
    request_id: &str,
    request_hash: &str,
    data: Value,
) -> PgResult<Value> {
    let next = auth
        .revision
        .checked_add(1)
        .ok_or(PgError::PreconditionsChanged)?;
    tx.execute(
        "UPDATE awr_team.projects SET project_revision=$3 WHERE tenant_id=$1 AND id=$2",
        &[&tenant, &project, &next],
    )
    .await?;
    let result = json!({"protocol":"awr-delivery-sync-v1","request_id":request_id,"op":op,
        "replayed":false,"committed_project_revision":next.to_string(),"source_snapshot_id":auth.snapshot,
        "work_id":set.work_id,"workstream_id":set.workstream_id,"data":data,
        "state_basis":"at_commit","acceptance_ready":false,"execution_authorized":false,"source_synchronized":false});
    tx.execute("INSERT INTO awr_team.delivery_sync_requests(tenant_id,project_id,actor_id,client_id,request_id,request_hash,result_json)
        VALUES($1,$2,$3,$4,$5,$6,$7)",&[&tenant,&project,&auth.actor_id,&auth.client_id,&request_id,&request_hash,&result]).await?;
    tx.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json,workstream_id)
        VALUES($1,$2,$3,$4,0,$5,$6,$7,$8,$9)",
        &[&tenant,&project,&crate::tx::new_id(),&next,&op,&auth.actor_id,&set.work_id,&data,&set.workstream_id.to_string()]).await?;
    let mut audit =
        crate::ops_audit::write_from_auth(auth, crate::OpsCategory::Delivery, op, "delivery");
    audit.request_id = Some(request_id.into());
    audit.work_id = Some(set.work_id.clone());
    audit.authority_version = Some(version(&set.authority_version)?);
    audit.target_id = data
        .get("candidate_digest")
        .or_else(|| data.get("inspection_id"))
        .or_else(|| data.get("connector_id"))
        .or_else(|| {
            data.get("observation_receipt")
                .and_then(|r| r.get("receipt_id"))
        })
        .and_then(Value::as_str)
        .map(str::to_owned);
    audit.digest = Some(crate::digest_of(&data));
    audit.summary = data;
    crate::record_in_tx(tx, tenant, project, &audit).await?;
    Ok(result)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
