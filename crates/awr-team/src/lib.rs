//! Team V1 domain contracts and pure rules.
//!
//! This crate has no database driver. SQLite and PostgreSQL adapters consume
//! these types; they do not own contract hashing or completion semantics.
mod access;
mod canonical;
mod completion;
mod contract;
pub mod delivery;
mod error;
mod ids;
mod permission;
mod planning;
mod snapshot;
mod version;
mod workstreams;

pub use access::{
    AuthContext, Envelope, RemoteProfile, SURFACES, authorize, execute, parse_envelope,
    same_error_on_all_surfaces, validate_only,
};
pub use canonical::{
    HASH_CODEC, canonical_json, contract_hash, reject_unknown_required_fields, request_hash,
};
pub use completion::{
    CompletionView, EvidenceBundle, EvidenceGrade, ReviewPolicy, current_completion,
};
pub use contract::{
    CrossWorkstreamDependencyPolicy, CrossWorkstreamReviewAssurance, DependencyAcceptanceMode,
    ExecutionSettlementMode, ExecutionSettlementPolicy, WorkContract, WorkDefinitionState,
};
pub use error::{TeamError, TeamResult};
pub use ids::{ActorId, ProjectId, RequestId, ScopeId, SessionId, TenantId, WorkId};
pub use permission::{
    Action, AuthorityScope, BusinessRole, LegacyGrant, LegacyRole, MigrationPreview,
    PERMISSION_POLICY_ID, PERMISSION_POLICY_VERSION, PersonLinkStatus, ResourceRef, RoleTemplate,
    SpecialAuthority, action_allowed_for_template, authority_from_template, authorize_action,
    constrain_actions_to_business_roles, deny_model_self_report_only, deny_role_name_only,
    deny_tool_visibility_only, independent_review_eligible, preview_legacy_migration,
    template_actions, template_grants_special, validate_business_roles, with_independent_review,
};
pub use planning::{
    AffectedTaskImpact, BaselineView, CandidateDiff, CandidateState, DraftChange,
    DraftDefinitionState, DraftOpKind, FORGE_COMPLETION_VIA_STATUS_ALLOWED, FieldDiff,
    HARD_DELETE_HISTORY_ALLOWED, OrdinaryPlanningSelfApprovePolicy, PLANNING_CODEC,
    PLANNING_CODEC_V2, PLANNING_CODEC_V3, PLANNING_CODEC_V4, PLANNING_CODEC_V5, PLANNING_CODEC_V6,
    PlanningApproval, PlanningCandidate, PlanningSuggestion, SUGGESTION_ADDS_FORMAL_WORK,
    SUGGESTION_API_WRITABLE_BY_READER, SUGGESTION_CLAIMABLE, SUGGESTION_MUTATES_LIVE_ACCEPTANCE,
    SUGGESTION_MUTATES_LIVE_DEPS, SuggestionState, TaskDraft, attested_actor_person,
    authorize_planning_action, authorize_planning_approve, authorize_planning_publish,
    build_candidate_diff, edit_candidate, ensure_independent_review_not_downgraded,
    planning_codec_for_changes, refuse_reader_suggestion_write, validate_candidate,
};
pub use snapshot::{
    ClaimPreconditions, LeaseProof, ProjectReadSnapshot, RequiredDependencyProof,
    SourceActivationPlan,
};
pub use version::{decode_u64, encode_u64};
pub use workstreams::{WorkstreamBundle, WorkstreamContract};

pub const PROTOCOL: &str = "awr-team";
pub const PROTOCOL_VERSION: u32 = 1;
