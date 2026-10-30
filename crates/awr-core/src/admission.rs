//! Governed plan admission. A source may declare a policy under which work needs a recorded,
//! approved admission decision before it can be claimed or executed. A decision binds the exact
//! scope it judged: the work contract, the declared definition, the reviewed examples and the
//! policy. This module holds the types, the request validation and a pure evaluator; the store
//! gathers the facts and persists decisions as events.
//!
//! What the program checks is provenance and currency: that a decision exists, is approved, and
//! still describes the current scope, that obligations of the declared purpose are present, and
//! that an independent review came from another session. It never judges whether the reasoning
//! behind a decision is sound.
use crate::{DefinitionReport, DeliveryTier, Error, Id, Result, WorkPurpose, ensure_public_data};
use serde::{Deserialize, Serialize};

pub const ADMISSION_POLICY_VERSION: u32 = 1;
pub const ADMISSION_EVENT: &str = "work.admission";
const MAX_TEXT: usize = 2048;
const MAX_TAGS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionMode {
    /// Report the admission state; never block.
    LegacyObserve,
    /// Claims and executions of governed work need a current approved decision.
    GovernedEnforce,
}

/// Which work of the declaring source is governed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionScope {
    All,
    Tags(Vec<String>),
}

/// The policy a ledger source declares under its `admission_policy` option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionPolicy {
    pub version: u32,
    pub mode: AdmissionMode,
    /// Must be explicit; a missing scope governs nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<AdmissionScope>,
    /// Work carrying one of these tags needs a decision from another session than the proposer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub independent_review_tags: Vec<String>,
    /// Agents allowed to approve a containment decision; empty means any attributed agent may.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supervisors: Vec<String>,
}

fn tag_ok(tag: &str) -> bool {
    !tag.trim().is_empty()
        && tag.len() <= 128
        && tag == tag.trim()
        && !tag.chars().any(char::is_control)
}

impl AdmissionPolicy {
    pub fn validate(&self) -> Result<()> {
        let invalid = |why: &str| Error::InvalidInput(format!("admission policy {why}"));
        if self.version != ADMISSION_POLICY_VERSION {
            return Err(invalid("must declare version 1"));
        }
        if self.mode == AdmissionMode::GovernedEnforce && self.scope.is_none() {
            return Err(invalid("governed_enforce requires an explicit scope"));
        }
        let scoped_tags = match &self.scope {
            Some(AdmissionScope::Tags(tags)) => tags.as_slice(),
            _ => &[],
        };
        if scoped_tags.is_empty() && matches!(self.scope, Some(AdmissionScope::Tags(_))) {
            return Err(invalid("a tag scope needs at least one tag"));
        }
        for tags in [scoped_tags, self.independent_review_tags.as_slice()] {
            if tags.len() > MAX_TAGS || tags.iter().any(|tag| !tag_ok(tag)) {
                return Err(invalid("tags must be nonblank, bounded and at most 32"));
            }
        }
        if self.supervisors.len() > MAX_TAGS || self.supervisors.iter().any(|agent| !tag_ok(agent))
        {
            return Err(invalid(
                "supervisors must be nonblank, bounded agent names and at most 32",
            ));
        }
        Ok(())
    }
    pub fn enforced(&self) -> bool {
        self.mode == AdmissionMode::GovernedEnforce
    }
    /// Whether the policy covers work with these tags.
    pub fn governs(&self, tags: &[String]) -> bool {
        match &self.scope {
            Some(AdmissionScope::All) => true,
            Some(AdmissionScope::Tags(scoped)) => scoped.iter().any(|tag| tags.contains(tag)),
            None => false,
        }
    }
    pub fn requires_independent_review(&self, tags: &[String]) -> bool {
        self.independent_review_tags
            .iter()
            .any(|tag| tags.contains(tag))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionDecision {
    /// A request for a decision; it is not a decision.
    Proposed,
    Approved,
    Denied,
}

/// The attributed judgment a proposal or an inline approval records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionJudgment {
    pub contribution: String,
    pub necessity: String,
    pub alternatives: String,
    pub deliverable: String,
    pub exit: String,
    pub responsibility: String,
}

pub(crate) fn bounded(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= MAX_TEXT
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}

pub(crate) fn fingerprint(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|c| c.is_ascii_hexdigit()))
}

impl AdmissionJudgment {
    pub fn validate(&self) -> Result<()> {
        if [
            &self.contribution,
            &self.necessity,
            &self.alternatives,
            &self.deliverable,
            &self.exit,
            &self.responsibility,
        ]
        .into_iter()
        .any(|text| !bounded(text))
        {
            return Err(Error::InvalidInput(
                "an admission judgment needs bounded contribution, necessity, alternatives, deliverable, exit and responsibility".into(),
            ));
        }
        Ok(())
    }
}

