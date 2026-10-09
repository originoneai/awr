//! AWR-TMCP-030: intersect WS-015/016 delegation action sets with TMCP-010
//! product permissions. Consumes stored person/agent grants; does not reimplement
//! the person model or claim state machine.

use crate::workstream_auth::{NavigationReadScope, ReaderAuthority};
use crate::{PgError, PgResult};
use awr_core::{
    AgentAuthorization, AuthorizationStatus, AuthorizedAction, ExecutionSubjectKind,
    bind_runtime_identity,
};
use awr_team::{Action, RoleTemplate, template_actions};
use std::collections::BTreeSet;

#[derive(Clone)]
pub(crate) struct ReadDelegation {
    grant: AgentAuthorization,
    actions: BTreeSet<Action>,
}

/// Map one WS-016 authorized action onto TMCP-010 product actions it may enable.
/// `ManageAuthorization` maps to nothing here: child grants are WS-016 store ops,
/// never project-admin / audit product power.
pub fn tmcp_actions_for_authorized(action: AuthorizedAction) -> BTreeSet<Action> {
    use Action::*;
    let mut out = BTreeSet::new();
    match action {
        AuthorizedAction::Inspect => {
            out.insert(WorkRead);
        }
        AuthorizedAction::AcceptResponsibility
        | AuthorizedAction::OccupyCollaboratively
        | AuthorizedAction::ClaimCoordination => {
            // Coordination is not side-effect permission.
            out.insert(ClaimManageOwn);
        }
        AuthorizedAction::StartWork => {
            out.insert(SessionMaintainOwn);
            out.insert(ExecutionRequestAndReportOwn);
            out.insert(DeliverySubmitAndRequestReview);
        }
        AuthorizedAction::Review => {
            // Independent review still needs `independent_review_grant` on the
            // TMCP scope; mapping alone never sets that flag.
            out.insert(ReviewDecide);
            out.insert(SessionMaintainOwn);
        }
        AuthorizedAction::ProposePlanning => {
            out.insert(PlanningPropose);
        }
        AuthorizedAction::AssignWork => {
            out.insert(WorkAssign);
            out.insert(SessionMaintainOwn);
        }
        AuthorizedAction::EditPlanning => {
            out.insert(PlanningEditDraft);
        }
        AuthorizedAction::ApprovePlanning => {
            out.insert(PlanningApprove);
        }
        AuthorizedAction::PublishPlanning => {
            out.insert(PlanningPublish);
        }
        AuthorizedAction::FinalizeDelivery => {
            out.insert(DeliveryFinalize);
            out.insert(SessionMaintainOwn);
        }
        AuthorizedAction::ManageAuthorization => {}
    }
    out
}

pub fn tmcp_actions_for_authorized_set(actions: &BTreeSet<AuthorizedAction>) -> BTreeSet<Action> {
    let mut out = BTreeSet::new();
    for action in actions {
        out.extend(tmcp_actions_for_authorized(*action));
    }
    out
}

/// Membership template ∩ explicit delegation. Templates never grant trusted
/// executor / reconcile specials (`template_grants_special` is always false).
pub fn intersect_delegation_with_template(
    role: RoleTemplate,
    delegated: &BTreeSet<Action>,
) -> BTreeSet<Action> {
    template_actions(role)
        .intersection(delegated)
        .copied()
        .collect()
}

pub fn actor_requires_explicit_delegation(actor_kind: &str) -> bool {
    actor_kind == "agent"
}

