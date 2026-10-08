//! Source admission derives impact and settlement from persisted project facts.
//! The caller's affected/stopped lists never authorize an execution transition.
use super::*;
use awr_core::DeliveryVersionPolicy;
use awr_team::DependencyAcceptanceMode;
use serde_json::json;
use std::collections::VecDeque;
use tokio_postgres::Row;

pub(super) async fn derive(
    candidate: &SourceProjection,
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    previous: Option<&str>,
) -> PgResult<BTreeSet<String>> {
    let Some(snapshot) = previous else {
        require_known_effects(
            tx,
            tenant,
            project,
            &candidate.hashes.keys().cloned().collect::<Vec<_>>(),
            candidate.bundle.is_some(),
        )
        .await?;
        return Ok(candidate.hashes.keys().cloned().collect());
    };
    tx.query_opt(
        "SELECT id FROM awr_team.source_snapshots WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant, &project, &snapshot],
    )
    .await?
    .ok_or(PgError::SourceDivergence)?;
    let catalog: Option<WorkstreamCatalog> = tx
        .query_opt(
            "SELECT catalog_json FROM awr_team.workstream_catalogs WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
            &[&tenant, &project, &snapshot],
        )
        .await?
        .map(|row| serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence))
        .transpose()?;
    if let Some(catalog) = &catalog {
        catalog.validate()?;
        if catalog.project_id != project {
            return Err(PgError::SourceDivergence);
        }
    }
    let rows = tx.query(
        "SELECT c.work_id,c.contract_hash,c.contract_json,o.workstream_id,o.ownership_version,
                current.workstream_id AS current_stream,current.ownership_version AS current_ownership
         FROM awr_team.work_contracts c
         LEFT JOIN awr_team.workstream_snapshot_ownership o
           ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=c.snapshot_id
          AND o.scope_id=c.scope_id AND o.work_id=c.work_id
         LEFT JOIN awr_team.workstream_ownership current
           ON current.tenant_id=c.tenant_id AND current.project_id=c.project_id AND current.work_id=c.work_id
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main'",
        &[&tenant,&project,&snapshot],
    ).await?;
    if rows.is_empty() {
        return Err(PgError::SourceDivergence);
    }
    let mut contracts = BTreeMap::new();
    let mut affected = BTreeSet::new();
    let mut old_edges = Vec::new();
    for row in rows {
        let work: String = row.get(0);
        let hash: String = row.get(1);
        let contract: WorkContract =
            serde_json::from_value(row.get(2)).map_err(|_| PgError::SourceDivergence)?;
        if contract.work_id.as_str() != work
            || contract.hash().map_err(|_| PgError::SourceDivergence)? != hash
        {
            return Err(PgError::SourceDivergence);
        }
        if let Some(old) = &catalog {
            let owner: String = row
                .get::<_, Option<String>>(3)
                .ok_or(PgError::SourceDivergence)?;
            let version: i64 = row
                .get::<_, Option<i64>>(4)
                .ok_or(PgError::SourceDivergence)?;
            if row.get::<_, Option<String>>("current_stream").as_deref() != Some(&owner)
                || row.get::<_, Option<i64>>("current_ownership") != Some(version)
            {
                return Err(PgError::SourceDivergence);
            }
            let id = owner.parse().map_err(|_| PgError::SourceDivergence)?;
            let stream = old.get(id)?;
            if candidate
                .bundle
                .as_ref()
                .and_then(|b| b.catalog.get(id).ok())
                != Some(stream)
            {
                affected.insert(work.clone());
            }
        } else if candidate.bundle.is_some() {
            // First explicit ownership is a real context change, not attribution
            // of a historical legacy executor to today's catalog.
            affected.insert(work.clone());
        }
        if candidate.hashes.get(&work) != Some(&hash) {
            affected.insert(work.clone());
        }
        for upstream in &contract.required_dependencies {
            old_edges.push(DependencyEdge {
                from: work.clone(),
                to: upstream.clone(),
                relation: "requires".into(),
                required: true,
            });
        }
        contracts.insert(work, contract);
    }
    if catalog.is_some() {
        validate_required_graph(&contracts.keys().cloned().collect::<Vec<_>>(), &old_edges)?;
    }
    require_known_effects(
        tx,
        tenant,
        project,
        &contracts.keys().cloned().collect::<Vec<_>>(),
        catalog.is_some() || candidate.bundle.is_some(),
    )
    .await?;
    affected.extend(
        candidate
            .hashes
            .keys()
            .filter(|w| !contracts.contains_key(*w))
            .cloned(),
    );
    let mut downstream: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in old_edges.iter().chain(candidate.edges.iter()) {
        downstream
            .entry(edge.to.clone())
            .or_default()
            .insert(edge.from.clone());
    }
    let next: BTreeMap<_, _> = candidate
        .contracts
        .iter()
        .map(|c| (c.work_id.as_str(), c))
        .collect();
    let mut pending: VecDeque<_> = affected.iter().cloned().collect();
    while let Some(upstream) = pending.pop_front() {
        for consumer in downstream.get(&upstream).into_iter().flatten() {
            if affected.contains(consumer) {
                continue;
            }
            // The consumer's own changed contract/catalog is an independent
            // seed. Only unchanged, currently verified fixed adoption cuts a
            // provider-version propagation edge in either graph.
            let fixed = next.get(consumer.as_str()).and_then(|c| {
                match c.dependency_acceptance.get(&upstream) {
                    Some(DependencyAcceptanceMode::CrossWorkstream(policy))
                        if policy.version_policy == DeliveryVersionPolicy::FixedDelivery =>
                    {
                        Some(*policy)
                    }
                    _ => None,
                }
            });
            let preserved = if let Some(policy) = fixed {
                crate::cross_workstream_adoption::adopted_receipt(
                    tx, tenant, project, snapshot, consumer, &upstream, policy,
                )
                .await?
                .is_some()
            } else {
                false
            };
            if !preserved && affected.insert(consumer.clone()) {
                pending.push_back(consumer.clone());
            }
        }
    }
    Ok(affected)
}

