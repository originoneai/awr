//! Authorized agents, narrowed delegation, and explainable claim rules (WS-016).
//!
//! Builds on the WS-015 person / execution-instance model:
//! - Agents act only under an explicit, inspectable, revocable authorization.
//! - Claim eligibility is explained from membership, person delegation, assignment
//!   policy, resources, and host capability. Self-reported skills are hints only.
//! - Responsibility accept, collaborative occupancy, and start-work admission are
//!   separate decisions. Unmet dependencies may allow ownership, but must not admit
//!   side-effecting execution. Concurrent claims yield one effective executor.
//! - Delegation cannot escalate; child grants are strict subsets. Changing model,
//!   client, or session cannot bypass revoke, expiry, scope, or reviewer separation.
use crate::{Error, ExecutionInstance, PersonId, Result, TaskResponsibility};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

fn validate_id(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.len() > 128
        || value.chars().any(|c| c.is_control() || c == '\0')
    {
        return Err(Error::InvalidInput(format!(
            "{field} requires a bounded non-empty identity"
        )));
    }
    Ok(())
}

fn validate_request_key(key: &str) -> Result<()> {
    validate_id(key, "request_key")
}

/// Business actions an authorization may permit. Unknown actions default-deny.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizedAction {
    Inspect,
    AcceptResponsibility,
    OccupyCollaboratively,
    ClaimCoordination,
    StartWork,
    /// Submit inert suggestions for explicitly covered work; no plan mutation.
    ProposePlanning,
    /// Assign responsibility; never starts execution or edits the plan.
    AssignWork,
    EditPlanning,
    ApprovePlanning,
    PublishPlanning,
    /// Finalize an independently accepted delivery; never grants review.
    FinalizeDelivery,
    Review,
    ManageAuthorization,
}

impl AuthorizedAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::AcceptResponsibility => "accept_responsibility",
            Self::OccupyCollaboratively => "occupy_collaboratively",
            Self::ClaimCoordination => "claim_coordination",
            Self::StartWork => "start_work",
            Self::ProposePlanning => "propose_planning",
            Self::AssignWork => "assign_work",
            Self::EditPlanning => "edit_planning",
            Self::ApprovePlanning => "approve_planning",
            Self::PublishPlanning => "publish_planning",
            Self::FinalizeDelivery => "finalize_delivery",
            Self::Review => "review",
            Self::ManageAuthorization => "manage_authorization",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "inspect" => Ok(Self::Inspect),
            "accept_responsibility" => Ok(Self::AcceptResponsibility),
            "occupy_collaboratively" => Ok(Self::OccupyCollaboratively),
            "claim_coordination" => Ok(Self::ClaimCoordination),
            "start_work" => Ok(Self::StartWork),
            "propose_planning" => Ok(Self::ProposePlanning),
            "assign_work" => Ok(Self::AssignWork),
            "edit_planning" => Ok(Self::EditPlanning),
            "approve_planning" => Ok(Self::ApprovePlanning),
            "publish_planning" => Ok(Self::PublishPlanning),
            "finalize_delivery" => Ok(Self::FinalizeDelivery),
            "review" => Ok(Self::Review),
            "manage_authorization" => Ok(Self::ManageAuthorization),
            _ => Err(Error::InvalidInput(format!(
                "unknown authorized action '{value}'; default deny"
            ))),
        }
    }
}

/// Scope of an agent authorization. Child scopes must nest inside the parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthorizationScope {
    Project {
        project_id: String,
    },
    Workstream {
        project_id: String,
        workstream_id: String,
    },
    Task {
        project_id: String,
        work_item_id: String,
    },
    TaskPool {
        project_id: String,
        pool_id: String,
    },
}

impl AuthorizationScope {
    pub fn project_id(&self) -> &str {
        match self {
            Self::Project { project_id }
            | Self::Workstream { project_id, .. }
            | Self::Task { project_id, .. }
            | Self::TaskPool { project_id, .. } => project_id,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Project { project_id } => validate_id(project_id, "project_id"),
            Self::Workstream {
                project_id,
                workstream_id,
            } => {
                validate_id(project_id, "project_id")?;
                validate_id(workstream_id, "workstream_id")
            }
            Self::Task {
                project_id,
                work_item_id,
            } => {
                validate_id(project_id, "project_id")?;
                validate_id(work_item_id, "work_item_id")
            }
            Self::TaskPool {
                project_id,
                pool_id,
            } => {
                validate_id(project_id, "project_id")?;
                validate_id(pool_id, "pool_id")
            }
        }
    }

    /// True when `self` equals or is strictly nested under `parent`.
    pub fn is_within(&self, parent: &Self) -> bool {
        if self.project_id() != parent.project_id() {
            return false;
        }
        match (parent, self) {
            (Self::Project { .. }, _) => true,
            (
                Self::Workstream {
                    workstream_id: p, ..
                },
                Self::Workstream {
                    workstream_id: c, ..
                },
            ) => p == c,
            (
                Self::Task {
                    work_item_id: p, ..
                },
                Self::Task {
                    work_item_id: c, ..
                },
            ) => p == c,
            (Self::TaskPool { pool_id: p, .. }, Self::TaskPool { pool_id: c, .. }) => p == c,
            (
                Self::TaskPool { pool_id: p, .. },
                Self::Task {
                    work_item_id: c, ..
                },
            ) => c == p || c.starts_with(&format!("{p}/")),
            _ => false,
        }
    }
}

