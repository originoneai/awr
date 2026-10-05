//! Assignment, acceptance and pool claims share the authenticated lease transaction.
use super::*;
use crate::responsibility::{apply_in_transaction, current, map_core};
use awr_core::{
    AcceptResponsibilityRequest, AgentAuthorization, AssignResponsibilityRequest,
    ClaimAvailableRequest, ClaimExecutionRequest, ExecutionInstance, PersonId,
    ResponsibilityEventType, ResponsibilityPending, ResponsibilityPendingKind, TaskResponsibility,
};

const ASSIGNMENT: &str = "awaiting_task_assignment_acceptance";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Assign {
    assignee_person_id: String,
    expected_responsibility_version: String,
    session_id: Option<String>,
    expected_session_version: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Take {
    session_id: String,
    expected_session_version: String,
    expected_responsibility_version: String,
    expected_work_version: String,
    ttl_seconds: i32,
    assignment_request_key: Option<String>,
}

pub(super) enum Action {
    Assign(Assign),
    Accept(Take),
    Available(Take),
}

impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        if op == "task.assign" {
            let a: Assign = serde_json::from_value(args).map_err(|_| invalid())?;
            if !identity(&a.assignee_person_id)
                || a.session_id.is_some() != a.expected_session_version.is_some()
                || a.session_id.as_deref().is_some_and(|s| !identity(s))
                || a.expected_session_version
                    .as_deref()
                    .is_some_and(|s| version(s).is_err() || s == "0")
            {
                return Err(invalid());
            }
            version(&a.expected_responsibility_version)?;
            return Ok(Self::Assign(a));
        }
        let a: Take = serde_json::from_value(args).map_err(|_| invalid())?;
        version(&a.expected_responsibility_version)?;
        claims::Action::parse("claim.acquire", lease_args(&a))?;
        match op {
            "task.accept_assignment"
                if a.assignment_request_key.as_deref().is_some_and(identity) =>
            {
                Ok(Self::Accept(a))
            }
            "task.claim_available" if a.assignment_request_key.is_none() => Ok(Self::Available(a)),
            _ => Err(invalid()),
        }
    }
}

fn lease_args(a: &Take) -> Value {
    json!({"session_id":a.session_id,"expected_session_version":a.expected_session_version,
        "expected_work_version":a.expected_work_version,"ttl_seconds":a.ttl_seconds})
}

/// Agents use the exact covering delegation's binding. Legacy non-Agent callers
/// may use one explicit binding, or their authenticated member actor identity.
/// This is also safe on read paths: it neither creates people nor locks rows.
pub(crate) async fn actor_instance(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
) -> PgResult<ExecutionInstance> {
    if let Some(id) = &auth.delegation_id {
        let row = tx
            .query_opt(
                "SELECT body_json FROM awr_team.agent_authorizations
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND status='active'",
                &[&tenant, &project, &id],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        let grant: AgentAuthorization =
            serde_json::from_value(row.get(0)).map_err(|_| PgError::Forbidden)?;
        let binding = grant.binding_id.as_deref().ok_or(PgError::Forbidden)?;
        let valid: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.person_agent_bindings b
            JOIN awr_team.persons p ON p.tenant_id=b.tenant_id AND p.project_id=b.project_id AND p.id=b.person_id
            WHERE b.tenant_id=$1 AND b.project_id=$2 AND b.id=$3 AND b.person_id=$4 AND b.agent_id=$5
              AND b.status='active' AND p.status='active')",
            &[&tenant,&project,&binding,&grant.responsible_person_id.as_str(),&auth.actor_id]).await?.get(0);
        if !valid || grant.subject_id != auth.actor_id || grant.client_id != auth.client_id {
            return Err(PgError::Forbidden);
        }
        return Ok(ExecutionInstance::AgentRun {
            person_id: grant.responsible_person_id,
            agent_id: auth.actor_id.clone(),
            binding_id: binding.into(),
        });
    }
    if auth.actor_kind == "agent" {
        return Err(PgError::Forbidden);
    }
    let bindings = tx.query("SELECT b.id,b.person_id,p.status FROM awr_team.person_agent_bindings b
        JOIN awr_team.persons p ON p.tenant_id=b.tenant_id AND p.project_id=b.project_id AND p.id=b.person_id
        WHERE b.tenant_id=$1 AND b.project_id=$2 AND b.agent_id=$3 AND b.status='active' ORDER BY b.id",
        &[&tenant,&project,&auth.actor_id]).await?;
    if bindings.len() > 1 {
        return Err(PgError::Forbidden);
    }
    if let Some(row) = bindings.first() {
        if row.get::<_, String>(2) != "active" {
            return Err(PgError::Forbidden);
        }
        return Ok(ExecutionInstance::AgentRun {
            person_id: PersonId::new(row.get::<_, String>(1)).map_err(map_core)?,
            agent_id: auth.actor_id.clone(),
            binding_id: row.get(0),
        });
    }
    let status = tx
        .query_opt(
            "SELECT status FROM awr_team.persons WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &auth.actor_id],
        )
        .await?;
    if status.is_some_and(|r| r.get::<_, String>(0) != "active") {
        return Err(PgError::Forbidden);
    }
    Ok(ExecutionInstance::Person {
        person_id: PersonId::new(&auth.actor_id).map_err(map_core)?,
    })
}

