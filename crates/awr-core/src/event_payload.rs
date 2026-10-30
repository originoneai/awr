//! Versioned event field contracts. Data in a generic event never grants lifecycle authority.
use crate::{Error, Id, Result};
use serde_json::{Value, json};
use std::io::Write;

pub const EVENT_PAYLOAD_SCHEMA_VERSION: u64 = 1;
pub const GENERIC_EVENT_PAYLOAD_CAP: usize = 1024 * 1024;
pub const DOMAIN_EVENT_PAYLOAD_CAP: usize = 16 * 1024 * 1024;
pub const EVENT_SUMMARY_CAP: usize = 8192;
pub const EVENT_TYPE_CAP: usize = 128;

const TEXT_FIELDS: &[(&str, usize)] = &[
    ("status", 128),
    ("tool", 256),
    ("operation", 256),
    ("error_code", 128),
    ("command", 4096),
    ("report", 4096),
    ("detail", GENERIC_EVENT_PAYLOAD_CAP),
    ("body", GENERIC_EVENT_PAYLOAD_CAP),
    ("stdout", GENERIC_EVENT_PAYLOAD_CAP),
    ("stderr", GENERIC_EVENT_PAYLOAD_CAP),
];
pub const GENERIC_EVENT_ID_FIELDS: &[&str] =
    &["source_id", "artifact_id", "checkpoint_id", "evidence_id"];

pub fn is_domain_event_type(kind: &str) -> bool {
    [
        "source.",
        "checkpoint.",
        "proposal.",
        "branch.",
        "client.",
        "execution.",
        "mcp.",
        "management.",
        "workstream.",
    ]
    .iter()
    .any(|prefix| kind.starts_with(prefix))
        || matches!(
            kind,
            "session.started"
                | "session.resumed"
                | "session.resumed_from"
                | "session.ended"
                | "session.handoff_received"
                | "session.interrupted"
                | "work.claimed"
                | "work.handoff"
                | "work.progressed"
                | "work.blocked"
                | "work.unblocked"
                | "work.cancelled"
                | "work.reopened"
                | "work.completed"
                | "work.draft_activated"
                | "work.ordinary_confirmed"
                | "work.admission"
                | "work.containment"
                | "work.reassessment"
                | "work.gap"
                | "claim.released"
                | "claim.expired"
                | "artifact.recorded"
                | "evidence.recorded"
        )
}

fn invalid() -> Error {
    // Neither an unknown field name nor its value is safe to echo into a diagnostic.
    Error::InvalidInput(
        "event payload contains an unknown field or invalid field type/value".into(),
    )
}
fn id(value: &Value) -> bool {
    value.as_str().is_some_and(|v| v.parse::<Id>().is_ok())
}
fn strings(value: &Value, count: usize, bytes: usize) -> bool {
    value.as_array().is_some_and(|v| {
        v.len() <= count
            && v.iter()
                .all(|v| v.as_str().is_some_and(|s| s.len() <= bytes))
    })
}
fn metric_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}
fn generic_fields(payload: &Value) -> Result<()> {
    for (key, value) in payload.as_object().ok_or_else(invalid)? {
        let valid = if let Some((_, cap)) = TEXT_FIELDS.iter().find(|(name, _)| key == name) {
            value.as_str().is_some_and(|v| v.len() <= *cap)
        } else if key == "mcp_operation_id" || GENERIC_EVENT_ID_FIELDS.contains(&key.as_str()) {
            id(value)
        } else {
            match key.as_str() {
                "exit_code" => value.as_i64().is_some(),
                "duration_ms" | "count" | "attempt" => value.as_u64().is_some(),
                "changed_entities" => strings(value, 256, 512),
                "tags" => strings(value, 32, 128),
                "metrics" => value.as_object().is_some_and(|m| {
                    m.len() <= 32 && m.iter().all(|(k, v)| metric_name(k) && v.is_number())
                }),
                _ => false,
            }
        };
        if !valid {
            return Err(invalid());
        }
    }
    Ok(())
}

