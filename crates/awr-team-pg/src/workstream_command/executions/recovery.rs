//! Explicit operator-issued authority, resolved under the command's auth locks.
//! Reconciliation settles effects; it does not approve work or upgrade its policy.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Facts {
    outcome: String,
    input_digest: String,
    output_digest: Option<String>,
    environment_digest: String,
    observed_paths: Vec<String>,
    note: String,
    executor_stopped: Option<bool>,
}
impl Facts {
    fn validate(&self) -> PgResult<()> {
        if !matches!(
            self.outcome.as_str(),
            "succeeded" | "failed" | "cancelled" | "unknown"
        ) || !digest(&self.input_digest)
            || !digest(&self.environment_digest)
            || self.output_digest.as_ref().is_some_and(|s| !digest(s))
            || (self.outcome == "succeeded" && self.output_digest.is_none())
            || self.observed_paths.len() > 128
            || self.observed_paths.iter().any(|p| !canonical_path(p))
            || self.note.trim().is_empty()
            || self.note.len() > 4096
            || self.note.contains('\0')
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attest {
    session_id: String,
    expected_session_version: String,
    execution_id: String,
    expected_execution_version: String,
    reviewed_receipt_id: Option<String>,
    facts: Facts,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reconcile {
    session_id: String,
    expected_session_version: String,
    execution_id: String,
    expected_execution_version: String,
    expected_work_version: String,
    reviewed_receipt_id: Option<String>,
    clear_recovery_block: bool,
    previous_epoch_recovery: Option<PreviousEpochRecovery>,
    facts: Facts,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreviousEpochRecovery {
    execution_epoch: String,
    executor_stopped: bool,
    review_reference: String,
}
pub(crate) enum Action {
    Attest(Attest),
    Reconcile(Reconcile),
}
impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        let a = match op {
            "execution.attest" => {
                Self::Attest(serde_json::from_value(args).map_err(|_| invalid())?)
            }
            "execution.reconcile" => {
                Self::Reconcile(serde_json::from_value(args).map_err(|_| invalid())?)
            }
            _ => return Err(invalid()),
        };
        let (id, v, facts) = a.execution();
        if !identity(id) || version(v)? == 0 {
            return Err(invalid());
        }
        facts.validate()?;
        if let Self::Attest(a) = &a {
            if a.reviewed_receipt_id.as_ref().is_some_and(|s| !identity(s)) {
                return Err(invalid());
            }
        }
        if let Self::Reconcile(r) = &a {
            if version(&r.expected_work_version)? == 0
                || r.reviewed_receipt_id.as_ref().is_some_and(|s| !identity(s))
                || (r.clear_recovery_block && r.facts.outcome == "unknown")
            {
                return Err(invalid());
            }
            if let Some(review) = &r.previous_epoch_recovery {
                if !identity(&review.execution_epoch)
                    || review.review_reference.trim().is_empty()
                    || review.review_reference.len() > 2048
                    || review.review_reference.chars().any(char::is_control)
                    || (r.facts.outcome != "unknown" && !review.executor_stopped)
                {
                    return Err(invalid());
                }
            }
        }
        Ok(a)
    }
    pub(super) fn session(&self) -> (&str, &str) {
        match self {
            Self::Attest(a) => (&a.session_id, &a.expected_session_version),
            Self::Reconcile(a) => (&a.session_id, &a.expected_session_version),
        }
    }
    fn execution(&self) -> (&str, &str, &Facts) {
        match self {
            Self::Attest(a) => (&a.execution_id, &a.expected_execution_version, &a.facts),
            Self::Reconcile(a) => (&a.execution_id, &a.expected_execution_version, &a.facts),
        }
    }
}

pub(super) async fn latest_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    execution: &str,
) -> PgResult<Option<Value>> {
    Ok(tx.query_opt("SELECT id,receipt_kind,digest,payload_json FROM awr_team.execution_receipts
        WHERE tenant_id=$1 AND project_id=$2 AND execution_id=$3 ORDER BY created_at DESC,id DESC LIMIT 1",
        &[&tenant,&project,&execution]).await?.map(|r| json!({"receipt_id":r.get::<_,String>(0),
            "receipt_kind":r.get::<_,String>(1),"digest":r.get::<_,String>(2),"payload":r.get::<_,Value>(3)})))
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    action: Action,
) -> PgResult<Value> {
    let access = auth
        .execution_access
        .get(&command.workstream_id)
        .copied()
        .unwrap_or_default();
    let reconcile = matches!(action, Action::Reconcile(_));
    if if reconcile {
        !access.reconcile
    } else {
        !access.attest
    } {
        return Err(PgError::Forbidden);
    }
    let (id, expected_version, facts) = action.execution();
    let r = load(tx, tenant, project, id).await?;
    if !reconcile && !recovery_cause::grant_matches(&r, auth, &command.workstream_id) {
        return Err(PgError::Forbidden);
    }
    require_binding(
        &r,
        &command.work_id,
        &command.workstream_id.to_string(),
        ownership,
    )?;
    if !reconcile
        && (r.get::<_, String>("executor_actor_id") != auth.actor_id
            || r.get::<_, Option<String>>("executor_client_id").as_deref() != Some(&auth.client_id)
            || r.get::<_, Option<String>>("session_id").as_deref() != Some(action.session().0))
    {
        return Err(PgError::Forbidden);
    }
    let execution_epoch: Option<String> = r.get("coordinator_epoch");
    let epoch_review = match &action {
        Action::Reconcile(a) => a.previous_epoch_recovery.as_ref(),
        _ => None,
    };
    if execution_epoch.as_deref() != Some(&auth.epoch) {
        // Only a currently authorized recovery operator can explicitly review an
        // attributed old epoch. Missing legacy attribution is never inferred.
        if !reconcile
            || execution_epoch
                .as_deref()
                .is_none_or(|old| epoch_review.is_none_or(|review| review.execution_epoch != old))
        {
            return Err(PgError::EpochChanged);
        }
    } else if epoch_review.is_some() {
        // A stale recovery plan cannot silently become an ordinary settlement.
        return Err(PgError::PreconditionsChanged);
    }
    let ev: i64 = r.get("execution_version");
    if ev != version(expected_version)?
        || ev == i64::MAX
        || r.get::<_, Option<String>>("input_digest").as_deref() != Some(&facts.input_digest)
    {
        return Err(PgError::PreconditionsChanged);
    }
    let current: String = r.get("state");
    let terminal = matches!(current.as_str(), "succeeded" | "failed" | "cancelled");
    if !matches!(current.as_str(), "running" | "unknown") && !(reconcile && terminal) {
        return Err(PgError::PreconditionsChanged);
    }
    if terminal
        && (current != facts.outcome
            || r.get::<_, Option<String>>("result_digest") != facts.output_digest
            || r.get::<_, Option<String>>("environment_digest").as_deref()
                != Some(&facts.environment_digest)
            || r.get::<_, Option<Value>>("observed_paths_json")
                != Some(json!(facts.observed_paths)))
    {
        return Err(PgError::PreconditionsChanged);
    }
    let mut clear = false;
    if let Action::Reconcile(a) = &action {
        let w = tx
            .query_one(
                "SELECT work_version FROM awr_team.work_runtime
            WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3 FOR UPDATE",
                &[&tenant, &project, &command.work_id],
            )
            .await?;
        let latest = latest_receipt(tx, tenant, project, id).await?;
        let latest_id = latest.as_ref().and_then(|r| r["receipt_id"].as_str());
        if w.get::<_, i64>(0) != version(&a.expected_work_version)?
            || latest_id != a.reviewed_receipt_id.as_deref()
        {
            return Err(PgError::PreconditionsChanged);
        }
        clear = a.clear_recovery_block;
    }
    let paths: Vec<String> = serde_json::from_value(r.get("declared_scope_json"))
        .map_err(|_| PgError::SourceDivergence)?;
    let exceeded = facts.observed_paths.iter().any(|p| {
        !paths
            .iter()
            .any(|s| canonical_path(s) && crate::graph::path_within_scope(s, p))
    });
    // A runner reporting effects outside its declared scope needs operator review.
    // A reconciler may settle a failed/cancelled attempt, never certify such a success.
    if reconcile && exceeded && facts.outcome == "succeeded" {
        return Err(PgError::ScopeExceeded);
    }
    // Absence preserves the legacy privileged settlement contract. An explicit
    // observation that the executor is still running cannot release resources.
    let settled = facts.outcome != "unknown"
        && facts.executor_stopped != Some(false)
        && (reconcile || !exceeded);
    let latest = latest_receipt(tx, tenant, project, id).await?;
    let controlled_clear = if let Action::Attest(a) = &action {
        settled
            && facts.executor_stopped == Some(true)
            && a.reviewed_receipt_id.is_some()
            && a.reviewed_receipt_id.as_deref()
                == latest.as_ref().and_then(|v| v["receipt_id"].as_str())
            && recovery_cause::available(
                tx,
                tenant,
                project,
                auth,
                &command.workstream_id,
                &command.expected_contract_hash,
                &r,
                latest.as_ref(),
            )
            .await?
    } else {
        false
    };
    let next = if settled {
        facts.outcome.as_str()
    } else {
        "unknown"
    };
    let kind = if reconcile {
        "reconcile"
    } else {
        "trusted_executor"
    };
    let receipt_id = crate::tx::new_id();
    let mut payload = json!({"outcome":facts.outcome,"input_digest":facts.input_digest,"output_digest":facts.output_digest,
        "environment_digest":facts.environment_digest,"observed_paths":facts.observed_paths,"note":facts.note,
        "scope_violation":exceeded,"effects_settled":settled,"client_id":auth.client_id,"actor_kind":auth.actor_kind,
        "session_id":action.session().0,"execution_session_id":r.get::<_,Option<String>>("session_id"),
        "execution_version":expected_version,"workstream_id":command.workstream_id,"ownership_version":ownership.to_string(),
        "contract_hash":r.get::<_,String>("contract_hash"),"coordinator_epoch":auth.epoch,
        "execution_coordinator_epoch":execution_epoch,"previous_epoch_recovery":epoch_review,
        "recovery_review_basis":if epoch_review.is_some() {Some("authorized_operator_assertion")} else {None},
        "grant_version":auth.grant_versions[&command.workstream_id].to_string(),
        "admission_attestation_grant_version":r.get::<_,Option<i64>>("attestation_grant_version").map(|v|v.to_string()),
        "reviewed_receipt_id":match &action { Action::Reconcile(a)=>a.reviewed_receipt_id.as_deref(), Action::Attest(a)=>a.reviewed_receipt_id.as_deref() }});
    if let Some(stopped) = facts.executor_stopped {
        payload["executor_stopped"] = json!(stopped);
    }
    if controlled_clear {
        payload["recovery_clear_basis"] = json!("controlled_run_report_receipt");
    }
    let hash = awr_team::request_hash(&payload).map_err(|_| invalid())?;
    tx.execute("INSERT INTO awr_team.execution_receipts(tenant_id,project_id,id,execution_id,reporter_actor_id,receipt_kind,digest,payload_json)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8)", &[&tenant,&project,&receipt_id,&id,&auth.actor_id,&kind,&hash,&payload]).await?;
    let reason = if settled {
        None
    } else if facts.executor_stopped == Some(false) {
        Some("executor_not_stopped")
    } else if exceeded {
        Some("scope_violation_requires_reconciliation")
    } else {
        Some("executor_effects_unknown")
    };
    tx.execute("UPDATE awr_team.executions SET state=$4,result_digest=$5,environment_digest=$6,observed_paths_json=$7,
        unknown_reason=$8,execution_version=execution_version+1,terminal_reported=terminal_reported OR $9
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant,&project,&id,&next,&facts.output_digest,&facts.environment_digest,&json!(facts.observed_paths),&reason,&(facts.outcome!="unknown")]).await?;
    let resource_state = if settled { "released" } else { "unknown" };
    let affected = tx.execute("UPDATE awr_team.resource_reservations SET state=$5
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND execution_id=$4 AND state IN ('reserved','unknown')",
        &[&tenant,&project,&command.work_id,&id,&resource_state]).await?;
    // Never release legacy/unbound or another attempt's reservations. A partial
    // settlement is committed while the remaining work-wide barrier stays put.
    let remaining: bool = tx.query_one("SELECT
        EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
            AND id<>$4 AND state NOT IN ('succeeded','failed','cancelled')) OR
        EXISTS(SELECT 1 FROM awr_team.resource_reservations WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
            AND state IN ('reserved','unknown'))", &[&tenant,&project,&command.work_id,&id]).await?.get(0);
    if !settled || remaining {
        tx.execute(
            "UPDATE awr_team.work_runtime SET recovery_blocked=true
            WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?;
    } else if clear || controlled_clear {
        tx.execute(
            "UPDATE awr_team.work_runtime SET recovery_blocked=false
            WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?;
    }
    let blocked: bool = tx
        .query_one(
            "SELECT recovery_blocked FROM awr_team.work_runtime
        WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    let wv = advance_work(tx, tenant, project, &command.work_id).await?;
    Ok(
        json!({"execution_id":id,"execution_version":(ev+1).to_string(),"work_version":wv.to_string(),
        "receipt_id":receipt_id,"receipt_kind":kind,"state":next,"effects_settled":settled,"scope_violation":exceeded,
        "resources_released":if settled {affected} else {0},"recovery_blocked":blocked,
        "terminal_reported":r.get::<_,bool>("terminal_reported") || facts.outcome!="unknown",
        "artifact_verified":false,"controlled_recovery_cleared":controlled_clear && !blocked,
        "recovery_clear_requested":clear,"unresolved_work_effects":remaining,"work_completed":false,
        "execution_coordinator_epoch":execution_epoch,"reporting_coordinator_epoch":auth.epoch,
        "previous_epoch_reconciled":epoch_review.is_some(),
        "next_action":if blocked {"Inspect remaining effects and recovery responsibility; do not restart or complete work."}
            else {"Refresh work context and the live lease before another operation; settlement is not work acceptance."}}),
    )
}
