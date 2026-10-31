//! A bounded local policy for the advice the assessment derives (DEC-040).
//!
//! A project may keep a small, versioned document that selects among rules the program already has: which optional
//! advice is shown, how many items and bytes an envelope may hold, and the order of the soft reason codes. It names
//! rules from a closed registry; it holds no script, no expression and no free text that is ever interpreted. It can
//! only narrow what the program does by default, never remove a hard protection, and never exceed what the host
//! allows (the ceiling). The version in force is chosen from the `as_of` of the evaluation, never from a clock, so
//! the same document, ceiling and `as_of` give the same policy and the same hash.
//!
//! Pure: no file, no store, no clock, no network. Reading and writing the file is the job of `awr-source`.
use crate::{
    ASSESSMENT_MAX_ADVISORY, ASSESSMENT_MAX_ASSESSMENTS, ASSESSMENT_MAX_CANDIDATES,
    ASSESSMENT_MAX_RETURN_BYTES, ASSESSMENT_POLICY_ID, ASSESSMENT_POLICY_VERSION, AssessmentPolicy,
    DEFAULT_HARD_REASON_CODES, DEFAULT_REASON_CODE_ORDER, ensure_public_text,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The one layout of the document this build reads; a newer one is refused, never guessed at.
pub const POLICY_FORMAT: u32 = 1;
/// The only scope a policy has in format 1.
pub const POLICY_SCOPE_PROJECT: &str = "project";
pub const MAX_POLICY_VERSIONS: usize = 64;
pub const MAX_POLICY_NOTE_BYTES: usize = 512;
pub const MAX_POLICY_ID_BYTES: usize = 64;

/// Advice a policy may switch off. Turning it off removes the advice, never the protection behind it: the
/// diagnostics, the claim, the completion and the freshness checks stay as they are.
pub const OPTIONAL_ADVICE: [&str; 6] = [
    "continue_prepare_current_work",
    "collect_user_reply",
    "refresh_sources",
    "repair_required_materials",
    "inspect_claim_conflict",
    "record_management_observations",
];

/// Advice no policy may name: it is what keeps an unknown effect from being run again and what says nothing when the
/// authority is gone.
pub const PROTECTED_ADVICE: [&str; 3] = [
    "query_original_operation_result",
    "no_advisory",
    "request_human_review",
];

/// One optional advice rule of the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdviceRule {
    pub id: &'static str,
    /// From this policy version on, a version may no longer set the rule; older versions keep their meaning.
    pub deprecated_in: Option<u32>,
}

/// The closed set of things a policy can select from. The built-in registry is the only one the program uses; tests
/// build others to prove the checks that the first release has no occasion for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub optional_advice: Vec<AdviceRule>,
    pub protected_advice: Vec<&'static str>,
    pub reason_code_order: Vec<&'static str>,
    pub hard_reason_codes: Vec<&'static str>,
}

impl Registry {
    pub fn builtin() -> Self {
        Self {
            optional_advice: OPTIONAL_ADVICE
                .iter()
                .map(|id| AdviceRule {
                    id,
                    deprecated_in: None,
                })
                .collect(),
            protected_advice: PROTECTED_ADVICE.to_vec(),
            reason_code_order: DEFAULT_REASON_CODE_ORDER.to_vec(),
            hard_reason_codes: DEFAULT_HARD_REASON_CODES.to_vec(),
        }
    }

    /// The reason codes a policy may reorder: those that are not hard guards, in their default order.
    pub fn soft_reason_codes(&self) -> Vec<&'static str> {
        self.reason_code_order
            .iter()
            .copied()
            .filter(|code| !self.hard_reason_codes.contains(code))
            .collect()
    }
}

/// What the host allows, whatever the project asks. A project value above it, or an explicit `true` for advice the host
/// has forced off, is a conflict and is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyCeiling {
    pub max_assessments: usize,
    pub max_advisory: usize,
    pub max_candidates: usize,
    pub max_return_bytes: usize,
    pub forced_off: BTreeSet<String>,
}

impl PolicyCeiling {
    /// No host restriction: the caps the program has always had.
    pub fn builtin() -> Self {
        Self {
            max_assessments: ASSESSMENT_MAX_ASSESSMENTS,
            max_advisory: ASSESSMENT_MAX_ADVISORY,
            max_candidates: ASSESSMENT_MAX_CANDIDATES,
            max_return_bytes: ASSESSMENT_MAX_RETURN_BYTES,
            forced_off: BTreeSet::new(),
        }
    }
}