fn domain_fields(kind: &str, payload: &Value) -> Result<()> {
    let allowed = if matches!(
        kind,
        "source.registered"
            | "source.configured"
            | "source.relocated"
            | "source.retired"
            | "source.freshness_changed"
            | "source.projected"
    ) {
        "source_id change_schema before after changes freshness fingerprint source_revision warnings workstream_moves"
    } else if matches!(kind, "proposal.apply_started") {
        "proposal_id source_id write_plan actor reason work_action source_write_confirmed"
    } else if matches!(
        kind,
        "proposal.applied"
            | "work.progressed"
            | "work.blocked"
            | "work.unblocked"
            | "work.cancelled"
            | "work.reopened"
            | "work.completed"
            | "work.draft_activated"
            | "work.ordinary_confirmed"
    ) {
        "proposal_id source_id attempt_event_id write_plan_id before_fingerprint after_fingerprint target_after_hash source_revision target_revision actor reason work_action host_edit action_reason creating_session_id released_claim_ids"
    } else if matches!(kind, "proposal.apply_conflict" | "proposal.apply_failed") {
        "proposal_id source_id attempt_event_id write_plan_id actor reason"
    } else if matches!(
        kind,
        "proposal.created"
            | "proposal.ready"
            | "proposal.approved"
            | "proposal.rejected"
            | "proposal.conflict"
            | "proposal.failed"
            | "proposal.required"
    ) {
        "action actor expected_revision from intent proposal_id proposal_revision reason source_id target_id target_key target_kind to work_action"
    } else {
        match kind {
            "workstream.conversation_selected" => "binding workstream_id",
            "client.compaction_observed" => "observation policy",
            "client.compaction_deferred" => "observation_event_id",
            "management.assessed" => "version request_key input observer assessment",
            "work.admission" => {
                "version request_key decision input bound judgment proposal_event proposer_session decider_agent independent_review_required reason"
            }
            "work.containment" => {
                "version request_key decision input bound judgment proposal_event decider_agent reason"
            }
            "work.reassessment" => {
                "version request_key decision input bound judgment root_cause decider_agent"
            }
            "work.gap" => "version request_key action input decider_agent",
            "mcp.operation_started" | "mcp.operation_finished" | "mcp.operation_recovered" => {
                "operation"
            }
            "mcp.wait_created" | "mcp.wait_replied" => "wait",
            "client.bound" | "client.updated" | "client.checkpointed" => "binding",
            "execution.registered"
            | "execution.starting"
            | "execution.running"
            | "execution.finished" => "execution",
            "execution.external_reported" => "execution_id report",
            "session.started" => {
                "agent_id provider model start_project_revision claim_id expired_claim_ids expires_at mcp_binding workstream_binding"
            }
            "work.claimed" => "claim_id expired_claim_ids agent_id expires_at",
            "session.resumed" | "session.resumed_from" => {
                "from_session_id to_session_id checkpoint_id recovery_after_revision prepared_context_hash prepared_project_revision claim_mode claim_id closed_claim_ids expired_claim_ids context_requires_refresh mcp_binding workstream_binding"
            }
            "session.handoff_received" => {
                "from_session_id to_session_id checkpoint_id recovery_after_revision prepared_context_hash prepared_project_revision claim_mode claim_id closed_claim_ids expired_claim_ids context_requires_refresh transferred_claim_id next_action open_loops mcp_binding workstream_binding"
            }
            "work.handoff" => {
                "from_session_id to_session_id checkpoint_id closed_claim_ids transferred_claim_id next_action open_loops context_requires_refresh"
            }
            "session.ended" => "status closed_claim_ids last_checkpoint_id",
            "session.interrupted" => {
                "session_id status released_claim_ids expired_claim_ids last_checkpoint_id reconciled_at reason"
            }
            "claim.released" | "claim.expired" => {
                "claim_id agent_id session_id expired_at reconciled_at reason"
            }
            "checkpoint.started" => "save_schema base_project_revision draft actor",
            "checkpoint.created" => {
                "checkpoint_id context_hash checkpoint_project_revision changed_entities next_action attempt_id session_delta workstream_binding actor"
            }
            "checkpoint.abandoned" => "attempt_id reason",
            "artifact.recorded" => "artifact_id source_event_id sha256 size locator",
            "evidence.recorded" => "evidence_id external_key level",
            "branch.created" => {
                "branch_id name parent_branch_id fork_project_revision git_binding actor reason source_copied git_write_performed"
            }
            "branch.switched" => {
                "selection actor reason git_write_performed source_write_performed"
            }
            "branch.current_cleared" => "branch_id current_branch_id reason",
            "branch.closed" | "branch.merged" | "branch.abandoned" => "closure",
            "branch.loop_carried" => "from_branch_id original target_work reason",
            _ => return Err(invalid()),
        }
    };
    let object = payload.as_object().ok_or_else(invalid)?;
    // These non-null receipt fields anchor the event to the operation that produced it.
    // Optional fields and nested snapshots still come from the typed domain constructors.
    let required = match kind {
        "workstream.conversation_selected" => "binding workstream_id",
        "client.compaction_observed" => "observation policy",
        "client.compaction_deferred" => "observation_event_id",
        "management.assessed" => "version request_key input observer assessment",
        "work.admission" | "work.containment" | "work.reassessment" => {
            "version request_key decision input bound"
        }
        "work.gap" => "version request_key action input decider_agent",
        "mcp.operation_started" | "mcp.operation_finished" | "mcp.operation_recovered" => {
            "operation"
        }
        "mcp.wait_created" | "mcp.wait_replied" => "wait",
        kind if kind.starts_with("source.") => "source_id change_schema after changes",
        "proposal.apply_started" => "proposal_id source_id write_plan source_write_confirmed",
        "proposal.applied" | "work.progressed" | "work.blocked" | "work.unblocked"
        | "work.cancelled" | "work.reopened" | "work.completed" => {
            "proposal_id source_id attempt_event_id write_plan_id source_revision target_revision"
        }
        "work.draft_activated" | "work.ordinary_confirmed" => {
            "proposal_id source_id attempt_event_id write_plan_id source_revision target_revision host_edit"
        }
        "proposal.apply_conflict" | "proposal.apply_failed" => {
            "proposal_id source_id attempt_event_id write_plan_id"
        }
        kind if kind.starts_with("proposal.") => {
            "proposal_id source_id action to proposal_revision expected_revision"
        }
        "client.bound" | "client.updated" | "client.checkpointed" => "binding",
        "execution.registered"
        | "execution.starting"
        | "execution.running"
        | "execution.finished" => "execution",
        "execution.external_reported" => "execution_id report",
        "session.started" => "agent_id provider model start_project_revision",
        "work.claimed" => "claim_id agent_id expired_claim_ids",
        "session.resumed" | "session.resumed_from" | "session.handoff_received" => {
            "from_session_id to_session_id closed_claim_ids context_requires_refresh"
        }
        // An unassigned handoff releases work and preserves its checkpoint for later pickup.
        "work.handoff" => {
            "from_session_id checkpoint_id closed_claim_ids next_action open_loops context_requires_refresh"
        }
        "session.ended" => "status closed_claim_ids",
        "session.interrupted" => {
            "session_id status released_claim_ids expired_claim_ids reconciled_at reason"
        }
        "claim.released" | "claim.expired" => "claim_id",
        "checkpoint.started" => "save_schema base_project_revision draft",
        "checkpoint.created" => {
            "checkpoint_id context_hash checkpoint_project_revision changed_entities next_action"
        }
        "checkpoint.abandoned" => "attempt_id reason",
        "artifact.recorded" => "artifact_id source_event_id sha256 size locator",
        "evidence.recorded" => "evidence_id external_key level",
        "branch.created" => {
            "branch_id name fork_project_revision source_copied git_write_performed"
        }
        "branch.switched" => "selection git_write_performed source_write_performed",
        "branch.current_cleared" => "branch_id reason",
        "branch.closed" | "branch.merged" | "branch.abandoned" => "closure",
        "branch.loop_carried" => "from_branch_id original target_work reason",
        _ => return Err(invalid()),
    };
    if required
        .split_whitespace()
        .any(|key| object.get(key).is_none_or(Value::is_null))
    {
        return Err(invalid());
    }
    for (key, value) in object {
        if key != "mcp_operation_id" && !allowed.split_whitespace().any(|name| name == key) {
            return Err(invalid());
        }
        // Nullable fields are emitted only by the typed domain operations, not generic append.
        if value.is_null() {
            continue;
        }
        let valid = match key.as_str() {
            "actor" if kind.starts_with("checkpoint.") => {
                value
                    .as_object()
                    .is_some_and(|v| v.len() == 3 && v.contains_key("agent_id"))
                    && value["identity_verified"] == false
                    && match value["origin"].as_str() {
                        Some("caller_supplied_agent") => {
                            value["agent_id"].as_str().is_some_and(|s| {
                                !s.trim().is_empty()
                                    && s.len() <= 256
                                    && !s.chars().any(char::is_control)
                            })
                        }
                        Some("undeclared" | "not_recorded") => value["agent_id"].is_null(),
                        _ => false,
                    }
            }
            "observation" if kind == "client.compaction_observed" => {
                serde_json::from_value::<crate::CompactionObservation>(value.clone()).is_ok()
            }
            "policy" if kind == "client.compaction_observed" => {
                serde_json::from_value::<crate::CompactionPolicy>(value.clone())
                    .is_ok_and(|p| p.validate().is_ok())
            }
            "version"
                if kind == "management.assessed"
                    || kind == "work.admission"
                    || kind == "work.containment"
                    || kind == "work.reassessment"
                    || kind == "work.gap" =>
            {
                value.as_u64() == Some(1)
            }
            "action" if kind == "work.gap" => {
                serde_json::from_value::<crate::GapAction>(value.clone()).is_ok()
            }
            "input" if kind == "work.gap" => {
                serde_json::from_value::<crate::GapInput>(value.clone()).is_ok()
            }
            "decider_agent" if kind == "work.gap" => value
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 256),
            "decision" if kind == "work.reassessment" => {
                serde_json::from_value::<crate::ReassessmentChoice>(value.clone()).is_ok()
            }
            "input" if kind == "work.reassessment" => value.is_object(),
            "bound" if kind == "work.reassessment" => {
                serde_json::from_value::<crate::ReassessmentBinding>(value.clone()).is_ok()
            }
            "judgment" if kind == "work.reassessment" => {
                serde_json::from_value::<crate::ReassessmentJudgment>(value.clone())
                    .is_ok_and(|j| j.validate().is_ok())
            }
            "root_cause" if kind == "work.reassessment" => {
                value.is_null()
                    || serde_json::from_value::<crate::RootCause>(value.clone())
                        .is_ok_and(|c| c.validate().is_ok())
            }
            "decider_agent" if kind == "work.reassessment" => value
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 256),
            "decision" if kind == "work.admission" || kind == "work.containment" => {
                matches!(value.as_str(), Some("proposed" | "approved" | "denied"))
            }
            "input" if kind == "work.admission" || kind == "work.containment" => value.is_object(),
            "bound" if kind == "work.admission" => {
                serde_json::from_value::<crate::AdmissionBinding>(value.clone()).is_ok()
            }
            "bound" if kind == "work.containment" => {
                serde_json::from_value::<crate::ContainmentBinding>(value.clone()).is_ok()
            }
            "judgment" if kind == "work.admission" => {
                serde_json::from_value::<crate::AdmissionJudgment>(value.clone())
                    .is_ok_and(|j| j.validate().is_ok())
            }
            "judgment" if kind == "work.containment" => {
                serde_json::from_value::<crate::ContainmentJudgment>(value.clone())
                    .is_ok_and(|j| j.validate().is_ok())
            }
            "proposal_event" | "proposer_session"
                if kind == "work.admission" || kind == "work.containment" =>
            {
                id(value)
            }
            "decider_agent" if kind == "work.admission" || kind == "work.containment" => value
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 256),
            "input" if kind == "management.assessed" => {
                value.is_object()
                    && serde_json::from_value::<crate::ManagementObservation>(
                        value["observation"].clone(),
                    )
                    .is_ok()
            }
            "assessment" if kind == "management.assessed" => {
                value.is_object()
                    && serde_json::from_value::<crate::ManagementDecision>(
                        value["decision"].clone(),
                    )
                    .is_ok()
            }
            "workstream_binding" => {
                serde_json::from_value::<crate::SessionWorkstream>(value.clone()).is_ok()
            }
            "workstream_moves" => {
                serde_json::from_value::<Vec<crate::WorkstreamMove>>(value.clone()).is_ok()
            }
            "binding" if kind == "workstream.conversation_selected" => {
                serde_json::from_value::<crate::McpSessionBinding>(value.clone())
                    .is_ok_and(|b| b.validate().is_ok())
            }
            "operation" => serde_json::from_value::<crate::McpOperation>(value.clone()).is_ok(),
            "wait" => serde_json::from_value::<crate::McpWait>(value.clone()).is_ok(),
            "mcp_binding" => serde_json::from_value::<crate::McpSessionBinding>(value.clone())
                .is_ok_and(|binding| binding.validate().is_ok()),
            "host_edit" => serde_json::from_value::<crate::HostEditBinding>(value.clone())
                .is_ok_and(|h| {
                    h.validate().is_ok()
                        && (kind != "work.draft_activated"
                            || h.action == crate::HostEditAction::ActivateDraft)
                        && (kind != "work.ordinary_confirmed"
                            || h.action == crate::HostEditAction::ConfirmOrdinary)
                }),
            "agent_id" => value.is_string(),
            key if key.ends_with("_id") => id(value),
            key if key.ends_with("_ids") => value.as_array().is_some_and(|v| v.iter().all(id)),
            key if key.ends_with("_revision") => value.as_u64().is_some(),
            "save_schema" | "change_schema" | "size" => value.as_u64().is_some(),
            "expires_at" | "expired_at" | "reconciled_at" => value.as_i64().is_some(),
            "context_requires_refresh"
            | "source_write_confirmed"
            | "source_copied"
            | "git_write_performed"
            | "independent_review_required"
            | "source_write_performed" => value.is_boolean(),
            "execution" | "binding" | "before" | "after" | "write_plan" | "work_action"
            | "draft" | "session_delta" | "git_binding" | "selection" | "closure" | "original"
            | "target_work" | "report" => value.is_object(),
            "changes" => value
                .as_array()
                .is_some_and(|v| v.iter().all(Value::is_object)),
            "warnings" | "changed_entities" | "open_loops" => {
                strings(value, usize::MAX, DOMAIN_EVENT_PAYLOAD_CAP)
            }
            _ => value.is_string(),
        };
        if !valid {
            return Err(invalid());
        }
    }
    Ok(())
}

