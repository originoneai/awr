use awr_core::{AuthorityMode, Error, Freshness, Result};
use awr_source::{
    FileInventory, IndexReport, Manifest, ProjectConfig, SourceSpec, compare_file_inventories,
    index_project, inventory_files, scan_project,
};
use awr_store::Store;
use clap::Subcommand;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Subcommand)]
pub enum SourceCommand {
    /// Read-only project file inventory for host freshness guards; no AWR database is opened.
    Inventory {
        #[arg(long = "include", required = true)]
        includes: Vec<PathBuf>,
        #[arg(long = "exclude-glob")]
        exclude_globs: Vec<String>,
        #[arg(long)]
        baseline: Option<PathBuf>,
    },
    /// Preview or accept a single file-source binding relocation; content must be unchanged.
    Relocate(crate::source_relocation::RelocateArgs),
    /// Inspect a relocation receipt and current source/configuration bindings without refreshing.
    RelocateStatus { fingerprint: String },
    /// Resume a reviewed interrupted relocation after validating its saved before/after state.
    RelocateRecover { fingerprint: String },
    /// Read source lifecycle/change receipts in an immutable event window without refreshing files.
    Changes(crate::source_changes::ChangesArgs),
    /// Preview or apply an exact replacement source mapping while preserving project identity.
    Configure {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        accept: bool,
        #[arg(long, requires = "accept")]
        expected_preview: Option<String>,
    },
    /// Inspect a configuration receipt without applying or reindexing anything.
    ConfigureStatus { preview_fingerprint: String },
    /// Read one registered source by ID, unambiguous domain or exact locator.
    Show(crate::drill::SourceRead),
    /// Read source lifecycle/change summaries, including retired source IDs.
    History {
        reference: String,
        #[command(flatten)]
        window: crate::drill::HistoryWindow,
    },
    /// Read registered sources and their last observed freshness without changing the database.
    List,
    /// Refresh source availability and report pending changes without parsing new facts.
    Scan,
    /// Reconcile source content and parsing configuration with the projection database.
    Reindex {
        #[arg(long)]
        force: bool,
    },
}

pub(crate) fn runtime_dir(root: &Path, create: bool) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    if create && root.metadata()?.permissions().readonly() {
        return Err(Error::RuleViolation(
            "project directory is read-only".into(),
        ));
    }
    let path = root.join(".awr");
    if create && !path.exists() {
        fs::create_dir(&path)?;
    }
    let actual = path
        .canonicalize()
        .map_err(|e| Error::SourceUnavailable(format!("{}: {e}", path.display())))?;
    if !actual.starts_with(&root) || !actual.is_dir() {
        return Err(Error::RuleViolation(
            "runtime directory escapes project root".into(),
        ));
    }
    Ok(actual)
}