/// Where a version gets what it does not set itself: the program's defaults, or an earlier version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Extends {
    /// Only the word `builtin`.
    Builtin(String),
    Version(u32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_assessments: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_advisory: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_candidates: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_return_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyOrder {
    /// A full permutation of the soft reason codes. Hard guards keep their slots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_reason_codes: Option<Vec<String>>,
}

/// One numbered version of the policy. Versions are never edited once they are in use; a change is a new version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyVersion {
    pub version: u32,
    /// The first `as_of` this version is in force for (the project revision for an explanation).
    #[serde(default)]
    pub effective_from: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<Extends>,
    /// Text for people. Never interpreted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub advice: BTreeMap<String, bool>,
    #[serde(default)]
    pub limits: PolicyLimits,
    #[serde(default)]
    pub order: PolicyOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDocument {
    pub format: u32,
    pub policy_id: String,
    #[serde(default = "project_scope")]
    pub scope: String,
    #[serde(default, rename = "version")]
    pub versions: Vec<PolicyVersion>,
}

fn project_scope() -> String {
    POLICY_SCOPE_PROJECT.into()
}

/// Why a document, a change or an evaluation is refused. The codes are stable; none of them carries text from the
/// document, only names from the closed registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyIssueCode {
    UnsupportedFormat,
    UnsupportedScope,
    InvalidPolicyId,
    TooManyVersions,
    VersionSequence,
    EffectiveFromOrder,
    InvalidExtends,
    InvalidNote,
    ProtectedRule,
    UnknownRule,
    DeprecatedRule,
    LimitOutOfRange,
    ExceedsCeiling,
    ConflictsWithCeiling,
    InvalidOrder,
    HistoryChanged,
}

impl PolicyIssueCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "unsupported_format",
            Self::UnsupportedScope => "unsupported_scope",
            Self::InvalidPolicyId => "invalid_policy_id",
            Self::TooManyVersions => "too_many_versions",
            Self::VersionSequence => "version_sequence",
            Self::EffectiveFromOrder => "effective_from_order",
            Self::InvalidExtends => "invalid_extends",
            Self::InvalidNote => "invalid_note",
            Self::ProtectedRule => "protected_rule",
            Self::UnknownRule => "unknown_rule",
            Self::DeprecatedRule => "deprecated_rule",
            Self::LimitOutOfRange => "limit_out_of_range",
            Self::ExceedsCeiling => "exceeds_ceiling",
            Self::ConflictsWithCeiling => "conflicts_with_ceiling",
            Self::InvalidOrder => "invalid_order",
            Self::HistoryChanged => "history_changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyIssue {
    pub code: PolicyIssueCode,
    /// The version the issue is in; none for the document as a whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// A name from the registry (never a name taken from the document that the registry does not know).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
}

impl PolicyIssue {
    fn new(code: PolicyIssueCode, version: Option<u32>) -> Self {
        Self {
            code,
            version,
            rule: None,
        }
    }

    fn rule(code: PolicyIssueCode, version: u32, rule: &str) -> Self {
        Self {
            code,
            version: Some(version),
            rule: Some(rule.into()),
        }
    }
}

impl std::fmt::Display for PolicyIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code.as_str())?;
        if let Some(version) = self.version {
            write!(f, " (version {version})")?;
        }
        if let Some(rule) = &self.rule {
            write!(f, " ({rule})")?;
        }
        Ok(())
    }
}

impl std::error::Error for PolicyIssue {}

fn valid_policy_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_POLICY_ID_BYTES
        && id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// Every reason a document is refused, in the order of the document, at most 32 of them.
