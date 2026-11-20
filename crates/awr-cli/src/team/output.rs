use crate::cli_failure::{CliFailure, CliResult, terminal_json};
use serde_json::Value;

pub(super) const QUERIES: [&str; 14] = [
    "capabilities",
    "workstreams.list",
    "work.next",
    "work.inbox",
    "work.list",
    "work.search",
    "work.prepare",
    "work.snapshot",
    "work.observe",
    "session.inspect",
    "claim.inspect",
    "execution.inspect",
    "command.inspect",
    "work.recovery",
];

/// Check only the boundary needed by this client. Preserve extension fields and
/// let the service remain the authority for business schema and semantics.
pub(super) fn validate(op: &str, value: &Value) -> CliResult<()> {
    let list = |data: &Value, total: bool| {
        data["items"].is_array()
            && (data["next_cursor"].is_null() && data.get("next_cursor").is_some()
                || data["next_cursor"].is_string())
            && (!total || data["total"].as_u64().is_some())
    };
    let version = value["protocol_version"].as_u64() == Some(1);
    let valid = value.is_object()
        && value.get("code").is_none()
        && match op {
            "capabilities" => {
                version
                    && value["protocol"] == "awr-team-workstream"
                    && value["identity"].is_object()
                    && value["queries"].is_array()
                    && value["commands"].is_array()
            }
            "workstreams.list" => list(value, true),
            "work.list" | "work.search" => {
                version && value["data"].is_object() && list(&value["data"], true)
            }
            "work.next" => {
                version
                    && list(&value["data"], false)
                    && value["data"]["resume"].is_array()
                    && value["data"]["resume_truncated"].is_boolean()
                    && value["data"]["identity"].is_object()
                    && value["data"]["execution_authorized"].is_boolean()
            }
            "work.inbox" => {
                version
                    && list(&value["data"], false)
                    && value["data"]["identity"].is_object()
                    && value["data"]["execution_authorized"].is_boolean()
            }
            "work.prepare" | "work.snapshot" => {
                version
                    && value["data"].is_object()
                    && value["data"]["context_complete"].is_boolean()
                    && value["data"]["completeness_reasons"].is_array()
                    && (op != "work.snapshot"
                        || value["data"]["snapshot"].is_object()
                            && value["data"]["observation"].is_object())
            }
            "work.observe" => {
                version
                    && value["data"].is_object()
                    && value["data"]["work_id"].is_string()
                    && value["data"]["observed_at_unix_ms"].as_i64().is_some()
                    && value["data"]["execution_authorized"].is_boolean()
            }
            "session.inspect" | "work.recovery" => {
                version
                    && value["data"].is_object()
                    && value["data"]["items"].is_array()
                    && value["data"]["current_contract_hash"].is_string()
            }
            "claim.inspect" | "execution.inspect" => {
                let id = if op == "claim.inspect" {
                    "claim_id"
                } else {
                    "execution_id"
                };
                version
                    && value["data"].is_object()
                    && value["data"][id].is_string()
                    && value["data"]["state"].is_string()
            }
            "command.inspect" => {
                version
                    && value["data"].is_object()
                    && match value["data"]["state"].as_str() {
                        Some("committed") => value["data"]["receipt"].is_object(),
                        Some("unknown") => value["data"]["request_id"].is_string(),
                        Some(_) => true,
                        None => false,
                    }
            }
            _ => false,
        };
    if valid {
        Ok(())
    } else {
        Err(CliFailure::invalid_team_response(
            "query result shape or protocol version",
            200,
        ))
    }
}

pub(super) fn print(op: &str, remote: &str, value: &Value, json: bool) {
    if json {
        println!("{value}");
        return;
    }
    println!("Remote: {remote}");
    let pretty = |v: &Value| terminal_json(v, true);
    match op {
        "capabilities" => {
            println!(
                "Protocol: {} (version {})",
                value["protocol"], value["protocol_version"]
            );
            println!("Identity: {}", pretty(&value["identity"]));
            println!(
                "Service queries: {}; service commands: {} (not CLI write support)",
                value["queries"].as_array().unwrap().len(),
                value["commands"].as_array().unwrap().len()
            );
            let supported: Vec<_> = QUERIES
                .iter()
                .filter(|op| {
                    value["queries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v == **op)
                })
                .copied()
                .collect();
            println!("Available CLI reads: {}", supported.join(", "));
            println!("Use --json for full service capabilities.");
        }
        "workstreams.list" => {
            for item in value["items"].as_array().unwrap() {
                let row: serde_json::Map<_, _> =
                    ["id", "external_key", "title", "state", "authority_version"]
                        .into_iter()
                        .map(|key| {
                            (
                                key.into(),
                                item.get(key)
                                    .cloned()
                                    .unwrap_or(Value::String("unavailable".into())),
                            )
                        })
                        .collect();
                println!("{}", terminal_json(&Value::Object(row), false));
            }
            page(value, true);
        }
        "work.list" | "work.search" => {
            metadata(value);
            for item in value["data"]["items"].as_array().unwrap() {
                println!("{}", terminal_json(item, false));
            }
            page(&value["data"], true);
        }
        "work.next" | "work.inbox" => {
            metadata(value);
            // All page facts and hints remain visible, including operations beyond
            // this CLI phase. JSON rendering safely escapes terminal controls.
            println!("{}", pretty(&value["data"]));
            page(&value["data"], false);
            if op == "work.inbox"
                && value["data"]["items"].as_array().unwrap().is_empty()
                && value["data"]["next_cursor"].is_string()
            {
                println!("No coordination items on this page; more pages remain.");
            }
            if value["data"]["resume_truncated"] == true {
                println!("Resume list truncated; item pagination does not extend the resume list.");
            }
            println!(
                "Queries do not grant execution authority. Follow unsupported next_query operations through MCP."
            );
        }
        _ => {
            if value["data"]["context_complete"] == false {
                println!(
                    "Read succeeded; context is incomplete; execution admission was not evaluated."
                );
            } else {
                println!("Read succeeded; this query does not grant execution authority.");
            }
            if op == "command.inspect" && value["data"]["state"] == "unknown" {
                println!(
                    "No committed receipt is visible. This does not prove the original command failed or cannot still commit."
                );
            }
            if matches!(op, "session.inspect" | "work.recovery") {
                println!(
                    "Checkpoint actions are historical. Use team work prepare to read fresh context before continuing; this query does not resume work."
                );
            }
            println!("{}", pretty(value));
        }
    }
}

fn metadata(value: &Value) {
    let mut metadata = value.as_object().expect("validated object").clone();
    metadata.remove("data");
    println!(
        "Metadata: {}",
        terminal_json(&Value::Object(metadata), false)
    );
}

fn page(data: &Value, total: bool) {
    println!("Page items: {}", data["items"].as_array().unwrap().len());
    if total {
        println!("Total: {}", data["total"]);
    }
    println!(
        "Next cursor: {}",
        terminal_json(&data["next_cursor"], false)
    );
}
