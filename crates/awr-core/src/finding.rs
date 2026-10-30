//! Findings of a review. A finding says something is wrong with a work; whether it sends the work
//! back is not up to how loudly it is put. Its severity is derived from what it is measured
//! against: the acceptance tiers adopted for the work and the hard rules of the project. Only a
//! finding that breaks a required criterion or a hard rule, and shows it with a counterexample
//! someone can reproduce, returns the work; a security or data-loss risk that is stated with its
//! reasoning does too. Everything else is kept as a deferred record, linked to the work and to
//! what it was measured against, which blocks nothing.
//!
//! This module holds the request, the pure rule that derives the severity, the fold of recorded
//! finding events and the count of rework cycles. The store gathers the facts (the tier of a
//! criterion, whether a rule is hard) and persists the events.
use crate::admission::bounded;
use crate::{DeliveryTier, Error, Id, Result, ensure_public_data};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const FINDING_EVENT: &str = "work.finding";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingAction {
    Record,
    Resolve,
}

/// A harm that matters whatever the tier of the criterion it comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskKind {
    Security,
    DataLoss,
}

/// What to do, so that someone else can see the finding for themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counterexample {
    pub given: String,
    pub when: String,
    pub then: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Risk {
    pub kind: RiskKind,
    pub reasoning: String,
}

/// What a finding request says apart from who sends it; this is what an event keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingInput {
    pub work: String,
    pub finding_key: String,
    pub action: Option<FindingAction>,
    /// The review round the finding comes from, counted from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The acceptance criterion of the work it is measured against, word for word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<String>,
    /// The hard rule of the project it is measured against, by key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<Counterexample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<Risk>,
    /// How a finding was dealt with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

/// One request to record a finding, or to close one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingRequest {
    pub work: String,
    /// An active session, or a declaring agent when no session is given (attribution only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub expected_revision: u64,
    /// Stable idempotency key: the same key with the same content returns the original receipt.
    pub request_key: String,
    /// Names the finding within the work and branch.
    pub finding_key: String,
    pub action: FindingAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<Counterexample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<Risk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

fn name_ok(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= 256
        && text == text.trim()
        && !text.chars().any(char::is_control)
}

impl FindingRequest {
    /// What a request key binds: the request without its revision guard and the key itself.
    pub fn input(&self) -> FindingInput {
        FindingInput {
            work: self.work.clone(),
            finding_key: self.finding_key.clone(),
            action: Some(self.action),
            round: self.round,
            summary: self.summary.clone(),
            criterion: self.criterion.clone(),
            rule: self.rule.clone(),
            counterexample: self.counterexample.clone(),
            risk: self.risk.clone(),
            resolution: self.resolution.clone(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |why: &str| Err(Error::InvalidInput(format!("finding request {why}")));
        if self.request_key.trim().is_empty()
            || self.request_key.len() > 512
            || self.request_key.chars().any(char::is_control)
        {
            return invalid("requires a bounded stable request_key");
        }
        if self.work.trim().is_empty() || self.work.len() > 512 {
            return invalid("requires the work key");
        }
        if !name_ok(&self.finding_key) {
            return invalid(
                "requires a bounded finding_key that names the finding within the work",
            );
        }
        match (&self.session, &self.agent) {
            (Some(_), None) => {}
            (None, Some(agent)) if name_ok(agent) => {}
            _ => {
                return invalid(
                    "names exactly one of an active session or a bounded declaring agent",
                );
            }
        }
        let text = |value: &Option<String>| value.as_deref().is_some_and(bounded);
        match self.action {
            FindingAction::Record => {
                if self.round.is_none_or(|r| r == 0 || r > 1000) || !text(&self.summary) {
                    return invalid(
                        "to record a finding needs its review round, counted from 1, and a summary",
                    );
                }
                if self.resolution.is_some() {
                    return invalid("to record a finding says nothing about how it was dealt with");
                }
                if self.criterion.as_deref().is_some_and(|c| !bounded(c))
                    || self.rule.as_deref().is_some_and(|r| !name_ok(r))
                {
                    return invalid("has an unbounded criterion or rule");
                }
                if let Some(example) = &self.counterexample
                    && !(bounded(&example.given)
                        && bounded(&example.when)
                        && bounded(&example.then))
                {
                    return invalid("needs a counterexample with given, when and then");
                }
                if let Some(risk) = &self.risk
                    && !bounded(&risk.reasoning)
                {
                    return invalid("needs the reasoning behind a stated risk");
                }
            }
            FindingAction::Resolve => {
                if !text(&self.resolution) {
                    return invalid("to resolve a finding needs how it was dealt with");
                }
                if self.round.is_some()
                    || self.summary.is_some()
                    || self.criterion.is_some()
                    || self.rule.is_some()
                    || self.counterexample.is_some()
                    || self.risk.is_some()
                {
                    return invalid("to resolve a finding restates nothing about it");
                }
            }
        }
        ensure_public_data(self)
    }
}

/// What the store established about what a finding is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindingFacts {
    /// The tier adopted for the criterion the finding names, when it names one.
    pub criterion_tier: Option<DeliveryTier>,
    /// Whether the rule the finding names is a hard rule of the project, when it names one.
    pub rule_is_hard: Option<bool>,
}

/// How a finding was judged: blocking, or deferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingAssessment {
    /// `blocking` or `non_blocking`.
    pub severity: String,
    /// Why, in the terms of the rule.
    pub basis: String,
}

