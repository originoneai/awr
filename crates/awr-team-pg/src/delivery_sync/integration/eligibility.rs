//! Resolve original AWR records and authenticated observations, never caller flags.
use super::*;
use std::collections::BTreeSet;

pub(super) async fn resolve(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    request: &PrepareDeliveryIntegration,
) -> PgResult<Eligibility> {
    if request.operation != IntegrationOperation::FastForward {
        return Err(PgError::Unsupported(
            "only fast-forward integration is supported".into(),
        ));
    }
    let set = &request.read_set;
    crate::workstream_command::task_intake::require_admissible(
        tx,
        tenant,
        project,
        auth,
        &command(auth, set, &request.request_id),
    )
    .await?;
    let (candidate, selected, snapshot, ownership, fence_current, selection) =
        snapshots::selection(tx, tenant, project, &set.work_id)
            .await?
            .ok_or(PgError::InactiveCandidate)?;
    auth::bind_candidate(tenant, project, set, &candidate.binding)?;
    if selected != request.candidate_digest
        || candidate.binding.digest().map_err(|_| invalid())? != selected
        || snapshot != auth.snapshot
        || ownership != version(&set.ownership_version)?
        || !fence_current
        || selection != version(&request.selection_version)?
    {
        return Err(PgError::PreconditionsChanged);
    }
    DeliveryRecord::Candidate(candidate.clone())
        .validate()
        .map_err(|_| PgError::BindingInvalid)?;
    let connector = tx.query_opt(
        "SELECT c.version,c.inspection_generation,c.resource,c.fact_source,c.enabled,c.coordinator_epoch,
                c.principal_actor_id,c.principal_client_id,a.kind,c.work_id,c.workstream_id
         FROM awr_team.delivery_connectors c JOIN awr_team.actors a ON a.tenant_id=c.tenant_id AND a.id=c.principal_actor_id
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.id=$3 FOR SHARE OF c,a",
        &[&tenant,&project,&request.connector_id],
    ).await?.ok_or(PgError::Forbidden)?;
    if connector.get::<_, i64>(0) != version(&request.connector_version)?
        || connector.get::<_, String>(2) != candidate.binding.target.resource
        || candidate
            .binding
            .source_revision
            .as_ref()
            .is_none_or(|r| r.resource != candidate.binding.target.resource)
        || connector.get::<_, String>(3) != "adapter_observation"
        || !connector.get::<_, bool>(4)
        || connector.get::<_, String>(5) != auth.epoch
        || connector.get::<_, String>(8) != "system"
        || connector.get::<_, String>(9) != set.work_id
        || connector.get::<_, String>(10) != set.workstream_id.to_string()
    {
        return Err(PgError::PreconditionsChanged);
    }
    let value: Value = tx.query_one(
        "SELECT contract_json FROM awr_team.work_contracts WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
        &[&tenant,&project,&auth.snapshot,&set.work_id],
    ).await?.get(0);
    let contract: awr_team::WorkContract =
        serde_json::from_value(value).map_err(|_| PgError::SourceDivergence)?;
    if contract.hash().map_err(|_| PgError::SourceDivergence)? != set.contract_hash
        || contract
            .verification_requirements
            .iter()
            .collect::<BTreeSet<_>>()
            != candidate.binding.required_checks.iter().collect()
    {
        return Err(PgError::BindingInvalid);
    }
    let ev = tx.query_opt(
        "SELECT work_id,contract_hash,digest,trust_basis,payload_json,artifact_id,output_digest,
                execution_result_digest,input_digest,execution_id,created_by
         FROM awr_team.evidence WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant,&project,&request.evidence_id],
    ).await?.ok_or(PgError::EvidenceInvalid)?;
    let payload: Value = ev.get("payload_json");
    let input: Option<String> = ev.get("input_digest");
    let output: Option<String> = ev.get("output_digest");
    let result: Option<String> = ev.get("execution_result_digest");
    let execution_id: Option<String> = ev.get("execution_id");
    let evidence_digest: String = ev.get("digest");
    let artifact_id: String = ev
        .get::<_, Option<String>>("artifact_id")
        .ok_or(PgError::EvidenceInvalid)?;
    if ev.get::<_, String>("work_id") != set.work_id
        || ev.get::<_, String>("contract_hash") != set.contract_hash
        || payload["delivery_candidate_digest"] != selected
        || payload["passed"] == false
        || crate::review::evidence_digest(
            &set.work_id,
            &set.contract_hash,
            input.as_deref(),
            output.as_deref(),
            result.as_deref(),
            &payload,
        )? != evidence_digest
        || !candidate
            .manifest
            .entries
            .iter()
            .any(|a| Some(a.sha256.as_str()) == output.as_deref())
    {
        return Err(PgError::EvidenceInvalid);
    }
    let mut artifacts = Vec::new();
    for entry in &candidate.manifest.entries {
        // Manifest IDs are logical delivery names; evidence.submit allocates its
        // own artifact ID. Bind those independent identities through actual bytes.
        let pinned =
            (Some(entry.sha256.as_str()) == output.as_deref()).then_some(artifact_id.as_str());
        let artifact = tx.query_opt(
            "SELECT a.sha256,a.byte_length,a.state,a.content,a.id FROM awr_team.artifacts a
             JOIN awr_team.evidence e ON e.tenant_id=a.tenant_id AND e.project_id=a.project_id AND e.artifact_id=a.id
             WHERE a.tenant_id=$1 AND a.project_id=$2 AND e.work_id=$3 AND a.sha256=$4 AND ($5::text IS NULL OR a.id=$5)
               AND e.contract_hash=$6 ORDER BY a.id LIMIT 1",
            &[&tenant,&project,&set.work_id,&entry.sha256,&pinned,&set.contract_hash],
        ).await?.ok_or(PgError::EvidenceInvalid)?;
        let bytes: Vec<u8> = artifact
            .get::<_, Option<Vec<u8>>>(3)
            .ok_or(PgError::EvidenceInvalid)?;
        if artifact.get::<_, String>(2) != "finalized"
            || artifact.get::<_, String>(0) != entry.sha256
            || crate::source::sha256_hex(&bytes) != entry.sha256
            || artifact.get::<_, i64>(1) != bytes.len() as i64
            || entry.byte_length != bytes.len().to_string()
        {
            return Err(PgError::EvidenceInvalid);
        }
        artifacts.push(json!({"manifest_artifact_id":entry.artifact_id,"stored_artifact_id":artifact.get::<_,String>(4),
            "sha256":entry.sha256,"byte_length":entry.byte_length}));
    }
    let policy = contract.completion_policy.as_str();
    let agent_policy = policy == crate::review::AGENT_REVIEW_POLICY;
    // Integration always requires an explicit approved round. Ordinary human
    // confirmation remains available through its existing completion workflow.
    if !agent_policy
        && !matches!(
            policy,
            "review"
                | "trusted_execution_and_review"
                | "trusted_execution_and_author_self_review"
                | "independent_review"
        )
    {
        return Err(PgError::ReviewRequired);
    }
    if ev.get::<_, String>("trust_basis")
        != if agent_policy {
            "caller_asserted"
        } else {
            "trusted_executor"
        }
    {
        return Err(PgError::EvidenceInvalid);
    }
    let execution_id = execution_id.ok_or(PgError::EvidenceInvalid)?;
    let exec = tx.query_opt(
        "SELECT state,contract_hash,input_digest,executor_actor_id,scope_id,result_digest,
                id,work_id,executor_client_id,session_id,workstream_id,ownership_version,environment_digest,
                observed_paths_json,coordinator_epoch,execution_version,claim_id,fence,declared_scope_json,
                settlement_policy_json,admission_mode,admission_lease_version,terminal_reported,workspace_effects_settled
         FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant,&project,&execution_id],
    ).await?.ok_or(PgError::EvidenceInvalid)?;
    if exec.get::<_, String>("state") != "succeeded"
        || exec.get::<_, String>("contract_hash") != set.contract_hash
        || exec.get::<_, String>("scope_id") != "main"
        || exec.get::<_, String>("work_id") != set.work_id
        || exec.get::<_, String>("executor_actor_id") != ev.get::<_, String>("created_by")
        || input.is_none()
        || exec.get::<_, Option<String>>("input_digest") != input
        || result.is_none()
        || exec.get::<_, Option<String>>("result_digest") != result
    {
        return Err(PgError::EvidenceInvalid);
    }
    let settlement = if agent_policy {
        crate::workstream_command::reviews::verify_integration_execution(
            tx,
            tenant,
            project,
            auth,
            &command(auth, set, &request.request_id),
            &exec,
            output.as_deref(),
        )
        .await?
    } else {
        Value::Null
    };
    let round = tx.query_opt(
        "SELECT id,work_id,contract_hash,state,bundle_hash,evidence_id,execution_id,artifact_digest,
                execution_result_digest,author_actor_id,author_client_id,author_person_id
         FROM awr_team.review_rounds WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND bundle_hash=$4 ORDER BY round_index DESC LIMIT 1",
        &[&tenant,&project,&set.work_id,&evidence_digest],
    ).await?.ok_or(PgError::ReviewRequired)?;
    if round.get::<_, String>("id") != request.review_round_id
        || round.get::<_, String>("state") != "approved"
        || round.get::<_, String>("contract_hash") != set.contract_hash
        || round.get::<_, Option<String>>("evidence_id").as_deref()
            != Some(request.evidence_id.as_str())
        || round.get::<_, Option<String>>("execution_id").as_deref() != Some(execution_id.as_str())
        || round.get::<_, Option<String>>("artifact_digest") != output
        || round.get::<_, Option<String>>("execution_result_digest") != result
    {
        return Err(PgError::ReviewRequired);
    }
    let decision = tx.query_opt(
        "SELECT id,work_id,bundle_hash,decision,reviewer_actor_id,reviewer_person_id,independence_kind,reviewer_client_id,approval_basis
         FROM awr_team.review_decisions WHERE tenant_id=$1 AND project_id=$2 AND review_round_id=$3 ORDER BY created_at DESC,id DESC LIMIT 1",
        &[&tenant,&project,&request.review_round_id],
    ).await?.ok_or(PgError::ReviewRequired)?;
    let reviewer: String = decision.get("reviewer_actor_id");
    let reviewer_client: Option<String> = decision.get("reviewer_client_id");
    let independence: String = decision.get("independence_kind");
    let basis: String = decision.get("approval_basis");
    if decision.get::<_, String>("id") != request.review_decision_id
        || decision.get::<_, String>("decision") != "approve"
        || decision.get::<_, String>("work_id") != set.work_id
        || decision.get::<_, String>("bundle_hash") != evidence_digest
        || if agent_policy {
            independence != "agent_review"
                || basis != "agent_review"
                || reviewer == ev.get::<_, String>("created_by")
                || reviewer == round.get::<_, String>("author_actor_id")
                || reviewer_client.as_deref().is_none_or(|c| {
                    Some(c)
                        == round
                            .get::<_, Option<String>>("author_client_id")
                            .as_deref()
                        || Some(c) == settlement["executor_client_id"].as_str()
                })
                || round.get::<_, Option<String>>("author_client_id").is_none()
        } else {
            !matches!(
                independence.as_str(),
                "team_independent" | "personal_self_review"
            ) || basis != crate::review::approval_basis(&independence)
                || independence == "personal_self_review"
                    && !crate::review::self_review_permitted(policy)
                || independence == "team_independent"
                    && (reviewer == round.get::<_, String>("author_actor_id")
                        || decision
                            .get::<_, Option<String>>("reviewer_person_id")
                            .is_none()
                        || decision.get::<_, Option<String>>("reviewer_person_id")
                            == round.get::<_, Option<String>>("author_person_id"))
        }
    {
        return Err(PgError::ReviewRequired);
    }
    let mut checks = Vec::new();
    let mut facts = Vec::new();
    for check in &candidate.binding.required_checks {
        let row = tx.query_opt(
            "SELECT f.id,f.envelope_json,h.generation,x.binding_digest,x.source_snapshot_id,x.ownership_version,x.coordinator_epoch,x.connector_version,x.selection_version
             FROM awr_team.delivery_fact_heads h JOIN awr_team.delivery_facts f ON f.tenant_id=h.tenant_id AND f.project_id=h.project_id AND f.id=h.fact_id
             JOIN awr_team.delivery_inbox i ON i.tenant_id=f.tenant_id AND i.project_id=f.project_id AND i.id=f.inbox_id
             JOIN awr_team.delivery_inspections x ON x.tenant_id=i.tenant_id AND x.project_id=i.project_id AND x.id=i.inspection_id
             WHERE h.tenant_id=$1 AND h.project_id=$2 AND h.connector_id=$3 AND h.work_id=$4 AND h.slot=$5",
            &[&tenant,&project,&request.connector_id,&set.work_id,&hash(&json!(["verification",check]))?],
        ).await?.ok_or(PgError::EvidenceInvalid)?;
        let envelope: DeliveryEnvelope =
            serde_json::from_value(row.get(1)).map_err(|_| PgError::SourceDivergence)?;
        envelope.validate().map_err(|_| PgError::EvidenceInvalid)?;
        envelope
            .record
            .validate_against(&candidate.binding)
            .map_err(|_| PgError::BindingInvalid)?;
        let DeliveryRecord::Verification(verification) = &envelope.record else {
            return Err(PgError::EvidenceInvalid);
        };
        if verification.check != *check
            || verification.outcome != VerificationOutcome::Passed
            || verification.provenance.source != FactSource::AdapterObservation
            || row.get::<_, String>(3) != selected
            || row.get::<_, String>(4) != auth.snapshot
            || row.get::<_, i64>(5) != ownership
            || row.get::<_, String>(6) != auth.epoch
            || row.get::<_, i64>(7) != connector.get::<_, i64>(0)
            || row.get::<_, i64>(8) != selection
        {
            return Err(PgError::EvidenceInvalid);
        }
        checks.push(VerificationRef {
            check: check.clone(),
            run_id: verification.run_id.clone(),
        });
        facts.push(json!({"fact_id":row.get::<_,String>(0),"generation":row.get::<_,i64>(2).to_string(),"envelope_digest":hash(&json!(envelope))?}));
    }
    Ok(Eligibility {
        candidate,
        checks,
        binding: json!({"read_set":set,"candidate_digest":selected,"selection_version":selection.to_string(),
            "connector_id":request.connector_id,"connector_version":request.connector_version,
            "worker_actor_id":connector.get::<_,String>(6),"worker_client_id":connector.get::<_,String>(7),
            "policy":policy,"evidence_id":request.evidence_id,"evidence_digest":evidence_digest,"artifact_id":artifact_id,"artifacts":artifacts,
            "review_round_id":request.review_round_id,"review_decision_id":request.review_decision_id,
            "reviewer_actor_id":reviewer,"reviewer_client_id":reviewer_client,"approval_basis":basis,"settlement":settlement,"verification_facts":facts}),
    })
}
