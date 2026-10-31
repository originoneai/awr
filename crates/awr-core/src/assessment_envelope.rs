//! Typed AssessmentEnvelope and unknown/conflict semantics (DEC-012).
//!
//! Pure composition from fixed inputs: no Store, filesystem, network, model,
//! Git, or implicit wall-clock. Reuses [`ManagementDecision`] and
//! [`FactSnapshot`]; does not invent a second rule engine or expression DSL.
//!
//! Boundaries:
//! - missing fields stay missing (never zero-filled)
//! - `unknown` never becomes `false` / low-risk
//! - hard-rule rejects cannot be offset by heuristic scores
//! - coverage / gaps / conflicts / applicability stay separate (no confidence)
use crate::{
    ActionGuidance, Error, FactSnapshot, ManagementDecision, ManagementMode, Result,
    SnapshotCoverage, ensure_public_data,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const ASSESSMENT_ENVELOPE_SCHEMA_ID: &str = "awr-assessment-envelope-v1";
pub const ASSESSMENT_ENVELOPE_SCHEMA_VERSION: u32 = 1;
pub const ASSESSMENT_PROFILE_MANAGEMENT_ACTION: &str = "management_and_action_rationale";
pub const ASSESSMENT_POLICY_ID: &str = "tip_management_v1";
pub const ASSESSMENT_POLICY_VERSION: u32 = 1;

/// Hard ceilings for a single envelope assembly. Callers may tighten, never raise.
pub const ASSESSMENT_MAX_ASSESSMENTS: usize = 64;
pub const ASSESSMENT_MAX_ADVISORY: usize = 32;
pub const ASSESSMENT_MAX_CANDIDATES: usize = 256;
pub const ASSESSMENT_MAX_RETURN_BYTES: usize = 256 * 1024;

/// Closed advisory action set (DEC-010); machine enums, not free-form scripts.
pub const ADVISORY_ACTION_CODES: &[&str] = &[
    "continue_prepare_current_work",
    "query_original_operation_result",
    "collect_user_reply",
    "refresh_sources",
    "repair_required_materials",
    "inspect_claim_conflict",
    "request_human_review",
    "record_management_observations",
    "no_advisory",
];

/// Deterministic default reason-code order (earlier = higher priority / sorts first).
/// Normative: stop / revoke / unknown-effect first, then gaps, then advisories.
pub const DEFAULT_REASON_CODE_ORDER: &[&str] = &[
    "execution_result_requires_query",
    "query_outcome_before_retry",
    "work_contract_changed",
    "unresolved_dependencies",
    "plan_invalidated",
    "persistent_wait",
    "deferred_wait",
    "work_handoff",
    "cross_session_resume",
    "handoff_or_collaboration",
    "multiple_outcomes",
    "scope_requires_planning",
    "independent_work_units",
    "continuous_management_retained",
    "claim_conflict",
    "hard_guard_reject",
    "missing_required_field",
    "conflicting_signals",
    "resource_limit_exceeded",
];

/// Reason codes treated as hard guards: cannot be canceled by heuristic scores.
pub const DEFAULT_HARD_REASON_CODES: &[&str] = &[
    "execution_result_requires_query",
    "query_outcome_before_retry",
    "work_contract_changed",
    "unresolved_dependencies",
    "plan_invalidated",
    "claim_conflict",
    "hard_guard_reject",
    "resource_limit_exceeded",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentSupport {
    /// Evidence supports a determinate conclusion for this question.
    Supported,
    /// Evidence is insufficient; must not become false / low-risk.
    Unknown,
    /// Conflicting evidence; preserve both sides via basis_refs.
    Conflicting,
    /// Question or field is outside the current profile / capability.
    Unsupported,
}

impl AssessmentSupport {
    /// Conflict-priority rank for aggregation (higher wins).
    pub fn conflict_priority(self) -> u8 {
        match self {
            Self::Conflicting => 4,
            Self::Unknown => 3,
            Self::Unsupported => 2,
            Self::Supported => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerStatus {
    Supported,
    Unknown,
    Conflicting,
    Unsupported,
    /// Layer was not run; never coerce to false.
    NotEvaluated,
}

impl LayerStatus {
    pub fn conflict_priority(self) -> u8 {
        match self {
            Self::Conflicting => 4,
            Self::Unknown => 3,
            Self::Unsupported => 2,
            Self::Supported => 1,
            Self::NotEvaluated => 0,
        }
    }

    pub fn from_support(support: AssessmentSupport) -> Self {
        match support {
            AssessmentSupport::Supported => Self::Supported,
            AssessmentSupport::Unknown => Self::Unknown,
            AssessmentSupport::Conflicting => Self::Conflicting,
            AssessmentSupport::Unsupported => Self::Unsupported,
        }
    }

    /// Combine with conflict priority; `NotEvaluated` never upgrades evidence.
    pub fn merge(self, other: Self) -> Self {
        if other.conflict_priority() > self.conflict_priority() {
            other
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentLayerId {
    WorkReadiness,
    ExecutionAdmission,
    ContextCompleteness,
    DeliveryObservation,
    CompletionValidity,
}

impl AssessmentLayerId {
    pub const ALL: [AssessmentLayerId; 5] = [
        Self::WorkReadiness,
        Self::ExecutionAdmission,
        Self::ContextCompleteness,
        Self::DeliveryObservation,
        Self::CompletionValidity,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkReadiness => "work_readiness",
            Self::ExecutionAdmission => "execution_admission",
            Self::ContextCompleteness => "context_completeness",
            Self::DeliveryObservation => "delivery_observation",
            Self::CompletionValidity => "completion_validity",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_fingerprint: Option<String>,
    pub policy_id: String,
    pub policy_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_main_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_snapshot_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerResult {
    pub status: LayerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub basis_refs: Vec<String>,
}

impl LayerResult {
    pub fn not_evaluated() -> Self {
        Self {
            status: LayerStatus::NotEvaluated,
            summary: None,
            basis_refs: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentItem {
    pub id: String,
    pub support: AssessmentSupport,
    /// Original sub-assessment conclusion; never rewritten to false/low-risk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<Value>,
    #[serde(default)]
    pub reason_codes: Vec<String>,
    #[serde(default)]
    pub basis_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<AssessmentLayerId>,
    /// Soft ranking units only — never calibrated probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heuristic_score: Option<i64>,
    /// When true, scores cannot override this item's reject/unknown semantics.
    #[serde(default)]
    pub hard_reject: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceQuality {
    #[serde(default)]
    pub required_fields: Vec<String>,
    #[serde(default)]
    pub observed_fields: Vec<String>,
    #[serde(default)]
    pub missing: Vec<String>,
    #[serde(default)]
    pub stale: Vec<String>,
    #[serde(default)]
    pub conflicting: Vec<String>,
    #[serde(default)]
    pub host_asserted_only: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<String>,
    /// Field-count ratio note — not probability.
    pub coverage_note: String,
    /// Explicit applicability / scope of this evidence block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applicability: Option<String>,
}

impl EvidenceQuality {
    pub fn from_snapshot_coverage(
        coverage: &SnapshotCoverage,
        applicability: Option<String>,
    ) -> Self {
        Self {
            required_fields: coverage.required_fields.clone(),
            observed_fields: coverage.observed_fields.clone(),
            missing: coverage.missing.clone(),
            stale: coverage.stale.clone(),
            conflicting: coverage.conflicting.clone(),
            host_asserted_only: coverage.host_asserted_only.clone(),
            unsupported: coverage.unsupported.clone(),
            coverage_note: coverage.coverage_note.clone(),
            applicability,
        }
    }

    pub fn empty(applicability: Option<String>) -> Self {
        Self {
            required_fields: vec![],
            observed_fields: vec![],
            missing: vec![],
            stale: vec![],
            conflicting: vec![],
            host_asserted_only: vec![],
            unsupported: vec![],
            coverage_note: "coverage is a field count ratio, not probability".into(),
            applicability,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvisoryAction {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default)]
    pub query_refs: Vec<String>,
    #[serde(default)]
    pub limits: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reevaluation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_ops: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_budget_ms: Option<u64>,
    pub read_scope: String,
    pub omitted_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionRationaleView {
    pub when: String,
    pub basis: String,
    pub next_action: String,
    pub recheck: String,
    pub max_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_view: Option<String>,
}

impl From<&ActionGuidance> for ActionRationaleView {
    fn from(g: &ActionGuidance) -> Self {
        Self {
            when: g.when.clone(),
            basis: g.basis.clone(),
            next_action: g.next_action.clone(),
            recheck: g.recheck.clone(),
            max_bytes: crate::ACTION_GUIDANCE_MAX_BYTES,
            response_view: Some("action".into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementEnvelopeView {
    pub decision: ManagementDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
    pub observation_basis: String,
    #[serde(default)]
    pub admission_gaps: Vec<String>,
    pub record_required: bool,
    pub next_action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyCompat {
    pub compatible: bool,
    #[serde(default)]
    pub source_views: Vec<String>,
    #[serde(default)]
    pub omitted_fields: Vec<String>,
}

/// Overall hard-gate outcome after compose. Soft scores never flip a reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardGateOutcome {
    /// No hard reject fired; soft scores may rank advisories only.
    Pass,
    /// At least one hard-rule reject; scores cannot offset.
    Reject,
    /// Hard path requires more evidence; not a false/low-risk pass.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentEnvelope {
    pub schema_id: String,
    pub schema_version: u32,
    pub assessment_profile: String,
    pub identity: AssessmentIdentity,
    pub layers: BTreeMap<String, LayerResult>,
    pub assessments: Vec<AssessmentItem>,
    pub evidence_quality: EvidenceQuality,
    pub advisory_actions: Vec<AdvisoryAction>,
    pub limits: AssessmentLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management: Option<ManagementEnvelopeView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_rationale: Option<ActionRationaleView>,
    pub legacy: LegacyCompat,
    #[serde(default)]
    pub unsupported_fields: BTreeMap<String, String>,
    pub hard_gate: HardGateOutcome,
    /// Canonical semantic hash of the envelope (excludes wall-clock receipts).
    pub assessment_hash: String,
}

/// Versioned policy for deterministic compose. No expression DSL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentPolicy {
    pub policy_id: String,
    pub policy_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_hash: Option<String>,
    /// Earlier codes sort / surface first.
    pub reason_code_order: Vec<String>,
    /// Codes that hard-reject and cannot be offset by scores.
    pub hard_reason_codes: Vec<String>,
    pub max_assessments: usize,
    pub max_advisory: usize,
    pub max_candidates: usize,
    pub max_return_bytes: usize,
    /// Optional advice this policy withholds (DEC-040). Protected advice can never be named here; empty, and then
    /// absent from the serialized policy, for every policy that existed before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_advice: Vec<String>,
}

impl Default for AssessmentPolicy {
    fn default() -> Self {
        Self {
            policy_id: ASSESSMENT_POLICY_ID.into(),
            policy_version: ASSESSMENT_POLICY_VERSION,
            policy_hash: None,
            reason_code_order: DEFAULT_REASON_CODE_ORDER
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            hard_reason_codes: DEFAULT_HARD_REASON_CODES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            max_assessments: ASSESSMENT_MAX_ASSESSMENTS,
            max_advisory: ASSESSMENT_MAX_ADVISORY,
            max_candidates: ASSESSMENT_MAX_CANDIDATES,
            max_return_bytes: ASSESSMENT_MAX_RETURN_BYTES,
            disabled_advice: Vec::new(),
        }
    }
}

impl AssessmentPolicy {
    pub fn capped(self) -> Result<Self> {
        if self.max_assessments == 0
            || self.max_advisory == 0
            || self.max_candidates == 0
            || self.max_return_bytes == 0
            || self.max_assessments > ASSESSMENT_MAX_ASSESSMENTS
            || self.max_advisory > ASSESSMENT_MAX_ADVISORY
            || self.max_candidates > ASSESSMENT_MAX_CANDIDATES
            || self.max_return_bytes > ASSESSMENT_MAX_RETURN_BYTES
        {
            return Err(Error::InvalidInput(format!(
                "assessment policy limits must be within 1..={}/{}/{}/{}",
                ASSESSMENT_MAX_ASSESSMENTS,
                ASSESSMENT_MAX_ADVISORY,
                ASSESSMENT_MAX_CANDIDATES,
                ASSESSMENT_MAX_RETURN_BYTES
            )));
        }
        if self.policy_id.trim().is_empty() {
            return Err(Error::InvalidInput(
                "assessment policy_id must be nonempty".into(),
            ));
        }
        if self
            .disabled_advice
            .iter()
            .any(|code| !crate::OPTIONAL_ADVICE.contains(&code.as_str()))
        {
            return Err(Error::InvalidInput(
                "assessment policy may withhold only optional advice".into(),
            ));
        }
        Ok(self)
    }

    /// Whether an advice code is withheld by this policy.
    pub fn withholds(&self, code: &str) -> bool {
        self.disabled_advice.iter().any(|c| c == code)
    }

    pub fn is_hard_reason(&self, code: &str) -> bool {
        self.hard_reason_codes.iter().any(|c| c == code)
    }

    pub fn reason_rank(&self, code: &str) -> usize {
        self.reason_code_order
            .iter()
            .position(|c| c == code)
            .unwrap_or(usize::MAX)
    }
}

/// Fixed inputs for pure envelope composition. No I/O.
#[derive(Debug, Clone)]
pub struct AssessmentComposeInput {
    pub identity: AssessmentIdentity,
    pub as_of: i64,
    pub policy: AssessmentPolicy,
    pub fact_snapshot: Option<FactSnapshot>,
    pub management: Option<ManagementDecision>,
    pub management_observation: Option<Value>,
    pub management_observation_basis: Option<String>,
    pub management_admission_gaps: Vec<String>,
    pub management_record_required: Option<bool>,
    pub management_next_action: Option<String>,
    pub action_rationale: Option<ActionGuidance>,
    /// Explicit layer results; missing layers stay `not_evaluated`.
    pub layers: BTreeMap<AssessmentLayerId, LayerResult>,
    pub assessments: Vec<AssessmentItem>,
    pub advisory_actions: Vec<AdvisoryAction>,
    pub evidence_quality: Option<EvidenceQuality>,
    pub evidence_applicability: Option<String>,
    pub unsupported_fields: BTreeMap<String, String>,
    pub legacy_source_views: Vec<String>,
    pub candidates_seen: usize,
    pub scan_ops: Option<u64>,
    pub time_budget_ms: Option<u64>,
    pub read_scope: String,
}

/// Sort reason codes by policy order, then lexicographically for stability.
pub fn sort_reason_codes(policy: &AssessmentPolicy, codes: &mut [String]) {
    codes.sort_by(|a, b| {
        policy
            .reason_rank(a)
            .cmp(&policy.reason_rank(b))
            .then_with(|| a.cmp(b))
    });
}

/// Stable sort of assessments: hard rejects first, then conflict priority,
/// then policy reason rank of the primary reason, then id.
pub fn sort_assessments(policy: &AssessmentPolicy, items: &mut [AssessmentItem]) {
    for item in items.iter_mut() {
        sort_reason_codes(policy, &mut item.reason_codes);
        item.basis_refs.sort();
    }
    items.sort_by(|a, b| {
        b.hard_reject
            .cmp(&a.hard_reject)
            .then_with(|| {
                b.support
                    .conflict_priority()
                    .cmp(&a.support.conflict_priority())
            })
            .then_with(|| {
                let ar = a
                    .reason_codes
                    .first()
                    .map(|c| policy.reason_rank(c))
                    .unwrap_or(usize::MAX);
                let br = b
                    .reason_codes
                    .first()
                    .map(|c| policy.reason_rank(c))
                    .unwrap_or(usize::MAX);
                ar.cmp(&br)
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn validate_advisory(action: &AdvisoryAction) -> Result<()> {
    ensure_public_data(action)?;
    if !ADVISORY_ACTION_CODES.contains(&action.code.as_str()) {
        return Err(Error::InvalidInput(format!(
            "advisory action code {} is not in the frozen advisory set",
            action.code
        )));
    }
    Ok(())
}

fn validate_assessment_item(item: &AssessmentItem) -> Result<()> {
    ensure_public_data(item)?;
    if item.id.trim().is_empty() || item.id.len() > 4096 {
        return Err(Error::InvalidInput(
            "assessment id must be a nonempty bounded name".into(),
        ));
    }
    // Forbidden: uncalibrated confidence / probability fields on conclusions.
    if let Some(Value::Object(map)) = &item.conclusion {
        for key in [
            "confidence",
            "probability",
            "calibrated_probability",
            "p_success",
        ] {
            if map.contains_key(key) {
                return Err(Error::InvalidInput(format!(
                    "assessment {} must not emit uncalibrated {} on conclusions",
                    item.id, key
                )));
            }
        }
    }
    // Unknown must not be represented as false / low-risk conclusion.
    if item.support == AssessmentSupport::Unknown {
        if item.conclusion == Some(Value::Bool(false)) {
            return Err(Error::InvalidInput(format!(
                "assessment {} with support=unknown must not coerce conclusion to false",
                item.id
            )));
        }
        if item.conclusion.as_ref().and_then(|v| v.as_str()) == Some("low_risk")
            || item.conclusion.as_ref().and_then(|v| v.as_str()) == Some("low")
        {
            return Err(Error::InvalidInput(format!(
                "assessment {} with support=unknown must not coerce conclusion to low-risk",
                item.id
            )));
        }
    }
    Ok(())
}

fn derive_management_assessment(decision: &ManagementDecision) -> AssessmentItem {
    let support = match decision.mode {
        ManagementMode::Undetermined => AssessmentSupport::Unknown,
        ManagementMode::Lightweight | ManagementMode::Continuous => AssessmentSupport::Supported,
    };
    let mut reason_codes: Vec<String> = decision.reasons.iter().map(|r| r.code.clone()).collect();
    reason_codes.sort();
    reason_codes.dedup();
    AssessmentItem {
        id: "management_mode".into(),
        support,
        conclusion: Some(json!(match decision.mode {
            ManagementMode::Undetermined => "undetermined",
            ManagementMode::Lightweight => "lightweight",
            ManagementMode::Continuous => "continuous",
        })),
        reason_codes,
        basis_refs: if decision.unknown_observations.is_empty() {
            decision
                .reasons
                .iter()
                .map(|r| format!("management.decision.reasons.{}", r.code))
                .collect()
        } else {
            vec!["management.decision.unknown_observations".into()]
        },
        layer: None,
        heuristic_score: None,
        hard_reject: decision
            .reasons
            .iter()
            .any(|r| DEFAULT_HARD_REASON_CODES.contains(&r.code.as_str())),
    }
}

fn default_layers_from_management(
    decision: &ManagementDecision,
) -> BTreeMap<AssessmentLayerId, LayerResult> {
    let mut layers = BTreeMap::new();
    for id in AssessmentLayerId::ALL {
        layers.insert(id, LayerResult::not_evaluated());
    }
    layers.insert(
        AssessmentLayerId::ExecutionAdmission,
        LayerResult {
            status: LayerStatus::Supported,
            summary: Some(decision.execution_admission.clone()),
            basis_refs: vec!["management.decision.execution_admission".into()],
        },
    );
    layers.insert(
        AssessmentLayerId::CompletionValidity,
        LayerResult {
            status: LayerStatus::Supported,
            summary: Some(decision.completion_policy.clone()),
            basis_refs: vec!["management.decision.completion_policy".into()],
        },
    );
    layers
}

fn merge_layers_from_assessments(
    layers: &mut BTreeMap<AssessmentLayerId, LayerResult>,
    assessments: &[AssessmentItem],
) {
    for item in assessments {
        let Some(layer_id) = item.layer else {
            continue;
        };
        let entry = layers
            .entry(layer_id)
            .or_insert_with(LayerResult::not_evaluated);
        let from_item = LayerStatus::from_support(item.support);
        entry.status = entry.status.merge(from_item);
        for r in &item.basis_refs {
            if !entry.basis_refs.contains(r) {
                entry.basis_refs.push(r.clone());
            }
        }
        entry.basis_refs.sort();
        entry.basis_refs.dedup();
        if entry.summary.is_none() {
            if let Some(Value::String(s)) = &item.conclusion {
                entry.summary = Some(s.clone());
            }
        }
    }
}

fn compute_hard_gate(policy: &AssessmentPolicy, assessments: &[AssessmentItem]) -> HardGateOutcome {
    let mut saw_unknown_hard = false;
    for item in assessments {
        let has_hard_code = item.reason_codes.iter().any(|c| policy.is_hard_reason(c));
        let is_hard = item.hard_reject || has_hard_code;
        if !is_hard {
            continue;
        }
        // Conflicting hard evidence or an explicit hard_reject always Reject.
        if item.hard_reject || item.support == AssessmentSupport::Conflicting {
            return HardGateOutcome::Reject;
        }
        match item.support {
            AssessmentSupport::Unknown => {
                // Hard path needs more evidence — not a false/low-risk pass.
                saw_unknown_hard = true;
            }
            AssessmentSupport::Supported | AssessmentSupport::Unsupported => {
                // Determinate hard reason codes block; soft scores cannot clear this.
                return HardGateOutcome::Reject;
            }
            AssessmentSupport::Conflicting => return HardGateOutcome::Reject,
        }
    }
    if saw_unknown_hard {
        HardGateOutcome::Unknown
    } else {
        HardGateOutcome::Pass
    }
}

/// Soft scores may rank advisories only when the hard gate is Pass.
/// They never flip Reject / Unknown to Pass.
pub fn soft_scores_may_rank(hard_gate: HardGateOutcome) -> bool {
    hard_gate == HardGateOutcome::Pass
}

/// Canonical semantic hash. Same envelope fields always hash the same way.
pub fn canonical_assessment_hash(envelope: &AssessmentEnvelope) -> Result<String> {
    assessment_hash_bytes(envelope)
}

fn assessment_hash_bytes(envelope: &AssessmentEnvelope) -> Result<String> {
    let payload = json!({
        "schema_id": envelope.schema_id,
        "schema_version": envelope.schema_version,
        "assessment_profile": envelope.assessment_profile,
        "identity": envelope.identity,
        "layers": envelope.layers,
        "assessments": envelope.assessments,
        "evidence_quality": envelope.evidence_quality,
        "advisory_actions": envelope.advisory_actions,
        "limits": {
            "candidates": envelope.limits.candidates,
            "scan_ops": envelope.limits.scan_ops,
            "return_bytes": envelope.limits.return_bytes,
            "time_budget_ms": envelope.limits.time_budget_ms,
            "read_scope": envelope.limits.read_scope,
            "omitted_count": envelope.limits.omitted_count,
            "truncation_reason": envelope.limits.truncation_reason,
        },
        "management": envelope.management,
        "action_rationale": envelope.action_rationale,
        "legacy": envelope.legacy,
        "unsupported_fields": envelope.unsupported_fields,
        "hard_gate": envelope.hard_gate,
    });
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&payload)?)
    ))
}

/// Compose a typed AssessmentEnvelope from fixed inputs.
/// Same input + policy + as_of → identical canonical output.
pub fn compose_assessment_envelope(input: AssessmentComposeInput) -> Result<AssessmentEnvelope> {
    ensure_public_data(&input.identity)?;
    if input.as_of < 0 {
        return Err(Error::InvalidInput(
            "assessment as_of must be a non-negative timestamp".into(),
        ));
    }
    if input.read_scope.trim().is_empty() {
        return Err(Error::InvalidInput(
            "assessment requires a nonempty read_scope".into(),
        ));
    }
    let policy = input.policy.capped()?;

    for item in &input.assessments {
        validate_assessment_item(item)?;
    }
    for action in &input.advisory_actions {
        validate_advisory(action)?;
    }
    for value in input.unsupported_fields.values() {
        if value != "unknown" {
            return Err(Error::InvalidInput(
                "unsupported_fields values must be the literal \"unknown\"".into(),
            ));
        }
    }

    let mut assessments = input.assessments;
    let mut layers = BTreeMap::new();
    for id in AssessmentLayerId::ALL {
        layers.insert(
            id,
            input
                .layers
                .get(&id)
                .cloned()
                .unwrap_or_else(LayerResult::not_evaluated),
        );
    }

    let management_view = if let Some(decision) = &input.management {
        // Derive management_mode assessment if caller did not supply one.
        if !assessments.iter().any(|a| a.id == "management_mode") {
            assessments.push(derive_management_assessment(decision));
        }
        let derived_layers = default_layers_from_management(decision);
        for (id, result) in derived_layers {
            let entry = layers.entry(id).or_insert_with(LayerResult::not_evaluated);
            if entry.status == LayerStatus::NotEvaluated {
                *entry = result;
            }
        }
        let record_required = input.management_record_required.unwrap_or_else(|| {
            decision.mode == ManagementMode::Undetermined
                || !decision.unknown_observations.is_empty()
        });
        let next_action = input.management_next_action.clone().unwrap_or_else(|| {
            if record_required {
                "record_current_assessment_with_explicit_host_observations".into()
            } else {
                "follow_required_actions_and_existing_workflow".into()
            }
        });
        Some(ManagementEnvelopeView {
            decision: decision.clone(),
            observation: input.management_observation.clone(),
            observation_basis: input
                .management_observation_basis
                .clone()
                .unwrap_or_else(|| "host_assertion_not_independently_verified".into()),
            admission_gaps: input.management_admission_gaps.clone(),
            record_required,
            next_action,
        })
    } else {
        None
    };

    merge_layers_from_assessments(&mut layers, &assessments);

    // Resource limits: truncate assessments / advisories deterministically.
    let mut truncated = false;
    let mut truncation_reason = None;
    let mut omitted_count = 0usize;
    let candidates_seen = input.candidates_seen.max(assessments.len());

    if candidates_seen > policy.max_candidates {
        truncated = true;
        truncation_reason = Some("candidates_exceeded".into());
    }
    if assessments.len() > policy.max_assessments {
        truncated = true;
        truncation_reason =
            Some(truncation_reason.unwrap_or_else(|| "assessments_exceeded".into()));
        omitted_count += assessments.len() - policy.max_assessments;
        assessments.truncate(policy.max_assessments);
    }

    sort_assessments(&policy, &mut assessments);

    let mut advisory_actions = input.advisory_actions;
    let offered = advisory_actions.len();
    advisory_actions.retain(|action| !policy.withholds(&action.code));
    let withheld = offered - advisory_actions.len();
    if withheld > 0 {
        truncated = true;
        truncation_reason =
            Some(truncation_reason.unwrap_or_else(|| "advice_withheld_by_policy".into()));
        omitted_count += withheld;
        if advisory_actions.is_empty() {
            advisory_actions.push(AdvisoryAction {
                code: "no_advisory".into(),
                when: Some("advice_withheld_by_policy".into()),
                query_refs: vec![],
                limits: vec!["withheld_advice_is_not_a_finding".into()],
                reevaluation: Some("after_policy_changed".into()),
            });
        }
    }
    advisory_actions.sort_by(|a, b| a.code.cmp(&b.code));
    if advisory_actions.len() > policy.max_advisory {
        truncated = true;
        truncation_reason = Some(truncation_reason.unwrap_or_else(|| "advisory_exceeded".into()));
        omitted_count += advisory_actions.len() - policy.max_advisory;
        advisory_actions.truncate(policy.max_advisory);
    }

    // Bound return bytes without inventing values.
    let mut kept = Vec::with_capacity(assessments.len());
    let mut bytes_used = 0usize;
    for item in assessments {
        let encoded = serde_json::to_vec(&item)?;
        if bytes_used + encoded.len() > policy.max_return_bytes {
            truncated = true;
            truncation_reason =
                Some(truncation_reason.unwrap_or_else(|| "return_bytes_exceeded".into()));
            omitted_count += 1;
            continue;
        }
        bytes_used += encoded.len();
        kept.push(item);
    }
    let assessments = kept;

    // Hard gate after truncation (only kept items participate).
    let mut hard_gate = compute_hard_gate(&policy, &assessments);
    if matches!(
        truncation_reason.as_deref(),
        Some("return_bytes_exceeded")
            | Some("candidates_exceeded")
            | Some("assessments_exceeded")
            | Some("advisory_exceeded")
    ) {
        // Resource overrun is a hard condition: never report low-risk Pass that
        // pretends the scan was complete. Soft scores cannot clear this.
        if hard_gate == HardGateOutcome::Pass {
            hard_gate = HardGateOutcome::Unknown;
        }
    }
    let _scores_ok = soft_scores_may_rank(hard_gate);

    let evidence_quality = if let Some(eq) = input.evidence_quality {
        eq
    } else if let Some(snap) = &input.fact_snapshot {
        EvidenceQuality::from_snapshot_coverage(
            &snap.coverage,
            input.evidence_applicability.clone(),
        )
    } else if let Some(decision) = management_view.as_ref().map(|m| &m.decision) {
        let required: Vec<String> = vec![
            "single_outcome".into(),
            "bounded_scope".into(),
            "single_executor".into(),
            "no_deferred_wait".into(),
            "plan_valid".into(),
            "outcome_known".into(),
            "independently_schedulable_units".into(),
        ];
        let missing = decision.unknown_observations.clone();
        let observed: Vec<String> = required
            .iter()
            .filter(|f| !missing.contains(f))
            .cloned()
            .collect();
        EvidenceQuality {
            required_fields: required,
            observed_fields: observed,
            missing,
            stale: vec![],
            conflicting: vec![],
            host_asserted_only: vec![],
            unsupported: vec![],
            coverage_note: "coverage is a field count ratio, not probability".into(),
            applicability: input.evidence_applicability.clone(),
        }
    } else {
        EvidenceQuality::empty(input.evidence_applicability.clone())
    };

    // Reject accidental confidence fields on evidence_quality via serde shape —
    // EvidenceQuality has no probability fields by construction.

    let mut identity = input.identity;
    identity.policy_id = policy.policy_id.clone();
    identity.policy_version = policy.policy_version;
    if identity.policy_hash.is_none() {
        identity.policy_hash = policy.policy_hash.clone();
    }
    identity.as_of = Some(input.as_of);
    if identity.fact_snapshot_hash.is_none() {
        identity.fact_snapshot_hash = input.fact_snapshot.as_ref().map(|s| s.content_hash.clone());
    }

    let layer_map: BTreeMap<String, LayerResult> = layers
        .into_iter()
        .map(|(id, result)| (id.as_str().to_string(), result))
        .collect();

    let action_rationale = input
        .action_rationale
        .as_ref()
        .map(ActionRationaleView::from);

    let mut unsupported_fields = input.unsupported_fields;
    // Profile fields not populated stay explicit unknown.
    if management_view.is_none() {
        unsupported_fields
            .entry("management".into())
            .or_insert_with(|| "unknown".into());
    }
    if action_rationale.is_none() {
        unsupported_fields
            .entry("action_rationale".into())
            .or_insert_with(|| "unknown".into());
    }

    let limits = AssessmentLimits {
        candidates: Some(candidates_seen),
        scan_ops: input.scan_ops,
        return_bytes: Some(bytes_used),
        time_budget_ms: input.time_budget_ms,
        read_scope: input.read_scope,
        omitted_count,
        truncation_reason: if truncated { truncation_reason } else { None },
    };

    let mut envelope = AssessmentEnvelope {
        schema_id: ASSESSMENT_ENVELOPE_SCHEMA_ID.into(),
        schema_version: ASSESSMENT_ENVELOPE_SCHEMA_VERSION,
        assessment_profile: ASSESSMENT_PROFILE_MANAGEMENT_ACTION.into(),
        identity,
        layers: layer_map,
        assessments,
        evidence_quality,
        advisory_actions,
        limits,
        management: management_view,
        action_rationale,
        legacy: LegacyCompat {
            compatible: true,
            source_views: input.legacy_source_views,
            omitted_fields: vec![],
        },
        unsupported_fields,
        hard_gate,
        assessment_hash: String::new(),
    };
    envelope.assessment_hash = assessment_hash_bytes(&envelope)?;
    ensure_public_data(&envelope)?;
    Ok(envelope)
}

/// Convenience: evaluate(frozen_facts, policy, as_of) using a FactSnapshot + optional management.
pub fn evaluate_assessment(
    facts: &FactSnapshot,
    policy: AssessmentPolicy,
    as_of: i64,
    management: Option<&ManagementDecision>,
    action_rationale: Option<&ActionGuidance>,
) -> Result<AssessmentEnvelope> {
    if as_of != facts.as_of {
        // Callers may re-stamp as_of only when explicitly intended; mismatch is an error
        // so replay stays honest about the bound snapshot clock.
        return Err(Error::InvalidInput(
            "evaluate_assessment as_of must match FactSnapshot.as_of".into(),
        ));
    }
    let identity = AssessmentIdentity {
        project_id: facts.identity.project_id.clone(),
        work_key: facts.identity.work_key.clone(),
        work_id: facts.identity.work_id.clone(),
        branch_id: facts.identity.branch_id.clone(),
        contract_fingerprint: facts.identity.work_contract_hash.clone(),
        policy_id: policy.policy_id.clone(),
        policy_version: policy.policy_version,
        policy_hash: policy.policy_hash.clone().or(facts.policy_hash.clone()),
        input_summary: Some(format!("fact_snapshot:{}", facts.content_hash)),
        as_of: Some(as_of),
        verified_main_sha: None,
        fact_snapshot_hash: Some(facts.content_hash.clone()),
    };

    // Map snapshot signal conflicts / missing into assessments without coercing unknown→false.
    let mut assessments = Vec::new();
    for field in &facts.coverage.conflicting {
        assessments.push(AssessmentItem {
            id: format!("signal_conflict:{field}"),
            support: AssessmentSupport::Conflicting,
            conclusion: None,
            reason_codes: vec!["conflicting_signals".into()],
            basis_refs: vec![format!("fact_snapshot.coverage.conflicting.{field}")],
            layer: None,
            heuristic_score: None,
            hard_reject: true,
        });
    }
    for field in &facts.coverage.missing {
        assessments.push(AssessmentItem {
            id: format!("signal_missing:{field}"),
            support: AssessmentSupport::Unknown,
            conclusion: None,
            reason_codes: vec!["missing_required_field".into()],
            basis_refs: vec![format!("fact_snapshot.coverage.missing.{field}")],
            layer: None,
            heuristic_score: None,
            hard_reject: false,
        });
    }

    compose_assessment_envelope(AssessmentComposeInput {
        identity,
        as_of,
        policy,
        fact_snapshot: Some(facts.clone()),
        management: management.cloned(),
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: action_rationale.cloned(),
        layers: BTreeMap::new(),
        assessments,
        advisory_actions: vec![],
        evidence_quality: None,
        evidence_applicability: Some(facts.scope.read_scope.clone()),
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec!["fact_snapshot".into()],
        candidates_seen: facts.limits.candidates_seen,
        scan_ops: Some(facts.limits.scan_ops),
        time_budget_ms: None,
        read_scope: facts.scope.read_scope.clone(),
    })
}

/// Aggregate support values with conflict priority while preserving each raw item.
pub fn aggregate_support(items: &[AssessmentSupport]) -> Option<AssessmentSupport> {
    items.iter().copied().max_by_key(|s| s.conflict_priority())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_support_rejects_false_conclusion() {
        let item = AssessmentItem {
            id: "x".into(),
            support: AssessmentSupport::Unknown,
            conclusion: Some(Value::Bool(false)),
            reason_codes: vec![],
            basis_refs: vec![],
            layer: None,
            heuristic_score: None,
            hard_reject: false,
        };
        assert!(validate_assessment_item(&item).is_err());
    }

    #[test]
    fn reason_sort_is_stable_and_policy_ordered() {
        let policy = AssessmentPolicy::default();
        let mut codes = vec![
            "independent_work_units".into(),
            "execution_result_requires_query".into(),
            "zzz_custom".into(),
            "claim_conflict".into(),
        ];
        sort_reason_codes(&policy, &mut codes);
        assert_eq!(codes[0], "execution_result_requires_query");
        assert_eq!(codes[1], "independent_work_units");
        assert_eq!(codes[2], "claim_conflict");
        assert_eq!(codes[3], "zzz_custom");
    }

    #[test]
    fn soft_scores_cannot_clear_hard_reject() {
        assert!(!soft_scores_may_rank(HardGateOutcome::Reject));
        assert!(!soft_scores_may_rank(HardGateOutcome::Unknown));
        assert!(soft_scores_may_rank(HardGateOutcome::Pass));
    }

    #[test]
    fn conflict_priority_orders_supports() {
        assert!(
            AssessmentSupport::Conflicting.conflict_priority()
                > AssessmentSupport::Unknown.conflict_priority()
        );
        assert!(
            AssessmentSupport::Unknown.conflict_priority()
                > AssessmentSupport::Unsupported.conflict_priority()
        );
        assert_eq!(
            aggregate_support(&[
                AssessmentSupport::Supported,
                AssessmentSupport::Unknown,
                AssessmentSupport::Conflicting
            ]),
            Some(AssessmentSupport::Conflicting)
        );
    }
}
