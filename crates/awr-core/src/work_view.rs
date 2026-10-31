//! The one shape in which a work item and its readiness are shown to a caller, whichever entry asks.
//! The CLI and the MCP server both build their `ready`, `status` and `work` replies from these functions, so
//! the same facts cannot be worded differently by one of them.
use crate::{Projected, SENSITIVE_CONTENT_WITHHELD, WorkItem, WorkReadiness, public_summary};
use serde_json::{Value, json};

/// Free text shortened for a summary line, or the marker that sensitive content was withheld.
pub fn short_text(text: &str) -> String {
    public_summary(text, 240).unwrap_or_else(|_| SENSITIVE_CONTENT_WITHHELD.into())
}

/// A work item as listed: identity, source, status, summary and the facts a caller needs to choose it.
pub fn work_brief(work: &Projected<WorkItem>) -> Value {
    json!({"id":work.item.meta.id,"external_key":work.item.meta.external_key,"title":work.item.title,"source_ref":work.item.meta.source_ref,
        "archived":work.item.archived,"ordinary_completion":work.item.ordinary_completion,"ordinary_work_policy":work.source.config["adapter_options"]["ordinary_work_policy"],
        "status":work.item.status,"raw_status":work.item.raw_status,"summary":short_text(if work.item.summary.is_empty(){&work.item.title}else{&work.item.summary}),
        "priority":work.item.priority,"milestone":work.item.milestone,"owner":work.item.owner,"next_action":work.item.next_action,
        "blocker":work.item.blocker,"revision":work.item.meta.revision,"source_revision":work.item.meta.source_ref.source_revision,"freshness":work.source.freshness})
}

/// A work item with its readiness: whether it can be selected, why not, and who holds a claim on it.
pub fn ready_brief(work: &WorkReadiness) -> Value {
    let mut value = work_brief(&work.work);
    value["ready"] = json!(work.ready);
    value["diagnostics"] = json!(work.diagnostics);
    value["active_claims"] = json!(work.active_claims.iter().map(|c| json!({"id":c.id,"agent_id":c.agent_id,"session_id":c.session_id,"expires_at":c.expires_at})).collect::<Vec<_>>());
    value
}
