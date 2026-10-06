use crate::{PgError, PgResult};
use awr_core::{Id, WorkstreamAccess, WorkstreamAction, WorkstreamCatalog, WorkstreamGrant};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tokio_postgres::Transaction;

/// Hash a high-entropy operator-issued bearer. No raw credential is stored.
/// Format: `awr1.<credential id>.<64 lowercase hexadecimal characters>`.
pub fn workstream_credential_hash(token: &str) -> PgResult<String> {
    token_id(token)?;
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(format!("awr-team-credential-v1:{token}"))
    ))
}

fn token_id(token: &str) -> PgResult<&str> {
    if token.len() > 199 {
        return Err(PgError::Forbidden);
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3
        || parts[0] != "awr1"
        || parts[1].is_empty()
        || parts[1].len() > 128
        || !parts[1]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        || parts[2].len() != 64
        || !parts[2]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(PgError::Forbidden);
    }
    Ok(parts[1])
}

#[derive(Clone)]
pub(crate) struct ReaderAuthority {
    pub tenant_id: String,
    pub actor_id: String,
    pub client_id: String,
    pub actor_kind: String,
    /// Raw project_memberships.role (legacy or TMCP template name).
    pub role: String,
    pub role_template: awr_team::RoleTemplate,
    pub business_roles: Option<std::collections::BTreeSet<awr_team::BusinessRole>>,
    pub assignment_grant: bool,
    pub membership_version: i64,
    /// Explicit independent review.decide grant (never implied by role template).
    pub independent_review: bool,
    /// Explicit Agent review grant; requires live WS-016 Review delegation too.
    pub agent_review: bool,
    pub execution_access: BTreeMap<Id, ExecutionAccess>,
    pub access: WorkstreamAccess,
    pub catalog: WorkstreamCatalog,
    pub snapshot: String,
    pub epoch: String,
    pub project_status: String,
    pub revision: i64,
    pub binding: String,
    pub grant_versions: BTreeMap<Id, i64>,
    /// When `Some`, TMCP actions are this set (membership ∩ WS-016 delegation).
    /// Agents always carry `Some` after resolution (empty = deny). Humans/system
    /// keep `None` and use the membership template unchanged.
    pub delegated_actions: Option<std::collections::BTreeSet<awr_team::Action>>,
    /// Covering WS-016 authorization id when `delegated_actions` is populated.
    pub delegation_id: Option<String>,
    /// Transaction-local discovery candidates; never used for command admission.
    pub read_delegations: Option<Vec<crate::delegation_auth::ReadDelegation>>,
    /// Exact selector-free work.next visibility; never reusable command authority.
    pub navigation_read_scope: Option<NavigationReadScope>,
}

impl ReaderAuthority {
    /// One live membership policy, before intersection with one delegation.
    pub(crate) fn membership_actions(&self) -> std::collections::BTreeSet<awr_team::Action> {
        membership_action_ceiling(
            &self.role,
            self.role_template,
            &self.actor_kind,
            self.independent_review,
            self.agent_review,
            self.assignment_grant,
            self.business_roles.as_ref(),
        )
    }
}

pub(crate) fn membership_action_ceiling(
    role: &str,
    template: awr_team::RoleTemplate,
    actor_kind: &str,
    independent_review: bool,
    agent_review: bool,
    assignment_grant: bool,
    business_roles: Option<&std::collections::BTreeSet<awr_team::BusinessRole>>,
) -> std::collections::BTreeSet<awr_team::Action> {
    let mut actions = awr_team::template_actions(template);
    let review = if actor_kind == "agent" {
        agent_review
    } else {
        role == "reviewer"
            || (independent_review && awr_team::independent_review_eligible(template))
    };
    if review {
        actions.insert(awr_team::Action::ReviewDecide);
    }
    if assignment_grant
        && matches!(
            template,
            awr_team::RoleTemplate::Maintainer | awr_team::RoleTemplate::ProjectAdmin
        )
    {
        actions.insert(awr_team::Action::WorkAssign);
    }
    awr_team::constrain_actions_to_business_roles(&actions, business_roles)
}

pub(crate) fn decode_business_roles(
    value: Option<serde_json::Value>,
) -> PgResult<Option<std::collections::BTreeSet<awr_team::BusinessRole>>> {
    value
        .map(|value| {
            let roles = serde_json::from_value(value).map_err(|_| PgError::Forbidden)?;
            awr_team::validate_business_roles(&roles).map_err(|_| PgError::Forbidden)?;
            Ok(roles)
        })
        .transpose()
}

pub(crate) fn role_ceiling_allows(
    roles: Option<&std::collections::BTreeSet<awr_team::BusinessRole>>,
    action: awr_team::Action,
) -> bool {
    awr_team::constrain_actions_to_business_roles(
        &std::collections::BTreeSet::from([action]),
        roles,
    )
    .contains(&action)
}

#[derive(Clone, Default, serde::Serialize)]
pub(crate) struct NavigationReadScope {
    pub streams: std::collections::BTreeSet<Id>,
    pub tasks: BTreeMap<String, Id>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ExecutionAccess {
    pub attest: bool,
    pub reconcile: bool,
}

/// Resolve every security fact in the action transaction. Row locks make a
/// concurrent disable/revocation linearize before or after the read, never in
/// its middle. No caller-supplied actor, client or grant is accepted as proof.
pub(crate) async fn authenticate(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    token: &str,
) -> PgResult<ReaderAuthority> {
    authenticate_inner(tx, tenant, project, token, false, ProjectLockMode::Share).await
}

/// How writers lock the project row during admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectLockMode {
    /// Source/admin and ordinary task writers serialize the project (audit order).
    Exclusive,
    /// Reserved for narrower task interleaving once audit cursors use a sequence.
    Share,
}

pub(crate) async fn authenticate_writer(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    token: &str,
) -> PgResult<ReaderAuthority> {
    authenticate_inner(tx, tenant, project, token, true, ProjectLockMode::Exclusive).await
}

