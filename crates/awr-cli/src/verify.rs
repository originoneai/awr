//! Commit gates for local observations and database-free CI checks.
use awr_core::{
    CheckGateRequest, CheckOutcome, DiscriminationMethod, Error, Freshness, Id, Result, Source,
    WorkItem, WorkStatus,
};
use awr_source::{
    GatePolicy, Manifest, ParseContext, SourceAdapter, SourceSnapshot, SourceSpec,
    YamlLedgerAdapter, fingerprint,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};

const HOOK: &str = include_str!("../../../examples/git-hooks/pre-push");

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Commit to verify; defaults to HEAD. Rerun uses the current checkout.
    #[arg(long)]
    commit: Option<String>,
    /// Select an exact work key. Repeat to select several.
    #[arg(long)]
    work: Vec<String>,
    /// Refuse a commit with no associated work.
    #[arg(long)]
    require_work: bool,
    /// Execute checks from the checked-out ledger, without a state database.
    #[arg(long)]
    rerun: bool,
    /// Compare completed work and prove failed checks at this older commit.
    #[arg(long, requires = "rerun")]
    baseline: Option<String>,
    #[command(subcommand)]
    command: Option<VerifyCommand>,
}

#[derive(Debug, Subcommand)]
enum VerifyCommand {
    /// Preview a pre-push hook; --accept installs it while preserving existing files.
    InstallHook {
        #[arg(long)]
        accept: bool,
    },
}

impl VerifyArgs {
    pub fn stateless(&self) -> bool {
        self.rerun || self.command.is_some()
    }
}