async fn require_target(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    command: &WorkstreamCommand,
    target: &str,
) -> PgResult<PersonId> {
    let r = tx.query_opt("SELECT a.kind,m.role,m.independent_review,m.agent_review,m.assignment_grant,m.business_roles,p.status
        FROM awr_team.actors a JOIN awr_team.project_memberships m ON m.tenant_id=a.tenant_id AND m.actor_id=a.id
        LEFT JOIN awr_team.persons p ON p.tenant_id=m.tenant_id AND p.project_id=m.project_id AND p.id=a.id
        WHERE a.tenant_id=$1 AND m.project_id=$2 AND a.id=$3 AND a.status='active' FOR SHARE OF a,m",
        &[&tenant,&project,&target]).await?.ok_or(PgError::Forbidden)?;
    if r.get::<_, Option<String>>(6).is_some_and(|s| s != "active") {
        return Err(PgError::Forbidden);
    }
    let role: String = r.get(1);
    let roles = crate::workstream_auth::decode_business_roles(r.get(5))?;
    let actions = crate::workstream_auth::membership_action_ceiling(
        &role,
        crate::workstream_auth::map_membership_role(&role).ok_or(PgError::Forbidden)?,
        &r.get::<_, String>(0),
        r.get(2),
        r.get(3),
        r.get(4),
        roles.as_ref(),
    );
    if ![
        awr_team::Action::ClaimManageOwn,
        awr_team::Action::ExecutionRequestAndReportOwn,
    ]
    .iter()
    .all(|a| actions.contains(a))
    {
        return Err(PgError::Forbidden);
    }
    let scope: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.workstream_grants g
        JOIN awr_team.actors a ON a.tenant_id=g.tenant_id AND a.id=g.actor_id AND a.status='active'
        WHERE g.tenant_id=$1 AND g.project_id=$2 AND g.workstream_id=$3 AND g.authority_version=$4
          AND g.can_read AND g.can_write AND (g.actor_id=$5 OR EXISTS(
            SELECT 1 FROM awr_team.person_agent_bindings b WHERE b.tenant_id=g.tenant_id AND b.project_id=g.project_id
              AND b.agent_id=g.actor_id AND b.person_id=$5 AND b.status='active')))",
        &[&tenant,&project,&command.workstream_id.to_string(),&version(&command.expected_authority_version)?,&target]).await?.get(0);
    if !scope {
        return Err(PgError::Forbidden);
    }
    PersonId::new(target).map_err(map_core)
}

pub(crate) async fn require_enabled(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
) -> PgResult<()> {
    let enabled: bool = tx.query_one("SELECT c.definition_state='enabled' AND s.status='active'
        FROM awr_team.work_contracts c JOIN awr_team.work_scopes s
          ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.scope_id
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main' AND c.work_id=$4",
        &[&tenant,&project,&auth.snapshot,&work]).await?.get(0);
    let terminal: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.work_runtime
        WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3
          AND (state IN ('completed','cancelled','archived') OR selected_completion_id IS NOT NULL))",
        &[&tenant,&project,&work]).await?.get(0);
    if !enabled || terminal {
        return Err(PgError::PreconditionsChanged);
    }
    Ok(())
}

