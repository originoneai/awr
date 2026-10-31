//! The project's assessment policy in the runtime (DEC-040): which version is in force, a read-only preview of a change,
//! and the one confirmed write.
//!
//! The file is the authority. An evaluation reads it and never writes it; the only function that writes it is
//! [`apply_policy_change`], and it writes exactly the bytes whose fingerprint the caller confirmed, over exactly the
//! bytes whose fingerprint the caller read. Nothing recorded by the runtime (observations, logs, receipts) reaches it.
use awr_core::*;
use awr_source::{
    AssessmentPolicyFile, apply_assessment_policy, read_assessment_policy, read_candidate,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The policy for one evaluation, with what the file said or why it was not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyInForce {
    pub resolved: ResolvedPolicy,
    /// Fingerprint of the file's bytes; none when there is no file.
    pub file_fingerprint: Option<String>,
    /// Stable codes; empty when all is well. An unusable file is never a partial policy: the built-in one applies.
    pub diagnostics: Vec<String>,
}

/// What an explanation says about the policy that shaped it. Absent when the built-in policy applied and nothing was said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyNotice {
    pub source: String,
    pub policy_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

impl PolicyInForce {
    pub fn notice(&self) -> Option<PolicyNotice> {
        let said_something = self.resolved.source == PolicySource::Project
            || !self.diagnostics.is_empty()
            || !self.resolved.ceiling_applied.is_empty();
        said_something.then(|| PolicyNotice {
            source: match self.resolved.source {
                PolicySource::Builtin => "builtin",
                PolicySource::Project => "project",
            }
            .into(),
            policy_id: self.resolved.policy_id.clone(),
            version: self.resolved.version,
            policy_hash: self.resolved.assessment.policy_hash.clone(),
            file_fingerprint: self.file_fingerprint.clone(),
            diagnostics: self.diagnostics.clone(),
        })
    }
}

fn read_failure_code(error: &Error) -> &'static str {
    match error {
        Error::Unsupported(_) => "project_policy_newer_format",
        Error::InvalidInput(_) => "project_policy_invalid",
        _ => "project_policy_unreadable",
    }
}

/// The policy in force for `as_of`. Never fails and never writes: a file that is missing is the built-in policy; a file
/// that cannot be used (newer format, invalid, unreadable, refused by the ceiling) is the built-in policy and a code
/// that says so, so advice is never shaped by half of a configuration.
pub fn policy_in_force(root: &Path, as_of: i64, ceiling: &PolicyCeiling) -> PolicyInForce {
    let registry = Registry::builtin();
    let builtin = |diagnostics: Vec<String>, fingerprint: Option<String>| PolicyInForce {
        resolved: resolve_policy(None, &registry, ceiling, as_of)
            .expect("the built-in policy always resolves"),
        file_fingerprint: fingerprint,
        diagnostics,
    };
    match read_assessment_policy(root) {
        Ok(None) => builtin(vec![], None),
        Err(error) => builtin(vec![read_failure_code(&error).into()], None),
        Ok(Some(file)) => match resolve_policy(Some(&file.document), &registry, ceiling, as_of) {
            Ok(resolved) => PolicyInForce {
                resolved,
                file_fingerprint: Some(file.fingerprint),
                diagnostics: vec![],
            },
            Err(issue) => builtin(
                vec![format!("project_policy_refused:{}", issue.code.as_str())],
                Some(file.fingerprint),
            ),
        },
    }
}

/// One version as the file lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionSummary {
    pub version: u32,
    pub effective_from: i64,
    pub extends: String,
}

/// What `policy show` prints.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyShown {
    pub as_of: i64,
    pub file: &'static str,
    pub file_fingerprint: Option<String>,
    pub versions: Vec<VersionSummary>,
    pub in_force: ResolvedPolicy,
    pub diagnostics: Vec<String>,
}

pub fn show_policy(root: &Path, as_of: i64, ceiling: &PolicyCeiling) -> PolicyShown {
    let in_force = policy_in_force(root, as_of, ceiling);
    let versions = read_assessment_policy(root)
        .ok()
        .flatten()
        .map(|file| summarize(&file.document))
        .unwrap_or_default();
    PolicyShown {
        as_of,
        file: awr_source::ASSESSMENT_POLICY_FILE,
        file_fingerprint: in_force.file_fingerprint.clone(),
        versions,
        diagnostics: in_force.diagnostics,
        in_force: in_force.resolved,
    }
}

fn summarize(document: &PolicyDocument) -> Vec<VersionSummary> {
    document
        .versions
        .iter()
        .map(|version| VersionSummary {
            version: version.version,
            effective_from: version.effective_from,
            extends: match &version.extends {
                None => "builtin".into(),
                Some(Extends::Builtin(word)) => word.clone(),
                Some(Extends::Version(earlier)) => format!("v{earlier}"),
            },
        })
        .collect()
}

