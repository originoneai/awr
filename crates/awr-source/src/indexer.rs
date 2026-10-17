use crate::{
    Locator, Manifest, MarkdownDirectoryAdapter, MarkdownHeadingAdapter, MarkdownRulesAdapter,
    ParseContext, SourceAdapter, SourceSpec, YamlLedgerAdapter, observe_source,
};
use awr_core::{Error, Freshness, Id, Result, Revision, Source};
use awr_store::{SourceRegistration, Store};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Debug, Clone, Serialize)]
pub struct IndexIssue {
    pub mapping: String,
    pub locator: Option<String>,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize)]
pub struct IndexedSource {
    pub source_id: Id,
    pub locator: String,
    pub action: String,
    pub source_revision: Revision,
    pub freshness: Freshness,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct IndexReport {
    pub ok: bool,
    pub project_id: Id,
    pub project_revision: Revision,
    /// Exact event interval observed by this operation; consumers acknowledge only after processing.
    pub change_window: SourceChangeWindow,
    pub projection_complete: bool,
    pub indexed: usize,
    pub unchanged: usize,
    pub retired: usize,
    pub pending: usize,
    pub sources: Vec<IndexedSource>,
    pub issues: Vec<IndexIssue>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SourceChangeWindow {
    pub after_revision: Revision,
    pub through_revision: Revision,
}
impl IndexReport {
    fn issue(&mut self, mapping: &str, locator: Option<&str>, error: &Error) {
        self.ok = false;
        self.issues.push(IndexIssue {
            mapping: awr_core::safe_diagnostic(mapping),
            locator: locator.map(awr_core::safe_diagnostic),
            code: error.code().into(),
            message: error.report().message,
            details: error.report().details,
        });
    }
    fn source(&mut self, source: Source, action: &str, warnings: Vec<String>) {
        self.sources.push(IndexedSource {
            source_id: source.id,
            locator: source.locator,
            action: action.into(),
            source_revision: source.revision,
            freshness: source.freshness,
            warnings,
        });
    }
}

pub fn source_adapter(name: &str) -> Result<Box<dyn SourceAdapter>> {
    match name {
        "yaml-ledger-v1" => Ok(Box::new(YamlLedgerAdapter)),
        "yaml-workstream-ledger-v1" => Ok(Box::new(crate::YamlWorkstreamLedgerAdapter)),
        "markdown-ledger-v1" => Ok(Box::new(crate::MarkdownLedgerAdapter)),
        "markdown-heading-v1" => Ok(Box::new(MarkdownHeadingAdapter)),
        "markdown-rules-v1" => Ok(Box::new(MarkdownRulesAdapter)),
        "markdown-directory-v1" => Ok(Box::new(MarkdownDirectoryAdapter)),
        _ => Err(Error::Unsupported(format!("source adapter {name}"))),
    }
}
fn mapping_key(spec: &SourceSpec) -> String {
    format!(
        "{}|{}",
        spec.domain,
        spec.locator.clone().unwrap_or_else(|| spec
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default())
    )
}
/// Configuration identity shared by indexing, source mutations and read-only diagnosis.
pub fn source_configuration(spec: &SourceSpec, minimal_context: bool) -> serde_json::Value {
    let mut config = json!({"mapping_key":mapping_key(spec),"adapter_options":spec.options,"adapter_version":match spec.adapter.as_str() {
        "markdown-ledger-v1" | "yaml-ledger-v1" => 5,
        "markdown-directory-v1" => 5,
        "yaml-workstream-ledger-v1" => 3,
        _ => 2,
    }});
    if minimal_context {
        config["context_profile"] = json!("minimal");
    }
    config
}
fn source_mapping(source: &Source) -> Option<&str> {
    source.config.get("mapping_key").and_then(|v| v.as_str())
}

/// Rebuildable source facts only. Existing runtime history is preserved, never inferred from files.
pub fn index_project(
    store: &mut Store,
    root: &Path,
    manifest: &Manifest,
    force: bool,
) -> Result<IndexReport> {
    let guard = store.lock_sources()?;
    index_project_locked(store, root, manifest, force, &guard, None)
}

/// Used by a journaled relocation while holding the same lock as normal refreshes.
pub fn index_project_locked(
    store: &mut Store,
    root: &Path,
    manifest: &Manifest,
    force: bool,
    guard: &awr_store::SourceLock,
    relocation: Option<&str>,
) -> Result<IndexReport> {
    store.check_source_lock(guard)?;
    let pending = root.join(".awr/mutations/source-relocation.pending");
    if pending.exists() {
        let key = String::from_utf8(crate::read_source_capped(&pending, 256)?)
            .map_err(|_| Error::InvalidInput("invalid relocation marker".into()))?;
        if relocation != Some(key.as_str()) {
            return Err(Error::SourceConflict("source relocation pending; inspect source relocate-status and explicitly recover it".into()));
        }
    }
    if root.join(".awr/project.toml").exists()
        && serde_json::to_value(Manifest::load(root)?)? != serde_json::to_value(manifest)?
    {
        return Err(Error::SourceConflict(
            "source mapping changed while waiting for refresh".into(),
        ));
    }
    process_project(store, root, manifest, Some(force), None)
}

pub type PreviewSources = BTreeMap<String, Vec<(Locator, crate::SourceSnapshot)>>;

/// Run the actual adapters and projection constraints only in an isolated memory store.
/// Proposed intake files are supplied as bytes rather than materialized on disk.
pub fn preview_index_project(
    store: &mut Store,
    root: &Path,
    manifest: &Manifest,
    captured: &PreviewSources,
) -> Result<IndexReport> {
    store.require_memory()?;
    process_project(store, root, manifest, Some(true), Some(captured))
}

/// Refresh source registry and availability without parsing or promoting pending facts to fresh.
pub fn scan_project(store: &mut Store, root: &Path, manifest: &Manifest) -> Result<IndexReport> {
    let _guard = store.lock_sources()?;
    if root
        .join(".awr/mutations/source-relocation.pending")
        .exists()
    {
        return Err(Error::SourceConflict(
            "source relocation pending; inspect its receipt before scanning".into(),
        ));
    }
    process_project(store, root, manifest, None, None)
}

fn process_project(
    store: &mut Store,
    root: &Path,
    manifest: &Manifest,
    mode: Option<bool>,
    captured: Option<&PreviewSources>,
) -> Result<IndexReport> {
    manifest.validate()?;
    let project = store.register_project(
        root,
        manifest
            .project
            .external_key
            .as_deref()
            .unwrap_or(&manifest.project.name),
        &manifest.project.name,
    )?;
    let previous = store.sources(project.id)?;
    let mut report = IndexReport {
        ok: true,
        project_id: project.id,
        project_revision: project.project_revision,
        change_window: SourceChangeWindow {
            after_revision: project.project_revision,
            through_revision: project.project_revision,
        },
        projection_complete: false,
        indexed: 0,
        unchanged: 0,
        retired: 0,
        pending: 0,
        sources: vec![],
        issues: vec![],
    };
    let mut desired = BTreeSet::new();
    let mut owners = BTreeMap::new();
    let mut plans = vec![];
    // Resolve all mappings first so aliases/overlapping directories cannot silently acquire one source twice.
    for spec in &manifest.sources {
        let key = mapping_key(spec);
        desired.insert(key.clone());
        let adapter = source_adapter(&spec.adapter)?;
        let discovered = if let Some(captured) = captured {
            Ok(captured
                .get(&key)
                .ok_or_else(|| {
                    Error::InvalidInput("preview source inventory is incomplete".into())
                })?
                .iter()
                .map(|(locator, _)| locator.clone())
                .collect())
        } else {
            adapter.discover(root, manifest, spec)
        }
        .and_then(|locators| {
            locators
                .into_iter()
                .map(|locator| {
                    let identity = if spec.adapter == "markdown-directory-v1" {
                        MarkdownDirectoryAdapter.source_identity(root, manifest, spec, &locator)?
                    } else {
                        locator.identity()?
                    };
                    Ok((locator, identity))
                })
                .collect::<Result<Vec<_>>>()
        });
        if let Ok(files) = &discovered {
            for (_, identity) in files {
                if let Some(other) =
                    owners.insert((spec.domain.clone(), identity.clone()), key.clone())
                {
                    return Err(Error::SourceConflict(format!(
                        "source {identity} belongs to overlapping mappings {other} and {key}"
                    )));
                }
            }
        }
        plans.push((key, spec, adapter, discovered));
    }
    // The complete workstream ledger validates goal references against the
    // candidate's already indexed supporting sources, regardless of TOML order.
    plans.sort_by_key(|(_, spec, _, _)| spec.adapter == "yaml-workstream-ledger-v1");
    let mut seen = BTreeSet::new();
    let mut discovered_mappings = BTreeSet::new();
    for (key, spec, adapter, discovered) in plans {
        let files = match discovered {
            Ok(files) => {
                discovered_mappings.insert(key.clone());
                files
            }
            Err(error) => {
                report.issue(&key, None, &error);
                let freshness = if matches!(error, Error::SourceUnavailable(_) | Error::Io(_)) {
                    Freshness::Unavailable
                } else {
                    Freshness::Stale
                };
                for source in previous
                    .iter()
                    .filter(|source| source_mapping(source) == Some(&key))
                {
                    match store.mark_source_freshness(source, freshness) {
                        Ok(source) => report.source(source, "unavailable", vec![]),
                        Err(error) => report.issue(&key, Some(&source.locator), &error),
                    }
                }
                continue;
            }
        };
        for (locator, identity) in files {
            let mut attempts = 0;
            let outcome = loop {
                attempts += 1;
                let result = index_one(
                    store,
                    root,
                    spec,
                    adapter.as_ref(),
                    &locator,
                    &identity,
                    mode,
                    manifest.project.context_profile == crate::ContextProfile::Minimal,
                    captured.and_then(|all| all.get(&key)).and_then(|files| {
                        files
                            .iter()
                            .find(|(path, _)| path.identity().ok() == locator.identity().ok())
                            .map(|(_, snapshot)| snapshot)
                    }),
                );
                if matches!(result, Err(Error::RevisionConflict { .. })) && attempts < 4 {
                    continue;
                }
                break result;
            };
            match outcome {
                Ok((source, indexed, warnings)) => {
                    seen.insert(source.id);
                    let action = match indexed {
                        Some(true) => {
                            report.indexed += 1;
                            "indexed"
                        }
                        Some(false) => {
                            report.unchanged += 1;
                            "unchanged"
                        }
                        None => {
                            report.pending += 1;
                            "pending"
                        }
                    };
                    report.source(source, action, warnings);
                }
                Err(error) => {
                    report.issue(&key, Some(&identity), &error);
                    // Report the stored state. Do not downgrade a concurrent successful writer after a conflict.
                    if let Some(source) = store
                        .sources(project.id)?
                        .into_iter()
                        .find(|source| source.domain == spec.domain && source.locator == identity)
                    {
                        seen.insert(source.id);
                        report.source(source, "failed", vec![]);
                    }
                }
            }
        }
    }
    for old in previous {
        let Some(key) = source_mapping(&old) else {
            continue;
        };
        if seen.contains(&old.id) {
            continue;
        }
        // A failed directory scan cannot prove deletion. A removed manifest mapping can.
        if !desired.contains(key) || discovered_mappings.contains(key) {
            match store.retire_source(&old) {
                Ok(()) => {
                    report.retired += 1;
                    let mut retired = old;
                    retired.freshness = Freshness::Stale;
                    retired.revision += 1;
                    report.source(retired, "retired", vec![]);
                }
                Err(error) => report.issue(key, Some(&old.locator), &error),
            }
        }
    }
    if mode.is_some() && report.ok {
        if let Err(error) = store.workstream_catalog(project.id) {
            report.issue("workstream_catalog", None, &error);
        }
    }
    report.project_revision = store.project(project.id)?.project_revision;
    report.change_window.through_revision = report.project_revision;
    report.projection_complete = report.ok && report.pending == 0;
    Ok(report)
}

fn index_one(
    store: &mut Store,
    root: &Path,
    spec: &SourceSpec,
    adapter: &dyn SourceAdapter,
    locator: &Locator,
    identity: &str,
    mode: Option<bool>,
    minimal_context: bool,
    captured: Option<&crate::SourceSnapshot>,
) -> Result<(Source, Option<bool>, Vec<String>)> {
    let project = store.project_by_root(root)?;
    let source = store.register_source(
        project.id,
        &SourceRegistration {
            domain: &spec.domain,
            role: &spec.role,
            locator: identity,
            format: if matches!(
                spec.adapter.as_str(),
                "yaml-ledger-v1" | "yaml-workstream-ledger-v1"
            ) {
                "yaml"
            } else {
                "markdown"
            },
            adapter: &spec.adapter,
        },
    )?;
    let source = store.configure_source(&source, source_configuration(spec, minimal_context))?;
    let cap = crate::source_read_cap(&spec.adapter)?;
    let observed = if let Some(snapshot) = captured {
        if snapshot.bytes.len() as u64 > cap {
            return Err(Error::InvalidInput(
                "proposed source exceeds its adapter read cap".into(),
            ));
        }
        snapshot.validate_content()?;
        crate::SourceObservation {
            source: store.mark_source_freshness(&source, Freshness::Stale)?,
            changed: true,
            snapshot: Some(snapshot.clone()),
            error: None,
        }
    } else {
        observe_source(store, &source, root, locator, cap)?
    };
    if let Some(error) = observed.error {
        return Err(error);
    }
    let snapshot = observed
        .snapshot
        .clone()
        .ok_or_else(|| Error::SourceUnavailable("source returned no snapshot".into()))?;
    let mut source = observed.source;
    // Only a healthy projection may take the unchanged fast path. Byte
    // equality alone cannot prove the indexed state is current: after a
    // failed projection or a retirement the stored fingerprint still holds
    // the last successful value, so unchanged bytes must reproject to heal
    // the retained error or re-activate retired entities. Skipping that
    // reprojection would leave the projection stuck (for example a restored
    // malformed ledger keeps reporting source_unreadable, or a re-added
    // identical file stays retired). A configuration change or a read
    // failure (changed=false, snapshot=None) also takes the path below.
    if mode != Some(true)
        && !observed.changed
        && source.freshness == Freshness::Fresh
        && store.source_review_matches(&source, snapshot.content_review.as_ref())?
    {
        let warnings = store.source_warnings(&source)?;
        return Ok((source, Some(false), warnings));
    }
    if mode.is_none() {
        let warnings = store.source_warnings(&source)?;
        return Ok((source, None, warnings));
    }
    source = store.mark_source_freshness(&source, Freshness::Stale)?;
    let context = ParseContext {
        source: &source,
        existing_ids: store.projection_ids(&source)?,
    };
    let batch = adapter.parse(&snapshot, &context, spec)?;
    let warnings = batch.warnings.clone();
    if captured.is_none()
        && matches!(locator, Locator::File(_))
        && locator.read(root, cap)?.fingerprint != snapshot.fingerprint
    {
        return Err(Error::SourceConflict(
            "source changed while parsing; reindex required".into(),
        ));
    }
    adapter.project(store, &source, &snapshot, batch)?;
    Ok((store.source(project.id, source.id)?, Some(true), warnings))
}