pub fn initialize(
    root: &Path,
    manifest_path: Option<&Path>,
    accept: bool,
    json_output: bool,
    expected_preview: Option<&str>,
) -> Result<()> {
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(Error::InvalidInput(
            "project root must be a directory".into(),
        ));
    }
    let existing_path = root.join(".awr/project.toml");
    let existing = if existing_path.exists() {
        Some(Manifest::load(&root)?)
    } else {
        None
    };
    let (candidate, candidates, ambiguous) = if let Some(path) = manifest_path {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        let bytes = awr_source::read_capped(&path, 64 * 1024)?;
        let candidate = Manifest::parse(
            std::str::from_utf8(&bytes).map_err(|e| Error::InvalidInput(e.to_string()))?,
        )?;
        (Some(candidate), vec![], vec![])
    } else if let Some(existing) = &existing {
        (Some(existing.clone()), vec![], vec![])
    } else {
        discover(&root)?
    };
    if let (Some(existing), Some(candidate)) = (&existing, &candidate) {
        if serde_json::to_value(existing)? != serde_json::to_value(candidate)? {
            return Err(Error::SourceConflict("project.toml already exists with another mapping; initialization never overwrites it".into()));
        }
    }
    if !accept {
        let toml = candidate
            .as_ref()
            .map(toml::to_string_pretty)
            .transpose()
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let effects = candidate
            .as_ref()
            .map(|m| crate::intake_plan::preview(&root, m, &BTreeMap::new(), false))
            .transpose()?;
        let preview = json!({"status":"preview","configuration_exists":existing.is_some(),"requires_accept":true,
            "preview":effects,"candidates":candidates,"ambiguous_domains":ambiguous,"authority_mapping":candidate,"manifest_toml":toml});
        if json_output {
            println!("{}", serde_json::to_string_pretty(&preview)?);
        } else {
            println!("Source authority mapping preview (no files changed).");
            if let Some(toml) = toml {
                println!("\n{toml}\nUse init --accept with this mapping to initialize.");
            } else {
                println!("{}", serde_json::to_string_pretty(&preview)?);
                println!("Supply an explicit --manifest, then use --accept.");
            }
        }
        return Ok(());
    }
    let manifest = candidate.ok_or_else(|| {
        Error::InvalidInput(if ambiguous.is_empty() {
            "no source candidates; supply an explicit --manifest".into()
        } else {
            format!(
                "multiple authority candidates for {}; supply an explicit --manifest",
                ambiguous.join(", ")
            )
        })
    })?;
    manifest.validate()?;
    let plan = crate::intake_plan::preview(&root, &manifest, &BTreeMap::new(), false)?;
    crate::intake_plan::check_expected(&plan, expected_preview)?;
    crate::intake_plan::require_applicable(&plan)?;
    if existing.is_none() && root.join(".awr/state.db").exists() {
        return Err(Error::SourceConflict("database exists without project.toml; restore its matching manifest before initializing".into()));
    }
    // Check the ignore target before creating configuration; append only missing runtime entries.
    let ignore = root.join(".gitignore");
    if fs::symlink_metadata(&ignore).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Error::RuleViolation(
            "initialization cannot append to a symlinked .gitignore".into(),
        ));
    }
    let previous_ignore = if ignore.exists() {
        fs::read_to_string(&ignore)?
    } else {
        String::new()
    };
    let needed = crate::intake_plan::IGNORE_ENTRIES;
    let missing: Vec<_> = needed
        .iter()
        .filter(|entry| !previous_ignore.lines().any(|line| line.trim() == **entry))
        .collect();
    let runtime = runtime_dir(&root, true)?;
    if existing.is_none() {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(runtime.join("project.toml"))?;
        file.write_all(
            toml::to_string_pretty(&manifest)
                .map_err(|e| Error::InvalidInput(e.to_string()))?
                .as_bytes(),
        )?;
        file.sync_all()?;
    }
    if !missing.is_empty() {
        let mut file = OpenOptions::new().append(true).create(true).open(ignore)?;
        if !previous_ignore.is_empty() && !previous_ignore.ends_with('\n') {
            writeln!(file)?;
        }
        writeln!(file, "\n# AWR local runtime state")?;
        for entry in missing {
            writeln!(file, "{entry}")?;
        }
        file.sync_all()?;
    }
    let mut store = Store::open(&runtime.join("state.db"))?;
    let report = index_project(&mut store, &root, &manifest, false)?;
    let project = store.project(report.project_id)?;
    let works = store.work_items(project.id)?;
    let ready = store.ready_work(
        project.id,
        project.current_branch_id,
        awr_core::now_millis()?,
    )?;
    let organization = awr_runtime::inspect_organization(
        &store,
        &project,
        project.current_branch_id,
        None,
        report.ok,
        &works,
        &ready,
    )?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"initialized":true,"configuration_created":existing.is_none(),
        "manifest":runtime.join("project.toml"),"authority_mapping":manifest.sources,"index":report,"organization":organization,"next_action":organization.next_action})
            )?
        );
    } else {
        println!(
            "Initialized {} using {}",
            manifest.project.name,
            runtime.join("project.toml").display()
        );
        print_report(&report);
        println!("{}", organization.rendered());
    }
    if !report.ok {
        return Err(Error::SourceStale(
            "project initialized with source issues; resolve them and run source reindex".into(),
        ));
    }
    Ok(())
}

