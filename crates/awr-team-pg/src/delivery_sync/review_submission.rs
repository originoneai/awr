//! Inspectability is a prerequisite for neutral review, never an acceptance proof.
use super::*;
use crate::workstream_auth::ReaderAuthority;
use serde_json::json;
use tokio_postgres::Transaction;

pub(crate) async fn configured(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<bool> {
    // Disabled mappings retain the declared delivery chain instead of silently
    // allowing report-only approval. Connector credentials are not disclosed.
    Ok(tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.delivery_connectors
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3)",
            &[&tenant, &project, &work],
        )
        .await?
        .get(0))
}

fn guidance(work: &str, reason: &str) -> Value {
    let (when, basis) = match reason {
        "candidate_missing" => (
            "no neutral candidate is selected",
            "this delivery chain requires a version-bound candidate",
        ),
        "evidence_missing" => (
            "the candidate has no review evidence",
            "each manifest entry needs original readable evidence",
        ),
        "candidate_not_bound_in_evidence" => (
            "the evidence does not bind the selected candidate",
            "payload.delivery_candidate_digest is missing or invalid",
        ),
        "selection_changed" => (
            "the original submission is no longer current",
            "historical artifacts cannot approve a replacement selection",
        ),
        "manifest_content_unavailable" => (
            "some original manifest content is missing or changed",
            "every entry must match its stored bytes, digest and length",
        ),
        "inspectable_current_submission" => (
            "the current original submission is readable",
            "its candidate and stored manifest bytes match",
        ),
        _ => (
            "the original submission binding is unavailable",
            "candidate and evidence integrity must be established",
        ),
    };
    json!({"when":when,"because":[basis],
        "action":{"op":"delivery.neutral.inspect","work_id":work,
            "note":"The author must publish actual code and report through the authorized project channel, select its exact candidate, and submit readable evidence for every manifest entry with payload.delivery_candidate_digest. Refresh work.prepare and claim.inspect before selection; never inspect peer paths."},
        "recheck_on":"candidate, contract, artifact, scope or authority changes"})
}

