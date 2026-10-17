use awr_core::*;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use clap::Subcommand;
use serde_json::json;
use std::path::Path;

#[derive(Debug, Subcommand)]
pub enum SpecificationCommand {
    /// Check explicit approved specification refs and coverage; never writes files/runtime.
    Check {
        #[arg(long)]
        spec: Option<String>,
        /// Complete result row ceiling; an oversized selection fails without truncation.
        #[arg(long, default_value_t = 1000)]
        limit: usize,
    },
}

pub fn run(root: &Path, command: &SpecificationCommand, json_output: bool) -> Result<()> {
    let SpecificationCommand::Check { spec, limit } = command;
    let root = root.canonicalize()?;
    let runtime = crate::source::runtime_dir(&root, false)?;
    let mut store = Store::read_snapshot(&runtime.join("state.db"), 256 * 1024 * 1024)?;
    let project = store.project_by_root(&root)?;
    let manifest = Manifest::load(&root)?;
    let verify = |store: &mut Store| -> Result<()> {
        let r = index_project(store, &root, &manifest, false)?;
        if !r.ok
            || r.pending != 0
            || r.indexed != 0
            || r.retired != 0
            || r.project_revision != project.project_revision
        {
            return Err(Error::SourceStale(
                "spec check requires current indexed sources; run source reindex and inspect again"
                    .into(),
            ));
        }
        Ok(())
    };
    verify(&mut store)?;
    let mut value = awr_runtime::check_specifications(
        &store,
        &root,
        &awr_runtime::SpecificationCheckRequest {
            spec: spec.clone(),
            limit: *limit,
        },
    )?;
    verify(&mut store)?;
    value["ok"] = json!(true);
    value["project_id"] = json!(project.id);
    value["project_revision"] = json!(project.project_revision);
    value["freshness_basis"] = json!("source_verified_readonly");
    value["source_refresh_performed"] = json!(false);
    value["source_state_fingerprint"] =
        json!(awr_source::source_state_fingerprint(&store, project.id)?);
    store.ensure_source_output(&value)?;
    ensure_no_credentials_value(&value)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "Specification check: {} (declared bindings only)",
            value["status"].as_str().unwrap()
        );
        for s in value["specifications"].as_array().unwrap() {
            println!(
                "{} @ {}: {}{}",
                s["key"].as_str().unwrap(),
                s["version"].as_str().unwrap(),
                s["status"].as_str().unwrap(),
                s["diagnostic"]
                    .as_str()
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            );
        }
        for r in value["references"].as_array().unwrap() {
            println!(
                "{} -> {} / {}: {} ({})",
                r["work"].as_str().unwrap(),
                r["reference"]["specification"].as_str().unwrap(),
                r["reference"]["item"].as_str().unwrap(),
                r["status"].as_str().unwrap(),
                r["reasons"]
            );
        }
        for r in value["requirements"].as_array().unwrap() {
            println!(
                "{} / {}: {}",
                r["specification"].as_str().unwrap(),
                r["item"].as_str().unwrap(),
                r["status"].as_str().unwrap()
            );
        }
        for d in value["diagnostics"].as_array().unwrap() {
            println!("unknown: {}", d.as_str().unwrap());
        }
        println!("{}", value["boundary"].as_str().unwrap());
    }
    Ok(())
}
