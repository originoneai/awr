//! Controlled decomposition. Splitting a parent, widening its scope or changing its dependencies
//! is a change of the parent's set of children. A containment decision binds the exact version of
//! what it judged: the parent's contract, the set of its children (each with its definition,
//! paths, acceptance and dependencies) and the dependencies between them. This module holds the
//! types, the request validation and a pure evaluator; the store gathers the facts and persists
//! decisions as events, as it does for admission.
//!
//! The program checks what can be checked without judgment: every child stays within the
//! parent's paths, names the parent, depends only on work that exists, and the decision is
//! current and made by an authorized agent. Whether the children really are contained in the
//! parent's intent remains an attributed judgment.
use crate::admission::{bounded, fingerprint};
use crate::{AdmissionDecision, Error, Id, Result, ensure_public_data};
use serde::{Deserialize, Serialize};

pub const CONTAINMENT_EVENT: &str = "work.containment";

/// Whether a readiness diagnostic code belongs to plan admission, which includes the containment
/// of a governed work's parent and the reassessment of a governed work that deviates.
pub fn is_admission_code(code: &str) -> bool {
    code.starts_with("admission_")
        || code.starts_with("containment_")
        || code.starts_with("reassessment_")
}
/// The attributed judgment that the children stay inside the parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainmentJudgment {
    /// Why every child serves the parent's intent and none widens it.
    pub contained_because: String,
    /// What already existed and was reused instead of becoming a new child.
    pub reuse_checked: String,
    /// What was deliberately left out of the children.
    pub out_of_scope_excluded: String,
}

impl ContainmentJudgment {
    pub fn validate(&self) -> Result<()> {
        if [
            &self.contained_because,
            &self.reuse_checked,
            &self.out_of_scope_excluded,
        ]
        .into_iter()
        .any(|text| !bounded(text))
        {
            return Err(Error::InvalidInput(
                "a containment judgment needs bounded contained_because, reuse_checked and out_of_scope_excluded".into(),
            ));
        }
        Ok(())
    }
}

