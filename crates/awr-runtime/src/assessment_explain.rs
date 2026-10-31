//! DEC-021: optional CLI/MCP explanation attachment over DEC-020's chain.
//!
//! Capability id `assessment.explain` is negotiated on existing prepare/assess
//! paths. Default (explain off) preserves legacy full/summary/action shapes.
//! Attachment is read-only: never claims work, never updates completion, never
//! auto-invokes tools. Does not rebuild a second adjudicator.
use crate::explanation_chain::{
    CompletionExplanationInput, DeliveryExplanationInput, ExplanationAuthority,
    ExplanationChainResult, ProbeSupport, UnresolvedSideEffect,
    explanation_chain_from_prepare_json,
};
use awr_core::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Capability catalog id (CLI `capabilities` / DEC-010 freeze).
pub const ASSESSMENT_EXPLAIN_CAPABILITY: &str = "assessment.explain";

/// Top-level field attached when explain is negotiated on.
pub const ASSESSMENT_EXPLAIN_FIELD: &str = "assessment_explanation";

/// Presentation payload wrapped around a DEC-020 chain result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentExplanationView {
    pub capability: String,
    pub enabled: bool,
    pub envelope: AssessmentEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_explanation_valid: Option<bool>,
    pub unchecked_by_layer: std::collections::BTreeMap<String, Vec<String>>,
    pub query_original_only: bool,
    /// The project policy that shaped this explanation, when one did or when the file could not be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<crate::assessment_policy::PolicyNotice>,
    /// Wire / render accounting without re-embedding prepare context body.
    pub metrics: ExplanationWireMetrics,
    /// Explicit non-side-effect guarantees for consumers and tests.
    pub side_effects: ExplanationSideEffects,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExplanationWireMetrics {
    pub envelope_bytes: usize,
    pub rendered_context_bytes: usize,
    pub explanation_embeds_rendered_context: bool,
    pub prepare_context_duplicated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExplanationSideEffects {
    pub claimed_work: bool,
    pub updated_completion: bool,
    pub auto_invoked_tools: bool,
    pub model_or_network: bool,
}

impl Default for ExplanationSideEffects {
    fn default() -> Self {
        Self {
            claimed_work: false,
            updated_completion: false,
            auto_invoked_tools: false,
            model_or_network: false,
        }
    }
}

/// Options for attaching an optional explanation onto an existing receipt.
#[derive(Debug, Clone, Default)]
pub struct AttachExplanationOptions {
    /// When false, the value is returned unchanged (legacy consumer path).
    pub enabled: bool,
    pub project_id: Option<String>,
    /// Stable as_of; prefer project_revision so CLI/MCP match on the same snapshot.
    pub as_of: Option<i64>,
    pub delivery: Option<DeliveryExplanationInput>,
    pub completion: Option<CompletionExplanationInput>,
    pub current_authority: Option<ExplanationAuthority>,
    pub prior_identity: Option<AssessmentIdentity>,
    /// A project whose assessment policy file is read for this explanation (DEC-040). Without it, and without
    /// `policy`, the built-in policy applies and the explanation is what it always was.
    pub policy_root: Option<std::path::PathBuf>,
    /// A policy handed in already resolved; it wins over `policy_root`.
    pub policy: Option<AssessmentPolicy>,
}

/// Whether a JSON object already carries the negotiated explain field.
pub fn has_assessment_explanation(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.contains_key(ASSESSMENT_EXPLAIN_FIELD))
}

