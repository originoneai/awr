//! Inspectability is a prerequisite for neutral review, never an acceptance proof.
use super::*;
use crate::workstream_auth::ReaderAuthority;
use serde_json::json;
use tokio_postgres::Transaction;

fn unavailable_source(reason: &str) -> Value {
    json!({"available":false,"reason":reason,"awr_checkout_verified":false,
        "note":"No source locator is disclosed. Use the current authorized delivery configuration; do not guess a remote, server path or tree."})
}

async fn source_navigation(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    candidate: &DeliveryCandidate,
    current: bool,
    inspectable: bool,
) -> PgResult<Value> {
    if !inspectable {
        return Ok(unavailable_source("submission_unavailable"));
    }
    if !current {
        return Ok(unavailable_source("candidate_stale"));
    }
    let Some(revision) = candidate.binding.source_revision.as_ref() else {
        return Ok(unavailable_source("source_revision_missing"));
    };
    if !matches!(
        revision.format,
        awr_team::delivery::RevisionFormat::GitSha1 | awr_team::delivery::RevisionFormat::GitSha256
    ) {
        return Ok(unavailable_source("source_revision_unsupported"));
    }
    let rows = tx
        .query(
            "SELECT id,version,enabled,coordinator_epoch,source_locator_contract,
                source_locator_url,source_locator_connector_version
         FROM awr_team.delivery_connectors
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND workstream_id=$4
           AND resource=$5 AND enabled AND coordinator_epoch=$6 ORDER BY id LIMIT 2",
            &[
                &tenant,
                &project,
                &candidate.binding.work_id.as_str(),
                &candidate.binding.workstream_id,
                &revision.resource,
                &auth.epoch,
            ],
        )
        .await?;
    if rows.is_empty() {
        let matching: bool = tx
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM awr_team.delivery_connectors
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND workstream_id=$4
                   AND resource=$5)",
                &[
                    &tenant,
                    &project,
                    &candidate.binding.work_id.as_str(),
                    &candidate.binding.workstream_id,
                    &revision.resource,
                ],
            )
            .await?
            .get(0);
        return Ok(unavailable_source(if matching {
            "connector_stale"
        } else {
            "connector_resource_mismatch"
        }));
    }
    if rows.len() != 1 {
        return Ok(unavailable_source("connector_ambiguous"));
    }
    let row = &rows[0];
    let version: i64 = row.get(1);
    let contract: Option<String> = row.get(4);
    let url: Option<String> = row.get(5);
    let locator_connector_version: Option<i64> = row.get(6);
    let (Some(contract), Some(url), Some(locator_connector_version)) =
        (contract, url, locator_connector_version)
    else {
        return Ok(unavailable_source("source_locator_not_configured"));
    };
    if locator_connector_version != version {
        return Ok(unavailable_source("source_locator_stale"));
    }
    if contract != super::connectors::GIT_SOURCE_CONTRACT
        || !super::connectors::safe_source_url(&url)
    {
        return Ok(unavailable_source("source_locator_unsupported"));
    }
    Ok(json!({"available":true,"contract":contract,"url":url,
        "connector_id":row.get::<_,String>(0),"connector_version":version.to_string(),
        "resource":revision.resource,"exact_revision":revision,
        "awr_checkout_verified":false,
        "note":"This credential-free locator names the candidate's exact Git revision. AWR has not fetched or verified a checkout, complete tree, advertised ref or server support for fetching an unadvertised SHA; verify the fetched object equals exact_revision."}))
}

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
            "the original report or manifest content is missing or changed",
            "the pinned report and every manifest entry must match their stored bytes and bindings",
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
            "note":"Publish the deliverable, select its exact candidate, and submit readable primary report and manifest evidence through delivery.evidence.submit with args.delivery_payload.delivery_candidate_digest. The report may stay in AWR; every manifest file still needs exact bytes. Refresh work.prepare and claim.inspect before selection; never inspect peer paths."},
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
    if expected_bundle.is_some() {
        result["source_navigation"] = unavailable_source("candidate_missing");
    }
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
    let contract: String = row.get(3);
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
        result["guidance"] = guidance(work, result["reason"].as_str().unwrap());
        return Ok(result);
    }
    let Some(original) = declared.and_then(Value::as_str).filter(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) else {
        let reason = if let Some((candidate, digest, source, owner, fence, _)) = &selected {
            let epoch_current: bool = tx.query_one(
                "SELECT EXISTS(SELECT 1 FROM awr_team.delivery_selections s
                 JOIN awr_team.claims c ON (c.tenant_id,c.project_id,c.id)=(s.tenant_id,s.project_id,s.claim_id)
                 WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.work_id=$3
                   AND c.coordinator_epoch=$4)",
                &[&tenant, &project, &work, &auth.epoch],
            ).await?.get(0);
            let binding = &candidate.binding;
            let binding_valid = binding.digest().map_err(|_| PgError::SourceDivergence)? == *digest
                && binding.tenant_id.as_str() == tenant
                && binding.project_id.as_str() == project
                && binding.scope_id.as_str() == "main"
                && binding.workstream_id == work_binding.workstream_id.to_string()
                && binding.work_id.as_str() == work;
            if !binding_valid {
                return Err(PgError::SourceDivergence);
            }
            if *owner != ownership || !*fence || !epoch_current {
                "submission_incomplete"
            } else {
                match completion::require_contracts(
                    tx,
                    tenant,
                    project,
                    work,
                    source,
                    &auth.snapshot,
                    &binding.contract_hash,
                )
                .await
                {
                    Ok(()) if binding.contract_hash == contract => {
                        "candidate_not_bound_in_evidence"
                    }
                    Ok(()) | Err(PgError::PreconditionsChanged) => "submission_incomplete",
                    Err(error) => return Err(error),
                }
            }
        } else {
            "candidate_missing"
        };
        result["reason"] = json!(reason);
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
    {
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
    if expected_bundle.is_some() {
        result["source_navigation"] = source_navigation(
            tx,
            tenant,
            project,
            auth,
            &candidate,
            current,
            result["inspectable"] == true,
        )
        .await?;
    }
    if result["inspectable"] == true && current {
        let first = &result["artifacts"][0];
        let query = first
            .get("segment_query")
            .unwrap_or(&first["query"])
            .clone();
        let revision_note = if result["source_revision"].is_null() {
            "Extra content lacks submission.source_revision: do not guess; until an authorized, verifiable binding shows immutability + candidate binding, version is unverified."
        } else {
            "Extra content: this round's submission.source_revision needs an authorized-channel immutability + candidate-binding check; else version unverified."
        };
        result["guidance"]["action"] = json!({"op":query["op"], "query":query,
            "note":format!("Read all manifest + report artifacts; not proof of complete source or code/verification, approval or integration. {revision_note} If segment_query exists, join contiguous pages; verify full length/sha256.")});
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
        // Diagnose the exact caller-selected evidence; never substitute or
        // silently select a different one. The label set mirrors inspect().
        let reason: &'static str = match state["reason"].as_str() {
            Some("candidate_missing") => "candidate_missing",
            Some("evidence_missing") => "evidence_missing",
            Some("candidate_not_bound_in_evidence") => "candidate_not_bound_in_evidence",
            Some("evidence_binding_changed") => "evidence_binding_changed",
            Some("original_candidate_unavailable") => "original_candidate_unavailable",
            Some("evidence_integrity_unavailable") => "evidence_integrity_unavailable",
            Some("manifest_content_unavailable") => "manifest_content_unavailable",
            Some("selection_changed") => "selection_changed",
            _ => "submission_incomplete",
        };
        return Err(PgError::review_submission_incomplete_reason(reason));
    }
    Ok(())
}
