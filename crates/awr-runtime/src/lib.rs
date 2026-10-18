//! Agent runtime operations over an explicitly selected project.
//! Callers refresh source projections before acquisition. Cleanup remains possible with stale sources.
use awr_core::{Claim, Event, Id, Revision, Session, SessionDraft, SessionOutcome, SessionStarted};
pub use awr_core::{Error, Result};
use std::path::Path;
mod artifact;
mod branch;
mod branch_close;
mod compaction;
mod completion;
pub use compaction::{
    DeferCompactionRequest, InspectCompactionRequest, ObserveCompactionRequest, defer_compaction,
    inspect_compaction, observe_compaction,
};
mod guidance;
pub use guidance::guide_prepared_work;
mod doctor;
mod document;
mod host_save;
pub use document::{
    DocumentReport, DocumentRequest, change_document, document_status, recover_document,
};
pub use host_save::{
    HostChange, HostSaveReport, HostSaveRequest, host_preview, host_recover, host_save, host_status,
};
mod agent_authorization;
mod assessment_explain;
mod assessment_replay;
mod assessment_shadow;
mod execution;
mod explanation_chain;
mod fact_snapshot;
mod fs_sync;
mod management;
mod mutation;
mod mutation_apply;
mod operation_readset;
mod organization;
mod planning_writeback;
mod read;
mod responsibility;
mod resume;
mod source_concurrency;
mod team_handoff;
mod work_action;
mod work_create;
mod work_edit;
pub use work_edit::edit_work;
mod selective_invalidation;
mod work_graph;
mod workstream_accounting;
mod workstream_eta;
mod workstream_nav;
mod workstream_usage;
pub use selective_invalidation::{
    AdoptedConsumerEdge, BoundaryDecision, BoundaryRevalidation, BoundarySnapshot,
    CancelSplitRelation, DiscoverDependencyRequest, ExecutionBoundary, PlanningChangeApplication,
    PlanningChangeStatus, ProviderChangeKind, ScopedPlanningChange, SelectiveInvalidationPlan,
    action_blocked_by_planning_changes, confirm_planning_change, consumers_by_provider,
    planning_change_fingerprint, record_discovered_dependency_change, reject_planning_change,
    revalidate_execution_boundary, select_downstream_reevaluation,
};
pub use work_graph::{
    SharedOutcomeRef, WorkGraphRequest, necessary_dependencies_ready, reference_shared_outcome,
    unique_shared_work_keys, validate_cross_stream_work_graph, work_graph,
};
pub use workstream_accounting::{
    ApprovedContractSnapshot, ContractAccountingReport, GoalQueryResult, GoalQueryView,
    OwnershipTransfer, ScopeAccountingError, TransferAccountingOutcome, VerifiedStageObservation,
    VerifiedWorkObservation, account_approved_scope, goal_query_view,
    refuse_goal_query_as_contract_rate, transfer_work_preserving_history, unique_owned_work_keys,
};
pub use workstream_eta::{
    AttestedEtaProject, EtaRuntimeError, attach_observation_handoff, estimate_and_persist,
    observation_handoff_for_estimate, query_eta_forecast, query_eta_forecasts_for_target,
    record_acceptance_datum, record_historical_sample, reestimate_and_persist,
};
pub use workstream_nav::{
    MAINLINE_NAV_PROTOCOL, MAINLINE_NAV_SCHEMA_VERSION, MainlineNavExtras, MainlineNavScope,
    MainlineNavWorkFact, assemble_mainline_nav, mainline_nav,
};
pub use workstream_usage::{
    AttestedUsageProject, UsageRuntimeError, ingest_usage_receipt, query_usage_cost_totals,
    query_usage_occurrence_bindings, query_usage_time_totals, record_usage_allocation,
    record_usage_correction, record_usage_counter_snapshot, record_usage_execution_interval,
    refuse_eta_from_cumulative_duration, usage_observation_for_ws043,
};
mod response_view;
mod workflow;
pub use artifact::ArtifactFile;
pub use assessment_explain::{
    ASSESSMENT_EXPLAIN_CAPABILITY, ASSESSMENT_EXPLAIN_FIELD, AssessmentExplanationView,
    AttachExplanationOptions, ExplanationSideEffects, ExplanationWireMetrics,
    attach_assessment_explanation, has_assessment_explanation, normalize_receipt_for_explanation,
    wire_bytes,
};
pub use assessment_replay::{
    ASSESSMENT_REPLAY_CAPABILITY, REPLAY_SNAPSHOT_SCHEMA_ID, REPLAY_SNAPSHOT_SCHEMA_VERSION,
    ReplayReport, ReplaySnapshot, ReplayStatus, capture_replay_snapshot, parse_replay_snapshot,
    replay_assessment, replay_assessment_from_bytes, replay_missing_snapshot, rule_hash_for_policy,
};
pub use assessment_shadow::{
    ASSESSMENT_ADVICE_MODE_CAPABILITY, ASSESSMENT_SHADOW_COMPARE_CAPABILITY, AdviceDeliveryMode,
    AdviceModeEffect, ArmSummary, CompareCosts, HardProtectionFlags, ShadowCompareReport,
    ShadowDifference, apply_advice_delivery_mode, attach_according_to_advice_mode,
    hard_protections_after_disable, shadow_compare,
};
use awr_store::Store;
pub use awr_store::{BranchFilter, EventCursor, EventPage, EventQuery};
pub use branch::{CreateBranchRequest, create_branch, observe_git_ref, switch_branch};
pub use branch_close::{CloseBranchRequest, close_branch};
pub use completion::{CompleteWorkRequest, complete_work};
pub use doctor::{ProjectDoctorReport, diagnose_project};
pub use execution::{
    HostIsolationEvidence, IsolationClass, classify_isolation, inspect_execution,
    inspect_work_executions, isolation_basis, refuse_unverified_strong_isolation,
    render_execution_observations,
};
pub use explanation_chain::{
    CompletionExplanationInput, DeliveryExplanationInput, EXPLANATION_CHAIN_PROFILE,
    ExplanationAuthority, ExplanationChainInput, ExplanationChainResult, FORBIDDEN_RERUN_CUES,
    ProbeSupport, UnresolvedSideEffect, compose_explanation_chain,
    explanation_chain_from_prepare_json, prior_explanation_still_valid,
};