/// Normalize prepare or assess receipts into the prepare-shaped JSON the chain parser expects.
pub fn normalize_receipt_for_explanation(value: &Value) -> Result<Value> {
    ensure_public_data(value)?;
    if value
        .pointer("/work/external_key")
        .and_then(|v| v.as_str())
        .is_some()
    {
        // Prepare-shaped (or already normalized).
        return Ok(value.clone());
    }
    // Assess-shaped: top-level `work` is a string key and `decision` sits at the root.
    let work_key = value.get("work").and_then(|v| v.as_str()).ok_or_else(|| {
        Error::InvalidInput(
            "assessment explain requires prepare work.external_key or assess work key".into(),
        )
    })?;
    let management = json!({
        "contract_fingerprint": value.get("contract_fingerprint"),
        "decision": value.get("decision"),
        "observation": value.get("observation"),
        "observation_basis": value.get("observation_basis"),
        "admission_gaps": value.get("admission_gaps").cloned().unwrap_or_else(|| json!([])),
        "record_required": value.get("record_required"),
        "next_action": value.get("next_action"),
        "observer": value.get("observer"),
    });
    Ok(json!({
        "work": {
            "external_key": work_key,
            "id": value.get("work_id"),
            "revision": value.get("work_revision"),
            "source_ref": value.get("source_ref"),
            "status": Value::Null,
            "next_action": value.get("next_action"),
        },
        // Assess does not assert readiness here — leave unknown (never coerce to false).
        "ready": Value::Null,
        "diagnostics": [],
        "active_claims": [],
        "management": management,
        "branch_id": value.get("branch_id"),
    }))
}

