//! What AWR already recorded about the outcome of work, counted without inventing any of it.
//!
//! Pure over records an adapter has read from ONE project: no store, no file, no clock (the instant of the
//! question is an input), no model, no network. It adds no authority and changes no receipt. The completion
//! of the engineering evidence, the confirmation of a human and the exit status of a supervised command are
//! counted on their own and never merged into one score; a missing duration or cost stays missing; a model
//! that never ran has no cohort; and nothing here is a rate, because a rate needs a denominator and a
//! definition of success that the receipts do not carry.
use crate::workstream_usage::{
    UsageCostTotals, UsageError, UsageReceipt, deduplicate_usage, usage_cost_totals,
};
use crate::{EvidenceLevel, ExecutionState, HostEditOrigin, Id};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const OUTCOME_HISTORY_VERSION: u32 = 1;
/// More records of one family than this are refused rather than summarised from a prefix.
pub const MAX_HISTORY_RECORDS: usize = 200_000;
const MAX_FILTER_KEYS: usize = 1_000;

/// What is never derived here, as pre-registered by the assessment corpus (P05, C17).
pub const NOT_DERIVED: [&str; 3] = [
    "model_success_rate",
    "dollar_savings",
    "semantic_understanding",
];

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OutcomeHistoryError {
    #[error("a record belongs to another project")]
    CrossProject,
    #[error("two works share an identity or a key")]
    DuplicateWork,
    #[error("the same receipt identity carries different content")]
    ConflictingReceipt,
    #[error("invalid request")]
    InvalidRequest,
    #[error("more than {MAX_HISTORY_RECORDS} records of one family")]
    TooLarge,
    #[error(transparent)]
    Usage(#[from] UsageError),
}

/// A work of the project, as the projection has it at the time of the question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryWork {
    pub work_item_id: Id,
    pub work_key: String,
    /// Digest of the acceptance criteria the work has now (`acceptance_digest`); none when not recorded.
    pub acceptance_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    /// `work.completed`: a completion with caller-supplied evidence bindings.
    Completed,
    /// `work.ordinary_confirmed`: a confirmation of ordinary work, with who made it.
    OrdinaryConfirmed,
    /// `work.cancelled`.
    Cancelled,
    /// `work.reopened`: withdraws what a completion, a confirmation or a cancellation said.
    Reopened,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryReceipt {
    pub event_id: Id,
    pub project_id: Id,
    pub work_item_id: Id,
    pub kind: ReceiptKind,
    pub at_ms: i64,
    pub project_revision: u64,
    /// The version of the work the receipt was written against; none when the receipt does not say.
    pub work_revision: Option<u64>,
    pub session_id: Option<Id>,
    /// Completed: the highest verification level of the evidence it bound.
    pub evidence_level: Option<EvidenceLevel>,
    /// Completed: digest of the acceptance criteria it bound (`acceptance_digest`).
    pub acceptance_digest: Option<String>,
    /// OrdinaryConfirmed: who confirmed.
    pub actor_origin: Option<HostEditOrigin>,
}

/// A supervised execution bound to a work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryExecution {
    pub execution_id: Id,
    pub project_id: Id,
    pub work_item_id: Id,
    pub session_id: Id,
    pub state: ExecutionState,
    pub registered_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySession {
    pub session_id: Id,
    pub project_id: Id,
    pub provider: String,
    pub model: String,
    pub agent: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryInput {
    pub project_id: Id,
    pub works: Vec<HistoryWork>,
    pub receipts: Vec<HistoryReceipt>,
    pub executions: Vec<HistoryExecution>,
    pub sessions: Vec<HistorySession>,
    /// Usage receipts as the store keeps them (corrections already applied by the adapter).
    pub usage: Vec<UsageReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CohortBy {
    /// One cohort of every work in scope.
    None,
    /// By the provider and model of the session that wrote the latest completion, confirmation or cancellation
    /// of the work (withdrawn or not), else of its latest execution.
    ProviderModel,
    /// By the agent of that session.
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRequest {
    /// The instant of the question: later records are left out and counted, never silently read.
    pub as_of_ms: i64,
    pub cohort_by: CohortBy,
    /// Only these works (by key); none: every work of the project.
    pub work_keys: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryScope {
    pub project_id: Id,
    pub as_of_ms: i64,
    pub cohort_by: CohortBy,
    pub work_keys: Option<Vec<String>>,
    /// Keys the filter named that no work of the project has.
    pub unknown_work_keys: Vec<String>,
}

/// What happened to the records read, by family.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyCounts {
    pub read: usize,
    pub used: usize,
    pub duplicates: usize,
    pub after_as_of: usize,
    /// Naming no work this project has.
    pub unlocatable: usize,
    pub outside_filter: usize,
    /// Used receipts that do not say which version of the work they were written against.
    pub without_version: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleCounts {
    pub works_in_scope: usize,
    pub receipts: FamilyCounts,
    pub executions: FamilyCounts,
    pub usage: FamilyCounts,
    /// Works whose latest receipt names a session that has no label here.
    pub sessions_unlabelled: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalCounts {
    /// A completion in force whose bound acceptance is the work's current one.
    pub completion_reported: usize,
    /// A completion in force that bound other acceptance than the work has now: not a pass under the new standard.
    pub completion_under_earlier_standard: usize,
    /// A completion in force that cannot be compared with the current acceptance.
    pub completion_standard_unknown: usize,
    /// The highest evidence level the completions in force bound, `not_recorded` when the receipt does not say.
    pub completion_by_evidence_level: BTreeMap<String, usize>,
    pub accepted_by_human: usize,
    pub accepted_by_ai: usize,
    pub accepted_by_delegate: usize,
    pub cancelled: usize,
    /// Two completions in force at once: neither is chosen.
    pub conflicting: usize,
    /// No completion, confirmation or cancellation in force: not a failure, not yet known.
    pub open: usize,
    pub reopened_works: usize,
    pub withdrawn_completions: usize,
    pub withdrawn_acceptances: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionCounts {
    pub succeeded: usize,
    pub failed: usize,
    /// Not finished at the instant of the question.
    pub unfinished: usize,
    pub observed_execution_ms: u64,
    /// Finished, but a start or an end is not recorded: not zero.
    pub without_duration: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostSummary {
    pub works_with_usage: usize,
    /// Cost not observed: not zero.
    pub works_without_usage: usize,
    /// Known subtotals by currency; actual and API-equivalent never added.
    pub actual_micros: BTreeMap<String, u64>,
    pub api_equivalent_micros: BTreeMap<String, u64>,
    pub unknown_cost_calls: usize,
    pub unique_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CohortKey {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeCohort {
    pub key: CohortKey,
    /// The works whose latest receipt names no labelled session (or no receipt, no execution).
    pub unattributed: bool,
    pub works: usize,
    pub signals: SignalCounts,
    pub executions: ExecutionCounts,
    pub cost: CostSummary,
}

/// `(observed, expected)` as integers; never a percentage.
pub type Coverage = (usize, usize);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryCoverage {
    pub works_with_outcome: Coverage,
    pub works_with_execution: Coverage,
    pub works_with_usage: Coverage,
    pub executions_with_duration: Coverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeHistory {
    pub version: u32,
    pub scope: HistoryScope,
    pub sample: SampleCounts,
    pub cohorts: Vec<OutcomeCohort>,
    pub coverage: HistoryCoverage,
    pub not_derived: Vec<String>,
    pub notes: Vec<String>,
    /// Over everything above: the same input, request and instant give the same hash.
    pub history_hash: String,
}

/// Digest of a set of acceptance criteria: sorted and unique, so the order of the list is not a version.
pub fn acceptance_digest(criteria: &[String]) -> String {
    let mut sorted: Vec<&str> = criteria.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let bytes = serde_json::to_vec(&sorted).expect("strings serialize");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// The name a level has in the report.
pub fn level_name(level: EvidenceLevel) -> &'static str {
    match level {
        EvidenceLevel::Designed => "designed",
        EvidenceLevel::Implemented => "implemented",
        EvidenceLevel::LocallyVerified => "locally_verified",
        EvidenceLevel::RealEnvironmentValidated => "real_environment_validated",
        EvidenceLevel::ReleaseCandidate => "release_candidate",
        EvidenceLevel::Released => "released",
        EvidenceLevel::Unknown => "unknown",
    }
}

type Order = (u64, Id);

#[derive(Clone)]
struct InForce {
    order: Order,
    session: Option<Id>,
    level: Option<EvidenceLevel>,
    digest: Option<String>,
    origin: Option<HostEditOrigin>,
}

#[derive(Default)]
struct WorkState {
    /// The session of the latest completion, confirmation or cancellation, withdrawn or not: who produced the outcome.
    outcome_author: Option<(Order, Option<Id>)>,
    completion: Option<InForce>,
    acceptance: Option<InForce>,
    cancelled: Option<InForce>,
    conflicting: bool,
    reopened: bool,
    withdrawn_completions: usize,
    withdrawn_acceptances: usize,
}

impl WorkState {
    fn apply(&mut self, receipt: &HistoryReceipt) {
        let held = InForce {
            order: (receipt.project_revision, receipt.event_id),
            session: receipt.session_id,
            level: receipt.evidence_level,
            digest: receipt.acceptance_digest.clone(),
            origin: receipt.actor_origin,
        };
        if receipt.kind != ReceiptKind::Reopened
            && self
                .outcome_author
                .as_ref()
                .is_none_or(|(order, _)| *order < held.order)
        {
            self.outcome_author = Some((held.order, held.session));
        }
        match receipt.kind {
            ReceiptKind::Completed => {
                // A second completion in force is neither the newer one nor the older one.
                if self.completion.is_some() {
                    self.conflicting = true;
                }
                self.completion = Some(held);
                self.cancelled = None;
            }
            ReceiptKind::OrdinaryConfirmed => self.acceptance = Some(held),
            ReceiptKind::Cancelled => self.cancelled = Some(held),
            ReceiptKind::Reopened => {
                self.withdrawn_completions += usize::from(self.completion.take().is_some());
                self.withdrawn_acceptances += usize::from(self.acceptance.take().is_some());
                self.cancelled = None;
                self.conflicting = false;
                self.reopened = true;
            }
        }
    }

    /// The session that wrote the latest completion, confirmation or cancellation of the work.
    fn latest_session(&self) -> Option<Id> {
        self.outcome_author
            .as_ref()
            .and_then(|(_, session)| *session)
    }

    fn has_outcome(&self) -> bool {
        self.completion.is_some() || self.acceptance.is_some() || self.cancelled.is_some()
    }
}

#[derive(Default)]
struct CohortAcc {
    works: usize,
    signals: SignalCounts,
    executions: ExecutionCounts,
    usage: Vec<UsageReceipt>,
    works_with_usage: usize,
}

fn check_size(len: usize) -> Result<(), OutcomeHistoryError> {
    if len > MAX_HISTORY_RECORDS {
        return Err(OutcomeHistoryError::TooLarge);
    }
    Ok(())
}

fn text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

/// Counts, for the works in scope, what the receipts, executions and usage of ONE project say at an instant.
pub fn aggregate_outcome_history(
    input: &HistoryInput,
    request: &HistoryRequest,
) -> Result<OutcomeHistory, OutcomeHistoryError> {
    if request.as_of_ms < 0 {
        return Err(OutcomeHistoryError::InvalidRequest);
    }
    if let Some(keys) = &request.work_keys {
        if keys.len() > MAX_FILTER_KEYS || keys.iter().any(|key| !text(key)) {
            return Err(OutcomeHistoryError::InvalidRequest);
        }
    }
    for len in [
        input.works.len(),
        input.receipts.len(),
        input.executions.len(),
        input.usage.len(),
    ] {
        check_size(len)?;
    }
    let project = input.project_id;
    if input.receipts.iter().any(|r| r.project_id != project)
        || input.executions.iter().any(|e| e.project_id != project)
        || input.sessions.iter().any(|s| s.project_id != project)
        || input
            .usage
            .iter()
            .any(|u| u.project_id != project.to_string())
    {
        return Err(OutcomeHistoryError::CrossProject);
    }

    // Identities: a work is found by its id or its key, and by nothing that is both of two works.
    let mut by_name: BTreeMap<String, Id> = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for work in &input.works {
        if !text(&work.work_key) || !ids.insert(work.work_item_id) {
            return Err(OutcomeHistoryError::DuplicateWork);
        }
        for name in [work.work_key.clone(), work.work_item_id.to_string()] {
            if by_name
                .insert(name, work.work_item_id)
                .is_some_and(|old| old != work.work_item_id)
            {
                return Err(OutcomeHistoryError::DuplicateWork);
            }
        }
    }
    let known: BTreeMap<Id, &HistoryWork> =
        input.works.iter().map(|w| (w.work_item_id, w)).collect();
    let in_scope: BTreeSet<Id> = match &request.work_keys {
        None => ids.clone(),
        Some(keys) => keys
            .iter()
            .filter_map(|key| by_name.get(key).copied())
            .collect(),
    };
    let unknown_work_keys: Vec<String> = request
        .work_keys
        .iter()
        .flatten()
        .filter(|key| !by_name.contains_key(*key))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut sample = SampleCounts {
        works_in_scope: in_scope.len(),
        ..Default::default()
    };

    // Receipts: once each, as of the instant, for works this project has, in the order they were written.
    sample.receipts.read = input.receipts.len();
    let mut seen: BTreeMap<Id, &HistoryReceipt> = BTreeMap::new();
    let mut used: Vec<&HistoryReceipt> = Vec::new();
    for receipt in &input.receipts {
        if let Some(first) = seen.get(&receipt.event_id) {
            if *first != receipt {
                return Err(OutcomeHistoryError::ConflictingReceipt);
            }
            sample.receipts.duplicates += 1;
            continue;
        }
        seen.insert(receipt.event_id, receipt);
        if receipt.at_ms > request.as_of_ms {
            sample.receipts.after_as_of += 1;
        } else if !known.contains_key(&receipt.work_item_id) {
            sample.receipts.unlocatable += 1;
        } else if !in_scope.contains(&receipt.work_item_id) {
            sample.receipts.outside_filter += 1;
        } else {
            used.push(receipt);
        }
    }
    sample.receipts.used = used.len();
    sample.receipts.without_version = used.iter().filter(|r| r.work_revision.is_none()).count();
    used.sort_by_key(|r| (r.project_revision, r.event_id));
    let mut states: BTreeMap<Id, WorkState> = BTreeMap::new();
    for receipt in used {
        states
            .entry(receipt.work_item_id)
            .or_default()
            .apply(receipt);
    }

    // Executions: as of the instant, an execution that ends later has not ended.
    sample.executions.read = input.executions.len();
    let mut executions: BTreeMap<Id, Vec<&HistoryExecution>> = BTreeMap::new();
    let mut seen_executions = BTreeSet::new();
    for execution in &input.executions {
        if !seen_executions.insert(execution.execution_id) {
            sample.executions.duplicates += 1;
        } else if execution.registered_at_ms > request.as_of_ms {
            sample.executions.after_as_of += 1;
        } else if !known.contains_key(&execution.work_item_id) {
            sample.executions.unlocatable += 1;
        } else if !in_scope.contains(&execution.work_item_id) {
            sample.executions.outside_filter += 1;
        } else {
            sample.executions.used += 1;
            executions
                .entry(execution.work_item_id)
                .or_default()
                .push(execution);
        }
    }

    // Usage: a call once, as of the instant, for a work by its key or its id.
    sample.usage.read = input.usage.len();
    let unique = deduplicate_usage(&project.to_string(), &input.usage)?;
    sample.usage.duplicates = input.usage.len() - unique.len();
    let mut usage: BTreeMap<Id, Vec<UsageReceipt>> = BTreeMap::new();
    for receipt in unique {
        let work = by_name.get(&receipt.attribution.work_id).copied();
        if receipt.occurred_at_ms > request.as_of_ms as u64 {
            sample.usage.after_as_of += 1;
        } else if work.is_none() {
            sample.usage.unlocatable += 1;
        } else if !work.is_some_and(|id| in_scope.contains(&id)) {
            sample.usage.outside_filter += 1;
        } else {
            sample.usage.used += 1;
            usage
                .entry(work.expect("checked"))
                .or_default()
                .push(receipt.clone());
        }
    }

    let sessions: BTreeMap<Id, &HistorySession> =
        input.sessions.iter().map(|s| (s.session_id, s)).collect();

    // Each work goes to the cohort of the session that wrote its latest receipt in force (else of its
    // latest execution); a session without a label leaves it unattributed rather than guessed.
    let mut cohorts: BTreeMap<(bool, CohortKey), CohortAcc> = BTreeMap::new();
    let mut with_outcome = 0;
    let mut with_execution = 0;
    let mut with_usage = 0;
    let mut durable = 0;
    let mut terminal = 0;
    for id in &in_scope {
        let work = known[id];
        let state = states.remove(id).unwrap_or_default();
        let runs = executions.remove(id).unwrap_or_default();
        let session = state.latest_session().or_else(|| {
            runs.iter()
                .max_by_key(|e| (e.registered_at_ms, e.execution_id))
                .map(|e| e.session_id)
        });
        let label = session.and_then(|s| sessions.get(&s).copied());
        if session.is_some() && label.is_none() {
            sample.sessions_unlabelled += 1;
        }
        let key = match (request.cohort_by, label) {
            (CohortBy::None, _) => (
                false,
                CohortKey {
                    provider: None,
                    model: None,
                    agent: None,
                },
            ),
            (_, None) => (
                true,
                CohortKey {
                    provider: None,
                    model: None,
                    agent: None,
                },
            ),
            (CohortBy::ProviderModel, Some(l)) => (
                false,
                CohortKey {
                    provider: Some(l.provider.clone()),
                    model: Some(l.model.clone()),
                    agent: None,
                },
            ),
            (CohortBy::Agent, Some(l)) => (
                false,
                CohortKey {
                    provider: None,
                    model: None,
                    agent: Some(l.agent.clone()),
                },
            ),
        };
        let cohort = cohorts.entry(key).or_default();
        cohort.works += 1;
        let signals = &mut cohort.signals;
        if state.reopened {
            signals.reopened_works += 1;
        }
        signals.withdrawn_completions += state.withdrawn_completions;
        signals.withdrawn_acceptances += state.withdrawn_acceptances;
        if state.conflicting {
            signals.conflicting += 1;
        } else if let Some(completion) = &state.completion {
            let level = completion.level.map_or("not_recorded", level_name);
            *signals
                .completion_by_evidence_level
                .entry(level.into())
                .or_default() += 1;
            match (&completion.digest, &work.acceptance_digest) {
                (Some(bound), Some(current)) if bound == current => {
                    signals.completion_reported += 1
                }
                (Some(_), Some(_)) => signals.completion_under_earlier_standard += 1,
                _ => signals.completion_standard_unknown += 1,
            }
        }
        if let Some(acceptance) = &state.acceptance {
            match acceptance.origin {
                Some(HostEditOrigin::Human) => signals.accepted_by_human += 1,
                Some(HostEditOrigin::AiAccepted) => signals.accepted_by_ai += 1,
                Some(HostEditOrigin::DelegatedAgent) => signals.accepted_by_delegate += 1,
                None => {}
            }
        }
        if state.cancelled.is_some() {
            signals.cancelled += 1;
        }
        if !state.has_outcome() && !state.conflicting {
            signals.open += 1;
        }
        if state.has_outcome() || state.conflicting {
            with_outcome += 1;
        }
        if !runs.is_empty() {
            with_execution += 1;
        }
        for run in runs {
            let ended = run
                .finished_at_ms
                .filter(|finished| *finished <= request.as_of_ms);
            match (run.state, ended) {
                (ExecutionState::Succeeded, Some(_)) => cohort.executions.succeeded += 1,
                (ExecutionState::Failed, Some(_)) => cohort.executions.failed += 1,
                _ => {
                    cohort.executions.unfinished += 1;
                    continue;
                }
            }
            terminal += 1;
            match (run.started_at_ms, ended) {
                (Some(start), Some(end)) if end >= start => {
                    cohort.executions.observed_execution_ms += (end - start) as u64;
                    durable += 1;
                }
                _ => cohort.executions.without_duration += 1,
            }
        }
        if let Some(receipts) = usage.remove(id) {
            cohort.works_with_usage += 1;
            with_usage += 1;
            cohort.usage.extend(receipts);
        }
    }

    let mut out = Vec::new();
    for ((unattributed, key), acc) in cohorts {
        let totals: UsageCostTotals = usage_cost_totals(&project.to_string(), &acc.usage)?;
        out.push(OutcomeCohort {
            key,
            unattributed,
            works: acc.works,
            signals: acc.signals,
            executions: acc.executions,
            cost: CostSummary {
                works_with_usage: acc.works_with_usage,
                works_without_usage: acc.works - acc.works_with_usage,
                actual_micros: totals.actual_micros,
                api_equivalent_micros: totals.api_equivalent_micros,
                unknown_cost_calls: totals.unknown_calls,
                unique_calls: totals.unique_calls,
            },
        });
    }
    let total = in_scope.len();
    let mut history = OutcomeHistory {
        version: OUTCOME_HISTORY_VERSION,
        scope: HistoryScope {
            project_id: project,
            as_of_ms: request.as_of_ms,
            cohort_by: request.cohort_by,
            work_keys: request.work_keys.clone(),
            unknown_work_keys,
        },
        sample,
        cohorts: out,
        coverage: HistoryCoverage {
            works_with_outcome: (with_outcome, total),
            works_with_execution: (with_execution, total),
            works_with_usage: (with_usage, total),
            executions_with_duration: (durable, terminal),
        },
        not_derived: NOT_DERIVED.iter().map(|s| s.to_string()).collect(),
        notes: vec![
            "Counts of receipts as they stand at as_of_ms; a rate needs a denominator and a definition of success that the receipts do not carry.".into(),
            "A work without a completion, a confirmation or a cancellation in force is open or unrecorded, not a failure; works nobody started appear only in works_in_scope.".into(),
            "Cohorts exist only for sessions that wrote a receipt or ran an execution; no cohort is made for a model that did not.".into(),
            "Completion, human confirmation and the exit status of a supervised command are separate signals; none implies another.".into(),
        ],
        history_hash: String::new(),
    };
    let bytes = serde_json::to_vec(&history).expect("history serializes");
    history.history_hash = format!("sha256:{:x}", Sha256::digest(bytes));
    Ok(history)
}
