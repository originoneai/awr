//! Recoverable, explicitly mapped project metadata maintenance.
use crate::mutation_apply::{
    SourceReplacement, directory, named_lock, new_file, recovery_root, source_lock,
};
use awr_core::*;
use awr_source::*;
use awr_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationChange {
    pub version: u32,
    pub request_key: String,
    pub actor: HostActor,
    pub reason: String,
    pub source_id: Id,
    pub source_fingerprint: String,
    pub mapping: BTreeMap<String, String>,
    pub values: Map<String, Value>,
}
#[derive(Serialize, Deserialize)]
struct Plan {
    root: PathBuf,
    project_id: Id,
    project_revision: Revision,
    source: Source,
    configuration_fingerprint: String,
    request: OrganizationChange,
    path: PathBuf,
    before: String,
    after: String,
    preview: Value,
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    plan: Plan,
    phase: String,
    project_revision: Revision,
}
fn hash(v: &impl Serialize) -> Result<String> {
    Ok(fingerprint(&serde_json::to_vec(v)?))
}
fn name(key: &str) -> Result<String> {
    if key.is_empty() || key.len() > 512 || key.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "bounded organization request key required".into(),
        ));
    }
    Ok(format!(
        "organization-{}",
        hash(&key)?.trim_start_matches("sha256:")
    ))
}
fn manifest_hash(root: &Path) -> Result<String> {
    Ok(fingerprint(&read_capped(
        &root.join(".awr/project.toml"),
        YAML_READ_CAP,
    )?))
}
fn semantics(value: &mut Value) {
    match value {
        Value::Object(o) => {
            o.remove("source_ref");
            for v in o.values_mut() {
                semantics(v);
            }
        }
        Value::Array(a) => {
            for v in a {
                semantics(v);
            }
        }
        _ => (),
    }
}
fn validate_values(store: &Store, project: Id, fields: &Value) -> Result<()> {
    let text = |key: &str| -> Result<Option<&str>> {
        match &fields[key] {
            Value::Null => Ok(None),
            Value::String(s) if !s.trim().is_empty() && s.len() <= 4096 => Ok(Some(s)),
            _ => Err(Error::InvalidInput(format!(
                "{key} must be a bounded nonempty string or null"
            ))),
        }
    };
    let phase = text("phase")?;
    let focus = text("focus")?;
    text("next_action")?;
    if let Some(phase) = phase {
        let p = store.plan(project, phase)?;
        if p.source.freshness != Freshness::Fresh {
            return Err(Error::SourceStale("phase source is stale".into()));
        }
    }
    let scope = match &fields["scope"] {
        Value::Null => None,
        Value::Array(a) if a.len() <= 100 => {
            let mut keys = BTreeSet::new();
            for v in a {
                let key = v
                    .as_str()
                    .ok_or_else(|| Error::InvalidInput("scope requires work keys".into()))?;
                store.work_item(project, key)?;
                if !keys.insert(key) {
                    return Err(Error::InvalidInput("duplicate scope work key".into()));
                }
            }
            Some(keys)
        }
        _ => {
            return Err(Error::InvalidInput(
                "scope requires up to 100 exact work keys".into(),
            ));
        }
    };
    if let Some(key) = focus {
        let w = store.work_item(project, key)?;
        if w.item.archived
            || matches!(
                w.item.status,
                WorkStatus::Completed | WorkStatus::Cancelled | WorkStatus::Unknown
            )
        {
            return Err(Error::InvalidTransition("current focus requires nonterminal, recognized source work; use domain actions to reopen it".into()));
        }
        if scope.as_ref().is_some_and(|s| !s.contains(key)) {
            return Err(Error::SourceConflict(
                "focus is outside the selected scope".into(),
            ));
        }
        if phase.is_some_and(|p| w.item.milestone.as_deref() != Some(p)) {
            return Err(Error::SourceConflict(
                "focus and phase disagree with the source work milestone".into(),
            ));
        }
    }
    Ok(())
}
fn plan(store: &Store, root: &Path, request: OrganizationChange) -> Result<Plan> {
    ensure_public_data(&request)?;
    request.actor.validate()?;
    name(&request.request_key)?;
    if request.version != 1 || request.reason.trim().is_empty() || request.reason.len() > 4096 {
        return Err(Error::InvalidInput(
            "organization version 1 and bounded reason required".into(),
        ));
    }
    let project = store.project_by_root(root)?;
    for source in store.sources(project.id)? {
        if source.freshness != Freshness::Fresh {
            return Err(Error::SourceStale(
                "refresh all registered sources before organization changes".into(),
            ));
        }
        let (_, _, current) = inspect_registered_source(root, &source)?;
        if current.fingerprint != source.fingerprint {
            return Err(Error::SourceConflict(
                "registered source changed; refresh and preview again".into(),
            ));
        }
    }
    let source = store.source(project.id, request.source_id)?;
    if source.adapter != "yaml-ledger-v1" || source.role != "primary" {
        return Err(Error::MutationUnsupported(
            "organization metadata requires a primary YAML ledger".into(),
        ));
    }
    if source.fingerprint != request.source_fingerprint {
        return Err(Error::SourceConflict(
            "organization source fingerprint changed".into(),
        ));
    }
    let (locator, spec, before) = inspect_registered_source(root, &source)?;
    let Locator::File(path) = locator else {
        return Err(Error::MutationUnsupported(
            "organization changes require a local file source".into(),
        ));
    };
    if awr_source::is_runtime_owned_path(root, &path) {
        return Err(Error::MutationUnsupported(
            "runtime paths are not business metadata; relocate authored documents outside .awr (except registered .awr/intake sources) and reindex".into(),
        ));
    }
    let (after, before_fields, after_fields) =
        edit_organization_fields(before.text()?, &request.mapping, &request.values)?;
    validate_values(store, project.id, &after_fields)?;
    let after_snapshot = SourceSnapshot {
        content_review: None,
        locator: before.locator.clone(),
        fingerprint: fingerprint(after.as_bytes()),
        bytes: after.as_bytes().to_vec(),
    };
    let context = ParseContext {
        source: &source,
        existing_ids: store.projection_ids(&source)?,
    };
    let adapter = source_adapter(&source.adapter)?;
    let mut old = serde_json::to_value(adapter.parse(&before, &context, &spec)?)?;
    let mut new = serde_json::to_value(adapter.parse(&after_snapshot, &context, &spec)?)?;
    semantics(&mut old);
    semantics(&mut new);
    // Adapters allocate provisional edge IDs; the store retains relation identity.
    // Compare every edge endpoint, relation and required flag, excluding that allocation.
    for batch in [&mut old, &mut new] {
        for edge in batch["edges"].as_array_mut().unwrap() {
            edge.as_object_mut().unwrap().remove("id");
        }
    }
    if old != new {
        return Err(Error::MutationUnsupported("organization metadata cannot alter projected entities, lifecycle, acceptance or evidence".into()));
    }
    let configuration_fingerprint = manifest_hash(root)?;
    let mut preview = json!({"version":1,"operation":"organization.change","request_key":request.request_key,"project_id":project.id,"project_revision":project.project_revision,"source_id":source.id,"source_revision":source.revision,"source_fingerprint":source.fingerprint,"configuration_fingerprint":configuration_fingerprint,"mapping":request.mapping,"before":before_fields,"after":after_fields,"after_fingerprint":after_snapshot.fingerprint,"can_apply":true,"entity_changes":0,"acceptance_changes":0,"request_fingerprint":hash(&request)?});
    preview["fingerprint"] = json!(hash(&preview)?);
    Ok(Plan {
        root: root.to_owned(),
        project_id: project.id,
        project_revision: project.project_revision,
        source,
        configuration_fingerprint,
        request,
        path,
        before: before.text()?.into(),
        after,
        preview,
    })
}
fn save(root: &Path, r: &Receipt) -> Result<()> {
    let base = recovery_root(root)?;
    let key = name(&r.plan.request.request_key)?;
    let dir = directory(&base, &key, &root.join(".awr/mutations").join(&key))?;
    let temporary = format!("{}.tmp", Id::new());
    let mut file = new_file(&dir, OsStr::new(&temporary))?;
    file.write_all(&serde_json::to_vec(r)?)?;
    file.sync_all()?;
    drop(file);
    dir.rename(&temporary, &dir, "receipt.json")?;
    crate::fs_sync::sync_directory(&dir)
}
fn load(root: &Path, key: &str) -> Result<Option<Receipt>> {
    let path = root
        .join(".awr/mutations")
        .join(name(key)?)
        .join("receipt.json");
    if !path.try_exists()? {
        return Ok(None);
    }
    let r: Receipt = serde_json::from_slice(&read_capped(&path, YAML_READ_CAP * 3)?)?;
    if r.plan.root != root
        || r.plan.request.request_key != key
        || r.plan.preview["after_fingerprint"] != fingerprint(r.plan.after.as_bytes())
        || r.plan.source.fingerprint != fingerprint(r.plan.before.as_bytes())
        || r.plan.preview["request_fingerprint"] != hash(&r.plan.request)?
    {
        return Err(Error::SourceConflict(
            "organization receipt binding differs".into(),
        ));
    }
    let mut preview = r.plan.preview.clone();
    let digest = preview
        .as_object_mut()
        .unwrap()
        .remove("fingerprint")
        .unwrap_or(Value::Null);
    if digest != hash(&preview)? {
        return Err(Error::SourceConflict("organization preview changed".into()));
    }
    Ok(Some(r))
}
fn report(r: &Receipt) -> Value {
    json!({"ok":r.phase=="completed","phase":r.phase,"project_id":r.plan.project_id,"project_revision":r.project_revision,"preview":r.plan.preview,"receipt":format!(".awr/mutations/{}/receipt.json",name(&r.plan.request.request_key).unwrap()),"source_write_performed":r.phase=="completed","configuration_write_performed":false,"effects":"exact mapped source metadata; source history and projections refreshed"})
}
pub fn organization_status(store: &Store, root: &Path, key: &str) -> Result<Value> {
    let r = load(root, key)?.ok_or_else(|| Error::NotFound("organization request".into()))?;
    if store.project_by_root(root)?.id != r.plan.project_id {
        return Err(Error::SourceConflict(
            "organization project identity changed".into(),
        ));
    }
    let mut v = report(&r);
    v["source_write_performed"] = json!(false);
    v["read_only"] = json!(true);
    Ok(v)
}
pub fn organization_preview(
    store: &Store,
    root: &Path,
    request: OrganizationChange,
) -> Result<Value> {
    let p = plan(store, root, request)?;
    Ok(
        json!({"ok":true,"preview":p.preview,"read_only":true,"source_write_performed":false,"runtime_write_performed":false}),
    )
}
fn apply(store: &mut Store, root: &Path, mut r: Receipt, expected: Revision) -> Result<Value> {
    let _file_guard = source_lock(root, r.plan.source.id)?;
    let guard = store.lock_sources()?;
    let actual_revision = store.project(r.plan.project_id)?.project_revision;
    if actual_revision != expected {
        return Err(Error::RevisionConflict {
            expected,
            actual: actual_revision,
        });
    }
    if manifest_hash(root)? != r.plan.configuration_fingerprint {
        return Err(Error::SourceConflict(
            "organization configuration changed; preserve the receipt for review".into(),
        ));
    }
    let source = store.source(r.plan.project_id, r.plan.source.id)?;
    if source.config != r.plan.source.config || source.locator != r.plan.source.locator {
        return Err(Error::SourceConflict(
            "organization source binding changed".into(),
        ));
    }
    let (_, _, actual) = inspect_registered_source(root, &source)?;
    if actual.bytes == r.plan.before.as_bytes() {
        // Repeat all reference and protected-entity checks against the current state.
        let mut request = r.plan.request.clone();
        request.source_fingerprint = source.fingerprint.clone();
        let current = plan(store, root, request)?;
        if current.after != r.plan.after || current.path != r.plan.path {
            return Err(Error::SourceConflict(
                "organization plan no longer matches".into(),
            ));
        }
        let permissions = std::fs::metadata(&r.plan.path)?.permissions();
        let mut staged =
            SourceReplacement::prepare(&r.plan.path, r.plan.after.as_bytes(), permissions)?;
        if inspect_registered_source(root, &source)?.2.bytes != actual.bytes {
            return Err(Error::SourceConflict(
                "organization source changed before write".into(),
            ));
        }
        let actual_revision = store.project(r.plan.project_id)?.project_revision;
        if actual_revision != expected {
            return Err(Error::RevisionConflict {
                expected,
                actual: actual_revision,
            });
        }
        staged.install()?;
        staged.sync_parent()?;
    } else if actual.bytes != r.plan.after.as_bytes() {
        return Err(Error::SourceConflict(
            "source differs from both recorded states; preserve external changes".into(),
        ));
    }
    let indexed = index_project_locked(store, root, &Manifest::load(root)?, false, &guard, None)?;
    if !indexed.ok {
        return Err(Error::SourceStale("organization file may be saved; inspect receipt and recover after fixing source issues".into()));
    }
    r.phase = "completed".into();
    r.project_revision = indexed.project_revision;
    save(root, &r)?;
    Ok(report(&r))
}
pub fn change_organization(
    store: &mut Store,
    root: &Path,
    request: OrganizationChange,
    expected: Revision,
    preview: &str,
) -> Result<Value> {
    let key = name(&request.request_key)?;
    let _request_guard = named_lock(root, &format!("{key}.lock"))?;
    if let Some(r) = load(root, &request.request_key)? {
        if hash(&r.plan.request)? != hash(&request)? || r.plan.preview["fingerprint"] != preview {
            return Err(Error::SourceConflict(
                "request key belongs to a different organization plan".into(),
            ));
        }
        if r.phase == "completed" {
            let mut v = report(&r);
            v["replayed"] = json!(true);
            v["source_write_performed"] = json!(false);
            return Ok(v);
        }
        return Err(Error::MutationConflict(
            "organization request is pending; inspect and recover explicitly".into(),
        ));
    }
    let p = plan(store, root, request)?;
    if p.project_revision != expected {
        return Err(Error::RevisionConflict {
            expected,
            actual: p.project_revision,
        });
    }
    if p.preview["fingerprint"] != preview {
        return Err(Error::SourceConflict(
            "organization preview is stale".into(),
        ));
    }
    let r = Receipt {
        project_revision: p.project_revision,
        plan: p,
        phase: "prepared".into(),
    };
    save(root, &r)?;
    apply(store, root, r, expected)
}
pub fn recover_organization(
    store: &mut Store,
    root: &Path,
    key: &str,
    expected: Revision,
) -> Result<Value> {
    let _request_guard = named_lock(root, &format!("{}.lock", name(key)?))?;
    let r = load(root, key)?.ok_or_else(|| Error::NotFound("organization request".into()))?;
    let project = store.project_by_root(root)?;
    if project.id != r.plan.project_id {
        return Err(Error::SourceConflict("organization project changed".into()));
    }
    if project.project_revision != expected {
        return Err(Error::RevisionConflict {
            expected,
            actual: project.project_revision,
        });
    }
    if r.phase == "completed" {
        let mut v = report(&r);
        v["source_write_performed"] = json!(false);
        return Ok(v);
    }
    apply(store, root, r, expected)
}

/// Read explicitly mapped source annotations; hosts decide whether to use the declared focus.
pub fn read_organization(
    store: &Store,
    root: &Path,
    source_id: Id,
    mapping: &BTreeMap<String, String>,
) -> Result<Value> {
    let project = store.project_by_root(root)?;
    let source = store.source(project.id, source_id)?;
    if source.adapter != "yaml-ledger-v1" {
        return Err(Error::Unsupported(
            "organization mapping requires YAML".into(),
        ));
    }
    let (_, _, snapshot) = inspect_registered_source(root, &source)?;
    if source.freshness != Freshness::Fresh || source.fingerprint != snapshot.fingerprint {
        return Err(Error::SourceStale(
            "organization source changed; reindex before reading current annotations".into(),
        ));
    }
    let fields = organization_fields(snapshot.text()?, mapping)?;
    Ok(
        json!({"ok":true,"read_only":true,"project_id":project.id,"project_revision":project.project_revision,"source_id":source.id,"source_revision":source.revision,"source_fingerprint":source.fingerprint,"mapping":mapping,"fields":fields,"selection_basis":"source annotations; no automatic claim, activation or completion"}),
    )
}
