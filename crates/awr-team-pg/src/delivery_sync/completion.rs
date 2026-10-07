//! Bind new acceptance to the candidate declared in the original evidence.
//! A logical manifest name and an allocated evidence artifact ID are distinct.
use super::*;
use crate::workstream_auth::ReaderAuthority;
use crate::workstream_command::WorkstreamCommand;
use awr_team::{WorkContract, delivery::DeliveryRecord};
use std::collections::BTreeSet;
use tokio_postgres::Transaction;

pub(crate) struct AcceptedEvidence<'a> {
    pub payload: &'a Value,
    pub artifact_id: Option<&'a str>,
    pub output_digest: Option<&'a str>,
}

/// The command transaction has already checked authority, execution and review.
/// Do not reconstruct an old receipt's binding from today's selected candidate.
pub(crate) async fn bind_new_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    contract: &WorkContract,
    evidence: AcceptedEvidence<'_>,
) -> PgResult<Option<String>> {
    let Some((candidate, selected, snapshot, ownership, fence_current, _)) =
        snapshots::selection(tx, tenant, project, &command.work_id).await?
    else {
        if evidence.payload.get("delivery_candidate_digest").is_some() {
            return Err(PgError::EvidenceInvalid);
        }
        return Ok(None);
    };
    let set = DeliveryReadSet {
        work_id: command.work_id.clone(),
        workstream_id: command.workstream_id,
        coordinator_epoch: auth.epoch.clone(),
        source_snapshot_id: auth.snapshot.clone(),
        authority_version: command.expected_authority_version.clone(),
        ownership_version: command.expected_ownership_version.clone(),
        contract_hash: command.expected_contract_hash.clone(),
    };
    auth::bind_candidate(tenant, project, &set, &candidate.binding)?;
    DeliveryRecord::Candidate(candidate.clone())
        .validate()
        .map_err(|_| PgError::BindingInvalid)?;
    if candidate
        .binding
        .digest()
        .map_err(|_| PgError::BindingInvalid)?
        != selected
        || ownership != version(&command.expected_ownership_version)?
        || !fence_current
        || contract.hash().map_err(|_| PgError::SourceDivergence)? != set.contract_hash
        || contract
            .verification_requirements
            .iter()
            .collect::<BTreeSet<_>>()
            != candidate.binding.required_checks.iter().collect()
    {
        return Err(PgError::PreconditionsChanged);
    }
    require_contracts(
        tx,
        tenant,
        project,
        &command.work_id,
        &snapshot,
        &auth.snapshot,
        &set.contract_hash,
    )
    .await?;
    verify_artifacts(tx, tenant, project, &candidate, &selected, evidence).await?;
    Ok(Some(selected))
}

/// Recompute both exact definitions; an old source identifier alone is no proof.
pub(super) async fn require_contracts(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    original: &str,
    current: &str,
    contract_hash: &str,
) -> PgResult<()> {
    let definitions = tx
        .query_opt(
            "SELECT original.contract_json,current.contract_json
         FROM awr_team.work_contracts original JOIN awr_team.work_contracts current
           ON current.tenant_id=original.tenant_id AND current.project_id=original.project_id
           AND current.scope_id=original.scope_id AND current.work_id=original.work_id
           AND current.contract_hash=original.contract_hash
         WHERE original.tenant_id=$1 AND original.project_id=$2 AND original.scope_id='main'
           AND original.work_id=$3 AND original.snapshot_id=$4 AND current.snapshot_id=$5
           AND original.contract_hash=$6",
            &[
                &tenant,
                &project,
                &work,
                &original,
                &current,
                &contract_hash,
            ],
        )
        .await?
        .ok_or(PgError::PreconditionsChanged)?;
    for index in [0, 1] {
        let definition: WorkContract = serde_json::from_value(definitions.get(index))
            .map_err(|_| PgError::SourceDivergence)?;
        if definition.work_id.as_str() != work
            || definition.hash().map_err(|_| PgError::SourceDivergence)? != contract_hash
        {
            return Err(PgError::PreconditionsChanged);
        }
    }
    Ok(())
}

/// Use only evidence that originally names this exact candidate. The primary
/// stored artifact is pinned; additional entries need their own bound evidence.
pub(super) async fn verify_artifacts(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    candidate: &DeliveryCandidate,
    selected: &str,
    evidence: AcceptedEvidence<'_>,
) -> PgResult<()> {
    DeliveryRecord::Candidate(candidate.clone())
        .validate()
        .map_err(|_| PgError::EvidenceInvalid)?;
    if candidate
        .binding
        .digest()
        .map_err(|_| PgError::EvidenceInvalid)?
        != selected
    {
        return Err(PgError::EvidenceInvalid);
    }
    if evidence.payload["delivery_candidate_digest"].as_str() != Some(selected)
        || evidence.payload["passed"] == false
        || !candidate
            .manifest
            .entries
            .iter()
            .any(|entry| Some(entry.sha256.as_str()) == evidence.output_digest)
    {
        return Err(PgError::EvidenceInvalid);
    }
    let primary = evidence.artifact_id.ok_or(PgError::EvidenceInvalid)?;
    let binding = &candidate.binding;
    for entry in &candidate.manifest.entries {
        let pinned = (Some(entry.sha256.as_str()) == evidence.output_digest).then_some(primary);
        let row = tx
            .query_opt(
                "SELECT a.sha256,a.byte_length,a.state,a.content,e.digest,e.input_digest,
                    e.output_digest,e.execution_result_digest,e.payload_json
             FROM awr_team.artifacts a JOIN awr_team.evidence e
               ON e.tenant_id=a.tenant_id AND e.project_id=a.project_id AND e.artifact_id=a.id
             WHERE a.tenant_id=$1 AND a.project_id=$2 AND e.work_id=$3 AND e.contract_hash=$4
               AND a.sha256=$5 AND ($6::text IS NULL OR a.id=$6)
               AND e.payload_json->>'delivery_candidate_digest'=$7
             ORDER BY a.id,e.id LIMIT 1",
                &[
                    &tenant,
                    &project,
                    &binding.work_id.as_str(),
                    &binding.contract_hash,
                    &entry.sha256,
                    &pinned,
                    &selected,
                ],
            )
            .await?
            .ok_or(PgError::EvidenceInvalid)?;
        let bytes: Vec<u8> = row
            .get::<_, Option<Vec<u8>>>(3)
            .ok_or(PgError::EvidenceInvalid)?;
        let input: Option<String> = row.get(5);
        let output: Option<String> = row.get(6);
        let result: Option<String> = row.get(7);
        let payload: Value = row.get(8);
        if row.get::<_, String>(2) != "finalized"
            || row.get::<_, String>(0) != entry.sha256
            || crate::source::sha256_hex(&bytes) != entry.sha256
            || row.get::<_, i64>(1) != bytes.len() as i64
            || entry.byte_length != bytes.len().to_string()
            || output.as_deref() != Some(entry.sha256.as_str())
            || payload["passed"] == false
            || crate::review::evidence_digest(
                binding.work_id.as_str(),
                &binding.contract_hash,
                input.as_deref(),
                output.as_deref(),
                result.as_deref(),
                &payload,
            )? != row.get::<_, String>(4)
        {
            return Err(PgError::EvidenceInvalid);
        }
    }
    Ok(())
}