/// One request to record an admission event for a work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionRequest {
    pub work: String,
    /// An active session. It is required to propose or decide for work that needs independent
    /// review, because only sessions can be shown to differ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Id>,
    /// The declaring agent when no session is given. It is attribution, not a verified identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub expected_revision: u64,
    /// Stable idempotency key: the same key with the same content returns the original receipt.
    pub request_key: String,
    pub decision: AdmissionDecision,
    /// The contract fingerprint the decider reviewed, from `work prepare` or `work assess`.
    pub contract_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judgment: Option<AdmissionJudgment>,
    /// The `proposed` event an approval or denial answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_event: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AdmissionRequest {
    /// What a request key binds: the request without its revision guard and the key itself.
    pub fn input(&self) -> serde_json::Value {
        serde_json::json!({
            "work": self.work, "session": self.session, "agent": self.agent,
            "contract_fingerprint": self.contract_fingerprint,
            "decision": self.decision, "judgment": self.judgment,
            "proposal_event": self.proposal_event, "reason": self.reason,
        })
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |why: &str| Err(Error::InvalidInput(format!("admission request {why}")));
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
        if !fingerprint(&self.contract_fingerprint) {
            return invalid("requires the reviewed contract_fingerprint (sha256:...)");
        }
        if self
            .reason
            .as_deref()
            .is_some_and(|reason| !bounded(reason))
        {
            return invalid("has an unbounded or blank reason");
        }
        match (
            self.decision,
            self.judgment.as_ref(),
            self.proposal_event,
            self.reason.as_ref(),
        ) {
            (AdmissionDecision::Proposed, Some(_), None, _) => {}
            (AdmissionDecision::Proposed, ..) => {
                return invalid("proposals need a judgment and no proposal_event");
            }
            (AdmissionDecision::Approved, Some(_), None, _) => {}
            (AdmissionDecision::Approved, None, Some(_), _) => {}
            (AdmissionDecision::Approved, ..) => {
                return invalid(
                    "approvals need either a judgment (inline) or the proposal_event they answer, not both",
                );
            }
            (AdmissionDecision::Denied, None, _, Some(_)) => {}
            (AdmissionDecision::Denied, ..) => {
                return invalid("denials need a reason and no judgment");
            }
        }
        if let Some(judgment) = &self.judgment {
            judgment.validate()?;
        }
        ensure_public_data(self)
    }
}

/// The exact scope a decision binds, or the scope as it currently stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionBinding {
    pub contract_fingerprint: String,
    /// The declared definition; none when the work declares none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_fingerprint: Option<String>,
    /// The declared examples and discrimination method, bound as one reviewed set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples_fingerprint: Option<String>,
    /// The effective policy of the owning source.
    pub policy_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<WorkPurpose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<DeliveryTier>,
}

impl AdmissionBinding {
    /// The names of the bindings that differ from `current`.
    pub fn changed_from(&self, current: &Self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        if self.contract_fingerprint != current.contract_fingerprint {
            changed.push("contract");
        }
        if self.definition_fingerprint != current.definition_fingerprint {
            changed.push("definition");
        }
        if self.examples_fingerprint != current.examples_fingerprint {
            changed.push("examples");
        }
        if self.policy_fingerprint != current.policy_fingerprint {
            changed.push("policy");
        }
        changed
    }
}

/// The latest decision event of a work as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedAdmission {
    pub event: Id,
    pub decision: AdmissionDecision,
    pub bound: AdmissionBinding,
    pub reason: Option<String>,
    /// The session that proposed what was decided; none for an inline decision.
    pub proposer_session: Option<Id>,
    pub decider_session: Option<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmissionDiagnostic {
    pub code: &'static str,
    pub detail: String,
}

/// Four facts that stay separate: a request or definition is recorded, its intent is adopted,
/// the work is admitted and the work is completed. None of them implies another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmissionFacts {
    pub recorded: bool,
    pub adopted: bool,
    pub admitted: bool,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmissionStatus {
    pub work: String,
    /// not_governed, legacy_observe or governed_enforce.
    pub mode: &'static str,
    pub governed: bool,
    pub enforced: bool,
    /// not_governed, missing, pending, denied, approved, stale or policy_invalid.
    pub state: &'static str,
    pub independent_review_required: bool,
    pub diagnostics: Vec<AdmissionDiagnostic>,
    pub facts: AdmissionFacts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<AdmissionBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_event: Option<Id>,
    pub basis: &'static str,
}

