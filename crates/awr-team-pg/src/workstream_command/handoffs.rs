//! Confirmed Team handoff commands (WS-017) under the authenticated command TX.
use super::*;
use awr_core::{
    AcceptHandoffRequest, CancelHandoffRequest, ExecutionInstance, HandoffKind, HandoffPackage,
    HandoffStatus, InspectHandoffRequest, PersonId, ProposeHandoffRequest, RejectHandoffRequest,
    TeamHandoff, TimeoutHandoffRequest, apply_handoff_accept, apply_handoff_cancel,
    apply_handoff_inspect, apply_handoff_propose, apply_handoff_reject, apply_handoff_timeout,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Propose {
    session_id: String,
    expected_session_version: String,
    handoff_id: String,
    kind: String,
    to_person_id: String,
    package: Option<HandoffPackage>,
    proposed_successor: Option<ExecutionInstance>,
    proposer_execution_id: Option<String>,
    proposer_fence: Option<String>,
    expires_at_ms: Option<i64>,
    #[serde(default, rename = "now_ms")]
    _reported_now_ms: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inspect {
    session_id: String,
    expected_session_version: String,
    handoff_id: String,
    expected_handoff_version: String,
    inspector_person_id: String,
    #[serde(default, rename = "now_ms")]
    _reported_now_ms: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Accept {
    session_id: String,
    expected_session_version: String,
    handoff_id: String,
    expected_handoff_version: String,
    acceptor_person_id: String,
    successor_execution: ExecutionInstance,
    inspection_request_id: Option<String>,
    #[serde(default, rename = "prior_execution_stopped")]
    _reported_prior_execution_stopped: Option<bool>,
    #[serde(default, rename = "prior_reconciled")]
    _reported_prior_reconciled: Option<bool>,
    #[serde(default, rename = "context_reprepared")]
    _reported_context_reprepared: Option<bool>,
    expected_current_fence: Option<String>,
    #[serde(default, rename = "now_ms")]
    _reported_now_ms: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reject {
    session_id: String,
    expected_session_version: String,
    handoff_id: String,
    expected_handoff_version: String,
    by_person_id: String,
    reason: String,
    #[serde(default, rename = "now_ms")]
    _reported_now_ms: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cancel {
    session_id: String,
    expected_session_version: String,
    handoff_id: String,
    expected_handoff_version: String,
    by_person_id: String,
    reason: String,
    #[serde(default, rename = "now_ms")]
    _reported_now_ms: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Timeout {
    session_id: String,
    expected_session_version: String,
    handoff_id: String,
    expected_handoff_version: String,
    #[serde(default, rename = "now_ms")]
    _reported_now_ms: Option<i64>,
}

pub(super) enum Action {
    Propose(Propose),
    Inspect(Inspect),
    Accept(Accept),
    Reject(Reject),
    Cancel(Cancel),
    Timeout(Timeout),
}

impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        let action = match op {
            "handoff.propose" => {
                let a: Propose = serde_json::from_value(args).map_err(|_| invalid())?;
                parse_kind(&a.kind)?;
                if let Some(package) = &a.package {
                    package.validate().map_err(|_| invalid())?;
                }
                if !identity(&a.handoff_id) || !identity(&a.to_person_id) {
                    return Err(invalid());
                }
                if let Some(f) = &a.proposer_fence {
                    version(f)?;
                }
                Self::Propose(a)
            }
            "handoff.inspect" => {
                let a: Inspect = serde_json::from_value(args).map_err(|_| invalid())?;
                if !identity(&a.handoff_id)
                    || !identity(&a.inspector_person_id)
                    || version(&a.expected_handoff_version)? == 0
                {
                    return Err(invalid());
                }
                Self::Inspect(a)
            }
            "handoff.accept" => {
                let a: Accept = serde_json::from_value(args).map_err(|_| invalid())?;
                a.successor_execution.validate().map_err(|_| invalid())?;
                if !identity(&a.handoff_id)
                    || !identity(&a.acceptor_person_id)
                    || version(&a.expected_handoff_version)? == 0
                {
                    return Err(invalid());
                }
                if let Some(f) = &a.expected_current_fence {
                    version(f)?;
                }
                if a.inspection_request_id
                    .as_deref()
                    .is_some_and(|id| !identity(id))
                {
                    return Err(invalid());
                }
                Self::Accept(a)
            }
            "handoff.reject" => {
                let a: Reject = serde_json::from_value(args).map_err(|_| invalid())?;
                if !identity(&a.handoff_id)
                    || !identity(&a.by_person_id)
                    || version(&a.expected_handoff_version)? == 0
                    || a.reason.trim().is_empty()
                    || a.reason.len() > 2048
                {
                    return Err(invalid());
                }
                Self::Reject(a)
            }
            "handoff.cancel" => {
                let a: Cancel = serde_json::from_value(args).map_err(|_| invalid())?;
                if !identity(&a.handoff_id)
                    || !identity(&a.by_person_id)
                    || version(&a.expected_handoff_version)? == 0
                    || a.reason.trim().is_empty()
                    || a.reason.len() > 2048
                {
                    return Err(invalid());
                }
                Self::Cancel(a)
            }
            "handoff.timeout" => {
                let a: Timeout = serde_json::from_value(args).map_err(|_| invalid())?;
                if !identity(&a.handoff_id) || version(&a.expected_handoff_version)? == 0 {
                    return Err(invalid());
                }
                Self::Timeout(a)
            }
            _ => return Err(invalid()),
        };
        let (session, expected) = action.session();
        if !identity(session) || version(expected)? == 0 {
            return Err(invalid());
        }
        Ok(action)
    }

    fn session(&self) -> (&str, &str) {
        match self {
            Self::Propose(a) => (&a.session_id, &a.expected_session_version),
            Self::Inspect(a) => (&a.session_id, &a.expected_session_version),
            Self::Accept(a) => (&a.session_id, &a.expected_session_version),
            Self::Reject(a) => (&a.session_id, &a.expected_session_version),
            Self::Cancel(a) => (&a.session_id, &a.expected_session_version),
            Self::Timeout(a) => (&a.session_id, &a.expected_session_version),
        }
    }

    pub(super) fn requires_active_stream(&self) -> bool {
        matches!(self, Self::Propose(_) | Self::Accept(_) | Self::Inspect(_))
    }
}

fn parse_kind(kind: &str) -> PgResult<HandoffKind> {
    match kind {
        "execution" => Ok(HandoffKind::Execution),
        "responsibility" => Ok(HandoffKind::Responsibility),
        _ => Err(invalid()),
    }
}

fn person(id: &str) -> PgResult<PersonId> {
    PersonId::new(id).map_err(|_| invalid())
}

fn map_core(err: awr_core::Error) -> PgError {
    match err {
        awr_core::Error::RevisionConflict { .. } => PgError::PreconditionsChanged,
        awr_core::Error::ClaimConflict(_) => PgError::ClaimHeld,
        awr_core::Error::RuleViolation(_) => PgError::Forbidden,
        awr_core::Error::NotFound(_) => PgError::Protocol(format!("not found: {err}")),
        awr_core::Error::InvalidInput(m) => PgError::Protocol(m),
        other => PgError::Protocol(other.to_string()),
    }
}

fn status_str(s: HandoffStatus) -> &'static str {
    match s {
        HandoffStatus::Proposed => "proposed",
        HandoffStatus::Inspected => "inspected",
        HandoffStatus::Accepted => "accepted",
        HandoffStatus::Rejected => "rejected",
        HandoffStatus::Cancelled => "cancelled",
        HandoffStatus::TimedOut => "timed_out",
    }
}

fn kind_str(k: HandoffKind) -> &'static str {
    match k {
        HandoffKind::Execution => "execution",
        HandoffKind::Responsibility => "responsibility",
    }
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    bearer: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    action: Action,
) -> PgResult<Applied> {
    let (session_id, expected_session_version) = action.session();
    let session_version = session(
        tx,
        tenant,
        project,
        auth,
        command,
        session_id,
        expected_session_version,
        ownership,
    )
    .await?;
    let now_ms = crate::team_handoff::server_now_ms(tx).await?;
    if action.requires_active_stream() {
        let enabled: bool = tx
            .query_one(
                "SELECT c.definition_state='enabled' AND s.status='active'
            FROM awr_team.work_contracts c JOIN awr_team.work_scopes s
              ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.scope_id
            WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main' AND c.work_id=$4",
                &[&tenant, &project, &auth.snapshot, &command.work_id],
            )
            .await?
            .get(0);
        if !enabled {
            return Err(PgError::PreconditionsChanged);
        }
    }
    // Resolve the acting person from authenticated actor + explicit person↔agent binding.
    let actor = super::task_intake::actor_instance(tx, tenant, project, auth).await?;
    let actor_person = actor.person_id().clone();
    let mut inspection = None;
    let handoff = match action {
        Action::Propose(a) => {
            let kind = parse_kind(&a.kind)?;
            require_predecessor(tx, tenant, project, &command.work_id, kind, &actor).await?;
            let prepared = prepare(tx, tenant, project, bearer, command, &a.session_id).await?;
            let checkpoint = tx
                .query_one(
                    "SELECT latest_checkpoint_id FROM awr_team.sessions
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                    &[&tenant, &project, &a.session_id],
                )
                .await?
                .get::<_, Option<String>>(0)
                .ok_or(PgError::PreconditionsChanged)?;
            let (package, _) = canonical_package(
                tx,
                tenant,
                project,
                command,
                &prepared,
                &checkpoint,
                &actor,
                a.package.as_ref(),
            )
            .await?;
            if package.consumed_context_digest != prepared["data"]["context_hash"]
                || a.package
                    .as_ref()
                    .is_some_and(|supplied| supplied != &package)
            {
                return Err(PgError::PreconditionsChanged);
            }
            let latest_execution = tx
                .query_opt(
                    "SELECT id FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND session_id=$4
                   AND executor_actor_id=$5 AND executor_client_id=$6 ORDER BY id DESC LIMIT 1",
                    &[
                        &tenant,
                        &project,
                        &command.work_id,
                        &a.session_id,
                        &auth.actor_id,
                        &auth.client_id,
                    ],
                )
                .await?
                .map(|r| r.get::<_, String>(0));
            if a.proposer_execution_id
                .as_ref()
                .is_some_and(|id| Some(id) != latest_execution.as_ref())
            {
                return Err(PgError::PreconditionsChanged);
            }
            ensure_person(tx, tenant, project, actor_person.as_str()).await?;
            require_receiver(tx, tenant, project, &a.to_person_id).await?;
            let fence = live_fence(tx, tenant, project, &command.work_id).await?;
            if a.proposer_fence
                .as_ref()
                .is_some_and(|s| version(s).ok() != fence)
            {
                return Err(PgError::PreconditionsChanged);
            }
            let req = ProposeHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                kind,
                package,
                to_person_id: person(&a.to_person_id)?,
                proposed_successor: a.proposed_successor,
                proposer_execution_id: latest_execution,
                proposer_fence: fence,
                expires_at_ms: a.expires_at_ms,
                now_ms,
            };
            let h = apply_handoff_propose(project, &command.work_id, &actor_person, &req)
                .map_err(map_core)?;
            persist(tx, tenant, project, &h).await?;
            h
        }
        Action::Inspect(a) => {
            let mut before = load_for_update(tx, tenant, project, &a.handoff_id)
                .await?
                .filter(|h| h.work_item_id == command.work_id)
                .ok_or_else(PgError::handoff_unavailable)?;
            require_person_match(&actor_person, &a.inspector_person_id)?;
            if before
                .expires_at_ms
                .is_some_and(|expires| now_ms >= expires)
            {
                return Err(PgError::PreconditionsChanged);
            }
            require_predecessor(
                tx,
                tenant,
                project,
                &command.work_id,
                before.kind,
                &before.package.current_execution,
            )
            .await?;
            let prepared = prepare(tx, tenant, project, bearer, command, &a.session_id).await?;
            let (package, predecessor) = canonical_package(
                tx,
                tenant,
                project,
                command,
                &prepared,
                &before.package.checkpoint_ids[0],
                &before.package.current_execution,
                Some(&before.package),
            )
            .await?;
            let package_changed = before.package != package;
            let req = InspectHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                expected_version: version(&a.expected_handoff_version)? as u64,
                inspector_person_id: actor_person.clone(),
                now_ms,
            };
            let mut h = apply_handoff_inspect(&before, &req).map_err(map_core)?;
            if package_changed {
                before.package = package;
                h.package = before.package;
                if h.version == before.version {
                    h.version = h
                        .version
                        .checked_add(1)
                        .ok_or(PgError::PreconditionsChanged)?;
                    h.updated_at_ms = now_ms;
                }
            }
            let consumption = consumption_binding(
                auth,
                command,
                &h,
                &a.session_id,
                session_version,
                &actor,
                &prepared,
                &predecessor,
            )?;
            inspection = Some(json!({"handoff":h,"prepared_context":prepared,
                "consumption":consumption,"inspection_request_id":command.request_id,
                "receipt_proves":"package_delivered_to_authenticated_client"}));
            persist(tx, tenant, project, &h).await?;
            h
        }
        Action::Accept(a) => {
            let before = load_for_update(tx, tenant, project, &a.handoff_id)
                .await?
                .filter(|h| h.work_item_id == command.work_id)
                .ok_or_else(PgError::handoff_unavailable)?;
            let live = live_fence(tx, tenant, project, &command.work_id).await?;
            let expected_fence = a
                .expected_current_fence
                .as_ref()
                .map(|s| version(s))
                .transpose()?;
            require_person_match(&actor_person, &a.acceptor_person_id)?;
            if live.is_some() && expected_fence.is_none() {
                return Err(PgError::missing_handoff_fence());
            }
            crate::source::require_work_settled(tx, tenant, project, &command.work_id).await?;
            let successor = super::task_intake::actor_instance(tx, tenant, project, auth).await?;
            if successor != a.successor_execution {
                return Err(PgError::Forbidden);
            }
            if before.proposer_fence != live || before.status != HandoffStatus::Inspected {
                return Err(PgError::PreconditionsChanged);
            }
            require_predecessor(
                tx,
                tenant,
                project,
                &command.work_id,
                before.kind,
                &before.package.current_execution,
            )
            .await?;
            let prepared = prepare(tx, tenant, project, bearer, command, &a.session_id).await?;
            if prepared["data"]["context_complete"] != true {
                return Err(PgError::PreconditionsChanged);
            }
            let (package, predecessor) = canonical_package(
                tx,
                tenant,
                project,
                command,
                &prepared,
                &before.package.checkpoint_ids[0],
                &before.package.current_execution,
                Some(&before.package),
            )
            .await?;
            if package != before.package {
                return Err(PgError::PreconditionsChanged);
            }
            let binding = consumption_binding(
                auth,
                command,
                &before,
                &a.session_id,
                session_version,
                &successor,
                &prepared,
                &predecessor,
            )?;
            let request = a
                .inspection_request_id
                .as_deref()
                .ok_or(PgError::PreconditionsChanged)?;
            let receipt = tx
                .query_opt(
                    "SELECT result_json FROM awr_team.operations
                 WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4
                   AND request_id=$5 AND op='handoff.inspect' AND state='committed'",
                    &[&tenant, &project, &auth.actor_id, &auth.client_id, &request],
                )
                .await?
                .ok_or(PgError::PreconditionsChanged)?
                .get::<_, Value>(0);
            if receipt["work_id"] != command.work_id || receipt["data"]["consumption"] != binding {
                return Err(PgError::PreconditionsChanged);
            }
            let req = AcceptHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                expected_version: version(&a.expected_handoff_version)? as u64,
                acceptor_person_id: actor_person.clone(),
                successor_execution: a.successor_execution,
                // These are proven by the shared settlement gate and bound read,
                // never by the compatibility annotations from the caller.
                prior_execution_stopped: true,
                prior_reconciled: true,
                context_reprepared: true,
                expected_current_fence: expected_fence,
                live_fence: live,
                unknown_executions_open: false,
                now_ms,
            };
            let was_open = before.status.is_open();
            let h = apply_handoff_accept(&before, &req).map_err(map_core)?;
            if was_open && h.status == HandoffStatus::Accepted {
                crate::team_handoff::commit_accepted_transfer(tx, tenant, project, &before, &h)
                    .await?;
            }
            persist(tx, tenant, project, &h).await?;
            h
        }
        Action::Reject(a) => {
            let before = load_for_update(tx, tenant, project, &a.handoff_id)
                .await?
                .filter(|h| h.work_item_id == command.work_id)
                .ok_or_else(PgError::handoff_unavailable)?;
            require_person_match(&actor_person, &a.by_person_id)?;
            let req = RejectHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                expected_version: version(&a.expected_handoff_version)? as u64,
                by_person_id: actor_person.clone(),
                reason: a.reason,
                now_ms,
            };
            let h = apply_handoff_reject(&before, &req).map_err(map_core)?;
            persist(tx, tenant, project, &h).await?;
            h
        }
        Action::Cancel(a) => {
            let before = load_for_update(tx, tenant, project, &a.handoff_id)
                .await?
                .filter(|h| h.work_item_id == command.work_id)
                .ok_or_else(PgError::handoff_unavailable)?;
            require_person_match(&actor_person, &a.by_person_id)?;
            let req = CancelHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                expected_version: version(&a.expected_handoff_version)? as u64,
                by_person_id: actor_person.clone(),
                reason: a.reason,
                now_ms,
            };
            let h = apply_handoff_cancel(&before, &req).map_err(map_core)?;
            persist(tx, tenant, project, &h).await?;
            h
        }
        Action::Timeout(a) => {
            let before = load_for_update(tx, tenant, project, &a.handoff_id)
                .await?
                .filter(|h| h.work_item_id == command.work_id)
                .ok_or_else(PgError::handoff_unavailable)?;
            let req = TimeoutHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                expected_version: version(&a.expected_handoff_version)? as u64,
                now_ms,
            };
            let h = apply_handoff_timeout(&before, &req).map_err(map_core)?;
            persist(tx, tenant, project, &h).await?;
            h
        }
    };
    let duty = handoff.duty_at(handoff.updated_at_ms).map_err(map_core)?;
    let responsibility =
        crate::responsibility::current(tx, tenant, project, &command.work_id).await?;
    let mut data = json!({
        "handoff_id": handoff.id,
        "kind": kind_str(handoff.kind),
        "status": status_str(handoff.status),
        "version": handoff.version.to_string(),
        "responsibility_version": responsibility.version.to_string(),
        "duty": duty,
        "context_requires_refresh": duty.context_requires_refresh,
        "successor_may_execute": duty.successor_may_execute,
        "timeout_does_not_stop_execution": true,
        "clock_basis":"database",
    });
    if let Some(details) = inspection {
        data.as_object_mut()
            .ok_or_else(invalid)?
            .extend(details.as_object().ok_or_else(invalid)?.clone());
    }
    if serde_json::to_vec(&data).map_err(|_| invalid())?.len() > 1_048_576 {
        return Err(PgError::ResponseTooLarge);
    }
    Ok(Applied {
        data,
        preceding_events: vec![(
            "team.handoff",
            json!({
                "handoff_id": handoff.id,
                "status": status_str(handoff.status),
                "kind": kind_str(handoff.kind),
            }),
        )],
    })
}