pub(crate) async fn dependencies_ready(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
) -> PgResult<bool> {
    let contract: Value = tx
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &work],
        )
        .await?
        .get(0);
    let parsed: awr_team::WorkContract =
        serde_json::from_value(contract.clone()).map_err(|_| PgError::SourceDivergence)?;
    let visible: i64 = tx.query_one("SELECT count(*) FROM awr_team.workstream_snapshot_ownership
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND workstream_id=$4 AND work_id=ANY($5)",
        &[&tenant,&project,&auth.snapshot,&stream,&parsed.required_dependencies]).await?.get(0);
    if visible as usize != parsed.required_dependencies.len() {
        return Ok(false);
    }
    Ok(
        crate::review::required_dependencies_covered(tx, tenant, project, work, "main", &contract)
            .await?
            .0,
    )
}

pub(crate) async fn require_admissible(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
) -> PgResult<()> {
    require_enabled(tx, tenant, project, auth, &command.work_id).await?;
    claims::require_resolved_effects(tx, tenant, project, &command.work_id).await?;
    let waiting: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM awr_team.wait_items
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='open')",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    if waiting {
        return Err(PgError::WaitOpen);
    }
    if !dependencies_ready(
        tx,
        tenant,
        project,
        auth,
        &command.work_id,
        &command.workstream_id.to_string(),
    )
    .await?
    {
        return Err(PgError::MissingDependency);
    }
    Ok(())
}

fn internal_key(
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    transition: &str,
) -> PgResult<String> {
    Ok(format!("intake-{}", awr_team::request_hash(&json!({"domain":"task-intake-key-v1",
        "actor":auth.actor_id,"client":auth.client_id,"request":command.request_id,"transition":transition})).map_err(|_| invalid())?))
}

pub(super) async fn lock_task(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
) -> PgResult<()> {
    let mut keys = ["assign", "take", "legacy_pool", "executor"]
        .iter()
        .map(|transition| internal_key(auth, command, transition))
        .collect::<PgResult<Vec<_>>>()?;
    keys.sort();
    crate::responsibility::lock_transition_keys(tx, tenant, project, &command.work_id, &keys).await
}

