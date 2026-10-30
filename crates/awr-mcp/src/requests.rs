//! Durable request identities and recovery from committed domain receipts.
use crate::{input_diagnostics::parse_tool, project::database};
use awr_core::*;
use awr_store::Store;
use rmcp::model::{CallToolResult, JsonObject};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

fn open(root: &Path) -> Result<(Store, Project)> {
    let store = Store::open_existing(&database(root)?)?;
    let project = store.project_by_root(root)?;
    Ok((store, project))
}
fn replay(operation: &McpOperation, revision: Revision, replayed: bool) -> CallToolResult {
    let mut value = operation
        .result
        .clone()
        .unwrap_or_else(|| json!({"ok":false,"write_outcome":"unknown"}));
    value["receipt_project_revision"] = value
        .get("project_revision")
        .cloned()
        .unwrap_or(Value::Null);
    value["project_revision"] = json!(revision);
    value["operation"] = json!({"id":operation.id,"request_id":operation.request_id,"status":operation.status,"replayed":replayed});
    if operation.is_error.unwrap_or(true) {
        CallToolResult::structured_error(value)
    } else {
        CallToolResult::structured(value)
    }
}
fn observation(store: &Store, project: &Project, operation: &McpOperation) -> Result<Value> {
    let events = store.mcp_operation_events(project.id, operation.id, 1000)?;
    Ok(
        json!({"ok":true,"read_only":true,"source_refresh_performed":false,"freshness_basis":"runtime_database",
        "project_revision":project.project_revision,"operation":operation,
        "outcome":if operation.status=="started"{"unknown"}else{"recorded"},
        "domain_receipts":events.iter().map(|e|json!({"id":e.id,"event_type":e.event_type,"project_revision":e.project_revision,"session_id":e.session_id,"work_item_id":e.work_item_id})).collect::<Vec<_>>(),
        "receipts_may_have_more":events.len()==1000,
        "next_action":if operation.status=="started"{"Do not replay. Inspect domain receipts; awr_operation_recover can record a proven committed outcome."}else{"Read the recorded result; compile current context before continuing."}}),
    )
}
fn unknown(store: &Store, project: &Project, operation: &McpOperation) -> Result<CallToolResult> {
    let mut value = observation(store, project, operation)?;
    value["ok"] = json!(false);
    value["write_outcome"] = json!("unknown");
    value["error"] = json!(
        Error::MutationConflict(
            "MCP request has no final response receipt; query or recover it before retrying".into()
        )
        .report()
    );
    Ok(CallToolResult::structured_error(value))
}
pub(crate) fn existing(
    root: &Path,
    client: &str,
    request: &str,
    name: &str,
    args: &JsonObject,
) -> Result<Option<CallToolResult>> {
    let store = Store::open_readonly(&database(root)?)?;
    let project = store.project_by_root(root)?;
    let Some(operation) = store.mcp_operation(project.id, client, request)? else {
        return Ok(None);
    };
    if operation.fingerprint != fingerprint(name, args)? {
        return Err(Error::SourceConflict(
            "request_id was already used with different arguments".into(),
        ));
    }
    Ok(Some(if operation.status == "started" {
        unknown(&store, &project, &operation)?
    } else {
        replay(&operation, project.project_revision, true)
    }))
}
fn fingerprint(name: &str, args: &JsonObject) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!({"tool":name,"arguments":args}))?)
    ))
}
pub(crate) fn execute(
    root: &Path,
    client: &str,
    request: &str,
    name: &str,
    original: &JsonObject,
    mut args: JsonObject,
    apply: impl FnOnce(JsonObject) -> Result<CallToolResult>,
) -> Result<CallToolResult> {
    let expected = args
        .get("expected_revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::InvalidInput("identified writes require expected_revision".into()))?;
    let (mut store, project) = open(root)?;
    let operation = store
        .begin_mcp_operation(
            project.id,
            expected,
            client,
            request,
            name,
            &fingerprint(name, original)?,
        )
        .map_err(|error| {
            if name == "awr_session_checkpoint" {
                error.for_checkpoint()
            } else {
                error
            }
        })?;
    args.insert(
        "expected_revision".into(),
        json!(operation.started_revision),
    );
    drop(store);
    let result = awr_store::with_mcp_operation(project.id, operation.id, || apply(args));
    let result = match result {
        Ok(result) => result,
        Err(error) => CallToolResult::structured_error(serde_json::to_value(error.report())?),
    };
    let mut value = result
        .structured_content
        .unwrap_or_else(|| json!({"content":result.content}));
    if value.get("read_only") == Some(&json!(true)) {
        value["domain_read_only"] = json!(true);
        value["read_only"] = json!(false);
    }
    for _ in 0..4 {
        let (mut store, project) = open(root)?;
        if let Some(current) = store.mcp_operation(project.id, client, request)? {
            if current.status != "started" {
                return Ok(replay(&current, project.project_revision, true));
            }
        }
        match store.finish_mcp_operation(
            project.id,
            project.project_revision,
            &operation,
            value.clone(),
            result.is_error.unwrap_or(false),
            false,
        ) {
            Ok((saved, event)) => return Ok(replay(&saved, event.project_revision, false)),
            Err(Error::RevisionConflict { .. }) => continue, // Retry bookkeeping only; never the domain action.
            Err(_) => return unknown(&store, &project, &operation),
        }
    }
    let (store, project) = open(root)?;
    unknown(&store, &project, &operation)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recover {
    request_id: String,
    expected_revision: Revision,
}
pub(crate) fn inspect(root: &Path, args: Value, client: &str) -> Result<CallToolResult> {
    let args: Inspect = parse_tool!("awr_operation_get", args);
    let store = Store::read_snapshot(&database(root)?, 256 * 1024 * 1024)?;
    let project = store.project_by_root(root)?;
    let operation = store
        .mcp_operation(project.id, client, &args.request_id)?
        .ok_or_else(|| Error::NotFound("MCP request".into()))?;
    let value = observation(&store, &project, &operation)?;
    ensure_public_value(&value)?;
    Ok(CallToolResult::structured(value))
}
pub(crate) fn recover(root: &Path, args: Value, client: &str) -> Result<CallToolResult> {
    let args: Recover = parse_tool!("awr_operation_recover", args);
    let (mut store, project) = open(root)?;
    let operation = store
        .mcp_operation(project.id, client, &args.request_id)?
        .ok_or_else(|| Error::NotFound("MCP request".into()))?;
    if operation.status != "started" {
        return Ok(replay(&operation, project.project_revision, true));
    }
    let events = store.mcp_operation_events(project.id, operation.id, 1000)?;
    let terminal = |event: &Event| match operation.tool.as_str() {
        "awr_session_start" => event.event_type == "session.started",
        "awr_session_resume" => event.event_type == "session.resumed",
        "awr_session_checkpoint" => event.event_type == "checkpoint.created",
        "awr_session_end" => event.event_type == "session.ended",
        "awr_session_claim" => matches!(
            event.event_type.as_str(),
            "work.claimed" | "claim.released" | "claim.expired"
        ),
        "awr_session_wait" => event.event_type == "mcp.wait_created",
        "awr_session_reply" => event.event_type == "mcp.wait_replied",
        "awr_evidence_record" => event.event_type == "evidence.recorded",
        "awr_work_manage" => event.event_type == "management.assessed",
        "awr_work_admit" => event.event_type == "work.admission",
        "awr_work_contain" => event.event_type == "work.containment",
        "awr_work_reassess" => event.event_type == "work.reassessment",
        "awr_work_gap" => event.event_type == "work.gap",
        "awr_compaction_observe" => event.event_type == "client.compaction_observed",
        "awr_compaction_defer" => event.event_type == "client.compaction_deferred",
        "awr_event_append" => !is_domain_event_type(&event.event_type),
        "awr_work_transition" => matches!(
            event.event_type.as_str(),
            "work.progressed"
                | "work.blocked"
                | "work.unblocked"
                | "work.cancelled"
                | "work.reopened"
                | "work.completed"
        ),
        _ => false,
    };
    let committed: Vec<_> = events.iter().filter(|e| terminal(e)).collect();
    // Absence of a receipt is not proof of absence of a side effect or of a live worker.
    if committed.len() != 1 || events.len() == 1000 {
        return unknown(&store, &project, &operation);
    }
    let mut value = observation(&store, &project, &operation)?;
    value["read_only"] = json!(false);
    value["write_outcome"] = json!("committed");
    value["recovered"] = json!(true);
    value["outcome"] = json!("committed");
    value["continuation_requires_context"] = json!(true);
    value["next_action"] = json!(
        "Outcome recovered from the bound domain receipt without replay. Inspect the session/work and compile fresh context."
    );
    value.as_object_mut().unwrap().remove("operation");
    let (saved, event) = store.finish_mcp_operation(
        project.id,
        args.expected_revision,
        &operation,
        value,
        false,
        true,
    )?;
    Ok(replay(&saved, event.project_revision, false))
}
