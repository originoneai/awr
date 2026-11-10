use awr_core::*;
use awr_runtime::{HostChange, HostSaveRequest};
use awr_store::Store;
use clap::Args;
use serde_json::{Map, json};
use std::path::Path;

#[derive(Debug, Args)]
pub struct EditArgs {
    /// Work key, or the internal work ID returned by recovery guidance.
    work: String,
    #[arg(long)]
    request_key: String,
    /// Agent identity for the retained edit receipt; does not grant permissions.
    #[arg(long)]
    actor: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    summary: Option<String>,
    #[arg(long)]
    priority: Option<String>,
    #[arg(long)]
    next_action: Option<String>,
    /// Replacement delivery criteria for a draft; repeat for each criterion.
    #[arg(long)]
    acceptance: Vec<String>,
    /// Replacement goal links for a draft; repeat for each goal.
    #[arg(long)]
    goal: Vec<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    milestone: Option<String>,
    /// Omit for preview; use exactly the returned source fingerprint when applying.
    #[arg(long, required_if_eq("accept", "true"))]
    source_fingerprint: Option<String>,
    #[arg(long)]
    accept: bool,
    #[arg(long, required_if_eq("accept", "true"))]
    expected_preview: Option<String>,
    #[arg(long, required_if_eq("accept", "true"))]
    expected_revision: Option<Revision>,
}

pub fn run(root: &Path, args: &EditArgs, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let mut store =
        Store::open_existing(&crate::source::runtime_dir(&root, false)?.join("state.db"))?;
    let mut target = args.work.clone();
    if target.parse::<Id>().is_ok() {
        let project = store.project_by_root(&root)?;
        match store.object_projection(project.id, EntityKind::WorkItem, &target) {
            Ok(projection) => {
                target = serde_json::from_value::<WorkItem>(projection.item)?
                    .meta
                    .external_key;
            }
            // Preserve receipt replay for a source record that has since been removed.
            Err(Error::NotFound(_)) if args.accept => (),
            Err(error) => return Err(error),
        }
    }
    let mut fields = Map::new();
    for (key, value) in [
        ("title", &args.title),
        ("summary", &args.summary),
        ("priority", &args.priority),
        ("next_action", &args.next_action),
        ("kind", &args.kind),
        ("milestone", &args.milestone),
    ] {
        if let Some(value) = value {
            fields.insert(key.into(), json!(value));
        }
    }
    for (key, values) in [("acceptance", &args.acceptance), ("goal", &args.goal)] {
        if !values.is_empty() {
            fields.insert(key.into(), json!(values));
        }
    }
    let request = HostSaveRequest {
        version: 1,
        request_key: args.request_key.clone(),
        actor: HostActor {
            host: "awr-cli".into(),
            subject: args.actor.clone(),
            origin: HostEditOrigin::DelegatedAgent,
        },
        reason: args.reason.clone(),
        change: HostChange::Fields {
            kind: EntityKind::WorkItem,
            target,
            source_fingerprint: args.source_fingerprint.clone().unwrap_or_default(),
            fields: json!(fields),
        },
    };
    let accept = if args.accept {
        Some((
            args.expected_revision
                .ok_or_else(|| Error::InvalidInput("apply requires --expected-revision".into()))?,
            args.expected_preview
                .as_deref()
                .ok_or_else(|| Error::InvalidInput("apply requires --expected-preview".into()))?,
        ))
    } else {
        None
    };
    let report = awr_runtime::edit_work(&mut store, &root, request, accept)?;
    if json_output {
        println!("{}", serde_json::to_string(&report.value)?);
    } else {
        println!(
            "Work edit: {}\nRequest: {}\nRevision: {}",
            report.value["status"], args.request_key, report.value["project_revision"]
        );
        if let Some(review) = report.value.get("review") {
            println!("{}", serde_json::to_string_pretty(review)?);
        }
        println!(
            "Preview: {}\nNext: {}\nOutcome query: awr host status --key {}",
            report.value["preview_fingerprint"],
            report.value["guidance"]["next_action"]
                .as_str()
                .unwrap_or(""),
            args.request_key
        );
    }
    match report.failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