pub use fact_snapshot::{
    PREPARE_FACT_MAX_BYTES, PREPARE_FACT_MAX_CANDIDATES, PREPARE_FACT_MAX_SCAN_OPS,
    PreparedFactView, fact_snapshot_from_prepared_view, prepared_view_from_prepare_json,
};
pub mod host_adapter;
pub use host_adapter::{
    AdapterActionOutcome, AdapterForensics, AdapterRegistry, AdapterStatus, ClaudeCodeAdapter,
    CodexCliAdapter, ExecutionHostAdapter, L0ManualAdapter, NativeExecutionHandle,
    ParallelDispatchPlan, ParallelScheduler, ParentExitEffect, PauseGate, ReconnectRetry,
    ScheduleDecision, SubtaskRecord, SubtaskState, built_in_registry,
    refuse_coordination_as_process_control, rollup_refs,
};
pub use management::{AssessManagementRequest, ManageWorkRequest, assess_management, manage_work};
pub use mutation::{
    CreateProposalRequest, ProposalReport, ReviewProposalAction, ReviewProposalRequest,
    create_proposal, review_proposal,
};
pub use operation_readset::{append_work_observation, classify_operation_replay_result};
pub use organization::{OrganizationReport, OrganizationState, inspect_organization};
pub use planning_writeback::{
    ActivationDisposition, ActivationImpactReport, AffectedWorkDecision, WorkRuntimeObservation,
    WritebackJournal, WritebackPhase, activate_ledger_writeback_precise, analyze_activation_impact,
    plan_ledger_writeback, planning_change_blocks_until_confirmed,
    planning_changes_as_selective_replan, reevaluate_graph_consumers,
    same_request_already_completed, writeback_journal_path,
};
pub use response_view::{compact_prepared_response, summarize_work_response};
pub use resume::{ResumeReport, ResumeRequest, resume_bound_session, resume_session};
pub use source_concurrency::{
    SourceConcurrencyReport, activate_precise_patch, activate_shard_candidate,
    classify_whole_file_gate, recover_shard_candidate,
};
pub use work_action::{WorkActionRequest, perform_work_action};
pub use work_create::{
    CreateWorkInput, CreationReport, create_work, creation_status, recover_creation,
};
pub use workflow::{
    PrepareCompletionRequest, PrepareWorkRequest, prepare_completion, prepare_work,
};