/// Load covering WS-016 grants for an agent and install the intersected TMCP
/// action set. Humans/system leave `delegated_actions = None` (full template).
/// Agents without a covering grant get an empty set (deny). Model/client/session
/// changes cannot revive revoked or expired grants.
pub(crate) async fn resolve_agent_delegation(
    tx: &tokio_postgres::Transaction<'_>,
    auth: &mut ReaderAuthority,
    project_id: &str,
    work_id: Option<&str>,
    session_id: Option<&str>,
    requested_action: Option<Action>,
    now_ms: i64,
) -> PgResult<()> {
    if !actor_requires_explicit_delegation(&auth.actor_kind) {
        auth.delegated_actions = None;
        auth.delegation_id = None;
        return Ok(());
    }

    let candidates = effective_delegations(tx, auth, project_id, session_id, now_ms).await?;
    if requested_action == Some(Action::WorkRead) && work_id.is_some() {
        // Keep action candidates only for advice on this read. The selected
        // WorkRead grant still binds disclosure and consumed context; commands
        // resolve their own grant afresh and never union these candidates.
        auth.read_delegations = Some(candidates.clone());
    }
    let task_stream = if let Some(work) = work_id.filter(|w| !w.is_empty()) {
        crate::tx::bind_workstream_scope(tx, &auth.tenant_id, project_id).await?;
        tx.query_opt(
            "SELECT workstream_id FROM awr_team.workstream_ownership
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
            &[&auth.tenant_id, &project_id, &work],
        )
        .await?
        .map(|row| row.get::<_, String>(0))
    } else {
        None
    };
    let chosen = candidates.iter().find(|candidate| {
        (!matches!(
            requested_action,
            Some(Action::PlanningEditDraft | Action::PlanningApprove | Action::PlanningPublish)
        ) || matches!(
            candidate.grant.scope,
            awr_core::AuthorizationScope::Project { .. }
        )) && work_id.filter(|w| !w.is_empty()).is_none_or(|work| {
            candidate
                .grant
                .covers_task(project_id, work, task_stream.as_deref())
        }) && requested_action.map_or(!candidate.actions.is_empty(), |action| {
            candidate.actions.contains(&action)
        })
    });
    install_delegation(auth, chosen);
    Ok(())
}

/// Existing planning commands affect the project. Scoped task/workstream grants
/// cannot be promoted to project authority by omitting a selector.
pub(crate) async fn authorize_project_action(
    tx: &tokio_postgres::Transaction<'_>,
    auth: &mut ReaderAuthority,
    project_id: &str,
    action: Action,
) -> PgResult<()> {
    if actor_requires_explicit_delegation(&auth.actor_kind) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let candidates = effective_delegations(tx, auth, project_id, None, now_ms).await?;
        let chosen = candidates.iter().find(|candidate| {
            matches!(
                candidate.grant.scope,
                awr_core::AuthorizationScope::Project { .. }
            ) && candidate.actions.contains(&action)
        });
        install_delegation(auth, chosen);
        if matches!(
            action,
            Action::PlanningEditDraft | Action::PlanningApprove | Action::PlanningPublish
        ) {
            // A project plan may add unowned work. A partial workstream writer
            // cannot use that absence of ownership to escape its live scope.
            for stream in auth
                .catalog
                .workstreams
                .iter()
                .filter(|s| s.state == awr_core::WorkstreamState::Active)
            {
                auth.access
                    .authorize(&auth.catalog, stream.id, awr_core::WorkstreamAction::Write)
                    .map_err(|_| PgError::Forbidden)?;
            }
        }
    }
    crate::workstream_auth::authorize_domain_action(auth, action, None, None)
}

fn install_delegation(auth: &mut ReaderAuthority, chosen: Option<&ReadDelegation>) {
    auth.delegation_id = chosen.map(|candidate| candidate.grant.id.clone());
    auth.delegated_actions = Some(
        chosen
            .map(|candidate| candidate.actions.clone())
            .unwrap_or_default(),
    );
}

