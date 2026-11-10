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

/// The legacy completion path: a work that delivers into a repository waits for the confirmed
/// integration of its selected candidate, as on the command path; any other work completes as
/// before.
pub(crate) async fn require_confirmed_integration_of_selection(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    policy: &str,
) -> PgResult<()> {
    if !DeliverySyncStore::delivers_into_repository_in_tx(tx, tenant, project, work, policy).await?
    {
        return Ok(());
    }
    DeliverySyncStore::require_confirmed_integration_in_tx(tx, tenant, project, work).await
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

/// Whether the delivery a work recorded under `recorded` is still the work's delivery under the
/// `current` project snapshot. Every activation creates a new snapshot of the whole planned
/// source, so the snapshot ids alone differ after any later plan. A work that is completed with
/// a selected completion receipt and whose contract hash is the same in both snapshots has lost
/// nothing: its integration, review and source note stay current. A work still in flight has to
/// refresh its selection, and a changed contract makes the delivery stale, as before. Reads only;
/// the write paths keep their exact snapshot checks.
///
/// The test is lighter than `acceptance::current` on purpose: it asks only for the completed state,
/// a selected completion and an unchanged contract hash. Every reader also compares the candidate
/// with the current contract itself (and an activation that changes a contract reopens the
/// completed work), so the hash term here is defence in depth. The inline SQL of
/// `integration_summary_in_tx` repeats this test and has to change together with this function.
pub(crate) async fn carried_across_snapshots(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    recorded: &str,
    current: &str,
) -> PgResult<bool> {
    if recorded == current {
        return Ok(true);
    }
    Ok(tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.work_runtime w
               WHERE w.tenant_id=$1 AND w.project_id=$2 AND w.scope_id='main' AND w.work_id=$3
                 AND w.state='completed' AND w.selected_completion_id IS NOT NULL)
             AND EXISTS(SELECT 1 FROM awr_team.work_contracts a JOIN awr_team.work_contracts b
               ON (b.tenant_id,b.project_id,b.scope_id,b.work_id,b.contract_hash)
                 =(a.tenant_id,a.project_id,a.scope_id,a.work_id,a.contract_hash)
               WHERE a.tenant_id=$1 AND a.project_id=$2 AND a.scope_id='main' AND a.work_id=$3
                 AND a.snapshot_id=$4 AND b.snapshot_id=$5)",
            &[&tenant, &project, &work, &recorded, &current],
        )
        .await?
        .get(0))
}

/// Use only evidence that originally names this exact candidate. The primary
/// stored artifact is pinned independently of the repository manifest.
pub(super) async fn verify_artifacts(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    candidate: &DeliveryCandidate,
    selected: &str,
    evidence: AcceptedEvidence<'_>,
) -> PgResult<()> {
    artifact_refs(tx, tenant, project, candidate, selected, evidence)
        .await
        .map(|_| ())
}

/// The same exact-byte checks serve completion and review content discovery.
/// References grant no read authority; artifact.content authenticates each read.
pub(crate) async fn artifact_refs(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    candidate: &DeliveryCandidate,
    selected: &str,
    evidence: AcceptedEvidence<'_>,
) -> PgResult<Vec<Value>> {
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
    {
        return Err(PgError::EvidenceInvalid);
    }
    let primary = evidence.artifact_id.ok_or(PgError::EvidenceInvalid)?;
    let primary_digest = evidence.output_digest.ok_or(PgError::EvidenceInvalid)?;
    let binding = &candidate.binding;
    struct ArtifactTarget<'a> {
        manifest_id: Option<&'a str>,
        sha256: &'a str,
        byte_length: Option<&'a str>,
        pinned: Option<&'a str>,
    }
    let mut targets = Vec::with_capacity(candidate.manifest.entries.len() + 1);
    // Execution reports may live only in AWR. They still need their exact stored
    // bytes, original evidence binding and ID; an equal report cannot replace one.
    if !candidate
        .manifest
        .entries
        .iter()
        .any(|e| e.sha256 == primary_digest)
    {
        targets.push(ArtifactTarget {
            manifest_id: None,
            sha256: primary_digest,
            byte_length: None,
            pinned: Some(primary),
        });
    }
    for entry in &candidate.manifest.entries {
        targets.push(ArtifactTarget {
            manifest_id: Some(&entry.artifact_id),
            sha256: &entry.sha256,
            byte_length: Some(&entry.byte_length),
            pinned: (entry.sha256 == primary_digest).then_some(primary),
        });
    }
    let mut references = Vec::with_capacity(targets.len());
    for target in targets {
        let row = tx
            .query_opt(
                "SELECT a.sha256,a.byte_length,a.state,a.content,e.digest,e.input_digest,
                     e.output_digest,e.execution_result_digest,e.payload_json,a.id
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
                    &target.sha256,
                    &target.pinned,
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
            || bytes.len() > crate::WorkstreamQuery::MAX_ARTIFACT_BYTES
            || row.get::<_, String>(0) != target.sha256
            || crate::source::sha256_hex(&bytes) != target.sha256
            || row.get::<_, i64>(1) != bytes.len() as i64
            || target
                .byte_length
                .is_some_and(|length| length != bytes.len().to_string())
            || output.as_deref() != Some(target.sha256)
            || (target.pinned.is_some() && payload != *evidence.payload)
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
        let mut query = serde_json::json!({"protocol_version":1,"op":"artifact.content",
            "work_id":binding.work_id,"artifact_id":row.get::<_,String>(9),
            "expected_sha256":target.sha256});
        if bytes.len() > 65_536 {
            query["max_context_bytes"] = serde_json::json!(bytes.len());
        }
        let mut reference = serde_json::json!({
            "manifest_artifact_id": target.manifest_id,
            "primary_evidence": target.pinned.is_some(),
            "query": query,
            "byte_length":bytes.len().to_string()
        });
        if bytes.len() > 2048 {
            let mut segment = reference["query"].clone();
            segment["op"] = serde_json::json!("artifact.segment");
            segment.as_object_mut().unwrap().remove("max_context_bytes");
            reference["segment_query"] = segment;
        }
        references.push(reference);
    }
    Ok(references)
}