async fn prepare(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    bearer: &str,
    command: &WorkstreamCommand,
    session: &str,
) -> PgResult<Value> {
    let query: WorkstreamQuery = serde_json::from_value(json!({
        "protocol_version":1,"op":"work.prepare","work_id":command.work_id,
        "session_id":session,"max_context_bytes":262144,
    }))
    .map_err(|_| invalid())?;
    // Resolve WorkRead separately; a handoff writer may not borrow read authority.
    authenticated_read(tx, tenant, project, bearer, &query).await
}

async fn require_predecessor(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    kind: HandoffKind,
    actor: &ExecutionInstance,
) -> PgResult<()> {
    let task = crate::responsibility::current(tx, tenant, project, work).await?;
    let permitted = match kind {
        HandoffKind::Execution => task.current_executor.as_ref() == Some(actor),
        HandoffKind::Responsibility => {
            task.owner.as_ref() == Some(actor.person_id())
                && task.current_executor.as_ref().is_none_or(|e| e == actor)
        }
    };
    if !permitted || task.pending.is_some() {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}

async fn require_receiver(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    person: &str,
) -> PgResult<()> {
    let eligible: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.persons p
         WHERE p.tenant_id=$1 AND p.project_id=$2 AND p.id=$3 AND p.status='active'
           AND EXISTS(SELECT 1 FROM awr_team.project_memberships m JOIN awr_team.actors a
             ON a.tenant_id=m.tenant_id AND a.id=m.actor_id
             WHERE m.tenant_id=p.tenant_id AND m.project_id=p.project_id
               AND a.status='active'
               AND (m.actor_id=p.id OR EXISTS(SELECT 1 FROM awr_team.person_agent_bindings b
                 WHERE b.tenant_id=p.tenant_id AND b.project_id=p.project_id AND b.person_id=p.id
                   AND b.agent_id=m.actor_id AND b.status='active'))))",
            &[&tenant, &project, &person],
        )
        .await?
        .get(0);
    if !eligible {
        return Err(PgError::Forbidden);
    }
    Ok(())
}