/// Project inspection stays explicit. A scoped proposer may recover only its
/// own suggestion receipt, under one current grant covering all affected work.
pub(crate) async fn authorize_planning_outcome(
    tx: &tokio_postgres::Transaction<'_>,
    auth: &mut ReaderAuthority,
    project_id: &str,
    receipt_identity: Option<(&str, &str, &str)>,
    affected_work_keys: Option<&[String]>,
    now_ms: i64,
) -> PgResult<()> {
    let actions = [
        Action::PlanningPropose,
        Action::PlanningEditDraft,
        Action::PlanningApprove,
        Action::PlanningPublish,
        Action::WorkRead,
    ];
    if !actor_requires_explicit_delegation(&auth.actor_kind) {
        return actions
            .iter()
            .find_map(|action| {
                crate::workstream_auth::authorize_domain_action(auth, *action, None, None).ok()
            })
            .ok_or(PgError::Forbidden);
    }
    let candidates = effective_delegations(tx, auth, project_id, None, now_ms).await?;
    if let Some(candidate) = candidates.iter().find(|candidate| {
        matches!(
            candidate.grant.scope,
            awr_core::AuthorizationScope::Project { .. }
        ) && actions
            .iter()
            .any(|action| candidate.actions.contains(action))
    }) {
        install_delegation(auth, Some(candidate));
        let action = actions
            .iter()
            .find(|action| candidate.actions.contains(action))
            .unwrap();
        return crate::workstream_auth::authorize_domain_action(auth, *action, None, None);
    }
    let mut tasks = Vec::new();
    if let Some((op, actor, client)) = receipt_identity {
        if op != "planning.propose" || actor != auth.actor_id || client != auth.client_id {
            return Err(PgError::Forbidden);
        }
        let keys = affected_work_keys.ok_or(PgError::Forbidden)?;
        if keys.is_empty() || keys.len() > 256 {
            return Err(PgError::Forbidden);
        }
        for work in keys.iter().collect::<BTreeSet<_>>() {
            let row = tx
                .query_opt(
                    "SELECT s.workstream_id FROM awr_team.workstream_snapshot_ownership s
                 JOIN awr_team.workstream_ownership o
                   ON o.tenant_id=s.tenant_id AND o.project_id=s.project_id
                  AND o.work_id=s.work_id AND o.workstream_id=s.workstream_id
                  AND o.ownership_version=s.ownership_version
                 WHERE s.tenant_id=$1 AND s.project_id=$2 AND s.snapshot_id=$3
                   AND s.scope_id='main' AND s.work_id=$4 FOR SHARE OF o",
                    &[&auth.tenant_id, &project_id, &auth.snapshot, &work.as_str()],
                )
                .await?
                .ok_or(PgError::Forbidden)?;
            let stream: awr_core::Id = row
                .get::<_, String>(0)
                .parse()
                .map_err(|_| PgError::Forbidden)?;
            auth.access
                .authorize(&auth.catalog, stream, awr_core::WorkstreamAction::Read)
                .map_err(|_| PgError::Forbidden)?;
            tasks.push((work, stream));
        }
    }
    let chosen = candidates
        .iter()
        .find(|candidate| {
            candidate.actions.contains(&Action::PlanningPropose)
                && tasks.iter().all(|(work, stream)| {
                    candidate
                        .grant
                        .covers_task(project_id, work, Some(&stream.to_string()))
                })
        })
        .ok_or(PgError::Forbidden)?;
    install_delegation(auth, Some(chosen));
    if tasks.is_empty() {
        // A missing receipt is unknown, never permission to execute or to
        // inspect another caller's request. No request body is exposed.
        return crate::workstream_auth::authorize_domain_action(
            auth,
            Action::PlanningPropose,
            None,
            None,
        );
    }
    for (work, stream) in tasks {
        crate::workstream_auth::authorize_domain_action(
            auth,
            Action::PlanningPropose,
            Some(stream),
            Some(work),
        )?;
    }
    Ok(())
}

/// Suggestions are inert, but an Agent must have one live delegation covering
/// every affected published task. Separate grants cannot synthesize authority.
pub(crate) async fn authorize_planning_suggestion(
    tx: &tokio_postgres::Transaction<'_>,
    auth: &mut ReaderAuthority,
    project_id: &str,
    affected_work_keys: &[String],
    now_ms: i64,
) -> PgResult<()> {
    if !actor_requires_explicit_delegation(&auth.actor_kind) {
        return crate::workstream_auth::authorize_domain_action(
            auth,
            Action::PlanningPropose,
            None,
            None,
        );
    }
    // Match the suggestion codec's bound before issuing per-task lookups.
    if affected_work_keys.is_empty() || affected_work_keys.len() > 256 {
        return Err(PgError::Forbidden);
    }
    crate::tx::bind_workstream_scope(tx, &auth.tenant_id, project_id).await?;
    let mut tasks = Vec::new();
    for work in affected_work_keys.iter().collect::<BTreeSet<_>>() {
        let row = tx
            .query_opt(
                "SELECT o.workstream_id FROM awr_team.workstream_ownership o
             JOIN awr_team.workstream_snapshot_ownership s
               ON s.tenant_id=o.tenant_id AND s.project_id=o.project_id AND s.work_id=o.work_id
              AND s.scope_id='main' AND s.workstream_id=o.workstream_id
              AND s.ownership_version=o.ownership_version
             JOIN awr_team.work_contracts c
               ON c.tenant_id=s.tenant_id AND c.project_id=s.project_id
              AND c.snapshot_id=s.snapshot_id AND c.scope_id=s.scope_id AND c.work_id=s.work_id
             WHERE o.tenant_id=$1 AND o.project_id=$2 AND o.work_id=$3
               AND s.snapshot_id=$4 AND c.definition_state='enabled'
             FOR SHARE OF o",
                &[&auth.tenant_id, &project_id, &work.as_str(), &auth.snapshot],
            )
            .await?
            .ok_or(PgError::Forbidden)?;
        let stream: awr_core::Id = row
            .get::<_, String>(0)
            .parse()
            .map_err(|_| PgError::Forbidden)?;
        auth.access
            .authorize(&auth.catalog, stream, awr_core::WorkstreamAction::Write)
            .map_err(|_| PgError::Forbidden)?;
        tasks.push((work, stream));
    }
    let candidates = effective_delegations(tx, auth, project_id, None, now_ms).await?;
    let chosen = candidates.iter().find(|candidate| {
        candidate.actions.contains(&Action::PlanningPropose)
            && tasks.iter().all(|(work, stream)| {
                candidate
                    .grant
                    .covers_task(project_id, work, Some(&stream.to_string()))
            })
    });
    install_delegation(auth, chosen);
    for (work, stream) in tasks {
        crate::workstream_auth::authorize_domain_action(
            auth,
            Action::PlanningPropose,
            Some(stream),
            Some(work),
        )?;
    }
    Ok(())
}