struct CappedBuffer {
    bytes: Vec<u8>,
    cap: usize,
}
impl Write for CappedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.cap - self.bytes.len() {
            return Err(std::io::Error::other("event payload byte cap exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Validate the final event just before insertion, including events constructed by a transaction.
/// Returns the already-checked JSON so insertion cannot serialize a different payload.
pub fn checked_event_payload(
    kind: &str,
    importance: &str,
    summary: &str,
    payload: &Value,
) -> Result<String> {
    if kind.is_empty()
        || kind.len() > EVENT_TYPE_CAP
        || !kind
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
        || !["low", "normal", "high", "critical"].contains(&importance)
        || summary.trim().is_empty()
        || summary.len() > EVENT_SUMMARY_CAP
    {
        return Err(Error::InvalidInput(
            "event requires a valid type of 1..128 bytes, importance and summary of 1..8192 bytes"
                .into(),
        ));
    }
    let cap = if is_domain_event_type(kind) {
        domain_fields(kind, payload)?;
        DOMAIN_EVENT_PAYLOAD_CAP
    } else {
        generic_fields(payload)?;
        GENERIC_EVENT_PAYLOAD_CAP
    };
    let mut output = CappedBuffer {
        bytes: Vec::new(),
        cap,
    };
    serde_json::to_writer(&mut output, payload)
        .map_err(|_| Error::InvalidInput(format!("event payload exceeds {cap} byte cap")))?;
    crate::ensure_public_text(kind)?;
    crate::ensure_public_text(summary)?;
    crate::ensure_public_value(payload)?;
    Ok(String::from_utf8(output.bytes).expect("JSON serializer emits UTF-8"))
}

/// The advertised generic payload schema and runtime validation share one field inventory.
pub fn generic_event_payload_schema() -> Value {
    let mut fields = serde_json::Map::new();
    for (name, cap) in TEXT_FIELDS {
        fields.insert((*name).into(), json!({"type":"string","maxLength":cap,"description":format!("At most {cap} UTF-8 bytes.")}));
    }
    for name in GENERIC_EVENT_ID_FIELDS {
        fields.insert((*name).into(), json!({"type":"string","minLength":26,"maxLength":26,"description":"ULID of an existing object in this project."}));
    }
    fields.insert("exit_code".into(), json!({"type":"integer"}));
    for name in ["duration_ms", "count", "attempt"] {
        fields.insert(name.into(), json!({"type":"integer","minimum":0}));
    }
    for (name, count, bytes) in [("tags", 32, 128), ("changed_entities", 256, 512)] {
        fields.insert(
            name.into(),
            json!({"type":"array","maxItems":count,"items":{"type":"string","maxLength":bytes}}),
        );
    }
    fields.insert("metrics".into(), json!({"type":"object","maxProperties":32,"propertyNames":{"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_.-]+$"},"additionalProperties":{"type":"number"}}));
    json!({"type":"object","properties":fields,"additionalProperties":false,
           "description":"Generic payload schema v1. Encoded JSON is limited to 1 MiB. Fields are observations, never lifecycle authority; body/stdout/stderr are excluded from default context and search."})
}