fn git_output(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        // A hook inherits repository variables that would redirect a baseline worktree.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()?)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = git_output(root, args)?;
    if !output.status.success() {
        return Err(Error::InvalidInput(format!(
            "Git {} failed: {}",
            args.first().unwrap_or(&"command"),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(git(root, args)?)
        .map(|s| s.trim().to_owned())
        .map_err(|_| Error::InvalidInput("Git returned a non-UTF-8 path or commit".into()))
}

fn commit(root: &Path, reference: &str) -> Result<String> {
    git_text(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
}

fn ledger_only_delta(root: &Path, bound: &str, sha: &str) -> Result<Option<Vec<String>>> {
    if !git_output(root, &["merge-base", "--is-ancestor", bound, sha])?
        .status
        .success()
    {
        return Ok(None);
    }
    let repository =
        PathBuf::from(git_text(root, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let manifest = Manifest::load(root)?;
    let mut ledgers = BTreeSet::new();
    for spec in manifest.sources.iter().filter(|s| s.domain == "ledger") {
        let Some(path) = &spec.path else { continue };
        let mut normalized = PathBuf::new();
        for component in root.join(path).components() {
            match component {
                Component::CurDir => (),
                Component::ParentDir => {
                    normalized.pop();
                }
                other => normalized.push(other.as_os_str()),
            }
        }
        if let Ok(path) = normalized.strip_prefix(&repository) {
            ledgers.insert(path.to_path_buf());
        }
    }
    // Disable rename detection so both the old and new path must qualify.
    let diff = git(
        root,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "--no-relative",
            "-z",
            bound,
            sha,
            "--",
        ],
    )?;
    let mut paths = BTreeSet::new();
    for bytes in diff.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let Ok(path) = std::str::from_utf8(bytes) else {
            return Ok(None);
        };
        if !ledgers.contains(Path::new(path)) {
            return Ok(None);
        }
        paths.insert(path.to_owned());
    }
    Ok(Some(paths.into_iter().collect()))
}

pub fn run(root: &Path, args: &VerifyArgs, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    if let Some(VerifyCommand::InstallHook { accept }) = args.command {
        return install_hook(&root, accept, json_output);
    }
    let policy = GatePolicy::load(&root)?;
    let sha = commit(&root, args.commit.as_deref().unwrap_or("HEAD"))?;
    let require_work = args.require_work || policy.verify.require_work;
    let mut report = if args.rerun {
        if sha != commit(&root, "HEAD")? {
            return Err(Error::InvalidInput(
                "rerun requires --commit to resolve to the checked-out HEAD".into(),
            ));
        }
        rerun(&root, args, &sha)?
    } else {
        local(&root, args, &sha, require_work)?
    };
    let empty = report["results"].as_array().is_some_and(Vec::is_empty);
    let met = report["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["met"] == true)
        && (!require_work || !empty);
    report["ok"] = json!(met);
    report["met"] = json!(met);
    report["require_work"] = json!(require_work);
    if empty {
        report["reason"] = json!("no_associated_work");
    }
    if json_output {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!("Verify {sha}: {}", if met { "passed" } else { "failed" });
        if empty {
            println!("No associated work for this commit.");
        }
        for result in report["results"].as_array().unwrap() {
            println!(
                "{}: {}",
                result["work"].as_str().unwrap(),
                if result["met"] == true {
                    "passed"
                } else {
                    "failed"
                }
            );
            for unmet in result["unmet"].as_array().into_iter().flatten() {
                println!("  {unmet}");
            }
        }
    }
    if met {
        Ok(())
    } else {
        Err(Error::EvidenceMissing(
            "commit verification failed; inspect the per-work results".into(),
        ))
    }
}

fn local(root: &Path, args: &VerifyArgs, sha: &str, require_work: bool) -> Result<Value> {
    let database = crate::source::runtime_dir(root, false)?.join("state.db");
    let mut origin = awr_store::Store::open_existing(&database)?;
    let snapshot = awr_source::refresh_snapshot(&mut origin, root)?;
    if !snapshot.refresh.ok {
        return Err(Error::SourceStale(
            "refresh project sources before verification".into(),
        ));
    }
    let store = &snapshot.store;
    let project = store.project(snapshot.refresh.project_id)?;
    let works: BTreeMap<_, _> = store
        .work_items(project.id)?
        .into_iter()
        .map(|work| (work.item.meta.id, work.item.meta.external_key))
        .collect();
    let mut query = awr_store::EventQuery {
        branch: awr_store::BranchFilter::exact(project.current_branch_id),
        event_type: Some("work.completed".into()),
        limit: 1000,
        ..Default::default()
    };
    let mut receipts = BTreeMap::new();
    let mut latest = BTreeMap::new();
    loop {
        let page = store.query_events(project.id, &query)?;
        for event in page.events {
            if event.payload["work_action"]["completion"]["source_sha"]
                .as_str()
                .is_some_and(|source| source.eq_ignore_ascii_case(sha))
                && let Some(work) = event.work_item_id
            {
                receipts.insert(work, event.clone());
            }
            if let Some(work) = event.work_item_id {
                latest.insert(work, event);
            }
        }
        query.cursor = page.next_cursor;
        if query.cursor.is_none() {
            break;
        }
    }
    let mut keys: BTreeSet<String> = args.work.iter().cloned().collect();
    if keys.is_empty() {
        for id in receipts.keys() {
            let key = works.get(id).ok_or_else(|| Error::SourceStale("completed work is absent from current sources; restore its definition for verification".into()))?;
            keys.insert(key.clone());
        }
    }
    let required_fallback = args.work.is_empty() && keys.is_empty() && require_work;
    let mut deltas = BTreeMap::new();
    if !args.work.is_empty() || required_fallback {
        for (id, key) in &works {
            if receipts.contains_key(id)
                || !latest.contains_key(id)
                || (!required_fallback && !keys.contains(key))
            {
                continue;
            }
            if let Some(binding) =
                store.current_completion_binding(project.id, key, project.current_branch_id)?
                && !binding.source_sha.eq_ignore_ascii_case(sha)
                && let Some(paths) = ledger_only_delta(root, &binding.source_sha, sha)?
            {
                deltas.insert(key.clone(), paths);
            }
        }
    }
    if required_fallback {
        keys.extend(deltas.keys().cloned());
    }
    let policy = GatePolicy::load(root)?.checks;
    let mut results = Vec::new();
    for key in keys {
        let binding =
            store.current_completion_binding(project.id, &key, project.current_branch_id)?;
        let work = store.work_item(project.id, &key)?.item;
        let delta = deltas.get(&key);
        let source_sha = if delta.is_some() {
            binding.as_ref().unwrap().source_sha.as_str()
        } else {
            sha
        };
        let receipt = receipts
            .get(&work.meta.id)
            .or_else(|| delta.and_then(|_| latest.get(&work.meta.id)));
        let session = match receipt {
            Some(event) => event.payload["creating_session_id"]
                .as_str()
                .map(str::parse::<Id>)
                .transpose()
                .map_err(|_| Error::Storage("invalid completion session identity".into()))?
                .or(event.session_id),
            None => None,
        };
        let gate = store.work_check_gate(
            project.id,
            &CheckGateRequest {
                work: key.clone(),
                branch: project.current_branch_id,
                session,
                source_sha: source_sha.into(),
                check_waiver: binding
                    .as_ref()
                    .filter(|b| b.source_sha.eq_ignore_ascii_case(source_sha))
                    .and_then(|b| b.check_waiver.clone()),
            },
            &policy,
        )?;
        let mut result =
            json!({"work":key,"met":!gate.would_refuse,"unmet":gate.unmet,"check_gate":gate});
        if let Some(paths) = delta {
            result["bound_commit"] = json!(source_sha);
            result["delta"] = json!("ledger_only");
            result["delta_paths"] = json!(paths.iter().take(16).collect::<Vec<_>>());
            if paths.len() > 16 {
                result["delta_paths_omitted"] = json!(paths.len() - 16);
            }
        }
        results.push(result);
    }
    Ok(
        json!({"mode":"local","commit":sha,"results":results,"project_revision":project.project_revision}),
    )
}

fn relative(path: &Path) -> Result<&str> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::InvalidInput(
            "verify requires repository-relative ledger and check paths".into(),
        ));
    }
    path.to_str()
        .ok_or_else(|| Error::InvalidInput("verify paths must be UTF-8".into()))
}

fn parse_ledger(spec: &SourceSpec, path: &str, bytes: Vec<u8>) -> Result<Vec<WorkItem>> {
    let source = Source {
        id: Id::new(),
        project_id: Id::new(),
        domain: spec.domain.clone(),
        role: spec.role.clone(),
        locator: format!("project:///{path}"),
        format: "yaml".into(),
        adapter: spec.adapter.clone(),
        revision: 0,
        fingerprint: fingerprint(&bytes),
        freshness: Freshness::Fresh,
        config: json!({}),
    };
    let snapshot = SourceSnapshot {
        locator: source.locator.clone(),
        fingerprint: source.fingerprint.clone(),
        bytes,
        content_review: None,
    };
    let context = ParseContext {
        source: &source,
        existing_ids: BTreeMap::new(),
    };
    Ok(YamlLedgerAdapter
        .parse(&snapshot, &context, spec)?
        .work_items)
}

fn ledger_work(
    root: &Path,
    manifest: &Manifest,
    baseline: Option<&str>,
) -> Result<BTreeMap<String, WorkItem>> {
    let mut works = BTreeMap::new();
    let mut ledgers = 0;
    let prefix = git_text(root, &["rev-parse", "--show-prefix"])?;
    for spec in manifest.sources.iter().filter(|s| s.domain == "ledger") {
        if !matches!(
            spec.adapter.as_str(),
            "yaml-ledger-v1" | "yaml-workstream-ledger-v1"
        ) {
            return Err(Error::Unsupported(
                "verify rerun requires a YAML ledger adapter".into(),
            ));
        }
        let path = relative(spec.path.as_deref().ok_or_else(|| {
            Error::InvalidInput("verify rerun requires a checked-in ledger path".into())
        })?)?;
        let bytes = if let Some(base) = baseline {
            let repo_path = format!("{prefix}{path}");
            if git(root, &["ls-tree", "--full-tree", base, "--", &repo_path])?.is_empty() {
                continue;
            }
            git(root, &["show", &format!("{base}:{repo_path}")])?
        } else {
            awr_source::read_under_root(root, path)?
        };
        ledgers += 1;
        for work in parse_ledger(spec, path, bytes)? {
            let key = work.meta.external_key.clone();
            if works.insert(key.clone(), work).is_some() {
                return Err(Error::InvalidInput(format!(
                    "duplicate verify work key: {key}"
                )));
            }
        }
    }
    if ledgers == 0 && baseline.is_none() {
        return Err(Error::InvalidInput(
            "verify rerun requires a checked-in YAML ledger".into(),
        ));
    }
    Ok(works)
}

struct BaselineWorktree {
    repository: PathBuf,
    path: PathBuf,
    registered: bool,
}
impl BaselineWorktree {
    fn new(root: &Path, base: &str) -> Result<Self> {
        let mut tree = Self {
            repository: root.into(),
            path: std::env::temp_dir().join(format!("awr-verify-{}", Id::new())),
            registered: true,
        };
        // The guard also removes a partially created worktree on failure.
        git(
            root,
            &[
                "worktree",
                "add",
                "--detach",
                tree.path.to_str().ok_or_else(|| {
                    Error::InvalidInput("temporary worktree path must be UTF-8".into())
                })?,
                base,
            ],
        )?;
        tree.path = tree.path.canonicalize()?;
        Ok(tree)
    }
    fn cleanup(&mut self) -> Result<()> {
        if self.registered {
            git(
                &self.repository,
                &["worktree", "remove", "--force", self.path.to_str().unwrap()],
            )?;
            self.registered = false;
        }
        if self.path.exists() {
            fs::remove_dir_all(&self.path)?;
        }
        Ok(())
    }
}
impl Drop for BaselineWorktree {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn copy_oracle(root: &Path, destination: &Path, path: &str) -> Result<()> {
    let path_ref = Path::new(path);
    relative(path_ref)?;
    let bytes = awr_source::read_under_root(root, path)?;
    if bytes.len() as u64 > awr_core::CHECK_FILE_CAP {
        return Err(Error::InvalidInput(
            "verify check file exceeds 4 MiB".into(),
        ));
    }
    let mut parent = destination.to_path_buf();
    for component in path_ref.parent().into_iter().flat_map(Path::components) {
        parent.push(component.as_os_str());
        match fs::symlink_metadata(&parent) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(Error::RuleViolation(
                    "baseline check path traverses a link or non-directory".into(),
                ));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&parent)?,
            Err(e) => return Err(e.into()),
        }
    }
    let target = parent.join(path_ref.file_name().unwrap());
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Error::RuleViolation(
            "baseline check file is a symbolic link".into(),
        ));
    }
    fs::write(&target, bytes)?;
    fs::set_permissions(&target, fs::metadata(root.join(path))?.permissions())?;
    Ok(())
}

