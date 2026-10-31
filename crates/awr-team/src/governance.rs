//! Governed work in a Team. A source may declare, per work, whether the work is governed and what
//! its definition says. The Team keeps that beside the contract and judges whether a recorded
//! decision still admits the work. The rules are the Core's (`evaluate_admission`): this module
//! only supplies the Team's facts, so a decision means the same thing in the local store and in the
//! Team. Nothing here grants anything. A source describes governance, it cannot approve it, and a
//! publish or import never writes a decision.
use crate::canonical::canonical_json;
use crate::error::{TeamError, TeamResult};
use awr_core::{
    AdmissionBinding, AdmissionInputs, AdmissionPolicy, AdmissionStatus, DefinitionSummary,
    RecordedAdmission, WorkDefinition, admission_binding, evaluate_admission,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GOVERNANCE_VERSION: u32 = 1;
/// The candidate file that carries governance in a publish package.
pub const GOVERNANCE_FILE: &str = "governance.json";
const MAX_WORKS: usize = 20_000;
const MAX_KEY: usize = 512;
const MAX_TAGS: usize = 32;
const MAX_TAG: usize = 128;

/// What the source declares about one governed work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkGovernance {
    pub external_key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The declared definition; none when the work declares none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<WorkDefinition>,
    /// What admission reads of the definition report, computed where the whole source is at hand.
    pub summary: DefinitionSummary,
}

/// The governance of one source snapshot: its effective policy and every work it governs, in the
/// order of their keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernanceCandidate {
    pub version: u32,
    pub policy: AdmissionPolicy,
    pub works: Vec<WorkGovernance>,
}

fn invalid(detail: impl Into<String>) -> TeamError {
    TeamError::InvalidContract(format!("governance: {}", detail.into()))
}

impl GovernanceCandidate {
    pub fn validate(&self) -> TeamResult<()> {
        if self.version != GOVERNANCE_VERSION {
            return Err(invalid(format!("unsupported version {}", self.version)));
        }
        self.policy.validate().map_err(|e| invalid(e.to_string()))?;
        if self.works.len() > MAX_WORKS {
            return Err(invalid(format!("more than {MAX_WORKS} works")));
        }
        let mut previous: Option<&str> = None;
        for work in &self.works {
            let key = work.external_key.as_str();
            if key.trim().is_empty() || key.len() > MAX_KEY || key != key.trim() {
                return Err(invalid("a work key is blank, padded or too long"));
            }
            // One canonical order, so equal governance always has equal bytes and one hash.
            if previous.is_some_and(|p| p >= key) {
                return Err(invalid(format!("works are not unique and sorted at {key}")));
            }
            previous = Some(key);
            if work.tags.len() > MAX_TAGS
                || work
                    .tags
                    .iter()
                    .any(|t| t.trim().is_empty() || t.len() > MAX_TAG || t != t.trim())
            {
                return Err(invalid(format!("{key}: tags are not bounded names")));
            }
            if let Some(definition) = &work.definition {
                definition
                    .validate()
                    .map_err(|e| invalid(format!("{key}: {e}")))?;
            }
            // A summary of a definition that is not declared cannot claim an adopted intent.
            if work.definition.is_none() && (work.summary.declared || work.summary.intent_adopted) {
                return Err(invalid(format!(
                    "{key}: the summary describes a definition the work does not declare"
                )));
            }
        }
        Ok(())
    }

    /// The hash of the whole candidate in its canonical form.
    pub fn hash(&self) -> TeamResult<String> {
        let value = serde_json::to_value(self).map_err(|e| invalid(e.to_string()))?;
        Ok(format!("{:x}", Sha256::digest(canonical_json(&value)?)))
    }

    pub fn work(&self, external_key: &str) -> Option<&WorkGovernance> {
        self.works
            .binary_search_by(|w| w.external_key.as_str().cmp(external_key))
            .ok()
            .map(|i| &self.works[i])
    }
}

impl WorkGovernance {
    /// The scope a decision on this work covers: its contract, the effective policy, its definition
    /// and its reviewed examples.
    pub fn binding(
        &self,
        policy: &AdmissionPolicy,
        contract_hash: &str,
    ) -> TeamResult<AdmissionBinding> {
        let policy = serde_json::to_value(policy).map_err(|e| invalid(e.to_string()))?;
        admission_binding(contract_hash, &policy, self.definition.as_ref())
            .map_err(|e| invalid(e.to_string()))
    }
}

/// The fingerprint of the scope a decision covers: what a decider reads and a decision is checked
/// against, so a decision on a scope that moved since it was read is refused.
pub fn binding_fingerprint(binding: &AdmissionBinding) -> TeamResult<String> {
    let value = serde_json::to_value(binding).map_err(|e| invalid(e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(canonical_json(&value)?)))
}

/// Whether the latest recorded decision still admits the work as the snapshot now describes it.
/// `completed` marks work that is already finished, which owes nothing further.
pub fn admission_status(
    policy: &AdmissionPolicy,
    work: &WorkGovernance,
    contract_hash: &str,
    completed: bool,
    latest: Option<&RecordedAdmission>,
) -> TeamResult<AdmissionStatus> {
    let current = work.binding(policy, contract_hash)?;
    Ok(evaluate_admission(&AdmissionInputs {
        work: &work.external_key,
        tags: &work.tags,
        completed,
        policy: Some(policy),
        current: &current,
        latest,
        definition: &work.summary,
    }))
}
