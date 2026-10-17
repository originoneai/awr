//! Optional presentation only. Domain checks and stored receipts always use full results.
use serde_json::{Value, json};

/// Opt-in low-noise prepare presentation. Only optional selection indexes move
/// to the existing full-result query; required facts and diagnostics stay inline.
pub fn compact_prepared_response(mut value: Value) -> Value {
    if value["ok"] != true
        || value["stage"] != "prepared"
        || value["context"]["completeness"]["complete"] != true
        || !matches!(
            value["response_view"]["view"].as_str(),
            Some("summary" | "action")
        )
        || !value["response_view"]["full_result"].is_object()
        || !value["response_view"]["omitted_fields"].is_array()
    {
        return value;
    }
    let omitted = &value["context"]["work_context"]["omitted_chunks"];
    if let Some(chunks) = omitted.as_array().filter(|chunks| {
        !chunks.is_empty()
            && chunks
                .iter()
                .all(|c| c["reason"] == "insufficient_budget_for_whole_chunk")
    }) {
        let count = chunks.len();
        value["context"]["work_context"]
            .as_object_mut()
            .unwrap()
            .remove("omitted_chunks");
        let path = "context.work_context.omitted_chunks";
        value["response_view"]["omitted_fields"]
            .as_array_mut()
            .unwrap()
            .push(json!(path));
        value["response_view"]["detail_counts"] = json!({path:count});
    }
    value
}

pub fn summarize_work_response(mut value: Value) -> Value {
    // Preserve failures (including partial writes and unknown outcomes) verbatim.
    if value["ok"] != true {
        return value;
    }
    let mut omitted = vec![];
    if value["stage"] == "prepared" {
        if let Some(pack) = value["context"]["work_context"].as_object_mut() {
            // These are indexes of the rendered packet, not its content or provenance.
            for field in ["selected_chunks", "selected_entities"] {
                if pack.remove(field).is_some() {
                    omitted.push(format!("context.work_context.{field}"));
                }
            }
        }
    } else if value["proposal"]["status"] == "applied" && value["write_outcome"] == "applied" {
        if let Some(patch) = value["proposal"]
            .as_object_mut()
            .and_then(|p| p.remove("patch"))
        {
            value["transition"] = patch["work_action"].clone();
            value["changes"] = patch["changes"].clone();
            value["target"] = patch["target"].clone();
            value["intent"] = patch["intent"].clone();
            omitted.push("proposal.patch".into());
            if let Some(payload) = value["event"]
                .as_object_mut()
                .and_then(|e| e.remove("payload"))
            {
                value["event"]["outcome"] = json!({"released_claim_ids":payload["released_claim_ids"],
                    "source_revision":payload["source_revision"],"target_revision":payload["target_revision"],
                    "after_fingerprint":payload["after_fingerprint"]});
                omitted.push("event.payload".into());
            }
        }
    }
    if !omitted.is_empty() {
        value["response_view"] = json!({"version":1,"view":"summary","omitted_fields":omitted});
    }
    value
}