async fn effective_delegations(
    tx: &tokio_postgres::Transaction<'_>,
    auth: &ReaderAuthority,
    project_id: &str,
    session_id: Option<&str>,
    now_ms: i64,
) -> PgResult<Vec<ReadDelegation>> {
    let rows = tx
        .query(
            "SELECT body_json FROM awr_team.agent_authorizations
             WHERE tenant_id=$1 AND project_id=$2 AND subject_id=$3 AND status='active'
             ORDER BY created_at_ms ASC, id ASC
             FOR SHARE",
            &[&auth.tenant_id, &project_id, &auth.actor_id],
        )
        .await?;

    let mut candidates = Vec::new();
    for row in rows {
        let body: serde_json::Value = row.get(0);
        let grant: AgentAuthorization = serde_json::from_value(body)
            .map_err(|e| PgError::Protocol(format!("corrupt agent authorization: {e}")))?;
        if !matches!(grant.subject_kind, ExecutionSubjectKind::Agent) {
            continue;
        }
        if grant.subject_id != auth.actor_id {
            continue;
        }
        if !matches!(grant.status, AuthorizationStatus::Active) {
            continue;
        }
        if !grant.is_effective_at(now_ms) {
            continue;
        }
        if bind_runtime_identity(&grant, &auth.client_id, session_id, None, now_ms).is_err() {
            continue;
        }
        // Live WS-015 relationship check: a valid-looking binding_id in JSON is
        // not enough — person and binding rows must be active and match.
        if !live_person_binding_covers(tx, &auth.tenant_id, project_id, &grant, &auth.actor_id)
            .await?
        {
            continue;
        }
        if grant.scope.project_id() != project_id {
            continue;
        }

        let mapped = tmcp_actions_for_authorized_set(&grant.actions);
        // Explicit membership grants and duty ceilings share the same policy
        // as normal authorization; a template-only prefilter loses opt-in actions.
        let intersected = mapped
            .intersection(&auth.membership_actions())
            .copied()
            .collect();
        candidates.push(ReadDelegation {
            grant,
            actions: intersected,
        });
    }

    // Resolve narrowing before choosing an action. Otherwise an action removed
    // by a child could fall back to the broader parent. Compute this before
    // task filtering: an out-of-scope task cannot resurrect a narrowed parent.
    // Independent grants may
    // cover different actions, but are never unioned into synthetic authority.
    let narrowed: BTreeSet<String> = candidates
        .iter()
        .filter_map(|child| {
            let parent_id = child.grant.parent_authorization_id.as_deref()?;
            candidates.iter().find_map(|parent| {
                (parent.grant.id == parent_id
                    && child.actions.is_subset(&parent.actions)
                    && child.grant.scope.is_within(&parent.grant.scope)
                    && (child.actions.len() < parent.actions.len()
                        || child.grant.scope != parent.grant.scope))
                    .then(|| parent_id.to_owned())
            })
        })
        .collect();
    candidates.retain(|candidate| !narrowed.contains(&candidate.grant.id));
    Ok(candidates)
}