async fn live_fence(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
) -> PgResult<Option<i64>> {
    Ok(tx
        .query_opt(
            "SELECT last_fence FROM awr_team.work_runtime
        WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &work],
        )
        .await?
        .map(|r| r.get(0)))
}

/// Canonical selectors come from the latest real checkpoint and current scoped
/// facts. Checkpoint prose and optional branch/directory remain reported notes.
async fn canonical_package(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    command: &WorkstreamCommand,
    prepared: &Value,
    checkpoint: &str,
    executor: &ExecutionInstance,
    annotations: Option<&HandoffPackage>,
) -> PgResult<(HandoffPackage, Value)> {
    let row = tx.query_opt(
        "SELECT c.context_hash,c.contract_hash,c.next_action,c.open_loops_json,
                s.id,s.session_version,s.actor_id,s.client_id,s.state,c.id
         FROM awr_team.checkpoints anchor JOIN awr_team.sessions s
           ON s.tenant_id=anchor.tenant_id AND s.project_id=anchor.project_id AND s.id=anchor.session_id
         JOIN awr_team.checkpoints c
           ON c.tenant_id=s.tenant_id AND c.project_id=s.project_id AND c.id=s.latest_checkpoint_id AND c.session_id=s.id
         WHERE anchor.tenant_id=$1 AND anchor.project_id=$2 AND anchor.id=$3
           AND s.work_id=$4 AND s.scope_id='main'
           AND s.workstream_id=$5 AND s.ownership_version=$6",
        &[&tenant, &project, &checkpoint, &command.work_id, &command.workstream_id.to_string(),
          &version(&command.expected_ownership_version)?],
    ).await?.ok_or(PgError::PreconditionsChanged)?;
    let consumed: String = row.get(0);
    let contract: String = row.get(1);
    if contract != command.expected_contract_hash || !digest(&consumed) {
        return Err(PgError::PreconditionsChanged);
    }
    // A forged old checkpoint must not become the current sender's attribution.
    let origin_actor: String = row.get(6);
    let current_checkpoint: String = row.get(9);
    let recorded: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.operations
         WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4
           AND op='session.checkpoint' AND state='committed'
           AND result_json->>'work_id'=$5 AND result_json->>'contract_hash'=$6
           AND result_json->'data'->>'session_id'=$7
           AND result_json->'data'->>'checkpoint_id'=$8
           AND result_json->'data'->>'context_hash'=$9)",
            &[
                &tenant,
                &project,
                &origin_actor,
                &row.get::<_, String>(7),
                &command.work_id,
                &contract,
                &row.get::<_, String>(4),
                &current_checkpoint,
                &consumed,
            ],
        )
        .await?
        .get(0);
    if !recorded {
        return Err(PgError::PreconditionsChanged);
    }
    let origin_person: bool = match executor {
        ExecutionInstance::Person { person_id } => origin_actor == person_id.as_str(),
        ExecutionInstance::AgentRun { agent_id, .. } => origin_actor == *agent_id,
    };
    if !origin_person {
        return Err(PgError::Forbidden);
    }
    let mut todos = vec![row.get::<_, String>(2)];
    let loops: Vec<String> =
        serde_json::from_value(row.get(3)).map_err(|_| PgError::SourceDivergence)?;
    todos.extend(loops);
    let artifact_rows = tx.query(
        "SELECT DISTINCT e.artifact_id,a.sha256,a.state FROM awr_team.evidence e
         LEFT JOIN awr_team.artifacts a ON a.tenant_id=e.tenant_id AND a.project_id=e.project_id AND a.id=e.artifact_id
         WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.work_id=$3 AND e.contract_hash=$4
           AND e.artifact_id IS NOT NULL ORDER BY e.artifact_id LIMIT 129",
        &[&tenant, &project, &command.work_id, &command.expected_contract_hash],
    ).await?;
    if artifact_rows.len() > 128 {
        return Err(PgError::ResponseTooLarge);
    }
    let mut artifacts = Vec::new();
    for artifact in artifact_rows {
        let sha: String = artifact
            .get::<_, Option<String>>(1)
            .ok_or(PgError::EvidenceInvalid)?;
        let state: String = artifact
            .get::<_, Option<String>>(2)
            .ok_or(PgError::EvidenceInvalid)?;
        if !digest(&sha) || !matches!(state.as_str(), "finalized" | "retained") {
            return Err(PgError::EvidenceInvalid);
        }
        artifacts.push(awr_core::ArtifactVersionRef {
            artifact_id: artifact.get(0),
            version: sha,
        });
    }
    let dependencies: Vec<String> = serde_json::from_value(
        prepared["data"]["visible_contract"]["required_dependencies"].clone(),
    )
    .map_err(|_| PgError::SourceDivergence)?;
    // Only same-stream readable predecessors are inspected here. Cross-stream
    // versions come exclusively from the consumer's authorized adoption view.
    let dependency_rows = tx.query(
        "SELECT c.work_id,c.contract_hash,r.work_version,r.selected_completion_id,r.recovery_blocked
         FROM awr_team.work_contracts c JOIN awr_team.workstream_snapshot_ownership o
           ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=c.snapshot_id
          AND o.scope_id=c.scope_id AND o.work_id=c.work_id
         LEFT JOIN awr_team.work_runtime r ON r.tenant_id=c.tenant_id AND r.project_id=c.project_id
          AND r.scope_id=c.scope_id AND r.work_id=c.work_id
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main'
           AND o.workstream_id=$4 AND c.work_id=ANY($5) ORDER BY c.work_id",
        &[&tenant, &project, &prepared["source_snapshot_id"].as_str().ok_or(PgError::SourceDivergence)?,
          &command.workstream_id.to_string(), &dependencies],
    ).await?;
    let dependency_versions: Vec<Value> = dependency_rows.iter().map(|r| json!({
        "work_id":r.get::<_,String>(0),"contract_hash":r.get::<_,String>(1),
        "work_version":r.get::<_,Option<i64>>(2).map(|v|v.to_string()),
        "completion_id":r.get::<_,Option<String>>(3),"recovery_blocked":r.get::<_,Option<bool>>(4),
    })).collect();
    let awaiting: Vec<String> = tx
        .query(
            "SELECT question FROM awr_team.wait_items
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='open' ORDER BY id LIMIT 65",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let mut unknown: Vec<String> = tx.query(
        "SELECT 'Execution '||id||': outcome unknown' FROM awr_team.executions
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown'
         UNION ALL SELECT 'Integration '||id||': outcome unknown' FROM awr_team.delivery_integration_intents
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown'
         ORDER BY 1 LIMIT 65", &[&tenant, &project, &command.work_id],
    ).await?.iter().map(|r| r.get(0)).collect();
    if prepared["data"]["runtime"]["recovery_blocked"] == true {
        unknown.push(
            "Work recovery is blocked; inspect current execution and settlement facts".into(),
        );
    }
    let claim = tx.query_opt(
        "SELECT id,session_id,actor_id,fence,lease_version,state,(extract(epoch FROM expires_at)*1000)::bigint
         FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3
         ORDER BY fence DESC LIMIT 1", &[&tenant,&project,&command.work_id],
    ).await?.map(|r| json!({"id":r.get::<_,String>(0),"session_id":r.get::<_,String>(1),
        "actor_id":r.get::<_,String>(2),"fence":r.get::<_,i64>(3).to_string(),
        "lease_version":r.get::<_,i64>(4).to_string(),"state":r.get::<_,String>(5),"expires_at_ms":r.get::<_,i64>(6)}));
    let execution = tx.query_opt(
        "SELECT id,session_id,claim_id,fence,execution_version,state,terminal_reported
         FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
         ORDER BY id DESC LIMIT 1", &[&tenant,&project,&command.work_id],
    ).await?.map(|r| json!({"id":r.get::<_,String>(0),"session_id":r.get::<_,Option<String>>(1),
        "claim_id":r.get::<_,Option<String>>(2),"fence":r.get::<_,i64>(3).to_string(),
        "version":r.get::<_,i64>(4).to_string(),"state":r.get::<_,String>(5),"terminal_reported":r.get::<_,bool>(6)}));
    let package = HandoffPackage {
        task_id: command.work_id.clone(),
        contract_version: contract.clone(),
        contract_hash: contract,
        current_person_id: executor.person_id().clone(),
        current_execution: executor.clone(),
        consumed_context_digest: consumed,
        checkpoint_ids: vec![current_checkpoint.clone()],
        artifact_versions: artifacts,
        branch_id: annotations.and_then(|p| p.branch_id.clone()),
        working_directory: annotations.and_then(|p| p.working_directory.clone()),
        dependency_ids: dependencies,
        todos,
        awaiting_replies: awaiting,
        unknown_side_effects: unknown,
    };
    package.validate().map_err(|_| PgError::ResponseTooLarge)?;
    let predecessor = json!({"checkpoint_id":current_checkpoint,"session_id":row.get::<_,String>(4),
        "session_version":row.get::<_,i64>(5).to_string(),"actor_id":origin_actor,
        "client_id":row.get::<_,String>(7),"session_state":row.get::<_,String>(8),
        "coordination_claim":claim,"latest_execution":execution,
        "dependency_versions":dependency_versions,"adopted_dependencies":prepared["data"]["adopted_dependencies"]});
    Ok((package, predecessor))
}