impl AdmissionStatus {
    /// The answer for work that no policy governs; it needs no facts and costs nothing to give.
    pub fn ungoverned(work: &str, mode: &'static str) -> Self {
        Self {
            work: work.into(),
            mode,
            governed: false,
            enforced: false,
            state: "not_governed",
            independent_review_required: false,
            diagnostics: vec![],
            facts: AdmissionFacts {
                recorded: false,
                adopted: false,
                admitted: false,
                completed: false,
            },
            current: None,
            decision_event: None,
            basis: BASIS,
        }
    }
    /// An unusable policy cannot say what is governed, so it fails closed rather than governing nothing.
    pub fn policy_invalid(work: &str) -> Self {
        Self {
            work: work.into(),
            mode: "policy_invalid",
            governed: true,
            enforced: true,
            state: "policy_invalid",
            independent_review_required: false,
            diagnostics: vec![AdmissionDiagnostic {
                code: "admission_policy_invalid",
                detail: "the admission_policy option of the owning source cannot be used; repair it before claiming".into(),
            }],
            facts: AdmissionFacts {
                recorded: false,
                adopted: false,
                admitted: false,
                completed: false,
            },
            current: None,
            decision_event: None,
            basis: BASIS,
        }
    }
}

const BASIS: &str = "Provenance and currency only: a decision exists, is approved and still describes the current scope, the obligations of the declared purpose are present, and a required review came from another session. Whether the reasoning is sound remains an attributed judgment.";

/// Everything the evaluator may read; the store fills it from the current projections.
pub struct AdmissionInputs<'a> {
    pub work: &'a str,
    pub tags: &'a [String],
    pub completed: bool,
    /// None when the owning source declares no admission policy.
    pub policy: Option<&'a AdmissionPolicy>,
    pub current: &'a AdmissionBinding,
    pub latest: Option<&'a RecordedAdmission>,
    pub definition: &'a DefinitionReport,
}

pub fn evaluate_admission(inputs: &AdmissionInputs<'_>) -> AdmissionStatus {
    let facts = |admitted: bool| AdmissionFacts {
        recorded: inputs.latest.is_some() || inputs.definition.state != "not_declared",
        adopted: inputs
            .definition
            .obligations
            .iter()
            .any(|o| o.obligation == "intent" && o.state == "satisfied"),
        admitted,
        completed: inputs.completed,
    };
    let status =
        |mode, governed, enforced, state, review, diagnostics: Vec<AdmissionDiagnostic>| {
            let admitted = state == "approved" && diagnostics.is_empty();
            AdmissionStatus {
                work: inputs.work.into(),
                mode,
                governed,
                enforced,
                state,
                independent_review_required: review,
                diagnostics,
                facts: facts(admitted),
                current: Some(inputs.current.clone()),
                decision_event: inputs.latest.map(|l| l.event),
                basis: BASIS,
            }
        };
    let Some(policy) = inputs.policy else {
        return AdmissionStatus::ungoverned(inputs.work, "not_governed");
    };
    let governed = policy.governs(inputs.tags);
    let mode = match policy.mode {
        AdmissionMode::LegacyObserve => "legacy_observe",
        AdmissionMode::GovernedEnforce => "governed_enforce",
    };
    let enforced = governed && policy.enforced();
    let review = governed && policy.requires_independent_review(inputs.tags);
    if !governed {
        return AdmissionStatus::ungoverned(inputs.work, mode);
    }
    let mut diagnostics = Vec::new();
    let state = match inputs.latest {
        None => {
            diagnostics.push(AdmissionDiagnostic {
                code: "admission_missing",
                detail: "no admission decision is recorded for this work".into(),
            });
            "missing"
        }
        Some(latest) => match latest.decision {
            AdmissionDecision::Proposed => {
                diagnostics.push(AdmissionDiagnostic {
                    code: "admission_pending",
                    detail: "an admission request is recorded but no decision answers it".into(),
                });
                "pending"
            }
            AdmissionDecision::Denied => {
                diagnostics.push(AdmissionDiagnostic {
                    code: "admission_denied",
                    detail: format!(
                        "the latest decision denies admission: {}",
                        latest.reason.as_deref().unwrap_or("no reason recorded")
                    ),
                });
                "denied"
            }
            AdmissionDecision::Approved => {
                let changed = latest.bound.changed_from(inputs.current);
                if changed.is_empty() {
                    "approved"
                } else {
                    diagnostics.push(AdmissionDiagnostic {
                        code: "admission_stale",
                        detail: format!(
                            "the approved scope no longer matches the current {}; review and decide again",
                            changed.join(", ")
                        ),
                    });
                    "stale"
                }
            }
        },
    };
    // Entry obligations follow the declared purpose; undeclared work is judged as implementation.
    let gaps: Vec<&str> = if inputs.definition.state == "not_declared" {
        vec!["definition_not_declared"]
    } else {
        inputs.definition.gaps.iter().map(|gap| gap.code).collect()
    };
    if !gaps.is_empty() {
        diagnostics.push(AdmissionDiagnostic {
            code: "admission_obligations_unmet",
            detail: format!("definition gaps: {}", gaps.join(", ")),
        });
    }
    status(mode, true, enforced, state, review, diagnostics)
}
