use crate::{Freshness, Revision, Rule, ScopeKind, Source};
use globset::GlobBuilder;
use serde::{Deserialize, Serialize};

/// A projection and its source state from the same database snapshot.
/// Freshness is based on the latest source scan/index, not a live filesystem check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Projected<T> {
    pub item: T,
    pub source: Source,
    pub project_revision: Revision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyGraph {
    pub work_item_key: String,
    pub required_only: bool,
    pub dependencies: Vec<Projected<crate::WorkItem>>,
    pub edges: Vec<Projected<crate::Edge>>,
    pub missing_keys: Vec<String>,
    pub cycle_keys: Vec<String>,
    pub project_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReadinessDiagnostic {
    pub code: String,
    pub work_item_key: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkReadiness {
    pub work: Projected<crate::WorkItem>,
    pub ready: bool,
    pub dependencies: DependencyGraph,
    pub active_claims: Vec<crate::Claim>,
    pub diagnostics: Vec<ReadinessDiagnostic>,
    pub branch_id: Option<crate::Id>,
    pub evaluated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadyReport {
    pub ready: Vec<WorkReadiness>,
    /// All nonterminal items that cannot currently be selected, with explicit reasons.
    pub blocked: Vec<WorkReadiness>,
    pub project_revision: Revision,
    pub branch_id: Option<crate::Id>,
    pub evaluated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedDecision {
    pub decision: Projected<crate::Decision>,
    pub relevance: Applicability,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceDraft {
    pub external_key: String,
    pub work_item_key: Option<String>,
    pub evidence_type: String,
    pub level: crate::EvidenceLevel,
    pub summary: String,
    pub locator: String,
    pub sha256: Option<String>,
    pub source_sha: Option<String>,
    pub command: Option<String>,
    pub scope: Vec<String>,
    pub branch_id: Option<crate::Id>,
    pub verified_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub item: crate::Evidence,
    pub source: Option<Source>,
    pub project_revision: Revision,
}

/// Relative to an explicitly requested source SHA and branch. This is not independent verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceCurrency {
    Current,
    Historical,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceAssessment {
    pub evidence: EvidenceRecord,
    pub currency: EvidenceCurrency,
    pub missing_bindings: Vec<String>,
    pub reasons: Vec<String>,
}

/// Presentation groups do not merge records, transfer bindings or infer verification.
/// Only byte-identical locator strings group together; aliases are not resolved.
pub fn evidence_groups(assessments: &[EvidenceAssessment]) -> Vec<serde_json::Value> {
    let mut groups = std::collections::BTreeMap::<&str, Vec<serde_json::Value>>::new();
    for assessment in assessments {
        let item = &assessment.evidence.item;
        groups
            .entry(&item.locator)
            .or_default()
            .push(serde_json::json!({
                "external_key": item.external_key, "evidence_type": item.evidence_type,
                "level": item.level, "currency": assessment.currency,
                "missing_bindings": assessment.missing_bindings, "reasons": assessment.reasons,
                "sha256": item.sha256, "source_sha": item.source_sha,
                "scope": item.scope, "branch_id": item.branch_id,
                "source_ref": item.source_ref, "verified_at": item.verified_at,
            }));
    }
    groups
        .into_iter()
        .map(|(locator, records)| {
            serde_json::json!({
                "locator":locator, "records":records,
            })
        })
        .collect()
}

pub fn is_source_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

impl EvidenceRecord {
    /// Assess currency relative to an explicitly requested source state.
    ///
    /// Without a verified work source, matching sections cannot override a source
    /// SHA mismatch. Use `assess_for_work` to compare ledger-snapshot evidence
    /// across unrelated edits to the same ledger. A section remains an additional
    /// contract binding, not a replacement for exact acceptance/report validation.
    pub fn assess(
        self,
        source_sha: Option<&str>,
        section_fp: Option<&str>,
        branch: Option<crate::Id>,
    ) -> EvidenceAssessment {
        self.assess_inner(source_sha, section_fp, None, branch)
    }

    /// Work-aware currency can reuse evidence whose recorded SHA identified the
    /// ledger snapshot at registration, as attested by its stored section stamp.
    /// Only a request for this work's current ledger snapshot may use that stamp
    /// across SHA drift. Git/code revision requests retain strict SHA equality.
    /// Callers must also verify the work source's freshness before using Current.
    pub fn assess_for_work(
        self,
        source_sha: Option<&str>,
        source_ref: &crate::SourceRef,
        branch: Option<crate::Id>,
    ) -> EvidenceAssessment {
        self.assess_inner(
            source_sha,
            source_ref.section_fingerprint.as_deref(),
            source_ref.source_fingerprint.strip_prefix("sha256:"),
            branch,
        )
    }

    fn assess_inner(
        self,
        source_sha: Option<&str>,
        section_fp: Option<&str>,
        ledger_sha: Option<&str>,
        branch: Option<crate::Id>,
    ) -> EvidenceAssessment {
        let mut reasons = Vec::new();
        let mut missing = Vec::new();
        if self.item.scope.is_empty() {
            missing.push("scope".into());
        }
        if self.item.locator.trim().is_empty() {
            missing.push("report".into());
        }
        if !self
            .item
            .sha256
            .as_ref()
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            missing.push("sha256".into());
        }
        if !self
            .item
            .source_sha
            .as_ref()
            .is_some_and(|s| is_source_sha(s))
        {
            missing.push("source_sha".into());
        }
        if !self
            .item
            .command
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            missing.push("command".into());
        }
        if self.item.verified_at.is_none() {
            missing.push("verified_at".into());
        }
        let stale = || {
            self.source
                .as_ref()
                .is_some_and(|s| s.freshness != Freshness::Fresh)
        };
        // record_evidence stamps a section only when source_sha identifies the
        // then-current ledger snapshot. Never extend this exception to an
        // arbitrary requested revision, or infer a stamp for legacy evidence.
        let same_ledger_contract = matches!(
            (self.item.source_sha.as_deref(), source_sha, ledger_sha,
             self.item.section_fingerprint.as_deref(), section_fp),
            (Some(recorded), Some(requested), Some(current), Some(before), Some(after))
                if recorded.len() == 64 && current.len() == 64
                    && requested.eq_ignore_ascii_case(current)
                    && before.eq_ignore_ascii_case(after)
        );
        let currency = if self.item.branch_id != branch {
            reasons.push("evidence belongs to a different branch".into());
            EvidenceCurrency::Historical
        } else if let (Some(recorded), Some(requested)) = (&self.item.source_sha, source_sha) {
            if !is_source_sha(recorded) || !is_source_sha(requested) {
                reasons.push("source SHA is not a full hash".into());
                EvidenceCurrency::Unknown
            } else if !recorded.eq_ignore_ascii_case(requested) && !same_ledger_contract {
                reasons.push("evidence was recorded for a different source SHA".into());
                EvidenceCurrency::Historical
            } else if let (Some(recorded_section), Some(requested_section)) =
                (self.item.section_fingerprint.as_deref(), section_fp)
            {
                if !recorded_section.eq_ignore_ascii_case(requested_section) {
                    reasons.push(
                        "evidence was recorded against a different work section (identity+acceptance)"
                            .into(),
                    );
                    EvidenceCurrency::Historical
                } else if stale() {
                    reasons.push("evidence source is not fresh".into());
                    EvidenceCurrency::Unknown
                } else {
                    if !recorded.eq_ignore_ascii_case(requested) {
                        reasons.push(
                            "ledger snapshot changed; the stamped work section is unchanged".into(),
                        );
                    }
                    EvidenceCurrency::Current
                }
            } else if stale() {
                reasons.push("evidence source is not fresh".into());
                EvidenceCurrency::Unknown
            } else {
                EvidenceCurrency::Current
            }
        } else if let (Some(recorded_section), Some(requested_section)) =
            (self.item.section_fingerprint.as_deref(), section_fp)
        {
            if !recorded_section.eq_ignore_ascii_case(requested_section) {
                reasons.push(
                    "evidence was recorded against a different work section (identity+acceptance)"
                        .into(),
                );
                EvidenceCurrency::Historical
            } else if stale() {
                reasons.push("evidence source is not fresh".into());
                EvidenceCurrency::Unknown
            } else {
                EvidenceCurrency::Current
            }
        } else {
            reasons.push(
                "both recorded and requested source SHA are needed to determine currency".into(),
            );
            EvidenceCurrency::Unknown
        };
        EvidenceAssessment {
            evidence: self,
            currency,
            missing_bindings: missing,
            reasons,
        }
    }
}

/// None means context was not supplied; Some([]) means a known empty set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleContext {
    pub project_key: Option<String>,
    pub paths: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub work_item_key: Option<String>,
    pub agent_id: Option<String>,
}

/// Concrete execution paths only. A directory/glob is an area, not proof of which files are touched.
/// None is unknown; Some([]) is an explicitly known empty scope.
pub fn concrete_scope_paths(paths: Option<&[String]>) -> Option<Vec<String>> {
    let paths = paths?;
    if paths.iter().any(|p| {
        p.trim().is_empty() || p.ends_with('/') || p.contains(['*', '?', '[', ']', '{', '}'])
    }) {
        return None;
    }
    Some(
        paths
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applicability {
    Applicable,
    NotApplicable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleMatch {
    pub rule: Projected<Rule>,
    pub applicability: Applicability,
    pub reasons: Vec<String>,
}

/// Match project-relative paths, with case-sensitive globs and literal separators.
/// A trailing slash selects a directory's descendants. Invalid/unsafe paths are unknown.
pub fn match_path_scope(pattern: &str, paths: &[String]) -> std::result::Result<bool, String> {
    fn relative(path: &str) -> bool {
        !path.is_empty()
            && !path.starts_with('/')
            && !path.contains('\\')
            && !path.contains(':')
            && !path.split('/').any(|part| part == "..")
    }
    if !relative(pattern) || paths.iter().any(|path| !relative(path)) {
        return Err("path scope and context must use project-relative forward-slash paths".into());
    }
    let clean = |path: &str| {
        path.split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut pattern_clean = clean(pattern);
    if pattern.ends_with('/') {
        pattern_clean.push_str("/**");
    }
    let matcher = GlobBuilder::new(&pattern_clean)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .map_err(|e| format!("invalid path scope: {e}"))?
        .compile_matcher();
    Ok(paths.iter().any(|path| matcher.is_match(clean(path))))
}

impl Projected<Rule> {
    /// Keep every rule in the result, including unresolved metadata and missing context.
    pub fn evaluate(self, context: &RuleContext) -> RuleMatch {
        let mut reasons = self.item.unresolved.clone();
        let scope_match = match &self.item.scope {
            None => Err("rule scope is unknown".into()),
            Some(scope) if scope.value.trim().is_empty() => Err("rule scope value is empty".into()),
            Some(scope) => {
                let equals = |value: &Option<String>, label: &str| {
                    value
                        .as_ref()
                        .map(|value| value == &scope.value)
                        .ok_or_else(|| format!("{label} context is unknown"))
                };
                match scope.kind {
                    ScopeKind::Project if scope.value == "*" => Ok(true),
                    ScopeKind::Project => equals(&context.project_key, "project"),
                    ScopeKind::WorkItem => equals(&context.work_item_key, "work item"),
                    ScopeKind::Agent => equals(&context.agent_id, "agent"),
                    ScopeKind::Tag => context
                        .tags
                        .as_ref()
                        .map(|tags| tags.contains(&scope.value))
                        .ok_or_else(|| "tag context is unknown".into()),
                    ScopeKind::Path => context
                        .paths
                        .as_ref()
                        .ok_or_else(|| "path context is unknown".into())
                        .and_then(|paths| match_path_scope(&scope.value, paths)),
                }
            }
        };
        if self.item.severity.is_none() {
            reasons.push("rule severity is unknown".into());
        }
        if self.source.freshness != Freshness::Fresh {
            reasons.push(format!("rule source is {:?}", self.source.freshness));
        }
        let applicability = match scope_match {
            Ok(false) if self.source.freshness == Freshness::Fresh => Applicability::NotApplicable,
            Ok(true) if reasons.is_empty() => Applicability::Applicable,
            Err(reason) => {
                reasons.push(reason);
                Applicability::Unknown
            }
            _ => Applicability::Unknown,
        };
        reasons.sort();
        reasons.dedup();
        RuleMatch {
            rule: self,
            applicability,
            reasons,
        }
    }
}