fn configure(
    root: &Path,
    path: &Path,
    accept: bool,
    expected: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let root = root.canonicalize()?;
    let existing = Manifest::load(&root)?;
    let input = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let bytes = awr_source::read_capped(&input, 64 * 1024)?;
    let candidate = Manifest::parse(
        std::str::from_utf8(&bytes)
            .map_err(|_| Error::InvalidInput("manifest must be UTF-8".into()))?,
    )?;
    if candidate.project.name != existing.project.name
        || candidate.project.external_key != existing.project.external_key
        || candidate.project.authority_mode != existing.project.authority_mode
    {
        return Err(Error::SourceConflict(
            "source configure preserves project name, external_key and authority mode".into(),
        ));
    }
    let plan = crate::intake_plan::preview(&root, &candidate, &BTreeMap::new(), true)?;
    if !accept {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"status":"preview", "preview":plan, "requires_accept":true})
            )?
        );
        return Ok(());
    }
    if expected.is_none() {
        return Err(Error::InvalidInput(
            "source configure --accept requires --expected-preview from the reviewed preview"
                .into(),
        ));
    }
    crate::intake_plan::check_expected(&plan, expected)?;
    crate::intake_plan::require_applicable(&plan)?;
    if root.metadata()?.permissions().readonly() {
        return Err(Error::RuleViolation(
            "project directory is read-only".into(),
        ));
    }
    let runtime = runtime_dir(&root, false)?;
    // Do not modify a configuration for a foreign, future or damaged runtime.
    let store = Store::open_readonly(&runtime.join("state.db"))?;
    let project = store.project_by_root(&root)?;
    drop(store);
    let after = plan["writes"][0]["after_text"]
        .as_str()
        .ok_or_else(|| Error::InvalidInput("configuration preview lacks its target text".into()))?
        .to_owned();
    let effect = crate::intake_plan::effect(&root, ".awr/project.toml", after)?;
    if effect.action == "no_change" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"ok":true,"write_outcome":"no_change","configuration_write_performed":false,"runtime_write_performed":false,"project_id":project.id,"project_revision":project.project_revision})
            )?
        );
        return Ok(());
    }
    let _lock = crate::client::lock(&root, "mutations", "source-configuration")?;
    let mut store = Store::open_existing(&runtime.join("state.db"))?;
    let guard = store.lock_sources()?;
    if root
        .join(".awr/mutations/source-relocation.pending")
        .exists()
    {
        return Err(Error::SourceConflict(
            "source relocation pending; recover it before configuring sources".into(),
        ));
    }
    let current = crate::intake_plan::preview(&root, &candidate, &BTreeMap::new(), true)?;
    crate::intake_plan::check_expected(&current, expected)?;
    crate::intake_plan::require_applicable(&current)?;
    let permissions = awr_source::open_file_exact(&root.join(".awr/project.toml"))?
        .metadata()?
        .permissions();
    if permissions.readonly() {
        return Err(Error::RuleViolation(
            "source configuration is read-only".into(),
        ));
    }
    let id = awr_core::Id::new();
    let key = plan["fingerprint"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();
    let recovery = runtime
        .join("mutations")
        .join(format!("source-config-{key}"));
    fs::create_dir(&recovery)?;
    let before = crate::intake_plan::current(&root, ".awr/project.toml", 64 * 1024)?
        .ok_or_else(|| Error::SourceConflict("source manifest disappeared".into()))?;
    for (name, content) in [
        ("before.toml", before.as_slice()),
        ("after.toml", effect.after_text.as_bytes()),
    ] {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(recovery.join(name))?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    let receipt_path = recovery.join("receipt.json");
    let mut receipt = serde_json::json!({"id":id,"operation":"source.configure","project_id":project.id,"preview":plan,"write_outcome":"pending","before_fingerprint":effect.before_fingerprint,"after_fingerprint":effect.after_fingerprint});
    save_configuration_receipt(&recovery, &receipt)?;
    // Use a held runtime directory for the final rename; do not follow replacement links.
    let directory = awr_source::open_dir_exact(&runtime)?;
    let stage = format!("config-{id}.tmp");
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut staged = directory.open_with(&stage, &options)?.into_std();
    staged.write_all(effect.after_text.as_bytes())?;
    staged.set_permissions(permissions)?;
    staged.sync_all()?;
    crate::intake_plan::verify_file(&root, &effect)?;
    directory.rename(&stage, &directory, "project.toml")?;
    receipt["write_outcome"] = serde_json::json!("applied");
    receipt["configuration_write_performed"] = serde_json::json!(true);
    let indexed =
        awr_source::index_project_locked(&mut store, &root, &candidate, false, &guard, None);
    let failure = match indexed {
        Ok(report) => {
            let failed = !report.ok;
            receipt["project_revision"] = serde_json::json!(report.project_revision);
            receipt["index"] = serde_json::to_value(report)?;
            failed.then(|| Error::SourceStale("configuration applied with source issues; inspect the result and reindex after fixing affected sources".into()))
        }
        Err(error) => {
            receipt["index_error"] = serde_json::to_value(error.report())?;
            Some(error)
        }
    };
    receipt["ok"] = serde_json::json!(failure.is_none());
    receipt["recovery_directory"] = serde_json::json!(recovery.strip_prefix(&root).unwrap());
    save_configuration_receipt(&recovery, &receipt)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!(
            "Source configuration applied; receipt {}",
            receipt_path.display()
        );
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

pub(crate) fn save_configuration_receipt(directory: &Path, receipt: &Value) -> Result<()> {
    let dir = awr_source::open_dir_exact(directory)?;
    let stage = format!("receipt-{}.tmp", awr_core::Id::new());
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = dir.open_with(&stage, &options)?;
    file.write_all(&serde_json::to_vec_pretty(receipt)?)?;
    file.sync_all()?;
    drop(file);
    dir.rename(&stage, &dir, "receipt.json")?;
    Ok(())
}

fn configure_status(root: &Path, fingerprint: &str) -> Result<()> {
    let key = fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint);
    if key.len() != 64 || !key.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::InvalidInput(
            "configuration receipt requires a SHA256 preview fingerprint".into(),
        ));
    }
    let root = root.canonicalize()?;
    let recovery = format!(
        ".awr/mutations/source-config-{}/receipt.json",
        key.to_ascii_lowercase()
    );
    let bytes = crate::intake_plan::current(&root, &recovery, awr_source::YAML_READ_CAP)?
        .ok_or_else(|| Error::NotFound("configuration receipt".into()))?;
    let receipt: Value = serde_json::from_slice(&bytes)?;
    if receipt["preview"]["fingerprint"].as_str()
        != Some(&format!("sha256:{}", key.to_ascii_lowercase()))
        || receipt["preview"]["project_root"] != serde_json::to_value(&root)?
    {
        return Err(Error::SourceConflict(
            "configuration receipt belongs to another preview or project".into(),
        ));
    }
    let observed = crate::intake_plan::current(&root, ".awr/project.toml", 64 * 1024)?
        .map(|b| awr_source::fingerprint(&b));
    let matches = match observed.as_deref() {
        Some(hash) if Some(hash) == receipt["after_fingerprint"].as_str() => "after",
        Some(hash) if Some(hash) == receipt["before_fingerprint"].as_str() => "before",
        _ => "conflict_or_missing",
    };
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"ok":true,"read_only":true,"source_write_performed":false,"runtime_write_performed":false,
        "receipt":receipt,"observed_configuration":matches,"current_fingerprint":observed,
        "interpretation":"stored write outcome and current bytes are separate observations; pending/conflicting results require inspection, never automatic replay"})
        )?
    );
    Ok(())
}