fn observation(argv: &[String], cwd: &Path, timeout: Duration) -> Result<Value> {
    let run = awr_runtime::run_check_direct(argv, cwd, timeout)?;
    Ok(
        json!({"outcome":run.outcome,"exit_code":run.exit_code,"reason":run.reason,
        "stdout_sha256":run.stdout_sha256,"stderr_sha256":run.stderr_sha256,
        "started_at":run.started_at,"finished_at":run.finished_at}),
    )
}

fn rerun(root: &Path, args: &VerifyArgs, sha: &str) -> Result<Value> {
    let manifest = Manifest::load(root)?;
    let works = ledger_work(root, &manifest, None)?;
    let base = args
        .baseline
        .as_deref()
        .map(|b| commit(root, b))
        .transpose()?;
    let old = base
        .as_deref()
        .map(|b| ledger_work(root, &manifest, Some(b)))
        .transpose()?
        .unwrap_or_default();
    let keys: BTreeSet<_> = if args.work.is_empty() {
        works
            .iter()
            .filter(|(key, work)| {
                work.status == WorkStatus::Completed
                    && old
                        .get(*key)
                        .is_none_or(|w| w.status != WorkStatus::Completed)
            })
            .map(|(key, _)| key.clone())
            .collect()
    } else {
        args.work.iter().cloned().collect()
    };
    if let Some(key) = keys.iter().find(|k| !works.contains_key(*k)) {
        return Err(Error::NotFound(format!("verify work {key}")));
    }
    let mut tree = if keys.is_empty() {
        None
    } else {
        base.as_deref()
            .map(|b| BaselineWorktree::new(root, b))
            .transpose()?
    };
    let prefix = git_text(root, &["rev-parse", "--show-prefix"])?;
    let mut results = Vec::new();
    for key in keys {
        let work = &works[&key];
        let mut unmet = Vec::new();
        let mut checks = Vec::new();
        if let Some(definition) = &work.definition {
            definition.validate()?;
            let discrimination = definition.discrimination.as_ref().or_else(|| {
                definition
                    .inherits
                    .as_ref()
                    .and_then(|k| works.get(k)?.definition.as_ref()?.discrimination.as_ref())
            });
            let baseline_required = discrimination
                .is_none_or(|d| d.method == DiscriminationMethod::FailBeforePassAfter);
            for criterion in &work.acceptance {
                if !definition
                    .examples
                    .iter()
                    .any(|e| &e.criterion == criterion)
                {
                    unmet.push(json!({"criterion":criterion,"reason":"check_unbound"}));
                }
            }
            for example in &definition.examples {
                let (Some(id), Some(check)) = (&example.id, &example.check) else {
                    unmet.push(json!({"example":example.id,"reason":"check_unbound"}));
                    continue;
                };
                let timeout = Duration::from_secs(u64::from(check.timeout_seconds.unwrap_or(300)));
                let before = if let Some(tree) = &tree
                    && baseline_required
                {
                    if !prefix.is_empty() {
                        // The directory handle confines creation even if BASE linked this prefix elsewhere.
                        awr_source::open_dir_exact(&tree.path)?.create_dir_all(&prefix)?;
                    }
                    let cwd = tree.path.join(&prefix).canonicalize()?;
                    if !cwd.starts_with(&tree.path) {
                        return Err(Error::RuleViolation(
                            "baseline project directory escapes its worktree".into(),
                        ));
                    }
                    for file in &check.files {
                        copy_oracle(root, &cwd, file)?;
                    }
                    let run = observation(&check.run, &cwd, timeout)?;
                    if run["outcome"] != json!(CheckOutcome::Failed) {
                        unmet.push(json!({"example":id,"reason":if run["outcome"] == json!(CheckOutcome::Passed) {"passed_before_change"} else {"baseline_inconclusive"}}));
                    }
                    Some(run)
                } else {
                    None
                };
                // Read each declared oracle even when no baseline is requested.
                for file in &check.files {
                    if awr_source::read_under_root(root, file)?.len() as u64
                        > awr_core::CHECK_FILE_CAP
                    {
                        return Err(Error::InvalidInput(
                            "verify check file exceeds 4 MiB".into(),
                        ));
                    }
                }
                let acceptance = observation(&check.run, root, timeout)?;
                if acceptance["outcome"] != json!(CheckOutcome::Passed) {
                    unmet.push(json!({"example":id,"reason":"acceptance_failed"}));
                }
                checks.push(json!({"example":id,"baseline":before,"acceptance":acceptance}));
            }
            if checks.is_empty() && unmet.is_empty() {
                unmet.push(json!({"reason":"check_unbound"}));
            }
        } else {
            unmet.push(json!({"reason":"check_unbound"}));
        }
        results.push(json!({"work":key,"met":unmet.is_empty(),"unmet":unmet,"checks":checks}));
    }
    let path = tree.as_ref().map(|t| t.path.clone());
    if let Some(tree) = &mut tree {
        tree.cleanup()?;
    }
    Ok(
        json!({"mode":"rerun","commit":sha,"baseline":base,"baseline_worktree":path,"results":results}),
    )
}

fn install_hook(root: &Path, accept: bool, json_output: bool) -> Result<()> {
    let path = PathBuf::from(git_text(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "hooks/pre-push",
        ],
    )?);
    let mut installed = false;
    let mut existing = fs::symlink_metadata(&path).is_ok();
    if accept && !existing {
        fs::create_dir_all(path.parent().unwrap())?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(HOOK.as_bytes())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o755))?;
                }
                file.sync_all()?;
                installed = true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => existing = true,
            Err(e) => return Err(e.into()),
        }
    }
    let report = json!({"ok":true,"path":path,"installed":installed,"existing":existing,"preview":HOOK,
        "manual_line":"awr verify --commit \"$commit\" || exit $?"});
    if json_output {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!("Pre-push hook: {}", path.display());
        if existing {
            println!(
                "Existing hook preserved. Add inside its pushed-commit loop:\n{}",
                report["manual_line"].as_str().unwrap()
            );
        } else if installed {
            println!("Installed.");
        } else {
            println!("{HOOK}\nAccept: awr verify install-hook --accept");
        }
    }
    Ok(())
}
