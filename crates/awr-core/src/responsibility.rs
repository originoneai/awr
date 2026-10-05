//! Task responsibility, person identity, and execution-instance assignment.
//!
//! Units (acceptance WS-015):
//! - **Task** — stable work unit (`work_item` / `work_id`)
//! - **Person** — responsibility unit (never a group label or agent name)
//! - **Execution instance** — a person acting, or an agent run explicitly bound to a person
//!
//! Roles on one task: sole owner, collaborators, current executor, independent reviewer.
//! Assign / accept responsibility / temporary execution claim are separate ops.
//! Claiming execution never steals ownership. Actor.kind must not infer person↔agent.
use crate::{Error, Id, Result};
use serde::{Deserialize, Serialize};

/// Opaque person identity. Not an actor id, group name, or agent label.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PersonId(String);

impl PersonId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_id(&value, "person_id")?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PersonId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Declared member provenance. It never changes responsibility or grants trust.
/// Absence of metadata on a legacy person means unspecified, not human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberIdentityKind {
    Human,
    SimulatedMember,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberIdentityMetadata {
    pub kind: MemberIdentityKind,
    /// Optional opaque experiment attribution, never a credential or permission.
    /// Multiple simulated members may have the same controller reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_ref: Option<String>,
}

impl MemberIdentityMetadata {
    pub fn validate(&self) -> Result<()> {
        if let Some(reference) = &self.controller_ref {
            validate_id(reference, "controller_ref")?;
        }
        Ok(())
    }

    /// A simulation anchor must remain non-human and cannot use system authority.
    pub fn matches_member_actor_kind(&self, actor_kind: &str) -> bool {
        matches!(
            (self.kind, actor_kind),
            (MemberIdentityKind::Human, "human") | (MemberIdentityKind::SimulatedMember, "agent")
        )
    }
}

/// Explicit person↔agent binding. Never derived from `actor.kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonAgentBinding {
    pub id: String,
    pub person_id: PersonId,
    pub agent_id: String,
    pub status: BindingStatus,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingStatus {
    Active,
    Disabled,
}

/// Who is currently executing on a task. Agent runs require an explicit binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionInstance {
    Person {
        person_id: PersonId,
    },
    AgentRun {
        person_id: PersonId,
        agent_id: String,
        binding_id: String,
    },
}

impl ExecutionInstance {
    pub fn person_id(&self) -> &PersonId {
        match self {
            Self::Person { person_id } | Self::AgentRun { person_id, .. } => person_id,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Person { person_id } => {
                validate_id(person_id.as_str(), "person_id")?;
            }
            Self::AgentRun {
                person_id,
                agent_id,
                binding_id,
            } => {
                validate_id(person_id.as_str(), "person_id")?;
                validate_id(agent_id, "agent_id")?;
                validate_id(binding_id, "binding_id")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponsibilityPendingKind {
    Departure,
    Disabled,
    NoAcceptor,
    LegacyIdentityMigration,
}

/// Explicit pending states; never silently resolved by renaming or actor.kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponsibilityPending {
    pub kind: ResponsibilityPendingKind,
    pub person_id: Option<PersonId>,
    pub legacy_ref: Option<String>,
    pub transfer_request_key: Option<String>,
    pub detail: String,
}

/// Current responsibility projection for one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskResponsibility {
    pub project_id: String,
    pub work_item_id: String,
    /// Sole owner. `None` = unassigned pool (still a valid task).
    pub owner: Option<PersonId>,
    pub collaborators: Vec<PersonId>,
    pub current_executor: Option<ExecutionInstance>,
    /// Must differ from owner/author when set for independent review.
    pub independent_reviewer: Option<PersonId>,
    pub version: u64,
    pub pending: Option<ResponsibilityPending>,
    pub personal_mode_default: bool,
}

impl TaskResponsibility {
    pub fn unassigned(project_id: impl Into<String>, work_item_id: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            work_item_id: work_item_id.into(),
            owner: None,
            collaborators: vec![],
            current_executor: None,
            independent_reviewer: None,
            version: 0,
            pending: None,
            personal_mode_default: false,
        }
    }

    /// Personal mode may default owner=self while retaining the same semantics.
    pub fn personal_default(
        project_id: impl Into<String>,
        work_item_id: impl Into<String>,
        self_person: PersonId,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            work_item_id: work_item_id.into(),
            owner: Some(self_person),
            collaborators: vec![],
            current_executor: None,
            independent_reviewer: None,
            version: 0,
            pending: None,
            personal_mode_default: true,
        }
    }

