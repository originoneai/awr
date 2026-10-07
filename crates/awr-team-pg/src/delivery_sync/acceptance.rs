//! Actual domain receipts schedule source synchronization without a provider event.
use super::*;
use crate::workstream_auth::ReaderAuthority;
use crate::workstream_command::WorkstreamCommand;
use awr_team::delivery::DeliveryRecord;
use serde_json::json;
use tokio_postgres::Transaction;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Origin {
    AdapterObservation,
    DomainAcceptance,
}

impl Origin {
    pub(super) fn parse(value: &str) -> PgResult<Self> {
        match value {
            "adapter_observation" => Ok(Self::AdapterObservation),
            "domain_acceptance" => Ok(Self::DomainAcceptance),
            _ => Err(PgError::SourceDivergence),
        }
    }
}

/// Called only after real authenticated finalization selected this new receipt.
/// The surrounding transaction owns command replay, review and acceptance gates.
pub(crate) async fn enqueue(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    receipt: &str,
    candidate: &str,
) -> PgResult<Value> {
    let set = DeliveryReadSet {
        work_id: command.work_id.clone(),
        workstream_id: command.workstream_id,
        coordinator_epoch: auth.epoch.clone(),
        source_snapshot_id: auth.snapshot.clone(),
        authority_version: command.expected_authority_version.clone(),
        ownership_version: command.expected_ownership_version.clone(),
        contract_hash: command.expected_contract_hash.clone(),
    };
    let (_, selected, _, _, _, selection) = snapshots::selection(tx, tenant, project, &set.work_id)
        .await?
        .ok_or(PgError::InactiveCandidate)?;
    if selected != candidate
        || !current(tx, tenant, project, &set, receipt, candidate, selection).await?
    {
        return Err(PgError::PreconditionsChanged);
    }
    let notification = crate::tx::new_id();
    tx.execute("INSERT INTO awr_team.delivery_notifications(tenant_id,project_id,id,work_id,origin,completion_receipt_id)
        VALUES($1,$2,$3,$4,'domain_acceptance',$5)",
        &[&tenant,&project,&notification,&set.work_id,&receipt]).await?;
    for kind in ["refresh", "source"] {
        tx.execute("INSERT INTO awr_team.delivery_sync_intents(tenant_id,project_id,id,notification_id,kind,
            work_id,candidate_digest,selection_version,read_set_json,origin,completion_receipt_id)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'domain_acceptance',$10)",
            &[&tenant,&project,&crate::tx::new_id(),&notification,&kind,&set.work_id,
                &candidate,&selection,&json!(set),&receipt]).await?;
    }
    Ok(
        json!({"origin":Origin::DomainAcceptance,"completion_receipt_id":receipt,
        "notification_id":notification,"state":"queued","source_synchronized":false}),
    )
}

/// A source intent is not an effect permit. Publication rechecks the actual
/// selected evidence, execution and artifact bytes before every source effect.
pub(super) async fn current(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    set: &DeliveryReadSet,
    receipt: &str,
    candidate_digest: &str,
    selection_version: i64,
) -> PgResult<bool> {
    let Some((candidate, selected, snapshot, ownership, fence, selection)) =
        snapshots::selection(tx, tenant, project, &set.work_id).await?
    else {
        return Ok(false);
    };
    DeliveryRecord::Candidate(candidate.clone())
        .validate()
        .map_err(|_| PgError::SourceDivergence)?;
    if selected != candidate_digest
        || selection != selection_version
        || candidate
            .binding
            .digest()
            .map_err(|_| PgError::SourceDivergence)?
            != selected
        || ownership != version(&set.ownership_version)?
        || !fence
        || auth::bind_candidate(tenant, project, set, &candidate.binding).is_err()
    {
        return Ok(false);
    }
    let valid: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.work_runtime w
        JOIN awr_team.completion_receipts c ON (c.tenant_id,c.project_id,c.id)=(w.tenant_id,w.project_id,w.selected_completion_id)
        WHERE w.tenant_id=$1 AND w.project_id=$2 AND w.scope_id='main' AND w.work_id=$3
            AND w.state='completed' AND NOT w.recovery_blocked AND c.id=$4
            AND c.scope_id='main' AND c.work_id=$3 AND c.contract_hash=$5 AND c.delivery_candidate_digest=$6)",
        &[&tenant,&project,&set.work_id,&receipt,&set.contract_hash,&selected]).await?.get(0);
    if !valid {
        return Ok(false);
    }
    match completion::require_contracts(
        tx,
        tenant,
        project,
        &set.work_id,
        &snapshot,
        &set.source_snapshot_id,
        &set.contract_hash,
    )
    .await
    {
        Ok(()) => Ok(true),
        Err(PgError::PreconditionsChanged) => Ok(false),
        Err(error) => Err(error),
    }
}