pub(crate) fn discover(root: &Path) -> Result<(Option<Manifest>, Vec<Value>, Vec<String>)> {
    let locations: &[(&str, &str, &[&str])] = &[
        (
            "goal",
            "markdown-heading-v1",
            &["GOALS.md", "docs/GOALS.md"],
        ),
        ("plan", "markdown-heading-v1", &["PLAN.md", "docs/PLAN.md"]),
        ("rules", "markdown-rules-v1", &["RULES.md", "docs/RULES.md"]),
        (
            "ledger",
            "yaml-ledger-v1",
            &[
                "ledger/work-ledger.yaml",
                "work-ledger.yaml",
                "ledger.yaml",
                "ledger/work-ledger.yml",
                "work-ledger.yml",
            ],
        ),
        (
            "decisions",
            "markdown-directory-v1",
            &["docs/decisions", "docs/adr", "docs/rfc", "adr"],
        ),
    ];
    let mut sources = vec![];
    let mut candidates = vec![];
    let mut ambiguous = vec![];
    for (domain, adapter, paths) in locations {
        let found: Vec<_> = paths
            .iter()
            .filter(|path| {
                if *domain == "decisions" {
                    root.join(path).is_dir()
                } else {
                    root.join(path).is_file()
                }
            })
            .collect();
        if found.len() > 1 && *domain != "decisions" {
            ambiguous.push((*domain).to_owned());
        }
        for path in found {
            candidates.push(json!({"domain":domain,"path":path,"adapter":adapter}));
            sources.push(SourceSpec {
                domain: (*domain).into(),
                role: if *domain == "decisions" {
                    "supporting"
                } else {
                    "primary"
                }
                .into(),
                path: Some(PathBuf::from(path)),
                locator: None,
                adapter: (*adapter).into(),
                options: Default::default(),
            });
        }
    }
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("AWR project")
        .to_owned();
    let manifest = if sources.is_empty() || !ambiguous.is_empty() {
        None
    } else {
        Some(Manifest {
            team: Default::default(),
            project: ProjectConfig {
                name,
                external_key: None,
                authority_mode: AuthorityMode::SourceFirst,
                authorized_roots: vec![],
                context_profile: awr_source::ContextProfile::Standard,
            },
            sources,
        })
    };
    Ok((manifest, candidates, ambiguous))
}

