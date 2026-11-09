use awr_core::{EventDraft, Id};
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::Instant,
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr claude {}", Id::new())));
        fs::create_dir(&p.0).unwrap();
        p.ok(&[
            "init",
            "--goal",
            "Deliver a reviewed search feature",
            "--accept",
        ]);
        p
    }
    fn run(&self, args: &[&str], input: Option<&Value>, gates_off: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_awr"));
        command
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .env("AWR_LOCAL_WEB", "0")
            .env_remove("AWR_HOOK_GATES")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if gates_off {
            command.env("AWR_HOOK_GATES", "off");
        }
        let mut child = command.spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(input).unwrap())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        child.wait_with_output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        decode(self.run(args, None, false))
    }
    fn hook(&self, input: &Value) -> Value {
        decode(self.run(
            &["client", "hook", "--client", "claude"],
            Some(input),
            false,
        ))
    }
    fn event(&self, name: &str) -> Value {
        json!({"session_id":"claude-conversation", "hook_event_name":name, "cwd":self.0, "turn_id":"turn-1"})
    }
    fn write(&self, path: &str) -> Value {
        let mut event = self.event("PreToolUse");
        event["tool_name"] = json!("Write");
        event["tool_input"] = json!({"file_path":path, "content":"Synthetic content"});
        event
    }
    fn policy(&self, hooks: &str) {
        fs::write(
            self.0.join(".awr/gates.toml"),
            format!("version = 1\n[hooks]\n{hooks}\n"),
        )
        .unwrap();
    }
    fn source_path(&self, domain: &str) -> PathBuf {
        let manifest = awr_source::Manifest::load(&self.0).unwrap();
        self.0.join(
            manifest
                .sources
                .iter()
                .find(|s| s.domain == domain)
                .unwrap()
                .path
                .as_ref()
                .unwrap(),
        )
    }
    fn bind_claim(&self) -> String {
        let rev = self.ok(&["status"])["project_revision"].to_string();
        let started = self.ok(&[
            "session",
            "start",
            "--work",
            "INTAKE-001",
            "--agent",
            "fixture",
            "--provider",
            "fixture",
            "--model",
            "test",
            "--claim",
            "--expected-revision",
            &rev,
        ]);
        let session = started["session"]["id"].as_str().unwrap().to_owned();
        self.ok(&[
            "client",
            "bind",
            "--client",
            "claude",
            "--external-session",
            "claude-conversation",
            "--work",
            "INTAKE-001",
            "--session",
            &session,
        ]);
        session
    }
    fn db_snapshot(&self) -> (u64, i64, Vec<u8>, Vec<u8>) {
        let store = Store::open_readonly(&self.0.join(".awr/state.db")).unwrap();
        let project = store
            .project_by_root(&self.0.canonicalize().unwrap())
            .unwrap();
        let conn = rusqlite::Connection::open_with_flags(
            self.0.join(".awr/state.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let count = conn
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .unwrap();
        (
            project.project_revision,
            count,
            fs::read(self.0.join(".awr/state.db")).unwrap(),
            fs::read(self.0.join(".awr/state.db-wal")).unwrap_or_default(),
        )
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn decode(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
// A pass-through leaves native permission checks in charge rather than auto-approving.
fn permission(value: &Value) -> Option<&str> {
    Some(
        value["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap_or("allow"),
    )
}

#[test]
fn criterion_1_session_start_injects_next_action_without_work_argument() {
    let p = Project::new();
    let output = p.hook(&p.event("SessionStart"));
    let context = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("Next:"), "{context}");
    assert!(context.contains("awr client bind --client claude"));
    assert!(context.contains("INTAKE-001"));
}

#[test]
fn criterion_1_missing_transcript_is_never_opened() {
    let p = Project::new();
    let transcript = p.0.join("missing/transcript.jsonl");
    let mut event = p.event("SessionStart");
    event["transcript_path"] = json!(transcript);
    assert!(p.hook(&event)["hookSpecificOutput"]["additionalContext"].is_string());
    assert!(!transcript.exists());
}

#[cfg(unix)]
#[test]
fn criterion_1_transcript_fifo_is_not_opened() {
    let p = Project::new();
    let transcript = p.0.join("transcript.pipe");
    assert!(
        Command::new("mkfifo")
            .arg(&transcript)
            .status()
            .unwrap()
            .success()
    );
    let mut event = p.event("SessionStart");
    event["transcript_path"] = json!(transcript);
    // Opening this pipe for reading would hang until a writer appeared.
    let mut child = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args([
            "--project",
            p.0.to_str().unwrap(),
            "client",
            "hook",
            "--client",
            "claude",
        ])
        .env("AWR_LOCAL_WEB", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&event).unwrap())
        .unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("hook tried to open a transcript or failed to return");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    decode(child.wait_with_output().unwrap());
}

#[test]
fn criterion_2_compact_source_uses_post_compact_continuity() {
    let p = Project::new();
    p.bind_claim();
    let mut event = p.event("SessionStart");
    event["source"] = json!("compact");
    let output = p.hook(&event);
    assert_eq!(output["awr"]["binding"]["last_hook_event"], "PostCompact");
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
}

#[test]
fn criterion_2_startup_source_is_not_post_compact() {
    let p = Project::new();
    p.bind_claim();
    for source in ["startup", "resume", "clear"] {
        let mut event = p.event("SessionStart");
        event["source"] = json!(source);
        assert_eq!(
            p.hook(&event)["awr"]["binding"]["last_hook_event"],
            "SessionStart"
        );
    }
}

#[test]
fn criterion_3_unclaimed_project_write_asks_and_deduplicates_turn() {
    let p = Project::new();
    p.policy("write_gate = 'ask'");
    let event = p.write("src/search.rs");
    let output = p.hook(&event);
    assert_eq!(permission(&output), Some("ask"));
    assert!(
        output["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("awr status")
    );
    let snapshot = p.db_snapshot();
    assert_eq!(permission(&p.hook(&event)), Some("allow"));
    assert_eq!(snapshot, p.db_snapshot());
    let mut next = event;
    next["turn_id"] = json!("turn-2");
    assert_eq!(permission(&p.hook(&next)), Some("ask"));
}

#[test]
fn criterion_3_exempt_outside_nonwrite_and_claimed_paths_allow() {
    let p = Project::new();
    p.policy("write_gate = 'ask'\nwrite_gate_exempt = ['.local/**']");
    for path in [".local/scratch.txt", "../outside.txt"] {
        assert_eq!(permission(&p.hook(&p.write(path))), Some("allow"));
    }
    let mut event = p.write("src/search.rs");
    event["tool_name"] = json!("Read");
    assert_eq!(permission(&p.hook(&event)), Some("allow"));
    p.bind_claim();
    assert_eq!(
        permission(&p.hook(&p.write("src/search.rs"))),
        Some("allow")
    );
}

#[test]
fn criterion_3_deny_and_environment_off_follow_policy() {
    let p = Project::new();
    p.policy("write_gate = 'deny'");
    let event = p.write("src/search.rs");
    assert_eq!(permission(&p.hook(&event)), Some("deny"));
    let snapshot = p.db_snapshot();
    let output = decode(p.run(
        &["client", "hook", "--client", "claude"],
        Some(&event),
        true,
    ));
    assert_eq!(permission(&output), Some("allow"));
    assert_eq!(snapshot, p.db_snapshot());
}

#[test]
fn criterion_4_completion_gate_unavailable_does_not_block_stop_or_release_claim() {
    let p = Project::new();
    p.policy("completion_reminder = true");
    let session = p.bind_claim();
    let mut event = p.event("Stop");
    event["stop_hook_active"] = json!(true);
    let output = p.hook(&event);
    assert_ne!(output["decision"], "block");
    assert_eq!(output["awr"]["checkpoint_saved"], true);
    let shown = p.ok(&["session", "show", &session]);
    assert_eq!(shown["session"]["status"], "active");
    assert!(
        shown["claims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|claim| claim["status"] == "active")
    );
}

#[test]
fn criterion_3_work_path_scope_asks_only_outside_declared_paths() {
    let p = Project::new();
    let path = p.source_path("ledger");
    let mut ledger: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&fs::read(&path).unwrap()).unwrap();
    ledger["work_items"][0]["paths"] = serde_yaml_ng::to_value(["src/**"]).unwrap();
    fs::write(path, serde_yaml_ng::to_string(&ledger).unwrap()).unwrap();
    p.bind_claim();
    p.policy("write_gate = 'deny'\nwrite_gate_paths = true");
    assert_eq!(
        permission(&p.hook(&p.write("src/search.rs"))),
        Some("allow")
    );
    let asked = p.hook(&p.write("tests/search.rs"));
    assert_eq!(permission(&asked), Some("ask"));
    assert!(
        asked["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("INTAKE-001")
    );
}

#[test]
fn criterion_3_expired_and_other_conversation_claims_do_not_authorize_writes() {
    let p = Project::new();
    p.bind_claim();
    p.policy("write_gate = 'ask'");
    let mut other = p.write("src/search.rs");
    other["session_id"] = json!("unbound-conversation");
    assert_eq!(permission(&p.hook(&other)), Some("ask"));
    let conn = rusqlite::Connection::open(p.0.join(".awr/state.db")).unwrap();
    conn.execute("UPDATE claims SET expires_at=0", []).unwrap();
    drop(conn);
    assert_eq!(permission(&p.hook(&p.write("src/search.rs"))), Some("ask"));
}

#[test]
fn criterion_3_concurrent_callbacks_interrupt_once_per_turn() {
    let p = std::sync::Arc::new(Project::new());
    p.policy("write_gate = 'ask'");
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let p = p.clone();
            std::thread::spawn(move || p.hook(&p.write("src/search.rs")))
        })
        .collect();
    let outputs: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(
        outputs
            .iter()
            .filter(|output| permission(output) == Some("ask"))
            .count(),
        1
    );
    let store = Store::open_readonly(&p.0.join(".awr/state.db")).unwrap();
    let project = store.project_by_root(&p.0.canonicalize().unwrap()).unwrap();
    let events = store
        .query_events(
            project.id,
            &awr_store::EventQuery {
                event_type: Some("client.gate".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(events.events.len(), 1);
}

#[cfg(unix)]
#[test]
fn criterion_3_symlink_and_parent_traversal_use_the_resolved_target() {
    let p = Project::new();
    let other = Project::new();
    p.policy("write_gate = 'ask'\nwrite_gate_exempt = ['.local/**']");
    std::os::unix::fs::symlink(&other.0, p.0.join("outside")).unwrap();
    assert_eq!(
        permission(&p.hook(&p.write("outside/file.txt"))),
        Some("allow")
    );
    assert_eq!(
        permission(&p.hook(&p.write("outside/../file.txt"))),
        Some("allow")
    );
    fs::create_dir_all(p.0.join(".local")).unwrap();
    fs::create_dir_all(p.0.join("src")).unwrap();
    std::os::unix::fs::symlink(p.0.join("src"), p.0.join(".local/link")).unwrap();
    assert_eq!(
        permission(&p.hook(&p.write(".local/link/file.rs"))),
        Some("ask")
    );
}

#[test]
fn criterion_3_all_write_tools_use_their_target_fields() {
    for (tool, field) in [
        ("Edit", "file_path"),
        ("Write", "file_path"),
        ("MultiEdit", "file_path"),
        ("NotebookEdit", "notebook_path"),
    ] {
        let p = Project::new();
        p.policy("write_gate = 'ask'");
        let mut event = p.event("PreToolUse");
        event["tool_name"] = json!(tool);
        event["tool_input"] = json!({field: "src/search.rs"});
        assert_eq!(permission(&p.hook(&event)), Some("ask"), "{tool}");
    }
}

#[test]
fn criterion_5_legacy_dialects_keep_session_context_and_checkpoint_shape() {
    let p = Project::new();
    for client in ["codex", "kimi", "generic"] {
        let mut event = p.event("SessionStart");
        event["session_id"] = json!(client);
        let args = ["client", "hook", "--client", client, "--work", "INTAKE-001"];
        let output = decode(p.run(&args, Some(&event), false));
        assert_eq!(output["continue"], true);
        assert!(output["hookSpecificOutput"]["additionalContext"].is_string());
        event["hook_event_name"] = json!("Stop");
        assert_eq!(
            decode(p.run(&args, Some(&event), false))["awr"]["checkpoint_saved"],
            true
        );
    }
}

#[test]
fn criterion_5_legacy_stop_delivery_key_preserves_optional_boolean() {
    use sha2::{Digest, Sha256};
    let p = Project::new();
    for (index, reported) in [Value::Null, json!(false), json!(true)]
        .into_iter()
        .enumerate()
    {
        let external = format!("legacy-flag-{index}");
        let mut event = p.event("SessionStart");
        event["session_id"] = json!(external);
        let args = [
            "client",
            "hook",
            "--client",
            "codex",
            "--work",
            "INTAKE-001",
        ];
        let started = decode(p.run(&args, Some(&event), false));
        let binding: awr_core::ClientBinding =
            serde_json::from_value(started["awr"]["binding"].clone()).unwrap();
        let store = Store::open_readonly(&p.0.join(".awr/state.db")).unwrap();
        let project = store.project_by_root(&p.0.canonicalize().unwrap()).unwrap();
        let sources = store.sources(project.id).unwrap();
        let state: Vec<_> = sources
            .iter()
            .map(|source| (&source.id, &source.fingerprint, &source.freshness))
            .collect();
        let basis = json!([
            "codex",
            external,
            "Stop",
            "turn-1",
            "",
            reported,
            binding.progress_revision,
            state,
            store
                .client_work_revision(project.id, binding.session_id)
                .unwrap()
        ]);
        let expected = format!("{:x}", Sha256::digest(serde_json::to_vec(&basis).unwrap()));
        drop(store);
        event["hook_event_name"] = json!("Stop");
        if !reported.is_null() {
            event["stop_hook_active"] = reported;
        }
        let stopped = decode(p.run(&args, Some(&event), false));
        assert_eq!(stopped["awr"]["binding"]["last_delivery_key"], expected);
    }
}

#[test]
fn criterion_6_internal_policy_error_allows_with_one_diagnostic_line() {
    let p = Project::new();
    fs::write(
        p.0.join(".awr/gates.toml"),
        "version = 99\n[hooks]\nwrite_gate = 'deny'\n",
    )
    .unwrap();
    for event in [p.write("src/search.rs"), p.event("Stop")] {
        let output = p.run(
            &["client", "hook", "--client", "claude"],
            Some(&event),
            false,
        );
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 1);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(permission(&value), Some("allow"));
        assert_ne!(value["decision"], "block");
    }
}

#[test]
fn criterion_6_database_failure_cannot_turn_into_a_block() {
    let p = Project::new();
    p.policy("write_gate = 'deny'\ncompletion_reminder = true");
    fs::rename(p.0.join(".awr/state.db"), p.0.join(".awr/unavailable.db")).unwrap();
    for event in [p.write("src/search.rs"), p.event("Stop")] {
        let output = p.run(
            &["client", "hook", "--client", "claude"],
            Some(&event),
            false,
        );
        assert!(output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(permission(&value), Some("allow"));
        assert_ne!(value["decision"], "block");
        assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 1);
    }
}

#[test]
fn criterion_6_corrupt_source_preserves_session_start_error() {
    for client in ["codex", "kimi", "generic", "claude"] {
        let p = Project::new();
        let args = ["client", "hook", "--client", client, "--work", "INTAKE-001"];
        // Exercise the existing source guard before creating a client session.
        fs::write(p.source_path("ledger"), "work_items: [\n").unwrap();
        let output = p.run(&args, Some(&p.event("SessionStart")), false);
        assert!(
            !output.status.success(),
            "{client} must reject stale sources"
        );
        assert!(output.stdout.is_empty(), "{client}");
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "SourceStale", "{client}: {error}");
    }
}

#[test]
fn criterion_7_allowed_write_neither_refreshes_sources_nor_writes_database() {
    let p = Project::new();
    p.policy("write_gate = 'ask'\nwrite_gate_exempt = ['.local/**']");
    fs::write(p.source_path("ledger"), "not a valid source\n").unwrap();
    let before = p.db_snapshot();
    assert_eq!(
        permission(&p.hook(&p.write(".local/scratch.txt"))),
        Some("allow")
    );
    assert_eq!(before, p.db_snapshot());
}

#[test]
fn criterion_7_write_gate_p95_on_ten_thousand_events() {
    let p = Project::new();
    p.policy("write_gate = 'ask'\nwrite_gate_exempt = ['.local/**']");
    p.bind_claim();
    {
        let mut store = Store::open(&p.0.join(".awr/state.db")).unwrap();
        let project = store.project_by_root(&p.0.canonicalize().unwrap()).unwrap();
        let mut revision = project.project_revision;
        for _ in 0..10_000 {
            revision = store
                .append_event(
                    project.id,
                    revision,
                    EventDraft::new("fixture.observation", "Synthetic benchmark observation"),
                )
                .unwrap()
                .project_revision;
        }
    }
    let before = p.db_snapshot();
    let mut samples = Vec::new();
    for _ in 0..40 {
        let start = Instant::now();
        assert_eq!(
            permission(&p.hook(&p.write("src/search.rs"))),
            Some("allow")
        );
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    assert_eq!(before, p.db_snapshot());
    samples.sort_by(f64::total_cmp);
    let p95 = samples[37];
    println!(
        "write_gate: fixture_events=10000 samples=40 p95_ms={p95:.3} release={}",
        !cfg!(debug_assertions)
    );
    if !cfg!(debug_assertions) {
        assert!(p95 <= 200.0, "write gate p95 was {p95:.3} ms");
    }
}
