//! DEC-020: compose an explanation chain over existing prepare/assess judgments.
//!
//! Attaches typed layer explanations and finite advisory actions on top of
//! conclusions already produced by management, readiness, context, delivery,
//! and completion paths. Does **not** duplicate condition branches into a
//! second adjudicator. Behavior fixes remain owned by EVO-012.
//!
//! Boundaries (acceptance):
//! - Preserve each layer's existing conclusion; envelope cites basis + unchecked.
//! - Unresolved side effects → only `query_original_operation_result`; never
//!   advise re-run because of high coverage / low risk / small change.
//! - Source or auth change invalidates prior explanations; unsupported
//!   exec-state / host probes return `unknown` and must not claim a process
//!   was stopped.
use crate::fact_snapshot::{PreparedFactView, fact_snapshot_from_prepared_view};
use awr_core::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Profile id for the five-layer prepare explanation chain.
pub const EXPLANATION_CHAIN_PROFILE: &str = "prepare_explanation_chain_v1";

/// Soft ranking cues that must never become a re-run advisory.
pub const FORBIDDEN_RERUN_CUES: &[&str] = &[
    "high_coverage",
    "low_risk",
    "small_change",
    "small_diff",
    "coverage_high",
    "risk_low",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeSupport {
    /// Probe path is implemented and may be cited when a result is already recorded.
    Supported,
    /// Probe is outside the current capability — emit `unknown`, never invent outcomes.
    Unsupported,
}

impl Default for ProbeSupport {
    fn default() -> Self {
        Self::Unsupported
    }
}

/// One recorded unresolved side effect that still needs the original op result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnresolvedSideEffect {
    pub original_op_ref: String,
    #[serde(default = "default_side_effect_reason")]
    pub reason_code: String,
}

fn default_side_effect_reason() -> String {
    "execution_result_requires_query".into()
}

/// Delivery / execution fragments already present on the prepare snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryExplanationInput {
    #[serde(default)]
    pub waiting_user: bool,
    #[serde(default)]
    pub wait_refs: Vec<String>,
    #[serde(default)]
    pub unresolved_side_effects: Vec<UnresolvedSideEffect>,
    #[serde(default)]
    pub historically_prepared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_present: Option<bool>,
    /// None = not checked (layer may stay not_evaluated for this facet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_state_probe: Option<ProbeSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_process_probe: Option<ProbeSupport>,
    /// Soft cues callers might be tempted to turn into re-run advice (must be ignored).
    #[serde(default)]
    pub soft_rerun_cues: Vec<String>,
}

/// Completion-layer observations when the completion path already ran; else leave unchecked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionExplanationInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validity_known: Option<bool>,
    #[serde(default)]
    pub standard_changed: bool,
    #[serde(default)]
    pub reopened: bool,
    #[serde(default)]
    pub basis_refs: Vec<String>,
}

/// Current source / auth identity used to invalidate prior envelopes (TOCTOU).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplanationAuthority {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_fingerprint: Option<String>,
    #[serde(default)]
    pub stop_or_revoke: bool,
}

/// Fixed inputs for pure explanation-chain composition. No Store / network / model.
#[derive(Debug, Clone)]
pub struct ExplanationChainInput {
    pub prepared: PreparedFactView,
    pub management_decision: Option<ManagementDecision>,
    pub management_observation: Option<Value>,
    pub management_observation_basis: Option<String>,
    pub management_admission_gaps: Vec<String>,
    pub management_record_required: Option<bool>,
    pub management_next_action: Option<String>,
    pub action_rationale: Option<ActionGuidance>,
    pub delivery: DeliveryExplanationInput,
    pub completion: CompletionExplanationInput,
    pub current_authority: ExplanationAuthority,
    pub prior_identity: Option<AssessmentIdentity>,
    pub policy: AssessmentPolicy,
    pub as_of: i64,
}

/// Result of binding explanations onto existing judgments.
#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExplanationChainResult {
    pub envelope: AssessmentEnvelope,
    /// `Some(false)` when a prior envelope is stale under current authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_explanation_valid: Option<bool>,
    /// Per-layer unchecked / not-run items (acceptance: envelope explains gaps).
    pub unchecked_by_layer: BTreeMap<String, Vec<String>>,
    /// True when any unresolved side effect forced query-only advice.
    pub query_original_only: bool,
}

fn authority_tag<'a>(summary: Option<&'a str>, key: &str) -> Option<&'a str> {
    let summary = summary?;
    let equals = format!("{key}=");
    for part in summary.split('|') {
        if let Some(value) = part.strip_prefix(&equals) {
            return Some(value);
        }
    }
    // Legacy envelopes stored a single `key:value` summary.
    let colon = format!("{key}:");
    summary.strip_prefix(&colon)
}