/// One request to record a containment event for the children of a parent work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainmentRequest {
    /// The parent work whose set of children is judged.
    pub parent: String,
    /// An active session, or a declaring agent when no session is given (attribution only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub expected_revision: u64,
    /// Stable idempotency key: the same key with the same content returns the original receipt.
    pub request_key: String,
    pub decision: AdmissionDecision,
    /// The change-set fingerprint the decider reviewed, from `work show` of the parent.
    pub change_set_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judgment: Option<ContainmentJudgment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_event: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ContainmentRequest {
    /// What a request key binds: the request without its revision guard and the key itself.
    pub fn input(&self) -> serde_json::Value {
        serde_json::json!({
            "parent": self.parent, "session": self.session, "agent": self.agent,
            "change_set_fingerprint": self.change_set_fingerprint,
            "decision": self.decision, "judgment": self.judgment,
            "proposal_event": self.proposal_event, "reason": self.reason,
        })
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |why: &str| Err(Error::InvalidInput(format!("containment request {why}")));
        if self.request_key.trim().is_empty()
            || self.request_key.len() > 512
            || self.request_key.chars().any(char::is_control)
        {
            return invalid("requires a bounded stable request_key");
        }
        if self.parent.trim().is_empty() || self.parent.len() > 512 {
            return invalid("requires the parent work key");
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
        if !fingerprint(&self.change_set_fingerprint) {
            return invalid("requires the reviewed change_set_fingerprint (sha256:...)");
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

/// The exact version of what a containment decision judged, or the version as it now stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainmentBinding {
    /// The parent's management contract (CG-001), which includes its declared definition.
    pub parent_contract: String,
    /// The children with their definitions, paths, acceptance and dependencies.
    pub change_set: String,
    /// The dependency edges of the children, including those on work outside the set.
    pub dependencies: String,
    /// The child keys, so a removed or added child can be named.
    pub children: Vec<String>,
}

impl ContainmentBinding {
    /// The names of the parts that differ from `current`, with each added or removed child named.
    pub fn changed_from(&self, current: &Self) -> Vec<String> {
        let mut changed = Vec::new();
        if self.parent_contract != current.parent_contract {
            changed.push("parent_contract".to_string());
        }
        if self.change_set != current.change_set {
            changed.push("change_set".to_string());
        }
        if self.dependencies != current.dependencies {
            changed.push("dependencies".to_string());
        }
        for key in &current.children {
            if !self.children.contains(key) {
                changed.push(format!("child_added:{key}"));
            }
        }
        for key in &self.children {
            if !current.children.contains(key) {
                changed.push(format!("child_removed:{key}"));
            }
        }
        changed
    }
}

/// The latest containment event of a parent as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedContainment {
    pub event: Id,
    pub decision: AdmissionDecision,
    pub bound: ContainmentBinding,
    pub reason: Option<String>,
    pub decider_agent: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainmentDiagnostic {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainmentStatus {
    pub parent: String,
    /// no_children, missing, pending, denied, approved or stale.
    pub state: &'static str,
    pub diagnostics: Vec<ContainmentDiagnostic>,
    pub children: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<ContainmentBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_event: Option<Id>,
    pub basis: &'static str,
}

const BASIS: &str = "The program checks that every child stays within the parent's paths, names the parent and depends on work that exists, and that a current decision by an authorized agent covers exactly this set. Whether the children are contained in the parent's intent remains an attributed judgment.";

impl ContainmentStatus {
    pub fn no_children(parent: &str) -> Self {
        Self {
            parent: parent.into(),
            state: "no_children",
            diagnostics: vec![],
            children: vec![],
            current: None,
            decision_event: None,
            basis: BASIS,
        }
    }
}

/// Everything the evaluator may read; the store fills it from the current projections.
pub struct ContainmentInputs<'a> {
    pub parent: &'a str,
    /// Agents allowed to approve; empty means any attributed agent may.
    pub supervisors: &'a [String],
    pub current: &'a ContainmentBinding,
    /// Deterministic failures of the children, as `(code, detail)`.
    pub failures: &'a [(String, String)],
    pub latest: Option<&'a RecordedContainment>,
}

pub fn evaluate_containment(inputs: &ContainmentInputs<'_>) -> ContainmentStatus {
    let mut diagnostics = Vec::new();
    let state = match inputs.latest {
        None => {
            diagnostics.push(ContainmentDiagnostic {
                code: "containment_missing",
                detail: "no containment decision is recorded for the children of this work".into(),
            });
            "missing"
        }
        Some(latest) => match latest.decision {
            AdmissionDecision::Proposed => {
                diagnostics.push(ContainmentDiagnostic {
                    code: "containment_pending",
                    detail: "a containment request is recorded but no decision answers it".into(),
                });
                "pending"
            }
            AdmissionDecision::Denied => {
                diagnostics.push(ContainmentDiagnostic {
                    code: "containment_denied",
                    detail: format!(
                        "the latest decision denies this set of children: {}",
                        latest.reason.as_deref().unwrap_or("no reason recorded")
                    ),
                });
                "denied"
            }
            AdmissionDecision::Approved => {
                let changed = latest.bound.changed_from(inputs.current);
                if !changed.is_empty() {
                    diagnostics.push(ContainmentDiagnostic {
                        code: "containment_stale",
                        detail: format!(
                            "the approved set no longer matches the current {}; review and decide again",
                            changed.join(", ")
                        ),
                    });
                    "stale"
                } else if !inputs.supervisors.is_empty()
                    && !inputs.supervisors.contains(&latest.decider_agent)
                {
                    diagnostics.push(ContainmentDiagnostic {
                        code: "containment_not_authorized",
                        detail: format!(
                            "{} is not among the supervisors the policy authorizes to approve a containment",
                            latest.decider_agent
                        ),
                    });
                    "approved"
                } else {
                    "approved"
                }
            }
        },
    };
    if !inputs.failures.is_empty() {
        diagnostics.push(ContainmentDiagnostic {
            code: "containment_unchecked",
            detail: inputs
                .failures
                .iter()
                .map(|(code, detail)| format!("{code}: {detail}"))
                .collect::<Vec<_>>()
                .join("; "),
        });
    }
    ContainmentStatus {
        parent: inputs.parent.into(),
        state,
        diagnostics,
        children: inputs.current.children.clone(),
        current: Some(inputs.current.clone()),
        decision_event: inputs.latest.map(|l| l.event),
        basis: BASIS,
    }
}
