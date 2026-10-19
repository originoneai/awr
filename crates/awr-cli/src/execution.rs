use crate::{query::QueryProject, session::RuntimeProject};
use awr_core::*;
use awr_store::Store;
use clap::Subcommand;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Debug, Subcommand)]
pub enum ExecutionCommand {
    /// Persist intent, then dispatch an independently supervised local command at most once per key.
    Run {
        #[arg(long)]
        session: Id,
        #[arg(long)]
        key: String,
        #[arg(long)]
        purpose: String,
        /// Bind an input file to the receipt by hashing it before the command starts.
        #[arg(long)]
        bind: Vec<PathBuf>,
        /// Fail registration instead of skipping an input that cannot be hashed.
        #[arg(long)]
        require_input_hash: bool,
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Register an external execution reference; its outcome remains unverified.
    Register {
        #[arg(long)]
        session: Id,
        #[arg(long)]
        key: String,
        #[arg(long)]
        purpose: String,
        #[arg(long)]
        reference: String,
    },
    /// Record a bounded ExternalExecutionReport JSON file; never changes supervisor facts.
    Report {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        expected_revision: Revision,
    },
    /// Look up the immutable report receipt before retrying after a lost response.
    ReportStatus {
        #[arg(long)]
        key: String,
    },
    List {
        #[arg(long)]
        work: Option<String>,
    },
    Show {
        id: Id,
    },
    /// Verify a managed supervisor/result without restarting or killing any command.
    Inspect {
        id: Id,
    },
    #[command(hide = true)]
    Worker {
        id: Id,
    },
}

fn retry<T>(
    store: &mut Store,
    project: Id,
    mut action: impl FnMut(&mut Store, Revision) -> Result<T>,
) -> Result<T> {
    for _ in 0..16 {
        let rev = store.project(project)?.project_revision;
        match action(store, rev) {
            Err(Error::RevisionConflict { .. }) => continue,
            other => return other,
        }
    }
    Err(Error::Storage(
        "project remained busy while recording execution".into(),
    ))
}

/// Ceiling for hashing one declared input; the same cap guards the drift report.
const INPUT_HASH_CAP: u64 = 256 * 1024 * 1024;

fn is_interpreter(program: &str) -> bool {
    let name = program.trim_end_matches(".exe");
    matches!(
        name,
        "node" | "bash" | "sh" | "zsh" | "pwsh" | "powershell" | "ruby" | "perl"
    ) || name == "python"
        || name.starts_with("python3")
}

/// The script an interpreter command names, when argv makes it determinable.
fn script_from_argv(command: &[String]) -> Option<String> {
    let program = command.first()?;
    let name = Path::new(program)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    if !is_interpreter(&name) {
        return None;
    }
    for argument in &command[1..] {
        if matches!(argument.as_str(), "-m" | "-c" | "-e") {
            return None;
        }
        if argument.starts_with('-') {
            continue;
        }
        return Some(argument.clone());
    }
    None
}

fn hash_file(path: &Path) -> Result<(String, u64, i64)> {
    let meta = fs::metadata(path).map_err(|_| {
        Error::SourceUnavailable(format!(
            "declared execution input cannot be read: {}",
            path.display()
        ))
    })?;
    if !meta.is_file() {
        return Err(Error::InvalidInput(format!(
            "execution input is not a regular file: {}",
            path.display()
        )));
    }
    if meta.len() > INPUT_HASH_CAP {
        return Err(Error::InvalidInput(format!(
            "execution input exceeds {INPUT_HASH_CAP} byte hashing limit: {}",
            path.display()
        )));
    }
    let mut file = fs::File::open(path).map_err(|_| {
        Error::SourceUnavailable(format!(
            "declared execution input cannot be read: {}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > INPUT_HASH_CAP {
            return Err(Error::InvalidInput(format!(
                "execution input exceeds {INPUT_HASH_CAP} byte hashing limit: {}",
                path.display()
            )));
        }
        hasher.update(&buffer[..read]);
    }
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos() as i64)
        .unwrap_or(0);
    Ok((format!("{:x}", hasher.finalize()), size, mtime_ns))
}

fn bound_inputs(
    root: &Path,
    bind: &[PathBuf],
    command: &[String],
    require: bool,
) -> Result<(Vec<ExecutionInput>, InputBinding)> {
    let mut inputs = Vec::new();
    for path in bind {
        let resolved = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        match hash_file(&resolved) {
            Ok((sha256, size, mtime_ns)) => inputs.push(ExecutionInput {
                path: path.to_string_lossy().into_owned(),
                sha256,
                size,
                mtime_ns,
                source: InputSource::Bind,
            }),
            Err(error) if require => return Err(error),
            Err(_) => {}
        }
    }
    if let Some(script) = script_from_argv(command) {
        let resolved = if Path::new(&script).is_absolute() {
            PathBuf::from(&script)
        } else {
            root.join(&script)
        };
        match hash_file(&resolved) {
            Ok((sha256, size, mtime_ns)) => inputs.push(ExecutionInput {
                path: script,
                sha256,
                size,
                mtime_ns,
                source: InputSource::ArgvScript,
            }),
            Err(error) if require => return Err(error),
            Err(_) => {}
        }
    }
    let binding = if inputs.is_empty() {
        InputBinding::None
    } else if bind.is_empty() {
        InputBinding::ArgvScript
    } else {
        InputBinding::Declared
    };
    Ok((inputs, binding))
}

fn input_drift(root: &Path, inputs: &[ExecutionInput]) -> Value {
    let mut files = Vec::new();
    let mut any = false;
    for input in inputs {
        let resolved = if Path::new(&input.path).is_absolute() {
            PathBuf::from(&input.path)
        } else {
            root.join(&input.path)
        };
        let (state, sha256_now) = match hash_file(&resolved) {
            Ok((sha256, _, _)) if sha256 == input.sha256 => ("unchanged", Some(sha256)),
            Ok((sha256, _, _)) => ("changed", Some(sha256)),
            Err(Error::InvalidInput(_)) => ("unreadable", None),
            Err(_) => ("missing", None),
        };
        if state != "unchanged" {
            any = true;
        }
        files.push(json!({
            "path": input.path,
            "sha256_at_run": input.sha256,
            "sha256_now": sha256_now,
            "state": state,
        }));
    }
    json!({"any": any, "files": files})
}

fn register(root: &Path, session: Id, intent: ExecutionIntent) -> Result<(Execution, bool)> {
    let _lock = crate::client::lock(root, "executions", &format!("key:{}", intent.operation_key))?;
    protect_runtime(root)?;
    let mut db = QueryProject::open(root)?;
    db.finish()?;
    let bound = db.store.session(db.project.id, session)?;
    if let Some(existing) = db
        .store
        .execution_by_key(db.project.id, &intent.operation_key)?
    {
        if existing.intent != intent
            || Some(existing.work_item_id) != bound.work_item_id
            || existing.branch_id != bound.branch_id
        {
            // A retry after `touch` still matches, because intent equality ignores
            // mtime. Reaching here with everything but the inputs equal means the
            // bytes changed between the two attempts, which is the one conflict
            // this feature exists to surface; name it precisely.
            let mut previous = existing.intent.clone();
            let mut requested = intent.clone();
            previous.inputs.clear();
            previous.input_binding = InputBinding::None;
            requested.inputs.clear();
            requested.input_binding = InputBinding::None;
            if previous == requested
                && Some(existing.work_item_id) == bound.work_item_id
                && existing.branch_id == bound.branch_id
            {
                return Err(Error::SourceConflict("operation key was used with different input bytes; use a new key for a new operation".into()));
            }
            return Err(Error::SourceConflict("operation key belongs to different work, branch or intent; use a new key for a new operation".into()));
        }
        return Ok((existing, false));
    }
    let project = db.project.id;
    let (e, _) = retry(&mut db.store, project, |s, r| {
        s.register_execution(project, r, session, intent.clone())
    })?;
    Ok((e, true))
}

fn protect_runtime(root: &Path) -> Result<()> {
    // Also protects projects initialized with older versions, without editing their root ignore file.
    let dir = awr_source::open_dir_exact(&root.canonicalize()?.join(".awr/executions"))?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    match dir.open_with(".gitignore", &options) {
        Ok(mut f) => {
            f.write_all(b"*\n")?;
            f.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            return Err(Error::Storage(format!(
                "creating execution runtime ignore: {e}"
            )));
        }
    }
    Ok(())
}

pub fn run(root: &Path, command: &ExecutionCommand, _json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let result = match command {
        ExecutionCommand::Report {
            input,
            expected_revision,
        } => {
            let bytes = awr_source::read_capped(&root.join(input), 1024 * 1024)?;
            let report: ExternalExecutionReport = serde_json::from_slice(&bytes).map_err(|_| {
                Error::InvalidInput("invalid external execution report JSON".into())
            })?;
            // External observations must remain recordable when source parsing fails.
            // This opens existing runtime state without refreshing or mutating source files.
            let mut db = RuntimeProject::open(&root, false)?;
            let (event, created) =
                db.store
                    .report_external_execution(db.project.id, *expected_revision, report)?;
            json!({"ok":true,"project_revision":db.store.project(db.project.id)?.project_revision,"event":event,"created":created,"receipt_recorded":true,"validation_basis":"host_supplied_report","outcome_verified":false,"source_write_performed":false,"source_refresh_performed":false})
        }
        ExecutionCommand::ReportStatus { key } => {
            let (store, project) = read_state(&root)?;
            let event = store.external_report_by_key(project.id, key)?;
            json!({"ok":true,"project_revision":project.project_revision,"found":event.is_some(),"event":event,"validation_basis":"host_supplied_report","outcome_verified":false,"side_effects_performed":false})
        }
        ExecutionCommand::Run {
            session,
            key,
            purpose,
            bind,
            require_input_hash,
            command,
        } => {
            let (inputs, input_binding) = bound_inputs(&root, bind, command, *require_input_hash)?;
            let (e, created) = register(
                &root,
                *session,
                ExecutionIntent {
                    operation_key: key.clone(),
                    purpose: purpose.clone(),
                    executor: ExecutorKind::ManagedLocal,
                    command: command.clone(),
                    cwd: root.to_string_lossy().into(),
                    external_reference: None,
                    inputs,
                    input_binding,
                },
            )?;
            let mut dispatch_error = None;
            if created {
                let mut worker = Command::new(std::env::current_exe()?);
                worker
                    .args([
                        "--project",
                        root.to_str()
                            .ok_or_else(|| Error::InvalidInput("non UTF-8 project path".into()))?,
                        "execution",
                        "worker",
                        &e.id.to_string(),
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if let Err(error) = detach(&mut worker).and_then(|_| worker.spawn()) {
                    dispatch_error = Some(format!(
                        "supervisor spawn failed ({:?}); intent remains registered, no automatic retry",
                        error.kind()
                    ));
                }
            }
            json!({"execution":e,"created":created,"dispatch_requested":created&&dispatch_error.is_none(),"dispatch_error":dispatch_error,"note":"Use execution show to read the recorded outcome. Repeating a key never starts a second command."})
        }
        ExecutionCommand::Register {
            session,
            key,
            purpose,
            reference,
        } => {
            let (e, created) = register(
                &root,
                *session,
                ExecutionIntent {
                    operation_key: key.clone(),
                    purpose: purpose.clone(),
                    executor: ExecutorKind::External,
                    command: vec![],
                    cwd: root.to_string_lossy().into(),
                    external_reference: Some(reference.clone()),
                    inputs: Vec::new(),
                    input_binding: InputBinding::None,
                },
            )?;
            json!({"execution":e,"created":created,"outcome_verified":false})
        }
        ExecutionCommand::List { work } => {
            let db = RuntimeProject::open(&root, false)?;
            let work = work
                .as_ref()
                .map(|key| {
                    db.store
                        .work_item(db.project.id, key)
                        .map(|w| w.item.meta.id)
                })
                .transpose()?;
            json!({"executions":db.store.executions(db.project.id,work)?,"live_verification_performed":false})
        }
        ExecutionCommand::Inspect { id } => {
            let (store, project) = read_state(&root)?;
            let e = store.execution(project.id, *id)?;
            json!({"observation":awr_runtime::inspect_execution(&root,&e)?,"external_report":store.latest_external_report(project.id,*id)?,"input_binding":e.intent.input_binding,"inputs":e.intent.inputs,"input_drift":input_drift(&root,&e.intent.inputs),"side_effects_performed":false})
        }
        ExecutionCommand::Show { id } => {
            let db = RuntimeProject::open(&root, false)?;
            json!({"execution":db.store.execution(db.project.id,*id)?,"external_report":db.store.latest_external_report(db.project.id,*id)?,"live_verification_performed":false})
        }
        ExecutionCommand::Worker { id } => return supervise(&root, *id),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub(crate) fn read_state(root: &Path) -> Result<(Store, Project)> {
    let root = root.canonicalize()?;
    let store = Store::open_readonly(&crate::source::runtime_dir(&root, false)?.join("state.db"))?;
    let project = store.project_by_root(&root)?;
    Ok((store, project))
}

fn detach(command: &mut Command) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::{
            Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
            Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType},
        };
        // Rust 1.93 CreateProcess inherits every inheritable handle, even when the new
        // standard streams are NUL. Do not let the supervisor retain our caller's pipe
        // endpoints: output() would otherwise wait for the entire background command.
        // These borrowed handles remain owned and open in this short-lived CLI process;
        // changing their inherit bit neither closes them nor affects the caller's copy.
        for handle in [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ] {
            // SAFETY: std supplies live borrowed standard handles. GetFileType accepts
            // absent/invalid handles; SetHandleInformation is called only for valid pipes.
            unsafe {
                if GetFileType(handle) == FILE_TYPE_PIPE
                    && SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
        }
        command.creation_flags(0x00000200 | 0x08000000);
    }
    Ok(())
}

fn new_log(dir: &cap_std::fs::Dir, name: &str) -> Result<fs::File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    Ok(dir.open_with(name, &options)?.into_std())
}

fn supervise(root: &Path, id: Id) -> Result<()> {
    let _lock = crate::client::lock(root, "executions", &format!("worker:{id}"))?;
    let mut db = RuntimeProject::open(root, false)?;
    let project = db.project.id;
    let e = db.store.execution(project, id)?;
    if e.state != ExecutionState::Registered || e.intent.executor != ExecutorKind::ManagedLocal {
        return Ok(());
    }
    if Path::new(&e.intent.cwd) != root {
        return Err(Error::SourceConflict(
            "execution root changed since registration".into(),
        ));
    }
    let parent = awr_source::open_dir_exact(&root.join(".awr/executions"))?;
    parent.create_dir(id.to_string())?;
    let dir = parent.open_dir(id.to_string())?;
    let stdout = new_log(&dir, "stdout.log")?;
    let stderr = new_log(&dir, "stderr.log")?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let identity = WorkerIdentity {
        nonce: Id::new(),
        pid: std::process::id(),
        port: listener.local_addr()?.port(),
        child_pid: None,
    };
    retry(&mut db.store, project, |s, r| {
        s.start_execution(project, r, id, identity.clone())
    })?;
    let mut command = Command::new(&e.intent.command[0]);
    command
        .args(&e.intent.command[1..])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return finish(
                &mut db.store,
                project,
                &dir,
                ExecutionResult {
                    execution_id: id,
                    nonce: identity.nonce,
                    finished_at: now_millis()?,
                    success: false,
                    exit_code: None,
                    signal: None,
                    error: Some(format!("command spawn failed ({:?})", error.kind())),
                },
            );
        }
    };
    let child_pid = child.id();
    // A failed running-event write must not abandon an already-started child. The durable
    // starting identity plus the supervisor and final file still provide recovery evidence.
    let _ = retry(&mut db.store, project, |s, r| {
        s.execution_running(project, r, id, identity.nonce, child_pid)
    });
    loop {
        if let Some(status) = child.try_wait()? {
            #[cfg(unix)]
            let signal = {
                use std::os::unix::process::ExitStatusExt;
                status.signal()
            };
            #[cfg(not(unix))]
            let signal = None;
            return finish(
                &mut db.store,
                project,
                &dir,
                ExecutionResult {
                    execution_id: id,
                    nonce: identity.nonce,
                    finished_at: now_millis()?,
                    success: status.success(),
                    exit_code: status.code(),
                    signal,
                    error: None,
                },
            );
        }
        // One bounded request per iteration, so clients cannot starve result collection.
        if let Ok((mut socket, address)) = listener.accept() {
            if address.ip().is_loopback() {
                socket.set_read_timeout(Some(Duration::from_millis(150)))?;
                socket.set_write_timeout(Some(Duration::from_millis(150)))?;
                let mut bytes = Vec::new();
                // The caller half-closes its write side; no unbounded line allocation.
                if Read::by_ref(&mut socket)
                    .take(2048)
                    .read_to_end(&mut bytes)
                    .is_ok()
                {
                    if let Ok(probe) = serde_json::from_slice::<ExecutionProbe>(&bytes) {
                        if probe.execution_id == id
                            && probe.nonce == identity.nonce
                            && child.try_wait()?.is_none()
                        {
                            let reply = ExecutionProbeReply {
                                execution_id: id,
                                nonce: identity.nonce,
                                worker_pid: identity.pid,
                                child_pid,
                                observed_at: now_millis()?,
                            };
                            let _ = socket.write_all(&serde_json::to_vec(&reply)?);
                            // Close the write side so the probe sees EOF instead of
                            // waiting out its read timeout after a complete reply.
                            let _ = socket.shutdown(std::net::Shutdown::Write);
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn finish(
    store: &mut Store,
    project: Id,
    dir: &cap_std::fs::Dir,
    result: ExecutionResult,
) -> Result<()> {
    let mut receipt = new_log(dir, "result.pending")?;
    receipt.write_all(&serde_json::to_vec(&result)?)?;
    receipt.sync_all()?;
    dir.rename("result.pending", dir, "result.json")?;
    retry(store, project, |s, r| {
        s.finish_execution(project, r, result.clone())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::script_from_argv;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn interpreter_scripts_are_detected_from_argv() {
        assert_eq!(
            script_from_argv(&args(&["python", "x.py"])),
            Some("x.py".into())
        );
        assert_eq!(
            script_from_argv(&args(&["python3.11", "x.py"])),
            Some("x.py".into())
        );
        assert_eq!(script_from_argv(&args(&["python", "-m", "mod"])), None);
        assert_eq!(
            script_from_argv(&args(&["node", "-e", "console.log(1)"])),
            None
        );
        assert_eq!(
            script_from_argv(&args(&["bash", "-x", "y.sh"])),
            Some("y.sh".into())
        );
        assert_eq!(
            script_from_argv(&args(&["pwsh", "-File", "z.ps1"])),
            Some("z.ps1".into())
        );
        assert_eq!(
            script_from_argv(&args(&["ruby", "r.rb"])),
            Some("r.rb".into())
        );
        assert_eq!(script_from_argv(&args(&["make", "all"])), None);
        assert_eq!(script_from_argv(&[]), None);
    }
}
