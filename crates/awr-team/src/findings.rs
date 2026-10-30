//! Findings of a Team review. The severity of a finding is a consequence of the record, not a claim
//! of whoever raises it: the Core's rule (`assess_finding`) decides from what the finding is
//! measured against, and this module supplies the Team's facts, the acceptance criteria and hard
//! rules of the contract and the tier adopted for the work. A re-review looks at the blocking
//! findings that are still open and at what changed since the reviewed bundle; anything else a
//! second round finds is a deferred record and does not reopen the round.
use crate::error::{TeamError, TeamResult};
use awr_core::{
    Counterexample, DeliveryTier, FindingAction, FindingFacts, FindingInput, Risk, assess_finding,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MAX_KEY: usize = 128;
const MAX_TEXT: usize = 2048;

/// What a re-review looks at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewScope {
    /// A blocking finding of an earlier round that is still open.
    Unresolved { finding_key: String },
    /// Content that changed since the reviewed bundle.
    Delta,
}

/// One finding as a reviewer states it in a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub finding_key: String,
    pub summary: String,
    /// The acceptance criterion of the work it is measured against, word for word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<String>,
    /// The hard rule of the contract it is measured against, word for word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<Counterexample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<Risk>,
    /// Required from the second round on: what of the re-review scope the finding belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ReviewScope>,
}

/// A finding with the severity the record gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JudgedFinding {
    pub finding: ReviewFinding,
    /// `blocking` or `non_blocking`.
    pub severity: String,
    /// `returns_work` for a blocking finding, `deferred` for the rest.
    pub disposition: &'static str,
    pub basis: String,
}

fn invalid(detail: impl Into<String>) -> TeamError {
    TeamError::InvalidInput(format!("finding: {}", detail.into()))
}

fn name_ok(text: &str, limit: usize) -> bool {
    !text.trim().is_empty()
        && text.len() <= limit
        && text == text.trim()
        && !text.chars().any(char::is_control)
}

fn text_ok(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= MAX_TEXT
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}

impl ReviewFinding {
    pub fn validate(&self) -> TeamResult<()> {
        if !name_ok(&self.finding_key, MAX_KEY) {
            return Err(invalid("the key is blank, padded or too long"));
        }
        if !text_ok(&self.summary) {
            return Err(invalid("the summary is blank or too long"));
        }
        for (field, value) in [("criterion", &self.criterion), ("rule", &self.rule)] {
            if value.as_deref().is_some_and(|v| !text_ok(v)) {
                return Err(invalid(format!("{field} is blank or too long")));
            }
        }
        if let Some(c) = &self.counterexample
            && ![&c.given, &c.when, &c.then].into_iter().all(|t| text_ok(t))
        {
            return Err(invalid("the counterexample needs given, when and then"));
        }
        if let Some(r) = &self.risk
            && !text_ok(&r.reasoning)
        {
            return Err(invalid("a stated risk needs its reasoning"));
        }
        if let Some(ReviewScope::Unresolved { finding_key }) = &self.scope
            && !name_ok(finding_key, MAX_KEY)
        {
            return Err(invalid("the scope names no finding"));
        }
        Ok(())
    }
}

/// Judges a finding raised in review round `round` (counted from 1) of a work whose contract has
/// these acceptance criteria and hard rules, with `tier` adopted for its criteria and `unresolved`
/// the keys of the blocking findings that are still open.
pub fn judge_finding(
    finding: &ReviewFinding,
    round: u32,
    acceptance: &[String],
    hard_rules: &[String],
    tier: DeliveryTier,
    unresolved: &BTreeSet<String>,
) -> TeamResult<JudgedFinding> {
    finding.validate()?;
    if finding
        .criterion
        .as_deref()
        .is_some_and(|c| !acceptance.iter().any(|a| a == c))
    {
        return Err(invalid(
            "the criterion is not an acceptance criterion of the work",
        ));
    }
    if finding
        .rule
        .as_deref()
        .is_some_and(|r| !hard_rules.iter().any(|h| h == r))
    {
        return Err(invalid("the rule is not a hard rule of the contract"));
    }
    let input = FindingInput {
        work: String::new(),
        finding_key: finding.finding_key.clone(),
        action: Some(FindingAction::Record),
        round: Some(round),
        summary: Some(finding.summary.clone()),
        criterion: finding.criterion.clone(),
        rule: finding.rule.clone(),
        counterexample: finding.counterexample.clone(),
        risk: finding.risk.clone(),
        resolution: None,
    };
    let facts = FindingFacts {
        criterion_tier: finding.criterion.as_ref().map(|_| tier),
        rule_is_hard: finding.rule.as_ref().map(|_| true),
    };
    let mut assessed = assess_finding(&input, &facts);
    // From the second round on, a re-review answers for the findings that were open and for what
    // changed; a stated security or data-loss risk is never out of scope.
    if round >= 2 && assessed.severity == "blocking" && finding.risk.is_none() {
        let in_scope = match &finding.scope {
            Some(ReviewScope::Delta) => true,
            Some(ReviewScope::Unresolved { finding_key }) => unresolved.contains(finding_key),
            None => false,
        };
        if !in_scope {
            assessed.severity = "non_blocking".into();
            assessed.basis = format!(
                "{}, but a re-review covers the unresolved blocking findings and the delta since the reviewed bundle, and this belongs to neither",
                assessed.basis.trim_end_matches('.')
            );
        }
    }
    let disposition = if assessed.severity == "blocking" {
        "returns_work"
    } else {
        "deferred"
    };
    Ok(JudgedFinding {
        finding: finding.clone(),
        severity: assessed.severity,
        disposition,
        basis: assessed.basis,
    })
}
