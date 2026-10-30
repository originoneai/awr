//! Bounded reassessment. A work that deviates from its plan (an objective blocker, a method that
//! keeps failing, an external wait, an outcome nobody knows) is not stopped by the program. The
//! program classifies what the records show, notices the few conditions that deserve a second
//! look and, while one holds, asks for a current attributed decision on that work and on no other.
//! Nothing here opens a task, changes the plan or freezes unrelated work; the store gathers the
//! facts from AWR's own records and persists decisions as events, as it does for admission.
//!
//! What is mechanical stays mechanical: the same command failing in the same directory is the
//! same method, and a success of that method ends a streak of failures. Whether several failures
//! share one root cause is a judgment, so it is only ever attributed by a decider and never
//! inferred from titles or messages.
use crate::admission::{bounded, fingerprint};
use crate::{EffectiveBudget, Error, Id, Result, WorkSize, ensure_public_data};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const REASSESSMENT_EVENT: &str = "work.reassessment";

/// Consecutive failures of one method, with no success of it in between, that call for a second
/// look.
pub const FAILED_METHOD_REPEATS: usize = 3;

const MAX_REFERENCES: usize = 32;

/// What kind of deviation a recorded fact is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviationKind {
    /// The work itself declares a blocker.
    ObjectiveBlocker,
    /// An attempt of a method ended in failure.
    FailedMethod,
    /// The work waits for something outside the agent: a reply, an environment.
    ExternalWait,
    /// An attempt started and nobody can say how it ended.
    UnknownOutcome,
}

/// One classified fact, with the record that carries it so the actual evidence stays reachable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Deviation {
    pub kind: DeviationKind,
    /// The record behind the fact: an execution, an event or the work itself.
    pub reference: String,
    /// When it happened, in milliseconds, as the record says.
    pub at: i64,
    /// `recorded` for AWR's own records, `host_reported` for an unverified host observation.
    pub basis: &'static str,
    /// The method of a failed attempt; the same command in the same directory is one method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub detail: String,
}

/// One attempt of a method, successful or not, so a success can end a streak of failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodAttempt {
    pub method: String,
    pub reference: String,
    /// Where the attempt ended in the journal: the project revision of its last event. It is a
    /// strict order, which a clock in milliseconds is not.
    pub order: u64,
    pub failed: bool,
}

/// A condition that deserves a second look, with the facts behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReassessmentTrigger {
    pub trigger: &'static str,
    pub references: Vec<String>,
    pub detail: String,
}

/// The triggers of one work: the conditions the records show right now.
pub fn triggers(deviations: &[Deviation], attempts: &[MethodAttempt]) -> Vec<ReassessmentTrigger> {
    let mut found = Vec::new();
    let mut methods: Vec<&str> = attempts.iter().map(|a| a.method.as_str()).collect();
    methods.sort_unstable();
    methods.dedup();
    for method in methods {
        let mut own: Vec<&MethodAttempt> = attempts.iter().filter(|a| a.method == method).collect();
        own.sort_by(|a, b| (a.order, &a.reference).cmp(&(b.order, &b.reference)));
        let streak: Vec<&MethodAttempt> =
            own.iter().rev().take_while(|a| a.failed).copied().collect();
        if streak.len() >= FAILED_METHOD_REPEATS {
            let mut references: Vec<String> = streak.iter().map(|a| a.reference.clone()).collect();
            references.sort();
            found.push(ReassessmentTrigger {
                trigger: "repeated_failed_method",
                detail: format!(
                    "the same method failed {} times in a row without a success in between ({})",
                    streak.len(),
                    references.join(", ")
                ),
                references,
            });
        }
    }
    let mut unknown: Vec<String> = deviations
        .iter()
        .filter(|d| d.kind == DeviationKind::UnknownOutcome)
        .map(|d| d.reference.clone())
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        found.push(ReassessmentTrigger {
            trigger: "unknown_outcome_unresolved",
            detail: format!(
                "no one can say how {} ended; query the outcome before any retry",
                unknown.join(", ")
            ),
            references: unknown,
        });
    }
    found
}

/// One line of a budget: what was used against what is allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetLine {
    pub used: u64,
    pub allowed: u64,
    /// `recorded` when AWR counted it from its own records, `host_reported` when only an
    /// unverified host observation could say.
    pub counted_from: &'static str,
    /// `declared` or `size_default`.
    pub allowance: &'static str,
}

