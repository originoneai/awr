//! Read-only mainline navigation (WS-042).
use awr_core::{Error, Result};
use awr_runtime::{MainlineNavExtras, MainlineNavScope, mainline_nav};
use clap::Args;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, Args)]
pub struct NavArgs {
    /// Workstream id or external key to select.
    #[arg(long)]
    workstream: Option<String>,
    /// Exact work keys to include; repeatable.
    #[arg(long)]
    work: Vec<String>,
    /// Goal key. Selects work that declares the goal in its source (for example a ledger `goal:` field)
    /// or carries it as a tag; an unknown goal is an error, not an empty selection.
    #[arg(long)]
    goal: Option<String>,
    #[arg(long)]
    milestone: Option<String>,
    /// Optional WS-040 accounting JSON object to attach authoritatively.
    #[arg(long)]
    accounting_json: Option<String>,
    /// Use the last recorded snapshot without refreshing business sources.
    #[arg(long)]
    cached: bool,
}

pub fn run(root: &Path, args: &NavArgs, json_output: bool) -> Result<()> {
    let query = crate::query::QueryProject::open_read(root, args.cached)?;
    let mut extras = MainlineNavExtras::default();
    if let Some(raw) = &args.accounting_json {
        let value: Value = serde_json::from_str(raw)
            .map_err(|e| Error::InvalidInput(format!("accounting_json: {e}")))?;
        extras.accounting = Some(serde_json::from_value(value).map_err(|e| {
            Error::InvalidInput(format!(
                "accounting_json does not match WorkstreamAccounting: {e}"
            ))
        })?);
    }
    let scope = MainlineNavScope {
        workstream: args.workstream.clone(),
        work: args.work.clone(),
        goal: args.goal.clone(),
        milestone: args.milestone.clone(),
    };
    let mut value = mainline_nav(&query.store, &query.project, &scope, extras)?;
    for (key, item) in query.metadata().as_object().expect("query metadata") {
        value[key] = item.clone();
    }
    query.check_revision()?;
    query.finish()?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    print_human(&value);
    Ok(())
}

fn print_human(value: &Value) {
    println!(
        "Mainline navigation ({})",
        value["protocol"].as_str().unwrap_or("awr-mainline-nav")
    );
    println!(
        "Project: {}  revision {}",
        value["project"]["name"].as_str().unwrap_or("?"),
        value["project"]["revision"]
    );
    println!(
        "Scope work count: {}",
        value["scope"]["work_count"].as_u64().unwrap_or(0)
    );
    if let Some(acc) = value.get("accounting") {
        if acc["available"] == true {
            println!(
                "Accounting: required={} planned.recorded={} implemented.recorded={} (stages independent; not a goal-query rate)",
                acc["required_count"], acc["planned"]["recorded"], acc["implemented"]["recorded"]
            );
        } else {
            println!(
                "Accounting: unavailable ({})",
                acc["reason"].as_str().unwrap_or("unknown")
            );
        }
    }
    if let Some(cross) = value["cross_dependencies"].as_array() {
        println!("Cross-workstream dependencies: {}", cross.len());
        for edge in cross.iter().take(10) {
            println!(
                "  {} -> {}  outcome: {}",
                edge["from"].as_str().unwrap_or("?"),
                edge["to"].as_str().unwrap_or("?"),
                edge["outcome"].as_str().unwrap_or("?")
            );
        }
    }
    if let Some(blockers) = value["blockers"].as_array() {
        println!("Blockers: {}", blockers.len());
        for b in blockers.iter().take(10) {
            println!(
                "  {}: {}",
                b["work"].as_str().unwrap_or("?"),
                b["blocker"].as_str().unwrap_or("?")
            );
        }
    }
    if let Some(nodes) = value["mainline_graph"]["nodes"].as_array() {
        println!("Mainline nodes: {}", nodes.len());
        for n in nodes.iter().take(20) {
            println!(
                "  [{}] {} — {}",
                n["status"].as_str().unwrap_or("?"),
                n["work_key"].as_str().unwrap_or("?"),
                n["title"].as_str().unwrap_or("?")
            );
            if let Some(person) = n["person_responsibility"].as_str() {
                println!("      person: {person}");
            }
            if let Some(agent) = n.get("agent_execution") {
                if !agent.is_null() {
                    println!("      agent: {agent}");
                }
            }
            if let Some(waits) = n["explainable_waits"].as_array() {
                for w in waits.iter().take(3) {
                    println!(
                        "      wait[{}]: {}",
                        w["kind"].as_str().unwrap_or("?"),
                        w["summary"].as_str().unwrap_or("?")
                    );
                }
            }
        }
    }
    if let Some(g) = value.get("guidance") {
        println!("Guidance:");
        println!("  When: {}", g["when"].as_str().unwrap_or(""));
        println!("  Basis: {}", g["basis"].as_str().unwrap_or(""));
        println!("  Next: {}", g["next_action"].as_str().unwrap_or(""));
        println!("  Recheck: {}", g["recheck"].as_str().unwrap_or(""));
    }
    println!(
        "Read-only nav entry; Team Web collaboration writes use the WS-044 web entry ({})",
        value["authorization_note"].as_str().unwrap_or("")
    );
    let _ = json!({});
}