    pub fn validate_structure(&self) -> Result<()> {
        validate_id(&self.project_id, "project_id")?;
        validate_id(&self.work_item_id, "work_item_id")?;
        if let Some(owner) = &self.owner {
            validate_id(owner.as_str(), "owner")?;
        }
        if self.collaborators.len() > 64 {
            return Err(Error::InvalidInput(
                "collaborators exceed the 64-person bound".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.collaborators {
            validate_id(c.as_str(), "collaborator")?;
            if !seen.insert(c.as_str()) {
                return Err(Error::InvalidInput(
                    "collaborators must be unique persons".into(),
                ));
            }
            if self.owner.as_ref() == Some(c) {
                return Err(Error::InvalidInput(
                    "owner cannot also be listed as collaborator".into(),
                ));
            }
        }
        if let Some(exec) = &self.current_executor {
            exec.validate()?;
        }
        if let Some(reviewer) = &self.independent_reviewer {
            validate_id(reviewer.as_str(), "independent_reviewer")?;
            if self.owner.as_ref() == Some(reviewer) {
                return Err(Error::InvalidInput(
                    "independent reviewer must differ from the sole owner".into(),
                ));
            }
        }
        if let Some(pending) = &self.pending {
            if pending.detail.trim().is_empty() || pending.detail.len() > 2048 {
                return Err(Error::InvalidInput(
                    "pending detail requires 1..2048 bytes".into(),
                ));
            }
            if let Some(legacy) = &pending.legacy_ref {
                validate_id(legacy, "legacy_ref")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponsibilityEventType {
    Assigned,
    Accepted,
    AvailableClaimed,
    ExecutionClaimed,
    ExecutionReleased,
    OwnerTransferProposed,
    OwnerTransferAccepted,
    OwnerTransferRejected,
    CollaboratorsUpdated,
    ReviewerUpdated,
    AgentBound,
    AgentUnbound,
    PendingMarked,
    PendingCleared,
}

/// Immutable history row for one responsibility change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponsibilityEvent {
    pub id: Id,
    pub project_id: String,
    pub work_item_id: String,
    pub event_type: ResponsibilityEventType,
    pub version_before: u64,
    pub version_after: u64,
    pub actor_person_id: Option<PersonId>,
    pub payload: serde_json::Value,
    pub created_at_ms: i64,
}

/// Idempotent receipt for a responsibility operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponsibilityReceipt {
    pub request_key: String,
    pub event_id: Id,
    pub project_id: String,
    pub work_item_id: String,
    pub op: ResponsibilityEventType,
    pub version_before: u64,
    pub version_after: u64,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignResponsibilityRequest {
    pub request_key: String,
    pub expected_version: u64,
    pub owner: Option<PersonId>,
    pub collaborators: Vec<PersonId>,
    pub independent_reviewer: Option<PersonId>,
    /// When true and owner is None, keep unassigned pool semantics.
    pub allow_unassigned: bool,
    pub authorized_by: PersonId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptResponsibilityRequest {
    pub request_key: String,
    pub expected_version: u64,
    pub acceptor: PersonId,
    /// Must match the pending assignment / transfer target.
    pub as_owner: bool,
}

/// Claim sole responsibility from the available pool, without starting execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimAvailableRequest {
    pub request_key: String,
    pub expected_version: u64,
    pub claimant: PersonId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimExecutionRequest {
    pub request_key: String,
    pub expected_version: u64,
    pub executor: ExecutionInstance,
    /// Optional link to an existing runtime/team claim id — does not imply ownership.
    pub coordination_claim_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferOwnerRequest {
    pub request_key: String,
    pub expected_version: u64,
    pub from_owner: PersonId,
    pub to_owner: PersonId,
    pub authorized_by: PersonId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindAgentRequest {
    pub request_key: String,
    pub person_id: PersonId,
    pub agent_id: String,
    pub binding_id: String,
}

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

/// Pure transition: assign responsibility (does not auto-accept).
pub fn apply_assign(
    current: &TaskResponsibility,
    req: &AssignResponsibilityRequest,
) -> Result<TaskResponsibility> {
    validate_request_key(&req.request_key)?;
    current.validate_structure()?;
    if current.version != req.expected_version {
        return Err(Error::RevisionConflict {
            expected: req.expected_version,
            actual: current.version,
        });
    }
    if req.owner.is_none() && !req.allow_unassigned {
        return Err(Error::InvalidInput(
            "assign without owner requires allow_unassigned for the unassigned pool".into(),
        ));
    }
    // Group/agent names are rejected by PersonId construction at the boundary;
    // authorized_by must be a person, not an agent label substitute.
    validate_id(req.authorized_by.as_str(), "authorized_by")?;
    let mut next = current.clone();
    next.owner = req.owner.clone();
    next.collaborators = req.collaborators.clone();
    next.independent_reviewer = req.independent_reviewer.clone();
    // Assignment alone does not steal or invent an executor.
    next.version = current
        .version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("responsibility version overflow".into()))?;
    // Fresh assign clears transfer-pending; departure/disable stay until explicit clear.
    if matches!(
        next.pending.as_ref().map(|p| p.kind),
        Some(ResponsibilityPendingKind::NoAcceptor)
    ) {
        next.pending = None;
    }
    next.validate_structure()?;
    Ok(next)
}

/// Accept pending ownership. Acceptor must be the designated owner.
pub fn apply_accept(
    current: &TaskResponsibility,
    req: &AcceptResponsibilityRequest,
) -> Result<TaskResponsibility> {
    validate_request_key(&req.request_key)?;
    current.validate_structure()?;
    if current.version != req.expected_version {
        return Err(Error::RevisionConflict {
            expected: req.expected_version,
            actual: current.version,
        });
    }
    if !req.as_owner {
        return Err(Error::InvalidInput(
            "accept currently supports as_owner only".into(),
        ));
    }
    match &current.owner {
        Some(owner) if owner == &req.acceptor => {}
        Some(_) => {
            return Err(Error::RuleViolation(
                "acceptor is not the assigned owner; ownership cannot be auto-stolen".into(),
            ));
        }
        None => {
            return Err(Error::RuleViolation(
                "unassigned pool has no owner to accept".into(),
            ));
        }
    }
    if let Some(pending) = &current.pending {
        if matches!(
            pending.kind,
            ResponsibilityPendingKind::Departure | ResponsibilityPendingKind::Disabled
        ) {
            return Err(Error::RuleViolation(
                "departure/disable pending must be cleared explicitly before accept".into(),
            ));
        }
    }
    let mut next = current.clone();
    next.version = current
        .version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("responsibility version overflow".into()))?;
    if matches!(
        next.pending.as_ref().map(|p| p.kind),
        Some(ResponsibilityPendingKind::NoAcceptor)
            | Some(ResponsibilityPendingKind::LegacyIdentityMigration)
    ) {
        next.pending = None;
    }
    next.validate_structure()?;
    Ok(next)
}

/// Take ownership only when no owner, reservation or executor exists.
/// Authentication, dependency admission and coordination leases are separate.
pub fn apply_claim_available(
    current: &TaskResponsibility,
    req: &ClaimAvailableRequest,
) -> Result<TaskResponsibility> {
    validate_request_key(&req.request_key)?;
    validate_id(req.claimant.as_str(), "claimant")?;
    current.validate_structure()?;
    if current.version != req.expected_version {
        return Err(Error::RevisionConflict {
            expected: req.expected_version,
            actual: current.version,
        });
    }
    if current.owner.is_some() || current.pending.is_some() || current.current_executor.is_some() {
        return Err(Error::ClaimConflict(
            "available ownership requires no owner, pending responsibility or executor".into(),
        ));
    }
    if current.independent_reviewer.as_ref() == Some(&req.claimant) {
        return Err(Error::RuleViolation(
            "the independent reviewer cannot claim ownership of this task".into(),
        ));
    }
    let mut next = current.clone();
    next.owner = Some(req.claimant.clone());
    next.collaborators.retain(|person| person != &req.claimant);
    next.version = current
        .version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("responsibility version overflow".into()))?;
    next.validate_structure()?;
    Ok(next)
}

/// Temporary execution claim. **Never** changes sole ownership.
pub fn apply_claim_execution(
    current: &TaskResponsibility,
    req: &ClaimExecutionRequest,
    bindings: &[PersonAgentBinding],
) -> Result<TaskResponsibility> {
    validate_request_key(&req.request_key)?;
    current.validate_structure()?;
    req.executor.validate()?;
    if current.version != req.expected_version {
        return Err(Error::RevisionConflict {
            expected: req.expected_version,
            actual: current.version,
        });
    }
    if let Some(claim) = &req.coordination_claim_id {
        validate_id(claim, "coordination_claim_id")?;
    }
    ensure_executor_binding(&req.executor, bindings)?;
    // Claiming execution while another active executor exists is a conflict,
    // unless it is the same person swapping agents (ownership unchanged).
    if let Some(existing) = &current.current_executor {
        let same_person = existing.person_id() == req.executor.person_id();
        if !same_person {
            return Err(Error::ClaimConflict(
                "another execution instance holds the task; release or wait".into(),
            ));
        }
    }
    let mut next = current.clone();
    // Ownership is untouched — even if the unassigned pool has no owner.
    next.current_executor = Some(req.executor.clone());
    next.version = current
        .version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("responsibility version overflow".into()))?;
    next.validate_structure()?;
    Ok(next)
}

pub fn apply_release_execution(
    current: &TaskResponsibility,
    request_key: &str,
    expected_version: u64,
    by_person: &PersonId,
) -> Result<TaskResponsibility> {
    validate_request_key(request_key)?;
    if current.version != expected_version {
        return Err(Error::RevisionConflict {
            expected: expected_version,
            actual: current.version,
        });
    }
    match &current.current_executor {
        Some(exec) if exec.person_id() == by_person => {}
        Some(_) => {
            return Err(Error::ClaimConflict(
                "only the current executor person may release the execution claim".into(),
            ));
        }
        None => {
            return Err(Error::NotFound(
                "no current execution instance to release".into(),
            ));
        }
    }
    let mut next = current.clone();
    next.current_executor = None;
    next.version = current
        .version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("responsibility version overflow".into()))?;
    Ok(next)
}

/// Propose ownership transfer. Requires authorization; remains pending until acceptor accepts.
pub fn apply_transfer_propose(
    current: &TaskResponsibility,
    req: &TransferOwnerRequest,
) -> Result<TaskResponsibility> {
    validate_request_key(&req.request_key)?;
    current.validate_structure()?;
    if current.version != req.expected_version {
        return Err(Error::RevisionConflict {
            expected: req.expected_version,
            actual: current.version,
        });
    }
    if req.from_owner == req.to_owner {
        return Err(Error::InvalidInput(
            "owner transfer requires a different acceptor person".into(),
        ));
    }
    match &current.owner {
        Some(owner) if owner == &req.from_owner => {}
        Some(_) => {
            return Err(Error::RuleViolation(
                "from_owner does not match current sole owner".into(),
            ));
        }
        None => {
            return Err(Error::RuleViolation(
                "unassigned pool has no owner to transfer".into(),
            ));
        }
    }
    // authorized_by must be the current owner (or an explicit grant surface later).
    if req.authorized_by != req.from_owner {
        return Err(Error::RuleViolation(
            "owner transfer requires authorization by the current owner".into(),
        ));
    }
    let mut next = current.clone();
    next.owner = Some(req.to_owner.clone());
    next.pending = Some(ResponsibilityPending {
        kind: ResponsibilityPendingKind::NoAcceptor,
        person_id: Some(req.to_owner.clone()),
        legacy_ref: None,
        transfer_request_key: Some(req.request_key.clone()),
        detail: "owner transfer awaiting acceptor".into(),
    });
    // Agent swap is unrelated — keep current_executor if same person; clear if leaving.
    if let Some(exec) = &next.current_executor {
        if exec.person_id() == &req.from_owner {
            next.current_executor = None;
        }
    }
    next.version = current
        .version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("responsibility version overflow".into()))?;
    next.validate_structure()?;
    Ok(next)
}

/// Same person changing agents does not change responsibility ownership.
pub fn apply_agent_swap_for_person(
    current: &TaskResponsibility,
    request_key: &str,
    expected_version: u64,
    person_id: &PersonId,
    new_executor: ExecutionInstance,
    bindings: &[PersonAgentBinding],
) -> Result<TaskResponsibility> {
    validate_request_key(request_key)?;
    if current.version != expected_version {
        return Err(Error::RevisionConflict {
            expected: expected_version,
            actual: current.version,
        });
    }
    if new_executor.person_id() != person_id {
        return Err(Error::InvalidInput(
            "agent swap must keep the same person_id".into(),
        ));
    }
    ensure_executor_binding(&new_executor, bindings)?;
    // Ownership must remain identical.
    let owner_before = current.owner.clone();
    let mut next = current.clone();
    next.current_executor = Some(new_executor);
    next.version = current
        .version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("responsibility version overflow".into()))?;
    if next.owner != owner_before {
        return Err(Error::RuleViolation(
            "agent swap must not alter responsibility ownership".into(),
        ));
    }
    next.validate_structure()?;
    Ok(next)
}

pub fn apply_mark_pending(
    current: &TaskResponsibility,
    request_key: &str,
    expected_version: u64,
    pending: ResponsibilityPending,
) -> Result<TaskResponsibility> {
    validate_request_key(request_key)?;
    if current.version != expected_version {
        return Err(Error::RevisionConflict {
            expected: expected_version,
            actual: current.version,
        });
    }
    if pending.detail.trim().is_empty() {
        return Err(Error::InvalidInput("pending detail required".into()));
    }
    let mut next = current.clone();
    next.pending = Some(pending);
    next.version = current
        .version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("responsibility version overflow".into()))?;
    next.validate_structure()?;
    Ok(next)
}

fn ensure_executor_binding(
    executor: &ExecutionInstance,
    bindings: &[PersonAgentBinding],
) -> Result<()> {
    match executor {
        ExecutionInstance::Person { .. } => Ok(()),
        ExecutionInstance::AgentRun {
            person_id,
            agent_id,
            binding_id,
        } => {
            let found = bindings.iter().any(|b| {
                b.id == *binding_id
                    && b.person_id == *person_id
                    && b.agent_id == *agent_id
                    && b.status == BindingStatus::Active
            });
            if !found {
                return Err(Error::RuleViolation(
                    "agent execution requires an explicit active person↔agent binding; actor.kind is not a substitute".into(),
                ));
            }
            Ok(())
        }
    }
}

/// Reject treating a raw actor.kind as a person/agent relationship.
pub fn refuse_infer_person_from_actor_kind(actor_kind: &str) -> Result<()> {
    let _ = actor_kind;
    Err(Error::RuleViolation(
        "person↔agent relations must be explicit bindings; actor.kind must not be inferred".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_provenance_is_explicit_and_cannot_confer_human_or_system_authority() {
        let simulated: MemberIdentityMetadata = serde_json::from_str(
            r#"{"kind":"simulated_member","controller_ref":"shared-controller"}"#,
        )
        .unwrap();
        simulated.validate().unwrap();
        assert!(simulated.matches_member_actor_kind("agent"));
        assert!(!simulated.matches_member_actor_kind("human"));
        assert!(!simulated.matches_member_actor_kind("system"));
        let human = MemberIdentityMetadata {
            kind: MemberIdentityKind::Human,
            controller_ref: None,
        };
        assert!(human.matches_member_actor_kind("human"));
        assert!(!human.matches_member_actor_kind("agent"));
        for invalid in [
            r#"{"controller_ref":"shared-controller"}"#,
            r#"{"kind":"model"}"#,
            r#"{"kind":"simulated_member","credential":"test-only"}"#,
        ] {
            assert!(serde_json::from_str::<MemberIdentityMetadata>(invalid).is_err());
        }
        for reference in [String::new(), "x".repeat(129), "control\ncharacter".into()] {
            assert!(
                MemberIdentityMetadata {
                    kind: MemberIdentityKind::SimulatedMember,
                    controller_ref: Some(reference),
                }
                .validate()
                .is_err()
            );
        }
    }

    fn person(s: &str) -> PersonId {
        PersonId::new(s).unwrap()
    }

    fn task() -> TaskResponsibility {
        TaskResponsibility::unassigned("proj-a", "work-a")
    }

    fn binding(person: &str, agent: &str, id: &str) -> PersonAgentBinding {
        PersonAgentBinding {
            id: id.into(),
            person_id: PersonId::new(person).unwrap(),
            agent_id: agent.into(),
            status: BindingStatus::Active,
            created_at_ms: 1,
        }
    }

    #[test]
    fn unassigned_pool_allows_execution_claim_without_stealing_owner() {
        let current = task();
        let next = apply_claim_execution(
            &current,
            &ClaimExecutionRequest {
                request_key: "claim-1".into(),
                expected_version: 0,
                executor: ExecutionInstance::Person {
                    person_id: person("alice"),
                },
                coordination_claim_id: Some("coord-claim-1".into()),
            },
            &[],
        )
        .unwrap();
        assert!(next.owner.is_none());
        assert_eq!(
            next.current_executor.as_ref().unwrap().person_id(),
            &person("alice")
        );
        assert_eq!(next.version, 1);
    }

    #[test]
    fn assign_accept_and_roles_are_distinct() {
        let current = task();
        let assigned = apply_assign(
            &current,
            &AssignResponsibilityRequest {
                request_key: "asg-1".into(),
                expected_version: 0,
                owner: Some(person("alice")),
                collaborators: vec![person("bob")],
                independent_reviewer: Some(person("carol")),
                allow_unassigned: false,
                authorized_by: person("admin"),
            },
        )
        .unwrap();
        assert_eq!(assigned.owner, Some(person("alice")));
        assert!(assigned.current_executor.is_none());
        let accepted = apply_accept(
            &assigned,
            &AcceptResponsibilityRequest {
                request_key: "acc-1".into(),
                expected_version: 1,
                acceptor: person("alice"),
                as_owner: true,
            },
        )
        .unwrap();
        assert_eq!(accepted.version, 2);
        assert!(
            apply_accept(
                &assigned,
                &AcceptResponsibilityRequest {
                    request_key: "acc-2".into(),
                    expected_version: 1,
                    acceptor: person("bob"),
                    as_owner: true,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn claim_does_not_auto_steal_ownership() {
        let mut current = task();
        current.owner = Some(person("alice"));
        current.version = 1;
        let next = apply_claim_execution(
            &current,
            &ClaimExecutionRequest {
                request_key: "c1".into(),
                expected_version: 1,
                executor: ExecutionInstance::Person {
                    person_id: person("bob"),
                },
                coordination_claim_id: None,
            },
            &[],
        )
        .unwrap();
        assert_eq!(next.owner, Some(person("alice")));
        assert_eq!(next.current_executor.unwrap().person_id(), &person("bob"));
    }

    #[test]
    fn agent_swap_keeps_ownership() {
        let bindings = vec![
            binding("alice", "agent-old", "bind-old"),
            binding("alice", "agent-new", "bind-new"),
        ];
        let mut current = TaskResponsibility::personal_default("p", "w", person("alice"));
        current.version = 1;
        current.current_executor = Some(ExecutionInstance::AgentRun {
            person_id: person("alice"),
            agent_id: "agent-old".into(),
            binding_id: "bind-old".into(),
        });
        let next = apply_agent_swap_for_person(
            &current,
            "swap-1",
            1,
            &person("alice"),
            ExecutionInstance::AgentRun {
                person_id: person("alice"),
                agent_id: "agent-new".into(),
                binding_id: "bind-new".into(),
            },
            &bindings,
        )
        .unwrap();
        assert_eq!(next.owner, Some(person("alice")));
        match next.current_executor.unwrap() {
            ExecutionInstance::AgentRun { agent_id, .. } => assert_eq!(agent_id, "agent-new"),
            _ => panic!("expected agent run"),
        }
    }

    #[test]
    fn transfer_requires_auth_and_acceptor_pending() {
        let mut current = task();
        current.owner = Some(person("alice"));
        current.version = 2;
        let proposed = apply_transfer_propose(
            &current,
            &TransferOwnerRequest {
                request_key: "xfer-1".into(),
                expected_version: 2,
                from_owner: person("alice"),
                to_owner: person("bob"),
                authorized_by: person("alice"),
            },
        )
        .unwrap();
        assert_eq!(proposed.owner, Some(person("bob")));
        assert_eq!(
            proposed.pending.as_ref().unwrap().kind,
            ResponsibilityPendingKind::NoAcceptor
        );
        assert!(
            apply_transfer_propose(
                &current,
                &TransferOwnerRequest {
                    request_key: "xfer-bad".into(),
                    expected_version: 2,
                    from_owner: person("alice"),
                    to_owner: person("bob"),
                    authorized_by: person("eve"),
                },
            )
            .is_err()
        );
        let accepted = apply_accept(
            &proposed,
            &AcceptResponsibilityRequest {
                request_key: "xfer-acc".into(),
                expected_version: 3,
                acceptor: person("bob"),
                as_owner: true,
            },
        )
        .unwrap();
        assert!(accepted.pending.is_none());
    }

    #[test]
    fn actor_kind_must_not_infer_binding() {
        assert!(refuse_infer_person_from_actor_kind("human").is_err());
        assert!(refuse_infer_person_from_actor_kind("agent").is_err());
        let current = task();
        let err = apply_claim_execution(
            &current,
            &ClaimExecutionRequest {
                request_key: "x".into(),
                expected_version: 0,
                executor: ExecutionInstance::AgentRun {
                    person_id: person("alice"),
                    agent_id: "agent-1".into(),
                    binding_id: "missing".into(),
                },
                coordination_claim_id: None,
            },
            &[],
        );
        assert!(err.is_err());
    }

    #[test]
    fn reviewer_must_be_independent_of_owner() {
        let current = task();
        assert!(
            apply_assign(
                &current,
                &AssignResponsibilityRequest {
                    request_key: "r1".into(),
                    expected_version: 0,
                    owner: Some(person("alice")),
                    collaborators: vec![],
                    independent_reviewer: Some(person("alice")),
                    allow_unassigned: false,
                    authorized_by: person("admin"),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn departure_pending_stays_explicit() {
        let mut current = task();
        current.owner = Some(person("alice"));
        current.version = 1;
        let next = apply_mark_pending(
            &current,
            "dep-1",
            1,
            ResponsibilityPending {
                kind: ResponsibilityPendingKind::Departure,
                person_id: Some(person("alice")),
                legacy_ref: None,
                transfer_request_key: None,
                detail: "alice departed".into(),
            },
        )
        .unwrap();
        assert_eq!(
            next.pending.as_ref().unwrap().kind,
            ResponsibilityPendingKind::Departure
        );
        assert!(
            apply_accept(
                &next,
                &AcceptResponsibilityRequest {
                    request_key: "a".into(),
                    expected_version: 2,
                    acceptor: person("alice"),
                    as_owner: true,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn available_claim_preserves_roles_without_starting_execution() {
        let mut current = task();
        current.collaborators = vec![person("alice"), person("bob")];
        current.independent_reviewer = Some(person("carol"));
        let before = current.clone();
        let next = apply_claim_available(
            &current,
            &ClaimAvailableRequest {
                request_key: "pool-claim".into(),
                expected_version: 0,
                claimant: person("alice"),
            },
        )
        .unwrap();
        assert_eq!(current, before);
        assert_eq!(next.owner, Some(person("alice")));
        assert_eq!(next.collaborators, vec![person("bob")]);
        assert_eq!(next.independent_reviewer, Some(person("carol")));
        assert_eq!(next.version, 1);
        assert!(next.current_executor.is_none());
        assert!(next.pending.is_none());
    }

    #[test]
    fn available_claim_refuses_owned_reserved_or_executing_tasks() {
        let request = ClaimAvailableRequest {
            request_key: "pool-claim".into(),
            expected_version: 0,
            claimant: person("alice"),
        };
        let mut owned = task();
        owned.owner = Some(person("bob"));
        let mut executing = task();
        executing.current_executor = Some(ExecutionInstance::Person {
            person_id: person("bob"),
        });
        let mut states = vec![owned, executing];
        for kind in [
            ResponsibilityPendingKind::NoAcceptor,
            ResponsibilityPendingKind::Departure,
            ResponsibilityPendingKind::Disabled,
            ResponsibilityPendingKind::LegacyIdentityMigration,
        ] {
            let mut reserved = task();
            reserved.pending = Some(ResponsibilityPending {
                kind,
                person_id: Some(person("bob")),
                legacy_ref: None,
                transfer_request_key: Some("reservation".into()),
                detail: "awaiting explicit resolution".into(),
            });
            states.push(reserved);
        }
        for state in states {
            let before = state.clone();
            assert!(matches!(
                apply_claim_available(&state, &request),
                Err(Error::ClaimConflict(_))
            ));
            assert_eq!(state, before);
        }
    }

    #[test]
    fn available_claim_enforces_version_and_reviewer_independence() {
        let mut current = task();
        current.independent_reviewer = Some(person("alice"));
        let mut request = ClaimAvailableRequest {
            request_key: "pool-claim".into(),
            expected_version: 0,
            claimant: person("alice"),
        };
        assert!(matches!(
            apply_claim_available(&current, &request),
            Err(Error::RuleViolation(_))
        ));
        request.claimant = person("bob");
        request.expected_version = 1;
        assert!(matches!(
            apply_claim_available(&current, &request),
            Err(Error::RevisionConflict {
                expected: 1,
                actual: 0
            })
        ));
        current.version = u64::MAX;
        request.expected_version = u64::MAX;
        assert!(matches!(
            apply_claim_available(&current, &request),
            Err(Error::InvalidInput(_))
        ));
    }

    #[test]
    fn available_claim_validates_deserialized_identities_and_request_keys() {
        let current = task();
        for (key, claimant) in [("", "alice"), ("pool", ""), ("pool", "line\nbreak")] {
            let request: ClaimAvailableRequest = serde_json::from_value(serde_json::json!({
                "request_key": key, "expected_version": 0, "claimant": claimant
            }))
            .unwrap();
            assert!(matches!(
                apply_claim_available(&current, &request),
                Err(Error::InvalidInput(_))
            ));
        }
        let mut invalid_task = current.clone();
        invalid_task.collaborators = vec![person("bob"), person("bob")];
        assert!(
            apply_claim_available(
                &invalid_task,
                &ClaimAvailableRequest {
                    request_key: "pool".into(),
                    expected_version: 0,
                    claimant: person("alice"),
                }
            )
            .is_err()
        );
    }
}