/// Read the original evidence binding, not whichever candidate is selected today.
/// Currentness and content integrity are assessed separately for historical rounds.
pub(crate) async fn inspect(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    evidence_id: Option<&str>,
    expected_bundle: Option<&str>,
) -> PgResult<Value> {
    let (work_binding, ownership) =
        crate::workstream_read::work_binding(tx, tenant, project, auth, work).await?;
    let selected = snapshots::selection(tx, tenant, project, work).await?;
    let required = configured(tx, tenant, project, work).await? || selected.is_some();
    let row = if let Some(id) = evidence_id {
        tx.query_opt(
            "SELECT payload_json,artifact_id,output_digest,contract_hash,digest,
                    input_digest,execution_result_digest
             FROM awr_team.evidence WHERE tenant_id=$1 AND project_id=$2
               AND work_id=$3 AND id=$4",
            &[&tenant, &project, &work, &id],
        )
        .await?
    } else {
        None
    };
    let mut result = json!({"delivery_required":required,"inspectable":false,
        "candidate_current":false,"candidate_digest":null,"source_revision":null,
        "artifacts":[],"reason":"candidate_missing","approval_inferred":false,
        "repository_verification_inferred":false});
    let Some(row) = row else {
        result["reason"] = json!(if selected.is_some() {
            "evidence_missing"
        } else {
            "candidate_missing"
        });
        result["guidance"] = guidance(work, result["reason"].as_str().unwrap());
        return Ok(result);
    };
    let payload: Value = row.get(0);
    let declared = payload.get("delivery_candidate_digest");
    if declared.is_none() && !required {
        // Legacy report/artifact reviews remain on their existing domain gates.
        result["reason"] = json!("legacy_evidence_review");
        return Ok(result);
    }
    result["delivery_required"] = json!(true);
    if expected_bundle.is_some_and(|digest| digest != row.get::<_, String>(4)) {
        result["reason"] = json!("evidence_binding_changed");
        result["guidance"] = guidance(work, result["reason"].as_str().unwrap());
        return Ok(result);
    }
    let Some(original) = declared.and_then(Value::as_str).filter(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) else {
        result["reason"] = json!(if selected.is_some() {
            "candidate_not_bound_in_evidence"
        } else {
            "candidate_missing"
        });
        result["guidance"] = guidance(work, result["reason"].as_str().unwrap());
        return Ok(result);
    };
    let candidate_row = tx
        .query_opt(
            "SELECT body_json FROM awr_team.delivery_candidates
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND binding_digest=$4",
            &[&tenant, &project, &work, &original],
        )
        .await?;
    let Some(candidate_row) = candidate_row else {
        result["reason"] = json!("original_candidate_unavailable");
        result["guidance"] = guidance(work, result["reason"].as_str().unwrap());
        return Ok(result);
    };
    let candidate: DeliveryCandidate =
        serde_json::from_value(candidate_row.get(0)).map_err(|_| PgError::SourceDivergence)?;
    let contract: String = row.get(3);
    if candidate
        .binding
        .digest()
        .map_err(|_| PgError::SourceDivergence)?
        != original
        || candidate.binding.tenant_id.as_str() != tenant
        || candidate.binding.project_id.as_str() != project
        || candidate.binding.scope_id.as_str() != "main"
        || candidate.binding.workstream_id != work_binding.workstream_id.to_string()
        || candidate.binding.work_id.as_str() != work
        || candidate.binding.contract_hash != contract
    {
        return Err(PgError::SourceDivergence);
    }
    result["candidate_digest"] = json!(original);
    result["source_revision"] = json!(candidate.binding.source_revision);
    let current = if let Some((_, digest, source, owner, fence, _)) = &selected {
        let epoch_current: bool = tx.query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.delivery_selections s
             JOIN awr_team.claims c ON (c.tenant_id,c.project_id,c.id)=(s.tenant_id,s.project_id,s.claim_id)
             WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.work_id=$3
               AND c.coordinator_epoch=$4)",
            &[&tenant,&project,&work,&auth.epoch],
        ).await?.get(0);
        if digest == original && *owner == ownership && *fence && epoch_current {
            match completion::require_contracts(
                tx,
                tenant,
                project,
                work,
                source,
                &auth.snapshot,
                &contract,
            )
            .await
            {
                Ok(()) => true,
                Err(PgError::PreconditionsChanged) => false,
                Err(error) => return Err(error),
            }
        } else {
            false
        }
    } else {
        false
    };
    result["candidate_current"] = json!(current);
    let artifact: Option<String> = row.get(1);
    let output: Option<String> = row.get(2);
    let input: Option<String> = row.get(5);
    let execution_result: Option<String> = row.get(6);
    if crate::review::evidence_digest(
        work,
        &contract,
        input.as_deref(),
        output.as_deref(),
        execution_result.as_deref(),
        &payload,
    )? != row.get::<_, String>(4)
    {
        result["reason"] = json!("evidence_integrity_unavailable");
    } else {
        match completion::artifact_refs(
            tx,
            tenant,
            project,
            &candidate,
            original,
            completion::AcceptedEvidence {
                payload: &payload,
                artifact_id: artifact.as_deref(),
                output_digest: output.as_deref(),
            },
        )
        .await
        {
            Ok(references) => {
                result["artifacts"] = json!(references);
                result["inspectable"] = json!(true);
                result["reason"] = json!(if current {
                    "inspectable_current_submission"
                } else {
                    "selection_changed"
                });
            }
            Err(PgError::EvidenceInvalid) => {
                result["reason"] = json!("manifest_content_unavailable")
            }
            Err(error) => return Err(error),
        }
    }
    result["guidance"] = guidance(work, result["reason"].as_str().unwrap());
    if result["inspectable"] == true && current {
        result["guidance"]["action"] = json!({"op":"artifact.content",
            "query":result["artifacts"][0]["query"],
            "note":"Read every listed exact artifact query before an independent decision. Readability is not test verification, approval or repository integration."});
    }
    Ok(result)
}

pub(crate) async fn require(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    evidence_id: Option<&str>,
    expected_bundle: &str,
) -> PgResult<()> {
    let state = inspect(
        tx,
        tenant,
        project,
        auth,
        work,
        evidence_id,
        Some(expected_bundle),
    )
    .await?;
    if state["delivery_required"] == true
        && (state["inspectable"] != true || state["candidate_current"] != true)
    {
        return Err(PgError::review_submission_incomplete());
    }
    Ok(())
}
