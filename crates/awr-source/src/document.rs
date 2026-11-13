//! Exact document edits. Prose is opaque; the registered adapter owns its metadata.
use crate::*;
use awr_core::{DecisionStatus, EntityKind, Freshness, Id, Source};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentEdit {
    Replace { text: String },
    Fragment { before: String, after: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentAction {
    Edit {
        source_id: Id,
        source_fingerprint: String,
        edit: DocumentEdit,
    },
    CreateDraft {
        path: PathBuf,
        title: String,
        body: String,
    },
}
pub struct PreparedDocument {
    pub path: PathBuf,
    pub source: Source,
    pub spec: SourceSpec,
    pub before: Option<SourceSnapshot>,
    pub after: SourceSnapshot,
}
fn snapshot(path: &Path, text: String) -> Result<SourceSnapshot> {
    let bytes = text.into_bytes();
    let locator = Locator::File(path.into()).identity()?;
    crate::limits::check_source_size_only(&bytes, MARKDOWN_READ_CAP)?;
    awr_core::ensure_public_bytes(&bytes)?;
    Ok(SourceSnapshot {
        content_review: None,
        locator,
        fingerprint: fingerprint(&bytes),
        bytes,
    })
}
fn invariant(batch: &ProjectionBatch) -> BTreeMap<(EntityKind, String), Value> {
    let mut values = BTreeMap::new();
    for g in &batch.goals {
        values.insert(
            (EntityKind::Goal, g.meta.external_key.clone()),
            json!([g.status]),
        );
    }
    for p in &batch.plans {
        values.insert(
            (EntityKind::Plan, p.meta.external_key.clone()),
            json!([p.status, p.kind]),
        );
    }
    for r in &batch.rules {
        values.insert(
            (EntityKind::Rule, r.meta.external_key.clone()),
            json!([r.severity, r.scope]),
        );
    }
    for d in &batch.decisions {
        values.insert(
            (EntityKind::Decision, d.meta.external_key.clone()),
            json!([d.status, d.raw_status]),
        );
    }
    values
}
/// Only one registered mapping may own the target path, including aliases in other domains.
pub fn document_path_registration(root: &Path, path: &Path) -> Result<SourceSpec> {
    let manifest = Manifest::load(root)?;
    let mut matches = Vec::new();
    for spec in &manifest.sources {
        let Locator::File(base) = Locator::from_spec(root, &manifest, spec)? else {
            continue;
        };
        let matches_path = if spec.adapter == "markdown-directory-v1" {
            path.strip_prefix(&base).ok().is_some_and(|relative| {
                !relative.as_os_str().is_empty()
                    && relative.components().all(|part| matches!(part,Component::Normal(name) if !name.to_string_lossy().starts_with('.')))
                    && (spec.options.get("recursive").and_then(toml::Value::as_bool).unwrap_or(true) || relative.components().count()==1)
            })
        } else {
            base == path
        };
        if matches_path {
            matches.push(spec.clone());
        }
    }
    if matches.len() != 1 {
        return Err(Error::SourceConflict(format!(
            "document target requires exactly one registered mapping; found {}",
            matches.len()
        )));
    }
    let spec = matches.pop().unwrap();
    if ![
        "markdown-heading-v1",
        "markdown-rules-v1",
        "markdown-directory-v1",
    ]
    .contains(&spec.adapter.as_str())
    {
        return Err(Error::MutationUnsupported(
            "document editing excludes work ledgers and unrecognized adapters".into(),
        ));
    }
    Ok(spec)
}
/// Re-validate declarations even where a heading adapter does not consume front matter.
fn declarations(snapshot: &SourceSnapshot, spec: &SourceSpec) -> Result<()> {
    let sections = markdown_sections(snapshot)?;
    let mut meta = crate::directory::frontmatter(snapshot.text()?)?;
    if let Some(header) = sections.iter().find(|s| s.level > 0) {
        for (key, value) in crate::directory::header_fields(&header.body)? {
            if ["status", "id", "title"].contains(&key.as_str()) {
                crate::directory::insert_metadata(&mut meta, &key, json!(value))?;
            }
        }
        if let Some(status) = header.attributes.get("status") {
            crate::directory::insert_metadata(&mut meta, "status", json!(status))?;
        }
    }
    // A heading source cannot silently ignore an alternate document status declaration.
    if spec.adapter != "markdown-directory-v1" && !meta["status"].is_null() {
        let effective = sections
            .iter()
            .find(|s| s.level > 0)
            .and_then(|s| s.attributes.get("status").map(String::as_str))
            .or_else(|| spec.options.get("status").and_then(toml::Value::as_str))
            .unwrap_or("unknown");
        if meta["status"].as_str() != Some(effective) {
            return Err(Error::SourceConflict("document metadata status differs from the registered heading declaration; select and reconcile the locations first".into()));
        }
    }
    Ok(())
}
pub fn prepare_document_edit(
    root: &Path,
    source: &Source,
    expected: &str,
    edit: &DocumentEdit,
    ids: BTreeMap<(EntityKind, String), Id>,
) -> Result<PreparedDocument> {
    let (locator, spec, before) = inspect_registered_source(root, source)?;
    let Locator::File(path) = locator else {
        return Err(Error::MutationUnsupported(
            "Git document snapshots are read-only".into(),
        ));
    };
    document_path_registration(root, &path)?;
    if expected != before.fingerprint || source.fingerprint != before.fingerprint {
        return Err(Error::SourceConflict(
            "document source changed since the edit was prepared".into(),
        ));
    }
    let text = before.text()?;
    let new_text = match edit {
        DocumentEdit::Replace { text } => text.clone(),
        DocumentEdit::Fragment { before, after } => {
            if before.is_empty() || text.match_indices(before).count() != 1 {
                return Err(Error::SourceConflict(
                    "document fragment must match exactly once; use an explicit longer fragment"
                        .into(),
                ));
            }
            text.replacen(before, after, 1)
        }
    };
    let after = snapshot(&path, new_text)?;
    declarations(&before, &spec)?;
    declarations(&after, &spec)?;
    let adapter = source_adapter(&spec.adapter)?;
    let context = ParseContext {
        source,
        existing_ids: ids,
    };
    let prior = adapter.parse(&before, &context, &spec)?;
    let next = adapter.parse(&after, &context, &spec)?;
    if invariant(&prior) != invariant(&next) {
        return Err(Error::RuleViolation("document body edits must retain object keys, status and rule constraints; adoption and lifecycle changes require explicit domain actions".into()));
    }
    Ok(PreparedDocument {
        path,
        source: source.clone(),
        spec,
        before: Some(before),
        after,
    })
}
pub fn prepare_document_draft(
    root: &Path,
    project: Id,
    path: &Path,
    title: &str,
    body: &str,
    key: &str,
) -> Result<PreparedDocument> {
    if path.is_absolute() || path.components().any(|part|!matches!(part,Component::Normal(name) if !name.to_string_lossy().starts_with('.'))) {
        return Err(Error::RuleViolation("draft path must be a visible relative path inside its registered directory".into()));
    }
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown"))
    {
        return Err(Error::InvalidInput(
            "draft requires a Markdown file extension".into(),
        ));
    }
    if title.trim().is_empty() || title.len() > 4096 || title.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "draft requires a bounded single-line title".into(),
        ));
    }
    let path = root.join(path);
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("draft has no parent".into()))?;
    open_dir_exact(parent)?;
    let spec = document_path_registration(root, &path)?;
    if spec.adapter != "markdown-directory-v1" {
        return Err(Error::MutationUnsupported(
            "new drafts require an explicitly registered Markdown decisions directory".into(),
        ));
    }
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
        Ok(_) => {
            return Err(Error::SourceConflict(
                "draft destination already exists; existing files are never overwritten".into(),
            ));
        }
    }
    let text = format!(
        "---\nid: {}\ntitle: {}\nstatus: proposed\n---\n# {}\n\n{}",
        serde_json::to_string(key)?,
        serde_json::to_string(title)?,
        title,
        body
    );
    let after = snapshot(&path, text)?;
    let source = Source {
        id: Id::new(),
        project_id: project,
        domain: spec.domain.clone(),
        role: spec.role.clone(),
        locator: after.locator.clone(),
        format: "markdown".into(),
        adapter: spec.adapter.clone(),
        revision: 0,
        fingerprint: String::new(),
        freshness: Freshness::Fresh,
        config: source_configuration(
            &spec,
            Manifest::load(root)?.project.context_profile == ContextProfile::Minimal,
        ),
    };
    declarations(&after, &spec)?;
    let batch = source_adapter(&spec.adapter)?.parse(
        &after,
        &ParseContext {
            source: &source,
            existing_ids: BTreeMap::new(),
        },
        &spec,
    )?;
    if batch.decisions.len() != 1
        || batch.decisions[0].status != DecisionStatus::Proposed
        || batch.decisions[0].meta.external_key != key
    {
        return Err(Error::RuleViolation(
            "new document must remain the requested draft".into(),
        ));
    }
    Ok(PreparedDocument {
        path,
        source,
        spec,
        before: None,
        after,
    })
}
