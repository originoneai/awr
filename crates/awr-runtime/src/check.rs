//! Direct subprocess checks, followed by journal observations and registered completion reports.
use crate::{
    ArtifactFile, Runtime,
    artifact::storage_directory,
    branch::{git_command, observe_git_ref},
};
use awr_core::*;
use awr_store::Store;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

// Stateless execution. Returned log files remain available until the result is dropped.
#[derive(Debug)]
pub struct DirectCheckResult {
    pub exit_code: Option<i32>,
    pub outcome: CheckOutcome,
    pub reason: Option<String>,
    pub log_dir: PathBuf,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub started_at: i64,
    pub finished_at: i64,
}
impl Drop for DirectCheckResult {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.log_dir);
    }
}

struct RunningChild(Child);
impl Drop for RunningChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
fn signal_group(child: &Child, signal: i32) -> Result<()> {
    // POSIX kill with a negative PID targets only the process group created for this run.
    unsafe extern "C" {
        fn kill(pid: std::ffi::c_int, signal: std::ffi::c_int) -> std::ffi::c_int;
    }
    let pid = i32::try_from(child.id())
        .map_err(|_| Error::InvalidInput("child PID exceeds POSIX range".into()))?;
    // SAFETY: the PID is a live owned child's group ID; the function takes no pointers.
    if unsafe { kill(-pid, signal) } == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(3) {
            return Err(error.into());
        }
    }
    Ok(())
}

fn terminate(child: &mut Child) -> Result<()> {
    #[cfg(unix)]
    {
        signal_group(child, 15)?;
        // Keep the child unreaped until group cleanup finishes, so its PID cannot be reused.
        std::thread::sleep(Duration::from_millis(200));
        // A descendant may survive the direct child; kill the owned group after the grace period.
        if let Err(error) = signal_group(child, 9) {
            // Darwin reports EPERM for a group containing only the unreaped, terminated child.
            // Observe exit only after the final group signal, while its PID is still reserved.
            let exited = child.try_wait()?.is_some();
            if !exited
                || !matches!(&error, Error::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied)
            {
                return Err(error);
            }
        }
    }
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            child.kill()?;
        }
    }
    #[cfg(not(any(unix, windows)))]
    child.kill()?;
    child.wait()?;
    Ok(())
}

fn digest_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 16384];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Run an argument vector directly at `cwd`, with no project database, admission or shell.
pub fn run_check_direct(
    argv: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<DirectCheckResult> {
    if argv.is_empty() || argv[0].is_empty() || timeout.is_zero() {
        return Err(Error::InvalidInput(
            "a direct check requires a command and a positive timeout".into(),
        ));
    }
    let log_dir = std::env::temp_dir().join(format!("awr-direct-check-{}", Id::new()));
    fs::create_dir(&log_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&log_dir, fs::Permissions::from_mode(0o700))?;
    }
    let mut result = DirectCheckResult {
        exit_code: None,
        outcome: CheckOutcome::Errored,
        reason: None,
        log_dir,
        stdout_sha256: String::new(),
        stderr_sha256: String::new(),
        started_at: now_millis()?,
        finished_at: 0,
    };
    let stdout = File::create(result.log_dir.join("stdout.log"))?;
    let stderr = File::create(result.log_dir.join("stderr.log"))?;
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    match command.spawn() {
        Err(_) => result.reason = Some("spawn_failed".into()),
        Ok(child) => {
            let mut child = RunningChild(child);
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(status) = child.0.try_wait()? {
                    result.exit_code = status.code();
                    result.outcome = if status.success() {
                        CheckOutcome::Passed
                    } else {
                        CheckOutcome::Failed
                    };
                    if result.exit_code.is_none() {
                        result.outcome = CheckOutcome::Unknown;
                        result.reason = Some("terminated_by_signal".into());
                    }
                    break;
                }
                if Instant::now() >= deadline {
                    terminate(&mut child.0)?;
                    result.outcome = CheckOutcome::Unknown;
                    result.reason = Some("timeout".into());
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    result.finished_at = now_millis()?;
    result.stdout_sha256 = digest_file(&result.log_dir.join("stdout.log"))?;
    result.stderr_sha256 = digest_file(&result.log_dir.join("stderr.log"))?;
    Ok(result)
}

// Project inputs and bounded oracle reads.
#[derive(Debug, Clone)]
pub struct RunCheckRequest {
    pub work: String,
    pub session: Id,
    pub phase: CheckPhase,
    pub examples: Vec<String>,
    pub timeout_seconds: Option<u32>,
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub event_id: Id,
    #[serde(flatten)]
    pub observation: CheckPayload,
}

fn oracle_files(
    root: &Path,
    check: &ExampleCheck,
) -> std::result::Result<Vec<OracleFile>, &'static str> {
    check
        .files
        .iter()
        .map(|path| {
            let resolved = root
                .join(path.replace('\\', "/"))
                .canonicalize()
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        "oracle_file_missing"
                    } else {
                        "oracle_file_unreadable"
                    }
                })?;
            if !resolved.starts_with(root) || !resolved.is_file() {
                return Err("oracle_file_unreadable");
            }
            let bytes = awr_source::read_capped(&resolved, CHECK_FILE_CAP)
                .map_err(|_| "oracle_file_unreadable")?;
            Ok(OracleFile {
                path: path.clone(),
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                size: bytes.len() as u64,
            })
        })
        .collect()
}

