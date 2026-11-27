//! Development intake for one source mapping; source CLI orchestration is tracked separately.
use awr_core::{EntityKind, Error, Freshness, Result};
use awr_source::{
    GithubPrSnapshotAdapter, Locator, Manifest, MarkdownHeadingAdapter, MarkdownRulesAdapter,
    ParseContext, SourceAdapter, YamlLedgerAdapter, observe_source,
};
use awr_store::{SourceRegistration, Store};
use serde_json::json;
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 4 {
        return Err(Error::InvalidInput(
            "index_source <root> <manifest-path> <source-index> <database>".into(),
        ));
    }
    let root = PathBuf::from(&args[0]).canonicalize()?;
    let manifest = Manifest::parse(&fs::read_to_string(&args[1])?)?;
    let index = args[2]
        .parse::<usize>()
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    let spec = manifest
        .sources
        .get(index)
        .ok_or_else(|| Error::InvalidInput("source index outside manifest".into()))?;
    let adapter: Box<dyn SourceAdapter> = match spec.adapter.as_str() {
        "yaml-ledger-v1" => Box::new(YamlLedgerAdapter),
        "markdown-heading-v1" => Box::new(MarkdownHeadingAdapter),
        "markdown-rules-v1" => Box::new(MarkdownRulesAdapter),
        "github-pr-snapshot-v1" => Box::new(GithubPrSnapshotAdapter),
        _ => return Err(Error::Unsupported(spec.adapter.clone())),
    };
    let locator = Locator::from_spec(&root, &manifest, spec)?;
    let snapshot = locator.read(&root, 16 * 1024 * 1024)?;
    let mut store = Store::open(&PathBuf::from(&args[3]))?;
    let project = store.register_project(
        &root,
        manifest
            .project
            .external_key
            .as_deref()
            .unwrap_or(&manifest.project.name),
        &manifest.project.name,
    )?;
    let source = store.register_source(
        project.id,
        &SourceRegistration {
            domain: &spec.domain,
            role: &spec.role,
            locator: &snapshot.locator,
            format: if spec.adapter == "yaml-ledger-v1" {
                "yaml"
            } else if spec.adapter == "github-pr-snapshot-v1" {
                "json"
            } else {
                "markdown"
            },
            adapter: adapter.name(),
        },
    )?;
    let observed = observe_source(&mut store, &source, &root, &locator, 16 * 1024 * 1024)?;
    if let Some(error) = observed.error {
        return Err(error);
    }
    let snapshot = observed
        .snapshot
        .ok_or_else(|| Error::SourceUnavailable("missing snapshot".into()))?;
    let mut warnings = vec![];
    if observed.changed || observed.source.freshness != Freshness::Fresh {
        let context = ParseContext {
            source: &observed.source,
            existing_ids: store.projection_ids(&source)?,
        };
        let batch = adapter.parse(&snapshot, &context, spec)?;
        warnings = batch.warnings.clone();
        adapter.project(&mut store, &observed.source, &snapshot, batch)?;
    }
    let source = store.source(project.id, source.id)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"source_id":source.id,"source_revision":source.revision,
        "freshness":source.freshness,"fingerprint":source.fingerprint,"project_revision":store.project(project.id)?.project_revision,
        "goals":store.source_projection_payloads(&source,EntityKind::Goal)?.len(),
        "plans":store.source_projection_payloads(&source,EntityKind::Plan)?.len(),
        "rules":store.source_projection_payloads(&source,EntityKind::Rule)?.len(),
        "work_items":store.source_projection_payloads(&source,EntityKind::WorkItem)?.len(),"warnings":warnings})
        )?
    );
    Ok(())
}
