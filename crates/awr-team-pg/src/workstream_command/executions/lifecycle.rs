//! Caller-managed execution: a transactional admission decision, not a remote
//! process supervisor. Workspace settlement is an explicit caller agreement;
//! it never settles untracked external effects or upgrades caller trust.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Start {
    pub(super) session_id: String,
    pub(super) expected_session_version: String,
    execution_id: String,
    expected_execution_version: String,
    expected_work_version: String,
    claim_id: String,
    expected_fence: String,
    expected_lease_version: String,
    execution_mode: String,
    expected_input_digest: Option<String>,
}
impl Start {
    pub(super) fn parse(args: Value) -> PgResult<Self> {
        let a: Self = serde_json::from_value(args).map_err(|_| invalid())?;
        if !identity(&a.execution_id)
            || !identity(&a.claim_id)
            || version(&a.expected_execution_version)? == 0
            || version(&a.expected_work_version)? == 0
            || version(&a.expected_fence)? == 0
            || version(&a.expected_lease_version)? == 0
        {
            return Err(invalid());
        }
        if !matches!(
            a.execution_mode.as_str(),
            "caller_managed" | "reference_write_v1"
        ) {
            return Err(PgError::Unsupported("execution mode".into()));
        }
        if a.expected_input_digest.as_ref().is_some_and(|s| !digest(s))
            || (a.execution_mode == "reference_write_v1" && a.expected_input_digest.is_none())
        {
            return Err(invalid());
        }
        Ok(a)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Report {
    pub(super) session_id: String,
    pub(super) expected_session_version: String,
    execution_id: String,
    expected_execution_version: String,
    outcome: String,
    output_digest: Option<String>,
    observed_paths: Vec<String>,
    note: String,
    workspace_settlement: Option<settlement::Declaration>,
}
impl Report {
    pub(super) fn parse(args: Value) -> PgResult<Self> {
        let a: Self = serde_json::from_value(args).map_err(|_| invalid())?;
        if !identity(&a.execution_id)
            || version(&a.expected_execution_version)? == 0
            || !matches!(
                a.outcome.as_str(),
                "succeeded" | "failed" | "cancelled" | "unknown"
            )
            || a.output_digest.as_ref().is_some_and(|s| !digest(s))
            || (a.outcome == "succeeded" && a.output_digest.is_none())
            || a.observed_paths.len() > 128
            || a.observed_paths.iter().any(|p| !canonical_path(p))
            || a.note.trim().is_empty()
            || a.note.len() > 4096
            || a.note.contains('\0')
        {
            return Err(invalid());
        }
        if let Some(declaration) = &a.workspace_settlement {
            declaration.validate()?;
        }
        Ok(a)
    }
}

async fn owned_execution(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    session: &str,
    execution: &str,
    expected_version: &str,
) -> PgResult<Row> {
    let r = load(tx, tenant, project, execution).await?;
    require_binding(
        &r,
        &command.work_id,
        &command.workstream_id.to_string(),
        ownership,
    )?;
    if r.get::<_, Option<String>>("session_id").as_deref() != Some(session)
        || r.get::<_, String>("executor_actor_id") != auth.actor_id
        || r.get::<_, Option<String>>("executor_client_id").as_deref() != Some(&auth.client_id)
    {
        return Err(PgError::Forbidden);
    }
    if r.get::<_, Option<String>>("coordinator_epoch").as_deref() != Some(&auth.epoch) {
        return Err(PgError::EpochChanged);
    }
    let v: i64 = r.get("execution_version");
    if v != version(expected_version)? || v == i64::MAX {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(r)
}

pub(super) async fn start(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    contract: &awr_team::WorkContract,
    a: Start,
) -> PgResult<Value> {
    let r = owned_execution(
        tx,
        tenant,
        project,
        auth,
        command,
        ownership,
        &a.session_id,
        &a.execution_id,
        &a.expected_execution_version,
    )
    .await?;
    if a.expected_input_digest
        .as_ref()
        .is_some_and(|input| r.get::<_, Option<String>>("input_digest").as_ref() != Some(input))
    {
        return Err(PgError::PreconditionsChanged);
    }
    // The bundled adapter may attest only when the operator delegated that
    // authority before admission. A mode name cannot grant executor trust.
    if a.execution_mode == "reference_write_v1"
        && !auth
            .execution_access
            .get(&command.workstream_id)
            .is_some_and(|a| a.attest)
    {
        return Err(PgError::Forbidden);
    }
    if r.get::<_, String>("state") != "prepared"
        || r.get::<_, bool>("cancel_requested")
        || r.get::<_, String>("contract_hash") != command.expected_contract_hash
        || r.get::<_, String>("fencing_class") != "uncontrolled"
    {
        return Err(PgError::PreconditionsChanged);
    }
    let settlement_policy = settlement::policy(&r)?;
    if settlement_policy != contract.execution_settlement {
        return Err(PgError::PreconditionsChanged);
    }
    if r.get::<_, Option<String>>("claim_id").as_deref() != Some(&a.claim_id) {
        return Err(PgError::Forbidden);
    }
    require_enabled(tx, tenant, project, auth, command).await?;
    claims::require_live(
        tx,
        tenant,
        project,
        auth,
        command,
        ownership,
        &a.session_id,
        &a.claim_id,
        &a.expected_fence,
        &a.expected_lease_version,
    )
    .await?;
    require_ready(
        tx,
        tenant,
        project,
        &command.work_id,
        &a.expected_work_version,
        &a.execution_id,
    )
    .await?;
    // An intent already exposed via another path cannot become a new dispatch.
    let exposed: bool = tx.query_one("SELECT
        EXISTS(SELECT 1 FROM awr_team.outbox WHERE tenant_id=$1 AND project_id=$2 AND aggregate_id=$3) OR
        EXISTS(SELECT 1 FROM awr_team.execution_receipts WHERE tenant_id=$1 AND project_id=$2 AND execution_id=$3)",
        &[&tenant,&project,&a.execution_id]).await?.get(0);
    if exposed {
        // Effects already exposed keep recovery handling even if a planning
        // block arrives later; do not convert that into a fresh denial here.
        return Err(PgError::RecoveryBlocked);
    }
    require_clear_of_selective_blocks(tx, tenant, project, &command.work_id).await?;
    // Cross-stream receipts need explicit export/adoption. A grant to both
    // streams does not implicitly create such a delivery contract.
    for upstream in &contract.required_dependencies {
        let row = tx.query_opt("SELECT workstream_id FROM awr_team.workstream_snapshot_ownership
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant,&project,&auth.snapshot,upstream]).await?;
        if row.is_none_or(|r| r.get::<_, String>(0) != command.workstream_id.to_string()) {
            return Err(PgError::BindingInvalid);
        }
    }
    let (covered, dependencies) = crate::review::required_dependencies_covered(
        tx,
        tenant,
        project,
        &command.work_id,
        "main",
        &json!(contract),
    )
    .await?;
    if !covered {
        return Err(PgError::BindingInvalid);
    }
    let declared: Vec<String> = serde_json::from_value(r.get("declared_scope_json"))
        .map_err(|_| PgError::SourceDivergence)?;
    if declared.len() > 128 {
        return Err(PgError::SourceDivergence);
    }
    require_paths(contract, &declared)?;
    // Store the same canonical key reserve_bound would. Parent segments are
    // rejected here, not only on the graph reserve entry.
    let paths = declared
        .iter()
        .map(|path| crate::graph::normalize_resource_key("dir", path))
        .collect::<PgResult<Vec<_>>>()?;
    // Existing project locking serializes check+reserve+start. Directory
    // bounds are conservative within a worktree's lexical namespace, not OS
    // locks; shared external/integration identities remain project-global
    // (WS-021).
    let existing = tx
        .query(
            "SELECT resource_kind,canonical_key,worktree_id FROM awr_team.resource_reservations
        WHERE tenant_id=$1 AND project_id=$2 AND state IN ('reserved','unknown')",
            &[&tenant, &project],
        )
        .await?;
    let worktree_id = if a.execution_mode == "caller_managed" {
        settlement_policy
            .as_ref()
            .map(|p| p.workspace_id.clone())
            .unwrap_or_default()
    } else {
        String::new()
    }; // an explicit lexical agreement, never proof of physical isolation
    if settlement_policy.is_some() {
        settlement::normalized_scope(&declared)?;
    }
    if paths.iter().any(|p| {
        let candidate = crate::graph::ResourceBound {
            kind: "dir".into(),
            key: p.clone(),
            worktree_id: worktree_id.clone(),
        };
        existing.iter().any(|r| {
            crate::graph::resources_conflict(
                &candidate,
                &crate::graph::ResourceBound {
                    kind: r.get::<_, String>(0),
                    key: r.get::<_, String>(1),
                    worktree_id: r.get::<_, String>(2),
                },
            )
        })
    }) {
        return Err(PgError::ResourceConflict);
    }
    let fence: i64 = a
        .expected_fence
        .parse()
        .map_err(|_| PgError::Protocol("invalid fence".into()))?;
    let lease_generation: i64 = a
        .expected_lease_version
        .parse()
        .map_err(|_| PgError::Protocol("invalid lease generation".into()))?;
    let mut resources = vec![];
    for path in &paths {
        let id = crate::tx::new_id();
        tx.execute(
            "INSERT INTO awr_team.resource_reservations(
                tenant_id,project_id,id,work_id,resource_kind,canonical_key,state,
                execution_id,worktree_id,lease_generation,fence)
            VALUES($1,$2,$3,$4,'dir',$5,'reserved',$6,$7,$8,$9)",
            &[
                &tenant,
                &project,
                &id,
                &command.work_id,
                path,
                &a.execution_id,
                &worktree_id,
                &lease_generation,
                &fence,
            ],
        )
        .await?;
        resources.push(json!({
            "reservation_id": id,
            "kind": "dir",
            "key": path,
            "worktree_id": worktree_id,
            "lease_generation": lease_generation.to_string(),
            "fence": fence.to_string(),
            "isolation": "lexical_worktree_bound_not_os_sandbox",
        }));
    }
    // Time advances during dependency/resource checks even while rows are locked.
    claims::require_live(
        tx,
        tenant,
        project,
        auth,
        command,
        ownership,
        &a.session_id,
        &a.claim_id,
        &a.expected_fence,
        &a.expected_lease_version,
    )
    .await?;
    let attestation_grant = auth
        .execution_access
        .get(&command.workstream_id)
        .filter(|access| {
            access.attest
                && (settlement_policy.is_none() || a.execution_mode == "reference_write_v1")
        })
        .map(|_| auth.grant_versions[&command.workstream_id]);
    tx.execute(
        "UPDATE awr_team.executions SET state='running',execution_version=execution_version+1,attestation_grant_version=$4,
         admission_mode=$5,admission_lease_version=$6
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant, &project, &a.execution_id, &attestation_grant, &a.execution_mode, &lease_generation],
    )
    .await?;
    let work_version = advance_work(tx, tenant, project, &command.work_id).await?;
    let remaining: i64 = tx
        .query_one(
            "SELECT floor(extract(epoch FROM (expires_at-clock_timestamp()))*1000)::bigint
        FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &a.claim_id],
        )
        .await?
        .get(0);
    if remaining <= 0 {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(
        json!({"execution_id":a.execution_id,"execution_version":(r.get::<_,i64>("execution_version")+1).to_string(),
        "session_id":a.session_id,"claim_id":a.claim_id,"state":"running","fence":a.expected_fence,
        "effect_key":r.get::<_,Option<String>>("effect_key"),"work_version":work_version.to_string(),
        "admission":"granted_at_commit","execution_mode":a.execution_mode,"dispatched":false,
        "input_digest":r.get::<_,Option<String>>("input_digest"),"declared_scope":paths,
        "lease_remaining_ms":remaining.to_string(),
        "fencing_class":"uncontrolled","exactly_once_supported":false,"scope_validation":"lexical_contract_only",
        "physical_isolation":"unverified_without_host_capability",
        "dependency_receipts":dependencies,"resources":resources,
        "result_authority":if attestation_grant.is_some() {"trusted_executor"} else {"caller_asserted"},
        "next_action":"Execute once under the current lease; report observations. A replay or unknown response never authorizes another start. Do not claim OS/sandbox isolation from AWR metadata alone."}),
    )
}

