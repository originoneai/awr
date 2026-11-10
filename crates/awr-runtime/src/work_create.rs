//! Source-backed draft creation; a request owns one durable single-file outcome.
use crate::mutation_apply::{
    SourceReplacement, directory, named_lock, new_file, recovery_root, source_lock,
};
use awr_core::*;
use awr_source::{
    Manifest, SourceSnapshot, fingerprint, index_project, inspect_registered_source,
    open_file_exact,
};
use awr_store::Store;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkInput {
    pub version: u32,
    pub request_key: String,
    pub title: String,
    #[serde(default)]
    pub source_id: Option<Id>,
    /// Explicit source facts; omitted fields are never invented. Empty preserves v1 receipts.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub fields: serde_json::Map<String, Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: u32,
    project_id: Id,
    project_root: PathBuf,
    project_revision: Revision,
    request: CreateWorkInput,
    source: Source,
    external_key: String,
    before_fingerprint: String,
    after_fingerprint: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    observations: Vec<Source>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    manifest_fingerprint: String,
    #[serde(default, skip_serializing_if = "is_false")]
    activate: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    plan: Plan,
    preview_fingerprint: String,
    phase: String,
    work_id: Option<Id>,
    project_revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activation: Option<Value>,
}
pub struct CreationReport {
    pub value: Value,
    pub failure: Option<Error>,
}

fn validate_key(key: &str) -> Result<()> {
    if key.trim().is_empty() || key.len() > 512 || key.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "request_key requires 1..512 bytes without control characters".into(),
        ));
    }
    ensure_public_text(key)
}
fn identity(project: Id, key: &str) -> Result<String> {
    validate_key(key)?;
    Ok(fingerprint(&serde_json::to_vec(&(project, key))?)
        .trim_start_matches("sha256:")
        .into())
}
fn receipt_name(project: Id, key: &str) -> Result<String> {
    Ok(format!("work-create-{}", identity(project, key)?))
}
fn read(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let file = open_file_exact(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > cap {
        return Err(Error::InvalidInput(
            "invalid or oversized creation recovery file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Err(Error::InvalidInput(
            "creation recovery file grew beyond its bound".into(),
        ));
    }
    Ok(bytes)
}
fn load(root: &Path, project: Id, key: &str) -> Result<Option<Receipt>> {
    let base = root
        .join(".awr/mutations")
        .join(receipt_name(project, key)?);
    match std::fs::symlink_metadata(&base) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(Error::RuleViolation(
                "creation recovery directory must be real".into(),
            ));
        }
        _ => (),
    }
    let value: Receipt = serde_json::from_slice(&read(&base.join("receipt.json"), 128 * 1024)?)?;
    if value.plan.version != 1
        || value.plan.request.version != 1
        || !["prepared", "applied", "completed"].contains(&value.phase.as_str())
    {
        return Err(Error::InvalidInput(
            "unsupported creation receipt version or phase".into(),
        ));
    }
    if value.plan.project_id != project
        || value.plan.project_root != root
        || value.plan.request.request_key != key
        || (plan_fingerprint(&value.plan)? != value.preview_fingerprint
            && fingerprint(&serde_json::to_vec(&value.plan)?) != value.preview_fingerprint)
    {
        return Err(Error::SourceConflict(
            "creation receipt differs from its bound project or plan".into(),
        ));
    }
    Ok(Some(value))
}
fn save(dir: &Dir, receipt: &Receipt) -> Result<()> {
    let temp = format!("receipt-{}.tmp", Id::new());
    let mut file = new_file(dir, OsStr::new(&temp))?;
    file.write_all(&serde_json::to_vec_pretty(receipt)?)?;
    file.sync_all()?;
    drop(file);
    dir.rename(&temp, dir, "receipt.json")?;
    crate::fs_sync::sync_directory(dir)
}
fn summary(receipt: &Receipt, replay: bool) -> Result<Value> {
    Ok(
        json!({"ok":receipt.phase=="completed","status":if receipt.phase=="completed"{"completed"}else{"pending_recovery"},
        "request_key":receipt.plan.request.request_key,"external_key":receipt.plan.external_key,"work_id":receipt.work_id,
        "project_id":receipt.plan.project_id,"project_revision":receipt.project_revision,"source_id":receipt.plan.source.id,
        "preview_fingerprint":receipt.preview_fingerprint,"phase":receipt.phase,"already_recorded":replay,
        "source_write_performed":false,"runtime_write_performed":false,"historical_outcome":true,
        "write_outcome":if receipt.phase=="completed"{"applied"}else{"pending_recovery"},
        "draft":!receipt.activation.as_ref().is_some_and(|v| v["ok"] == true),"completion_claimed":false,
        "activation":receipt.activation,
        "recovery_directory":format!(".awr/mutations/{}",receipt_name(receipt.plan.project_id,&receipt.plan.request.request_key)?)}),
    )
}
pub fn creation_status(store: &Store, root: &Path, key: &str) -> Result<CreationReport> {
    let root = root.canonicalize()?;
    let project = store.project_by_root(&root)?;
    let receipt = load(&root, project.id, key)?;
    let value = if let Some(r) = receipt {
        let mut value = summary(&r, true)?;
        value["found"] = json!(true);
        value["read_only"] = json!(true);
        value["current_source"] = json!(match inspect_registered_source(&root, &r.plan.source) {
            Ok((_, _, s)) if s.fingerprint == r.plan.after_fingerprint => "after",
            Ok((_, _, s)) if s.fingerprint == r.plan.before_fingerprint => "before",
            Ok(_) => "externally_changed",
            Err(_) => "unavailable_or_registration_changed",
        });
        value
    } else {
        json!({"ok":true,"found":false,"read_only":true,"source_write_performed":false,"runtime_write_performed":false})
    };
    Ok(CreationReport {
        value,
        failure: None,
    })
}

