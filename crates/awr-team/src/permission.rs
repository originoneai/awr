//! Frozen Team MCP business action permissions, role templates, and legacy
//! grant migration preview (AWR-TMCP-010).
//!
//! Pure contract only: no authentication, credential loading, or MCP/HTTP
//! dispatch. Role labels, tool discovery lists, and model self-reports never
//! grant authority by themselves.

use crate::error::{TeamError, TeamResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PERMISSION_POLICY_ID: &str = "awr-team-mcp-permission-v2";
pub const PERMISSION_POLICY_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleTemplate {
    Reader,
    Developer,
    Maintainer,
    ProjectAdmin,
}

impl RoleTemplate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Developer => "developer",
            Self::Maintainer => "maintainer",
            Self::ProjectAdmin => "project_admin",
        }
    }

    pub fn parse(name: &str) -> TeamResult<Self> {
        match name {
            "reader" => Ok(Self::Reader),
            "developer" => Ok(Self::Developer),
            "maintainer" => Ok(Self::Maintainer),
            "project_admin" => Ok(Self::ProjectAdmin),
            _ => Err(TeamError::InvalidInput(format!(
                "unknown role template: {name}"
            ))),
        }
    }

    pub fn all() -> [Self; 4] {
        [
            Self::Reader,
            Self::Developer,
            Self::Maintainer,
            Self::ProjectAdmin,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Action {
    #[serde(rename = "work.read")]
    WorkRead,
    #[serde(rename = "work.assign")]
    WorkAssign,
    #[serde(rename = "session.maintain_own")]
    SessionMaintainOwn,
    #[serde(rename = "claim.manage_own")]
    ClaimManageOwn,
    #[serde(rename = "execution.request_and_report_own")]
    ExecutionRequestAndReportOwn,
    #[serde(rename = "planning.propose")]
    PlanningPropose,
    #[serde(rename = "planning.edit_draft")]
    PlanningEditDraft,
    #[serde(rename = "planning.approve")]
    PlanningApprove,
    #[serde(rename = "planning.publish")]
    PlanningPublish,
    #[serde(rename = "delivery.submit_and_request_review")]
    DeliverySubmitAndRequestReview,
    #[serde(rename = "review.decide")]
    ReviewDecide,
    #[serde(rename = "delivery.finalize")]
    DeliveryFinalize,
    #[serde(rename = "access.manage_project")]
    AccessManageProject,
    #[serde(rename = "audit.read_project")]
    AuditReadProject,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkRead => "work.read",
            Self::WorkAssign => "work.assign",
            Self::SessionMaintainOwn => "session.maintain_own",
            Self::ClaimManageOwn => "claim.manage_own",
            Self::ExecutionRequestAndReportOwn => "execution.request_and_report_own",
            Self::PlanningPropose => "planning.propose",
            Self::PlanningEditDraft => "planning.edit_draft",
            Self::PlanningApprove => "planning.approve",
            Self::PlanningPublish => "planning.publish",
            Self::DeliverySubmitAndRequestReview => "delivery.submit_and_request_review",
            Self::ReviewDecide => "review.decide",
            Self::DeliveryFinalize => "delivery.finalize",
            Self::AccessManageProject => "access.manage_project",
            Self::AuditReadProject => "audit.read_project",
        }
    }

    pub fn parse(name: &str) -> TeamResult<Self> {
        match name {
            "work.read" => Ok(Self::WorkRead),
            "work.assign" => Ok(Self::WorkAssign),
            "session.maintain_own" => Ok(Self::SessionMaintainOwn),
            "claim.manage_own" => Ok(Self::ClaimManageOwn),
            "execution.request_and_report_own" => Ok(Self::ExecutionRequestAndReportOwn),
            "planning.propose" => Ok(Self::PlanningPropose),
            "planning.edit_draft" => Ok(Self::PlanningEditDraft),
            "planning.approve" => Ok(Self::PlanningApprove),
            "planning.publish" => Ok(Self::PlanningPublish),
            "delivery.submit_and_request_review" => Ok(Self::DeliverySubmitAndRequestReview),
            "review.decide" => Ok(Self::ReviewDecide),
            "delivery.finalize" => Ok(Self::DeliveryFinalize),
            "access.manage_project" => Ok(Self::AccessManageProject),
            "audit.read_project" => Ok(Self::AuditReadProject),
            _ => Err(TeamError::PermissionDenied(format!(
                "unknown action denied by default: {name}"
            ))),
        }
    }

    pub fn all() -> [Self; 14] {
        [
            Self::WorkRead,
            Self::WorkAssign,
            Self::SessionMaintainOwn,
            Self::ClaimManageOwn,
            Self::ExecutionRequestAndReportOwn,
            Self::PlanningPropose,
            Self::PlanningEditDraft,
            Self::PlanningApprove,
            Self::PlanningPublish,
            Self::DeliverySubmitAndRequestReview,
            Self::ReviewDecide,
            Self::DeliveryFinalize,
            Self::AccessManageProject,
            Self::AuditReadProject,
        ]
    }

    pub fn new_privileged_actions() -> [Self; 7] {
        [
            Self::WorkAssign,
            Self::PlanningPropose,
            Self::PlanningEditDraft,
            Self::PlanningApprove,
            Self::PlanningPublish,
            Self::AccessManageProject,
            Self::ReviewDecide,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecialAuthority {
    DatabaseOwner,
    SchemaMigration,
    ArbitrarySourceFilesystem,
    TrustedExecutorAttestation,
    ExecutionReconciliation,
    CrossProjectOrTenantAdministration,
}

impl SpecialAuthority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DatabaseOwner => "database_owner",
            Self::SchemaMigration => "schema_migration",
            Self::ArbitrarySourceFilesystem => "arbitrary_source_filesystem",
            Self::TrustedExecutorAttestation => "trusted_executor_attestation",
            Self::ExecutionReconciliation => "execution_reconciliation",
            Self::CrossProjectOrTenantAdministration => "cross_project_or_tenant_administration",
        }
    }

    pub fn all() -> [Self; 6] {
        [
            Self::DatabaseOwner,
            Self::SchemaMigration,
            Self::ArbitrarySourceFilesystem,
            Self::TrustedExecutorAttestation,
            Self::ExecutionReconciliation,
            Self::CrossProjectOrTenantAdministration,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRef {
    pub tenant_id: String,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workstream_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityScope {
    pub tenant_id: String,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workstream_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub work_ids: BTreeSet<String>,
    pub person_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_identity: Option<String>,
    pub client_id: String,
    pub allowed_actions: BTreeSet<Action>,
    #[serde(default)]
    pub independent_review_grant: bool,
    /// Agent approval never confers independent human review authority.
    #[serde(default)]
    pub agent_review_grant: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after_unix_ms: Option<u64>,
    #[serde(default)]
    pub revoked: bool,
    pub policy_version: u32,
}

pub fn template_actions(role: RoleTemplate) -> BTreeSet<Action> {
    use Action::*;
    let mut set = BTreeSet::new();
    match role {
        RoleTemplate::Reader => {
            set.insert(WorkRead);
        }
        RoleTemplate::Developer => {
            set.extend([
                WorkRead,
                SessionMaintainOwn,
                ClaimManageOwn,
                ExecutionRequestAndReportOwn,
                PlanningPropose,
                DeliverySubmitAndRequestReview,
            ]);
        }
        RoleTemplate::Maintainer => {
            set.extend([
                WorkRead,
                SessionMaintainOwn,
                ClaimManageOwn,
                ExecutionRequestAndReportOwn,
                PlanningPropose,
                PlanningEditDraft,
                PlanningApprove,
                PlanningPublish,
                DeliverySubmitAndRequestReview,
                DeliveryFinalize,
            ]);
        }
        RoleTemplate::ProjectAdmin => {
            set.extend([
                WorkRead,
                SessionMaintainOwn,
                ClaimManageOwn,
                ExecutionRequestAndReportOwn,
                PlanningPropose,
                PlanningEditDraft,
                PlanningApprove,
                PlanningPublish,
                DeliverySubmitAndRequestReview,
                DeliveryFinalize,
                AccessManageProject,
                AuditReadProject,
            ]);
        }
    }
    set
}

pub fn independent_review_eligible(role: RoleTemplate) -> bool {
    !matches!(role, RoleTemplate::Reader)
}

/// Business duties restrict existing grants; labels never create authority.
/// Several duties may be explicitly approved for one membership. This union
/// does not combine independent Agent delegations into synthetic authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusinessRole {
    Observer,
    Developer,
    Reviewer,
    Supervisor,
    Deliverer,
    Administrator,
}

impl BusinessRole {
    pub fn all() -> [Self; 6] {
        [
            Self::Observer,
            Self::Developer,
            Self::Reviewer,
            Self::Supervisor,
            Self::Deliverer,
            Self::Administrator,
        ]
    }

    pub fn action_ceiling(self) -> BTreeSet<Action> {
        use Action::*;
        match self {
            Self::Observer => BTreeSet::from([WorkRead]),
            Self::Developer => template_actions(RoleTemplate::Developer),
            Self::Reviewer => BTreeSet::from([WorkRead, SessionMaintainOwn, ReviewDecide]),
            Self::Supervisor => BTreeSet::from([
                WorkRead,
                WorkAssign,
                SessionMaintainOwn,
                PlanningPropose,
                PlanningEditDraft,
                PlanningApprove,
                PlanningPublish,
                AuditReadProject,
            ]),
            Self::Deliverer => BTreeSet::from([WorkRead, SessionMaintainOwn, DeliveryFinalize]),
            Self::Administrator => {
                BTreeSet::from([WorkRead, AccessManageProject, AuditReadProject])
            }
        }
    }
}

pub fn validate_business_roles(roles: &BTreeSet<BusinessRole>) -> TeamResult<()> {
    if roles.is_empty() {
        return Err(TeamError::InvalidInput(
            "business roles must not be empty".into(),
        ));
    }
    Ok(())
}

/// None preserves legacy policy. A declared set can only narrow the supplied
/// actions (which already include any separately eligible review grant).
pub fn constrain_actions_to_business_roles(
    actions: &BTreeSet<Action>,
    roles: Option<&BTreeSet<BusinessRole>>,
) -> BTreeSet<Action> {
    let Some(roles) = roles else {
        return actions.clone();
    };
    let ceiling: BTreeSet<_> = roles
        .iter()
        .flat_map(|role| role.action_ceiling())
        .collect();
    actions.intersection(&ceiling).copied().collect()
}

#[cfg(test)]
mod business_role_tests {
    use super::*;

    #[test]
    fn all_six_duties_have_bounded_action_contracts() {
        use Action::*;
        let granted = Action::all().into_iter().collect();
        let cases = [
            (BusinessRole::Observer, BTreeSet::from([WorkRead])),
            (
                BusinessRole::Developer,
                BTreeSet::from([
                    WorkRead,
                    SessionMaintainOwn,
                    ClaimManageOwn,
                    ExecutionRequestAndReportOwn,
                    PlanningPropose,
                    DeliverySubmitAndRequestReview,
                ]),
            ),
            (
                BusinessRole::Reviewer,
                BTreeSet::from([WorkRead, SessionMaintainOwn, ReviewDecide]),
            ),
            (
                BusinessRole::Supervisor,
                BTreeSet::from([
                    WorkRead,
                    WorkAssign,
                    SessionMaintainOwn,
                    PlanningPropose,
                    PlanningEditDraft,
                    PlanningApprove,
                    PlanningPublish,
                    AuditReadProject,
                ]),
            ),
            (
                BusinessRole::Deliverer,
                BTreeSet::from([WorkRead, SessionMaintainOwn, DeliveryFinalize]),
            ),
            (
                BusinessRole::Administrator,
                BTreeSet::from([WorkRead, AccessManageProject, AuditReadProject]),
            ),
        ];
        for (role, expected) in cases {
            assert_eq!(
                constrain_actions_to_business_roles(&granted, Some(&BTreeSet::from([role]))),
                expected
            );
        }
        assert_eq!(constrain_actions_to_business_roles(&granted, None), granted);
        assert!(constrain_actions_to_business_roles(&granted, Some(&BTreeSet::new())).is_empty());
        assert!(validate_business_roles(&BTreeSet::new()).is_err());
        assert!(serde_json::from_str::<BusinessRole>("\"owner\"").is_err());
    }

    #[test]
    fn combined_duties_only_filter_existing_authority_and_review_needs_a_grant() {
        let roles = BTreeSet::from([
            BusinessRole::Developer,
            BusinessRole::Reviewer,
            BusinessRole::Deliverer,
        ]);
        let mut scope = authority_from_template(
            RoleTemplate::Developer,
            "tenant",
            "project",
            "person",
            "client",
        );
        scope.allowed_actions =
            constrain_actions_to_business_roles(&scope.allowed_actions, Some(&roles));
        let resource = ResourceRef {
            tenant_id: "tenant".into(),
            project_id: "project".into(),
            workstream_id: None,
            work_id: None,
        };
        assert!(
            authorize_action(&scope, Action::ExecutionRequestAndReportOwn, &resource, 0).is_ok()
        );
        assert!(authorize_action(&scope, Action::ReviewDecide, &resource, 0).is_err());
        assert!(authorize_action(&scope, Action::DeliveryFinalize, &resource, 0).is_err());
        let mut scope = with_independent_review(scope, RoleTemplate::Developer).unwrap();
        scope.allowed_actions =
            constrain_actions_to_business_roles(&scope.allowed_actions, Some(&roles));
        assert!(authorize_action(&scope, Action::ReviewDecide, &resource, 0).is_ok());
        let reader = template_actions(RoleTemplate::Reader);
        assert_eq!(
            constrain_actions_to_business_roles(&reader, Some(&roles)),
            reader
        );
    }
}

fn scope_covers(
    scope: &AuthorityScope,
    resource: &ResourceRef,
    now_unix_ms: u64,
) -> TeamResult<()> {
    if scope.revoked {
        return Err(TeamError::PermissionDenied("authority revoked".into()));
    }
    if let Some(exp) = scope.not_after_unix_ms {
        if now_unix_ms > exp {
            return Err(TeamError::PermissionDenied("authority expired".into()));
        }
    }
    if !matches!(scope.policy_version, 1 | PERMISSION_POLICY_VERSION) {
        return Err(TeamError::PermissionDenied(
            "permission policy version mismatch".into(),
        ));
    }
    if scope.tenant_id != resource.tenant_id || scope.project_id != resource.project_id {
        return Err(TeamError::PermissionDenied(
            "tenant/project scope mismatch".into(),
        ));
    }
    if let Some(ref required_ws) = scope.workstream_id {
        match &resource.workstream_id {
            Some(ws) if ws == required_ws => {}
            _ => {
                return Err(TeamError::PermissionDenied(
                    "workstream scope mismatch".into(),
                ));
            }
        }
    }
    if !scope.work_ids.is_empty() {
        match &resource.work_id {
            Some(id) if scope.work_ids.contains(id) => {}
            _ => {
                return Err(TeamError::PermissionDenied(
                    "work id outside authorized set".into(),
                ));
            }
        }
    }
    if scope.person_id.trim().is_empty()
        || scope.client_id.trim().is_empty()
        || scope.tenant_id.trim().is_empty()
        || scope.project_id.trim().is_empty()
    {
        return Err(TeamError::PermissionDenied(
            "authority missing required identity fields".into(),
        ));
    }
    Ok(())
}

pub fn authorize_action(
    scope: &AuthorityScope,
    action: Action,
    resource: &ResourceRef,
    now_unix_ms: u64,
) -> TeamResult<()> {
    scope_covers(scope, resource, now_unix_ms)?;
    if action == Action::WorkAssign && scope.policy_version < 2 {
        return Err(TeamError::PermissionDenied(
            "work.assign requires permission policy v2".into(),
        ));
    }
    if action == Action::ReviewDecide {
        if !(scope.independent_review_grant || scope.agent_review_grant)
            || !scope.allowed_actions.contains(&Action::ReviewDecide)
        {
            return Err(TeamError::PermissionDenied(
                "review.decide requires independent review grant".into(),
            ));
        }
        return Ok(());
    }
    if scope.allowed_actions.contains(&action) {
        Ok(())
    } else {
        Err(TeamError::PermissionDenied(format!(
            "action {} not permitted by authority",
            action.as_str()
        )))
    }
}

pub fn authority_from_template(
    role: RoleTemplate,
    tenant_id: impl Into<String>,
    project_id: impl Into<String>,
    person_id: impl Into<String>,
    client_id: impl Into<String>,
) -> AuthorityScope {
    AuthorityScope {
        tenant_id: tenant_id.into(),
        project_id: project_id.into(),
        workstream_id: None,
        work_ids: BTreeSet::new(),
        person_id: person_id.into(),
        execution_identity: None,
        client_id: client_id.into(),
        allowed_actions: template_actions(role),
        independent_review_grant: false,
        agent_review_grant: false,
        not_after_unix_ms: None,
        revoked: false,
        policy_version: PERMISSION_POLICY_VERSION,
    }
}

pub fn with_independent_review(
    mut scope: AuthorityScope,
    role: RoleTemplate,
) -> TeamResult<AuthorityScope> {
    if !independent_review_eligible(role) {
        return Err(TeamError::PermissionDenied(
            "reader template cannot receive independent review grant".into(),
        ));
    }
    scope.independent_review_grant = true;
    scope.allowed_actions.insert(Action::ReviewDecide);
    Ok(scope)
}

pub fn deny_role_name_only(_role_name: &str, action: Action) -> TeamResult<()> {
    Err(TeamError::PermissionDenied(format!(
        "role name alone cannot authorize {}",
        action.as_str()
    )))
}

pub fn deny_tool_visibility_only(_tool_name: &str, action: Action) -> TeamResult<()> {
    Err(TeamError::PermissionDenied(format!(
        "tool visibility alone cannot authorize {}",
        action.as_str()
    )))
}

pub fn deny_model_self_report_only(_claim: &str, action: Action) -> TeamResult<()> {
    Err(TeamError::PermissionDenied(format!(
        "model self-report alone cannot authorize {}",
        action.as_str()
    )))
}

pub fn template_grants_special(_role: RoleTemplate, _special: SpecialAuthority) -> bool {
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonLinkStatus {
    Verified,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyRole {
    Reader,
    Reviewer,
    Worker,
    Admin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyGrant {
    Read,
    Write,
    Manage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationPreview {
    pub legacy_role: Option<LegacyRole>,
    pub legacy_grant: Option<LegacyGrant>,
    pub suggested_template: Option<RoleTemplate>,
    pub granted_actions: BTreeSet<Action>,
    pub withheld_new_actions: BTreeSet<Action>,
    pub person_link: PersonLinkStatus,
    pub independent_review_granted: bool,
    pub notes: Vec<String>,
}

pub fn preview_legacy_migration(
    legacy_role: Option<LegacyRole>,
    legacy_grant: Option<LegacyGrant>,
    person_link: PersonLinkStatus,
) -> MigrationPreview {
    let mut notes = Vec::new();
    if person_link == PersonLinkStatus::Unknown {
        notes.push("person relationship has no verified evidence; link stays unknown".into());
        return MigrationPreview {
            legacy_role,
            legacy_grant,
            suggested_template: None,
            granted_actions: BTreeSet::new(),
            withheld_new_actions: Action::new_privileged_actions().into_iter().collect(),
            person_link,
            independent_review_granted: false,
            notes,
        };
    }

    let mut role_template = None;
    let mut role_actions: Option<BTreeSet<Action>> = None;
    if let Some(role) = legacy_role {
        match role {
            LegacyRole::Reader => {
                role_template = Some(RoleTemplate::Reader);
                role_actions = Some(template_actions(RoleTemplate::Reader));
                notes.push("legacy reader maps to reader template".into());
            }
            LegacyRole::Reviewer => {
                role_template = Some(RoleTemplate::Reader);
                role_actions = Some(template_actions(RoleTemplate::Reader));
                notes.push(
                    "legacy reviewer maps to reader; review.decide needs a separate grant".into(),
                );
            }
            LegacyRole::Worker => {
                role_template = Some(RoleTemplate::Developer);
                role_actions = Some(template_actions(RoleTemplate::Developer));
                notes.push("legacy worker maps to developer template before withholding".into());
            }
            LegacyRole::Admin => {
                role_template = Some(RoleTemplate::ProjectAdmin);
                role_actions = Some(template_actions(RoleTemplate::ProjectAdmin));
                notes.push("legacy admin maps to project_admin template before withholding".into());
            }
        }
    }

    let mut grant_template = None;
    let mut grant_actions: Option<BTreeSet<Action>> = None;
    if let Some(grant) = legacy_grant {
        match grant {
            LegacyGrant::Read => {
                grant_template = Some(RoleTemplate::Reader);
                grant_actions = Some(template_actions(RoleTemplate::Reader));
                notes.push("legacy read grant contributes reader-template actions".into());
            }
            LegacyGrant::Write => {
                grant_template = Some(RoleTemplate::Developer);
                grant_actions = Some(template_actions(RoleTemplate::Developer));
                notes.push("legacy write grant contributes developer-template actions".into());
            }
            LegacyGrant::Manage => {
                grant_template = Some(RoleTemplate::ProjectAdmin);
                grant_actions = Some(template_actions(RoleTemplate::ProjectAdmin));
                notes.push("legacy manage grant contributes project_admin-template actions".into());
            }
        }
    }

    // Effective legacy authority is membership ∩ client grant (never union).
    // Missing either side contributes no actions from that side.
    let (suggested, mut granted) = match (role_actions, grant_actions) {
        (Some(role_set), Some(grant_set)) => {
            notes.push(
                "legacy membership and client grant are intersected; neither side expands the other"
                    .into(),
            );
            let suggested = role_template.or(grant_template);
            let intersection: BTreeSet<_> = role_set.intersection(&grant_set).copied().collect();
            (suggested, intersection)
        }
        (Some(role_set), None) => {
            notes.push("legacy grant missing; membership actions used before withholding".into());
            (role_template, role_set)
        }
        (None, Some(grant_set)) => {
            notes.push(
                "legacy membership missing; client grant actions used before withholding".into(),
            );
            (grant_template, grant_set)
        }
        (None, None) => {
            notes.push("legacy membership and grant both missing; no actions granted".into());
            (None, BTreeSet::new())
        }
    };

    // Newly introduced privileges are always withheld, independent of grant branch
    // and whether they appeared in the intersection.
    let withheld: BTreeSet<Action> = Action::new_privileged_actions().into_iter().collect();
    for action in &withheld {
        granted.remove(action);
    }
    notes.push(
        "newly introduced privileges (planning.*, access.manage_project, review.decide) are withheld from migration"
            .into(),
    );
    notes.push("independent review is never granted by migration preview".into());

    MigrationPreview {
        legacy_role,
        legacy_grant,
        suggested_template: suggested,
        granted_actions: granted,
        withheld_new_actions: withheld,
        person_link,
        independent_review_granted: false,
        notes,
    }
}

pub fn action_allowed_for_template(role: RoleTemplate, action: Action) -> bool {
    template_actions(role).contains(&action)
}