pub fn validate_document(
    document: &PolicyDocument,
    registry: &Registry,
    ceiling: &PolicyCeiling,
) -> Vec<PolicyIssue> {
    const MAX_ISSUES: usize = 32;
    let mut issues = Vec::new();
    if document.format != POLICY_FORMAT {
        issues.push(PolicyIssue::new(PolicyIssueCode::UnsupportedFormat, None));
        return issues;
    }
    if document.scope != POLICY_SCOPE_PROJECT {
        issues.push(PolicyIssue::new(PolicyIssueCode::UnsupportedScope, None));
    }
    if !valid_policy_id(&document.policy_id) {
        issues.push(PolicyIssue::new(PolicyIssueCode::InvalidPolicyId, None));
    }
    if document.versions.len() > MAX_POLICY_VERSIONS {
        issues.push(PolicyIssue::new(PolicyIssueCode::TooManyVersions, None));
        return issues;
    }
    let soft = registry.soft_reason_codes();
    let mut previous_effective = 0i64;
    for (index, version) in document.versions.iter().enumerate() {
        let number = version.version;
        if number as usize != index + 1 {
            issues.push(PolicyIssue::new(
                PolicyIssueCode::VersionSequence,
                Some(number),
            ));
        }
        if version.effective_from < previous_effective {
            issues.push(PolicyIssue::new(
                PolicyIssueCode::EffectiveFromOrder,
                Some(number),
            ));
        }
        previous_effective = previous_effective.max(version.effective_from);
        match &version.extends {
            None => {}
            Some(Extends::Builtin(word)) if word == "builtin" => {}
            Some(Extends::Version(earlier)) if *earlier >= 1 && *earlier < number => {}
            Some(_) => issues.push(PolicyIssue::new(
                PolicyIssueCode::InvalidExtends,
                Some(number),
            )),
        }
        if let Some(note) = &version.note {
            if note.len() > MAX_POLICY_NOTE_BYTES
                || note.chars().any(char::is_control)
                || ensure_public_text(note).is_err()
            {
                issues.push(PolicyIssue::new(PolicyIssueCode::InvalidNote, Some(number)));
            }
        }
        for (name, enabled) in &version.advice {
            if registry.protected_advice.contains(&name.as_str()) {
                issues.push(PolicyIssue::rule(
                    PolicyIssueCode::ProtectedRule,
                    number,
                    name,
                ));
            } else if let Some(rule) = registry
                .optional_advice
                .iter()
                .find(|rule| rule.id == name.as_str())
            {
                if rule.deprecated_in.is_some_and(|from| number >= from) {
                    issues.push(PolicyIssue::rule(
                        PolicyIssueCode::DeprecatedRule,
                        number,
                        rule.id,
                    ));
                } else if *enabled && ceiling.forced_off.contains(rule.id) {
                    issues.push(PolicyIssue::rule(
                        PolicyIssueCode::ConflictsWithCeiling,
                        number,
                        rule.id,
                    ));
                }
            } else {
                issues.push(PolicyIssue::new(PolicyIssueCode::UnknownRule, Some(number)));
            }
        }
        let limits = [
            (
                version.limits.max_assessments,
                ASSESSMENT_MAX_ASSESSMENTS,
                ceiling.max_assessments,
            ),
            (
                version.limits.max_advisory,
                ASSESSMENT_MAX_ADVISORY,
                ceiling.max_advisory,
            ),
            (
                version.limits.max_candidates,
                ASSESSMENT_MAX_CANDIDATES,
                ceiling.max_candidates,
            ),
            (
                version.limits.max_return_bytes,
                ASSESSMENT_MAX_RETURN_BYTES,
                ceiling.max_return_bytes,
            ),
        ];
        for (value, cap, host) in limits {
            let Some(value) = value else { continue };
            if value == 0 || value > cap {
                issues.push(PolicyIssue::new(
                    PolicyIssueCode::LimitOutOfRange,
                    Some(number),
                ));
            } else if value > host {
                issues.push(PolicyIssue::new(
                    PolicyIssueCode::ExceedsCeiling,
                    Some(number),
                ));
            }
        }
        if let Some(order) = &version.order.soft_reason_codes {
            let given: BTreeSet<&str> = order.iter().map(String::as_str).collect();
            let wanted: BTreeSet<&str> = soft.iter().copied().collect();
            if order.len() != soft.len() || given != wanted {
                issues.push(PolicyIssue::new(
                    PolicyIssueCode::InvalidOrder,
                    Some(number),
                ));
            }
        }
        if issues.len() >= MAX_ISSUES {
            issues.truncate(MAX_ISSUES);
            break;
        }
    }
    issues
}

/// A change may only add versions: every version already in the file stays exactly as it is, so what an old `as_of`
/// resolved to still resolves to the same. A rollback is a new version that restates an older one.
pub fn check_history_preserved(
    current: &PolicyDocument,
    candidate: &PolicyDocument,
) -> Result<(), PolicyIssue> {
    let same_header = current.format == candidate.format
        && current.policy_id == candidate.policy_id
        && current.scope == candidate.scope;
    if !same_header
        || candidate.versions.len() < current.versions.len()
        || candidate.versions[..current.versions.len()] != current.versions[..]
    {
        return Err(PolicyIssue::new(PolicyIssueCode::HistoryChanged, None));
    }
    Ok(())
}