async fn require_known_effects(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    known: &[String],
    scoped: bool,
) -> PgResult<()> {
    let blocked: bool = tx.query_one(
        "SELECT EXISTS(SELECT 1 FROM awr_team.executions e
            LEFT JOIN awr_team.workstream_ownership o ON o.tenant_id=e.tenant_id AND o.project_id=e.project_id AND o.work_id=e.work_id
            WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.state NOT IN ('succeeded','failed','cancelled')
              AND (NOT e.work_id=ANY($3) OR ($4 AND (e.scope_id<>'main' OR e.workstream_id IS NULL
                OR e.workstream_id IS DISTINCT FROM o.workstream_id OR e.ownership_version IS DISTINCT FROM o.ownership_version))))
          OR EXISTS(SELECT 1 FROM awr_team.work_runtime
            WHERE tenant_id=$1 AND project_id=$2 AND recovery_blocked AND NOT work_id=ANY($3))
          OR EXISTS(SELECT 1 FROM awr_team.resource_reservations
            WHERE tenant_id=$1 AND project_id=$2 AND state IN ('reserved','unknown') AND NOT work_id=ANY($3))",
        &[&tenant,&project,&known,&scoped],
    ).await?.get(0);
    if blocked {
        return Err(PgError::RecoveryBlocked);
    }
    Ok(())
}

pub(super) async fn require_settled(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    affected: &BTreeSet<String>,
) -> PgResult<()> {
    let ids: Vec<_> = affected.iter().cloned().collect();
    let blocked: bool = tx.query_one(
        "SELECT EXISTS(SELECT 1 FROM awr_team.work_runtime
            WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=ANY($3) AND recovery_blocked)
          OR EXISTS(SELECT 1 FROM awr_team.resource_reservations
            WHERE tenant_id=$1 AND project_id=$2 AND work_id=ANY($3) AND state IN ('reserved','unknown'))
          OR EXISTS(SELECT 1 FROM awr_team.outbox o JOIN awr_team.executions e
            ON e.tenant_id=o.tenant_id AND e.project_id=o.project_id AND e.id=o.aggregate_id
            WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.work_id=ANY($3) AND o.state IN ('pending','sending'))",
        &[&tenant,&project,&ids],
    ).await?.get(0);
    if blocked {
        return Err(PgError::RecoveryBlocked);
    }
    let executions = tx.query(
        "SELECT e.*,
            EXISTS(SELECT 1 FROM awr_team.outbox o WHERE o.tenant_id=e.tenant_id AND o.project_id=e.project_id AND o.aggregate_id=e.id) AS exposed_outbox,
            EXISTS(SELECT 1 FROM awr_team.execution_receipts r WHERE r.tenant_id=e.tenant_id AND r.project_id=e.project_id AND r.execution_id=e.id) AS exposed_receipt
         FROM awr_team.executions e WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.work_id=ANY($3)",
        &[&tenant,&project,&ids],
    ).await?;
    for run in executions {
        let state: String = run.get("state");
        if matches!(state.as_str(), "prepared" | "cancelled")
            && !run.get::<_, bool>("exposed_outbox")
            && !run.get::<_, bool>("exposed_receipt")
            && !run.get::<_, bool>("terminal_reported")
        {
            continue;
        }
        if !matches!(state.as_str(), "succeeded" | "failed" | "cancelled") {
            return Err(PgError::RecoveryBlocked);
        }
        let receipt = tx.query_opt(
            "SELECT receipt_kind,digest,payload_json,reporter_actor_id FROM awr_team.execution_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND execution_id=$3 ORDER BY created_at DESC,id DESC LIMIT 1",
            &[&tenant,&project,&run.get::<_,String>("id")],
        ).await?.ok_or(PgError::RecoveryBlocked)?;
        let kind: String = receipt.get(0);
        let hash: String = receipt.get(1);
        let payload: Value = receipt.get(2);
        if awr_team::request_hash(&payload).map_err(|_| PgError::EvidenceInvalid)? != hash
            || !bound_terminal_receipt(&run, &payload, &kind)
        {
            return Err(PgError::RecoveryBlocked);
        }
        if kind == "caller_asserted" {
            if receipt.get::<_, String>(3) != run.get::<_, String>("executor_actor_id")
                || !workspace_resources_match(tx, tenant, project, &run, &payload).await?
            {
                return Err(PgError::RecoveryBlocked);
            }
        }
    }
    Ok(())
}