/// The rule that makes severity a consequence of the record instead of a claim of whoever raises
/// the finding. A security or data-loss risk that is stated with its reasoning blocks. Otherwise a
/// finding blocks only when it breaks a required criterion or a hard rule and shows it with a
/// counterexample; without one it does not block, and neither does one against a later criterion,
/// against nothing the project adopted, or a matter of style.
pub fn assess_finding(input: &FindingInput, facts: &FindingFacts) -> FindingAssessment {
    let blocking = |basis: String| FindingAssessment {
        severity: "blocking".into(),
        basis,
    };
    if let Some(risk) = &input.risk {
        let kind = match risk.kind {
            RiskKind::Security => "security",
            RiskKind::DataLoss => "data-loss",
        };
        return blocking(format!(
            "it states a {kind} risk with its reasoning, which blocks whatever tier the criterion has"
        ));
    }
    let required = facts.criterion_tier == Some(DeliveryTier::Required);
    let hard = facts.rule_is_hard == Some(true);
    let against = match (required, hard) {
        (true, true) => Some("a required criterion and a hard rule"),
        (true, false) => Some("a required criterion"),
        (false, true) => Some("a hard rule"),
        (false, false) => None,
    };
    match (against, input.counterexample.is_some()) {
        (Some(against), true) => blocking(format!(
            "it breaks {against} and shows it with a counterexample that can be reproduced"
        )),
        (Some(against), false) => FindingAssessment {
            severity: "non_blocking".into(),
            basis: format!(
                "it names {against} but has no reproducible counterexample, and states no security or data-loss risk"
            ),
        },
        (None, _) => FindingAssessment {
            severity: "non_blocking".into(),
            basis: if facts.criterion_tier == Some(DeliveryTier::Later) {
                "the criterion it names is later-tier, and it names no hard rule".into()
            } else {
                "it names neither a required criterion nor a hard rule, so it is a matter of taste or style"
                    .into()
            },
        },
    }
}

/// A finding as the recorded events leave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub key: String,
    pub round: u32,
    pub summary: String,
    /// `blocking` or `non_blocking`, as derived when it was recorded.
    pub severity: String,
    pub basis: String,
    /// `returns_work` for a blocking finding, `deferred` for the rest.
    pub disposition: &'static str,
    /// open or resolved.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criterion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<Counterexample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<Risk>,
    pub recorded_event: Id,
    pub recorded_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_event: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
}

/// One recorded finding event, in journal order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedFinding {
    pub event: Id,
    pub decider_agent: String,
    pub input: FindingInput,
    pub assessment: Option<FindingAssessment>,
}