/// Capability that can be independently verified. Self-reported skills are never here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VerifiableCapability {
    HostDeclared {
        host_id: String,
        capability_id: String,
        proof_digest: String,
    },
    ResourceLease {
        resource_id: String,
        lease_id: String,
    },
    Membership {
        project_id: String,
        membership_version: u64,
    },
}

impl VerifiableCapability {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::HostDeclared {
                host_id,
                capability_id,
                proof_digest,
            } => {
                validate_id(host_id, "host_id")?;
                validate_id(capability_id, "capability_id")?;
                if proof_digest.len() < 16
                    || proof_digest.len() > 128
                    || !proof_digest
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'-')
                {
                    return Err(Error::InvalidInput(
                        "host capability proof_digest must be a bounded verifiable digest".into(),
                    ));
                }
            }
            Self::ResourceLease {
                resource_id,
                lease_id,
            } => {
                validate_id(resource_id, "resource_id")?;
                validate_id(lease_id, "lease_id")?;
            }
            Self::Membership { project_id, .. } => validate_id(project_id, "project_id")?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationStatus {
    Active,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSubjectKind {
    Person,
    Agent,
    /// Team platform service account; requires `maintainer_person_id`.
    PlatformService,
}

/// Explicit agent (or person/service) authorization grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAuthorization {
    pub id: String,
    pub authorizer_person_id: PersonId,
    pub responsible_person_id: PersonId,
    pub subject_kind: ExecutionSubjectKind,
    pub subject_id: String,
    pub client_id: String,
    pub session_id: Option<String>,
    pub model_id: Option<String>,
    pub scope: AuthorizationScope,
    pub actions: BTreeSet<AuthorizedAction>,
    pub expires_at_ms: Option<i64>,
    pub status: AuthorizationStatus,
    pub revoked_at_ms: Option<i64>,
    pub revoked_by: Option<PersonId>,
    pub verifiable_capabilities: Vec<VerifiableCapability>,
    /// Matching hints only — never substitute for permission or proof.
    pub self_reported_skill_hints: Vec<String>,
    pub parent_authorization_id: Option<String>,
    pub maintainer_person_id: Option<PersonId>,
    pub created_at_ms: i64,
    pub binding_id: Option<String>,
}

impl AgentAuthorization {
    pub fn validate(&self) -> Result<()> {
        validate_id(&self.id, "authorization_id")?;
        validate_id(self.authorizer_person_id.as_str(), "authorizer_person_id")?;
        validate_id(self.responsible_person_id.as_str(), "responsible_person_id")?;
        validate_id(&self.subject_id, "subject_id")?;
        validate_id(&self.client_id, "client_id")?;
        if let Some(session) = &self.session_id {
            validate_id(session, "session_id")?;
        }
        if let Some(model) = &self.model_id {
            validate_id(model, "model_id")?;
        }
        if let Some(binding) = &self.binding_id {
            validate_id(binding, "binding_id")?;
        }
        if let Some(parent) = &self.parent_authorization_id {
            validate_id(parent, "parent_authorization_id")?;
        }
        self.scope.validate()?;
        if self.actions.is_empty() {
            return Err(Error::InvalidInput(
                "authorization requires at least one explicit action".into(),
            ));
        }
        if self.actions.len() > 32 {
            return Err(Error::InvalidInput(
                "authorization actions exceed the 32-action bound".into(),
            ));
        }
        if self.verifiable_capabilities.len() > 64 {
            return Err(Error::InvalidInput(
                "verifiable_capabilities exceed the 64-item bound".into(),
            ));
        }
        for cap in &self.verifiable_capabilities {
            cap.validate()?;
        }
        if self.self_reported_skill_hints.len() > 32 {
            return Err(Error::InvalidInput(
                "self_reported_skill_hints exceed the 32-item bound".into(),
            ));
        }
        for hint in &self.self_reported_skill_hints {
            if hint.trim().is_empty() || hint.len() > 256 {
                return Err(Error::InvalidInput(
                    "self_reported_skill_hints require bounded non-empty strings".into(),
                ));
            }
        }
        if self.created_at_ms < 0 {
            return Err(Error::InvalidInput(
                "created_at_ms must be non-negative".into(),
            ));
        }
        if let Some(exp) = self.expires_at_ms {
            if exp < self.created_at_ms {
                return Err(Error::InvalidInput(
                    "expires_at_ms must not precede created_at_ms".into(),
                ));
            }
        }
        match self.subject_kind {
            ExecutionSubjectKind::Person => {
                if self.subject_id != self.responsible_person_id.as_str() {
                    return Err(Error::RuleViolation(
                        "person subject_id must equal responsible_person_id".into(),
                    ));
                }
            }
            ExecutionSubjectKind::Agent => {
                if self.binding_id.is_none() {
                    return Err(Error::RuleViolation(
                        "agent authorization requires an explicit person↔agent binding_id".into(),
                    ));
                }
            }
            ExecutionSubjectKind::PlatformService => {
                let Some(maintainer) = &self.maintainer_person_id else {
                    return Err(Error::RuleViolation(
                        "platform service account authorization requires a maintainer person"
                            .into(),
                    ));
                };
                validate_id(maintainer.as_str(), "maintainer_person_id")?;
            }
        }
        if matches!(self.status, AuthorizationStatus::Revoked)
            && (self.revoked_at_ms.is_none() || self.revoked_by.is_none())
        {
            return Err(Error::InvalidInput(
                "revoked authorization requires revoked_at_ms and revoked_by".into(),
            ));
        }
        Ok(())
    }

