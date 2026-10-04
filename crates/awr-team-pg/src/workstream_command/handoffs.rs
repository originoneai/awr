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
    package: HandoffPackage,
    proposed_successor: Option<ExecutionInstance>,
    proposer_execution_id: Option<String>,
    proposer_fence: Option<String>,
    expires_at_ms: Option<i64>,
    now_ms: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inspect {
    session_id: String,
    expected_session_version: String,
    handoff_id: String,
    expected_handoff_version: String,
    inspector_person_id: String,
    now_ms: i64,
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
    prior_execution_stopped: bool,
    prior_reconciled: bool,
    context_reprepared: bool,
    expected_current_fence: Option<String>,
    now_ms: i64,
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
    now_ms: i64,
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
    now_ms: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Timeout {
    session_id: String,
    expected_session_version: String,
    handoff_id: String,
    expected_handoff_version: String,
    now_ms: i64,
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
                a.package.validate().map_err(|_| invalid())?;
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
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    action: Action,
) -> PgResult<Applied> {
    let (session_id, expected_session_version) = action.session();
    session(
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
    let actor_person = resolve_authenticated_person(tx, tenant, project, auth).await?;
    let handoff = match action {
        Action::Propose(a) => {
            if a.package.task_id != command.work_id {
                return Err(invalid());
            }
            ensure_person(tx, tenant, project, actor_person.as_str()).await?;
            ensure_person(tx, tenant, project, &a.to_person_id).await?;
            let fence = a.proposer_fence.as_ref().map(|s| version(s)).transpose()?;
            let req = ProposeHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                kind: parse_kind(&a.kind)?,
                package: a.package,
                to_person_id: person(&a.to_person_id)?,
                proposed_successor: a.proposed_successor,
                proposer_execution_id: a.proposer_execution_id,
                proposer_fence: fence,
                expires_at_ms: a.expires_at_ms,
                now_ms: a.now_ms,
            };
            let h = apply_handoff_propose(project, &command.work_id, &actor_person, &req)
                .map_err(map_core)?;
            persist(tx, tenant, project, &h).await?;
            h
        }
        Action::Inspect(a) => {
            let before = load_for_update(tx, tenant, project, &a.handoff_id)
                .await?
                .filter(|h| h.work_item_id == command.work_id)
                .ok_or_else(PgError::handoff_unavailable)?;
            require_person_match(&actor_person, &a.inspector_person_id)?;
            let req = InspectHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                expected_version: version(&a.expected_handoff_version)? as u64,
                inspector_person_id: actor_person.clone(),
                now_ms: a.now_ms,
            };
            let h = apply_handoff_inspect(&before, &req).map_err(map_core)?;
            persist(tx, tenant, project, &h).await?;
            h
        }
        Action::Accept(a) => {
            let before = load_for_update(tx, tenant, project, &a.handoff_id)
                .await?
                .filter(|h| h.work_item_id == command.work_id)
                .ok_or_else(PgError::handoff_unavailable)?;
            let unknown: i64 = tx
                .query_one(
                    "SELECT COUNT(*)::bigint FROM awr_team.executions
                     WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown'",
                    &[&tenant, &project, &command.work_id],
                )
                .await?
                .get(0);
            let live = tx
                .query_opt(
                    "SELECT last_fence FROM awr_team.work_runtime
                     WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
                    &[&tenant, &project, &command.work_id],
                )
                .await?
                .map(|r| r.get::<_, i64>(0));
            let expected_fence = a
                .expected_current_fence
                .as_ref()
                .map(|s| version(s))
                .transpose()?;
            require_person_match(&actor_person, &a.acceptor_person_id)?;
            if live.is_some() && expected_fence.is_none() {
                return Err(PgError::missing_handoff_fence());
            }
            let req = AcceptHandoffRequest {
                request_key: command.request_id.clone(),
                handoff_id: a.handoff_id,
                expected_version: version(&a.expected_handoff_version)? as u64,
                acceptor_person_id: actor_person.clone(),
                successor_execution: a.successor_execution,
                prior_execution_stopped: a.prior_execution_stopped,
                prior_reconciled: a.prior_reconciled,
                context_reprepared: a.context_reprepared,
                expected_current_fence: expected_fence,
                live_fence: live,
                unknown_executions_open: unknown > 0,
                now_ms: a.now_ms,
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
                now_ms: a.now_ms,
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
                now_ms: a.now_ms,
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
                now_ms: a.now_ms,
            };
            let h = apply_handoff_timeout(&before, &req).map_err(map_core)?;
            persist(tx, tenant, project, &h).await?;
            h
        }
    };
    let duty = handoff.duty_at(handoff.updated_at_ms).map_err(map_core)?;
    Ok(Applied {
        data: json!({
            "handoff_id": handoff.id,
            "kind": kind_str(handoff.kind),
            "status": status_str(handoff.status),
            "version": handoff.version.to_string(),
            "duty": duty,
            "context_requires_refresh": duty.context_requires_refresh,
            "successor_may_execute": duty.successor_may_execute,
            "timeout_does_not_stop_execution": true,
        }),
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

fn require_person_match(authenticated: &PersonId, claimed: &str) -> PgResult<()> {
    if authenticated.as_str() != claimed {
        return Err(PgError::Forbidden);
    }
    Ok(())
}

/// Resolve the person speaking for this authenticated credential.
/// Prefer an active explicit person↔agent binding for agent actors; otherwise
/// require a person row whose id equals the authenticated actor_id.
async fn resolve_authenticated_person(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
) -> PgResult<PersonId> {
    let bindings = tx
        .query(
            "SELECT person_id FROM awr_team.person_agent_bindings
             WHERE tenant_id=$1 AND project_id=$2 AND agent_id=$3 AND status='active'
             ORDER BY id",
            &[&tenant, &project, &auth.actor_id],
        )
        .await?;
    if bindings.len() > 1 {
        return Err(PgError::Protocol(
            "multiple active person↔agent bindings for actor; refuse ambiguous handoff identity"
                .into(),
        ));
    }
    if let Some(row) = bindings.first() {
        let person_id: String = row.get(0);
        return person(&person_id);
    }
    let person_row = tx
        .query_opt(
            "SELECT id FROM awr_team.persons
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND status='active'",
            &[&tenant, &project, &auth.actor_id],
        )
        .await?;
    if person_row.is_some() {
        return person(&auth.actor_id);
    }
    // Human/system actors may still use actor_id as person key when the person row
    // is created on first propose; agent actors must have an explicit binding.
    if auth.actor_kind == "agent" {
        return Err(PgError::Forbidden);
    }
    person(&auth.actor_id)
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
            Ok(Some(
                serde_json::from_value(v).map_err(|e| PgError::Protocol(e.to_string()))?,
            ))
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
    let duty = h.duty_at(h.updated_at_ms).map_err(map_core)?;
    Ok(json!({
        "handoff": body,
        "duty": duty,
        "context_requires_refresh": true,
        "timeout_does_not_stop_execution": true,
    }))
}