pub(super) async fn report(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    contract: &awr_team::WorkContract,
    a: Report,
) -> PgResult<Value> {
    let r = owned_execution(
        tx,
        tenant,
        project,
        auth,
        command,
        ownership,
        &a.session_id,
        &a.execution_id,
        &a.expected_execution_version,
    )
    .await?;
    if !matches!(r.get::<_, String>("state").as_str(), "running" | "unknown") {
        return Err(PgError::PreconditionsChanged);
    }
    let paths: Vec<String> = serde_json::from_value(r.get("declared_scope_json"))
        .map_err(|_| PgError::SourceDivergence)?;
    let exceeded = a.observed_paths.iter().any(|p| {
        !paths
            .iter()
            .any(|s| canonical_path(s) && crate::graph::path_within_scope(s, p))
    });
    let settled = settlement::evaluate(
        tx,
        tenant,
        project,
        auth,
        command,
        ownership,
        &r,
        contract,
        a.workspace_settlement.as_ref(),
        &a.outcome,
        exceeded,
    )
    .await?;
    let effects_settled = settled.is_some();
    let terminal_reported = a.outcome != "unknown";
    let attributable = !effects_settled
        && recovery_cause::may_attribute(
            tx,
            tenant,
            project,
            auth,
            &command.workstream_id,
            &command.expected_contract_hash,
            &r,
            exceeded,
        )
        .await?;
    let id = crate::tx::new_id();
    let mut payload = json!({"outcome":a.outcome,"output_digest":a.output_digest,"observed_paths":a.observed_paths,
        "note":a.note,"client_id":auth.client_id,"session_id":a.session_id,
        "workstream_id":command.workstream_id,"ownership_version":ownership.to_string(),
        "execution_version":a.expected_execution_version,"coordinator_epoch":auth.epoch,
        "contract_hash":r.get::<_,String>("contract_hash"),"scope_violation":exceeded});
    if let Some(settled) = &settled {
        settled.bind_payload(&mut payload, &r, &a.expected_session_version);
    } else if a.workspace_settlement.is_some() {
        payload["workspace_settlement"] = json!(a.workspace_settlement);
        payload["terminal_reported"] = json!(terminal_reported);
        payload["effects_settled"] = json!(false);
        payload["artifact_verified"] = json!(false);
    }
    let hash = awr_team::request_hash(&payload).map_err(|_| invalid())?;
    tx.execute("INSERT INTO awr_team.execution_receipts(tenant_id,project_id,id,execution_id,reporter_actor_id,receipt_kind,digest,payload_json)
        VALUES($1,$2,$3,$4,$5,'caller_asserted',$6,$7)",
        &[&tenant,&project,&id,&a.execution_id,&auth.actor_id,&hash,&payload]).await?;
    let next = if effects_settled {
        a.outcome.as_str()
    } else {
        "unknown"
    };
    if let Some(settled) = &settled {
        tx.execute(
            "UPDATE awr_team.executions SET state=$4,execution_version=execution_version+1,
             result_digest=$5,environment_digest=$6,observed_paths_json=$7,
             terminal_reported=true,workspace_effects_settled=true,unknown_reason=NULL
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &tenant,
                &project,
                &a.execution_id,
                &next,
                &a.output_digest,
                &settled.declaration.environment_digest,
                &json!(a.observed_paths),
            ],
        )
        .await?;
        settlement::release(tx, tenant, project, &r, settled).await?;
        // Preserve the existing false barrier; never blanket-clear work recovery.
    } else {
        tx.execute(
            "UPDATE awr_team.executions SET state='unknown',execution_version=execution_version+1,
             terminal_reported=$4,unknown_reason='caller_report_requires_reconciliation'
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &a.execution_id, &terminal_reported],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.work_runtime SET recovery_blocked=true
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.resource_reservations SET state='unknown'
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND execution_id=$4 AND state='reserved'",
            &[&tenant, &project, &command.work_id, &a.execution_id],
        ).await?;
        if attributable {
            recovery_cause::attribute(tx, tenant, project, &r, &id).await?;
        }
    }
    let work_version = advance_work(tx, tenant, project, &command.work_id).await?;
    if let Some(settled) = &settled {
        // The lease may expire while exact-resource and receipt checks execute.
        claims::require_live(
            tx,
            tenant,
            project,
            auth,
            command,
            ownership,
            &a.session_id,
            &settled.declaration.claim_id,
            &settled.declaration.expected_fence,
            &settled.declaration.expected_lease_version,
        )
        .await?;
    }
    Ok(
        json!({"execution_id":a.execution_id,"execution_version":(r.get::<_,i64>("execution_version")+1).to_string(),
        "session_id":a.session_id,"state":next,"receipt_id":id,"receipt_kind":"caller_asserted",
        "reported_outcome":a.outcome,"scope_violation":exceeded,"recovery_blocked":!effects_settled,
        "terminal_reported":terminal_reported,"artifact_verified":false,"effects_settled":effects_settled,
        "settlement_scope":if effects_settled {Some("admitted_workspace_paths")} else {None},
        "work_version":work_version.to_string(),"work_completed":false,"resource_release_performed":effects_settled,
        "resources_released":settled.as_ref().map_or(0,|s|s.resources.len()),
        "reconciliation_supported":true,
        "next_action":if effects_settled && a.outcome == "succeeded" {"Refresh work context; submit the exact readable artifact for independent Agent review. Workspace settlement is a caller assertion, not external delivery or acceptance."}
            else if effects_settled {"Refresh work context and assess a new attempt or handoff. Settled failure or cancellation is not task completion; do not replay the stopped execution."}
            else {"Have an authorized recovery operator inspect the receipt and reconcile actual effects. Do not retry or complete while recovery is blocked."}}),
    )
}
