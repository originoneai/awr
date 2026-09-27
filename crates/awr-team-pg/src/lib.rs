//! PostgreSQL coordination store for Team V1.
//! Personal SQLite runtime does not depend on this crate.
mod agent_authorization;
mod bootstrap;
mod delegation_auth;
mod delivery_adoption;
mod error;
mod execution;
mod graph;
mod import;
mod lease;
mod lock_order;
mod migrate;
mod operator_access;
mod operator_agent;
mod operator_backup;
mod operator_execution_attribution;
mod operator_history;
mod operator_quarantine;
mod operator_recovery;
mod ops_audit;
mod path;
mod pool;
mod read;
mod responsibility;
mod review;
mod runner;
mod scoped_runner;
mod selective_invalidation;
mod source;
mod team_handoff;
mod tx;
mod workstream_auth;
mod workstream_command;
mod workstream_read;

pub use agent_authorization::AuthorizationStore;
pub use bootstrap::Bootstrap;
pub use delegation_auth::{
    actor_requires_explicit_delegation, intersect_delegation_with_template,
    tmcp_actions_for_authorized, tmcp_actions_for_authorized_set,
};
pub use delivery_adoption::DeliveryAdoptionStore;
pub use error::{PgError, PgResult};
pub use execution::{
    ExecutionRecord, ExecutionStore, OutboxDelivery, admit_live_fence, exactly_once_supported,
    unknown_effect_retains_resources,
};
pub use graph::{
    DependencyEdge, EdgeMutation, GraphStore, ResourceBound, ResourceDomain, ResourceLeaseBind,
    SharedOutcomeRef, SplitProposal, necessary_dependencies_ready, paths_conflict,
    reference_shared_outcome, require_main_scope, resource_domain, resources_conflict,
    validate_cross_stream_graph, validate_required_graph, validate_resource_kind,
};
pub use import::{BackupRecord, FencingBarrier, ImportJob, ImportStore, InspectReport, RestoreRun};
pub use lease::{ClaimRecord, LeaseStore, SessionRecord};
pub use lock_order::{
    ResourceLockKey, lock_claim_after_work, lock_resources_sorted, lock_works_sorted,
    sort_resource_keys, sort_work_ids,
};
mod feedback;

pub use migrate::{EXPECTED_SCHEMA_VERSION, check_schema, migrate};
pub use operator_access::{
    AccessActor, AccessCredential, AccessGrant, AccessPlan, AdminAccessPlan, OperatorAccess,
    ProjectAccessStore,
};
pub use operator_agent::{
    AgentAuthorizationIssuePlan, AgentProvisionPlan, AgentRenewPlan, OperatorAgent,
};
pub use operator_backup::OperatorBackup;
pub use operator_execution_attribution::{
    ExecutionAttributionEntry, ExecutionAttributionPlan, OperatorExecutionAttribution,
};
pub use operator_history::OperatorHistory;
pub use operator_quarantine::OperatorQuarantine;
pub use operator_recovery::OperatorRecovery;
pub use ops_audit::{
    DENY_CAPACITY_PER_PROJECT, OpsAuditStore, OpsAuditWrite, OpsCategory, OpsDenyWrite,
    OpsHistoryFilter, digest_of, record_deny, record_in_tx, redact_summary,
};
pub use path::{
    MAX_FILE_BYTES, MAX_PACKAGE_BYTES, MAX_SOURCE_FILES, validate_package, validate_source_path,
};
pub use pool::{PgClient, PgPool};
pub use read::{
    EventCursor, EventPage, EventRecord, PreparedWork, ReadStore, WorkGraph, capabilities,
    dispatch_query,
};
pub use responsibility::ResponsibilityStore;
pub use review::{CompletionReceipt, EvidenceRecord, PrDelivery, ReviewRound, ReviewStore};
#[doc(hidden)]
pub use runner::fence_key;
pub use runner::{CrashPoint, ReferenceRunner, RunnerOutcome};
pub use scoped_runner::{
    ReferenceReportRequest, ReferenceRunRequest, ReferenceWrite, ReferenceWritePlan,
    ScopedReferenceRunner,
};
pub use selective_invalidation::{
    AdoptedConsumerEdge, BoundaryDecision, BoundaryRevalidateRequest, BoundaryRevalidation,
    BoundarySnapshot, CancelSplitRelation, DecidePlanningChangeRequest, ExecutionBoundary,
    ProviderChangeKind, RecordPlanningChangeRequest, ScopedPlanningChange,
    SelectiveInvalidateRequest, SelectiveInvalidationPlan, SelectiveInvalidationReceipt,
    SelectiveInvalidationStore, revalidate_execution_boundary, select_downstream_reevaluation,
};
pub use source::planning::{DraftCandidateCreate, PlanningCommandBind, SuggestionSubmit};
pub use source::planning_ops::{
    PlanningApproveRequest, PlanningDraftRequest, PlanningPublishRequest, PlanningSuggestRequest,
};
pub use source::writeback::{ActivationImpactGate, WritebackActivateRequest};
pub use source::{
    CandidateRecord, CurrentSource, CurrentWorkstreamSource, IngestRequest, SOURCE_BINDING_FILE,
    SOURCE_PROVENANCE_FILE, SoleSourceBinding, SoleSourceKind, SourceFile, SourceStore,
    WORKSTREAMS_FILE,
};
pub use team_handoff::HandoffStore;
pub use tx::{CommandOutcome, CommandRequest, TeamStore};
pub use workstream_auth::{
    command_business_action, map_membership_role, query_business_action, workstream_credential_hash,
};
pub use workstream_command::{WorkstreamCommand, WorkstreamCommandStore};
pub use workstream_read::{WorkstreamQuery, WorkstreamReadStore};

pub const SCHEMA: &str = "awr_team";

/// Connect a single dedicated client (owner migration / bootstrap path).
/// Domain stores use pooled connections via [`PgPool`] instead (ADR-0004).
pub async fn connect(url: &str) -> PgResult<tokio_postgres::Client> {
    pool::connect(url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_contract_is_stable() {
        assert_eq!(SCHEMA, "awr_team");
        assert_eq!(EXPECTED_SCHEMA_VERSION, 36);
    }
}
