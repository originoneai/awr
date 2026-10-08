//! Execution intents and caller-managed admission under live authority.
//! Preparation never dispatches. Admission is not physical effect confinement.
mod lifecycle;
mod recovery;
mod recovery_cause;
pub(super) mod settlement;
use super::*;
use tokio_postgres::Row;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prepare {
    session_id: String,
    expected_session_version: String,
    claim_id: String,
    expected_fence: String,
    expected_lease_version: String,
    expected_work_version: String,
    input_digest: String,
    declared_scope: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cancel {
    session_id: String,
    expected_session_version: String,
    execution_id: String,
    expected_execution_version: String,
}
pub(super) enum Action {
    Prepare(Prepare),
    Cancel(Cancel),
    Start(lifecycle::Start),
    Report(lifecycle::Report),
    Recovery(recovery::Action),
}

const PREPARE_FIELDS: &[&str] = &[
    "session_id",
    "expected_session_version",
    "claim_id",
    "expected_fence",
    "expected_lease_version",
    "expected_work_version",
    "input_digest",
    "declared_scope",
];
const PREPARE_NON_SESSION_FIELDS: &[&str] = &[
    "claim_id",
    "expected_fence",
    "expected_lease_version",
    "expected_work_version",
    "input_digest",
    "declared_scope",
];

fn validate_prepare(a: &Prepare) -> PgResult<()> {
    if !identity(&a.session_id)
        || version(&a.expected_session_version)? == 0
        || !identity(&a.claim_id)
        || !digest(&a.input_digest)
        || version(&a.expected_fence)? == 0
        || version(&a.expected_lease_version)? == 0
        || version(&a.expected_work_version)? == 0
        || a.declared_scope.len() > 128
        || a.declared_scope.iter().any(|p| !canonical_path(p))
    {
        return Err(invalid());
    }
    let mut unique = a.declared_scope.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != a.declared_scope.len() {
        return Err(invalid());
    }
    Ok(())
}

fn missing_prepare_session_binding(args: &Value) -> bool {
    let Some(object) = args.as_object() else {
        return false;
    };
    let missing_session =
        !object.contains_key("session_id") || !object.contains_key("expected_session_version");
    if !missing_session
        || !PREPARE_NON_SESSION_FIELDS
            .iter()
            .all(|field| object.contains_key(*field))
        || !object
            .keys()
            .all(|field| PREPARE_FIELDS.contains(&field.as_str()))
    {
        return false;
    }
    let mut completed = args.clone();
    let completed = completed.as_object_mut().expect("checked object");
    completed
        .entry("session_id")
        .or_insert_with(|| json!("missing-session-placeholder"));
    completed
        .entry("expected_session_version")
        .or_insert_with(|| json!("1"));
    serde_json::from_value::<Prepare>(Value::Object(completed.clone()))
        .is_ok_and(|prepare| validate_prepare(&prepare).is_ok())
}

impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        let action = match op {
            "execution.start" => Self::Start(lifecycle::Start::parse(args)?),
            "execution.report" => Self::Report(lifecycle::Report::parse(args)?),
            "execution.attest" | "execution.reconcile" => {
                Self::Recovery(recovery::Action::parse(op, args)?)
            }
            "execution.prepare" => {
                if missing_prepare_session_binding(&args) {
                    return Err(PgError::missing_execution_prepare_session_binding());
                }
                let a: Prepare = serde_json::from_value(args).map_err(|_| invalid())?;
                validate_prepare(&a)?;
                Self::Prepare(a)
            }
            "execution.cancel" => {
                let a: Cancel = serde_json::from_value(args).map_err(|_| invalid())?;
                if !identity(&a.execution_id) || version(&a.expected_execution_version)? == 0 {
                    return Err(invalid());
                }
                Self::Cancel(a)
            }
            _ => return Err(invalid()),
        };
        let (id, v) = action.session();
        if !identity(id) || version(v)? == 0 {
            return Err(invalid());
        }
        Ok(action)
    }
    fn session(&self) -> (&str, &str) {
        match self {
            Self::Prepare(a) => (&a.session_id, &a.expected_session_version),
            Self::Cancel(a) => (&a.session_id, &a.expected_session_version),
            Self::Start(a) => (&a.session_id, &a.expected_session_version),
            Self::Report(a) => (&a.session_id, &a.expected_session_version),
            Self::Recovery(a) => a.session(),
        }
    }
}