async fn change<F>(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    transition: &str,
    op: ResponsibilityEventType,
    action: F,
) -> PgResult<TaskResponsibility>
where
    F: FnOnce(&TaskResponsibility, &[awr_core::PersonAgentBinding]) -> PgResult<TaskResponsibility>,
{
    let key = internal_key(auth, command, transition)?;
    let actor = actor_instance(tx, tenant, project, auth).await?;
    let hash = awr_team::request_hash(&json!({"domain":"task-intake-transition-v1","tenant":tenant,
        "project":project,"actor":auth.actor_id,"client":auth.client_id,"command":command,"transition":transition})).map_err(|_| invalid())?;
    Ok(apply_in_transaction(
        tx,
        tenant,
        project,
        &command.work_id,
        &key,
        op,
        hash,
        action,
        Some(actor.person_id().as_str()),
        json!({"command":command.op,"request_id":command.request_id,"transition":transition}),
    )
    .await?
    .0)
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
    let actor = actor_instance(tx, tenant, project, auth).await?;
    match action {
        Action::Assign(a) => {
            if let (Some(id), Some(v)) = (&a.session_id, &a.expected_session_version) {
                session(tx, tenant, project, auth, command, id, v, ownership).await?;
            }
            require_enabled(tx, tenant, project, auth, &command.work_id).await?;
            claims::require_resolved_effects(tx, tenant, project, &command.work_id).await?;
            let held: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.claims WHERE tenant_id=$1
                AND project_id=$2 AND work_id=$3 AND state='active' AND expires_at>clock_timestamp())",
                &[&tenant,&project,&command.work_id]).await?.get(0);
            if held {
                return Err(PgError::ClaimHeld);
            }
            let target =
                require_target(tx, tenant, project, command, &a.assignee_person_id).await?;
            let key = internal_key(auth, command, "assign")?;
            let expected = version(&a.expected_responsibility_version)? as u64;
            let after = change(
                tx,
                tenant,
                project,
                auth,
                command,
                "assign",
                ResponsibilityEventType::Assigned,
                |before, _| {
                    if before.owner.is_some()
                        || before.pending.is_some()
                        || before.current_executor.is_some()
                    {
                        return Err(PgError::ClaimHeld);
                    }
                    let req = AssignResponsibilityRequest {
                        request_key: key.clone(),
                        expected_version: expected,
                        owner: Some(target.clone()),
                        collaborators: before
                            .collaborators
                            .iter()
                            .filter(|p| **p != target)
                            .cloned()
                            .collect(),
                        independent_reviewer: before.independent_reviewer.clone(),
                        allow_unassigned: false,
                        authorized_by: actor.person_id().clone(),
                    };
                    let mut after = awr_core::apply_assign(before, &req).map_err(map_core)?;
                    after.pending = Some(ResponsibilityPending {
                        kind: ResponsibilityPendingKind::NoAcceptor,
                        person_id: Some(target),
                        legacy_ref: None,
                        transfer_request_key: Some(key.clone()),
                        detail: ASSIGNMENT.into(),
                    });
                    after.validate_structure().map_err(map_core)?;
                    Ok(after)
                },
            )
            .await?;
            Ok(Applied {
                data: json!({"responsibility":view(&after),"assignment_request_key":key,
                "coordination_claim_acquired":false,"execution_authorized":false}),
                preceding_events: vec![],
            })
        }
        Action::Accept(a) | Action::Available(a) => {
            session(
                tx,
                tenant,
                project,
                auth,
                command,
                &a.session_id,
                &a.expected_session_version,
                ownership,
            )
            .await?;
            require_admissible(tx, tenant, project, auth, command).await?;
            let expected = version(&a.expected_responsibility_version)? as u64;
            let key = internal_key(auth, command, "take")?;
            let is_accept = command.op == "task.accept_assignment";
            change(
                tx,
                tenant,
                project,
                auth,
                command,
                "take",
                if is_accept {
                    ResponsibilityEventType::Accepted
                } else {
                    ResponsibilityEventType::AvailableClaimed
                },
                |before, _| {
                    if is_accept {
                        let pending = before
                            .pending
                            .as_ref()
                            .ok_or(PgError::PreconditionsChanged)?;
                        if pending.kind != ResponsibilityPendingKind::NoAcceptor
                            || pending.detail != ASSIGNMENT
                            || pending.person_id.as_ref() != Some(actor.person_id())
                            || pending.transfer_request_key != a.assignment_request_key
                        {
                            return Err(PgError::Forbidden);
                        }
                        awr_core::apply_accept(
                            before,
                            &AcceptResponsibilityRequest {
                                request_key: key.clone(),
                                expected_version: expected,
                                acceptor: actor.person_id().clone(),
                                as_owner: true,
                            },
                        )
                        .map_err(map_core)
                    } else {
                        awr_core::apply_claim_available(
                            before,
                            &ClaimAvailableRequest {
                                request_key: key.clone(),
                                expected_version: expected,
                                claimant: actor.person_id().clone(),
                            },
                        )
                        .map_err(map_core)
                    }
                },
            )
            .await?;
            let mut result = claims::apply(
                tx,
                tenant,
                project,
                auth,
                command,
                ownership,
                claims::Action::parse("claim.acquire", lease_args(&a))?,
            )
            .await?;
            result.data["responsibility"] =
                view(&current(tx, tenant, project, &command.work_id).await?);
            result.data["coordination_claim_acquired"] = json!(true);
            Ok(result)
        }
    }
}

pub(crate) fn view(task: &TaskResponsibility) -> Value {
    let mut v = json!(task);
    v["version"] = json!(task.version.to_string());
    v
}

/// A retained executor also retains its client boundary. Only an accepted
/// handoff by this authenticated client at the current responsibility version
/// permits switching clients. Expiry/release are not transfer authority.
pub(crate) async fn require_executor(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
) -> PgResult<()> {
    let actor = actor_instance(tx, tenant, project, auth).await?;
    let task = current(tx, tenant, project, &command.work_id).await?;
    if task.pending.is_some()
        || task.owner.as_ref().is_some_and(|p| {
            p != actor.person_id() && !task.collaborators.contains(actor.person_id())
        })
        || task.current_executor.as_ref().is_some_and(|e| e != &actor)
    {
        return Err(PgError::ClaimHeld);
    }
    if !client_may_continue(tx, tenant, project, auth, &task).await? {
        return Err(PgError::ClaimHeld);
    }
    Ok(())
}