fn delivery_from_receipt(value: &Value) -> DeliveryExplanationInput {
    let waiting =
        value.pointer("/continuity/state").and_then(|v| v.as_str()) == Some("waiting_user");
    let wait_refs = value
        .pointer("/continuity/waits")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|w| {
                    w.get("id")
                        .or_else(|| w.get("wait_id"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut unresolved = Vec::new();
    if let Some(diags) = value.get("diagnostics").and_then(|v| v.as_array()) {
        for d in diags {
            let code = d.get("code").and_then(|v| v.as_str()).unwrap_or_default();
            if code.contains("execution_result_requires_query")
                || code == "execution_result_requires_query"
            {
                unresolved.push(UnresolvedSideEffect {
                    original_op_ref: d
                        .get("reference")
                        .or_else(|| d.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("execution")
                        .to_string(),
                    reason_code: "execution_result_requires_query".into(),
                });
            }
        }
    }
    // Management reasons may also carry query-original signals.
    if let Some(reasons) = value
        .pointer("/management/decision/reasons")
        .or_else(|| value.pointer("/decision/reasons"))
        .and_then(|v| v.as_array())
    {
        for r in reasons {
            if r.get("code").and_then(|v| v.as_str()) == Some("execution_result_requires_query") {
                unresolved.push(UnresolvedSideEffect {
                    original_op_ref: r
                        .get("reference")
                        .and_then(|v| v.as_str())
                        .unwrap_or("execution")
                        .to_string(),
                    reason_code: "execution_result_requires_query".into(),
                });
            }
        }
    }
    // Prepare stage and context consumption are not a delivery acknowledgement.
    // Only an explicit delivery history may set these; otherwise the facet stays unchecked.
    let historically_prepared = value
        .get("historically_prepared")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let ack_present = if historically_prepared {
        value.get("delivery_ack").and_then(|v| v.as_bool())
    } else {
        None
    };
    DeliveryExplanationInput {
        waiting_user: waiting,
        wait_refs,
        unresolved_side_effects: unresolved,
        historically_prepared,
        ack_present,
        exec_state_probe: Some(ProbeSupport::Unsupported),
        host_process_probe: Some(ProbeSupport::Unsupported),
        soft_rerun_cues: vec![],
    }
}

fn authority_from_receipt(value: &Value) -> ExplanationAuthority {
    let contract = value
        .pointer("/management/contract_fingerprint")
        .or_else(|| value.get("contract_fingerprint"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let source_revision = value
        .pointer("/work/source_ref/source_revision")
        .or_else(|| value.pointer("/source_ref/source_revision"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    ExplanationAuthority {
        contract_hash: contract,
        source_revision,
        auth_fingerprint: None,
        stop_or_revoke: false,
    }
}

fn stable_as_of(value: &Value, override_as_of: Option<i64>) -> i64 {
    if let Some(v) = override_as_of {
        return v;
    }
    value
        .get("project_revision")
        .and_then(|v| v.as_i64())
        .or_else(|| value.pointer("/work/revision").and_then(|v| v.as_i64()))
        .or_else(|| value.get("work_revision").and_then(|v| v.as_i64()))
        .unwrap_or(0)
}

fn action_rationale_from_receipt(value: &Value) -> Option<ActionGuidance> {
    value
        .get("guidance")
        .and_then(|g| serde_json::from_value::<ActionGuidance>(g.clone()).ok())
}

fn rendered_context_bytes(value: &Value) -> usize {
    value
        .pointer("/context/work_context/rendered_context")
        .and_then(|v| v.as_str())
        .map(|s| s.len())
        .unwrap_or(0)
}

fn build_view(
    chain: ExplanationChainResult,
    rendered_ctx_bytes: usize,
    policy: Option<crate::assessment_policy::PolicyNotice>,
) -> Result<AssessmentExplanationView> {
    let envelope_bytes = serde_json::to_vec(&chain.envelope)?.len();
    let envelope_text = serde_json::to_string(&chain.envelope)?;
    let embeds = rendered_ctx_bytes > 0
        && value_rendered_context_embedded(&envelope_text, rendered_ctx_bytes);
    Ok(AssessmentExplanationView {
        capability: ASSESSMENT_EXPLAIN_CAPABILITY.into(),
        enabled: true,
        envelope: chain.envelope,
        prior_explanation_valid: chain.prior_explanation_valid,
        unchecked_by_layer: chain.unchecked_by_layer,
        query_original_only: chain.query_original_only,
        policy,
        metrics: ExplanationWireMetrics {
            envelope_bytes,
            rendered_context_bytes: rendered_ctx_bytes,
            explanation_embeds_rendered_context: embeds,
            prepare_context_duplicated: embeds,
        },
        side_effects: ExplanationSideEffects::default(),
    })
}

fn value_rendered_context_embedded(envelope_text: &str, _rendered_ctx_bytes: usize) -> bool {
    // We never copy rendered_context into the chain; this guards regressions.
    envelope_text.contains("rendered_context")
}

/// Attach optional assessment explanation onto an existing prepare/assess receipt.
///
/// - `enabled=false` → identity (legacy full/summary/action consumers unchanged).
/// - `enabled=true` → adds `assessment_explanation` from DEC-020 chain; does not
///   re-emit prepare context body; performs no claim/completion/tool side effects.
pub fn attach_assessment_explanation(
    mut value: Value,
    options: &AttachExplanationOptions,
) -> Result<Value> {
    ensure_public_data(&value)?;
    if !options.enabled {
        // Legacy path: strip accidental field if a caller forced it off after attach.
        if let Some(obj) = value.as_object_mut() {
            obj.remove(ASSESSMENT_EXPLAIN_FIELD);
        }
        return Ok(value);
    }
    if has_assessment_explanation(&value) {
        return Ok(value);
    }

    let rendered_ctx_bytes = rendered_context_bytes(&value);
    let normalized = normalize_receipt_for_explanation(&value)?;
    let as_of = stable_as_of(&value, options.as_of);
    let delivery = options
        .delivery
        .clone()
        .unwrap_or_else(|| delivery_from_receipt(&value));
    let completion = options.completion.clone().unwrap_or_default();
    let authority = options
        .current_authority
        .clone()
        .unwrap_or_else(|| authority_from_receipt(&value));
    let action_rationale = action_rationale_from_receipt(&value);
    let project_id = options.project_id.clone().or_else(|| {
        value
            .get("project_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });

    let in_force = options
        .policy_root
        .as_ref()
        .map(|root| crate::policy_in_force(root, as_of, &PolicyCeiling::builtin()));
    let policy = match (&options.policy, &in_force) {
        (Some(policy), _) => policy.clone(),
        (None, Some(in_force)) => in_force.resolved.assessment.clone(),
        (None, None) => AssessmentPolicy::default(),
    };
    let notice = match (&options.policy, &in_force) {
        (None, Some(in_force)) => in_force.notice(),
        _ => None,
    };
    let chain = explanation_chain_from_prepare_json(
        &normalized,
        project_id,
        as_of,
        delivery,
        completion,
        authority,
        options.prior_identity.clone(),
        action_rationale,
        policy,
    )?;
    let view = build_view(chain, rendered_ctx_bytes, notice)?;
    // Hard boundary: explanation must not claim it duplicated prepare context.
    if view.metrics.prepare_context_duplicated {
        return Err(Error::InvalidInput(
            "assessment explanation must not re-embed rendered prepare context".into(),
        ));
    }
    value[ASSESSMENT_EXPLAIN_FIELD] = serde_json::to_value(view)?;
    ensure_public_data(&value)?;
    Ok(value)
}

/// Measure full wire bytes of a response value (not body text alone).
pub fn wire_bytes(value: &Value) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_prepare() -> Value {
        json!({
            "version": 1,
            "stage": "prepared",
            "project_revision": 7,
            "work": {
                "id": "01WORK",
                "external_key": "W",
                "status": "ready",
                "revision": 1,
                "source_ref": {"source_revision": "3"},
                "next_action": "Do it"
            },
            "ready": true,
            "diagnostics": [],
            "active_claims": [],
            "context": {
                "completeness": {"complete": true, "issues": [], "branch_id": "main"},
                "work_context": {
                    "rendered_context": "REQUIRED CONTEXT BODY THAT IS LONG ENOUGH TO DETECT DUPLICATION IF COPIED INTO THE ENVELOPE ACCIDENTALLY",
                    "context_hash": "abc"
                }
            },
            "context_consumed": false,
            "completion_claimed": false,
            "management": {
                "contract_fingerprint": "c1",
                "decision": {
                    "version": 1,
                    "mode": "lightweight",
                    "reasons": [],
                    "unknown_observations": [],
                    "reevaluation_signals": [],
                    "required_actions": [
                        "preserve_identity_intent_scope_and_current_state",
                        "consume_required_context_and_hard_rules",
                        "retain_completion_basis_and_actual_outcome",
                        "check_source_versions_permissions_claims_and_request_identity"
                    ],
                    "optional_maintenance": [],
                    "completion_policy": "unchanged_source_policy",
                    "execution_admission": "not_granted_by_management_classification"
                },
                "observation_basis": "host_assertion_not_independently_verified",
                "admission_gaps": [],
                "record_required": false,
                "next_action": "follow_required_actions_and_existing_workflow"
            },
            "continuity": {"state": "available", "waits": []},
            "next_action": "consume_context_then_start_or_resume_session"
        })
    }

    #[test]
    fn explain_off_preserves_legacy_shape() {
        let original = sample_prepare();
        let out = attach_assessment_explanation(
            original.clone(),
            &AttachExplanationOptions {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out, original);
        assert!(!has_assessment_explanation(&out));
    }

    #[test]
    fn explain_on_attaches_bounded_envelope_without_duplicating_context() {
        let original = sample_prepare();
        let rendered = rendered_context_bytes(&original);
        let out = attach_assessment_explanation(
            original.clone(),
            &AttachExplanationOptions {
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(has_assessment_explanation(&out));
        // Legacy prepare fields remain.
        assert_eq!(out["ready"], original["ready"]);
        assert_eq!(out["management"], original["management"]);
        assert_eq!(
            out["context"]["work_context"]["rendered_context"],
            original["context"]["work_context"]["rendered_context"]
        );
        let explanation = &out[ASSESSMENT_EXPLAIN_FIELD];
        assert_eq!(explanation["capability"], ASSESSMENT_EXPLAIN_CAPABILITY);
        assert_eq!(explanation["side_effects"]["claimed_work"], false);
        assert_eq!(explanation["side_effects"]["updated_completion"], false);
        assert_eq!(explanation["side_effects"]["auto_invoked_tools"], false);
        assert_eq!(explanation["side_effects"]["model_or_network"], false);
        assert_eq!(explanation["metrics"]["prepare_context_duplicated"], false);
        assert_eq!(explanation["metrics"]["rendered_context_bytes"], rendered);
        let full = wire_bytes(&out).unwrap();
        let legacy = wire_bytes(&original).unwrap();
        assert!(full > legacy);
        // Envelope must not contain the rendered prepare body string.
        let envelope = serde_json::to_string(&explanation["envelope"]).unwrap();
        assert!(
            !envelope.contains(
                original["context"]["work_context"]["rendered_context"]
                    .as_str()
                    .unwrap()
            )
        );
        assert_eq!(
            explanation["metrics"]["explanation_embeds_rendered_context"],
            false
        );
    }

    #[test]
    fn assess_receipt_normalizes_and_attaches() {
        let assess = json!({
            "version": 1,
            "work": "W",
            "work_id": "01WORK",
            "work_revision": 1,
            "source_ref": {"source_revision": "3"},
            "branch_id": "main",
            "contract_fingerprint": "c1",
            "decision": {
                "version": 1,
                "mode": "lightweight",
                "reasons": [],
                "unknown_observations": [],
                "reevaluation_signals": [],
                "required_actions": [
                    "preserve_identity_intent_scope_and_current_state",
                    "consume_required_context_and_hard_rules",
                    "retain_completion_basis_and_actual_outcome",
                    "check_source_versions_permissions_claims_and_request_identity"
                ],
                "optional_maintenance": [],
                "completion_policy": "unchanged_source_policy",
                "execution_admission": "not_granted_by_management_classification"
            },
            "observation": null,
            "observation_basis": "host_assertion_not_independently_verified",
            "admission_gaps": [],
            "record_required": false,
            "next_action": "follow_required_actions_and_existing_workflow",
            "project_revision": 7
        });
        let out = attach_assessment_explanation(
            assess.clone(),
            &AttachExplanationOptions {
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(has_assessment_explanation(&out));
        assert_eq!(out["decision"], assess["decision"]);
        assert_eq!(out["work"], "W");
    }

    #[test]
    fn same_inputs_yield_same_assessment_hash() {
        let prepare = sample_prepare();
        let a = attach_assessment_explanation(
            prepare.clone(),
            &AttachExplanationOptions {
                enabled: true,
                as_of: Some(7),
                ..Default::default()
            },
        )
        .unwrap();
        let b = attach_assessment_explanation(
            prepare,
            &AttachExplanationOptions {
                enabled: true,
                as_of: Some(7),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            a[ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessment_hash"],
            b[ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessment_hash"]
        );
    }

    #[test]
    fn context_consumed_and_prepare_stage_are_not_a_delivery_ack() {
        let mut prepare = sample_prepare();
        prepare["stage"] = json!("prepared");
        prepare["context_consumed"] = json!(true);
        let out = attach_assessment_explanation(
            prepare,
            &AttachExplanationOptions {
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let assessments = out[ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessments"]
            .as_array()
            .unwrap();
        assert!(
            assessments.iter().all(|item| item["id"] != "delivery.ack"),
            "context consumption must not become a delivery ack: {assessments:?}"
        );
    }

    #[test]
    fn explicit_prepared_without_ack_stays_unverified() {
        let mut prepare = sample_prepare();
        prepare["historically_prepared"] = json!(true);
        prepare["delivery_ack"] = json!(false);
        let out = attach_assessment_explanation(
            prepare,
            &AttachExplanationOptions {
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let ack = out[ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "delivery.ack")
            .unwrap();
        assert_eq!(ack["conclusion"], "not_verified");
        assert_eq!(ack["support"], "unknown");
    }

    #[test]
    fn project_revision_is_not_a_source_revision() {
        let assess = json!({
            "version": 1,
            "work": "W",
            "work_id": "01WORK",
            "work_revision": 1,
            "branch_id": "main",
            "contract_fingerprint": "c1",
            "decision": {
                "version": 1,
                "mode": "lightweight",
                "reasons": [],
                "unknown_observations": [],
                "reevaluation_signals": [],
                "required_actions": [
                    "preserve_identity_intent_scope_and_current_state",
                    "consume_required_context_and_hard_rules",
                    "retain_completion_basis_and_actual_outcome",
                    "check_source_versions_permissions_claims_and_request_identity"
                ],
                "optional_maintenance": [],
                "completion_policy": "unchanged_source_policy",
                "execution_admission": "not_granted_by_management_classification"
            },
            "observation_basis": "host_assertion_not_independently_verified",
            "admission_gaps": [],
            "record_required": false,
            "next_action": "follow_required_actions_and_existing_workflow",
            "project_revision": 7
        });
        let out = attach_assessment_explanation(
            assess,
            &AttachExplanationOptions {
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let summary = out[ASSESSMENT_EXPLAIN_FIELD]["envelope"]["identity"]["input_summary"]
            .as_str()
            .unwrap_or("");
        assert!(
            !summary.contains("source_revision=7") && !summary.contains("source_revision:7"),
            "{summary}"
        );
    }
}