/// Folds the recorded events of a work into its findings, in the order they were recorded.
/// History is never rewritten: a resolved finding stays, with how it was dealt with.
pub fn fold_findings(records: &[RecordedFinding]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    for record in records {
        let input = &record.input;
        match input.action {
            Some(FindingAction::Record) => {
                if findings.iter().any(|f| f.key == input.finding_key) {
                    continue;
                }
                let (Some(round), Some(summary), Some(assessment)) = (
                    input.round,
                    input.summary.clone(),
                    record.assessment.clone(),
                ) else {
                    continue;
                };
                let blocking = assessment.severity == "blocking";
                findings.push(Finding {
                    key: input.finding_key.clone(),
                    round,
                    summary,
                    severity: assessment.severity,
                    basis: assessment.basis,
                    disposition: if blocking { "returns_work" } else { "deferred" },
                    state: "open",
                    criterion: input.criterion.clone(),
                    rule: input.rule.clone(),
                    counterexample: input.counterexample.clone(),
                    risk: input.risk.clone(),
                    recorded_event: record.event,
                    recorded_by: record.decider_agent.clone(),
                    resolution: None,
                    resolved_event: None,
                    resolved_by: None,
                });
            }
            Some(FindingAction::Resolve) => {
                if let Some(finding) = findings
                    .iter_mut()
                    .find(|f| f.key == input.finding_key && f.state == "open")
                {
                    finding.state = "resolved";
                    finding.resolution = input.resolution.clone();
                    finding.resolved_event = Some(record.event);
                    finding.resolved_by = Some(record.decider_agent.clone());
                }
            }
            None => {}
        }
    }
    findings
}

/// The blocking findings that still hold the work.
pub fn open_blocking(findings: &[Finding]) -> Vec<&Finding> {
    findings
        .iter()
        .filter(|f| f.state == "open" && f.severity == "blocking")
        .collect()
}

/// A record that bears on how often a work has been sent back, in journal order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReworkRecord {
    /// The work was completed.
    Completed,
    /// The work was reopened: it was sent back after completion.
    Reopened,
    /// A blocking finding of a review round was recorded.
    BlockingFinding { round: u32 },
}

/// The rework cycles AWR can count from its own records. A work is sent back when it is reopened
/// and when a review round finds blocking issues in work that is not completed. A blocking finding
/// against completed work is answered by the reopening that follows, which is what is counted, so
/// one return is never counted twice.
pub fn rework_cycles(records: &[ReworkRecord]) -> u64 {
    let mut completed = false;
    let mut reopened = 0u64;
    let mut rounds = BTreeSet::new();
    for record in records {
        match record {
            ReworkRecord::Completed => completed = true,
            ReworkRecord::Reopened => {
                completed = false;
                reopened += 1;
            }
            ReworkRecord::BlockingFinding { round } if !completed => {
                rounds.insert(*round);
            }
            ReworkRecord::BlockingFinding { .. } => {}
        }
    }
    reopened + rounds.len() as u64
}

const BASIS: &str = "A finding's severity is derived from what it is measured against, not from how it is put: the tier adopted for the criterion it names, the hard rules of the project, a counterexample someone can reproduce, or a stated security or data-loss risk. Only blocking findings return the work; the rest are deferred records that block nothing.";

/// The findings of one work, with what they hold and what they leave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindingStatus {
    pub work: String,
    pub findings: Vec<Finding>,
    /// Keys of the blocking findings that still hold the work.
    pub open_blocking: Vec<String>,
    /// Keys of the deferred records that are still open; they hold nothing.
    pub open_deferred: Vec<String>,
    pub basis: &'static str,
}

impl FindingStatus {
    pub fn of(work: &str, findings: Vec<Finding>) -> Self {
        Self {
            work: work.into(),
            open_blocking: open_blocking(&findings)
                .iter()
                .map(|f| f.key.clone())
                .collect(),
            open_deferred: findings
                .iter()
                .filter(|f| f.state == "open" && f.severity != "blocking")
                .map(|f| f.key.clone())
                .collect(),
            findings,
            basis: BASIS,
        }
    }
}