async fn client_may_continue(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    task: &TaskResponsibility,
) -> PgResult<bool> {
    if let Some(r) = tx.query_opt("SELECT c.actor_id,s.client_id,c.state FROM awr_team.claims c
        JOIN awr_team.sessions s ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.session_id
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.scope_id='main' AND c.work_id=$3 ORDER BY c.fence DESC LIMIT 1",
        &[&tenant,&project,&task.work_item_id]).await? {
        if r.get::<_,String>(0) != auth.actor_id || r.get::<_,String>(1) != auth.client_id {
            let accepted: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM awr_team.operations
                WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND op='handoff.accept' AND state='committed'
                  AND result_json->>'work_id'=$5 AND result_json->'data'->>'status'='accepted'
                  AND result_json->'data'->>'responsibility_version'=$6)",
                &[&tenant,&project,&auth.actor_id,&auth.client_id,&task.work_item_id,&task.version.to_string()]).await?.get(0);
            if r.get::<_,String>(2) != "handed_off" || !accepted { return Ok(false); }
        }
    }
    Ok(true)
}

pub(crate) async fn read_state(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    work: &str,
    stream: &str,
    session_id: Option<&str>,
) -> PgResult<Value> {
    // A read-only delegation may name a different member from the covering
    // coordination delegation. Resolve advisory identity through the same
    // selector used by commands, without combining those permissions.
    let mut coordination = auth.clone();
    if auth.actor_kind == "agent"
        && auth
            .membership_actions()
            .contains(&awr_team::Action::ClaimManageOwn)
        && !auth
            .delegated_actions
            .as_ref()
            .is_some_and(|s| s.contains(&awr_team::Action::ClaimManageOwn))
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_millis() as i64)
            .unwrap_or(0);
        match crate::delegation_auth::resolve_agent_delegation(
            tx,
            &mut coordination,
            project,
            Some(work),
            session_id,
            Some(awr_team::Action::ClaimManageOwn),
            now,
        )
        .await
        {
            Ok(()) => {}
            Err(PgError::Forbidden) => coordination = auth.clone(),
            Err(e) => return Err(e),
        }
    }
    let auth = &coordination;
    let task = current(tx, tenant, project, work).await?;
    let actor = match actor_instance(tx, tenant, project, auth).await {
        Ok(actor) => Some(actor),
        Err(PgError::Forbidden) => None,
        Err(e) => return Err(e),
    };
    let owns = actor
        .as_ref()
        .is_some_and(|a| task.owner.as_ref() == Some(a.person_id()));
    let permitted_executor = match &actor {
        Some(a) if task.current_executor.as_ref().is_none_or(|e| e == a) => {
            client_may_continue(tx, tenant, project, auth, &task).await?
        }
        _ => false,
    };
    let relation = if let Some(pending) = &task.pending {
        if pending.kind == ResponsibilityPendingKind::NoAcceptor && pending.detail == ASSIGNMENT {
            if owns {
                "assigned_to_me"
            } else {
                "assigned_to_other"
            }
        } else {
            "responsibility_pending"
        }
    } else if task.owner.is_none() && task.current_executor.is_none() {
        "pool"
    } else if owns && permitted_executor {
        "owned_by_me"
    } else if owns {
        "handoff_required"
    } else {
        "owned_by_other"
    };
    let mut data = view(&task);
    data["relation"] = json!(relation);
    data["member_person_id"] = json!(actor.as_ref().map(|a| a.person_id().as_str()));
    data["coordination_delegation_id"] = json!(auth.delegation_id);
    data["current_executor_matches_client"] = json!(permitted_executor);
    let stream_id = stream.parse().map_err(|_| PgError::SourceDivergence)?;
    data["coordination_allowed_advisory"] = json!(
        crate::workstream_auth::authorize_domain_action(
            auth,
            awr_team::Action::ClaimManageOwn,
            Some(stream_id),
            Some(work)
        )
        .is_ok()
            && auth
                .access
                .authorize(&auth.catalog, stream_id, awr_core::WorkstreamAction::Write)
                .is_ok()
    );
    Ok(data)
}