/// Hard ceilings; a caller may select a smaller budget, never raise these limits.
pub const ARTIFACT_IMPORT_CAP: u64 = 64 * 1024 * 1024;
pub const REGISTERED_CONTENT_READ_CAP: u64 = 16 * 1024 * 1024;

/// Refuse a locator that provably resolves outside the project root and its authorized roots.
///
/// This is the write-side twin of the read-path rule, so registration cannot store a record
/// that `read_registered_file` is already guaranteed to reject. Both sides call
/// `read::within_project_or_authorized`, so the containment rule exists once. Only provable
/// escapes are refused: a locator that cannot be resolved yet keeps its previous behaviour,
/// because this guard does not predict where a caller will place the file next.
fn reject_provably_outside_root(store: &Store, project: Id, locator: &str) -> Result<()> {
    // Project locators carry their own rule: `awr_source::Locator` resolves them
    // against the project root only and refuses every escaping component at parse
    // time. Returning early keeps this guard's domain disjoint from that rule, so
    // the two checks cannot disagree about which one sees the locator first.
    if locator.starts_with("project://") {
        return Ok(());
    }
    let project = store.project(project)?;
    let Ok(root) = project.root.canonicalize() else {
        return Ok(());
    };
    let candidate = if Path::new(locator).is_absolute() {
        Path::new(locator).to_path_buf()
    } else {
        root.join(locator)
    };
    let Ok(path) = candidate.canonicalize() else {
        return Ok(());
    };
    if read::within_project_or_authorized(&root, &path)? {
        Ok(())
    } else {
        Err(Error::RuleViolation(
            "registered file is outside project and authorized roots".into(),
        ))
    }
}

