use awr_source::fingerprint;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr execution {}", awr_core::Id::new())));
        fs::create_dir(&p.0).unwrap();
        p.ok(&["init", "--goal", "Export a reviewed report", "--accept"]);
        p
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .env("AWR_EXECUTION_TEST_ROOT", &self.0)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        decode(self.run(args))
    }
    fn hook(&self, external: &str, event: &str) -> Value {
        let mut child = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args([
                "--project",
                self.0.to_str().unwrap(),
                "--json",
                "client",
                "hook",
                "--client",
                "generic",
                "--work",
                "INTAKE-001",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&serde_json::to_vec(&serde_json::json!({"session_id":external,"hook_event_name":event,"cwd":self.0,"turn_id":"work-progress"})).unwrap()).unwrap();
        decode(child.wait_with_output().unwrap())
    }
    fn bind(&self, name: &str) -> String {
        self.ok(&[
            "client",
            "bind",
            "--client",
            "generic",
            "--external-session",
            name,
            "--work",
            "INTAKE-001",
        ])["binding"]["session_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn launch(&self, session: &str, key: &str, helper: &str) -> Value {
        self.ok(&[
            "execution",
            "run",
            "--session",
            session,
            "--key",
            key,
            "--purpose",
            "Build the report",
            "--",
            std::env::current_exe().unwrap().to_str().unwrap(),
            "--ignored",
            "--exact",
            helper,
            "--nocapture",
        ])
    }
    fn wait(&self, id: &str, states: &[&str]) -> Value {
        let start = Instant::now();
        loop {
            let e = self.ok(&["execution", "show", id])["execution"].clone();
            if states.contains(&e["state"].as_str().unwrap()) {
                return e;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "execution timed out: {e}"
            );
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        // Release an owned controlled fixture even if its parent's assertion failed.
        if self.0.join("child-waiting").exists() {
            let _ = fs::write(self.0.join("release-child"), "release");
            let start = Instant::now();
            while !self.0.join("controlled-child-finished").exists()
                && start.elapsed() < Duration::from_secs(3)
            {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn decode(o: Output) -> Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
#[ignore = "owned child process fixture, launched explicitly by integration checks"]
fn successful_child() {
    let root = PathBuf::from(std::env::var_os("AWR_EXECUTION_TEST_ROOT").unwrap());
    let mut count = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("launches.txt"))
        .unwrap();
    writeln!(count, "one launch").unwrap();
    println!("report started");
    std::thread::sleep(Duration::from_millis(1500));
    fs::write(root.join("report.txt"), "Reviewed report artifact").unwrap();
    println!("report complete");
}
#[test]
#[ignore = "owned child process fixture, launched explicitly by integration checks"]
fn failed_child() {
    eprintln!("report input rejected");
    std::process::exit(7);
}

#[test]
#[ignore = "owned child process fixture for recovery while work remains running"]
fn slow_child() {
    let root = PathBuf::from(std::env::var_os("AWR_EXECUTION_TEST_ROOT").unwrap());
    fs::write(root.join("child-waiting"), "waiting for the test parent").unwrap();
    let start = Instant::now();
    while !root.join("release-child").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "launcher did not return while its managed command was running"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    successful_child();
    fs::write(root.join("controlled-child-finished"), "finished").unwrap();
}

#[test]
fn resume_and_client_context_include_live_work_and_completed_results() {
    let p = Project::new();
    let s = p.bind("recovery-source");
    let live = p.launch(&s, "live-report", "slow_child");
    let live_id = live["execution"]["id"].as_str().unwrap();
    p.wait(live_id, &["running"]);
    let failed = p.launch(&s, "failed-report", "failed_child");
    let failed_id = failed["execution"]["id"].as_str().unwrap();
    p.wait(failed_id, &["failed"]);
    p.ok(&[
        "client",
        "progress",
        "--client",
        "generic",
        "--external-session",
        "recovery-source",
        "--next-action",
        "Collect the report then review the failed import",
        "--open-loop",
        "Independent report review remains",
    ]);
    let cp = p.hook("recovery-source", "Stop");
    assert_eq!(cp["awr"]["checkpoint_saved"], true);
    let revision = p.ok(&["status"])["project_revision"].to_string();
    let inspection = p.ok(&["recovery", "inspect", "--session", &s]);
    assert_eq!(inspection["side_effects_performed"], false);
    assert_eq!(p.ok(&["status"])["project_revision"].to_string(), revision);
    let observations = inspection["executions"].as_array().unwrap();
    assert!(
        observations.iter().any(|o| o["execution_id"] == live_id
            && o["state"] == "running"
            && o["verified"] == true)
    );
    assert!(
        observations.iter().any(|o| o["execution_id"] == failed_id
            && o["state"] == "failed"
            && o["exit_code"] == 7)
    );
    let resumed = p.ok(&[
        "session",
        "resume",
        "--from-session",
        &s,
        "--agent",
        "successor",
        "--provider",
        "generic",
        "--model",
        "fixture",
        "--no-claim",
        "--expected-revision",
        &revision,
        "--budget",
        "12000",
    ]);
    assert_eq!(resumed["context_ready"], true);
    assert!(
        resumed["executions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["execution_id"] == live_id && o["state"] == "running")
    );
    let context = resumed["context"].to_string();
    assert!(context.contains(live_id));
    assert!(context.contains(failed_id));
    assert!(context.contains("Independent report review remains"));
    let successor = resumed["resumed"]["session"]["id"].as_str().unwrap();
    let bound = p.ok(&[
        "client",
        "bind",
        "--client",
        "generic",
        "--external-session",
        "recovery-successor",
        "--work",
        "INTAKE-001",
        "--session",
        successor,
    ]);
    assert_eq!(
        bound["context"]["context"]["executions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let native = p.hook("recovery-successor", "SessionStart");
    let text = native["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains("owned_supervisor_identity_and_live_child_probe"));
    assert!(text.contains(failed_id));
    fs::write(p.0.join("release-child"), "release").unwrap();
    p.wait(live_id, &["succeeded"]);
    let completed = p.ok(&["execution", "inspect", live_id]);
    assert_eq!(completed["observation"]["state"], "succeeded");
    assert_eq!(completed["observation"]["verified"], true);
    assert_eq!(completed["observation"]["exit_code"], 0);
}

#[test]
fn a_result_receipt_survives_a_failed_final_journal_write_and_rejects_wrong_identity() {
    let p = Project::new();
    let s = p.bind("receipt-recovery");
    let launched = p.launch(&s, "receipt-gap", "slow_child");
    let id = launched["execution"]["id"].as_str().unwrap();
    let running = p.wait(id, &["running"]);
    let conn = rusqlite::Connection::open(p.0.join(".awr/state.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fixture_fail_finish BEFORE INSERT ON events WHEN NEW.event_type='execution.finished' BEGIN SELECT RAISE(ABORT,'injected final journal write failure'); END;").unwrap();
    drop(conn);
    fs::write(p.0.join("release-child"), "release").unwrap();
    let path = p.0.join(running["receipt"].as_str().unwrap());
    let start = Instant::now();
    while !path.is_file() {
        assert!(start.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        p.ok(&["execution", "show", id])["execution"]["state"],
        "running"
    );
    let observed = p.ok(&["execution", "inspect", id]);
    assert_eq!(observed["observation"]["state"], "succeeded");
    assert_eq!(
        observed["observation"]["basis"],
        "supervisor_result_receipt"
    );
    let mut receipt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    receipt["nonce"] = serde_json::json!(awr_core::Id::new());
    fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    let bad = p.ok(&["execution", "inspect", id]);
    assert_eq!(bad["observation"]["state"], "unknown");
    assert_eq!(bad["observation"]["verified"], false);
    assert_eq!(
        bad["observation"]["basis"],
        "invalid_or_unreadable_completion_receipt"
    );
}

#[cfg(unix)]
#[test]
fn missing_supervisor_is_unknown_and_inspection_neither_restarts_nor_kills_child() {
    let p = Project::new();
    let s = p.bind("interrupted-supervisor");
    let launched = p.launch(&s, "interrupted-report", "slow_child");
    let id = launched["execution"]["id"].as_str().unwrap();
    let running = p.wait(id, &["running"]);
    assert_eq!(
        p.ok(&["execution", "inspect", id])["observation"]["state"],
        "running"
    );
    // Terminate only the fixture supervisor whose execution identity was just verified.
    let killed = Command::new("kill")
        .args(["-KILL", &running["worker"]["pid"].to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let begin = Instant::now();
    let unknown = loop {
        let o = p.ok(&["execution", "inspect", id]);
        if o["observation"]["state"] == "unknown" {
            break o;
        }
        assert!(begin.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(unknown["observation"]["verified"], false);
    assert_eq!(
        unknown["observation"]["basis"],
        "supervisor_unreachable_without_completion_receipt"
    );
    let duplicate = p.launch(&s, "interrupted-report", "slow_child");
    assert_eq!(duplicate["created"], false);
    fs::write(p.0.join("release-child"), "release").unwrap();
    let begin = Instant::now();
    while !p.0.join("report.txt").exists() {
        assert!(begin.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        fs::read_to_string(p.0.join("launches.txt"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    // The artifact's existence alone is not a supervisor completion receipt.
    assert_eq!(
        p.ok(&["execution", "inspect", id])["observation"]["state"],
        "unknown"
    );
}

#[test]
fn managed_command_outlives_calling_session_and_keeps_result_logs() {
    let p = Project::new();
    let s = p.bind("first");
    let launched = p.launch(&s, "report-one", "slow_child");
    let id = launched["execution"]["id"].as_str().unwrap();
    assert_eq!(launched["execution"]["state"], "registered");
    assert_eq!(launched["created"], true);
    p.wait(id, &["running"]);
    let rev = p.ok(&["status"])["project_revision"].to_string();
    p.ok(&[
        "session",
        "end",
        "--session",
        &s,
        "--expected-revision",
        &rev,
    ]);
    fs::write(p.0.join("release-child"), "release").unwrap();
    let finished = p.wait(id, &["succeeded", "failed"]);
    assert_eq!(finished["state"], "succeeded");
    assert_eq!(finished["exit_code"], 0);
    assert!(p.0.join("report.txt").is_file());
    assert!(
        fs::read_to_string(p.0.join(finished["stdout"].as_str().unwrap()))
            .unwrap()
            .contains("report complete")
    );
    let receipt: Value =
        serde_json::from_slice(&fs::read(p.0.join(finished["receipt"].as_str().unwrap())).unwrap())
            .unwrap();
    assert_eq!(receipt["execution_id"], id);
    assert_eq!(receipt["success"], true);
    let next = p.bind("next");
    let duplicate = p.launch(&next, "report-one", "slow_child");
    assert_eq!(duplicate["created"], false);
    assert_eq!(duplicate["execution"]["id"], id);
    assert_eq!(
        fs::read_to_string(p.0.join("launches.txt"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        p.ok(&["execution", "list", "--work", "INTAKE-001"])["executions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn concurrent_registration_dispatches_only_one_child_and_rejects_changed_intent() {
    let p = Arc::new(Project::new());
    let s = p.bind("parallel");
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let p = p.clone();
            let s = s.clone();
            std::thread::spawn(move || p.launch(&s, "same-operation", "successful_child"))
        })
        .collect();
    let values: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(values.iter().filter(|v| v["created"] == true).count(), 1);
    assert!(
        values
            .iter()
            .all(|v| v["execution"]["id"] == values[0]["execution"]["id"])
    );
    p.wait(
        values[0]["execution"]["id"].as_str().unwrap(),
        &["succeeded"],
    );
    assert_eq!(
        fs::read_to_string(p.0.join("launches.txt"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let collision = p.run(&[
        "execution",
        "run",
        "--session",
        &s,
        "--key",
        "same-operation",
        "--purpose",
        "Different intent",
        "--",
        "different-command",
    ]);
    assert!(!collision.status.success());
    assert!(String::from_utf8_lossy(&collision.stderr).contains("SourceConflict"));
}
#[test]
fn failures_and_spawn_errors_have_durable_terminal_receipts() {
    let p = Project::new();
    let s = p.bind("failure");
    let e = p.launch(&s, "failed-report", "failed_child");
    let failed = p.wait(e["execution"]["id"].as_str().unwrap(), &["failed"]);
    assert_eq!(failed["exit_code"], 7);
    assert!(
        fs::read_to_string(p.0.join(failed["stderr"].as_str().unwrap()))
            .unwrap()
            .contains("input rejected")
    );
    let e = p.ok(&[
        "execution",
        "run",
        "--session",
        &s,
        "--key",
        "missing-tool",
        "--purpose",
        "Build report",
        "--",
        "awr-fixture-command-that-does-not-exist",
    ]);
    let failed = p.wait(e["execution"]["id"].as_str().unwrap(), &["failed"]);
    assert!(failed["exit_code"].is_null());
    assert!(failed["error"].as_str().unwrap().contains("spawn failed"));
}
#[test]
fn external_reference_is_not_a_completion_and_generic_events_cannot_forge_it() {
    let p = Project::new();
    let s = p.bind("external");
    let e = p.ok(&[
        "execution",
        "register",
        "--session",
        &s,
        "--key",
        "external-build",
        "--purpose",
        "Track build",
        "--reference",
        "ci://build/42",
    ]);
    assert_eq!(e["execution"]["state"], "registered");
    assert_eq!(e["outcome_verified"], false);
    assert!(e["execution"]["worker"].is_null());
    let rev = p.ok(&["status"])["project_revision"].to_string();
    let forged = p.run(&[
        "event",
        "append",
        "--type",
        "execution.finished",
        "--summary",
        "Pretend completed",
        "--expected-revision",
        &rev,
    ]);
    assert!(!forged.status.success());
    assert!(String::from_utf8_lossy(&forged.stderr).contains("reserved"));
    let inspected = p.ok(&[
        "execution",
        "inspect",
        e["execution"]["id"].as_str().unwrap(),
    ]);
    assert_eq!(inspected["observation"]["state"], "unknown");
    assert_eq!(
        inspected["observation"]["basis"],
        "external_reference_requires_executor_adapter"
    );
}

#[test]
fn execution_context_is_deterministic_and_never_silently_drops_registered_work() {
    let p = Project::new();
    let s = p.bind("context-budget");
    for i in 0..8 {
        p.ok(&[
            "execution",
            "register",
            "--session",
            &s,
            "--key",
            &format!("external-job-{i}"),
            "--purpose",
            "Track the independent report build",
            "--reference",
            &format!("ci://build/{i}"),
        ]);
    }
    let args = ["context", "compile", "--session", &s, "--budget", "12000"];
    let first = p.ok(&args);
    let inspected = p.ok(&["recovery", "inspect", "--session", &s]);
    assert_eq!(inspected["executions"].as_array().unwrap().len(), 8);
    let second = p.ok(&args);
    assert_eq!(
        first["work_context"]["context_hash"],
        second["work_context"]["context_hash"]
    );
    let text = first.to_string();
    for i in 0..8 {
        assert!(text.contains(&format!("external-job-{i}")));
    }
    let too_small = p.run(&["context", "compile", "--session", &s, "--budget", "500"]);
    assert!(!too_small.status.success());
    assert!(String::from_utf8_lossy(&too_small.stderr).contains("BudgetExceeded"));
}

fn helper_command() -> String {
    std::env::current_exe()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

fn managed_args<'a>(
    session: &'a str,
    key: &'a str,
    purpose: &'a str,
    extra: &[&'a str],
    exe: &'a str,
) -> Vec<&'a str> {
    let mut args = vec![
        "execution",
        "run",
        "--session",
        session,
        "--key",
        key,
        "--purpose",
        purpose,
    ];
    args.extend_from_slice(extra);
    args.push("--");
    args.push(exe);
    args.extend_from_slice(&["--ignored", "--exact", "failed_child", "--nocapture"]);
    args
}

fn run_managed(p: &Project, session: &str, key: &str, purpose: &str, extra: &[&str]) -> Value {
    let exe = helper_command();
    p.ok(&managed_args(session, key, purpose, extra, &exe))
}

fn run_managed_output(
    p: &Project,
    session: &str,
    key: &str,
    purpose: &str,
    extra: &[&str],
) -> Output {
    let exe = helper_command();
    p.run(&managed_args(session, key, purpose, extra, &exe))
}

#[test]
fn an_execution_receipt_binds_input_bytes_and_reports_drift() {
    let p = Project::new();
    let body = b"print('one')\n";
    fs::write(p.0.join("a.py"), body).unwrap();
    let session = p.bind("input-binding");
    let run = run_managed(&p, &session, "bind-1", "Bind inputs", &["--bind", "a.py"]);
    let execution = &run["execution"];
    assert_eq!(execution["intent"]["input_binding"], "declared");
    let inputs = execution["intent"]["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0]["path"], "a.py");
    assert_eq!(inputs[0]["source"], "bind");
    assert_eq!(inputs[0]["size"], body.len() as u64);
    let expected = format!("{:x}", Sha256::digest(body));
    assert_eq!(inputs[0]["sha256"], expected);
    assert_eq!(inputs[0]["sha256"].as_str().unwrap().len(), 64);
    let id = execution["id"].as_str().unwrap().to_owned();

    let first = p.ok(&["execution", "inspect", &id]);
    assert_eq!(first["input_binding"], "declared");
    assert_eq!(first["input_drift"]["any"], false);
    assert_eq!(first["input_drift"]["files"][0]["state"], "unchanged");

    fs::write(p.0.join("a.py"), b"print('two')\n").unwrap();
    let second = p.ok(&["execution", "inspect", &id]);
    assert_eq!(second["input_drift"]["any"], true);
    assert_eq!(second["input_drift"]["files"][0]["state"], "changed");
    assert_eq!(second["input_drift"]["files"][0]["sha256_at_run"], expected);
    assert_ne!(
        second["input_drift"]["files"][0]["sha256_now"],
        second["input_drift"]["files"][0]["sha256_at_run"]
    );
}

#[test]
fn an_interpreter_script_is_bound_from_argv_and_absence_is_explicit() {
    let p = Project::new();
    fs::write(p.0.join("script.py"), b"print('x')\n").unwrap();
    let session = p.bind("argv-binding");
    let run = p.ok(&[
        "execution",
        "run",
        "--session",
        &session,
        "--key",
        "argv-1",
        "--purpose",
        "Argv script",
        "--",
        "python",
        "script.py",
    ]);
    let execution = &run["execution"];
    assert_eq!(execution["intent"]["input_binding"], "argv_script");
    assert_eq!(execution["intent"]["inputs"][0]["path"], "script.py");
    assert_eq!(execution["intent"]["inputs"][0]["source"], "argv_script");

    let none = p.ok(&[
        "execution",
        "run",
        "--session",
        &session,
        "--key",
        "argv-2",
        "--purpose",
        "No script",
        "--",
        "node",
        "-e",
        "console.log(1)",
    ]);
    assert_eq!(none["execution"]["intent"]["inputs"], json!([]));
    assert_eq!(none["execution"]["intent"]["input_binding"], "none");
}

#[test]
fn a_missing_input_fails_registration_when_required() {
    let p = Project::new();
    let session = p.bind("require-input");
    let revision = p.ok(&["status"])["project_revision"].to_string();
    let output = run_managed_output(
        &p,
        &session,
        "require-1",
        "Require input",
        &["--bind", "missing.py", "--require-input-hash"],
    );
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(report["code"], "SourceUnavailable");
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("cannot be read")
    );
    assert_eq!(p.ok(&["status"])["project_revision"].to_string(), revision);
}

#[test]
fn a_touched_input_keeps_the_key_idempotent_while_changed_bytes_conflict() {
    let p = Project::new();
    let path = p.0.join("a.py");
    fs::write(&path, b"print('one')\n").unwrap();
    let session = p.bind("idempotent");
    let first = run_managed(&p, &session, "same-1", "Same key", &["--bind", "a.py"]);
    let id = first["execution"]["id"].as_str().unwrap().to_owned();

    // Same bytes, rewritten (mtime changes) and spelled `./a.py`: still the same intent.
    fs::write(&path, b"print('one')\n").unwrap();
    let touched = run_managed(&p, &session, "same-1", "Same key", &["--bind", "./a.py"]);
    assert_eq!(touched["created"], false);
    assert_eq!(touched["execution"]["id"], id);

    fs::write(&path, b"print('two')\n").unwrap();
    let output = run_managed_output(&p, &session, "same-1", "Same key", &["--bind", "a.py"]);
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(report["code"], "SourceConflict");
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("input binding conflicts with the receipt recorded for this operation key")
    );
}

#[test]
fn adding_inputs_to_an_existing_key_is_a_named_conflict() {
    let p = Project::new();
    fs::write(p.0.join("a.py"), b"print('one')\n").unwrap();
    let session = p.bind("upgrade");
    let first = run_managed(&p, &session, "upgrade-1", "No inputs", &[]);
    assert_eq!(first["execution"]["intent"]["input_binding"], "none");

    let output = run_managed_output(&p, &session, "upgrade-1", "No inputs", &["--bind", "a.py"]);
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(report["code"], "SourceConflict");
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("input binding conflicts with the receipt recorded for this operation key")
    );
}

/// AC8: the streaming hash behind `--bind` and `awr_source::fingerprint` agree
/// byte for byte. The receipt is the observable side, because this crate is
/// binary-only and its private hasher is not reachable from an integration test.
/// Both sides could be wrong in the same way; AC1's independent check is the
/// authority on correctness, this only pins the shared convention.
#[test]
fn content_sha256_matches_the_source_fingerprint_for_edge_sizes() {
    let p = Project::new();
    let session = p.bind("hash-parity");
    let samples: [(&str, Vec<u8>); 3] = [
        ("empty.bin", Vec::new()),
        ("one-byte.bin", vec![0x5a]),
        (
            "one-mib.bin",
            (0..1024 * 1024).map(|index| (index % 251) as u8).collect(),
        ),
    ];
    for (index, (name, body)) in samples.iter().enumerate() {
        fs::write(p.0.join(name), body).unwrap();
        let key = format!("hash-{index}");
        let run = run_managed(&p, &session, &key, "Hash parity", &["--bind", name]);
        let input = &run["execution"]["intent"]["inputs"][0];
        let recorded = input["sha256"].as_str().unwrap();
        let expected = fingerprint(body);
        assert_eq!(
            recorded,
            expected.strip_prefix("sha256:").unwrap(),
            "the streaming hash must match awr_source::fingerprint for {name}"
        );
        assert_eq!(recorded.len(), 64);
        assert_eq!(input["size"], body.len() as u64);
    }
}