/// Replace only generic intake hints. Client declaration, recovery, live
/// execution and delivery retain priority; no read grants execution authority.
pub(crate) fn guidance(data: &Value, fallback: Value) -> Value {
    if data["definition_state"]
        .as_str()
        .is_some_and(|s| s != "enabled")
        && matches!(
            fallback["code"].as_str(),
            Some(
                "restore_context"
                    | "start_session"
                    | "other_client_session"
                    | "declare_client"
                    | "report_at_boundary"
            )
        )
    {
        return json!({"code":"work_definition_inactive","when":"the current task definition is inactive",
            "because":"the published definition is not enabled","action":{"op":"work.next","note":"Select enabled work; archive or draft state does not grant execution authority."},
            "recheck_on":"source definition or recovery changes"});
    }
    if !matches!(
        fallback["code"].as_str(),
        Some("start_session" | "other_client_session" | "report_at_boundary")
    ) {
        return fallback;
    }
    let relation = data["responsibility"]["relation"]
        .as_str()
        .unwrap_or("unknown");
    let (op, note) = match relation {
        "assigned_to_other" | "owned_by_other" => (
            "work.next",
            "This task belongs to another member. Select your assignment or an available task.",
        ),
        "responsibility_pending" | "handoff_required" => (
            "work.recovery",
            "Inspect the pending responsibility and request controlled handoff before claiming.",
        ),
        "pool" | "assigned_to_me" | "owned_by_me"
            if data["responsibility"]["coordination_allowed_advisory"] != true =>
        {
            (
                "work.next",
                "Observe this task within your current scope; taking it requires a current development grant.",
            )
        }
        "pool" | "assigned_to_me" | "owned_by_me" if data["claim"]["lease_live"] != true => {
            let owns_session =
                data["session"]["state"] == "active" && fallback["code"] != "other_client_session";
            if !owns_session {
                (
                    "session.start",
                    "Consume work.prepare and start your own session before taking responsibility and a coordination lease.",
                )
            } else if relation == "assigned_to_me" {
                (
                    "task.accept_assignment",
                    "Accept the current assignment with its request key and responsibility version after dependencies are ready.",
                )
            } else if relation == "pool" {
                (
                    "task.claim_available",
                    "Claim the available task and coordination lease together using current responsibility and work versions.",
                )
            } else {
                (
                    "claim.acquire",
                    "Continue your owned task with current work versions after checking dependencies and settled effects.",
                )
            }
        }
        _ => return fallback,
    };
    json!({"code":"task_intake","when":relation,"because":format!("current responsibility version {}",data["responsibility"]["version"].as_str().unwrap_or("unknown")),
        "action":{"op":op,"note":note},"recheck_on":"responsibility, dependency, lease or permission changes"})
}

pub(super) async fn bind_claim(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    claim: &str,
) -> PgResult<()> {
    let actor = actor_instance(tx, tenant, project, auth).await?;
    let task = current(tx, tenant, project, &command.work_id).await?;
    if task.owner.is_none() {
        let key = internal_key(auth, command, "legacy_pool")?;
        change(
            tx,
            tenant,
            project,
            auth,
            command,
            "legacy_pool",
            ResponsibilityEventType::AvailableClaimed,
            |before, _| {
                awr_core::apply_claim_available(
                    before,
                    &ClaimAvailableRequest {
                        request_key: key,
                        expected_version: task.version,
                        claimant: actor.person_id().clone(),
                    },
                )
                .map_err(map_core)
            },
        )
        .await?;
    }
    let task = current(tx, tenant, project, &command.work_id).await?;
    if task.current_executor.as_ref() == Some(&actor) {
        return Ok(());
    }
    let key = internal_key(auth, command, "executor")?;
    change(
        tx,
        tenant,
        project,
        auth,
        command,
        "executor",
        ResponsibilityEventType::ExecutionClaimed,
        |before, bindings| {
            awr_core::apply_claim_execution(
                before,
                &ClaimExecutionRequest {
                    request_key: key,
                    expected_version: task.version,
                    executor: actor.clone(),
                    coordination_claim_id: Some(claim.into()),
                },
                bindings,
            )
            .map_err(map_core)
        },
    )
    .await?;
    Ok(())
}