fn consumption_binding(
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    handoff: &TeamHandoff,
    session: &str,
    session_version: i64,
    receiver: &ExecutionInstance,
    prepared: &Value,
    predecessor: &Value,
) -> PgResult<Value> {
    Ok(
        json!({"protocol":"awr-team-handoff-consumption-v1","handoff_id":handoff.id,
        "handoff_version":handoff.version.to_string(),
        "package_sha256":awr_team::request_hash(&json!(handoff.package)).map_err(|_| invalid())?,
        "actor_id":auth.actor_id,"client_id":auth.client_id,"command_delegation_id":auth.delegation_id,
        "member_person_id":receiver.person_id(),"execution_instance":receiver,
        "membership_version":auth.membership_version.to_string(),
        "session_id":session,"session_version":session_version.to_string(),
        "ownership_version":command.expected_ownership_version,"contract_hash":command.expected_contract_hash,
        "coordinator_epoch":auth.epoch,"context_hash":prepared["data"]["context_hash"],
        "context_complete":prepared["data"]["context_complete"],"predecessor":predecessor,
        "receipt_is_execution_authority":false}),
    )
}

fn require_person_match(authenticated: &PersonId, claimed: &str) -> PgResult<()> {
    if authenticated.as_str() != claimed {
        return Err(PgError::Forbidden);
    }
    Ok(())
}