fn authority_facet_still_valid(
    prior: &AssessmentIdentity,
    key: &str,
    current: Option<&str>,
) -> bool {
    let Some(now) = current.filter(|value| !value.is_empty()) else {
        return true;
    };
    match authority_tag(prior.input_summary.as_deref(), key) {
        Some(old) => old == now,
        // A current source or auth that the prior envelope did not record is not the same authority.
        None => false,
    }
}

fn explanation_input_summary(chain_hash: &str, authority: &ExplanationAuthority) -> String {
    let mut parts = Vec::new();
    if let Some(src) = authority
        .source_revision
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("source_revision={src}"));
    }
    if let Some(auth) = authority
        .auth_fingerprint
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("auth={auth}"));
    }
    parts.push(format!("chain={chain_hash}"));
    parts.join("|")
}

/// Whether a prior envelope identity remains usable under current authority.
pub fn prior_explanation_still_valid(
    prior: &AssessmentIdentity,
    current: &ExplanationAuthority,
) -> bool {
    if current.stop_or_revoke {
        return false;
    }
    if let (Some(old), Some(now)) = (
        prior.contract_fingerprint.as_deref(),
        current.contract_hash.as_deref(),
    ) {
        if old != now {
            return false;
        }
    }
    if !authority_facet_still_valid(prior, "source_revision", current.source_revision.as_deref()) {
        return false;
    }
    if !authority_facet_still_valid(prior, "auth", current.auth_fingerprint.as_deref()) {
        return false;
    }
    true
}