fn covers_stream(grant: &AgentAuthorization, stream: awr_core::Id) -> bool {
    match &grant.scope {
        awr_core::AuthorizationScope::Project { .. } => true,
        awr_core::AuthorizationScope::Workstream { workstream_id, .. } => {
            workstream_id == &stream.to_string()
        }
        // Task grants do not disclose the other tasks in their workstream.
        _ => false,
    }
}

/// Discovery can expose several independently authorized read scopes. It never
/// combines their action sets into a reusable command authority.
pub(crate) async fn resolve_agent_read_delegation(
    tx: &tokio_postgres::Transaction<'_>,
    auth: &mut ReaderAuthority,
    project: &str,
    request: &crate::WorkstreamQuery,
    now_ms: i64,
) -> PgResult<()> {
    let discovery = request.work_id.is_none()
        && request.session_id.is_none()
        && matches!(
            request.op.as_str(),
            "capabilities"
                | "workstreams.list"
                | "work.next"
                | "work.inbox"
                | "work.list"
                | "work.search"
                | "events.list"
        );
    if !actor_requires_explicit_delegation(&auth.actor_kind) || !discovery {
        resolve_agent_delegation(
            tx,
            auth,
            project,
            request.work_id.as_deref(),
            request.session_id.as_deref(),
            crate::workstream_auth::query_business_action(&request.op),
            now_ms,
        )
        .await?;
        return restrict_read_scope(tx, auth, project, request).await;
    }
    let candidates = effective_delegations(tx, auth, project, None, now_ms).await?;
    if let Some(stream) = request.workstream_id {
        let chosen = candidates.iter().find(|candidate| {
            candidate.actions.contains(&Action::WorkRead) && covers_stream(&candidate.grant, stream)
        });
        install_delegation(auth, chosen);
        return restrict_read_scope(tx, auth, project, request).await;
    }
    let readers: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            candidate.actions.contains(&Action::WorkRead)
                && matches!(
                    candidate.grant.scope,
                    awr_core::AuthorizationScope::Project { .. }
                        | awr_core::AuthorizationScope::Workstream { .. }
                )
        })
        .collect();
    if matches!(request.op.as_str(), "work.next" | "work.inbox") {
        let mut scope = NavigationReadScope::default();
        for stream in &auth.catalog.workstreams {
            if readers
                .iter()
                .any(|reader| covers_stream(&reader.grant, stream.id))
                && auth
                    .access
                    .authorize(&auth.catalog, stream.id, awr_core::WorkstreamAction::Read)
                    .is_ok()
            {
                scope.streams.insert(stream.id);
            }
        }
        let tasks: Vec<_> = candidates
            .iter()
            .filter_map(|candidate| {
                if !candidate.actions.contains(&Action::WorkRead) {
                    return None;
                }
                match &candidate.grant.scope {
                    awr_core::AuthorizationScope::Task { work_item_id, .. } => {
                        Some(work_item_id.clone())
                    }
                    _ => None,
                }
            })
            .collect();
        let rows = tx.query("SELECT work_id,workstream_id FROM awr_team.workstream_snapshot_ownership
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=ANY($4)",
            &[&auth.tenant_id, &project, &auth.snapshot, &tasks]).await?;
        for row in rows {
            let stream = row
                .get::<_, String>(1)
                .parse()
                .map_err(|_| PgError::SourceDivergence)?;
            if auth
                .access
                .authorize(&auth.catalog, stream, awr_core::WorkstreamAction::Read)
                .is_ok()
            {
                scope.tasks.insert(row.get(0), stream);
            }
        }
        if scope.streams.is_empty() && scope.tasks.is_empty() {
            return Err(PgError::Forbidden);
        }
        // A task's owner is retained for per-row checks, never promoted to a
        // broadly readable stream. SQL applies the exact task-owner predicate.
        auth.access.grants.retain(|access| {
            scope.streams.contains(&access.workstream_id)
                || scope
                    .tasks
                    .values()
                    .any(|stream| *stream == access.workstream_id)
        });
        auth.navigation_read_scope = Some(scope);
    } else {
        if readers.is_empty() {
            return Err(PgError::Forbidden);
        }
        auth.access.grants.retain(|access| {
            readers
                .iter()
                .any(|candidate| covers_stream(&candidate.grant, access.workstream_id))
        });
    }
    // Navigation also depends on non-read actions, so bind its cursor to every
    // effective candidate, including action-only grants. Each row still selects
    // a single covering grant when deciding whether to offer a claim.
    auth.binding = awr_team::request_hash(&serde_json::json!({
        "identity": auth.binding,
        "authorizations": candidates.iter().map(|candidate| &candidate.grant).collect::<Vec<_>>(),
        "navigation_scope": auth.navigation_read_scope
    }))
    .map_err(|_| PgError::Forbidden)?;
    auth.delegation_id = None;
    auth.delegated_actions = Some(BTreeSet::from([Action::WorkRead]));
    auth.read_delegations = Some(candidates);
    Ok(())
}

