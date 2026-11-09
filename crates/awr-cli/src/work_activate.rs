use awr_core::*;
use awr_runtime::{HostChange, HostSaveRequest};
use clap::Args;
use std::path::Path;

#[derive(Debug, Args)]
pub struct ActivateArgs {
    work: String,
    #[arg(long)]
    request_key: String,
    /// Caller label retained with delegated-agent provenance.
    #[arg(long, default_value = "awr-cli")]
    actor: String,
    #[arg(long)]
    accept: bool,
    #[arg(long, required_if_eq("accept", "true"))]
    source_fingerprint: Option<String>,
    #[arg(long, required_if_eq("accept", "true"))]
    expected_preview: Option<String>,
    #[arg(long, required_if_eq("accept", "true"))]
    expected_revision: Option<Revision>,
}

pub fn run(root: &Path, args: &ActivateArgs, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let mut project = crate::query::QueryProject::open(&root)?;
    let source_fingerprint = match &args.source_fingerprint {
        Some(fp) => fp.clone(),
        None => {
            project
                .store
                .work_item(project.project.id, &args.work)?
                .source
                .fingerprint
        }
    };
    let request = HostSaveRequest {
        version: 1,
        request_key: args.request_key.clone(),
        actor: HostActor {
            host: "awr-cli".into(),
            subject: args.actor.clone(),
            origin: HostEditOrigin::DelegatedAgent,
        },
        reason: "Activate the selected draft at the caller's explicit request".into(),
        change: HostChange::ActivateDraft {
            work: args.work.clone(),
            source_fingerprint: source_fingerprint.clone(),
        },
    };
    let mut report = if args.accept {
        awr_runtime::host_save(
            &mut project.store,
            &root,
            request,
            args.expected_revision.expect("clap requires revision"),
            args.expected_preview.as_deref(),
        )?
    } else {
        awr_runtime::host_preview(&mut project.store, &root, request)?
    };
    report.value["source_fingerprint"] = serde_json::json!(source_fingerprint);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report.value)?);
    } else {
        println!("Work activation: {}", report.value["status"]);
        if !args.accept {
            println!(
                "Source fingerprint: {}\nPreview: {}\nRevision: {}",
                source_fingerprint,
                report.value["preview"]["fingerprint"],
                report.value["project_revision"]
            );
        }
    }
    match report.failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