// The remote coordinator can validate portable lexical scope, not inspect the
// executor's filesystem. Reject aliases instead of claiming physical confinement.
fn canonical_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 4096
        && !p.chars().any(char::is_control)
        && !p.contains(['\\', ':'])
        && p.split('/').all(|s| !matches!(s, "" | "." | ".."))
}

#[cfg(test)]
mod input_guidance_tests {
    use super::*;

    fn prepare_args() -> Value {
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "claim_id":"claim-a",
            "expected_fence":"1",
            "expected_lease_version":"1",
            "expected_work_version":"1",
            "input_digest":"a".repeat(64),
            "declared_scope":["src"]
        })
    }

    #[test]
    fn prepare_names_an_otherwise_valid_missing_session_binding() {
        for missing in [
            &["session_id"][..],
            &["expected_session_version"][..],
            &["session_id", "expected_session_version"][..],
        ] {
            let mut args = prepare_args();
            let object = args.as_object_mut().unwrap();
            for field in missing {
                object.remove(*field);
            }
            let error = Action::parse("execution.prepare", args).err().unwrap();
            assert!(error.is_missing_execution_prepare_session_binding());
        }
    }

    #[test]
    fn prepare_keeps_malformed_unknown_and_other_incomplete_inputs_generic() {
        let mut cases = Vec::new();

        let mut malformed_version = prepare_args();
        malformed_version
            .as_object_mut()
            .unwrap()
            .remove("session_id");
        malformed_version["expected_session_version"] = json!(1);
        cases.push(malformed_version);

        let mut malformed_session = prepare_args();
        malformed_session
            .as_object_mut()
            .unwrap()
            .remove("expected_session_version");
        malformed_session["session_id"] = json!("");
        cases.push(malformed_session);

        let mut unknown = prepare_args();
        unknown.as_object_mut().unwrap().remove("session_id");
        unknown["unexpected"] = json!(true);
        cases.push(unknown);

        let mut other_missing = prepare_args();
        let object = other_missing.as_object_mut().unwrap();
        object.remove("session_id");
        object.remove("claim_id");
        cases.push(other_missing);

        for args in cases {
            let error = Action::parse("execution.prepare", args).err().unwrap();
            assert!(error.is_invalid_command_fields(), "{error}");
        }
        assert!(Action::parse("execution.prepare", prepare_args()).is_ok());
    }
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    contract: &awr_team::WorkContract,
    action: Action,
) -> PgResult<Applied> {
    let (sid, version) = action.session();
    session(tx, tenant, project, auth, command, sid, version, ownership).await?;
    let data = match action {
        Action::Prepare(a) => {
            prepare(tx, tenant, project, auth, command, ownership, contract, a).await?
        }
        Action::Cancel(a) => cancel(tx, tenant, project, auth, command, ownership, a).await?,
        Action::Start(a) => {
            lifecycle::start(tx, tenant, project, auth, command, ownership, contract, a).await?
        }
        Action::Report(a) => {
            lifecycle::report(tx, tenant, project, auth, command, ownership, contract, a).await?
        }
        Action::Recovery(a) => {
            recovery::apply(tx, tenant, project, auth, command, ownership, a).await?
        }
    };
    Ok(Applied {
        data,
        preceding_events: vec![],
    })
}

