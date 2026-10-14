use crate::{Error, Evidence, EvidenceLevel, Id, Result, WorkItem, is_source_sha};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn verification_rank(level: EvidenceLevel) -> Option<u8> {
    match level {
        EvidenceLevel::Designed => Some(0),
        EvidenceLevel::Implemented => Some(1),
        EvidenceLevel::LocallyVerified => Some(2),
        EvidenceLevel::RealEnvironmentValidated => Some(3),
        EvidenceLevel::ReleaseCandidate => Some(4),
        EvidenceLevel::Released => Some(5),
        EvidenceLevel::Unknown => None,
    }
}
fn minimum_level() -> EvidenceLevel {
    EvidenceLevel::LocallyVerified
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceEvidenceRequest {
    pub criterion: String,
    pub evidence: Vec<String>,
}

/// Explicit mapping from each authoritative acceptance criterion to registered evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionInput {
    pub version: u32,
    pub source_sha: String,
    #[serde(default = "minimum_level")]
    pub minimum_level: EvidenceLevel,
    pub acceptance: Vec<AcceptanceEvidenceRequest>,
    #[serde(default)]
    pub required_evidence: Vec<String>,
}
impl CompletionInput {
    pub fn validate(&self, work: &WorkItem) -> Result<()> {
        if self.version != 1
            || !is_source_sha(&self.source_sha)
            || verification_rank(self.minimum_level).is_none_or(|r| r < 2)
        {
            return Err(Error::InvalidInput("completion requires version 1, a full source SHA and at least locally_verified evidence".into()));
        }
        validate_criteria(&work.acceptance)?;
        let declared = self
            .acceptance
            .iter()
            .map(|a| a.criterion.as_str())
            .collect::<BTreeSet<_>>();
        if declared.len() != self.acceptance.len()
            || declared != work.acceptance.iter().map(String::as_str).collect()
            || self.acceptance.iter().any(|a| a.evidence.is_empty())
        {
            return Err(Error::EvidenceMissing("completion must map every source acceptance criterion exactly once to nonempty evidence references".into()));
        }
        if self.references().is_empty()
            || self.references().len() > 100
            || self
                .references()
                .iter()
                .any(|s| s.trim().is_empty() || s.len() > 4096)
        {
            return Err(Error::InvalidInput(
                "completion requires 1..100 bounded evidence references".into(),
            ));
        }
        Ok(())
    }
    pub fn references(&self) -> BTreeSet<String> {
        self.acceptance
            .iter()
            .flat_map(|a| a.evidence.iter())
            .chain(self.required_evidence.iter())
            .cloned()
            .collect()
    }
}
pub fn validate_criteria(criteria: &[String]) -> Result<()> {
    if criteria.is_empty()
        || criteria.iter().any(|s| s.trim().is_empty())
        || criteria.iter().collect::<BTreeSet<_>>().len() != criteria.len()
    {
        return Err(Error::EvidenceMissing(
            "source acceptance must be nonempty, unambiguous criteria before work can complete"
                .into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionCheck {
    pub name: String,
    pub passed: bool,
    #[serde(alias = "result")]
    pub details: String,
    #[serde(default)]
    pub criteria: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
// Producer metadata may accompany this versioned report; every byte remains hash-bound.
pub struct CompletionReport {
    pub version: u32,
    pub work_item: String,
    pub source_sha: String,
    pub command: String,
    pub scope: Vec<String>,
    pub verified_at: i64,
    pub checks: Vec<CompletionCheck>,
}
impl CompletionReport {
    pub fn validate(
        &self,
        evidence: &Evidence,
        work_key: &str,
        criteria: &[String],
        at: i64,
    ) -> Result<()> {
        // Diagnostics stay value-free: submitted report content must never be
        // echoed back, so failures identify the JSON pointer of the offending
        // field plus a static rule and repair hint (see also #321).
        let known = criteria.iter().map(String::as_str).collect::<BTreeSet<_>>();
        if self.version != 1 {
            return Err(Error::EvidenceMissing(
                "/version must be 1 in the verification report".into(),
            ));
        }
        if self.work_item != work_key {
            return Err(Error::EvidenceMissing(
                "/work_item must equal the key of the work item being completed".into(),
            ));
        }
        if !is_source_sha(&self.source_sha) {
            return Err(Error::EvidenceMissing(
                "verification report /source_sha must be a full source commit SHA".into(),
            ));
        }
        if !evidence
            .source_sha
            .as_ref()
            .is_some_and(|sha| sha.eq_ignore_ascii_case(&self.source_sha))
        {
            return Err(Error::EvidenceMissing(
                "verification report /source_sha must equal the SHA bound to the evidence".into(),
            ));
        }
        if evidence.command.as_deref() != Some(&self.command) || self.command.trim().is_empty() {
            return Err(Error::EvidenceMissing(
                "verification report /command must equal the command registered with the evidence and be nonempty".into(),
            ));
        }
        if self.scope.iter().collect::<BTreeSet<_>>() != evidence.scope.iter().collect() {
            return Err(Error::EvidenceMissing(
                "verification report /scope must match the scope registered with the evidence"
                    .into(),
            ));
        }
        if self.verified_at < 0 {
            return Err(Error::EvidenceMissing(
                "verification report /verified_at must be a non-negative Unix epoch milliseconds integer (now_millis())".into(),
            ));
        }
        if self.verified_at > at {
            return Err(Error::EvidenceMissing(
                "verification report /verified_at is later than the current time; use Unix epoch milliseconds (now_millis())".into(),
            ));
        }
        if evidence.verified_at != Some(self.verified_at) {
            return Err(Error::EvidenceMissing(
                "verification report /verified_at must equal the verified_at registered with the evidence".into(),
            ));
        }
        if self.checks.is_empty() {
            return Err(Error::EvidenceMissing(
                "verification report requires at least one check".into(),
            ));
        }
        for (check_index, check) in self.checks.iter().enumerate() {
            let check_path = format!("/checks/{check_index}");
            if !check.passed {
                return Err(Error::EvidenceMissing(format!(
                    "{check_path}/passed records passed=false; completion requires passing checks"
                )));
            }
            if check.name.trim().is_empty() {
                return Err(Error::EvidenceMissing(format!(
                    "{check_path}/name requires nonempty unique names"
                )));
            }
            if check.details.trim().is_empty() {
                return Err(Error::EvidenceMissing(format!(
                    "{check_path}/details requires nonempty details"
                )));
            }
            for (criterion_index, _) in check.criteria.iter().enumerate() {
                if !known.contains(check.criteria[criterion_index].as_str()) {
                    return Err(Error::EvidenceMissing(format!(
                        "{check_path}/criteria/{criterion_index} must match an authoritative acceptance criterion from the ledger byte-for-byte"
                    )));
                }
            }
        }
        if self
            .checks
            .iter()
            .map(|c| &c.name)
            .collect::<BTreeSet<_>>()
            .len()
            != self.checks.len()
        {
            return Err(Error::EvidenceMissing(
                "verification report check names must be unique".into(),
            ));
        }
        Ok(())
    }
    pub fn covers(&self, criterion: &str) -> bool {
        self.checks
            .iter()
            .any(|c| c.passed && c.criteria.iter().any(|s| s == criterion))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceEvidenceBinding {
    pub criterion: String,
    pub evidence: Vec<Id>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionBinding {
    pub version: u32,
    pub source_sha: String,
    pub minimum_level: EvidenceLevel,
    pub acceptance: Vec<AcceptanceEvidenceBinding>,
    /// All selected records, including extra required evidence. Metadata is immutable here;
    /// application rechecks the stored records and actual report bytes.
    pub evidence: Vec<Evidence>,
}
impl CompletionBinding {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || !is_source_sha(&self.source_sha)
            || verification_rank(self.minimum_level).is_none_or(|r| r < 2)
        {
            return Err(Error::InvalidInput(
                "invalid completion binding version, SHA or minimum level".into(),
            ));
        }
        let ids = self.evidence.iter().map(|e| e.id).collect::<BTreeSet<_>>();
        if ids.is_empty() || ids.len() != self.evidence.len() || ids.len() > 100 {
            return Err(Error::EvidenceMissing(
                "completion needs 1..100 distinct bound evidence records".into(),
            ));
        }
        validate_criteria(
            &self
                .acceptance
                .iter()
                .map(|a| a.criterion.clone())
                .collect::<Vec<_>>(),
        )?;
        if self.acceptance.iter().any(|a| {
            a.evidence.is_empty()
                || a.evidence.iter().any(|id| !ids.contains(id))
                || a.evidence.iter().collect::<BTreeSet<_>>().len() != a.evidence.len()
        }) {
            return Err(Error::EvidenceMissing(
                "acceptance mapping references missing or repeated evidence".into(),
            ));
        }
        for e in &self.evidence {
            if verification_rank(e.level)
                .is_none_or(|r| r < verification_rank(self.minimum_level).unwrap())
                || !e
                    .source_sha
                    .as_ref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&self.source_sha))
            {
                return Err(Error::EvidenceMissing(
                    "evidence does not meet the bound level or source SHA".into(),
                ));
            }
            let assessment = crate::EvidenceRecord {
                item: e.clone(),
                source: None,
                project_revision: 0,
            }
            .assess(Some(&self.source_sha), None, e.branch_id);
            if !assessment.missing_bindings.is_empty() {
                return Err(Error::EvidenceMissing(format!(
                    "evidence {} lacks {}",
                    e.id,
                    assessment.missing_bindings.join(", ")
                )));
            }
        }
        Ok(())
    }
    pub fn verified_level(&self) -> Result<EvidenceLevel> {
        self.validate()?;
        Ok(self
            .evidence
            .iter()
            .min_by_key(|e| verification_rank(e.level))
            .unwrap()
            .level)
    }
}

/// Preserve the source's existing references and verification metadata, append bound
/// report locators, and record only the evidence level actually supported by all records.
pub fn completion_source_changes(record: &Value, binding: &CompletionBinding) -> Result<Value> {
    binding.validate()?;
    let mut evidence = match record.get("evidence") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        _ => {
            return Err(Error::MutationUnsupported(
                "source evidence is not an appendable list".into(),
            ));
        }
    };
    for item in &binding.evidence {
        let existing = evidence.iter().any(|e| {
            e.as_str() == Some(&item.locator)
                || e.get("locator").and_then(Value::as_str) == Some(&item.locator)
                || e.get("path").and_then(Value::as_str) == Some(&item.locator)
        });
        if !existing {
            evidence.push(json!(item.locator));
        }
    }
    let mut verification = match record.get("verification") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(v)) => v.clone(),
        _ => {
            return Err(Error::MutationUnsupported(
                "source verification metadata is not a mapping".into(),
            ));
        }
    };
    let level = serde_json::to_value(binding.verified_level()?)?;
    verification.insert("evidence_level".into(), level.clone());
    let mut changes = json!({"status":"completed","evidence":evidence,"verification":verification});
    if record.get("evidence_level").is_some() {
        changes["evidence_level"] = level;
    }
    Ok(changes)
}