/// Advisory navigation uses a covering grant for this exact work; an action in
/// another workstream cannot make this row claimable. Commands resolve afresh.
pub(crate) fn authorize_navigation_action(
    auth: &ReaderAuthority,
    action: Action,
    stream: awr_core::Id,
    work: &str,
) -> PgResult<()> {
    navigation_authority(auth, action, stream, work).map(|_| ())
}

/// Use the same single covering delegation for an advisory action and its
/// attributed member. Callers must not reconstruct or union discovery grants.
pub(crate) fn navigation_authority(
    auth: &ReaderAuthority,
    action: Action,
    stream: awr_core::Id,
    work: &str,
) -> PgResult<ReaderAuthority> {
    let Some(candidates) = &auth.read_delegations else {
        crate::workstream_auth::authorize_domain_action(auth, action, Some(stream), Some(work))?;
        return Ok(auth.clone());
    };
    let chosen = candidates
        .iter()
        .find(|candidate| {
            candidate.actions.contains(&action)
                && candidate.grant.covers_task(
                    &auth.access.project_id,
                    work,
                    Some(&stream.to_string()),
                )
        })
        .ok_or(PgError::Forbidden)?;
    let mut scoped = auth.clone();
    install_delegation(&mut scoped, Some(chosen));
    crate::workstream_auth::authorize_domain_action(&scoped, action, Some(stream), Some(work))?;
    Ok(scoped)
}

/// Keep selector-free discovery inside the selected delegation, not merely
/// inside the broader access grant. Task/pool reads require an explicit work ID;
/// the query's normal validation still rejects selectors on project-wide ops.
pub(crate) async fn restrict_read_scope(
    tx: &tokio_postgres::Transaction<'_>,
    auth: &mut ReaderAuthority,
    project: &str,
    request: &crate::WorkstreamQuery,
) -> PgResult<()> {
    if !actor_requires_explicit_delegation(&auth.actor_kind) {
        return Ok(());
    }
    let id = auth.delegation_id.as_deref().ok_or(PgError::Forbidden)?;
    let row=tx.query_opt("SELECT body_json FROM awr_team.agent_authorizations WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR SHARE",&[&auth.tenant_id,&project,&id]).await?.ok_or(PgError::Forbidden)?;
    let grant: AgentAuthorization =
        serde_json::from_value(row.get(0)).map_err(|_| PgError::Forbidden)?;
    let stream = match &grant.scope {
        awr_core::AuthorizationScope::Project { .. } => None,
        awr_core::AuthorizationScope::Workstream { workstream_id, .. } => {
            Some(workstream_id.clone())
        }
        awr_core::AuthorizationScope::Task { .. }
        | awr_core::AuthorizationScope::TaskPool { .. } => {
            let work = request.work_id.as_deref().ok_or(PgError::Forbidden)?;
            let row=tx.query_opt("SELECT workstream_id FROM awr_team.workstream_snapshot_ownership WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id=$4",&[&auth.tenant_id,&project,&auth.snapshot,&work]).await?.ok_or(PgError::Forbidden)?;
            let stream: String = row.get(0);
            if !grant.covers_task(project, work, Some(&stream)) {
                return Err(PgError::Forbidden);
            }
            Some(stream)
        }
    };
    if stream.is_some() && request.op == "planning.outcome" {
        return Err(PgError::Forbidden);
    }
    if let Some(stream) = stream {
        let stream = stream
            .parse::<awr_core::Id>()
            .map_err(|_| PgError::Forbidden)?;
        auth.access.grants.retain(|g| g.workstream_id == stream);
    }
    // Cursors and consumed context must not outlive the selected read grant.
    auth.binding =
        awr_team::request_hash(&serde_json::json!({"identity":auth.binding,"authorization":grant}))
            .map_err(|_| PgError::Forbidden)?;
    Ok(())
}