/// What a change would do, read-only. `ok` says whether [`apply_policy_change`] would accept it now; the two
/// fingerprints are what the caller must send back to confirm.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyPreview {
    pub ok: bool,
    pub as_of: i64,
    pub file: &'static str,
    /// The bytes of the file as they are now; none when there is no file.
    pub current_fingerprint: Option<String>,
    /// The bytes the change would write.
    pub candidate_fingerprint: String,
    pub unchanged: bool,
    pub appended_versions: Vec<u32>,
    pub issues: Vec<PolicyIssue>,
    /// The policy in force now and the one that would be in force at the same `as_of` after the change.
    pub in_force_before: ResolvedPolicy,
    pub in_force_after: Option<ResolvedPolicy>,
    pub changes: Vec<PolicyChange>,
    pub writes: bool,
}

fn refusal(issues: &[PolicyIssue]) -> Error {
    Error::InvalidInput(format!(
        "assessment policy change refused: {}",
        issues
            .iter()
            .map(PolicyIssue::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn current_file(root: &Path) -> Result<Option<AssessmentPolicyFile>> {
    read_assessment_policy(root)
}

fn plan(
    root: &Path,
    candidate: &[u8],
    as_of: i64,
    ceiling: &PolicyCeiling,
) -> Result<(
    Option<AssessmentPolicyFile>,
    AssessmentPolicyFile,
    PolicyPreview,
)> {
    let registry = Registry::builtin();
    let candidate_file = read_candidate(candidate)?;
    let current = current_file(root)?;
    let mut issues = validate_document(&candidate_file.document, &registry, ceiling);
    if let Some(current) = &current {
        if let Err(issue) = check_history_preserved(&current.document, &candidate_file.document) {
            issues.push(issue);
        }
    }
    let before = resolve_policy(
        current.as_ref().map(|file| &file.document),
        &registry,
        ceiling,
        as_of,
    )
    .or_else(|_| resolve_policy(None, &registry, ceiling, as_of))
    .expect("the built-in policy always resolves");
    let after = issues
        .is_empty()
        .then(|| resolve_policy(Some(&candidate_file.document), &registry, ceiling, as_of).ok())
        .flatten();
    let first_new = current
        .as_ref()
        .map_or(0, |file| file.document.versions.len());
    let appended_versions = candidate_file
        .document
        .versions
        .iter()
        .skip(first_new)
        .map(|version| version.version)
        .collect();
    let changes = after
        .as_ref()
        .map(|after| describe_change(&before, after))
        .unwrap_or_default();
    let preview = PolicyPreview {
        ok: issues.is_empty() && after.is_some(),
        as_of,
        file: awr_source::ASSESSMENT_POLICY_FILE,
        current_fingerprint: current.as_ref().map(|file| file.fingerprint.clone()),
        candidate_fingerprint: candidate_file.fingerprint.clone(),
        unchanged: current
            .as_ref()
            .is_some_and(|file| file.fingerprint == candidate_file.fingerprint),
        appended_versions,
        issues,
        in_force_before: before,
        in_force_after: after,
        changes,
        writes: false,
    };
    Ok((current, candidate_file, preview))
}

/// Reads the candidate and the file and says what the change would do. Writes nothing.
pub fn preview_policy_change(
    root: &Path,
    candidate: &[u8],
    as_of: i64,
    ceiling: &PolicyCeiling,
) -> Result<PolicyPreview> {
    plan(root, candidate, as_of, ceiling).map(|(_, _, preview)| preview)
}

/// What a confirmed change did.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyApplied {
    pub file: &'static str,
    pub before_fingerprint: Option<String>,
    pub after_fingerprint: String,
    pub appended_versions: Vec<u32>,
    pub changes: Vec<PolicyChange>,
    pub wrote: bool,
}

/// The one write. The caller names the fingerprint of the file it read (`None`: there was none) and the fingerprint of the
/// candidate it previewed; if either is not what is there now, nothing is written. A candidate that is invalid, that
/// edits or drops a version already in the file, or that goes beyond the ceiling is refused with the codes.
pub fn apply_policy_change(
    root: &Path,
    candidate: &[u8],
    expected_current: Option<&str>,
    expected_candidate: &str,
    as_of: i64,
    ceiling: &PolicyCeiling,
) -> Result<PolicyApplied> {
    let (current, candidate_file, preview) = plan(root, candidate, as_of, ceiling)?;
    if candidate_file.fingerprint != expected_candidate {
        return Err(Error::SourceConflict(
            "the candidate is not the one that was previewed".into(),
        ));
    }
    let current_fingerprint = current.as_ref().map(|file| file.fingerprint.as_str());
    if current_fingerprint != expected_current {
        return Err(Error::SourceConflict(
            "the policy file changed since it was read".into(),
        ));
    }
    if !preview.ok {
        return Err(refusal(&preview.issues));
    }
    let wrote = !preview.unchanged;
    if wrote {
        apply_assessment_policy(root, expected_current, candidate)?;
    }
    Ok(PolicyApplied {
        file: awr_source::ASSESSMENT_POLICY_FILE,
        before_fingerprint: preview.current_fingerprint,
        after_fingerprint: candidate_file.fingerprint,
        appended_versions: preview.appended_versions,
        changes: preview.changes,
        wrote,
    })
}