async fn prepare(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    contract: &awr_team::WorkContract,
    a: Prepare,
) -> PgResult<Value> {
    require_enabled(tx, tenant, project, auth, command).await?;
    let fence = claims::require_live(
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
        "",
    )
    .await?;
    require_clear_of_selective_blocks(tx, tenant, project, &command.work_id).await?;
    require_paths(contract, &a.declared_scope)?;
    if contract.execution_settlement.is_some() {
        settlement::normalized_scope(&a.declared_scope)?;
    }
    let id = crate::tx::new_id();
    let declared = json!(a.declared_scope);
    let settlement_policy = contract.execution_settlement.as_ref().map(|p| json!(p));
    let executor_origin = if contract.completion_policy == crate::review::simulated_member::POLICY {
        let origin = crate::review::simulated_member::capture(tx, tenant, project, auth).await?;
        origin.require_simulated_agent()?;
        Some(origin)
    } else {
        None
    };
    let executor_origin_json = executor_origin.as_ref().map(|o| json!(o));
    tx.execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,session_id,claim_id,fence,
        contract_hash,input_digest,executor_actor_id,state,effect_key,fencing_class,declared_scope_json,
        scope_id,coordinator_epoch,workstream_id,ownership_version,executor_client_id,settlement_policy_json,executor_origin_json)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'prepared',$3,'uncontrolled',$11,'main',$12,$13,$14,$15,$16,$17)",
        &[&tenant,&project,&id,&command.work_id,&a.session_id,&a.claim_id,&fence,
          &command.expected_contract_hash,&a.input_digest,&auth.actor_id,&declared,&auth.epoch,
          &command.workstream_id.to_string(),&ownership,&auth.client_id,&settlement_policy,&executor_origin_json]).await?;
    // No outbox row: an intent cannot be mistaken for an admitted dispatch.
    let work_version = advance_work(tx, tenant, project, &command.work_id).await?;
    let mut data = json!({"execution_id":id,"execution_version":"1","session_id":a.session_id,"claim_id":a.claim_id,
        "fence":fence.to_string(),"state":"prepared","effect_key":id,"input_digest":a.input_digest,
        "contract_hash":command.expected_contract_hash,"work_version":work_version.to_string(),
        "dispatched":false,"admission":"not_evaluated","fencing_class":"uncontrolled",
        "exactly_once_supported":false,"scope_validation":"lexical_contract_only",
        "settlement_policy":settlement_policy});
    crate::review::simulated_member::add_summary(&mut data, executor_origin.as_ref());
    Ok(data)
}

/// Refuse prepare/start when an active planning-change block or invalid
/// dependency binding targets this work. Live check in the admission
/// transaction — a prior Allow boundary receipt is not fresh permission.
pub(crate) async fn require_clear_of_selective_blocks(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<()> {
    crate::source::writeback::admission::require_effect_clear(tx, tenant, project, work).await?;
    if let Some(change_id) = tx
        .query_opt(
            "SELECT change_id FROM awr_team.planning_change_action_blocks
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND active=true
             ORDER BY change_id LIMIT 1",
            &[&tenant, &project, &work],
        )
        .await?
        .map(|r| r.get::<_, String>(0))
    {
        return Err(PgError::ActionBlockedByInvalidation(change_id));
    }
    let invalid: bool = tx
        .query_one(
            "SELECT EXISTS(
                SELECT 1 FROM awr_team.dependency_bindings
                 WHERE tenant_id=$1 AND project_id=$2
                   AND downstream_work_id=$3 AND valid=false)",
            &[&tenant, &project, &work],
        )
        .await?
        .get(0);
    if invalid {
        return Err(PgError::ActionBlockedByInvalidation(
            "invalid_dependency_binding".into(),
        ));
    }
    Ok(())
}

async fn require_enabled(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
) -> PgResult<()> {
    let enabled: bool = tx.query_one("SELECT c.definition_state='enabled' AND s.status='active'
        FROM awr_team.work_contracts c JOIN awr_team.work_scopes s
          ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.scope_id
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main' AND c.work_id=$4",
        &[&tenant,&project,&auth.snapshot,&command.work_id]).await?.get(0);
    if !enabled {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}

async fn require_ready(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    expected_version: &str,
    except_execution: &str,
) -> PgResult<()> {
    let r = tx.query_one("SELECT state,work_version,recovery_blocked,selected_completion_id
        FROM awr_team.work_runtime WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3 FOR UPDATE",
        &[&tenant,&project,&work]).await?;
    if r.get::<_, i64>(1) != version(expected_version)?
        || r.get::<_, i64>(1) == i64::MAX
        || matches!(
            r.get::<_, String>(0).as_str(),
            "completed" | "cancelled" | "archived"
        )
        || r.get::<_, Option<String>>(3).is_some()
    {
        return Err(PgError::PreconditionsChanged);
    }
    let unresolved: bool = tx.query_one("SELECT
        EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
            AND id<>$4 AND state NOT IN ('succeeded','failed','cancelled')) OR
        EXISTS(SELECT 1 FROM awr_team.resource_reservations WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown')",
        &[&tenant,&project,&work,&except_execution]).await?.get(0);
    if r.get::<_, bool>(2) || unresolved {
        return Err(PgError::RecoveryBlocked);
    }
    let waiting: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.wait_items
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='open')",
            &[&tenant, &project, &work],
        )
        .await?
        .get(0);
    if waiting {
        return Err(PgError::WaitOpen);
    }
    Ok(())
}