pub fn create_work(
    store: &mut Store,
    root: &Path,
    input: CreateWorkInput,
    accept: bool,
    expected_preview: Option<&str>,
    expected_revision: Option<Revision>,
) -> Result<CreationReport> {
    create_work_with_activation(
        store,
        root,
        input,
        accept,
        expected_preview,
        expected_revision,
        false,
    )
}

/// The CLI can request activation without changing the existing MCP creation input shape.
pub fn create_work_with_activation(
    store: &mut Store,
    root: &Path,
    input: CreateWorkInput,
    accept: bool,
    expected_preview: Option<&str>,
    expected_revision: Option<Revision>,
    activate: bool,
) -> Result<CreationReport> {
    let root = root.canonicalize()?;
    let project = store.project_by_root(&root)?;
    validate_key(&input.request_key)?;
    ensure_public_data(&input)?;
    if input.version != 1
        || input.title.trim().is_empty()
        || input.title.len() > 4096
        || input.title.chars().any(char::is_control)
    {
        return Err(Error::InvalidInput(
            "creation requires version 1 and a 1..4096 byte title without control characters"
                .into(),
        ));
    }
    let key = identity(project.id, &input.request_key)?;
    let _request_lock = if accept {
        Some(named_lock(&root, &format!("work-create-{key}.lock"))?)
    } else {
        None
    };
    if let Some(mut receipt) = load(&root, project.id, &input.request_key)? {
        if receipt.plan.request != input || receipt.plan.activate != activate {
            return Err(Error::SourceConflict(
                "request_key already belongs to different creation content".into(),
            ));
        }
        let mut report = CreationReport {
            value: summary(&receipt, true)?,
            failure: (receipt.phase!="completed").then(||Error::MutationConflict("creation is pending; inspect its receipt and explicitly recover instead of replaying".into())),
        };
        if accept
            && receipt.plan.activate
            && receipt.activation.is_none()
            && receipt.phase == "completed"
        {
            let dir = awr_source::open_dir_exact(
                &root
                    .join(".awr/mutations")
                    .join(receipt_name(project.id, &input.request_key)?),
            )?;
            activate_created(store, &root, &dir, &mut receipt, &mut report)?;
        }
        return Ok(report);
    }

    let manifest = Manifest::load(&root)?;
    let report = index_project(store, &root, &manifest, false)?;
    if !report.ok {
        return Err(Error::SourceStale(
            "refresh every registered source before creating work".into(),
        )
        .with_details(json!({"preview_still_valid":false})));
    }
    let revision = store.project(project.id)?.project_revision;
    let sources = store.sources(project.id)?;
    let observations = sources.clone();
    let candidates: Vec<_> = sources
        .into_iter()
        .filter(|s| {
            s.domain == "ledger" && input.source_id.map_or(s.role == "primary", |id| s.id == id)
        })
        .collect();
    if candidates.len() != 1 {
        return Err(Error::SourceConflict(
            "choose exactly one registered ledger source for creation".into(),
        ));
    }
    let source = candidates.into_iter().next().unwrap();
    let _source_lock = if accept {
        Some(source_lock(&root, source.id)?)
    } else {
        None
    };
    let external_key = format!("WORK-{}", &key[..32]);
    if store
        .work_items(project.id)?
        .iter()
        .any(|w| w.item.meta.external_key == external_key)
    {
        return Err(Error::SourceConflict(
            "generated work key already exists; inspect its source instead of overwriting".into(),
        ));
    }
    let prepared = awr_source::prepare_work_creation_with_fields(
        &root,
        &source,
        &external_key,
        &input.title,
        &input.fields,
    )?;
    crate::work_graph::validate_graph_snapshot(store, &root, &source, &prepared.after)?;
    let plan = Plan {
        version: 1,
        project_id: project.id,
        project_root: root.clone(),
        project_revision: revision,
        request: input,
        source,
        external_key,
        before_fingerprint: prepared.before.fingerprint.clone(),
        after_fingerprint: prepared.after.fingerprint.clone(),
        observations,
        manifest_fingerprint: fingerprint(&awr_source::read_capped(
            &root.join(".awr/project.toml"),
            1024 * 1024,
        )?),
        activate,
    };
    let preview = plan_fingerprint(&plan)?;
    if !accept {
        return Ok(CreationReport {
            value: json!({"ok":true,"status":"preview","project_revision":revision,"requires_accept":true,
            "preview":{"fingerprint":preview,"plan":plan,"before_text":prepared.before.text()?,"after_text":prepared.after.text()?,"new_record":prepared.record},
            "source_write_performed":false,"runtime_write_performed":true,"draft":true,"activation_requested":activate,"completion_claimed":false}),
            failure: None,
        });
    }
    if expected_revision.is_none() || expected_preview.is_none() {
        return Err(Error::SourceConflict(
            "creation accept requires the exact reviewed preview and project revision".into(),
        ));
    }
    if expected_preview != Some(preview.as_str()) {
        let changed = !store
            .query_source_events(
                project.id,
                &awr_store::EventQuery {
                    after_revision: expected_revision.unwrap(),
                    limit: 1,
                    ..Default::default()
                },
            )?
            .events
            .is_empty();
        let error = if changed {
            Error::SourceStale("creation sources changed; review a fresh preview".into())
        } else {
            Error::SourceConflict("creation accept requires the exact reviewed preview".into())
        };
        return Err(
            error.with_details(json!({"preview_still_valid":false,"current_revision":revision}))
        );
    }
    if let Some(expected) = expected_revision
        && expected != revision
    {
        return Err(Error::RevisionConflict {
            expected,
            actual: revision,
        }
        .with_details(
            json!({"current_revision":revision,"preview_still_valid":true,
            "retry":{"expected_revision":revision}}),
        ));
    }
    let mutations = recovery_root(&root)?;
    let name = receipt_name(project.id, &plan.request.request_key)?;
    mutations.create_dir(&name)?;
    let dir = directory(&mutations, &name, &root.join(".awr/mutations").join(&name))?;
    for (name, bytes) in [
        ("before.yaml", &prepared.before.bytes),
        ("after.yaml", &prepared.after.bytes),
    ] {
        let mut file = new_file(&dir, OsStr::new(name))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    let mut receipt = Receipt {
        plan,
        preview_fingerprint: preview,
        phase: "prepared".into(),
        work_id: None,
        project_revision: revision,
        activation: None,
    };
    save(&dir, &receipt)?;
    crate::fs_sync::sync_directory(&mutations)?;
    let mut report = finish(store, &root, &dir, &mut receipt, expected_revision.unwrap())?;
    drop(_source_lock);
    activate_created(store, &root, &dir, &mut receipt, &mut report)?;
    Ok(report)
}

pub fn recover_creation(
    store: &mut Store,
    root: &Path,
    key: &str,
    expected_revision: Revision,
) -> Result<CreationReport> {
    let root = root.canonicalize()?;
    let project = store.project_by_root(&root)?;
    let _request_lock = named_lock(
        &root,
        &format!("work-create-{}.lock", identity(project.id, key)?),
    )?;
    let mut receipt =
        load(&root, project.id, key)?.ok_or_else(|| Error::NotFound("creation receipt".into()))?;
    let dir = awr_source::open_dir_exact(
        &root
            .join(".awr/mutations")
            .join(receipt_name(project.id, key)?),
    )?;
    let mut report = if receipt.phase == "completed" {
        CreationReport {
            value: summary(&receipt, true)?,
            failure: None,
        }
    } else {
        let _source_lock = source_lock(&root, receipt.plan.source.id)?;
        finish(store, &root, &dir, &mut receipt, expected_revision)?
    };
    activate_created(store, &root, &dir, &mut receipt, &mut report)?;
    Ok(report)
}
fn finish(
    store: &mut Store,
    root: &Path,
    dir: &Dir,
    receipt: &mut Receipt,
    expected: Revision,
) -> Result<CreationReport> {
    let mut wrote = false;
    let mut index_report = None;
    let result = (|| -> Result<()> {
        let actual = store.project(receipt.plan.project_id)?.project_revision;
        if actual != expected {
            return Err(Error::RevisionConflict { expected, actual });
        }
        let plan = &receipt.plan;
        let base = root
            .join(".awr/mutations")
            .join(receipt_name(plan.project_id, &plan.request.request_key)?);
        let before = read(&base.join("before.yaml"), awr_source::YAML_READ_CAP)?;
        let after = read(&base.join("after.yaml"), awr_source::YAML_READ_CAP)?;
        if fingerprint(&before) != plan.before_fingerprint
            || fingerprint(&after) != plan.after_fingerprint
        {
            return Err(Error::SourceConflict(
                "creation snapshots differ from the reviewed plan".into(),
            ));
        }
        let (locator, spec, current) = inspect_registered_source(root, &plan.source)?;
        let awr_source::Locator::File(path) = locator else {
            return Err(Error::MutationUnsupported(
                "creation cannot write Git sources".into(),
            ));
        };
        if current.fingerprint != plan.before_fingerprint
            && current.fingerprint != plan.after_fingerprint
        {
            return Err(Error::SourceConflict(
                "creation recovery retains externally changed source bytes".into(),
            ));
        }
        // Recovery also reparses its immutable after snapshot through the registered adapter.
        awr_source::source_adapter(&plan.source.adapter)?.parse(
            &SourceSnapshot {
                content_review: None,
                locator: current.locator.clone(),
                bytes: after.clone(),
                fingerprint: plan.after_fingerprint.clone(),
            },
            &awr_source::ParseContext {
                source: &plan.source,
                existing_ids: store.projection_ids(&plan.source)?,
            },
            &spec,
        )?;
        if current.fingerprint == plan.before_fingerprint {
            if !plan.observations.is_empty() {
                if fingerprint(&awr_source::read_capped(
                    &root.join(".awr/project.toml"),
                    1024 * 1024,
                )?) != plan.manifest_fingerprint
                {
                    return Err(Error::SourceConflict(
                        "creation mapping changed before source replacement".into(),
                    ));
                }
                for source in &plan.observations {
                    if source.id != plan.source.id
                        && inspect_registered_source(root, source)?.2.fingerprint
                            != source.fingerprint
                    {
                        return Err(Error::SourceConflict(
                            "creation context source changed before source replacement".into(),
                        ));
                    }
                }
            }
            let permissions = open_file_exact(&path)?.metadata()?.permissions();
            if permissions.readonly() {
                return Err(Error::RuleViolation("ledger is read-only".into()));
            }
            let mut replacement = SourceReplacement::prepare(&path, &after, permissions)?;
            let (_, _, last) = inspect_registered_source(root, &plan.source)?;
            if last.fingerprint != plan.before_fingerprint {
                return Err(Error::SourceConflict(
                    "ledger changed immediately before creation write".into(),
                ));
            }
            let actual = store.project(plan.project_id)?.project_revision;
            if actual != expected {
                return Err(Error::RevisionConflict { expected, actual });
            }
            replacement.install()?;
            wrote = true;
        }
        receipt.phase = "applied".into();
        save(dir, receipt)?;
        let indexed = index_project(store, root, &Manifest::load(root)?, false)?;
        index_report = Some(serde_json::to_value(&indexed)?);
        receipt.project_revision = indexed.project_revision;
        if !indexed.ok {
            return Err(Error::SourceStale(
                "creation source applied but project projection is incomplete".into(),
            ));
        }
        let work = store
            .work_items(plan.project_id)?
            .into_iter()
            .find(|w| w.item.meta.external_key == plan.external_key)
            .ok_or_else(|| {
                Error::SourceConflict("created work is absent from its projection".into())
            })?;
        if work.source.id != plan.source.id
            || work.source.fingerprint != plan.after_fingerprint
            || work.item.title != plan.request.title
        {
            return Err(Error::SourceConflict(
                "created work differs from the reviewed source identity".into(),
            ));
        }
        let (_, _, last) = inspect_registered_source(root, &plan.source)?;
        if last.fingerprint != plan.after_fingerprint {
            return Err(Error::SourceConflict(
                "ledger changed before creation finalization".into(),
            ));
        }
        receipt.work_id = Some(work.item.meta.id);
        receipt.phase = "completed".into();
        save(dir, receipt)
    })();
    let mut value = summary(receipt, false)?;
    value["source_write_performed"] = json!(wrote);
    value["runtime_write_performed"] = json!(true);
    value["historical_outcome"] = json!(false);
    if let Some(report) = index_report {
        value["index"] = report;
    }
    if let Err(error) = result {
        value["ok"] = json!(false);
        value["status"] = json!("pending_recovery");
        value["write_outcome"] = json!("pending_recovery");
        value["error"] = serde_json::to_value(error.report())?;
        return Ok(CreationReport {
            value,
            failure: Some(error),
        });
    }
    Ok(CreationReport {
        value,
        failure: None,
    })
}

fn is_false(value: &bool) -> bool {
    !value
}

fn plan_fingerprint(plan: &Plan) -> Result<String> {
    let mut value = serde_json::to_value(plan)?;
    // Runtime events must not invalidate a review of unchanged source facts.
    value.as_object_mut().unwrap().remove("project_revision");
    Ok(fingerprint(&serde_json::to_vec(&value)?))
}

fn activate_created(
    store: &mut Store,
    root: &Path,
    dir: &Dir,
    receipt: &mut Receipt,
    report: &mut CreationReport,
) -> Result<()> {
    if !receipt.plan.activate || receipt.phase != "completed" || receipt.activation.is_some() {
        return Ok(());
    }
    let activation_key = format!(
        "work-create-activate-{}",
        identity(receipt.plan.project_id, &receipt.plan.request.request_key)?
    );
    let result = (|| -> Result<Value> {
        let prior = crate::host_status(store, root, &activation_key)?;
        if prior.value["found"] == true {
            return Ok(prior.value);
        }
        if fingerprint(&awr_source::read_capped(
            &root.join(".awr/project.toml"),
            1024 * 1024,
        )?) != receipt.plan.manifest_fingerprint
        {
            return Err(Error::SourceConflict(
                "creation mapping changed before activation".into(),
            ));
        }
        for source in &receipt.plan.observations {
            if source.id != receipt.plan.source.id
                && inspect_registered_source(root, source)?.2.fingerprint != source.fingerprint
            {
                return Err(Error::SourceConflict(
                    "creation context source changed before activation".into(),
                ));
            }
        }
        let request = crate::HostSaveRequest {
            version: 1,
            request_key: activation_key.clone(),
            actor: HostActor {
                host: "awr-cli".into(),
                subject: "work-create".into(),
                origin: HostEditOrigin::DelegatedAgent,
            },
            reason: "Activate the newly created draft at the caller's explicit request".into(),
            change: crate::HostChange::ActivateDraft {
                work: receipt.plan.external_key.clone(),
                source_fingerprint: receipt.plan.after_fingerprint.clone(),
            },
        };
        let preview = crate::host_preview(store, root, request.clone())?;
        let mut applied = crate::host_save(
            store,
            root,
            request,
            preview.value["project_revision"]
                .as_u64()
                .expect("host revision"),
            preview.value["preview"]["fingerprint"].as_str(),
        )?;
        if let Some(error) = applied.failure {
            applied.value["error"] = json!(error.report());
        }
        Ok(applied.value)
    })();
    let mut activation = match result {
        Ok(value) => value,
        Err(error) => json!({"ok":false,"status":"rejected","error":error.report()}),
    };
    if activation["ok"] != true {
        activation["next_action"] = activation["error"]["details"]["next_action"]
            .as_str()
            .map(|next| json!(next))
            .unwrap_or_else(|| {
                json!(cli_next_action(&[
                    "awr",
                    "work",
                    "show",
                    &receipt.plan.external_key
                ]))
            });
    }
    if activation["source_write_performed"] == true {
        report.value["source_write_performed"] = json!(true);
    }
    receipt.activation = Some(activation);
    receipt.project_revision = store.project(receipt.plan.project_id)?.project_revision;
    save(dir, receipt)?;
    report.value["activation"] = json!(receipt.activation);
    report.value["draft"] = json!(!receipt.activation.as_ref().is_some_and(|v| v["ok"] == true));
    report.value["project_revision"] = json!(receipt.project_revision);
    report.value["runtime_write_performed"] = json!(true);
    report.value["historical_outcome"] = json!(false);
    Ok(())
}
