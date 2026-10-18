//! Single-source relocation with an explicit, recoverable identity binding.
use awr_core::{Error, Id, Result, Source};
use awr_source::{
    Locator, Manifest, ParseContext, SourceSpec, fingerprint, source_adapter, source_configuration,
    source_read_cap,
};
use awr_store::Store;
use clap::Args;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Args)]
pub struct RelocateArgs {
    #[arg(long)]
    pub source: Id,
    /// Existing destination file inside this project, with unchanged contents.
    #[arg(long)]
    pub to: PathBuf,
    #[arg(long)]
    pub accept: bool,
    #[arg(long, requires = "accept")]
    pub expected_preview: Option<String>,
}
const MARKER: &str = ".awr/mutations/source-relocation.pending";
fn relative(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(Error::RuleViolation(
            "relocation paths must be relative files inside this project".into(),
        ));
    }
    Ok(())
}
fn read(root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
    crate::intake_plan::current(root, path, awr_source::YAML_READ_CAP)
}
fn directory(root: &Path, key: &str) -> Result<PathBuf> {
    let hash = key.strip_prefix("sha256:").ok_or_else(|| {
        Error::InvalidInput("relocation requires its SHA256 preview fingerprint".into())
    })?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidInput("invalid relocation fingerprint".into()));
    }
    Ok(root
        .join(".awr/mutations")
        .join(format!("source-relocate-{hash}")))
}
fn plan(root: &Path, args: &RelocateArgs) -> Result<Value> {
    relative(&args.to)?;
    let manifest = Manifest::load(root)?;
    let mut store = Store::preview_snapshot(&root.join(".awr/state.db"), 256 * 1024 * 1024)?;
    let project = store.project_by_root(root)?;
    let source = store.source(project.id, args.source)?;
    if source.fingerprint.is_empty() {
        return Err(Error::SourceStale(
            "relocation requires an indexed source baseline".into(),
        ));
    }
    let index = manifest
        .sources
        .iter()
        .position(|s| source_configuration(s, false)["mapping_key"] == source.config["mapping_key"])
        .ok_or_else(|| {
            Error::SourceConflict("source is not bound to the current manifest".into())
        })?;
    let previous = &manifest.sources[index];
    let old_path = previous.path.as_ref().ok_or_else(|| {
        Error::Unsupported("relocation currently requires a file path mapping".into())
    })?;
    relative(old_path)?;
    if previous.adapter == "markdown-directory-v1" {
        return Err(Error::Unsupported(
            "relocate one explicitly mapped file, not a directory inventory".into(),
        ));
    }
    if Locator::File(root.join(old_path)).identity()? != source.locator {
        return Err(Error::SourceConflict(
            "source path and retained identity do not match".into(),
        ));
    }
    let old_bytes = crate::intake_plan::current(
        root,
        old_path
            .to_str()
            .ok_or_else(|| Error::InvalidInput("source path must be UTF-8".into()))?,
        source_read_cap(&source.adapter)?,
    )?;
    if old_bytes
        .as_ref()
        .is_some_and(|b| fingerprint(b) != source.fingerprint)
    {
        return Err(Error::SourceConflict(
            "original source changed; reindex it before relocating".into(),
        ));
    }
    let mut candidate = manifest.clone();
    candidate.sources[index].path = Some(args.to.clone());
    candidate.sources[index].locator = None;
    candidate.validate()?;
    let spec = &candidate.sources[index];
    let locator = Locator::from_spec(root, &candidate, spec)?;
    if !matches!(&locator, Locator::File(p) if p.starts_with(root)) {
        return Err(Error::RuleViolation(
            "relocation target escapes the project".into(),
        ));
    }
    let snapshot = locator.read(root, source_read_cap(&source.adapter)?)?;
    if snapshot.fingerprint != source.fingerprint {
        return Err(Error::SourceConflict(
            "relocation requires unchanged contents; edit and refresh separately".into(),
        ));
    }
    let old_ids = store.projection_ids(&source)?;
    let config = source_configuration(
        spec,
        manifest.project.context_profile == awr_source::ContextProfile::Minimal,
    );
    let guard = store.lock_sources()?;
    let moved = store.relocate_source(&source, &snapshot.locator, config.clone(), &guard)?;
    let adapter = source_adapter(&source.adapter)?;
    let batch = adapter.parse(
        &snapshot,
        &ParseContext {
            source: &moved,
            existing_ids: old_ids.clone(),
        },
        spec,
    )?;
    adapter.project(&mut store, &moved, &snapshot, batch)?;
    if store.projection_ids(&moved)? != old_ids {
        return Err(Error::Unsupported("adapter keys depend on the old locator; identity-preserving relocation is unavailable for this mapping".into()));
    }
    let after =
        toml::to_string_pretty(&candidate).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let effect = crate::intake_plan::effect(root, ".awr/project.toml", after)?;
    let mut plan = json!({"version":1,"operation":"source.relocate","project_root":root,"project_id":project.id,
        "source":source,"from":old_path,"to":args.to,"old_file_present":old_bytes.is_some(),
        "target_locator":snapshot.locator,"target_fingerprint":snapshot.fingerprint,"target_config":config,
        "spec":spec,"configuration":effect,"preserved_object_count":old_ids.len(),"can_apply":true,
        "source_write_performed":false,"runtime_write_performed":false,
        "effects":["replace manifest mapping", "retain source identity and append relocation event", "refresh projection references"],
        "content_policy":"destination already exists with identical bytes; original is neither moved nor removed", "filesystem_atomic":false});
    plan["fingerprint"] = json!(fingerprint(&serde_json::to_vec(&plan)?));
    Ok(plan)
}
fn load(root: &Path, key: &str) -> Result<Value> {
    let directory = directory(root, key)?;
    let bytes =
        awr_source::read_source_capped(&directory.join("receipt.json"), awr_source::YAML_READ_CAP)?;
    let v: Value = serde_json::from_slice(&bytes)?;
    if v["plan"]["project_root"] != json!(root) || v["plan"]["fingerprint"] != key {
        return Err(Error::SourceConflict(
            "relocation receipt belongs to another project or request".into(),
        ));
    }
    Ok(v)
}
fn observed(root: &Path, receipt: &Value) -> Result<Value> {
    let p = &receipt["plan"];
    let store = Store::preview_snapshot(&root.join(".awr/state.db"), 256 * 1024 * 1024)?;
    let project = store.project_by_root(root)?;
    let source: Source = serde_json::from_value(p["source"].clone())?;
    if project.id != source.project_id {
        return Err(Error::SourceConflict(
            "relocation project identity changed".into(),
        ));
    }
    let source = store.source(project.id, source.id)?;
    Ok(
        json!({"configuration_fingerprint":read(root,".awr/project.toml")?.map(|b|fingerprint(&b)),
        "source_locator":source.locator,"source_fingerprint":source.fingerprint,"source_revision":source.revision,
        "pending_marker":read(root,MARKER)?.map(|b|String::from_utf8_lossy(&b).into_owned()),"project_revision":project.project_revision}),
    )
}
pub fn status(root: &Path, key: &str) -> Result<()> {
    let root = root.canonicalize()?;
    let receipt = load(&root, key)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"receipt":receipt,"observed":observed(&root,&receipt)?,"read_only":true})
        )?
    );
    Ok(())
}
pub fn run(root: &Path, args: &RelocateArgs) -> Result<()> {
    let root = root.canonicalize()?;
    crate::source::runtime_dir(&root, false)?;
    if args.accept {
        let key = args
            .expected_preview
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("accept requires --expected-preview".into()))?;
        if directory(&root, key)?.join("receipt.json").exists() {
            let receipt = load(&root, key)?;
            if receipt["plan"]["source"]["id"] != json!(args.source)
                || receipt["plan"]["to"] != json!(args.to)
            {
                return Err(Error::SourceConflict(
                    "request arguments differ from the recorded relocation".into(),
                ));
            }
            if receipt["write_outcome"] == "completed" {
                return status(&root, key);
            }
            return Err(Error::SourceConflict(
                "relocation has a receipt; inspect relocate-status and use relocate-recover".into(),
            ));
        }
    }
    let preview = plan(&root, args)?;
    if !args.accept {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"status":"preview","preview":preview}))?
        );
        return Ok(());
    }
    let key = args.expected_preview.as_deref().unwrap();
    crate::intake_plan::check_expected(&preview, Some(key))?;
    let mut store = Store::open_existing(&root.join(".awr/state.db"))?;
    let guard = store.lock_sources()?;
    let current = plan(&root, args)?;
    crate::intake_plan::check_expected(&current, Some(key))?;
    if read(&root, MARKER)?.is_some() {
        return Err(Error::SourceConflict(
            "another relocation is pending".into(),
        ));
    }
    let directory = directory(&root, key)?;
    fs::create_dir_all(directory.parent().unwrap())?;
    awr_source::open_dir_exact(directory.parent().unwrap())?;
    fs::create_dir(&directory)?;
    let receipt =
        json!({"version":1,"plan":preview,"write_outcome":"pending","filesystem_atomic":false});
    crate::source::save_configuration_receipt(&directory, &receipt)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(MARKER))?;
    file.write_all(key.as_bytes())?;
    file.sync_all()?;
    drop(file);
    finish(&root, key, receipt, &mut store, &guard)
}
pub fn recover(root: &Path, key: &str) -> Result<()> {
    let root = root.canonicalize()?;
    let receipt = load(&root, key)?;
    if receipt["write_outcome"] == "completed" && read(&root, MARKER)?.is_none() {
        return status(&root, key);
    }
    let mut store = Store::open_existing(&root.join(".awr/state.db"))?;
    let guard = store.lock_sources()?;
    finish(&root, key, receipt, &mut store, &guard)
}
fn finish(
    root: &Path,
    key: &str,
    mut receipt: Value,
    store: &mut Store,
    guard: &awr_store::SourceLock,
) -> Result<()> {
    let directory = directory(root, key)?;
    let result = (|| -> Result<()> {
        let p = receipt["plan"].clone();
        let before: Source = serde_json::from_value(p["source"].clone())?;
        let project = store.project_by_root(root)?;
        if project.id != before.project_id {
            return Err(Error::SourceConflict("project identity changed".into()));
        }
        let marker = read(root, MARKER)?;
        if marker.as_deref().is_some_and(|m| m != key.as_bytes()) {
            return Err(Error::SourceConflict(
                "another relocation is pending".into(),
            ));
        }
        let text = p["configuration"]["after_text"]
            .as_str()
            .ok_or_else(|| Error::InvalidInput("missing relocation manifest".into()))?;
        let manifest = Manifest::parse(text)?;
        let spec: SourceSpec = serde_json::from_value(p["spec"].clone())?;
        let target = Locator::from_spec(root, &manifest, &spec)?
            .read(root, source_read_cap(&spec.adapter)?)?;
        if target.fingerprint != p["target_fingerprint"] || target.locator != p["target_locator"] {
            return Err(Error::SourceConflict(
                "destination changed; retain receipt and restore reviewed bytes before recovery"
                    .into(),
            ));
        }
        let from = p["from"].as_str().ok_or_else(|| {
            Error::InvalidInput("relocation receipt is missing the original path".into())
        })?;
        let old = read(root, from)?;
        if old
            .as_ref()
            .is_some_and(|b| fingerprint(b) != before.fingerprint)
        {
            return Err(Error::SourceConflict(
                "original source changed after relocation preview".into(),
            ));
        }
        let config = read(root, ".awr/project.toml")?
            .ok_or_else(|| Error::SourceConflict("source manifest missing".into()))?;
        let hash = fingerprint(&config);
        if hash != p["configuration"]["before_fingerprint"]
            && hash != p["configuration"]["after_fingerprint"]
        {
            return Err(Error::SourceConflict(
                "source manifest changed outside this relocation".into(),
            ));
        }
        if hash != p["configuration"]["after_fingerprint"]
            && awr_source::open_file_exact(&root.join(".awr/project.toml"))?
                .metadata()?
                .permissions()
                .readonly()
        {
            return Err(Error::RuleViolation(
                "source manifest is read-only; preserve the pending receipt".into(),
            ));
        }
        let source = store.source(project.id, before.id)?;
        if source.locator == before.locator {
            if source.revision != before.revision
                || source.fingerprint != before.fingerprint
                || source.config != before.config
            {
                return Err(Error::SourceConflict(
                    "original source binding changed".into(),
                ));
            }
        } else if source.locator != target.locator
            || source.config != p["target_config"]
            || source.fingerprint != target.fingerprint
        {
            return Err(Error::SourceConflict(
                "retained source no longer matches either relocation state".into(),
            ));
        }
        // Checks above are read-only. A receipt created before its marker is recoverable too.
        if marker.is_none() {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join(MARKER))?;
            f.write_all(key.as_bytes())?;
            f.sync_all()?;
        }
        if source.locator == before.locator {
            store.relocate_source(&before, &target.locator, p["target_config"].clone(), guard)?;
        }
        receipt["write_outcome"] = json!("binding_relocated");
        crate::source::save_configuration_receipt(&directory, &receipt)?;
        if hash != p["configuration"]["after_fingerprint"] {
            let file = awr_source::open_file_exact(&root.join(".awr/project.toml"))?;
            let permissions = file.metadata()?.permissions();
            if permissions.readonly() {
                return Err(Error::RuleViolation(
                    "source manifest is read-only; relocation remains recoverable".into(),
                ));
            }
            drop(file);
            let stage = directory.join(format!("manifest-{}.tmp", Id::new()));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&stage)?;
            file.write_all(text.as_bytes())?;
            file.set_permissions(permissions)?;
            file.sync_all()?;
            drop(file);
            if read(root, ".awr/project.toml")? != Some(config) {
                return Err(Error::SourceConflict(
                    "manifest changed before replacement".into(),
                ));
            }
            fs::rename(stage, root.join(".awr/project.toml"))?;
        }
        receipt["write_outcome"] = json!("configuration_relocated");
        crate::source::save_configuration_receipt(&directory, &receipt)?;
        let report =
            awr_source::index_project_locked(store, root, &manifest, false, guard, Some(key))?;
        let ok = report.ok;
        receipt["index"] = serde_json::to_value(report)?;
        if !ok {
            return Err(Error::SourceStale(
                "relocation indexed with issues; inspect its receipt before recovering".into(),
            ));
        }
        if Locator::from_spec(root, &manifest, &spec)?
            .read(root, source_read_cap(&spec.adapter)?)?
            .fingerprint
            != target.fingerprint
        {
            return Err(Error::SourceConflict(
                "destination changed during relocation; inspect and recover the retained receipt"
                    .into(),
            ));
        }
        receipt["write_outcome"] = json!("completed");
        receipt["ok"] = json!(true);
        receipt["error"] = Value::Null;
        crate::source::save_configuration_receipt(&directory, &receipt)?;
        if read(root, MARKER)?.as_deref() == Some(key.as_bytes()) {
            fs::remove_file(root.join(MARKER))?;
        }
        Ok(())
    })();
    if let Err(error) = &result {
        receipt["ok"] = json!(false);
        receipt["error"] = serde_json::to_value(error.report())?;
        crate::source::save_configuration_receipt(&directory, &receipt)?;
    }
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    result
}