impl BudgetLine {
    /// The budget is spent once the work needs more than it allows: a fourth rework cycle under
    /// a budget of three.
    pub fn exhausted(&self) -> bool {
        self.used > self.allowed
    }
}

/// What the work has used of its budgets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetUse {
    pub size: WorkSize,
    pub size_basis: &'static str,
    /// Cycles in which the work was returned.
    pub rework: BudgetLine,
    /// Minutes under a claim, which can only overstate the effort that was active.
    pub effort_minutes: BudgetLine,
}

/// Counting from AWR's own records before the host's counter: the host's number is used only
/// when AWR has recorded nothing of its own.
pub fn count_with_fallback(own: u64, host: Option<u64>) -> (u64, &'static str) {
    match host {
        Some(host) if own == 0 => (host, "host_reported"),
        _ => (own, "recorded"),
    }
}

impl BudgetUse {
    pub fn of(
        budget: &EffectiveBudget,
        rework: (u64, &'static str),
        effort_minutes: (u64, &'static str),
    ) -> Self {
        Self {
            size: budget.size,
            size_basis: budget.size_basis,
            rework: BudgetLine {
                used: rework.0,
                allowed: u64::from(budget.rework_cycles),
                counted_from: rework.1,
                allowance: budget.rework_basis,
            },
            effort_minutes: BudgetLine {
                used: effort_minutes.0,
                allowed: budget.active_effort_minutes,
                counted_from: effort_minutes.1,
                allowance: budget.effort_basis,
            },
        }
    }

    /// The trigger a spent budget is. Rework is bound by the exact count, so a further cycle
    /// needs a new decision; effort grows by the minute and is bound by the allowance it is over.
    pub fn trigger(&self) -> Option<ReassessmentTrigger> {
        let mut references = Vec::new();
        let mut spent = Vec::new();
        if self.rework.exhausted() {
            references.push(format!(
                "budget:rework:{}>{}",
                self.rework.used, self.rework.allowed
            ));
            spent.push(format!(
                "{} rework cycles against a budget of {} ({})",
                self.rework.used, self.rework.allowed, self.rework.counted_from
            ));
        }
        if self.effort_minutes.exhausted() {
            references.push(format!(
                "budget:effort:over:{}",
                self.effort_minutes.allowed
            ));
            spent.push(format!(
                "{} minutes of effort against a budget of {} ({})",
                self.effort_minutes.used,
                self.effort_minutes.allowed,
                self.effort_minutes.counted_from
            ));
        }
        if references.is_empty() {
            return None;
        }
        Some(ReassessmentTrigger {
            trigger: "budget_exhausted",
            detail: format!("the budget is spent: {}", spent.join("; ")),
            references,
        })
    }
}

/// What a decision answered: the facts that triggered a second look, by reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReassessmentBinding {
    pub facts_fingerprint: String,
    pub references: Vec<String>,
}

impl ReassessmentBinding {
    /// The facts behind a set of triggers, or nothing while no trigger holds.
    pub fn of(triggers: &[ReassessmentTrigger]) -> Option<Self> {
        let references: BTreeSet<&String> =
            triggers.iter().flat_map(|t| t.references.iter()).collect();
        if references.is_empty() {
            return None;
        }
        let references: Vec<String> = references.into_iter().cloned().collect();
        let digest = Sha256::digest(
            serde_json::to_vec(&references).expect("a list of strings always serializes"),
        );
        Some(Self {
            facts_fingerprint: format!("sha256:{digest:x}"),
            references,
        })
    }

    /// The facts that appeared or disappeared since this binding was decided.
    pub fn changed_from(&self, current: &Self) -> Vec<String> {
        let mut changed = Vec::new();
        for reference in &current.references {
            if !self.references.contains(reference) {
                changed.push(format!("fact_added:{reference}"));
            }
        }
        for reference in &self.references {
            if !current.references.contains(reference) {
                changed.push(format!("fact_removed:{reference}"));
            }
        }
        changed
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.references.iter().any(|r| other.references.contains(r))
    }
}

/// What the decider concluded the facts call for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReassessmentChoice {
    /// The facts were looked at and the work goes on as it is.
    Continue,
    /// The work waits for something outside it.
    Wait,
    /// Existing work or an existing result is used instead of a new attempt.
    Reuse,
    /// The method changes; the plan and the intent stay.
    AlterMethod,
    /// The work is split; the split itself is decided where splits are decided.
    Split,
    /// The plan is replaced; the replacement is made where plans are made.
    Replan,
    /// A spent budget: the work is delivered as it is, with the known issues recorded.
    DeliverWithKnownIssues,
    /// A spent budget: the scope shrinks to what the budget can pay for.
    ReduceScope,
    /// A spent budget: the decision is left to someone else, and the work stays held until
    /// that someone decides.
    Escalate,
}