fn parse_management_decision(view: &PreparedFactView) -> Option<ManagementDecision> {
    let mgmt = view.management.as_ref()?;
    mgmt.get("decision")
        .cloned()
        .or_else(|| mgmt.pointer("/assessment/decision").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
}

fn layer_result(
    status: LayerStatus,
    summary: Option<String>,
    basis_refs: Vec<String>,
) -> LayerResult {
    LayerResult {
        status,
        summary,
        basis_refs,
    }
}

fn map_work_readiness(
    view: &PreparedFactView,
    unchecked: &mut Vec<String>,
) -> (LayerResult, Option<AssessmentItem>) {
    match view.ready {
        Some(true) => {
            let mut basis = vec!["prepare.ready".into()];
            for (i, _) in view.diagnostics.iter().enumerate() {
                basis.push(format!("prepare.diagnostics[{i}]"));
            }
            (
                layer_result(LayerStatus::Supported, Some("ready".into()), basis.clone()),
                Some(AssessmentItem {
                    id: "work_readiness.ready".into(),
                    support: AssessmentSupport::Supported,
                    conclusion: Some(json!(true)),
                    reason_codes: vec![],
                    basis_refs: basis,
                    layer: Some(AssessmentLayerId::WorkReadiness),
                    heuristic_score: None,
                    hard_reject: false,
                }),
            )
        }
        Some(false) => {
            let mut codes = Vec::new();
            let mut basis = vec!["prepare.ready".into()];
            for (i, diag) in view.diagnostics.iter().enumerate() {
                basis.push(format!("prepare.diagnostics[{i}]"));
                if let Some(code) = diag
                    .get("code")
                    .and_then(|v| v.as_str())
                    .or_else(|| diag.as_str())
                {
                    codes.push(code.to_string());
                }
            }
            if codes.is_empty() {
                codes.push("readiness_blocker".into());
            }
            // Preserve not-ready; do not invent a second readiness rule.
            let hard = codes.iter().any(|c| {
                matches!(
                    c.as_str(),
                    "unresolved_dependencies" | "active_claim" | "claim_conflict"
                )
            });
            (
                layer_result(
                    if hard {
                        LayerStatus::Unsupported
                    } else {
                        LayerStatus::Unknown
                    },
                    Some("not_ready".into()),
                    basis.clone(),
                ),
                Some(AssessmentItem {
                    id: "work_readiness.ready".into(),
                    support: if hard {
                        AssessmentSupport::Unsupported
                    } else {
                        AssessmentSupport::Unknown
                    },
                    conclusion: Some(json!(false)),
                    reason_codes: codes,
                    basis_refs: basis,
                    layer: Some(AssessmentLayerId::WorkReadiness),
                    heuristic_score: None,
                    hard_reject: hard,
                }),
            )
        }
        None => {
            unchecked.push("prepare.ready".into());
            (
                LayerResult::not_evaluated(),
                Some(AssessmentItem {
                    id: "work_readiness.ready".into(),
                    support: AssessmentSupport::Unsupported,
                    conclusion: None,
                    reason_codes: vec!["missing_required_field".into()],
                    basis_refs: vec![],
                    layer: Some(AssessmentLayerId::WorkReadiness),
                    heuristic_score: None,
                    hard_reject: false,
                }),
            )
        }
    }
}

fn map_execution_admission(
    decision: Option<&ManagementDecision>,
    unchecked: &mut Vec<String>,
) -> (LayerResult, Option<AssessmentItem>) {
    match decision {
        Some(d) => {
            let admission = d.execution_admission.clone();
            (
                layer_result(
                    LayerStatus::Supported,
                    Some(admission.clone()),
                    vec!["management.decision.execution_admission".into()],
                ),
                Some(AssessmentItem {
                    id: "execution_admission".into(),
                    support: AssessmentSupport::Supported,
                    conclusion: Some(json!(admission)),
                    reason_codes: vec![],
                    basis_refs: vec!["management.decision.execution_admission".into()],
                    layer: Some(AssessmentLayerId::ExecutionAdmission),
                    heuristic_score: None,
                    // Management never grants admission; continuous mode is not a grant.
                    hard_reject: false,
                }),
            )
        }
        None => {
            unchecked.push("management.decision.execution_admission".into());
            (LayerResult::not_evaluated(), None)
        }
    }
}

fn map_context_completeness(
    view: &PreparedFactView,
    unchecked: &mut Vec<String>,
) -> (LayerResult, Option<AssessmentItem>) {
    match view.context_complete {
        Some(true) => {
            let mut basis = vec!["context.completeness.complete".into()];
            for issue in &view.context_issues {
                basis.push(format!("context.completeness.issues.{issue}"));
            }
            (
                layer_result(
                    LayerStatus::Supported,
                    Some("complete".into()),
                    basis.clone(),
                ),
                Some(AssessmentItem {
                    id: "context_completeness".into(),
                    support: AssessmentSupport::Supported,
                    conclusion: Some(json!(true)),
                    reason_codes: vec![],
                    basis_refs: basis,
                    layer: Some(AssessmentLayerId::ContextCompleteness),
                    heuristic_score: None,
                    hard_reject: false,
                }),
            )
        }
        Some(false) => {
            // Known incomplete: support=Supported with conclusion=false (unknown≠false).
            let mut basis = vec!["context.completeness.complete".into()];
            let mut codes = Vec::new();
            for issue in &view.context_issues {
                basis.push(format!("context.completeness.issues.{issue}"));
                codes.push(issue.clone());
            }
            (
                layer_result(
                    LayerStatus::Supported,
                    Some("incomplete".into()),
                    basis.clone(),
                ),
                Some(AssessmentItem {
                    id: "context_completeness".into(),
                    support: AssessmentSupport::Supported,
                    conclusion: Some(json!(false)),
                    reason_codes: codes,
                    basis_refs: basis,
                    layer: Some(AssessmentLayerId::ContextCompleteness),
                    heuristic_score: None,
                    hard_reject: false,
                }),
            )
        }
        None => {
            unchecked.push("context.completeness.complete".into());
            (LayerResult::not_evaluated(), None)
        }
    }
}

fn map_delivery(
    delivery: &DeliveryExplanationInput,
    unchecked: &mut Vec<String>,
) -> (LayerResult, Vec<AssessmentItem>, bool) {
    let mut items = Vec::new();
    let mut query_only = false;
    let mut basis = Vec::new();
    let mut status = LayerStatus::NotEvaluated;

    if delivery.waiting_user {
        status = status.merge(LayerStatus::Supported);
        basis.extend(delivery.wait_refs.iter().cloned());
        if basis.is_empty() {
            basis.push("continuity.waits".into());
        }
        items.push(AssessmentItem {
            id: "delivery.waiting_user".into(),
            support: AssessmentSupport::Supported,
            conclusion: Some(json!("waiting_user")),
            reason_codes: vec!["persistent_wait".into()],
            basis_refs: basis.clone(),
            layer: Some(AssessmentLayerId::DeliveryObservation),
            heuristic_score: None,
            hard_reject: false,
        });
    }

    if !delivery.unresolved_side_effects.is_empty() {
        query_only = true;
        status = status.merge(LayerStatus::Unknown);
        for effect in &delivery.unresolved_side_effects {
            basis.push(effect.original_op_ref.clone());
            items.push(AssessmentItem {
                id: format!("delivery.unresolved:{}", effect.original_op_ref),
                support: AssessmentSupport::Unknown,
                conclusion: None,
                reason_codes: vec![effect.reason_code.clone()],
                basis_refs: vec![effect.original_op_ref.clone()],
                layer: Some(AssessmentLayerId::DeliveryObservation),
                heuristic_score: None,
                hard_reject: true,
            });
        }
    }

    if delivery.historically_prepared {
        match delivery.ack_present {
            Some(true) => {
                status = status.merge(LayerStatus::Supported);
                basis.push("delivery.ack".into());
                items.push(AssessmentItem {
                    id: "delivery.ack".into(),
                    support: AssessmentSupport::Supported,
                    conclusion: Some(json!("acked")),
                    reason_codes: vec![],
                    basis_refs: vec!["delivery.ack".into()],
                    layer: Some(AssessmentLayerId::DeliveryObservation),
                    heuristic_score: None,
                    hard_reject: false,
                });
            }
            Some(false) | None => {
                status = status.merge(LayerStatus::Unknown);
                basis.push("delivery.prepared_without_ack".into());
                items.push(AssessmentItem {
                    id: "delivery.ack".into(),
                    support: AssessmentSupport::Unknown,
                    conclusion: Some(json!("not_verified")),
                    reason_codes: vec!["delivery_unknown".into()],
                    basis_refs: vec!["delivery.prepared_without_ack".into()],
                    layer: Some(AssessmentLayerId::DeliveryObservation),
                    heuristic_score: None,
                    hard_reject: true,
                });
            }
        }
    }

    // Explicit unsupported probes: unknown, and never claim a real process was stopped.
    // Absent (None) means the facet was not checked — record as unchecked, do not invent.
    match delivery.exec_state_probe {
        None => unchecked.push("exec_state_probe".into()),
        Some(ProbeSupport::Unsupported) => {
            status = status.merge(LayerStatus::Unknown);
            items.push(AssessmentItem {
                id: "delivery.exec_state_probe".into(),
                support: AssessmentSupport::Unknown,
                conclusion: Some(json!("unknown")),
                reason_codes: vec!["unsupported_exec_state_probe".into()],
                basis_refs: vec!["delivery.exec_state_probe".into()],
                layer: Some(AssessmentLayerId::DeliveryObservation),
                heuristic_score: None,
                hard_reject: false,
            });
        }
        Some(ProbeSupport::Supported) => {
            basis.push("delivery.exec_state_probe.supported".into());
        }
    }
    match delivery.host_process_probe {
        None => unchecked.push("host_process_probe".into()),
        Some(ProbeSupport::Unsupported) => {
            status = status.merge(LayerStatus::Unknown);
            items.push(AssessmentItem {
                id: "delivery.host_process_probe".into(),
                support: AssessmentSupport::Unknown,
                conclusion: Some(json!("unknown")),
                reason_codes: vec!["unsupported_host_process_probe".into()],
                basis_refs: vec!["delivery.host_process_probe".into()],
                layer: Some(AssessmentLayerId::DeliveryObservation),
                heuristic_score: None,
                hard_reject: false,
            });
        }
        Some(ProbeSupport::Supported) => {
            basis.push("delivery.host_process_probe.supported".into());
        }
    }

    if status == LayerStatus::NotEvaluated {
        unchecked.push("delivery_observation".into());
        (LayerResult::not_evaluated(), items, query_only)
    } else {
        let summary = if query_only {
            Some("unresolved_side_effects_query_original".into())
        } else if delivery.historically_prepared && delivery.ack_present != Some(true) {
            Some("not_verified".into())
        } else {
            Some("recorded_delivery_observation".into())
        };
        (layer_result(status, summary, basis), items, query_only)
    }
}

fn map_completion(
    decision: Option<&ManagementDecision>,
    completion: &CompletionExplanationInput,
    unchecked: &mut Vec<String>,
) -> (LayerResult, Option<AssessmentItem>) {
    if completion.reopened || completion.standard_changed {
        let mut basis = completion.basis_refs.clone();
        if basis.is_empty() {
            basis.push("completion.reassessment_required".into());
        }
        return (
            layer_result(
                LayerStatus::Unknown,
                Some("reassessment_required".into()),
                basis.clone(),
            ),
            Some(AssessmentItem {
                id: "completion_validity".into(),
                support: AssessmentSupport::Unknown,
                conclusion: Some(json!("reassessment_required")),
                reason_codes: vec!["completion_standard_changed".into()],
                basis_refs: basis,
                layer: Some(AssessmentLayerId::CompletionValidity),
                heuristic_score: None,
                hard_reject: false,
            }),
        );
    }
    if let Some(valid) = completion.validity_known {
        let mut basis = completion.basis_refs.clone();
        if basis.is_empty() {
            basis.push("completion.validity".into());
        }
        return (
            layer_result(
                LayerStatus::Supported,
                Some(if valid { "valid" } else { "invalid" }.into()),
                basis.clone(),
            ),
            Some(AssessmentItem {
                id: "completion_validity".into(),
                support: AssessmentSupport::Supported,
                conclusion: Some(json!(valid)),
                reason_codes: vec![],
                basis_refs: basis,
                layer: Some(AssessmentLayerId::CompletionValidity),
                heuristic_score: None,
                hard_reject: !valid,
            }),
        );
    }
    // First-batch echo: management does not change completion_policy.
    if let Some(d) = decision {
        return (
            layer_result(
                LayerStatus::Supported,
                Some(d.completion_policy.clone()),
                vec!["management.decision.completion_policy".into()],
            ),
            Some(AssessmentItem {
                id: "completion_validity".into(),
                support: AssessmentSupport::Supported,
                conclusion: Some(json!(d.completion_policy.clone())),
                reason_codes: vec![],
                basis_refs: vec!["management.decision.completion_policy".into()],
                layer: Some(AssessmentLayerId::CompletionValidity),
                heuristic_score: None,
                hard_reject: false,
            }),
        );
    }
    unchecked.push("completion_validity".into());
    (LayerResult::not_evaluated(), None)
}

fn advisory_from_existing(
    decision: Option<&ManagementDecision>,
    view: &PreparedFactView,
    delivery: &DeliveryExplanationInput,
    query_only: bool,
    authority: &ExplanationAuthority,
) -> Vec<AdvisoryAction> {
    let mut actions = Vec::new();

    if authority.stop_or_revoke {
        actions.push(AdvisoryAction {
            code: "no_advisory".into(),
            when: Some("stop_or_revoke_active".into()),
            query_refs: vec![],
            limits: vec![
                "cached_advice_must_not_bypass_current_check".into(),
                "soft_scores_cannot_clear_revoke".into(),
            ],
            reevaluation: Some("after_authorization_restored_and_reassessed".into()),
        });
        return actions;
    }

    if query_only || !delivery.unresolved_side_effects.is_empty() {
        let query_refs: Vec<String> = delivery
            .unresolved_side_effects
            .iter()
            .map(|e| e.original_op_ref.clone())
            .collect();
        let mut limits = vec![
            "do_not_rerun_for_high_coverage".into(),
            "do_not_rerun_for_low_risk".into(),
            "do_not_rerun_for_small_change".into(),
            "query_original_result_only".into(),
        ];
        for cue in &delivery.soft_rerun_cues {
            let normalized = cue.to_ascii_lowercase().replace('-', "_");
            if FORBIDDEN_RERUN_CUES.iter().any(|f| normalized.contains(f)) {
                limits.push(format!("ignored_soft_rerun_cue:{normalized}"));
            }
        }
        actions.push(AdvisoryAction {
            code: "query_original_operation_result".into(),
            when: Some("unresolved_side_effect".into()),
            query_refs,
            limits,
            reevaluation: Some("after_original_operation_result_known".into()),
        });
        // Hard rule: no continue_prepare / no other re-run-adjacent advice while unresolved.
        return actions;
    }

    if let Some(d) = decision {
        for reason in &d.reasons {
            let code = match reason.code.as_str() {
                "execution_result_requires_query" | "query_outcome_before_retry" => {
                    "query_original_operation_result"
                }
                "work_contract_changed" => "refresh_sources",
                "unresolved_dependencies" | "plan_invalidated" => "repair_required_materials",
                "persistent_wait" | "deferred_wait" => "collect_user_reply",
                "claim_conflict"
                | "handoff_or_collaboration"
                | "work_handoff"
                | "cross_session_resume" => "inspect_claim_conflict",
                _ => continue,
            };
            if actions.iter().any(|a| a.code == code) {
                continue;
            }
            actions.push(AdvisoryAction {
                code: code.into(),
                when: Some(reason.code.clone()),
                query_refs: if code == "query_original_operation_result" {
                    vec![reason.reference.clone()]
                } else {
                    vec![]
                },
                limits: vec!["advisory_only_not_admission".into()],
                reevaluation: Some(format!("on_{}_change", reason.code)),
            });
        }
        for action in &d.required_actions {
            if action.contains("query_unknown") || action.contains("query_outcome") {
                if !actions
                    .iter()
                    .any(|a| a.code == "query_original_operation_result")
                {
                    actions.push(AdvisoryAction {
                        code: "query_original_operation_result".into(),
                        when: Some(action.clone()),
                        query_refs: vec![],
                        limits: vec![
                            "do_not_rerun_for_high_coverage".into(),
                            "do_not_rerun_for_low_risk".into(),
                            "do_not_rerun_for_small_change".into(),
                        ],
                        reevaluation: Some("after_original_operation_result_known".into()),
                    });
                }
            }
        }
    }

    if delivery.waiting_user && !actions.iter().any(|a| a.code == "collect_user_reply") {
        actions.push(AdvisoryAction {
            code: "collect_user_reply".into(),
            when: Some("waiting_user".into()),
            query_refs: delivery.wait_refs.clone(),
            limits: vec!["do_not_repeat_request_while_waiting".into()],
            reevaluation: Some("after_user_reply".into()),
        });
    }

    if view.ready == Some(false)
        && !actions
            .iter()
            .any(|a| a.code == "repair_required_materials" || a.code == "inspect_claim_conflict")
    {
        actions.push(AdvisoryAction {
            code: "repair_required_materials".into(),
            when: Some("readiness_blocker".into()),
            query_refs: vec![],
            limits: vec!["do_not_infer_ready_from_context_alone".into()],
            reevaluation: Some("after_dependency_or_claim_change".into()),
        });
    }

    if view
        .management
        .as_ref()
        .is_some_and(|m| m.get("record_required") == Some(&Value::Bool(true)))
        || decision.is_some_and(|d| d.mode == ManagementMode::Undetermined)
    {
        if !actions
            .iter()
            .any(|a| a.code == "record_management_observations")
        {
            actions.push(AdvisoryAction {
                code: "record_management_observations".into(),
                when: Some("management_record_required_or_undetermined".into()),
                query_refs: vec![],
                limits: vec!["do_not_invent_missing_host_facts".into()],
                reevaluation: Some("after_explicit_host_observations_recorded".into()),
            });
        }
    }

    if actions.is_empty() {
        if view.ready == Some(true) && view.context_complete != Some(false) {
            actions.push(AdvisoryAction {
                code: "continue_prepare_current_work".into(),
                when: Some("ready_and_no_higher_priority_blocker".into()),
                query_refs: vec![],
                limits: vec!["does_not_grant_execution_admission".into()],
                reevaluation: Some("on_source_claim_wait_or_outcome_change".into()),
            });
        } else {
            actions.push(AdvisoryAction {
                code: "no_advisory".into(),
                when: Some("insufficient_judgments_for_advice".into()),
                query_refs: vec![],
                limits: vec![],
                reevaluation: Some("after_prepare_snapshot_refreshed".into()),
            });
        }
    }

    actions
}

/// Compose the five-layer explanation chain from existing prepare/assess results.
pub fn compose_explanation_chain(input: ExplanationChainInput) -> Result<ExplanationChainResult> {
    ensure_public_data(&input.prepared)?;
    if input.as_of < 0 {
        return Err(Error::InvalidInput(
            "explanation chain as_of must be non-negative".into(),
        ));
    }
    if input.as_of != input.prepared.as_of {
        return Err(Error::InvalidInput(
            "explanation chain as_of must match PreparedFactView.as_of".into(),
        ));
    }

    let decision = input
        .management_decision
        .clone()
        .or_else(|| parse_management_decision(&input.prepared));

    let mut unchecked_by_layer: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut layers = BTreeMap::new();
    let mut assessments = Vec::new();

    let mut readiness_unchecked = Vec::new();
    let (readiness_layer, readiness_item) =
        map_work_readiness(&input.prepared, &mut readiness_unchecked);
    layers.insert(AssessmentLayerId::WorkReadiness, readiness_layer);
    unchecked_by_layer.insert(
        AssessmentLayerId::WorkReadiness.as_str().into(),
        readiness_unchecked,
    );
    if let Some(item) = readiness_item {
        assessments.push(item);
    }

    let mut admission_unchecked = Vec::new();
    let (admission_layer, admission_item) =
        map_execution_admission(decision.as_ref(), &mut admission_unchecked);
    layers.insert(AssessmentLayerId::ExecutionAdmission, admission_layer);
    unchecked_by_layer.insert(
        AssessmentLayerId::ExecutionAdmission.as_str().into(),
        admission_unchecked,
    );
    if let Some(item) = admission_item {
        assessments.push(item);
    }

    let mut context_unchecked = Vec::new();
    let (context_layer, context_item) =
        map_context_completeness(&input.prepared, &mut context_unchecked);
    layers.insert(AssessmentLayerId::ContextCompleteness, context_layer);
    unchecked_by_layer.insert(
        AssessmentLayerId::ContextCompleteness.as_str().into(),
        context_unchecked,
    );
    if let Some(item) = context_item {
        assessments.push(item);
    }

    let mut delivery_unchecked = Vec::new();
    let (delivery_layer, delivery_items, query_only) =
        map_delivery(&input.delivery, &mut delivery_unchecked);
    layers.insert(AssessmentLayerId::DeliveryObservation, delivery_layer);
    unchecked_by_layer.insert(
        AssessmentLayerId::DeliveryObservation.as_str().into(),
        delivery_unchecked,
    );
    assessments.extend(delivery_items);

    let mut completion_unchecked = Vec::new();
    let (completion_layer, completion_item) = map_completion(
        decision.as_ref(),
        &input.completion,
        &mut completion_unchecked,
    );
    layers.insert(AssessmentLayerId::CompletionValidity, completion_layer);
    unchecked_by_layer.insert(
        AssessmentLayerId::CompletionValidity.as_str().into(),
        completion_unchecked,
    );
    if let Some(item) = completion_item {
        assessments.push(item);
    }

    // Stop/revoke: inject hard reject that soft scores cannot clear.
    if input.current_authority.stop_or_revoke {
        assessments.push(AssessmentItem {
            id: "authority.stop_or_revoke".into(),
            support: AssessmentSupport::Conflicting,
            conclusion: Some(json!("revoked")),
            reason_codes: vec!["hard_guard_reject".into()],
            basis_refs: vec!["authority.stop_or_revoke".into()],
            layer: Some(AssessmentLayerId::ExecutionAdmission),
            heuristic_score: None,
            hard_reject: true,
        });
    }

    let advisory_actions = advisory_from_existing(
        decision.as_ref(),
        &input.prepared,
        &input.delivery,
        query_only,
        &input.current_authority,
    );

    // Guard: unresolved side effects must never produce re-run-adjacent advisories.
    if query_only {
        for action in &advisory_actions {
            if action.code != "query_original_operation_result" && action.code != "no_advisory" {
                return Err(Error::InvalidInput(format!(
                    "unresolved side effects forbid advisory {}; only query_original_operation_result is allowed",
                    action.code
                )));
            }
            let joined = action.limits.join(" ");
            for cue in FORBIDDEN_RERUN_CUES {
                // Limits must explicitly forbid these cues; never encode them as when/re-run.
                if action.when.as_deref().is_some_and(|w| {
                    w.to_ascii_lowercase().contains(cue) && w.to_ascii_lowercase().contains("rerun")
                }) {
                    return Err(Error::InvalidInput(format!(
                        "advisory must not recommend re-run from soft cue {cue}"
                    )));
                }
            }
            let _ = joined;
        }
    }

    let snapshot = fact_snapshot_from_prepared_view(&input.prepared)?;

    let input_summary = explanation_input_summary(&snapshot.content_hash, &input.current_authority);

    let identity = AssessmentIdentity {
        project_id: input.prepared.project_id.clone(),
        work_key: Some(input.prepared.work_key.clone()),
        work_id: input.prepared.work_id.clone(),
        branch_id: input.prepared.branch_id.clone(),
        contract_fingerprint: input
            .current_authority
            .contract_hash
            .clone()
            .or(input.prepared.work_contract_hash.clone()),
        policy_id: input.policy.policy_id.clone(),
        policy_version: input.policy.policy_version,
        policy_hash: input.policy.policy_hash.clone(),
        input_summary: Some(input_summary),
        as_of: Some(input.as_of),
        verified_main_sha: None,
        fact_snapshot_hash: Some(snapshot.content_hash.clone()),
    };

    let prior_explanation_valid = input
        .prior_identity
        .as_ref()
        .map(|prior| prior_explanation_still_valid(prior, &input.current_authority));

    // Stale prior under authority change: surface refresh advisory when not query-only.
    let mut advisory_actions = advisory_actions;
    if prior_explanation_valid == Some(false)
        && !query_only
        && !input.current_authority.stop_or_revoke
        && !advisory_actions.iter().any(|a| a.code == "refresh_sources")
    {
        advisory_actions.insert(
            0,
            AdvisoryAction {
                code: "refresh_sources".into(),
                when: Some("prior_explanation_invalidated".into()),
                query_refs: vec![],
                limits: vec!["stale_envelope_is_not_admission".into()],
                reevaluation: Some("after_source_or_auth_refresh".into()),
            },
        );
    }

    let management_obs = input.management_observation.clone().or_else(|| {
        input
            .prepared
            .management
            .as_ref()
            .and_then(|m| m.get("observation").cloned())
    });
    let management_basis = input.management_observation_basis.clone().or_else(|| {
        input.prepared.management.as_ref().and_then(|m| {
            m.get("observation_basis")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
    });
    let admission_gaps = if input.management_admission_gaps.is_empty() {
        input
            .prepared
            .management
            .as_ref()
            .and_then(|m| m.get("admission_gaps"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        input.management_admission_gaps.clone()
    };
    let record_required = input.management_record_required.or_else(|| {
        input
            .prepared
            .management
            .as_ref()
            .and_then(|m| m.get("record_required"))
            .and_then(|v| v.as_bool())
    });
    let next_action = input.management_next_action.clone().or_else(|| {
        input
            .prepared
            .management
            .as_ref()
            .and_then(|m| m.get("next_action"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });

    let mut envelope = compose_assessment_envelope(AssessmentComposeInput {
        identity,
        as_of: input.as_of,
        policy: input.policy,
        fact_snapshot: Some(snapshot),
        management: decision,
        management_observation: management_obs,
        management_observation_basis: management_basis,
        management_admission_gaps: admission_gaps,
        management_record_required: record_required,
        management_next_action: next_action,
        action_rationale: input.action_rationale,
        layers,
        assessments,
        advisory_actions,
        evidence_quality: None,
        evidence_applicability: Some("prepare_explanation_chain".into()),
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![
            "prepare".into(),
            "management".into(),
            "fact_snapshot".into(),
        ],
        candidates_seen: input.prepared.diagnostics.len()
            + input.prepared.context_issues.len()
            + input.delivery.unresolved_side_effects.len(),
        scan_ops: Some(1),
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })?;

    // Annotate layer summaries with unchecked items for acceptance criterion 1.
    for (layer_id, unchecked) in &unchecked_by_layer {
        if unchecked.is_empty() {
            continue;
        }
        if let Some(layer) = envelope.layers.get_mut(layer_id) {
            let note = format!("unchecked:{}", unchecked.join(","));
            layer.summary = Some(match &layer.summary {
                Some(s) if !s.is_empty() => format!("{s}; {note}"),
                _ => note,
            });
            for u in unchecked {
                let pref = format!("unchecked.{u}");
                if !layer.basis_refs.contains(&pref) {
                    layer.basis_refs.push(pref);
                }
            }
        }
    }

    // Stamp profile for DEC-020 consumers without breaking schema_id.
    envelope.assessment_profile = EXPLANATION_CHAIN_PROFILE.into();
    envelope.assessment_hash = canonical_assessment_hash(&envelope)?;

    // Final safety: never claim a process was stopped.
    for item in &envelope.assessments {
        if let Some(Value::String(s)) = &item.conclusion {
            let lower = s.to_ascii_lowercase();
            if lower.contains("stopped") || lower.contains("killed") || lower.contains("terminated")
            {
                if item.id.contains("host_process_probe") || item.id.contains("exec_state_probe") {
                    return Err(Error::InvalidInput(
                        "unsupported probes must not claim a real process was stopped".into(),
                    ));
                }
            }
        }
    }

    Ok(ExplanationChainResult {
        envelope,
        prior_explanation_valid,
        unchecked_by_layer,
        query_original_only: query_only,
    })
}

/// Convenience: build chain input from a prepare JSON value (no Store I/O).
pub fn explanation_chain_from_prepare_json(
    prepare: &Value,
    project_id: Option<String>,
    as_of: i64,
    delivery: DeliveryExplanationInput,
    completion: CompletionExplanationInput,
    current_authority: ExplanationAuthority,
    prior_identity: Option<AssessmentIdentity>,
    action_rationale: Option<ActionGuidance>,
    policy: AssessmentPolicy,
) -> Result<ExplanationChainResult> {
    let prepared =
        crate::fact_snapshot::prepared_view_from_prepare_json(prepare, project_id, as_of, None)?;
    let management_decision = parse_management_decision(&prepared);
    compose_explanation_chain(ExplanationChainInput {
        prepared,
        management_decision,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale,
        delivery,
        completion,
        current_authority,
        prior_identity,
        policy,
        as_of,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_rerun_cues_are_named() {
        assert!(FORBIDDEN_RERUN_CUES.contains(&"high_coverage"));
        assert!(FORBIDDEN_RERUN_CUES.contains(&"low_risk"));
        assert!(FORBIDDEN_RERUN_CUES.contains(&"small_change"));
    }

    #[test]
    fn stop_revokes_prior_explanation() {
        let prior = AssessmentIdentity {
            project_id: None,
            work_key: Some("W".into()),
            work_id: None,
            branch_id: None,
            contract_fingerprint: Some("c1".into()),
            policy_id: ASSESSMENT_POLICY_ID.into(),
            policy_version: 1,
            policy_hash: None,
            input_summary: None,
            as_of: Some(1),
            verified_main_sha: None,
            fact_snapshot_hash: None,
        };
        let auth = ExplanationAuthority {
            contract_hash: Some("c1".into()),
            source_revision: None,
            auth_fingerprint: None,
            stop_or_revoke: true,
        };
        assert!(!prior_explanation_still_valid(&prior, &auth));
    }

    #[test]
    fn matching_auth_does_not_hide_a_source_change() {
        let prior = AssessmentIdentity {
            project_id: None,
            work_key: Some("W".into()),
            work_id: None,
            branch_id: None,
            contract_fingerprint: Some("c1".into()),
            policy_id: ASSESSMENT_POLICY_ID.into(),
            policy_version: 1,
            policy_hash: None,
            input_summary: Some("auth:auth-ok".into()),
            as_of: Some(1),
            verified_main_sha: None,
            fact_snapshot_hash: Some("snap".into()),
        };
        let auth = ExplanationAuthority {
            contract_hash: Some("c1".into()),
            source_revision: Some("775".into()),
            auth_fingerprint: Some("auth-ok".into()),
            stop_or_revoke: false,
        };
        assert!(!prior_explanation_still_valid(&prior, &auth));
    }
}