#[allow(dead_code)]
pub(crate) async fn authenticate_task_writer(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    token: &str,
) -> PgResult<ReaderAuthority> {
    authenticate_inner(tx, tenant, project, token, true, ProjectLockMode::Share).await
}

async fn authenticate_inner(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    token: &str,
    write: bool,
    lock: ProjectLockMode,
) -> PgResult<ReaderAuthority> {
    let credential_id = token_id(token)?;
    let hash = workstream_credential_hash(token)?;
    crate::tx::bind_workstream_scope(tx, tenant, project).await?;
    let mode = tx
        .query_opt(
            "SELECT enabled FROM awr_team.workstream_modes
        WHERE tenant_id=$1 AND project_id=$2 FOR SHARE",
            &[&tenant, &project],
        )
        .await?
        .ok_or(PgError::Forbidden)?;
    // Lock the project before identity/policy records; source/admin and ordinary
    // task writers use Exclusive for a total audit order. Never upgrade a shared
    // lock inside the same transaction.
    let project_query = if write && lock == ProjectLockMode::Exclusive {
        "SELECT active_snapshot_id,coordinator_epoch,project_revision,status FROM awr_team.projects
        WHERE tenant_id=$1 AND id=$2 FOR UPDATE"
    } else {
        "SELECT active_snapshot_id,coordinator_epoch,project_revision,status FROM awr_team.projects
        WHERE tenant_id=$1 AND id=$2 FOR SHARE"
    };
    let p = tx
        .query_opt(project_query, &[&tenant, &project])
        .await?
        .ok_or(PgError::Forbidden)?;
    let identity = tx.query_opt("SELECT c.actor_id,c.client_id,m.membership_version,m.role,a.kind,m.independent_review,m.agent_review,m.business_roles,m.assignment_grant
        FROM awr_team.credentials c
        JOIN awr_team.tenants t ON t.id=c.tenant_id
        JOIN awr_team.actors a ON a.tenant_id=c.tenant_id AND a.id=c.actor_id
        JOIN awr_team.project_memberships m ON m.tenant_id=c.tenant_id AND m.actor_id=c.actor_id AND m.project_id=$2
        WHERE c.tenant_id=$1 AND c.id=$3 AND c.secret_hash=$4
          AND (c.project_id IS NULL OR c.project_id=$2)
          AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>clock_timestamp())
          AND t.status='active' AND a.status='active'
        FOR SHARE OF t,a,c,m", &[&tenant,&project,&credential_id,&hash]).await?.ok_or(PgError::Forbidden)?;
    if !mode.get::<_, bool>(0) {
        return Err(PgError::Unsupported(
            "project has not enabled workstreams".into(),
        ));
    }
    let actor: String = identity.get(0);
    let client: String = identity.get(1);
    let membership: i64 = identity.get(2);
    let role: String = identity.get(3);
    let actor_kind: String = identity.get(4);
    let independent_review: bool = identity.get(5);
    let agent_review: bool = identity.get(6);
    let business_roles = decode_business_roles(identity.get(7))?;
    let assignment_grant: bool = identity.get(8);
    let snapshot: String = p
        .get::<_, Option<String>>(0)
        .ok_or(PgError::InactiveCandidate)?;
    let row = tx
        .query_opt(
            "SELECT catalog_json FROM awr_team.workstream_catalogs
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
            &[&tenant, &project, &snapshot],
        )
        .await?
        .ok_or(PgError::InactiveCandidate)?;
    let catalog: WorkstreamCatalog =
        serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
    catalog.validate()?;
    if catalog.project_id != project {
        return Err(PgError::SourceDivergence);
    }
    let rows = tx.query("SELECT workstream_id,authority_version,can_read,can_write,can_manage,grant_version,
        can_attest_execution,can_reconcile_execution
        FROM awr_team.workstream_grants WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND active
        ORDER BY workstream_id FOR SHARE", &[&tenant,&project,&actor,&client]).await?;
    let mut grants = Vec::new();
    let mut grant_versions = BTreeMap::new();
    let mut execution_access = BTreeMap::new();
    for row in rows {
        let id: Id = row
            .get::<_, String>(0)
            .parse()
            .map_err(|_| PgError::Forbidden)?;
        let authority: i64 = row.get(1);
        let write = row.get::<_, bool>(3) && membership_allows_write(&role);
        let manage = row.get::<_, bool>(4)
            && membership_allows_manage(&role)
            && role_ceiling_allows(
                business_roles.as_ref(),
                awr_team::Action::AccessManageProject,
            );
        let execution = role_ceiling_allows(
            business_roles.as_ref(),
            awr_team::Action::ExecutionRequestAndReportOwn,
        );
        execution_access.insert(
            id,
            ExecutionAccess {
                attest: write && execution && actor_kind == "system" && row.get::<_, bool>(6),
                reconcile: write
                    && manage
                    && execution
                    && matches!(actor_kind.as_str(), "system" | "human")
                    && row.get::<_, bool>(7),
            },
        );
        grants.push(WorkstreamGrant {
            workstream_id: id,
            authority_version: authority.try_into().map_err(|_| PgError::Forbidden)?,
            read: row.get(2),
            write,
            manage,
        });
        grant_versions.insert(id, row.get(5));
    }
    let mut binding_facts = json!({"tenant":tenant,"project":project,"credential":credential_id,
        "actor":actor,"client":client,"actor_kind":actor_kind,"membership":membership,"role":role});
    if let Some(roles) = &business_roles {
        binding_facts["business_roles"] = json!(roles);
    }
    if assignment_grant {
        binding_facts["assignment_grant"] = json!(true);
    }
    let binding = awr_team::request_hash(&binding_facts).map_err(|_| PgError::Forbidden)?;
    let access = WorkstreamAccess {
        project_id: project.into(),
        subject: binding.clone(),
        grants,
    };
    access.validate()?;
    let role_template = map_membership_role(&role).ok_or(PgError::Forbidden)?;
    let delegation_required = crate::actor_requires_explicit_delegation(&actor_kind);
    Ok(ReaderAuthority {
        tenant_id: tenant.into(),
        actor_id: actor,
        client_id: client,
        actor_kind,
        role,
        role_template,
        business_roles,
        assignment_grant,
        membership_version: membership,
        independent_review,
        agent_review,
        execution_access,
        access,
        catalog,
        snapshot,
        epoch: p.get(1),
        revision: p.get(2),
        project_status: p.get(3),
        binding,
        grant_versions,
        delegated_actions: delegation_required.then(std::collections::BTreeSet::new),
        delegation_id: None,
        read_delegations: None,
        navigation_read_scope: None,
    })
}

/// Map membership role labels onto TMCP-010 role templates.
/// Legacy reader/reviewer/worker/admin remain valid DB values; template names are
/// accepted for forward compatibility when membership storage expands (TMCP-012).
pub fn map_membership_role(role: &str) -> Option<awr_team::RoleTemplate> {
    use awr_team::RoleTemplate::*;
    Some(match role {
        "reader" | "reviewer" => Reader,
        "worker" | "developer" => Developer,
        "maintainer" => Maintainer,
        "admin" | "project_admin" => ProjectAdmin,
        _ => return None,
    })
}

fn membership_allows_write(role: &str) -> bool {
    !matches!(role, "reader")
}

fn membership_allows_manage(role: &str) -> bool {
    matches!(role, "admin" | "project_admin")
}

/// Map a workstream command op onto a TMCP-010 business action.
/// Attest/reconcile are special authorities (not template actions).
pub fn command_business_action(op: &str) -> Option<awr_team::Action> {
    use awr_team::Action::*;
    Some(match op {
        "task.assign" => WorkAssign,
        "task.accept_assignment" | "task.claim_available" => ClaimManageOwn,
        "session.start" | "session.checkpoint" | "session.end" => SessionMaintainOwn,
        "claim.acquire" | "claim.renew" | "claim.release" | "handoff.propose"
        | "handoff.accept" | "handoff.reject" | "handoff.cancel" | "handoff.timeout"
        | "handoff.inspect" => ClaimManageOwn,
        "execution.prepare" | "execution.start" | "execution.cancel" | "execution.report" => {
            ExecutionRequestAndReportOwn
        }
        "evidence.submit"
        | "review.open"
        | "delivery.submit_and_request_review"
        | "delivery.register_pr"
        | "delivery.observe_pr"
        | "delivery.candidate.select"
        | "delivery.inspection.reserve"
        | "delivery.facts.ingest" => DeliverySubmitAndRequestReview,
        "review.accept" | "review.return" | "review.decide" => ReviewDecide,
        // Rework is author/executor acknowledgment of a return — not independent review.
        "work.rework" => DeliverySubmitAndRequestReview,
        "work.complete" | "delivery.finalize" => DeliveryFinalize,
        "planning.propose" => PlanningPropose,
        "planning.edit_draft" => PlanningEditDraft,
        "planning.approve" => PlanningApprove,
        "planning.publish" => PlanningPublish,
        "access.manage_project"
        | "delivery.connector.configure"
        | "delivery.source.prepare"
        | "delivery.source.renew"
        | "delivery.source.write"
        | "delivery.source.confirm"
        | "delivery.source.abandon" => AccessManageProject,
        "audit.read_project" => AuditReadProject,
        "execution.attest" | "execution.reconcile" => return None,
        _ => return None,
    })
}

/// Every authenticated workstream query is a `work.read` decision. Discovery
/// lists are navigation-only and still require live read authority.
pub fn query_business_action(op: &str) -> Option<awr_team::Action> {
    const QUERIES: &[&str] = &[
        "capabilities",
        "workstreams.list",
        "work.list",
        "work.search",
        "work.prepare",
        "work.snapshot",
        "work.observe",
        "work.next",
        "audit.requests",
        "audit.development",
        "events.list",
        "session.inspect",
        "work.recovery",
        "command.inspect",
        "claim.inspect",
        "execution.inspect",
        "handoff.inspect",
        "evidence.inspect",
        "review.inspect",
        "completion.inspect",
        "delivery.inspect",
        "delivery.neutral.inspect",
        "delivery.neutral.outcome",
        "delivery.source.status",
        "source.content",
        "artifact.content",
        "planning.outcome",
        "audit.history",
        "audit.export",
        "audit.count",
    ];
    if QUERIES.contains(&op) {
        Some(awr_team::Action::WorkRead)
    } else {
        None
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build the TMCP-010 authority scope from verified credential facts only.
pub(crate) fn authority_scope(
    auth: &ReaderAuthority,
    stream: Option<Id>,
    work_id: Option<&str>,
) -> awr_team::AuthorityScope {
    let mut scope = awr_team::authority_from_template(
        auth.role_template,
        auth.tenant_id.clone(),
        auth.access.project_id.clone(),
        auth.actor_id.clone(),
        auth.client_id.clone(),
    );
    scope.workstream_id = stream.map(|id| id.to_string());
    if let Some(work) = work_id {
        if !work.is_empty() {
            scope.work_ids.insert(work.into());
        }
    }
    scope.execution_identity = Some(auth.actor_id.clone());
    scope.allowed_actions = auth.membership_actions();
    if let Some(actions) = &auth.delegated_actions {
        // Agents: never inherit the full membership template (TMCP-030).
        scope.allowed_actions = scope
            .allowed_actions
            .intersection(actions)
            .copied()
            .collect();
        // Delegation alone never confers independent review.
        scope.independent_review_grant = false;
        scope.agent_review_grant = auth.actor_kind == "agent"
            && auth.agent_review
            && scope
                .allowed_actions
                .contains(&awr_team::Action::ReviewDecide);
    } else if scope
        .allowed_actions
        .contains(&awr_team::Action::ReviewDecide)
    {
        // Explicit membership grant on an eligible template (TMCP-031).
        scope.independent_review_grant = true;
    }
    scope.policy_version = awr_team::PERMISSION_POLICY_VERSION;
    scope.revoked = false;
    scope
}

/// Shared action decision used by PG command/query paths and HTTP/MCP (same store).
/// Bodies, tool names, reconnects and old protocols never supply the scope.
pub(crate) fn authorize_domain_action(
    auth: &ReaderAuthority,
    action: awr_team::Action,
    stream: Option<Id>,
    work_id: Option<&str>,
) -> PgResult<()> {
    // Re-derive from the live membership label so stale template caches cannot drift.
    let live = map_membership_role(&auth.role).ok_or(PgError::Forbidden)?;
    if crate::delegation_auth::actor_requires_explicit_delegation(&auth.actor_kind) {
        match &auth.delegated_actions {
            None => return Err(PgError::Forbidden),
            Some(actions) if actions.is_empty() => return Err(PgError::Forbidden),
            Some(actions) if !actions.contains(&action) => return Err(PgError::Forbidden),
            Some(_) => {}
        }
    }
    if live != auth.role_template || auth.membership_version < 1 {
        return Err(PgError::Forbidden);
    }
    let scope = authority_scope(auth, stream, work_id);
    let resource = awr_team::ResourceRef {
        tenant_id: auth.tenant_id.clone(),
        project_id: auth.access.project_id.clone(),
        workstream_id: stream.map(|id| id.to_string()),
        work_id: work_id.filter(|s| !s.is_empty()).map(str::to_owned),
    };
    awr_team::authorize_action(&scope, action, &resource, now_unix_ms()).map_err(|err| match err {
        awr_team::TeamError::PermissionDenied(_) => PgError::Forbidden,
        _ => PgError::Forbidden,
    })
}

/// Shared Team domain-entry authority for HTTP/MCP/PG command paths.
/// Combines the WS-014 write-boundary contract with TMCP-010 business actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DomainAuthority {
    /// Explicit write grant that may preserve work while a stream is paused.
    WritePreserve,
    /// Explicit write grant on an active workstream (ordinary mutations).
    WriteActive,
    /// Explicit human/system project-management and workstream management scope.
    ManageProject,
    /// Trusted executor attestation (system actor + explicit grant).
    Attest,
    /// Operator reconciliation (manage + explicit reconcile grant).
    Reconcile,
}

/// When authorization is rechecked relative to idempotent replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandAuthPhase {
    /// Before mutation: permits exact receipt replay after credential checks.
    Admission,
    /// After replay miss: enforces stream activity and special execution grants.
    Effect,
}

/// Map a Team workstream command op to its domain authority. Unknown ops are
/// unsupported capabilities and must be refused by callers.
pub(crate) fn command_authority(op: &str) -> Option<DomainAuthority> {
    Some(match op {
        "session.checkpoint" | "session.end" | "claim.release" | "execution.cancel"
        | "execution.report" | "handoff.reject" | "handoff.cancel" | "handoff.timeout"
        | "handoff.inspect" | "review.return" | "work.rework" => DomainAuthority::WritePreserve,
        "session.start"
        | "task.assign"
        | "task.accept_assignment"
        | "task.claim_available"
        | "claim.acquire"
        | "claim.renew"
        | "execution.prepare"
        | "execution.start"
        | "handoff.propose"
        | "handoff.accept"
        | "evidence.submit"
        | "review.open"
        | "review.accept"
        | "review.decide"
        | "work.complete"
        | "delivery.finalize"
        | "delivery.submit_and_request_review"
        | "delivery.register_pr"
        | "delivery.observe_pr"
        | "delivery.candidate.select"
        | "delivery.inspection.reserve"
        | "delivery.facts.ingest" => DomainAuthority::WriteActive,
        "delivery.connector.configure"
        | "delivery.source.prepare"
        | "delivery.source.renew"
        | "delivery.source.write"
        | "delivery.source.confirm"
        | "delivery.source.abandon" => DomainAuthority::ManageProject,
        "execution.attest" => DomainAuthority::Attest,
        "execution.reconcile" => DomainAuthority::Reconcile,
        _ => return None,
    })
}

fn has_write(auth: &ReaderAuthority, stream: Id) -> bool {
    auth.access
        .grants
        .iter()
        .any(|grant| grant.workstream_id == stream && grant.write)
}

/// Shared authorization used by Team PG command dispatch (HTTP/MCP call the same
/// store). Request bodies, tool names and reconnects never supply grants.
///
/// Admission requires a current read grant and the matching TMCP-010 business
/// action. Ordinary/special commands also require an explicit write bit; neutral
/// management instead requires eligible identity and explicit management scope.
/// Readers cannot mutate and developers cannot exercise access privileges. Effect adds
/// active-stream or attest/reconcile checks after idempotent replay so historical
/// receipts remain replayable for the original client even if the stream later
/// pauses or a special grant is revoked. Auth failure returns before business writes.
pub(crate) fn authorize_command(
    auth: &ReaderAuthority,
    stream: Id,
    work_id: &str,
    op: &str,
    phase: CommandAuthPhase,
) -> PgResult<()> {
    let required = command_authority(op).ok_or_else(|| {
        PgError::Unsupported(format!("unsupported workstream command capability: {op}"))
    })?;
    auth.access
        .authorize(&auth.catalog, stream, WorkstreamAction::Read)?;
    // Template actions for ordinary commands; attest/reconcile stay special-only.
    if let Some(action) = command_business_action(op) {
        authorize_domain_action(auth, action, Some(stream), Some(work_id))?;
    } else if !matches!(
        required,
        DomainAuthority::Attest | DomainAuthority::Reconcile
    ) {
        return Err(PgError::Forbidden);
    }
    // Neutral source/connector management follows its existing domain boundary:
    // management is not an ordinary developer write bit, including on replay.
    if required == DomainAuthority::ManageProject {
        if !matches!(auth.actor_kind.as_str(), "human" | "system") {
            return Err(PgError::Forbidden);
        }
        auth.access
            .authorize(&auth.catalog, stream, WorkstreamAction::Manage)?;
        return Ok(());
    }
    if !has_write(auth, stream) {
        return Err(PgError::Forbidden);
    }
    if matches!(
        required,
        DomainAuthority::Attest | DomainAuthority::Reconcile
    ) && (!role_ceiling_allows(
        auth.business_roles.as_ref(),
        awr_team::Action::ExecutionRequestAndReportOwn,
    ) || required == DomainAuthority::Reconcile
        && !role_ceiling_allows(
            auth.business_roles.as_ref(),
            awr_team::Action::AccessManageProject,
        ))
    {
        return Err(PgError::Forbidden);
    }
    if phase == CommandAuthPhase::Admission {
        return Ok(());
    }
    match required {
        DomainAuthority::ManageProject => unreachable!("management was checked before replay"),
        DomainAuthority::WritePreserve => Ok(()),
        DomainAuthority::WriteActive => {
            auth.access
                .authorize(&auth.catalog, stream, WorkstreamAction::Write)?;
            Ok(())
        }
        DomainAuthority::Attest => {
            if auth.actor_kind == "agent"
                || auth.actor_kind == "human" && auth.role_template != auth.role_template
            {
                // agents never attest; product-role humans also never auto-upgrade
                // (attest bit requires actor_kind==system in authenticate).
            }
            if auth.actor_kind != "system"
                || !auth
                    .execution_access
                    .get(&stream)
                    .is_some_and(|access| access.attest)
            {
                return Err(PgError::Forbidden);
            }
            Ok(())
        }
        DomainAuthority::Reconcile => {
            if auth.actor_kind == "agent"
                || !auth
                    .execution_access
                    .get(&stream)
                    .is_some_and(|access| access.reconcile)
            {
                return Err(PgError::Forbidden);
            }
            Ok(())
        }
    }
}

/// Shared read-side action gate (search/count/pagination/events/inspect).
pub(crate) fn authorize_query(
    auth: &ReaderAuthority,
    stream: Option<Id>,
    work_id: Option<&str>,
    op: &str,
) -> PgResult<()> {
    let action = query_business_action(op).ok_or_else(|| {
        PgError::Unsupported(format!("unsupported workstream query capability: {op}"))
    })?;
    authorize_domain_action(auth, action, stream, work_id)
}

/// Capability metadata that makes `scope=main` historical semantics, old-client
/// write refusal, and the local-file vs server-ACL boundary explicit. Callers
/// merge these into live capabilities responses; unsupported keys stay refused.
pub(crate) fn workstream_boundary_capabilities() -> serde_json::Value {
    json!({
        "scope_id": "main",
        "scope_main_semantics": "historical_team_rows_retain_scope_id_main_while_workstream_id_isolates",
        "old_client_write_boundary": "legacy_unscoped_team_entrypoints_refuse_enabled_projects",
        "authorization": "transactional_workstream_grants",
        "domain_entry_authorization": "shared_command_gate",
        "action_authorization": "tmcp_010_shared_decision",
        "delegation_action_intersection": "tmcp_030_ws016_intersect",
        "trusted_executor_upgrade": "never_from_product_roles",
        "permission_policy_id": awr_team::PERMISSION_POLICY_ID,
        "permission_policy_version": awr_team::PERMISSION_POLICY_VERSION,
        "write_authorization_phases": ["admission_write_grant", "effect_active_or_special"],
        "unsupported_capabilities": "refused",
        "local_file_access": "not_server_acl_or_confidentiality_sandbox",
        "reference_runner_effects": "operator_local_bounded_files_require_explicit_attestation_grant",
        "operator_recovery_inspection": "schema_owner_cli_read_only_enabled_projects",
        "operator_history_migration": "sessions_inactive_claims_events_v1",
        "operator_enabled_backup": "logical_manifest_fencing_and_ownership_rebuild_v1",
        "frontend_filtering": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::{
        WORKSTREAM_CATALOG_VERSION, Workstream, WorkstreamCatalog, WorkstreamGrant, WorkstreamState,
    };

    fn id(value: u128) -> Id {
        Id::from(value)
    }

    fn catalog(state: WorkstreamState) -> WorkstreamCatalog {
        WorkstreamCatalog {
            version: WORKSTREAM_CATALOG_VERSION,
            project_id: "project".into(),
            legacy_default: Some(id(1)),
            workstreams: vec![Workstream {
                id: id(1),
                project_id: "project".into(),
                external_key: "api".into(),
                title: "api".into(),
                state,
                authority_version: 1,
                goal_keys: vec!["g".into()],
                acceptance_contracts: vec!["c".into()],
            }],
        }
    }

    fn authority(
        write: bool,
        manage: bool,
        attest: bool,
        reconcile: bool,
        state: WorkstreamState,
    ) -> ReaderAuthority {
        authority_with_role(
            if manage {
                "admin"
            } else if write {
                "worker"
            } else {
                "reader"
            },
            write,
            manage,
            attest,
            reconcile,
            state,
        )
    }

    fn authority_with_role(
        role: &str,
        write: bool,
        manage: bool,
        attest: bool,
        reconcile: bool,
        state: WorkstreamState,
    ) -> ReaderAuthority {
        let stream = id(1);
        let mut execution_access = BTreeMap::new();
        execution_access.insert(stream, ExecutionAccess { attest, reconcile });
        let role_template = map_membership_role(role).expect("test role");
        ReaderAuthority {
            tenant_id: "tenant".into(),
            actor_id: "actor".into(),
            client_id: "client".into(),
            actor_kind: "system".into(),
            role: role.into(),
            role_template,
            membership_version: 1,
            business_roles: None,
            assignment_grant: false,
            independent_review: false,
            agent_review: false,
            execution_access,
            access: WorkstreamAccess {
                project_id: "project".into(),
                subject: "subject".into(),
                grants: vec![WorkstreamGrant {
                    workstream_id: stream,
                    authority_version: 1,
                    read: true,
                    write,
                    manage,
                }],
            },
            catalog: catalog(state),
            snapshot: "snap".into(),
            epoch: "epoch".into(),
            project_status: "active".into(),
            revision: 1,
            binding: "binding".into(),
            grant_versions: BTreeMap::from([(stream, 1)]),
            delegated_actions: None,
            delegation_id: None,
            read_delegations: None,
            navigation_read_scope: None,
        }
    }

    #[test]
    fn business_ceiling_is_applied_after_legacy_and_explicit_review_grants() {
        use awr_team::{Action, BusinessRole};
        use std::collections::BTreeSet;
        let mut auth = authority_with_role(
            "reviewer",
            true,
            false,
            false,
            false,
            WorkstreamState::Active,
        );
        assert!(
            authorize_domain_action(&auth, Action::ReviewDecide, Some(id(1)), Some("work")).is_ok()
        );
        auth.business_roles = Some(BTreeSet::from([BusinessRole::Observer]));
        assert!(
            authorize_domain_action(&auth, Action::ReviewDecide, Some(id(1)), Some("work"))
                .is_err()
        );
        auth.business_roles = Some(BTreeSet::from([BusinessRole::Reviewer]));
        assert!(
            authorize_domain_action(&auth, Action::ReviewDecide, Some(id(1)), Some("work")).is_ok()
        );
        auth.role = "project_admin".into();
        auth.role_template = awr_team::RoleTemplate::ProjectAdmin;
        assert!(
            authorize_domain_action(&auth, Action::ReviewDecide, Some(id(1)), Some("work"))
                .is_err()
        );
        auth.independent_review = true;
        assert!(
            authorize_domain_action(&auth, Action::ReviewDecide, Some(id(1)), Some("work")).is_ok()
        );
        auth.business_roles = Some(BTreeSet::from([BusinessRole::Supervisor]));
        assert!(
            authorize_domain_action(&auth, Action::ReviewDecide, Some(id(1)), Some("work"))
                .is_err()
        );
        auth.actor_kind = "agent".into();
        auth.agent_review = true;
        auth.delegated_actions = Some(BTreeSet::from([Action::WorkRead, Action::ReviewDecide]));
        assert!(
            authorize_domain_action(&auth, Action::ReviewDecide, Some(id(1)), Some("work"))
                .is_err()
        );
        auth.business_roles = Some(BTreeSet::from([BusinessRole::Reviewer]));
        assert!(
            authorize_domain_action(&auth, Action::ReviewDecide, Some(id(1)), Some("work")).is_ok()
        );
    }

    #[test]
    fn business_duties_never_create_or_bypass_special_execution_authority() {
        use awr_team::BusinessRole;
        use std::collections::BTreeSet;
        let mut auth = authority_with_role(
            "project_admin",
            true,
            true,
            true,
            true,
            WorkstreamState::Active,
        );
        for phase in [CommandAuthPhase::Admission, CommandAuthPhase::Effect] {
            assert!(authorize_command(&auth, id(1), "work", "execution.attest", phase).is_ok());
            auth.business_roles = Some(BTreeSet::from([BusinessRole::Observer]));
            assert!(authorize_command(&auth, id(1), "work", "execution.attest", phase).is_err());
            assert!(authorize_command(&auth, id(1), "work", "execution.reconcile", phase).is_err());
            assert!(!crate::delegation_auth::execution_side_effect_permitted(
                &auth
            ));
            auth.business_roles = Some(BTreeSet::from([
                BusinessRole::Developer,
                BusinessRole::Administrator,
            ]));
            assert!(authorize_command(&auth, id(1), "work", "execution.reconcile", phase).is_ok());
            auth.business_roles = None;
        }
        auth.execution_access.get_mut(&id(1)).unwrap().attest = false;
        auth.business_roles = Some(BTreeSet::from([BusinessRole::Developer]));
        assert!(
            authorize_command(
                &auth,
                id(1),
                "work",
                "execution.attest",
                CommandAuthPhase::Effect
            )
            .is_err()
        );
    }

    #[test]
    fn every_supported_command_maps_to_a_domain_authority() {
        for op in crate::workstream_command::COMMANDS {
            assert!(
                command_authority(op).is_some(),
                "missing authority mapping for {op}"
            );
        }
        assert_eq!(command_authority("planning.publish"), None);
        assert_eq!(command_authority("work.claim"), None);
    }

    #[test]
    fn neutral_management_requires_management_scope_and_eligible_identity_on_replay() {
        for op in [
            "delivery.connector.configure",
            "delivery.source.prepare",
            "delivery.source.renew",
            "delivery.source.write",
            "delivery.source.confirm",
            "delivery.source.abandon",
        ] {
            assert_eq!(command_authority(op), Some(DomainAuthority::ManageProject));
            assert_eq!(
                command_business_action(op),
                Some(awr_team::Action::AccessManageProject)
            );
            for phase in [CommandAuthPhase::Admission, CommandAuthPhase::Effect] {
                let developer = authority(true, false, false, false, WorkstreamState::Active);
                assert!(authorize_command(&developer, id(1), "work", op, phase).is_err());
                let mut manager = authority(false, true, false, false, WorkstreamState::Active);
                assert!(authorize_command(&manager, id(1), "work", op, phase).is_ok());
                manager.actor_kind = "agent".into();
                manager.delegated_actions = Some(std::collections::BTreeSet::from([
                    awr_team::Action::AccessManageProject,
                ]));
                assert!(authorize_command(&manager, id(1), "work", op, phase).is_err());
            }
        }
        for op in [
            "delivery.candidate.select",
            "delivery.inspection.reserve",
            "delivery.facts.ingest",
        ] {
            assert_eq!(command_authority(op), Some(DomainAuthority::WriteActive));
            assert_eq!(
                command_business_action(op),
                Some(awr_team::Action::DeliverySubmitAndRequestReview)
            );
        }
        for op in [
            "delivery.neutral.inspect",
            "delivery.neutral.outcome",
            "delivery.source.status",
        ] {
            assert_eq!(query_business_action(op), Some(awr_team::Action::WorkRead));
        }
    }

    #[test]
    fn admission_rejects_readers_and_unknown_capabilities() {
        let reader = authority(false, false, false, false, WorkstreamState::Active);
        assert!(matches!(
            authorize_command(
                &reader,
                id(1),
                "work-a",
                "session.start",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Forbidden)
        ));
        let writer = authority(true, false, false, false, WorkstreamState::Active);
        assert!(matches!(
            authorize_command(
                &writer,
                id(1),
                "work-a",
                "planning.publish",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Unsupported(_))
        ));
        assert!(
            authorize_command(
                &writer,
                id(1),
                "work-a",
                "session.checkpoint",
                CommandAuthPhase::Admission
            )
            .is_ok()
        );
    }

    #[test]
    fn effect_enforces_active_stream_and_special_grants_after_admission() {
        let paused_writer = authority(true, false, false, false, WorkstreamState::Paused);
        assert!(
            authorize_command(
                &paused_writer,
                id(1),
                "work-a",
                "session.end",
                CommandAuthPhase::Effect
            )
            .is_ok()
        );
        assert!(matches!(
            authorize_command(
                &paused_writer,
                id(1),
                "work-a",
                "session.start",
                CommandAuthPhase::Effect
            ),
            Err(PgError::Workstream(awr_core::WorkstreamError::Inactive))
        ));

        let writer = authority(true, false, false, false, WorkstreamState::Active);
        assert!(matches!(
            authorize_command(
                &writer,
                id(1),
                "work-a",
                "execution.attest",
                CommandAuthPhase::Effect
            ),
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            authorize_command(
                &writer,
                id(1),
                "work-a",
                "execution.reconcile",
                CommandAuthPhase::Effect
            ),
            Err(PgError::Forbidden)
        ));

        let attester = authority(true, false, true, false, WorkstreamState::Active);
        assert!(
            authorize_command(
                &attester,
                id(1),
                "work-a",
                "execution.attest",
                CommandAuthPhase::Effect
            )
            .is_ok()
        );

        let reconciler = authority(true, true, false, true, WorkstreamState::Active);
        assert!(
            authorize_command(
                &reconciler,
                id(1),
                "work-a",
                "execution.reconcile",
                CommandAuthPhase::Effect
            )
            .is_ok()
        );
    }

    #[test]
    fn membership_roles_map_onto_tmcp_templates() {
        assert_eq!(
            map_membership_role("reader"),
            Some(awr_team::RoleTemplate::Reader)
        );
        assert_eq!(
            map_membership_role("reviewer"),
            Some(awr_team::RoleTemplate::Reader)
        );
        assert_eq!(
            map_membership_role("worker"),
            Some(awr_team::RoleTemplate::Developer)
        );
        assert_eq!(
            map_membership_role("developer"),
            Some(awr_team::RoleTemplate::Developer)
        );
        assert_eq!(
            map_membership_role("maintainer"),
            Some(awr_team::RoleTemplate::Maintainer)
        );
        assert_eq!(
            map_membership_role("admin"),
            Some(awr_team::RoleTemplate::ProjectAdmin)
        );
        assert_eq!(
            map_membership_role("project_admin"),
            Some(awr_team::RoleTemplate::ProjectAdmin)
        );
        assert_eq!(map_membership_role("nope"), None);
    }

    #[test]
    fn command_ops_map_to_tmcp_actions_and_specials_stay_unmapped() {
        for op in ["session.start", "session.checkpoint", "session.end"] {
            assert_eq!(
                command_business_action(op),
                Some(awr_team::Action::SessionMaintainOwn)
            );
        }
        for op in ["claim.acquire", "claim.renew", "claim.release"] {
            assert_eq!(
                command_business_action(op),
                Some(awr_team::Action::ClaimManageOwn)
            );
        }
        for op in [
            "execution.prepare",
            "execution.start",
            "execution.cancel",
            "execution.report",
        ] {
            assert_eq!(
                command_business_action(op),
                Some(awr_team::Action::ExecutionRequestAndReportOwn)
            );
        }
        assert_eq!(command_business_action("execution.attest"), None);
        assert_eq!(command_business_action("execution.reconcile"), None);
        assert_eq!(
            command_business_action("planning.publish"),
            Some(awr_team::Action::PlanningPublish)
        );
        assert_eq!(
            query_business_action("work.search"),
            Some(awr_team::Action::WorkRead)
        );
        assert_eq!(
            query_business_action("events.list"),
            Some(awr_team::Action::WorkRead)
        );
        assert_eq!(query_business_action("unknown"), None);
    }

    #[test]
    fn reader_and_developer_action_matrix_at_domain_gate() {
        let reader = authority_with_role(
            "reader",
            false,
            false,
            false,
            false,
            WorkstreamState::Active,
        );
        assert!(matches!(
            authorize_domain_action(&reader, awr_team::Action::WorkRead, Some(id(1)), Some("w")),
            Ok(())
        ));
        assert!(matches!(
            authorize_domain_action(
                &reader,
                awr_team::Action::ClaimManageOwn,
                Some(id(1)),
                Some("w")
            ),
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            authorize_command(
                &reader,
                id(1),
                "w",
                "claim.acquire",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            authorize_command(
                &reader,
                id(1),
                "w",
                "review.accept",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            authorize_command(
                &reader,
                id(1),
                "w",
                "work.complete",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Forbidden)
        ));

        let developer =
            authority_with_role("worker", true, false, false, false, WorkstreamState::Active);
        assert!(
            authorize_command(
                &developer,
                id(1),
                "w",
                "session.start",
                CommandAuthPhase::Admission
            )
            .is_ok()
        );
        assert!(
            authorize_command(
                &developer,
                id(1),
                "w",
                "execution.prepare",
                CommandAuthPhase::Admission
            )
            .is_ok()
        );
        assert!(
            authorize_command(
                &developer,
                id(1),
                "w",
                "evidence.submit",
                CommandAuthPhase::Admission
            )
            .is_ok()
        );
        assert!(matches!(
            authorize_command(
                &developer,
                id(1),
                "w",
                "review.accept",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Forbidden)
        ));
        let reviewer = authority_with_role(
            "reviewer",
            true,
            false,
            false,
            false,
            WorkstreamState::Active,
        );
        assert!(
            authorize_command(
                &reviewer,
                id(1),
                "w",
                "review.accept",
                CommandAuthPhase::Admission
            )
            .is_ok()
        );
        assert!(matches!(
            authorize_command(
                &reviewer,
                id(1),
                "w",
                "evidence.submit",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            authorize_domain_action(
                &developer,
                awr_team::Action::PlanningPublish,
                Some(id(1)),
                Some("w")
            ),
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            authorize_domain_action(
                &developer,
                awr_team::Action::PlanningEditDraft,
                Some(id(1)),
                Some("w")
            ),
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            authorize_domain_action(
                &developer,
                awr_team::Action::AccessManageProject,
                Some(id(1)),
                Some("w")
            ),
            Err(PgError::Forbidden)
        ));

        let maintainer = authority_with_role(
            "maintainer",
            true,
            false,
            false,
            false,
            WorkstreamState::Active,
        );
        assert!(
            authorize_domain_action(
                &maintainer,
                awr_team::Action::PlanningPublish,
                Some(id(1)),
                Some("w")
            )
            .is_ok()
        );
        assert!(matches!(
            authorize_domain_action(
                &maintainer,
                awr_team::Action::AccessManageProject,
                Some(id(1)),
                Some("w")
            ),
            Err(PgError::Forbidden)
        ));

        let admin = authority_with_role("admin", true, true, false, false, WorkstreamState::Active);
        assert!(
            authorize_domain_action(
                &admin,
                awr_team::Action::AccessManageProject,
                Some(id(1)),
                Some("w")
            )
            .is_ok()
        );
        assert!(matches!(
            authorize_domain_action(
                &admin,
                awr_team::Action::ReviewDecide,
                Some(id(1)),
                Some("w")
            ),
            Err(PgError::Forbidden)
        ));
    }

    #[test]
    fn expired_or_revoked_scope_and_work_bounds_are_refused() {
        let auth = authority(true, false, false, false, WorkstreamState::Active);
        let resource = awr_team::ResourceRef {
            tenant_id: "tenant".into(),
            project_id: "project".into(),
            workstream_id: Some(id(1).to_string()),
            work_id: Some("w".into()),
        };
        let mut scope = authority_scope(&auth, Some(id(1)), Some("w"));
        scope.revoked = true;
        assert!(matches!(
            awr_team::authorize_action(&scope, awr_team::Action::SessionMaintainOwn, &resource, 1),
            Err(awr_team::TeamError::PermissionDenied(_))
        ));
        scope.revoked = false;
        scope.not_after_unix_ms = Some(10);
        assert!(matches!(
            awr_team::authorize_action(&scope, awr_team::Action::SessionMaintainOwn, &resource, 11),
            Err(awr_team::TeamError::PermissionDenied(_))
        ));
        scope.not_after_unix_ms = None;
        scope.policy_version = 0;
        assert!(matches!(
            awr_team::authorize_action(&scope, awr_team::Action::SessionMaintainOwn, &resource, 1),
            Err(awr_team::TeamError::PermissionDenied(_))
        ));
        scope.policy_version = awr_team::PERMISSION_POLICY_VERSION;
        let other = awr_team::ResourceRef {
            work_id: Some("other-work".into()),
            ..resource.clone()
        };
        assert!(matches!(
            awr_team::authorize_action(&scope, awr_team::Action::SessionMaintainOwn, &other, 1),
            Err(awr_team::TeamError::PermissionDenied(_))
        ));
        let other_ws = awr_team::ResourceRef {
            workstream_id: Some(id(2).to_string()),
            ..resource
        };
        assert!(matches!(
            awr_team::authorize_action(&scope, awr_team::Action::SessionMaintainOwn, &other_ws, 1),
            Err(awr_team::TeamError::PermissionDenied(_))
        ));
    }

    #[test]
    fn boundary_capabilities_make_scope_main_and_local_file_limits_explicit() {
        let caps = workstream_boundary_capabilities();
        assert_eq!(caps["scope_id"], "main");
        assert_eq!(caps["frontend_filtering"], false);
        assert_eq!(
            caps["operator_recovery_inspection"],
            "schema_owner_cli_read_only_enabled_projects"
        );
        assert_eq!(
            caps["operator_history_migration"],
            "sessions_inactive_claims_events_v1"
        );
        assert_eq!(
            caps["operator_enabled_backup"],
            "logical_manifest_fencing_and_ownership_rebuild_v1"
        );
        assert_eq!(caps["unsupported_capabilities"], "refused");
        assert_eq!(
            caps["local_file_access"],
            "not_server_acl_or_confidentiality_sandbox"
        );
        assert_eq!(
            caps["old_client_write_boundary"],
            "legacy_unscoped_team_entrypoints_refuse_enabled_projects"
        );
        assert_eq!(caps["domain_entry_authorization"], "shared_command_gate");
        assert_eq!(caps["action_authorization"], "tmcp_010_shared_decision");
        assert_eq!(caps["permission_policy_id"], awr_team::PERMISSION_POLICY_ID);
        assert_eq!(
            caps["permission_policy_version"],
            awr_team::PERMISSION_POLICY_VERSION
        );
    }
}
