use awr_core::*;
use awr_runtime::{HostChange, HostSaveRequest};
use clap::Args;
use std::path::Path;

#[derive(Debug, Args)]
pub struct ActivateArgs {
    /// Work key, or the internal work ID returned by recovery guidance.
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
    let mut key = args.work.clone();
    let mut source_fingerprint = args.source_fingerprint.clone();
    if source_fingerprint.is_none() || key.parse::<Id>().is_ok() {
        match project
            .store
            .object_projection(project.project.id, EntityKind::WorkItem, &key)
        {
            Ok(selected) => {
                key = serde_json::from_value::<WorkItem>(selected.item)?
                    .meta
                    .external_key;
                if source_fingerprint.is_none() {
                    source_fingerprint = Some(selected.source.fingerprint);
                }
            }
            // A completed receipt remains replayable after the source record is removed.
            Err(Error::NotFound(_)) if args.accept && source_fingerprint.is_some() => (),
            Err(error) => return Err(error),
        }
    }
    let source_fingerprint =
        source_fingerprint.expect("preview resolved the source or accept supplied it");
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
            work: key.clone(),
            source_fingerprint: source_fingerprint.clone(),
        },
    };
    let result = if args.accept {
        awr_runtime::host_save(
            &mut project.store,
            &root,
            request,
            args.expected_revision.expect("clap requires revision"),
            args.expected_preview.as_deref(),
        )
    } else {
        awr_runtime::host_preview(&mut project.store, &root, request)
    };
    let mut report = result.map_err(|error| {
        if let Some(next) = crate::help::definition_error_next_action(&serde_json::json!(error.report())) {
            error.with_details(serde_json::json!({"next_action":next}))
        } else if error.report().details.as_ref().is_some_and(|details| details["next_action"].is_string()) {
            error
        } else {
            let command = cli_next_action(&["awr", "work", "show", &key]);
            error.with_details(serde_json::json!({"next_action":if command.len() <= 300 && !command.chars().any(char::is_control) {command} else {"awr status".into()}}))
        }
    })?;
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