pub struct Runtime<'a> {
    store: &'a mut Store,
    project: Id,
}
impl<'a> Runtime<'a> {
    pub fn record_evidence(
        &mut self,
        expected: Revision,
        draft: awr_core::EvidenceDraft,
    ) -> Result<(awr_core::Evidence, Event)> {
        let draft = self.bind_project_locator(draft)?;
        reject_provably_outside_root(self.store, self.project, &draft.locator)?;
        self.store.record_evidence(self.project, expected, draft)
    }
    /// Resolve a `project://` locator exactly once, at registration.
    ///
    /// The stored string is the canonical project-relative identity, so the same
    /// evidence row resolves against the same relative file on every node that
    /// holds the checkout. A missing, escaping or ambiguous target is a typed
    /// error at this point and never a guess at read time. `file://`, `git://`
    /// and plain paths pass through untouched.
    fn bind_project_locator(
        &self,
        mut draft: awr_core::EvidenceDraft,
    ) -> Result<awr_core::EvidenceDraft> {
        if !draft.locator.starts_with("project://") {
            return Ok(draft);
        }
        let project = self.store.project(self.project)?;
        let root = project.root.canonicalize()?;
        let manifest = awr_source::Manifest::load(&root)?;
        let locator = awr_source::Locator::parse(&root, &manifest, &draft.locator)?;
        let snapshot = locator.read(&root, REGISTERED_CONTENT_READ_CAP)?;
        if let Some(declared) = draft.sha256.as_deref() {
            let actual = snapshot
                .fingerprint
                .strip_prefix("sha256:")
                .unwrap_or(&snapshot.fingerprint);
            if !actual.eq_ignore_ascii_case(declared) {
                return Err(Error::SourceConflict(format!(
                    "declared sha256 does not match the bytes the project locator resolves to: expected {declared}, actual {actual}"
                )));
            }
        }
        draft.locator = snapshot.locator;
        Ok(draft)
    }
    pub fn handoff(
        &mut self,
        expected: Revision,
        from: Id,
        to: Option<Id>,
        ttl_ms: Option<u64>,
    ) -> Result<(awr_core::Handoff, Event)> {
        self.store.handoff(self.project, expected, from, to, ttl_ms)
    }
    pub fn checkpoint(
        &mut self,
        expected: Revision,
        session: Id,
        draft: awr_core::CheckpointDraft,
    ) -> Result<(awr_core::Checkpoint, Event)> {
        self.checkpoint_as(expected, session, draft, None)
    }
    /// Record an optional caller declaration independently from the session label.
    pub fn checkpoint_as(
        &mut self,
        expected: Revision,
        session: Id,
        draft: awr_core::CheckpointDraft,
        agent: Option<&str>,
    ) -> Result<(awr_core::Checkpoint, Event)> {
        let started =
            self.store
                .begin_checkpoint_save_as(self.project, expected, session, draft, agent)?;
        self.store
            .finish_checkpoint_save(self.project, started.project_revision, started.id)
            .map_err(|error| Error::CheckpointIncomplete {
                attempt_id: started.id,
                reason: error.to_string(),
            })
    }
    pub fn append_event(
        &mut self,
        expected: Revision,
        draft: awr_core::EventDraft,
    ) -> Result<Event> {
        self.store.append_event(self.project, expected, draft)
    }
    /// Preserve an explicit event branch selection, including main.
    pub fn append_event_in_branch(
        &mut self,
        expected: Revision,
        draft: awr_core::EventDraft,
    ) -> Result<Event> {
        self.store
            .append_event_in_branch(self.project, expected, draft)
    }
    pub fn events(&self, query: &EventQuery) -> Result<EventPage> {
        self.store.query_events(self.project, query)
    }
    pub fn attach(store: &'a mut Store, project: Id) -> Result<Self> {
        store.project(project)?;
        Ok(Self { store, project })
    }
    pub fn start_session(
        &mut self,
        expected: Revision,
        draft: SessionDraft,
    ) -> Result<(SessionStarted, Event)> {
        self.store.start_session(self.project, expected, draft)
    }
    /// Start (or reuse) a session bound to a host client/conversation pair,
    /// the same binding surface the MCP channel uses.
    pub fn start_bound_session(
        &mut self,
        expected: Revision,
        draft: SessionDraft,
        binding: awr_core::McpSessionBinding,
    ) -> Result<(SessionStarted, Event)> {
        self.store
            .start_bound_session(self.project, expected, draft, binding)
    }
    /// Explicit scope selection, validated against the work when one is supplied.
    pub fn start_session_in_workstream(
        &mut self,
        expected: Revision,
        draft: SessionDraft,
        binding: Option<awr_core::McpSessionBinding>,
        workstream: Id,
    ) -> Result<(SessionStarted, Event)> {
        self.store
            .start_session_in_workstream(self.project, expected, draft, binding, workstream)
    }
    pub fn session_workstream(&self, session: Id) -> Result<awr_core::SessionWorkstream> {
        self.store.session_workstream(self.project, session)
    }
    /// Trusted host selection only; authentication and grants belong to the adapter.
    pub fn select_conversation_workstream(
        &mut self,
        expected: Revision,
        binding: awr_core::McpSessionBinding,
        workstream: Id,
    ) -> Result<((), Event)> {
        self.store
            .select_conversation_workstream(self.project, expected, binding, workstream)
    }
    pub fn acquire_claim(
        &mut self,
        expected: Revision,
        session: Id,
        ttl_ms: Option<u64>,
    ) -> Result<(Claim, Event)> {
        self.store
            .acquire_claim(self.project, expected, session, ttl_ms)
    }
    pub fn release_claim(
        &mut self,
        expected: Revision,
        session: Id,
        claim: Id,
    ) -> Result<(Claim, Event)> {
        self.store
            .release_claim(self.project, expected, session, claim)
    }
    pub fn end_session(
        &mut self,
        expected: Revision,
        session: Id,
        outcome: SessionOutcome,
    ) -> Result<(Session, Event)> {
        self.store
            .end_session(self.project, expected, session, outcome)
    }
}
mod ordinary;
pub use ordinary::*;

mod batch;
pub use batch::*;

mod status_summary;
pub use status_summary::{StatusScope, summarize_status};
mod status_action;
pub use status_action::{action_status, action_status_page};
mod work_progress;
pub use work_progress::work_progress;

mod organization_change;
pub use organization_change::{
    OrganizationChange, change_organization, organization_preview, organization_status,
    read_organization, recover_organization,
};