fn positive_decimal(value: &Value) -> Option<i64> {
    let text = value.as_str()?;
    let number = text.parse::<i64>().ok()?;
    (number > 0 && number.to_string() == text).then_some(number)
}

fn required_string_matches(run: &Row, payload: &Value, key: &str, column: &str) -> bool {
    payload[key].as_str().is_some_and(|text| {
        !text.trim().is_empty()
            && !text.contains('\0')
            && run.get::<_, Option<String>>(column).as_deref() == Some(text)
    })
}

fn required_digest_matches(run: &Row, payload: &Value, key: &str) -> bool {
    payload[key].as_str().is_some_and(|text| {
        text.len() == 64
            && text.bytes().all(|b| b.is_ascii_hexdigit())
            && run.get::<_, Option<String>>(key).as_deref() == Some(text)
    })
}

fn bound_terminal_receipt(run: &Row, payload: &Value, kind: &str) -> bool {
    let version = positive_decimal(&payload["execution_version"]);
    if version.and_then(|v| v.checked_add(1)) != Some(run.get::<_, i64>("execution_version"))
        || payload["outcome"] != run.get::<_, String>("state")
        || payload["effects_settled"] != true
        || !run.get::<_, bool>("terminal_reported")
        || payload["contract_hash"] != run.get::<_, String>("contract_hash")
        || !required_digest_matches(run, payload, "input_digest")
        || !required_digest_matches(run, payload, "environment_digest")
        || payload["output_digest"].as_str()
            != run.get::<_, Option<String>>("result_digest").as_deref()
        || Some(&payload["observed_paths"])
            != run.get::<_, Option<Value>>("observed_paths_json").as_ref()
        || !required_string_matches(run, payload, "workstream_id", "workstream_id")
        || positive_decimal(&payload["ownership_version"])
            .zip(run.get::<_, Option<i64>>("ownership_version"))
            .is_none_or(|(declared, stored)| declared != stored)
    {
        return false;
    }
    match kind {
        "caller_asserted" => {
            run.get::<_, bool>("workspace_effects_settled")
                && run.get::<_, Option<String>>("admission_mode").as_deref()
                    == Some("caller_managed")
                && payload["admission_mode"] == "caller_managed"
                && payload["settlement_scope"] == "admitted_workspace_paths"
                && payload["execution_id"] == run.get::<_, String>("id")
                && payload["work_id"] == run.get::<_, String>("work_id")
                && required_string_matches(run, payload, "session_id", "session_id")
                && required_string_matches(run, payload, "client_id", "executor_client_id")
                && required_string_matches(run, payload, "claim_id", "claim_id")
                && positive_decimal(&payload["fence"]) == Some(run.get::<_, i64>("fence"))
                && positive_decimal(&payload["admission_lease_version"])
                    .zip(run.get::<_, Option<i64>>("admission_lease_version"))
                    .is_some_and(|(declared, stored)| declared == stored)
                && Some(&payload["settlement_policy"])
                    == run
                        .get::<_, Option<Value>>("settlement_policy_json")
                        .as_ref()
                && required_string_matches(run, payload, "coordinator_epoch", "coordinator_epoch")
                && payload["workspace_settlement"]["executor_stopped"] == true
                && payload["workspace_settlement"]["no_external_effects"] == true
                && payload["workspace_settlement"]["workspace_id"]
                    == payload["settlement_policy"]["workspace_id"]
                && payload["workspace_settlement"]["claim_id"] == payload["claim_id"]
                && payload["workspace_settlement"]["expected_fence"] == payload["fence"]
                && payload["workspace_settlement"]["input_digest"] == payload["input_digest"]
                && payload["workspace_settlement"]["environment_digest"]
                    == payload["environment_digest"]
                && positive_decimal(&payload["workspace_settlement"]["expected_lease_version"])
                    .zip(run.get::<_, Option<i64>>("admission_lease_version"))
                    .is_some_and(|(declared, admitted)| declared >= admitted)
                && payload["scope_violation"] == false
        }
        "trusted_executor" | "reconcile" => {
            // Preserve the established privileged recovery assertion's scope.
            // It is not independent observation of an external process.
            required_string_matches(run, payload, "execution_session_id", "session_id")
                && required_string_matches(
                    run,
                    payload,
                    "execution_coordinator_epoch",
                    "coordinator_epoch",
                )
                && payload["executor_stopped"] != false
        }
        _ => false,
    }
}