/// Where the policy in force comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    /// No project policy is in force: the program's defaults (and the ceiling).
    Builtin,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveLimits {
    pub max_assessments: usize,
    pub max_advisory: usize,
    pub max_candidates: usize,
    pub max_return_bytes: usize,
}

/// The policy in force for one evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedPolicy {
    pub source: PolicySource,
    pub policy_id: String,
    /// The version in force; none for the built-in policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_from: Option<i64>,
    /// The inheritance chain, oldest first: `builtin`, then each version applied.
    pub chain: Vec<String>,
    pub disabled_advice: Vec<String>,
    pub limits: EffectiveLimits,
    pub reason_code_order: Vec<String>,
    /// What the ceiling changed or forced, by name.
    pub ceiling_applied: Vec<String>,
    #[serde(skip)]
    pub assessment: AssessmentPolicy,
}

fn policy_hash(policy: &AssessmentPolicy) -> String {
    let mut bare = policy.clone();
    bare.policy_hash = None;
    let body = serde_json::to_vec(&bare).expect("an assessment policy serializes");
    let mut hasher = Sha256::new();
    hasher.update(b"awr-assessment-policy-v1\0");
    hasher.update(&body);
    format!("sha256:{:x}", hasher.finalize())
}

/// The policy in force for `as_of`: the highest version whose `effective_from` is not after it, with the chain of
/// `extends` applied oldest first, and the ceiling applied last. No document, or none in force yet, is the built-in
/// policy. A document that is not valid is an error, never a partial policy.
pub fn resolve_policy(
    document: Option<&PolicyDocument>,
    registry: &Registry,
    ceiling: &PolicyCeiling,
    as_of: i64,
) -> Result<ResolvedPolicy, PolicyIssue> {
    if let Some(document) = document {
        if let Some(issue) = validate_document(document, registry, ceiling)
            .into_iter()
            .next()
        {
            return Err(issue);
        }
    }
    let in_force = document.and_then(|document| {
        document
            .versions
            .iter()
            .filter(|version| version.effective_from <= as_of)
            .max_by_key(|version| version.version)
            .map(|version| (document, version))
    });
    let mut chain = vec!["builtin".to_string()];
    let mut advice: BTreeMap<&str, bool> = BTreeMap::new();
    let mut limits = EffectiveLimits {
        max_assessments: ASSESSMENT_MAX_ASSESSMENTS,
        max_advisory: ASSESSMENT_MAX_ADVISORY,
        max_candidates: ASSESSMENT_MAX_CANDIDATES,
        max_return_bytes: ASSESSMENT_MAX_RETURN_BYTES,
    };
    let mut soft_order: Vec<String> = registry
        .soft_reason_codes()
        .into_iter()
        .map(String::from)
        .collect();
    if let Some((document, version)) = in_force {
        let mut path = vec![version];
        let mut next = version;
        while let Some(Extends::Version(earlier)) = &next.extends {
            next = &document.versions[*earlier as usize - 1];
            path.push(next);
        }
        for version in path.iter().rev() {
            chain.push(format!("v{}", version.version));
            for rule in &registry.optional_advice {
                if let Some(enabled) = version.advice.get(rule.id) {
                    advice.insert(rule.id, *enabled);
                }
            }
            if let Some(value) = version.limits.max_assessments {
                limits.max_assessments = value;
            }
            if let Some(value) = version.limits.max_advisory {
                limits.max_advisory = value;
            }
            if let Some(value) = version.limits.max_candidates {
                limits.max_candidates = value;
            }
            if let Some(value) = version.limits.max_return_bytes {
                limits.max_return_bytes = value;
            }
            if let Some(order) = &version.order.soft_reason_codes {
                soft_order = order.clone();
            }
        }
    }
    let mut ceiling_applied = Vec::new();
    let caps = [
        (
            "max_assessments",
            &mut limits.max_assessments,
            ceiling.max_assessments,
        ),
        (
            "max_advisory",
            &mut limits.max_advisory,
            ceiling.max_advisory,
        ),
        (
            "max_candidates",
            &mut limits.max_candidates,
            ceiling.max_candidates,
        ),
        (
            "max_return_bytes",
            &mut limits.max_return_bytes,
            ceiling.max_return_bytes,
        ),
    ];
    for (name, value, host) in caps {
        if *value > host {
            *value = host;
            ceiling_applied.push(name.to_string());
        }
    }
    let mut disabled: BTreeSet<String> = advice
        .iter()
        .filter(|(_, enabled)| !**enabled)
        .map(|(rule, _)| (*rule).to_string())
        .collect();
    for forced in &ceiling.forced_off {
        if registry
            .optional_advice
            .iter()
            .any(|rule| rule.id == forced)
            && disabled.insert(forced.clone())
        {
            ceiling_applied.push(forced.clone());
        }
    }
    // Hard guards keep the slot they have by default; the soft codes fill the other slots in the policy's order.
    let mut soft = soft_order.iter();
    let reason_code_order: Vec<String> = registry
        .reason_code_order
        .iter()
        .map(|code| {
            if registry.hard_reason_codes.contains(code) {
                (*code).to_string()
            } else {
                soft.next().cloned().unwrap_or_else(|| (*code).to_string())
            }
        })
        .collect();
    let mut assessment = AssessmentPolicy::default();
    assessment.reason_code_order = reason_code_order.clone();
    assessment.hard_reason_codes = registry
        .hard_reason_codes
        .iter()
        .map(|code| (*code).to_string())
        .collect();
    assessment.max_assessments = limits.max_assessments;
    assessment.max_advisory = limits.max_advisory;
    assessment.max_candidates = limits.max_candidates;
    assessment.max_return_bytes = limits.max_return_bytes;
    assessment.disabled_advice = disabled.iter().cloned().collect();
    let (source, policy_id, version, effective_from) = match in_force {
        Some((document, version)) => (
            PolicySource::Project,
            document.policy_id.clone(),
            Some(version.version),
            Some(version.effective_from),
        ),
        None => (
            PolicySource::Builtin,
            ASSESSMENT_POLICY_ID.to_string(),
            None,
            None,
        ),
    };
    assessment.policy_id = policy_id.clone();
    assessment.policy_version = version.unwrap_or(ASSESSMENT_POLICY_VERSION);
    let unchanged = in_force.is_none() && assessment == AssessmentPolicy::default();
    assessment.policy_hash = (!unchanged).then(|| policy_hash(&assessment));
    Ok(ResolvedPolicy {
        source,
        policy_id,
        version,
        effective_from,
        chain,
        disabled_advice: assessment.disabled_advice.clone(),
        limits,
        reason_code_order,
        ceiling_applied,
        assessment,
    })
}