fn require_paths(contract: &awr_team::WorkContract, paths: &[String]) -> PgResult<()> {
    if paths.iter().any(|p| {
        !canonical_path(p)
            || !contract
                .scope_paths
                .iter()
                .any(|s| canonical_path(s) && crate::graph::path_within_scope(s, p))
    }) {
        return Err(PgError::ScopeExceeded);
    }
    Ok(())
}

async fn cancel(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    a: Cancel,
) -> PgResult<Value> {
    let r = load(tx, tenant, project, &a.execution_id).await?;
    require_binding(
        &r,
        &command.work_id,
        &command.workstream_id.to_string(),
        ownership,
    )?;
    if r.get::<_, Option<String>>("session_id").as_deref() != Some(&a.session_id)
        || r.get::<_, String>("executor_actor_id") != auth.actor_id
        || r.get::<_, Option<String>>("executor_client_id").as_deref() != Some(&auth.client_id)
    {
        return Err(PgError::Forbidden);
    }
    if r.get::<_, Option<String>>("coordinator_epoch").as_deref() != Some(&auth.epoch) {
        return Err(PgError::EpochChanged);
    }
    let ev: i64 = r.get("execution_version");
    if ev != version(&a.expected_execution_version)? || ev == i64::MAX {
        return Err(PgError::PreconditionsChanged);
    }
    let state: String = r.get("state");
    if matches!(state.as_str(), "succeeded" | "failed" | "cancelled") {
        return Err(PgError::PreconditionsChanged);
    }
    // Only an intent never exposed to a dispatcher/executor can be cancelled
    // synchronously. An acknowledged request is not a confirmed external stop.
    let exposed: bool = tx.query_one("SELECT
        EXISTS(SELECT 1 FROM awr_team.outbox WHERE tenant_id=$1 AND project_id=$2 AND aggregate_id=$3) OR
        EXISTS(SELECT 1 FROM awr_team.execution_receipts WHERE tenant_id=$1 AND project_id=$2 AND execution_id=$3)",
        &[&tenant,&project,&a.execution_id]).await?.get(0);
    let stopped = state == "prepared" && !exposed;
    let next = if stopped { "cancelled" } else { &state };
    tx.execute("UPDATE awr_team.executions SET state=$4,cancel_requested=true,execution_version=execution_version+1
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",&[&tenant,&project,&a.execution_id,&next]).await?;
    let work_version = advance_work(tx, tenant, project, &command.work_id).await?;
    Ok(
        json!({"execution_id":a.execution_id,"execution_version":(ev+1).to_string(),"session_id":a.session_id,
        "state":next,"cancel_requested":true,"stop_confirmed":stopped,
        "work_version":work_version.to_string(),"resource_release_performed":false}),
    )
}

async fn advance_work(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<i64> {
    Ok(tx
        .query_opt(
            "UPDATE awr_team.work_runtime SET work_version=work_version+1
        WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3
          AND work_version>=0 AND work_version<9223372036854775807 RETURNING work_version",
            &[&tenant, &project, &work],
        )
        .await?
        .ok_or(PgError::PreconditionsChanged)?
        .get(0))
}

async fn load(tx: &Transaction<'_>, tenant: &str, project: &str, id: &str) -> PgResult<Row> {
    tx.query_opt("SELECT e.*,s.actor_id AS session_actor,s.client_id AS session_client,
        s.work_id AS session_work,s.workstream_id AS session_stream,s.ownership_version AS session_ownership,
        c.work_id AS claim_work,c.session_id AS claim_session,c.actor_id AS claim_actor,
        c.workstream_id AS claim_stream,c.ownership_version AS claim_ownership,c.fence AS claim_fence,
        c.coordinator_epoch AS claim_epoch,c.lease_version AS claim_lease_version,
        (c.state='active' AND c.expires_at>clock_timestamp() AND s.state='active' AND w.last_fence=e.fence) AS lease_live,
        w.recovery_blocked,w.recovery_execution_id,w.recovery_receipt_id,w.last_fence AS work_last_fence
        FROM awr_team.executions e
        JOIN awr_team.sessions s ON s.tenant_id=e.tenant_id AND s.project_id=e.project_id AND s.id=e.session_id AND s.scope_id=e.scope_id
        JOIN awr_team.claims c ON c.tenant_id=e.tenant_id AND c.project_id=e.project_id AND c.id=e.claim_id AND c.scope_id=e.scope_id
        JOIN awr_team.work_runtime w ON w.tenant_id=e.tenant_id AND w.project_id=e.project_id AND w.work_id=e.work_id AND w.scope_id=e.scope_id
        WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.id=$3 AND e.scope_id='main'",
        &[&tenant,&project,&id]).await?.ok_or(PgError::Forbidden)
}

fn require_binding(r: &Row, work: &str, stream: &str, ownership: i64) -> PgResult<()> {
    if r.get::<_, String>("work_id") != work
        || r.get::<_, String>("session_work") != work
        || r.get::<_, String>("claim_work") != work
        || r.get::<_, Option<String>>("session_id").as_deref()
            != Some(r.get::<_, String>("claim_session").as_str())
        || r.get::<_, String>("executor_actor_id") != r.get::<_, String>("session_actor")
        || r.get::<_, String>("executor_actor_id") != r.get::<_, String>("claim_actor")
        || r.get::<_, Option<String>>("executor_client_id").as_deref()
            != Some(r.get::<_, String>("session_client").as_str())
        || ["workstream_id", "session_stream", "claim_stream"]
            .iter()
            .any(|k| r.get::<_, Option<String>>(*k).as_deref() != Some(stream))
        || ["ownership_version", "session_ownership", "claim_ownership"]
            .iter()
            .any(|k| r.get::<_, Option<i64>>(*k) != Some(ownership))
        || r.get::<_, i64>("fence") != r.get::<_, i64>("claim_fence")
        || r.get::<_, Option<String>>("coordinator_epoch").is_none()
        || r.get::<_, Option<String>>("coordinator_epoch")
            != r.get::<_, Option<String>>("claim_epoch")
    {
        return Err(PgError::Forbidden);
    }
    Ok(())
}

pub(crate) async fn inspect(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
    ownership: i64,
    id: &str,
    session: Option<&str>,
) -> PgResult<Value> {
    let r = load(tx, tenant, project, id).await?;
    require_binding(&r, work, stream, ownership)?;
    if session.is_some_and(|s| r.get::<_, Option<String>>("session_id").as_deref() != Some(s)) {
        return Err(PgError::Forbidden);
    }
    let epoch_matches =
        r.get::<_, Option<String>>("coordinator_epoch").as_deref() == Some(&auth.epoch);
    let current_hash: String = tx
        .query_one(
            "SELECT contract_hash FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &work],
        )
        .await?
        .get(0);
    let owned = r.get::<_, String>("executor_actor_id") == auth.actor_id
        && r.get::<_, Option<String>>("executor_client_id").as_deref() == Some(&auth.client_id);
    let stream_id: Id = stream.parse().map_err(|_| PgError::Forbidden)?;
    let authority = auth
        .execution_access
        .get(&stream_id)
        .copied()
        .unwrap_or_default();
    let receipt_visible = owned || authority.reconcile;
    let latest = recovery::latest_receipt(tx, tenant, project, id).await?;
    let controlled_confirmation_available = recovery_cause::available(
        tx,
        tenant,
        project,
        auth,
        &stream_id,
        &current_hash,
        &r,
        latest.as_ref(),
    )
    .await?;
    let terminal = matches!(
        r.get::<_, String>("state").as_str(),
        "succeeded" | "failed" | "cancelled"
    );
    let valid_receipt = latest.as_ref().is_some_and(|v| {
        awr_team::request_hash(&v["payload"]).ok().as_deref() == v["digest"].as_str()
    });
    let workspace_settled = terminal && r.get::<_, bool>("workspace_effects_settled");
    let confirmed_settled = terminal
        && valid_receipt
        && latest.as_ref().is_some_and(|v| {
            matches!(
                v["receipt_kind"].as_str(),
                Some("trusted_executor" | "reconcile")
            ) && v["payload"]["effects_settled"] == true
        });
    let artifact_verified = accepted_artifact(tx, tenant, project, work, id, &current_hash).await?;
    let visible_latest = if receipt_visible {
        latest.as_ref()
    } else {
        None
    };
    Ok(
        json!({"execution_id":id,"execution_version":r.get::<_,i64>("execution_version").to_string(),
        "work_id":work,"session_id":r.get::<_,Option<String>>("session_id"),"claim_id":r.get::<_,Option<String>>("claim_id"),
        "fence":r.get::<_,i64>("fence").to_string(),"state":r.get::<_,String>("state"),
        "cancel_requested":r.get::<_,bool>("cancel_requested"),"contract_hash":r.get::<_,String>("contract_hash"),
        "contract_matches_current":r.get::<_,String>("contract_hash")==current_hash,
        "epoch_matches_current":epoch_matches,"lease_live":r.get::<_,bool>("lease_live")&&epoch_matches,
        "execution_coordinator_epoch":r.get::<_,Option<String>>("coordinator_epoch"),
        "owned_by_client":owned,"attestation_authority":authority.attest && owned && epoch_matches && recovery_cause::grant_matches(&r,auth,&stream_id),
        "reconciliation_authority":authority.reconcile,"receipt_details_available":receipt_visible,
        "previous_epoch_review_required":!epoch_matches,
        "previous_epoch_recovery_available":!epoch_matches && authority.reconcile && r.get::<_,Option<String>>("coordinator_epoch").is_some(),
        "latest_receipt":visible_latest,
        "terminal_reported":r.get::<_,bool>("terminal_reported"),"artifact_verified":artifact_verified,
        "artifact_verification_basis":if artifact_verified {Some("current_completion_readable_artifact_digest")} else {None},
        "effects_settled":workspace_settled || confirmed_settled,
        "settlement_scope":if workspace_settled {Some("admitted_workspace_paths")} else if confirmed_settled {Some("authorized_bound_resources")} else {None},
        "settlement_basis":if workspace_settled {Some("caller_asserted")} else if confirmed_settled {latest.as_ref().and_then(|v|v["receipt_kind"].as_str())} else {None},
        "controlled_confirmation_available":controlled_confirmation_available,
        "recovery_cause":if !r.get::<_,bool>("recovery_blocked") {"none"} else if r.get::<_,Option<String>>("recovery_execution_id").as_deref()==Some(id) {"attributed_execution_report"} else {"unattributed_or_other_execution"},
        "recovery_blocked":r.get::<_,bool>("recovery_blocked"),
        "execution_authorized":false,"automatic_resume":false}),
    )
}

/// Verification is a current accepted artifact binding, not a terminal report.
async fn accepted_artifact(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    execution: &str,
    contract: &str,
) -> PgResult<bool> {
    let row = tx.query_opt("SELECT e.digest,e.input_digest,e.output_digest,e.execution_result_digest,
          e.payload_json,a.sha256,a.content,a.state
        FROM awr_team.work_runtime w JOIN awr_team.completion_receipts c
          ON c.tenant_id=w.tenant_id AND c.project_id=w.project_id AND c.id=w.selected_completion_id
        JOIN awr_team.evidence e ON e.tenant_id=c.tenant_id AND e.project_id=c.project_id AND e.id=c.evidence_id
        JOIN awr_team.artifacts a ON a.tenant_id=e.tenant_id AND a.project_id=e.project_id AND a.id=e.artifact_id
        WHERE w.tenant_id=$1 AND w.project_id=$2 AND w.scope_id='main' AND w.work_id=$3 AND w.state='completed'
          AND c.work_id=$3 AND c.execution_id=$4 AND e.execution_id=$4 AND e.work_id=$3
          AND c.contract_hash=$5 AND e.contract_hash=$5
          AND c.result_digest=e.digest AND c.evidence_bundle_hash=e.digest", &[&tenant,&project,&work,&execution,&contract]).await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let Some(content) = row.get::<_, Option<Vec<u8>>>(6) else {
        return Ok(false);
    };
    let input: Option<String> = row.get(1);
    let output: Option<String> = row.get(2);
    let result: Option<String> = row.get(3);
    let recomputed = crate::review::evidence_digest(
        work,
        contract,
        input.as_deref(),
        output.as_deref(),
        result.as_deref(),
        &row.get(4),
    )?;
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(&content));
    Ok(row.get::<_, String>(7) == "finalized"
        && row.get::<_, String>(5) == digest
        && row.get::<_, String>(0) == recomputed
        && output.as_deref() == Some(&digest))
}