    pub fn is_effective_at(&self, now_ms: i64) -> bool {
        if !matches!(self.status, AuthorizationStatus::Active) {
            return false;
        }
        if let Some(exp) = self.expires_at_ms {
            if now_ms >= exp {
                return false;
            }
        }
        true
    }

    pub fn permits(&self, action: AuthorizedAction) -> bool {
        self.actions.contains(&action)
    }

    /// Whether this grant covers `work_item_id` given verified task ownership.
    ///
    /// `task_workstream_id` must be the authoritative owning workstream when the
    /// task is stream-owned; pass `None` for project-local / unscoped tasks.
    /// Workstream-scoped grants admit only tasks owned by that stream.
    pub fn covers_task(
        &self,
        project_id: &str,
        work_item_id: &str,
        task_workstream_id: Option<&str>,
    ) -> bool {
        if self.scope.project_id() != project_id {
            return false;
        }
        match &self.scope {
            AuthorizationScope::Project { .. } => true,
            AuthorizationScope::Workstream { workstream_id, .. } => {
                task_workstream_id == Some(workstream_id.as_str())
            }
            AuthorizationScope::Task {
                work_item_id: scoped,
                ..
            } => scoped == work_item_id,
            AuthorizationScope::TaskPool { pool_id, .. } => {
                work_item_id == pool_id || work_item_id.starts_with(&format!("{pool_id}/"))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueAuthorizationRequest {
    pub request_key: String,
    pub authorization: AgentAuthorization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeAuthorizationRequest {
    pub request_key: String,
    pub authorization_id: String,
    pub revoked_by: PersonId,
    pub revoked_at_ms: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegateAuthorizationRequest {
    pub request_key: String,
    pub parent_authorization_id: String,
    pub child: AgentAuthorization,
}

pub fn validate_issue(req: &IssueAuthorizationRequest) -> Result<()> {
    validate_request_key(&req.request_key)?;
    req.authorization.validate()?;
    if !matches!(req.authorization.status, AuthorizationStatus::Active) {
        return Err(Error::InvalidInput(
            "newly issued authorization must be active".into(),
        ));
    }
    if req.authorization.revoked_at_ms.is_some() || req.authorization.revoked_by.is_some() {
        return Err(Error::InvalidInput(
            "newly issued authorization must not carry revoke metadata".into(),
        ));
    }
    Ok(())
}

/// The grant's scope project is the project it may admit. Callers must not
/// store it under a different project id.
pub fn require_authorization_project(auth: &AgentAuthorization, project_id: &str) -> Result<()> {
    if auth.scope.project_id() != project_id {
        return Err(Error::RuleViolation(
            "authorization scope project does not match the addressed project".into(),
        ));
    }
    Ok(())
}

pub fn apply_revoke(
    current: &AgentAuthorization,
    req: &RevokeAuthorizationRequest,
) -> Result<AgentAuthorization> {
    validate_request_key(&req.request_key)?;
    validate_id(&req.authorization_id, "authorization_id")?;
    validate_id(req.revoked_by.as_str(), "revoked_by")?;
    if req.reason.trim().is_empty() || req.reason.len() > 2048 {
        return Err(Error::InvalidInput(
            "revoke reason requires 1..2048 bytes".into(),
        ));
    }
    if current.id != req.authorization_id {
        return Err(Error::NotFound(
            "authorization id does not match the loaded grant".into(),
        ));
    }
    if matches!(current.status, AuthorizationStatus::Revoked) {
        return Ok(current.clone());
    }
    let allowed = req.revoked_by == current.authorizer_person_id
        || req.revoked_by == current.responsible_person_id
        || current
            .maintainer_person_id
            .as_ref()
            .is_some_and(|m| m == &req.revoked_by);
    if !allowed {
        return Err(Error::RuleViolation(
            "revoke requires authorizer, responsible person, or platform maintainer".into(),
        ));
    }
    let mut next = current.clone();
    next.status = AuthorizationStatus::Revoked;
    next.revoked_at_ms = Some(req.revoked_at_ms);
    next.revoked_by = Some(req.revoked_by.clone());
    next.validate()?;
    Ok(next)
}

/// Narrowing-only delegation. Child cannot escalate actions, scope, or lifetime.
pub fn apply_delegate(
    parent: &AgentAuthorization,
    req: &DelegateAuthorizationRequest,
    now_ms: i64,
) -> Result<AgentAuthorization> {
    validate_request_key(&req.request_key)?;
    if parent.id != req.parent_authorization_id {
        return Err(Error::NotFound(
            "parent authorization id does not match the loaded grant".into(),
        ));
    }
    if !parent.is_effective_at(now_ms) {
        return Err(Error::RuleViolation(
            "cannot delegate from a revoked or expired authorization".into(),
        ));
    }
    if !(parent.permits(AuthorizedAction::ManageAuthorization)
        || parent.permits(AuthorizedAction::OccupyCollaboratively)
        || parent.permits(AuthorizedAction::StartWork))
    {
        return Err(Error::RuleViolation(
            "parent authorization cannot create child agent grants".into(),
        ));
    }
    let mut child = req.child.clone();
    child.parent_authorization_id = Some(parent.id.clone());
    child.validate()?;
    if child.responsible_person_id != parent.responsible_person_id {
        return Err(Error::RuleViolation(
            "delegation cannot transfer responsibility to a different person".into(),
        ));
    }
    if child.authorizer_person_id != parent.responsible_person_id
        && child.authorizer_person_id != parent.authorizer_person_id
    {
        return Err(Error::RuleViolation(
            "child authorizer must be the parent responsible person or parent authorizer".into(),
        ));
    }
    if !child.actions.is_subset(&parent.actions) {
        return Err(Error::RuleViolation(
            "delegation cannot escalate actions beyond the parent grant".into(),
        ));
    }
    if child.scope != parent.scope && !child.scope.is_within(&parent.scope) {
        return Err(Error::RuleViolation(
            "delegation cannot widen authorization scope".into(),
        ));
    }
    match (parent.expires_at_ms, child.expires_at_ms) {
        (Some(p), Some(c)) if c > p => {
            return Err(Error::RuleViolation(
                "delegation cannot extend expiry beyond the parent grant".into(),
            ));
        }
        (Some(_), None) => {
            return Err(Error::RuleViolation(
                "delegation from an expiring parent requires a child expiry".into(),
            ));
        }
        _ => {}
    }
    if !matches!(child.status, AuthorizationStatus::Active) {
        return Err(Error::InvalidInput(
            "delegated child must be issued as active".into(),
        ));
    }
    Ok(child)
}

/// Changing model, client, or session never resurrects a revoked/expired grant.
pub fn bind_runtime_identity(
    auth: &AgentAuthorization,
    client_id: &str,
    session_id: Option<&str>,
    model_id: Option<&str>,
    now_ms: i64,
) -> Result<()> {
    validate_id(client_id, "client_id")?;
    if let Some(s) = session_id {
        validate_id(s, "session_id")?;
    }
    if let Some(m) = model_id {
        validate_id(m, "model_id")?;
    }
    if !auth.is_effective_at(now_ms) {
        return Err(Error::RuleViolation(
            "model/client/session change cannot bypass revoke or expiry".into(),
        ));
    }
    if auth.client_id != client_id {
        return Err(Error::RuleViolation(
            "client change requires a new explicit authorization; it cannot reuse a foreign grant"
                .into(),
        ));
    }
    if let Some(bound) = &auth.session_id {
        if session_id != Some(bound.as_str()) {
            return Err(Error::RuleViolation(
                "session change cannot reuse a session-bound authorization".into(),
            ));
        }
    }
    Ok(())
}

// --- Explainable claim eligibility -------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimDecisionKind {
    ResponsibilityAccept,
    CollaborativeOccupancy,
    StartWorkAdmission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaimEligibilityFactor {
    MembershipOk {
        project_id: String,
        membership_version: u64,
    },
    MembershipMissing {
        project_id: String,
        detail: String,
    },
    DelegationOk {
        authorization_id: String,
    },
    DelegationMissing {
        detail: String,
    },
    DelegationInactive {
        authorization_id: String,
        detail: String,
    },
    AssignmentPolicyOk {
        policy: String,
    },
    AssignmentPolicyDenied {
        policy: String,
        detail: String,
    },
    ResourceOk {
        resource_id: String,
    },
    ResourceUnavailable {
        resource_id: String,
        detail: String,
    },
    HostCapabilityVerified {
        host_id: String,
        capability_id: String,
    },
    HostCapabilityMissing {
        capability_id: String,
        detail: String,
    },
    ConcurrentExecutorPresent {
        person_id: String,
        detail: String,
    },
    ConcurrentExecutorClear,
    DependenciesMet,
    DependenciesUnmet {
        detail: String,
    },
    SelfReportedSkillHintIgnored {
        skill: String,
    },
    ReviewerSeparationOk,
    ReviewerSeparationViolated {
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionOutcome {
    pub kind: ClaimDecisionKind,
    pub allowed: bool,
    pub reasons: Vec<ClaimEligibilityFactor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimEligibilityExplanation {
    pub project_id: String,
    pub work_item_id: String,
    pub candidate_person_id: PersonId,
    pub authorization_id: Option<String>,
    pub responsibility_accept: DecisionOutcome,
    pub collaborative_occupancy: DecisionOutcome,
    pub start_work_admission: DecisionOutcome,
    pub skill_hints_ignored: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimEvaluationInput<'a> {
    pub project_id: &'a str,
    pub work_item_id: &'a str,
    /// Verified owning workstream for `work_item_id`, when the task is stream-owned.
    pub task_workstream_id: Option<&'a str>,
    pub candidate_person: &'a PersonId,
    pub authorization: Option<&'a AgentAuthorization>,
    pub now_ms: i64,
    pub is_project_member: bool,
    pub membership_version: u64,
    pub assignment_policy: &'a str,
    pub assignment_policy_allows: bool,
    pub required_resources: &'a [String],
    pub available_resource_ids: &'a BTreeSet<String>,
    pub required_host_capabilities: &'a [String],
    pub verified_host_capabilities: &'a BTreeSet<String>,
    pub host_id: &'a str,
    pub dependencies_satisfied: bool,
    pub task: &'a TaskResponsibility,
    pub requested_executor: &'a ExecutionInstance,
}

pub fn explain_claim_eligibility(
    input: &ClaimEvaluationInput<'_>,
) -> Result<ClaimEligibilityExplanation> {
    validate_id(input.project_id, "project_id")?;
    validate_id(input.work_item_id, "work_item_id")?;

    let mut shared = Vec::new();
    let mut skill_hints_ignored = Vec::new();

    if let Some(auth) = input.authorization {
        for hint in &auth.self_reported_skill_hints {
            skill_hints_ignored.push(hint.clone());
            shared.push(ClaimEligibilityFactor::SelfReportedSkillHintIgnored {
                skill: hint.clone(),
            });
        }
    }

    if input.is_project_member {
        shared.push(ClaimEligibilityFactor::MembershipOk {
            project_id: input.project_id.into(),
            membership_version: input.membership_version,
        });
    } else {
        shared.push(ClaimEligibilityFactor::MembershipMissing {
            project_id: input.project_id.into(),
            detail: "candidate is not an active project member".into(),
        });
    }

    let mut auth_ok = false;
    match input.authorization {
        Some(auth) => {
            if !auth.is_effective_at(input.now_ms) {
                shared.push(ClaimEligibilityFactor::DelegationInactive {
                    authorization_id: auth.id.clone(),
                    detail: "authorization revoked or expired".into(),
                });
            } else if &auth.responsible_person_id != input.candidate_person {
                shared.push(ClaimEligibilityFactor::DelegationInactive {
                    authorization_id: auth.id.clone(),
                    detail: "authorization responsible person does not match candidate".into(),
                });
            } else if !auth.covers_task(
                input.project_id,
                input.work_item_id,
                input.task_workstream_id,
            ) {
                shared.push(ClaimEligibilityFactor::DelegationInactive {
                    authorization_id: auth.id.clone(),
                    detail: "authorization scope does not cover this task".into(),
                });
            } else {
                shared.push(ClaimEligibilityFactor::DelegationOk {
                    authorization_id: auth.id.clone(),
                });
                auth_ok = true;
            }
        }
        None => match input.requested_executor {
            ExecutionInstance::AgentRun { .. } => {
                shared.push(ClaimEligibilityFactor::DelegationMissing {
                    detail: "agent execution requires an explicit agent authorization".into(),
                });
            }
            ExecutionInstance::Person { person_id } if person_id == input.candidate_person => {
                shared.push(ClaimEligibilityFactor::DelegationOk {
                    authorization_id: "person-direct".into(),
                });
                auth_ok = true;
            }
            ExecutionInstance::Person { .. } => {
                shared.push(ClaimEligibilityFactor::DelegationMissing {
                    detail: "executor person does not match candidate".into(),
                });
            }
        },
    }

    if input.assignment_policy_allows {
        shared.push(ClaimEligibilityFactor::AssignmentPolicyOk {
            policy: input.assignment_policy.into(),
        });
    } else {
        shared.push(ClaimEligibilityFactor::AssignmentPolicyDenied {
            policy: input.assignment_policy.into(),
            detail: "assignment policy does not permit this candidate".into(),
        });
    }

    let mut resources_ok = true;
    for resource in input.required_resources {
        if input.available_resource_ids.contains(resource) {
            shared.push(ClaimEligibilityFactor::ResourceOk {
                resource_id: resource.clone(),
            });
        } else {
            resources_ok = false;
            shared.push(ClaimEligibilityFactor::ResourceUnavailable {
                resource_id: resource.clone(),
                detail: "required resource is not available to the candidate".into(),
            });
        }
    }

    let mut host_ok = true;
    for cap in input.required_host_capabilities {
        if input.verified_host_capabilities.contains(cap) {
            shared.push(ClaimEligibilityFactor::HostCapabilityVerified {
                host_id: input.host_id.into(),
                capability_id: cap.clone(),
            });
        } else {
            host_ok = false;
            shared.push(ClaimEligibilityFactor::HostCapabilityMissing {
                capability_id: cap.clone(),
                detail: "host capability is not verified; self-reported skills are not proof"
                    .into(),
            });
        }
    }

    let reviewer_ok = match &input.task.independent_reviewer {
        Some(reviewer) if reviewer == input.candidate_person => {
            if input.task.owner.as_ref() == Some(input.candidate_person) {
                shared.push(ClaimEligibilityFactor::ReviewerSeparationViolated {
                    detail: "independent reviewer must differ from the sole owner".into(),
                });
                false
            } else {
                shared.push(ClaimEligibilityFactor::ReviewerSeparationOk);
                true
            }
        }
        _ => {
            shared.push(ClaimEligibilityFactor::ReviewerSeparationOk);
            true
        }
    };

    let mut concurrent_clear = true;
    match &input.task.current_executor {
        Some(existing) if existing.person_id() != input.candidate_person => {
            concurrent_clear = false;
            shared.push(ClaimEligibilityFactor::ConcurrentExecutorPresent {
                person_id: existing.person_id().as_str().into(),
                detail: "another execution instance holds the task; only one effective executor"
                    .into(),
            });
        }
        _ => shared.push(ClaimEligibilityFactor::ConcurrentExecutorClear),
    }

    if input.dependencies_satisfied {
        shared.push(ClaimEligibilityFactor::DependenciesMet);
    } else {
        shared.push(ClaimEligibilityFactor::DependenciesUnmet {
            detail: "dependencies unmet: ownership may be assigned, but start-work is not admitted"
                .into(),
        });
    }

    let base_ok = input.is_project_member
        && auth_ok
        && input.assignment_policy_allows
        && resources_ok
        && host_ok
        && reviewer_ok;

    let auth_allows_accept = input
        .authorization
        .map(|a| a.permits(AuthorizedAction::AcceptResponsibility))
        .unwrap_or(true);
    let auth_allows_occupy = input
        .authorization
        .map(|a| {
            a.permits(AuthorizedAction::OccupyCollaboratively)
                || a.permits(AuthorizedAction::ClaimCoordination)
        })
        .unwrap_or(true);
    let auth_allows_start = input
        .authorization
        .map(|a| a.permits(AuthorizedAction::StartWork))
        .unwrap_or(true);

    let accept_allowed = base_ok && auth_allows_accept;
    let occupy_allowed = base_ok && auth_allows_occupy && concurrent_clear;
    let start_allowed =
        base_ok && auth_allows_start && concurrent_clear && input.dependencies_satisfied;

    Ok(ClaimEligibilityExplanation {
        project_id: input.project_id.into(),
        work_item_id: input.work_item_id.into(),
        candidate_person_id: input.candidate_person.clone(),
        authorization_id: input.authorization.map(|a| a.id.clone()),
        responsibility_accept: DecisionOutcome {
            kind: ClaimDecisionKind::ResponsibilityAccept,
            allowed: accept_allowed,
            reasons: shared.clone(),
        },
        collaborative_occupancy: DecisionOutcome {
            kind: ClaimDecisionKind::CollaborativeOccupancy,
            allowed: occupy_allowed,
            reasons: shared.clone(),
        },
        start_work_admission: DecisionOutcome {
            kind: ClaimDecisionKind::StartWorkAdmission,
            allowed: start_allowed,
            reasons: shared,
        },
        skill_hints_ignored,
    })
}

/// Concurrent claims keep a single effective executor.
pub fn resolve_single_effective_executor(
    task: &TaskResponsibility,
    candidate: &PersonId,
    occupancy_allowed: bool,
) -> Result<Option<PersonId>> {
    if let Some(existing) = &task.current_executor {
        return Ok(Some(existing.person_id().clone()));
    }
    if occupancy_allowed {
        Ok(Some(candidate.clone()))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str) -> PersonId {
        PersonId::new(id).unwrap()
    }

    fn base_auth() -> AgentAuthorization {
        AgentAuthorization {
            id: "auth-1".into(),
            authorizer_person_id: person("alice"),
            responsible_person_id: person("bob"),
            subject_kind: ExecutionSubjectKind::Agent,
            subject_id: "agent-bob".into(),
            client_id: "client-1".into(),
            session_id: Some("sess-1".into()),
            model_id: Some("model-a".into()),
            scope: AuthorizationScope::Project {
                project_id: "proj".into(),
            },
            actions: BTreeSet::from([
                AuthorizedAction::OccupyCollaboratively,
                AuthorizedAction::ClaimCoordination,
                AuthorizedAction::StartWork,
                AuthorizedAction::AcceptResponsibility,
                AuthorizedAction::ManageAuthorization,
            ]),
            expires_at_ms: Some(10_000),
            status: AuthorizationStatus::Active,
            revoked_at_ms: None,
            revoked_by: None,
            verifiable_capabilities: vec![VerifiableCapability::HostDeclared {
                host_id: "host-1".into(),
                capability_id: "shell.exec".into(),
                proof_digest: "abcdef0123456789".into(),
            }],
            self_reported_skill_hints: vec!["rust".into()],
            parent_authorization_id: None,
            maintainer_person_id: None,
            created_at_ms: 1_000,
            binding_id: Some("bind-1".into()),
        }
    }

    #[test]
    fn platform_service_requires_maintainer() {
        let mut auth = base_auth();
        auth.subject_kind = ExecutionSubjectKind::PlatformService;
        auth.subject_id = "svc-team".into();
        auth.binding_id = None;
        auth.maintainer_person_id = None;
        assert!(auth.validate().is_err());
        auth.maintainer_person_id = Some(person("ops"));
        assert!(auth.validate().is_ok());
    }

    #[test]
    fn delegation_cannot_escalate_actions_or_expiry() {
        let parent = base_auth();
        let mut child = base_auth();
        child.id = "auth-child".into();
        child.authorizer_person_id = person("bob");
        child.actions.insert(AuthorizedAction::Review);
        child.expires_at_ms = Some(20_000);
        assert!(
            apply_delegate(
                &parent,
                &DelegateAuthorizationRequest {
                    request_key: "dlg-1".into(),
                    parent_authorization_id: parent.id.clone(),
                    child: child.clone(),
                },
                2_000,
            )
            .is_err()
        );

        child.actions = BTreeSet::from([AuthorizedAction::StartWork]);
        child.expires_at_ms = Some(5_000);
        child.scope = AuthorizationScope::Task {
            project_id: "proj".into(),
            work_item_id: "work-1".into(),
        };
        let ok = apply_delegate(
            &parent,
            &DelegateAuthorizationRequest {
                request_key: "dlg-2".into(),
                parent_authorization_id: parent.id.clone(),
                child,
            },
            2_000,
        )
        .unwrap();
        assert_eq!(ok.parent_authorization_id.as_deref(), Some("auth-1"));
        assert!(ok.permits(AuthorizedAction::StartWork));
        assert!(!ok.permits(AuthorizedAction::Review));
    }

    #[test]
    fn client_or_session_change_cannot_bypass_revoke() {
        let mut auth = base_auth();
        auth.status = AuthorizationStatus::Revoked;
        auth.revoked_at_ms = Some(3_000);
        auth.revoked_by = Some(person("alice"));
        assert!(
            bind_runtime_identity(&auth, "client-1", Some("sess-1"), Some("model-b"), 4_000)
                .is_err()
        );

        let auth = base_auth();
        assert!(bind_runtime_identity(&auth, "client-other", Some("sess-1"), None, 2_000).is_err());
    }

    #[test]
    fn explain_separates_accept_occupy_and_start_when_deps_unmet() {
        let auth = base_auth();
        let task = TaskResponsibility::unassigned("proj", "work-1");
        let resources = BTreeSet::from(["cpu".into()]);
        let caps = BTreeSet::from(["shell.exec".into()]);
        let bob = person("bob");
        let executor = ExecutionInstance::AgentRun {
            person_id: bob.clone(),
            agent_id: "agent-bob".into(),
            binding_id: "bind-1".into(),
        };
        let explanation = explain_claim_eligibility(&ClaimEvaluationInput {
            project_id: "proj",
            work_item_id: "work-1",
            task_workstream_id: None,
            candidate_person: &bob,
            authorization: Some(&auth),
            now_ms: 2_000,
            is_project_member: true,
            membership_version: 3,
            assignment_policy: "pool-open",
            assignment_policy_allows: true,
            required_resources: &["cpu".into()],
            available_resource_ids: &resources,
            required_host_capabilities: &["shell.exec".into()],
            verified_host_capabilities: &caps,
            host_id: "host-1",
            dependencies_satisfied: false,
            task: &task,
            requested_executor: &executor,
        })
        .unwrap();
        assert!(explanation.responsibility_accept.allowed);
        assert!(explanation.collaborative_occupancy.allowed);
        assert!(!explanation.start_work_admission.allowed);
        assert_eq!(explanation.skill_hints_ignored, vec!["rust".to_string()]);
        assert!(
            explanation
                .start_work_admission
                .reasons
                .iter()
                .any(|r| { matches!(r, ClaimEligibilityFactor::DependenciesUnmet { .. }) })
        );
        assert!(explanation.start_work_admission.reasons.iter().any(|r| {
            matches!(
                r,
                ClaimEligibilityFactor::SelfReportedSkillHintIgnored { .. }
            )
        }));
    }

    #[test]
    fn concurrent_claims_keep_single_effective_executor() {
        let alice = person("alice");
        let bob = person("bob");
        let mut task = TaskResponsibility::unassigned("proj", "work-1");
        task.current_executor = Some(ExecutionInstance::Person {
            person_id: alice.clone(),
        });
        let retained = resolve_single_effective_executor(&task, &bob, true).unwrap();
        assert_eq!(retained, Some(alice));

        let resources = BTreeSet::new();
        let caps = BTreeSet::new();
        let auth = base_auth();
        let executor = ExecutionInstance::Person {
            person_id: bob.clone(),
        };
        let explanation = explain_claim_eligibility(&ClaimEvaluationInput {
            project_id: "proj",
            work_item_id: "work-1",
            task_workstream_id: None,
            candidate_person: &bob,
            authorization: Some(&auth),
            now_ms: 2_000,
            is_project_member: true,
            membership_version: 1,
            assignment_policy: "open",
            assignment_policy_allows: true,
            required_resources: &[],
            available_resource_ids: &resources,
            required_host_capabilities: &[],
            verified_host_capabilities: &caps,
            host_id: "host-1",
            dependencies_satisfied: true,
            task: &task,
            requested_executor: &executor,
        })
        .unwrap();
        assert!(!explanation.collaborative_occupancy.allowed);
        assert!(!explanation.start_work_admission.allowed);
    }

    #[test]
    fn revoke_is_inspectable_and_blocks_eligibility() {
        let auth = base_auth();
        let revoked = apply_revoke(
            &auth,
            &RevokeAuthorizationRequest {
                request_key: "rev-1".into(),
                authorization_id: auth.id.clone(),
                revoked_by: person("alice"),
                revoked_at_ms: 3_000,
                reason: "session ended".into(),
            },
        )
        .unwrap();
        assert!(matches!(revoked.status, AuthorizationStatus::Revoked));
        assert!(!revoked.is_effective_at(4_000));
    }

    #[test]
    fn workstream_scoped_grant_covers_owned_task_only() {
        let mut auth = base_auth();
        auth.scope = AuthorizationScope::Workstream {
            project_id: "proj".into(),
            workstream_id: "stream-a".into(),
        };
        assert!(auth.covers_task("proj", "work-1", Some("stream-a")));
        assert!(!auth.covers_task("proj", "work-1", Some("stream-b")));
        assert!(!auth.covers_task("proj", "work-1", None));
        assert!(!auth.covers_task("other", "work-1", Some("stream-a")));

        // Project / task positive controls still pass.
        auth.scope = AuthorizationScope::Project {
            project_id: "proj".into(),
        };
        assert!(auth.covers_task("proj", "work-1", Some("stream-a")));
        assert!(auth.covers_task("proj", "work-1", None));
        auth.scope = AuthorizationScope::Task {
            project_id: "proj".into(),
            work_item_id: "work-1".into(),
        };
        assert!(auth.covers_task("proj", "work-1", Some("stream-a")));
        assert!(!auth.covers_task("proj", "work-2", Some("stream-a")));

        // Eligibility reports DelegationInactive for foreign-stream ownership.
        auth.scope = AuthorizationScope::Workstream {
            project_id: "proj".into(),
            workstream_id: "stream-a".into(),
        };
        let task = TaskResponsibility::unassigned("proj", "work-1");
        let resources = BTreeSet::new();
        let caps = BTreeSet::new();
        let bob = person("bob");
        let executor = ExecutionInstance::AgentRun {
            person_id: bob.clone(),
            agent_id: "agent-bob".into(),
            binding_id: "bind-1".into(),
        };
        let explanation = explain_claim_eligibility(&ClaimEvaluationInput {
            project_id: "proj",
            work_item_id: "work-1",
            task_workstream_id: Some("stream-b"),
            candidate_person: &bob,
            authorization: Some(&auth),
            now_ms: 2_000,
            is_project_member: true,
            membership_version: 1,
            assignment_policy: "open",
            assignment_policy_allows: true,
            required_resources: &[],
            available_resource_ids: &resources,
            required_host_capabilities: &[],
            verified_host_capabilities: &caps,
            host_id: "host-1",
            dependencies_satisfied: true,
            task: &task,
            requested_executor: &executor,
        })
        .unwrap();
        assert!(explanation.responsibility_accept.reasons.iter().any(|r| {
            matches!(
                r,
                ClaimEligibilityFactor::DelegationInactive { detail, .. }
                    if detail.contains("scope does not cover")
            )
        }));
        let ok = explain_claim_eligibility(&ClaimEvaluationInput {
            project_id: "proj",
            work_item_id: "work-1",
            task_workstream_id: Some("stream-a"),
            candidate_person: &bob,
            authorization: Some(&auth),
            now_ms: 2_000,
            is_project_member: true,
            membership_version: 1,
            assignment_policy: "open",
            assignment_policy_allows: true,
            required_resources: &[],
            available_resource_ids: &resources,
            required_host_capabilities: &[],
            verified_host_capabilities: &caps,
            host_id: "host-1",
            dependencies_satisfied: true,
            task: &task,
            requested_executor: &executor,
        })
        .unwrap();
        assert!(ok.responsibility_accept.allowed);
    }
}