fn preview(argv: &[String]) -> String {
    let run = argv.join(" ");
    let mut end = run.len().min(256);
    while !run.is_char_boundary(end) {
        end -= 1;
    }
    run[..end]
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn log_directory(
    root: &Path,
    key: &str,
    phase: CheckPhase,
    example: &str,
) -> Result<(String, Dir)> {
    let key = if key != "."
        && key != ".."
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        key.into()
    } else {
        format!("work-{:x}", Sha256::digest(key.as_bytes()))
    };
    let mut dir = storage_directory(root)?;
    for name in ["checks", key.as_str()] {
        match dir.create_dir(name) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        if !dir.symlink_metadata(name)?.is_dir() {
            return Err(Error::RuleViolation(
                "check storage must use real directories".into(),
            ));
        }
        dir = dir.open_dir_nofollow(name)?;
    }
    let first_sequence = dir
        .entries()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|name| name.split('-').next()?.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("check sequence exhausted".into()))?;
    for sequence in first_sequence.. {
        let name = format!("{sequence}-{}-{example}", phase.as_str());
        match dir.create_dir(&name) {
            Ok(()) => {
                return Ok((
                    format!(".awr/artifacts/checks/{key}/{name}"),
                    dir.open_dir_nofollow(&name)?,
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!("sequence cannot exhaust u64")
}

fn write_log(dir: &Dir, name: &str, source: Option<&Path>) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .create_new(true)
        .write(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut target = dir.open_with(name, &options)?;
    if let Some(path) = source {
        std::io::copy(&mut File::open(path)?, &mut target)?;
    }
    target.sync_all()?;
    Ok(())
}

/// Observe selected checks sequentially; recording a failure is itself a successful operation.
pub fn run_check(store: &mut Store, root: &Path, request: &RunCheckRequest) -> Result<Value> {
    let root = root.canonicalize()?;
    let project = store.project_by_root(&root)?;
    store.require_check(
        project.id,
        request.expected_revision,
        &request.work,
        request.session,
        request.phase,
    )?;
    let mutation = if request.phase == CheckPhase::Mutation {
        Some(awr_source::GatePolicy::load(&root)?.checks.mutation.ok_or_else(|| {
            Error::Unsupported("declare [checks.mutation] with a run argument vector in .awr/gates.toml before running --phase mutation".into())
        })?)
    } else {
        None
    };
    let work = store.work_item(project.id, &request.work)?.item;
    let definition = work
        .definition
        .as_ref()
        .ok_or_else(|| Error::InvalidInput("work check requires declared examples".into()))?;
    definition.validate()?;
    let report = store.work_definition_report(project.id, &request.work)?;
    let required: BTreeSet<_> = report
        .criterion_tiers
        .iter()
        .filter(|(_, t)| *t == DeliveryTier::Required)
        .map(|(c, _)| c.as_str())
        .collect();
    let bound: Vec<_> = definition
        .examples
        .iter()
        .filter(|e| e.check.is_some())
        .collect();
    let required_ids: BTreeSet<_> = bound
        .iter()
        .filter(|e| required.contains(e.criterion.as_str()))
        .filter_map(|e| e.id.as_deref())
        .collect();
    let unmapped: Vec<_> = work
        .acceptance
        .iter()
        .filter(|criterion| !bound.iter().any(|example| &example.criterion == *criterion))
        .collect();
    let selected: BTreeSet<_> = request.examples.iter().map(String::as_str).collect();
    if bound
        .iter()
        .any(|e| e.id.is_none() || !work.acceptance.contains(&e.criterion))
        || definition
            .examples
            .iter()
            .filter_map(|e| e.id.as_ref())
            .collect::<BTreeSet<_>>()
            .len()
            != definition
                .examples
                .iter()
                .filter(|e| e.id.is_some())
                .count()
        || selected
            .iter()
            .any(|id| !bound.iter().any(|e| e.id.as_deref() == Some(*id)))
        || request.timeout_seconds == Some(0)
    {
        return Err(Error::InvalidInput(
            "select unique, check-bound example IDs and a positive timeout".into(),
        ));
    }
    let examples: Vec<_> = bound
        .into_iter()
        .filter(|e| {
            if selected.is_empty() {
                required.contains(e.criterion.as_str())
            } else {
                selected.contains(e.id.as_deref().unwrap())
            }
        })
        .collect();
    if examples.is_empty() {
        return Err(Error::InvalidInput(
            "no check-bound examples were selected".into(),
        ));
    }
    let mut revision = request.expected_revision;
    let mut results = Vec::new();
    let mutation_oracles: Option<Vec<_>> = mutation.as_ref().map(|_| {
        examples
            .iter()
            .map(|e| oracle_files(&root, e.check.as_ref().unwrap()))
            .collect()
    });
    // One project-wide report is shared by the selected examples' oracle-bound observations.
    let mut mutation_run = None;
    let mut mutation_report = None;
    for (index, example) in examples.into_iter().enumerate() {
        store.require_check(
            project.id,
            revision,
            &request.work,
            request.session,
            request.phase,
        )?;
        let id = example.id.as_deref().unwrap();
        let check = example.check.as_ref().unwrap();
        let argv = mutation.as_ref().map_or(&check.run, |m| &m.run);
        let before = mutation_oracles
            .as_ref()
            .map_or_else(|| oracle_files(&root, check), |files| files[index].clone());
        let source = observe_git_ref(&root, "HEAD").ok().map(|b| b.commit_sha);
        let tree_state = if source.is_some() {
            match git_command(&root, &["status", "--porcelain=v1", "--untracked-files=no"]).output()
            {
                Ok(o) if o.status.success() => {
                    if o.stdout.is_empty() {
                        CheckTreeState::Clean
                    } else {
                        CheckTreeState::Dirty
                    }
                }
                _ => CheckTreeState::Unknown,
            }
        } else {
            CheckTreeState::Unknown
        };
        let (log_dir, dir) = log_directory(&root, &work.meta.external_key, request.phase, id)?;
        let empty_hash = format!("{:x}", Sha256::digest([]));
        let started = now_millis()?;
        let mut observation = CheckPayload {
            version: 1,
            example: id.into(),
            criterion_sha256: format!("{:x}", Sha256::digest(example.criterion.as_bytes())),
            phase: request.phase,
            outcome: CheckOutcome::Errored,
            reason: before.as_ref().err().map(|r| r.to_string()),
            log_dir,
            exit_code: None,
            example_oracle: before
                .as_ref()
                .ok()
                .map(|f| example_oracle(id, &check.run, f))
                .transpose()?,
            oracle_files: before.as_ref().cloned().unwrap_or_default(),
            run_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(argv)?)),
            run_preview: preview(argv),
            source_sha: source,
            tree_state,
            stdout_sha256: empty_hash.clone(),
            stderr_sha256: empty_hash,
            started_at: started,
            finished_at: started,
            mutants: None,
            survivors: None,
        };
        // Bound the full event before running an arbitrary command.
        observation.validate()?;
        if before.is_ok() {
            let timeout = Duration::from_secs(u64::from(
                request
                    .timeout_seconds
                    .or(if mutation.is_some() {
                        None
                    } else {
                        check.timeout_seconds
                    })
                    .unwrap_or(300),
            ));
            let single;
            let direct = if mutation.is_some() {
                if mutation_run.is_none() {
                    mutation_run = Some(run_check_direct(argv, &root, timeout)?);
                }
                mutation_run.as_ref().unwrap()
            } else {
                single = run_check_direct(argv, &root, timeout)?;
                &single
            };
            write_log(&dir, "stdout.log", Some(&direct.log_dir.join("stdout.log")))?;
            write_log(&dir, "stderr.log", Some(&direct.log_dir.join("stderr.log")))?;
            observation.exit_code = direct.exit_code;
            observation.outcome = direct.outcome;
            observation.reason = direct.reason.clone();
            observation.stdout_sha256 = direct.stdout_sha256.clone();
            observation.stderr_sha256 = direct.stderr_sha256.clone();
            observation.started_at = direct.started_at;
            observation.finished_at = direct.finished_at;
            if mutation.is_some() && direct.outcome == CheckOutcome::Passed {
                if mutation_report.is_none() {
                    let bytes = awr_source::read_capped(
                        &direct.log_dir.join("stdout.log"),
                        MUTATION_REPORT_CAP as u64,
                    )?;
                    mutation_report = Some(MutationReport::parse(&bytes)?);
                }
                let report = mutation_report.as_ref().unwrap();
                if observation
                    .source_sha
                    .as_ref()
                    .is_none_or(|sha| !sha.eq_ignore_ascii_case(&report.source_sha))
                {
                    return Err(Error::InvalidInput(
                        "mutation report source_sha must match the current Git commit".into(),
                    ));
                }
                let counts = report.counts();
                observation.mutants = Some(counts);
                observation.survivors = Some(report.survivors());
                if !counts.passed() {
                    observation.outcome = CheckOutcome::Failed;
                    observation.reason = Some(
                        if counts.survived > 0 {
                            "mutants_survived"
                        } else {
                            "mutation_timeout"
                        }
                        .into(),
                    );
                }
            }
            if oracle_files(&root, check).as_ref().ok() != before.as_ref().ok() {
                observation.outcome = CheckOutcome::Errored;
                observation.reason = Some("oracle_changed_during_run".into());
            }
            if mutation.is_some()
                && observe_git_ref(&root, "HEAD").ok().map(|b| b.commit_sha)
                    != observation.source_sha
            {
                observation.outcome = CheckOutcome::Errored;
                observation.reason = Some("source_changed_during_run".into());
            }
        } else {
            write_log(&dir, "stdout.log", None)?;
            write_log(&dir, "stderr.log", None)?;
            observation.finished_at = now_millis()?;
        }
        let event = store.record_check(
            project.id,
            &RecordCheckRequest {
                work: request.work.clone(),
                session: request.session,
                expected_revision: revision,
                payload: observation.clone(),
            },
        )?;
        revision = event.project_revision;
        results.push(CheckResult {
            event_id: event.id,
            observation,
        });
    }
    let met = results
        .iter()
        .all(|r| request.phase.met(r.observation.outcome));
    let mut value = json!({"ok":true,"work":work.meta.external_key,"phase":request.phase,"results":results,"met":met,"project_revision":revision,"completion_input":null,"evidence":null,"unmapped":unmapped});
    let complete_selection = required_ids
        .iter()
        .all(|id| results.iter().any(|r| r.observation.example == *id));
    let source = results
        .first()
        .and_then(|r| r.observation.source_sha.as_deref());
    let source_consistent = source.is_some()
        && results
            .iter()
            .all(|r| r.observation.source_sha.as_deref() == source);
    if request.phase == CheckPhase::Acceptance
        && met
        && complete_selection
        && !required_ids.is_empty()
        && source_consistent
    {
        generate_completion(
            store, &root, project.id, &work, request, &results, &mut value,
        )?;
    }
    let next = if let Some(failed) = results
        .iter()
        .find(|r| !request.phase.met(r.observation.outcome))
    {
        if let Some(survivors) = &failed.observation.survivors
            && !survivors.is_empty()
        {
            format!(
                "Add an example or record a finding for {}; inspect {}",
                survivors
                    .iter()
                    .map(|m| format!("{} ({}:{})", m.id, m.file, m.line))
                    .collect::<Vec<_>>()
                    .join(", "),
                failed.observation.log_dir
            )
        } else {
            format!(
                "Inspect {} and rerun example {}",
                failed.observation.log_dir, failed.observation.example
            )
        }
    } else if request.phase == CheckPhase::Baseline {
        "Request admission: awr work admit --input <file>".into()
    } else if !value["completion_input"].is_null() {
        if value["unmapped"].as_array().is_some_and(|a| !a.is_empty()) {
            "Register evidence for unmapped criteria and merge it into the completion input".into()
        } else {
            format!(
                "awr work complete {} --session {} --reason <reason> --input <file> --expected-revision {}",
                request.work, request.session, value["project_revision"]
            )
        }
    } else if !complete_selection {
        "Run all required examples together to generate completion evidence".into()
    } else if !source_consistent {
        "Record a Git commit and rerun acceptance to bind completion evidence".into()
    } else {
        "Review the recorded check results".into()
    };
    value["next_action"] = json!(next);
    Ok(value)
}

// Completion generation reuses managed artifact import and the existing report schema.
fn generate_completion(
    store: &mut Store,
    root: &Path,
    project: Id,
    work: &WorkItem,
    request: &RunCheckRequest,
    results: &[CheckResult],
    value: &mut Value,
) -> Result<()> {
    let source_sha = results[0].observation.source_sha.clone().unwrap();
    let command = format!(
        "awr work check {} --phase acceptance",
        work.meta.external_key
    );
    let verified_at = now_millis()?;
    let scope = vec![work.meta.external_key.clone()];
    let report = CompletionReport {
        version: 1,
        work_item: work.meta.external_key.clone(),
        source_sha: source_sha.clone(),
        command: command.clone(),
        scope: scope.clone(),
        verified_at,
        checks: results
            .iter()
            .map(|r| {
                let criterion = work
                    .definition
                    .as_ref()
                    .unwrap()
                    .examples
                    .iter()
                    .find(|e| e.id.as_deref() == Some(&r.observation.example))
                    .unwrap()
                    .criterion
                    .clone();
                CompletionCheck {
                    name: r.observation.example.clone(),
                    passed: true,
                    details: format!(
                        "work.check event {}; exit {}",
                        r.event_id,
                        r.observation.exit_code.unwrap_or_default()
                    ),
                    criteria: vec![criterion],
                }
            })
            .collect(),
    };
    let path = root.join(format!(".awr/check-report-{}.json", Id::new()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    file.write_all(&serde_json::to_vec_pretty(&report)?)?;
    file.sync_all()?;
    drop(file);
    let producer = results.last().unwrap().event_id;
    let imported = Runtime::attach(store, project)?.import_artifact(
        value["project_revision"].as_u64().unwrap(),
        ArtifactFile {
            path: path.clone(),
            artifact_type: "completion_report".into(),
            mime: "application/json".into(),
            source_event_id: producer,
            max_bytes: crate::ARTIFACT_IMPORT_CAP,
        },
    );
    let _ = fs::remove_file(path);
    let (artifact, event) = imported?;
    let key = format!("check-{}", artifact.id);
    let branch = store.session(project, request.session)?.branch_id;
    let (evidence, event) = store.record_check_evidence(
        project,
        event.project_revision,
        EvidenceDraft {
            external_key: key.clone(),
            work_item_key: Some(work.meta.external_key.clone()),
            evidence_type: "check_run".into(),
            level: EvidenceLevel::LocallyVerified,
            summary: "Direct acceptance checks passed".into(),
            locator: artifact.locator,
            sha256: Some(artifact.sha256),
            source_sha: Some(source_sha.clone()),
            command: Some(command),
            scope,
            branch_id: branch,
            verified_at: Some(verified_at),
        },
        &results.iter().map(|r| r.event_id).collect::<Vec<_>>(),
    )?;
    let mapped: BTreeSet<_> = report
        .checks
        .iter()
        .flat_map(|c| c.criteria.iter().map(String::as_str))
        .collect();
    let unmapped: Vec<_> = work
        .acceptance
        .iter()
        .filter(|c| !mapped.contains(c.as_str()))
        .collect();
    let input = CompletionInput {
        version: 1,
        source_sha,
        minimum_level: EvidenceLevel::LocallyVerified,
        acceptance: work
            .acceptance
            .iter()
            .map(|c| AcceptanceEvidenceRequest {
                criterion: c.clone(),
                evidence: if mapped.contains(c.as_str()) {
                    vec![key.clone()]
                } else {
                    vec![]
                },
            })
            .collect(),
        required_evidence: vec![],
        check_waiver: None,
    };
    value["completion_input"] = serde_json::to_value(input)?;
    value["unmapped"] = json!(unmapped);
    value["evidence"] = serde_json::to_value(&evidence)?;
    value["evidence"]["trust"] = serde_json::to_value(store.evidence_trust(project, evidence.id)?)?;
    value["project_revision"] = json!(event.project_revision);
    Ok(())
}

/// Read recorded facts without source refresh, writes, subprocesses or file hashing.
pub fn check_status(store: &Store, root: &Path, key: &str) -> Result<Value> {
    let project = store.project_by_root(&root.canonicalize()?)?;
    let work = store.work_item(project.id, key)?.item;
    let report = store.work_definition_report(project.id, key)?;
    let events = store.check_events(project.id, key, project.current_branch_id)?;
    let examples:Vec<_>=work.definition.as_ref().map(|d|d.examples.iter().map(|e| {
        let tier=report.criterion_tiers.iter().find(|(c,_)|c==&e.criterion).map(|(_,t)|t);
        let runs:Vec<_>=events.iter().filter(|r|e.id.as_deref().is_some_and(|id|r.payload["example"]==id)).map(|r|json!({"event_id":r.id,"phase":r.payload["phase"],"outcome":r.payload["outcome"],"reason":r.payload["reason"],"example_oracle":r.payload["example_oracle"],"log_dir":r.payload["log_dir"]})).collect();
        json!({"id":e.id,"criterion":e.criterion,"kind":e.kind,"tier":tier,"has_check":e.check.is_some(),"runs":runs})
    }).collect()).unwrap_or_default();
    Ok(
        json!({"ok":true,"work":key,"examples":examples,"project_revision":project.project_revision,"read_only":true,"next_action":"Review recorded checks or run a selected phase"}),
    )
}