/// One difference between two resolved policies, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyChange {
    pub field: String,
    pub before: String,
    pub after: String,
}

/// What a change does to the policy in force, field by field. Pure; used by the preview.
pub fn describe_change(before: &ResolvedPolicy, after: &ResolvedPolicy) -> Vec<PolicyChange> {
    let mut changes = Vec::new();
    let mut push = |field: &str, before: String, after: String| {
        if before != after {
            changes.push(PolicyChange {
                field: field.into(),
                before,
                after,
            });
        }
    };
    push(
        "policy",
        format!(
            "{} v{}",
            before.policy_id,
            before.version.map_or("-".into(), |v| v.to_string())
        ),
        format!(
            "{} v{}",
            after.policy_id,
            after.version.map_or("-".into(), |v| v.to_string())
        ),
    );
    for code in OPTIONAL_ADVICE {
        let state = |policy: &ResolvedPolicy| {
            if policy.disabled_advice.iter().any(|c| c == code) {
                "off"
            } else {
                "on"
            }
            .to_string()
        };
        push(&format!("advice.{code}"), state(before), state(after));
    }
    push(
        "limits.max_assessments",
        before.limits.max_assessments.to_string(),
        after.limits.max_assessments.to_string(),
    );
    push(
        "limits.max_advisory",
        before.limits.max_advisory.to_string(),
        after.limits.max_advisory.to_string(),
    );
    push(
        "limits.max_candidates",
        before.limits.max_candidates.to_string(),
        after.limits.max_candidates.to_string(),
    );
    push(
        "limits.max_return_bytes",
        before.limits.max_return_bytes.to_string(),
        after.limits.max_return_bytes.to_string(),
    );
    push(
        "order.reason_codes",
        before.reason_code_order.join(","),
        after.reason_code_order.join(","),
    );
    changes
}