impl ReassessmentChoice {
    /// Whether the choice can answer facts that trigger a second look.
    pub fn answers_facts(self) -> bool {
        matches!(
            self,
            Self::Continue
                | Self::Wait
                | Self::Reuse
                | Self::AlterMethod
                | Self::Split
                | Self::Replan
        )
    }

    /// Whether the choice can answer a spent budget. `continue` cannot: the budget is spent.
    pub fn answers_budget(self) -> bool {
        matches!(
            self,
            Self::DeliverWithKnownIssues | Self::ReduceScope | Self::Replan | Self::Escalate
        )
    }
}

/// What a decision answers: facts that trigger a second look, or a budget that is spent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReassessmentSubject {
    #[default]
    Facts,
    Budget,
}

/// The attributed judgment behind a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReassessmentJudgment {
    /// Why this choice answers the facts.
    pub reason: String,
    /// What happens next, concretely.
    pub next_step: String,
    /// The actual evidence that was read, such as outputs, logs or reports.
    pub evidence_considered: String,
}

impl ReassessmentJudgment {
    pub fn validate(&self) -> Result<()> {
        if [&self.reason, &self.next_step, &self.evidence_considered]
            .into_iter()
            .any(|text| !bounded(text))
        {
            return Err(Error::InvalidInput(
                "a reassessment judgment needs bounded reason, next_step and evidence_considered"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// An attributed grouping of facts under one root cause. The program only checks that the facts
/// belong to the work; it never groups on its own by titles or messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootCause {
    pub label: String,
    pub references: Vec<String>,
}

impl RootCause {
    pub fn validate(&self) -> Result<()> {
        let distinct: BTreeSet<&String> = self.references.iter().collect();
        if !bounded(&self.label)
            || self.label.len() > 256
            || self.references.is_empty()
            || self.references.len() > MAX_REFERENCES
            || distinct.len() != self.references.len()
            || self
                .references
                .iter()
                .any(|r| r.trim().is_empty() || r.len() > 512 || r.chars().any(char::is_control))
        {
            return Err(Error::InvalidInput(
                "a root cause needs a bounded label and between 1 and 32 distinct references"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// One request to record a reassessment decision for a work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReassessmentRequest {
    pub work: String,
    /// An active session, or a declaring agent when no session is given (attribution only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub expected_revision: u64,
    /// Stable idempotency key: the same key with the same content returns the original receipt.
    pub request_key: String,
    pub decision: ReassessmentChoice,
    /// What the decision answers; facts unless it says the budget.
    #[serde(default)]
    pub answers: ReassessmentSubject,
    /// The fingerprint the decider reviewed, from `work show` of the work: the facts, or for a
    /// budget the one in `reassessment.budget.current`.
    pub facts_fingerprint: String,
    pub judgment: ReassessmentJudgment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_cause: Option<RootCause>,
    /// The issues a delivery goes out with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_issues: Vec<String>,
    /// What the scope is reduced to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduced_scope: Option<String>,
    /// Who the decision is left to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalated_to: Option<String>,
}

impl ReassessmentRequest {
    /// What a request key binds: the request without its revision guard and the key itself.
    pub fn input(&self) -> serde_json::Value {
        let mut input = serde_json::json!({
            "work": self.work, "session": self.session, "agent": self.agent,
            "decision": self.decision, "facts_fingerprint": self.facts_fingerprint,
            "judgment": self.judgment, "root_cause": self.root_cause,
        });
        // What only a budget decision carries is left out otherwise, so a request about facts
        // keeps the content it always had.
        if self.answers == ReassessmentSubject::Budget {
            input["answers"] = serde_json::json!(self.answers);
        }
        if !self.known_issues.is_empty() {
            input["known_issues"] = serde_json::json!(self.known_issues);
        }
        if let Some(scope) = &self.reduced_scope {
            input["reduced_scope"] = serde_json::json!(scope);
        }
        if let Some(to) = &self.escalated_to {
            input["escalated_to"] = serde_json::json!(to);
        }
        input
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |why: &str| Err(Error::InvalidInput(format!("reassessment request {why}")));
        if self.request_key.trim().is_empty()
            || self.request_key.len() > 512
            || self.request_key.chars().any(char::is_control)
        {
            return invalid("requires a bounded stable request_key");
        }
        if self.work.trim().is_empty() || self.work.len() > 512 {
            return invalid("requires the work key");
        }
        match (&self.session, &self.agent) {
            (Some(_), None) => {}
            (None, Some(agent))
                if !agent.trim().is_empty()
                    && agent.len() <= 256
                    && agent == agent.trim()
                    && !agent.chars().any(char::is_control) => {}
            _ => {
                return invalid(
                    "names exactly one of an active session or a bounded declaring agent",
                );
            }
        }
        if !fingerprint(&self.facts_fingerprint) {
            return invalid("requires the reviewed facts_fingerprint (sha256:...)");
        }
        self.judgment.validate()?;
        if let Some(cause) = &self.root_cause {
            cause.validate()?;
        }
        let fits = match self.answers {
            ReassessmentSubject::Facts => self.decision.answers_facts(),
            ReassessmentSubject::Budget => self.decision.answers_budget(),
        };
        if !fits {
            return invalid(
                "chooses something that does not answer what it says it answers: facts are answered by continue, wait, reuse, alter_method, split or replan, a spent budget by deliver_with_known_issues, reduce_scope, replan or escalate",
            );
        }
        let issues_ok =
            self.known_issues.len() <= 16 && self.known_issues.iter().all(|i| bounded(i));
        let scope_ok = self.reduced_scope.as_deref().is_none_or(bounded);
        let to_ok = self.escalated_to.as_deref().is_none_or(|to| {
            !to.trim().is_empty()
                && to.len() <= 256
                && to == to.trim()
                && !to.chars().any(char::is_control)
        });
        if !(issues_ok && scope_ok && to_ok) {
            return invalid("has unbounded known issues, reduced scope or escalation target");
        }
        let (issues, scope, to) = (
            !self.known_issues.is_empty(),
            self.reduced_scope.is_some(),
            self.escalated_to.is_some(),
        );
        let exact = match self.decision {
            ReassessmentChoice::DeliverWithKnownIssues => {
                (issues, scope, to) == (true, false, false)
            }
            ReassessmentChoice::ReduceScope => (issues, scope, to) == (false, true, false),
            ReassessmentChoice::Escalate => (issues, scope, to) == (false, false, true),
            _ => (issues, scope, to) == (false, false, false),
        };
        if !exact {
            return invalid(
                "carries what its choice needs and nothing else: known issues to deliver with, the scope it is reduced to, or who it is escalated to",
            );
        }
        ensure_public_data(self)
    }
}

/// The latest reassessment event of a work as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedReassessment {
    pub event: Id,
    pub decision: ReassessmentChoice,
    pub subject: ReassessmentSubject,
    pub bound: ReassessmentBinding,
    pub decider_agent: String,
    pub root_cause: Option<RootCause>,
    pub escalated_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReassessmentDiagnostic {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReassessmentStatus {
    pub work: String,
    /// clear, required, current or stale.
    pub state: &'static str,
    pub diagnostics: Vec<ReassessmentDiagnostic>,
    pub triggers: Vec<ReassessmentTrigger>,
    /// Every classified fact, whether or not it triggers anything.
    pub deviations: Vec<Deviation>,
    /// Host observations that are older than the records they would speak about.
    pub stale_observations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<ReassessmentBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_event: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetStatus>,
    pub basis: &'static str,
}

/// The budgets of a work and whether they call for a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetStatus {
    /// within, required, current, stale or escalated.
    pub state: &'static str,
    #[serde(rename = "use")]
    pub usage: BudgetUse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<ReassessmentTrigger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<ReassessmentBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_event: Option<Id>,
}

const BASIS: &str = "The program classifies the records of the work and notices repeated failures of one method and outcomes nobody knows. While one holds, it asks for a current decision on this work only; it opens no task and holds no other work. Whether failures share a root cause stays an attributed judgment.";

/// Everything the evaluator may read; the store fills it from the work's own records.
pub struct ReassessmentInputs<'a> {
    pub work: &'a str,
    pub deviations: &'a [Deviation],
    pub attempts: &'a [MethodAttempt],
    pub stale_observations: &'a [String],
    /// The latest decision that answered facts.
    pub latest: Option<&'a RecordedReassessment>,
    /// What the work has used of its budgets, when it is known.
    pub budget: Option<&'a BudgetUse>,
    /// The latest decision that answered the budget.
    pub latest_budget: Option<&'a RecordedReassessment>,
}

/// Whether two budget bindings speak about the same budget, rework or effort, whatever the
/// count was when each was made.
fn shares_a_budget(a: &ReassessmentBinding, b: &ReassessmentBinding) -> bool {
    let budget = |reference: &String| {
        reference
            .splitn(3, ':')
            .take(2)
            .collect::<Vec<_>>()
            .join(":")
    };
    a.references
        .iter()
        .any(|x| b.references.iter().any(|y| budget(x) == budget(y)))
}

pub fn evaluate_reassessment(inputs: &ReassessmentInputs<'_>) -> ReassessmentStatus {
    let found = triggers(inputs.deviations, inputs.attempts);
    let current = ReassessmentBinding::of(&found);
    let mut diagnostics = Vec::new();
    let summary = || {
        found
            .iter()
            .map(|t| format!("{}: {}", t.trigger, t.detail))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let state = match (&current, inputs.latest) {
        (None, _) => "clear",
        (Some(_), None) => {
            diagnostics.push(ReassessmentDiagnostic {
                code: "reassessment_required",
                detail: format!("{}; no reassessment is recorded for these facts", summary()),
            });
            "required"
        }
        (Some(now), Some(latest)) if latest.bound.facts_fingerprint == now.facts_fingerprint => {
            "current"
        }
        (Some(now), Some(latest)) if !latest.bound.overlaps(now) => {
            diagnostics.push(ReassessmentDiagnostic {
                code: "reassessment_required",
                detail: format!(
                    "{}; the latest reassessment answered other facts",
                    summary()
                ),
            });
            "required"
        }
        (Some(now), Some(latest)) => {
            diagnostics.push(ReassessmentDiagnostic {
                code: "reassessment_stale",
                detail: format!(
                    "{}; the latest reassessment no longer matches the facts ({}); decide again",
                    summary(),
                    latest.bound.changed_from(now).join(", ")
                ),
            });
            "stale"
        }
    };
    let budget = inputs.budget.map(|usage| {
        let trigger = usage.trigger();
        let current = trigger
            .as_ref()
            .and_then(|t| ReassessmentBinding::of(std::slice::from_ref(t)));
        let state = match (&current, inputs.latest_budget) {
            (None, _) => "within",
            (Some(_), None) => {
                diagnostics.push(ReassessmentDiagnostic {
                    code: "reassessment_budget_required",
                    detail: format!(
                        "{}; deliver with known issues, reduce the scope, replan or escalate",
                        trigger.as_ref().map_or("", |t| t.detail.as_str())
                    ),
                });
                "required"
            }
            (Some(now), Some(latest))
                if latest.bound.facts_fingerprint == now.facts_fingerprint =>
            {
                if latest.decision == ReassessmentChoice::Escalate {
                    diagnostics.push(ReassessmentDiagnostic {
                        code: "reassessment_budget_escalated",
                        detail: format!(
                            "the spent budget was escalated to {}; someone else has to decide",
                            latest.escalated_to.as_deref().unwrap_or("someone else")
                        ),
                    });
                    "escalated"
                } else {
                    "current"
                }
            }
            (Some(now), Some(latest)) if !shares_a_budget(&latest.bound, now) => {
                diagnostics.push(ReassessmentDiagnostic {
                    code: "reassessment_budget_required",
                    detail: format!(
                        "{}; the latest decision answered another budget",
                        trigger.as_ref().map_or("", |t| t.detail.as_str())
                    ),
                });
                "required"
            }
            (Some(now), Some(latest)) => {
                diagnostics.push(ReassessmentDiagnostic {
                    code: "reassessment_budget_stale",
                    detail: format!(
                        "{}; the latest decision no longer matches the budget ({}); decide again",
                        trigger.as_ref().map_or("", |t| t.detail.as_str()),
                        latest.bound.changed_from(now).join(", ")
                    ),
                });
                "stale"
            }
        };
        BudgetStatus {
            state,
            usage: usage.clone(),
            trigger,
            current,
            decision_event: inputs.latest_budget.map(|l| l.event),
        }
    });
    ReassessmentStatus {
        work: inputs.work.into(),
        state,
        diagnostics,
        triggers: found,
        deviations: inputs.deviations.to_vec(),
        stale_observations: inputs.stale_observations.to_vec(),
        current,
        decision_event: inputs.latest.map(|l| l.event),
        budget,
        basis: BASIS,
    }
}