async fn workspace_resources_match(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    run: &Row,
    payload: &Value,
) -> PgResult<bool> {
    let rows = tx.query(
        "SELECT id,resource_kind,canonical_key,worktree_id,fence,lease_generation,state FROM awr_team.resource_reservations
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND execution_id=$4 ORDER BY id",
        &[&tenant,&project,&run.get::<_,String>("work_id"),&run.get::<_,String>("id")],
    ).await?;
    let proofs: Vec<Value> = rows.iter().map(|r| json!({
        "reservation_id":r.get::<_,String>(0),"kind":r.get::<_,String>(1),"key":r.get::<_,String>(2),
        "workspace_id":r.get::<_,String>(3),"fence":r.get::<_,i64>(4).to_string(),
        "admission_lease_version":r.get::<_,i64>(5).to_string(),
    })).collect();
    let declared: Vec<String> = serde_json::from_value(run.get("declared_scope_json"))
        .map_err(|_| PgError::SourceDivergence)?;
    let paths: BTreeSet<_> = declared
        .iter()
        .map(|p| crate::graph::normalize_resource_key("dir", p))
        .collect::<PgResult<_>>()?;
    Ok(!paths.is_empty()
        && paths.len() == declared.len()
        && rows.len() == paths.len()
        && rows.iter().all(|r| {
            r.get::<_, String>(6) == "released"
                && r.get::<_, String>(1) == "dir"
                && paths.contains(&r.get::<_, String>(2))
                && payload["settlement_policy"]["workspace_id"] == r.get::<_, String>(3)
                && r.get::<_, i64>(4) == run.get::<_, i64>("fence")
                && Some(r.get::<_, i64>(5)) == run.get::<_, Option<i64>>("admission_lease_version")
        })
        && payload["resource_proof"] == json!(proofs))
}

pub(super) async fn invalidate(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    affected: &BTreeSet<String>,
) -> PgResult<Value> {
    let ids: Vec<_> = affected.iter().cloned().collect();
    // Admission, dispatch, adoption and revocation hold the same project lock.
    // No exposed effect or reservation was accepted by require_settled.
    let invalidated = tx.query(
        "UPDATE awr_team.executions e SET state='cancelled',cancel_requested=true,execution_version=execution_version+1
         WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.scope_id='main' AND e.work_id=ANY($3) AND e.state='prepared'
           AND NOT EXISTS(SELECT 1 FROM awr_team.outbox o WHERE o.tenant_id=e.tenant_id AND o.project_id=e.project_id AND o.aggregate_id=e.id)
           AND NOT EXISTS(SELECT 1 FROM awr_team.execution_receipts r WHERE r.tenant_id=e.tenant_id AND r.project_id=e.project_id AND r.execution_id=e.id)
         RETURNING id,work_id,execution_version",
        &[&tenant,&project,&ids],
    ).await?;
    tx.execute(
        "UPDATE awr_team.work_runtime SET work_version=work_version+1
         WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=ANY($3) AND state <> 'completed'",
        &[&tenant,&project,&ids],
    ).await?;
    let rounds = tx.execute(
        "UPDATE awr_team.review_rounds SET state='invalidated'
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=ANY($3) AND state IN ('open','approved')
           AND NOT EXISTS(SELECT 1 FROM awr_team.completion_receipts accepted
             WHERE accepted.tenant_id=$1 AND accepted.project_id=$2 AND accepted.work_id=awr_team.review_rounds.work_id
               AND accepted.approved_by_json->>'review_round_id'=awr_team.review_rounds.id)",
        &[&tenant,&project,&ids],
    ).await?;
    Ok(
        json!({"basis":"server_projected_contract_graph_and_persisted_settlement_v1",
        "affected_work_ids":ids,"invalidated_preparations":invalidated.iter().map(|r| json!({
            "execution_id":r.get::<_,String>(0),"work_id":r.get::<_,String>(1),"execution_version":r.get::<_,i64>(2).to_string()
        })).collect::<Vec<_>>(),"invalidated_unaccepted_review_rounds":rounds,
        "claims_released":false,"external_stop_observed":false,"accepted_history_retained":true}),
    )
}