async fn live_person_binding_covers(
    tx: &tokio_postgres::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    grant: &AgentAuthorization,
    agent_actor_id: &str,
) -> PgResult<bool> {
    let Some(binding_id) = grant.binding_id.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    let row = tx
        .query_opt(
            "SELECT b.status, b.person_id, b.agent_id, p.status
             FROM awr_team.person_agent_bindings b
             JOIN awr_team.persons p
               ON p.tenant_id=b.tenant_id AND p.project_id=b.project_id AND p.id=b.person_id
             WHERE b.tenant_id=$1 AND b.project_id=$2 AND b.id=$3
             FOR SHARE OF b, p",
            &[&tenant_id, &project_id, &binding_id],
        )
        .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let binding_status: String = row.get(0);
    let person_id: String = row.get(1);
    let agent_id: String = row.get(2);
    let person_status: String = row.get(3);
    if binding_status != "active" || person_status != "active" {
        return Ok(false);
    }
    if agent_id != agent_actor_id {
        return Ok(false);
    }
    if person_id != grant.responsible_person_id.as_str() {
        return Ok(false);
    }
    Ok(true)
}

/// Side-effecting execution requires StartWork-mapped TMCP execution permission.
pub(crate) fn execution_side_effect_permitted(auth: &ReaderAuthority) -> bool {
    if !auth
        .membership_actions()
        .contains(&Action::ExecutionRequestAndReportOwn)
    {
        return false;
    }
    match &auth.delegated_actions {
        Some(actions) => actions.contains(&Action::ExecutionRequestAndReportOwn),
        None => {
            template_actions(auth.role_template).contains(&Action::ExecutionRequestAndReportOwn)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::AuthorizationScope;

    #[test]
    fn supervisor_actions_are_explicit_separate_and_round_trip() {
        for (authorized, product, supports_session) in [
            (AuthorizedAction::AssignWork, Action::WorkAssign, true),
            (
                AuthorizedAction::EditPlanning,
                Action::PlanningEditDraft,
                false,
            ),
            (
                AuthorizedAction::ApprovePlanning,
                Action::PlanningApprove,
                false,
            ),
            (
                AuthorizedAction::PublishPlanning,
                Action::PlanningPublish,
                false,
            ),
            (
                AuthorizedAction::FinalizeDelivery,
                Action::DeliveryFinalize,
                true,
            ),
        ] {
            let mut expected = BTreeSet::from([product]);
            if supports_session {
                expected.insert(Action::SessionMaintainOwn);
            }
            assert_eq!(tmcp_actions_for_authorized(authorized), expected);
            assert_eq!(
                AuthorizedAction::parse(authorized.as_str()).unwrap(),
                authorized
            );
            assert_eq!(
                serde_json::from_value::<AuthorizedAction>(
                    serde_json::to_value(authorized).unwrap()
                )
                .unwrap(),
                authorized
            );
        }
        for ordinary in [AuthorizedAction::StartWork, AuthorizedAction::Review] {
            let mapped = tmcp_actions_for_authorized(ordinary);
            assert!(!mapped.contains(&Action::WorkAssign));
            assert!(!mapped.contains(&Action::DeliveryFinalize));
        }
        for role in RoleTemplate::all() {
            assert!(
                !intersect_delegation_with_template(role, &BTreeSet::from([Action::WorkAssign]))
                    .contains(&Action::WorkAssign)
            );
        }
    }

    #[test]
    fn business_session_support_never_maps_to_development_or_extra_duties() {
        for (authorized, product) in [
            (AuthorizedAction::Review, Action::ReviewDecide),
            (AuthorizedAction::AssignWork, Action::WorkAssign),
            (AuthorizedAction::FinalizeDelivery, Action::DeliveryFinalize),
        ] {
            assert_eq!(
                tmcp_actions_for_authorized(authorized),
                BTreeSet::from([product, Action::SessionMaintainOwn])
            );
        }
        assert_eq!(
            tmcp_actions_for_authorized(AuthorizedAction::Inspect),
            BTreeSet::from([Action::WorkRead])
        );
    }

    #[test]
    fn planning_suggestion_is_explicit_and_never_plan_or_execution_power() {
        assert_eq!(
            tmcp_actions_for_authorized(AuthorizedAction::ProposePlanning),
            BTreeSet::from([Action::PlanningPropose]),
        );
        let start = tmcp_actions_for_authorized(AuthorizedAction::StartWork);
        assert!(!start.contains(&Action::PlanningPropose));
        for role in RoleTemplate::all() {
            let effective = intersect_delegation_with_template(
                role,
                &tmcp_actions_for_authorized(AuthorizedAction::ProposePlanning),
            );
            assert!(effective.is_subset(&BTreeSet::from([Action::PlanningPropose])));
        }
        assert_eq!(
            AuthorizedAction::parse("propose_planning").unwrap(),
            AuthorizedAction::ProposePlanning
        );
        assert_eq!(
            AuthorizedAction::ProposePlanning.as_str(),
            "propose_planning"
        );
        assert_eq!(
            serde_json::to_value(AuthorizedAction::ProposePlanning).unwrap(),
            "propose_planning"
        );
    }

    #[test]
    fn admin_agent_only_gets_explicit_work_slice() {
        let delegated = tmcp_actions_for_authorized_set(&BTreeSet::from([
            AuthorizedAction::ClaimCoordination,
            AuthorizedAction::StartWork,
        ]));
        let effective = intersect_delegation_with_template(RoleTemplate::ProjectAdmin, &delegated);
        assert!(effective.contains(&Action::ClaimManageOwn));
        assert!(effective.contains(&Action::ExecutionRequestAndReportOwn));
        assert!(effective.contains(&Action::SessionMaintainOwn));
        assert!(!effective.contains(&Action::AccessManageProject));
        assert!(!effective.contains(&Action::PlanningPublish));
        assert!(!effective.contains(&Action::AuditReadProject));
        assert!(!effective.contains(&Action::PlanningApprove));
    }

    #[test]
    fn claim_coordination_is_not_side_effect_permission() {
        let delegated =
            tmcp_actions_for_authorized_set(&BTreeSet::from([AuthorizedAction::ClaimCoordination]));
        let effective = intersect_delegation_with_template(RoleTemplate::Developer, &delegated);
        assert_eq!(effective, BTreeSet::from([Action::ClaimManageOwn]));
        assert!(!effective.contains(&Action::ExecutionRequestAndReportOwn));
    }

    #[test]
    fn manage_authorization_does_not_map_to_project_admin_actions() {
        let delegated = tmcp_actions_for_authorized_set(&BTreeSet::from([
            AuthorizedAction::ManageAuthorization,
        ]));
        assert!(delegated.is_empty());
        let effective = intersect_delegation_with_template(RoleTemplate::ProjectAdmin, &delegated);
        assert!(effective.is_empty());
    }

    #[test]
    fn product_roles_never_grant_special_executor_authority() {
        for role in RoleTemplate::all() {
            assert!(!awr_team::template_grants_special(
                role,
                awr_team::SpecialAuthority::TrustedExecutorAttestation
            ));
            assert!(!awr_team::template_grants_special(
                role,
                awr_team::SpecialAuthority::ExecutionReconciliation
            ));
        }
    }

    #[test]
    fn child_intersection_only_narrows() {
        let parent = intersect_delegation_with_template(
            RoleTemplate::ProjectAdmin,
            &tmcp_actions_for_authorized_set(&BTreeSet::from([
                AuthorizedAction::StartWork,
                AuthorizedAction::ClaimCoordination,
                AuthorizedAction::Inspect,
            ])),
        );
        let child = intersect_delegation_with_template(
            RoleTemplate::ProjectAdmin,
            &tmcp_actions_for_authorized_set(&BTreeSet::from([AuthorizedAction::StartWork])),
        );
        assert!(child.is_subset(&parent));
        assert!(child.len() < parent.len());
    }

    #[test]
    fn authorization_scope_exposes_project_id() {
        let scope = AuthorizationScope::Project {
            project_id: "p".into(),
        };
        assert_eq!(scope.project_id(), "p");
    }
}