pub fn run(root: &Path, command: &SourceCommand, json_output: bool) -> Result<()> {
    match command {
        SourceCommand::Inventory {
            includes,
            exclude_globs,
            baseline,
        } => {
            let root = root.canonicalize()?;
            let current = inventory_files(&root, includes, exclude_globs)?;
            if let Some(baseline) = baseline {
                if baseline
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                {
                    return Err(Error::RuleViolation(
                        "inventory baseline must be a project-relative file".into(),
                    ));
                }
                let mut file = awr_source::open_file_exact(&root.join(baseline))?;
                let mut bytes = Vec::new();
                Read::by_ref(&mut file)
                    .take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > 16 * 1024 * 1024 {
                    return Err(Error::BudgetExceeded {
                        required: bytes.len(),
                        budget: 16 * 1024 * 1024,
                    });
                }
                let before: FileInventory = serde_json::from_slice(&bytes)?;
                let diff = compare_file_inventories(&before, &current)?;
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&diff)?);
                } else {
                    println!(
                        "File inventory fresh: {}; changed paths: {} ({} omitted)",
                        diff.fresh, diff.total_changes, diff.omitted_changes
                    );
                    for change in diff.changes {
                        println!("{:?}: {}", change.kind, change.path);
                    }
                }
            } else if json_output {
                println!("{}", serde_json::to_string_pretty(&current)?);
            } else {
                println!(
                    "File inventory: {} files; digest {}",
                    current.files.len(),
                    current.digest
                );
            }
            return Ok(());
        }
        SourceCommand::Relocate(args) => return crate::source_relocation::run(root, args),
        SourceCommand::RelocateStatus { fingerprint } => {
            return crate::source_relocation::status(root, fingerprint);
        }
        SourceCommand::RelocateRecover { fingerprint } => {
            return crate::source_relocation::recover(root, fingerprint);
        }
        SourceCommand::Changes(args) => return crate::source_changes::run(root, args, json_output),
        SourceCommand::ConfigureStatus {
            preview_fingerprint,
        } => {
            return configure_status(root, preview_fingerprint);
        }
        SourceCommand::Configure {
            manifest,
            accept,
            expected_preview,
        } => {
            return configure(
                root,
                manifest,
                *accept,
                expected_preview.as_deref(),
                json_output,
            );
        }
        SourceCommand::Show(request) => {
            return crate::drill::source_show(root, request, json_output);
        }
        SourceCommand::History { reference, window } => {
            return crate::drill::source_history(root, reference, window, json_output);
        }
        _ => (),
    }
    let root = root.canonicalize()?;
    let manifest = Manifest::load(&root)?;
    let runtime = runtime_dir(&root, false)?;
    match command {
        SourceCommand::Inventory { .. }
        | SourceCommand::Relocate(_)
        | SourceCommand::RelocateStatus { .. }
        | SourceCommand::RelocateRecover { .. }
        | SourceCommand::Configure { .. }
        | SourceCommand::Changes(_)
        | SourceCommand::ConfigureStatus { .. }
        | SourceCommand::Show(_)
        | SourceCommand::History { .. } => unreachable!(),
        SourceCommand::List => {
            let store = Store::open_readonly(&runtime.join("state.db"))?;
            let project = store.project_by_root(&root)?;
            let sources = store.sources(project.id)?;
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"project_id":project.id,"project_revision":project.project_revision,
                "freshness_basis":"last_scan_or_index","mappings":manifest.sources,"sources":sources})
                    )?
                );
            } else {
                println!("Sources at last observation; use source scan to refresh:");
                for source in sources {
                    println!(
                        "{} {} {} r{} {}",
                        source.domain,
                        source.role,
                        freshness(source.freshness),
                        source.revision,
                        source.locator
                    );
                }
            }
        }
        SourceCommand::Scan | SourceCommand::Reindex { .. } => {
            let mut store = Store::open(&runtime.join("state.db"))?;
            let report = match command {
                SourceCommand::Scan => scan_project(&mut store, &root, &manifest)?,
                SourceCommand::Reindex { force } => {
                    index_project(&mut store, &root, &manifest, *force)?
                }
                _ => unreachable!(),
            };
            if json_output {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
            if !report.ok {
                return Err(Error::SourceStale(
                    "source operation incomplete; inspect reported issues".into(),
                ));
            }
        }
    }
    Ok(())
}
fn freshness(value: Freshness) -> &'static str {
    match value {
        Freshness::Fresh => "fresh",
        Freshness::Stale => "stale",
        Freshness::Unavailable => "unavailable",
    }
}
fn print_report(report: &IndexReport) {
    println!(
        "Operation ok: {}; projection complete: {}",
        report.ok, report.projection_complete
    );
    println!(
        "Indexed: {}; unchanged: {}; pending: {}; retired: {}; project revision: {}",
        report.indexed, report.unchanged, report.pending, report.retired, report.project_revision
    );
    let mut warnings = BTreeMap::new();
    for source in &report.sources {
        println!(
            "{} {} {}",
            source.action,
            freshness(source.freshness),
            source.locator
        );
        for warning in &source.warnings {
            warnings.insert(format!("{}: {warning}", source.locator), ());
        }
    }
    for warning in warnings.keys() {
        println!("warning: {warning}");
    }
    for issue in &report.issues {
        println!("{}: {}: {}", issue.code, issue.mapping, issue.message);
        if issue
            .details
            .as_ref()
            .and_then(|d| d.get("location"))
            .is_none()
            && let Some(locator) = &issue.locator
        {
            println!("  Source: {locator}");
        }
        let details = awr_core::render_diagnostic_details(issue.details.as_ref());
        if !details.is_empty() {
            println!("{details}");
        }
    }
}