async fn ensure_person(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    person_id: &str,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
         VALUES($1,$2,$3,$3,'active') ON CONFLICT(tenant_id,project_id,id) DO NOTHING",
        &[&tenant, &project, &person_id],
    )
    .await?;
    Ok(())
}

async fn load_for_update(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    id: &str,
) -> PgResult<Option<TeamHandoff>> {
    let row = tx
        .query_opt(
            "SELECT body_json FROM awr_team.team_handoffs
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
            &[&tenant, &project, &id],
        )
        .await?;
    match row {
        None => Ok(None),
        Some(r) => {
            let v: Value = r.get(0);
            let handoff: TeamHandoff =
                serde_json::from_value(v).map_err(|_| PgError::SourceDivergence)?;
            handoff
                .package
                .validate()
                .map_err(|_| PgError::SourceDivergence)?;
            Ok(Some(handoff))
        }
    }
}

async fn persist(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    h: &TeamHandoff,
) -> PgResult<()> {
    let body = serde_json::to_value(h).map_err(|e| PgError::Protocol(e.to_string()))?;
    let package = serde_json::to_value(&h.package).map_err(|e| PgError::Protocol(e.to_string()))?;
    let proposed = h
        .proposed_successor
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| PgError::Protocol(e.to_string()))?;
    let accepted = h
        .accepted_successor
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| PgError::Protocol(e.to_string()))?;
    tx.execute(
        "INSERT INTO awr_team.team_handoffs(
            tenant_id,project_id,id,work_id,kind,status,version,from_person_id,to_person_id,
            package_json,proposed_successor_json,accepted_successor_json,proposer_execution_id,
            proposer_fence,expires_at_ms,created_at_ms,updated_at_ms,inspected_at_ms,
            terminal_at_ms,terminal_reason,accept_request_key,body_json)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
         ON CONFLICT(tenant_id,project_id,id) DO UPDATE SET
            status=EXCLUDED.status, version=EXCLUDED.version, package_json=EXCLUDED.package_json,
            proposed_successor_json=EXCLUDED.proposed_successor_json,
            accepted_successor_json=EXCLUDED.accepted_successor_json,
            updated_at_ms=EXCLUDED.updated_at_ms, inspected_at_ms=EXCLUDED.inspected_at_ms,
            terminal_at_ms=EXCLUDED.terminal_at_ms, terminal_reason=EXCLUDED.terminal_reason,
            accept_request_key=EXCLUDED.accept_request_key, body_json=EXCLUDED.body_json",
        &[
            &tenant,
            &project,
            &h.id,
            &h.work_item_id,
            &kind_str(h.kind),
            &status_str(h.status),
            &(h.version as i64),
            &h.from_person_id.as_str(),
            &h.to_person_id.as_str(),
            &package,
            &proposed,
            &accepted,
            &h.proposer_execution_id,
            &h.proposer_fence,
            &h.expires_at_ms,
            &h.created_at_ms,
            &h.updated_at_ms,
            &h.inspected_at_ms,
            &h.terminal_at_ms,
            &h.terminal_reason,
            &h.accept_request_key,
            &body,
        ],
    )
    .await
    .map_err(|e| {
        if e.code()
            .map(|c| *c == tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
            .unwrap_or(false)
        {
            PgError::PreconditionsChanged
        } else {
            PgError::Db(e)
        }
    })?;
    Ok(())
}

pub(crate) async fn inspect_query(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work: &str,
    handoff_id: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT body_json FROM awr_team.team_handoffs
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND work_id=$4",
            &[&tenant, &project, &handoff_id, &work],
        )
        .await?
        .ok_or_else(PgError::handoff_unavailable)?;
    let body: Value = row.get(0);
    let h: TeamHandoff =
        serde_json::from_value(body.clone()).map_err(|e| PgError::Protocol(e.to_string()))?;
    if h.work_item_id != work {
        return Err(PgError::handoff_unavailable());
    }
    let duty = h
        .duty_at(crate::team_handoff::server_now_ms(tx).await?)
        .map_err(map_core)?;
    Ok(json!({
        "handoff": body,
        "duty": duty,
        "context_requires_refresh": true,
        "timeout_does_not_stop_execution": true,
    }))
}
