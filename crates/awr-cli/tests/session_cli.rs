use awr_core::{CheckpointDraft, Error, EventDraft, Id};
use awr_store::Store;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-session-cli-{}", Id::new()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("work-ledger.yaml"),"work_items:\n- id: W\n  title: Implement runtime\n  owner: source-team\n  status: in_progress\n  next_action: Wire session\n- id: OTHER\n  title: Other work\n  status: ready\n").unwrap();
        let f = Self(root);
        assert!(f.run(&["init", "--accept"]).status.success());
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.0)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert!(
            r.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn error(&self, args: &[&str], code: &str) {
        let r = self.run(args);
        assert!(!r.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stderr).unwrap()["code"],
            code
        );
    }
    fn revision(&self) -> String {
        self.ok(&["session", "list"])["project_revision"].to_string()
    }
    fn start(&self, agent: &str, claim: bool) -> Value {
        let rev = self.ok(&["status"])["project_revision"].to_string();
        let mut args = vec![
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            agent,
            "--provider",
            "fixture",
            "--model",
            "test",
            "--expected-revision",
            &rev,
        ];
        if claim {
            args.push("--claim");
        }
        self.ok(&args)
    }
    fn checkpoint(&self, id: &str) -> Value {
        self.ok(&[
            "session",
            "checkpoint",
            "--session",
            id,
            "--context-hash",
            &"a".repeat(64),
            "--digest",
            "Implemented session plumbing",
            "--next-action",
            "Connect context compiler",
            "--open-loop",
            "Keep unfinished artifact review",
            "--expected-revision",
            &self.revision(),
        ])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn checkpoint_help_explains_the_consumed_hash_and_progress_fields() {
    let result = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["session", "checkpoint", "--help"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let help = String::from_utf8(result.stdout).unwrap();
    for text in [
        "64 hexadecimal characters",
        "without a sha256: prefix",
        "work_context.context_hash",
        "work prepare",
        "context compile",
        "Nonblank progress summary",
        "Nonblank next action",
    ] {
        assert!(help.contains(text), "Missing {text:?} in {help}");
    }
}

#[test]
fn checkpoint_rejects_prefixed_hash_with_actionable_error_and_keeps_bare_hashes() {
    let f = Fixture::new();
    let session = f.start("executor", false);
    let sid = session["session"]["id"].as_str().unwrap();
    let revision = f.revision();
    let hash = "A".repeat(64);
    let mut args = vec![
        "session",
        "checkpoint",
        "--session",
        sid,
        "--context-hash",
        &hash,
        "--digest",
        "Recorded progress",
        "--next-action",
        "Continue",
        "--expected-revision",
        &revision,
    ];
    let prefixed = format!("sha256:{hash}");
    args[5] = &prefixed;
    let result = f.run(&args);
    assert!(!result.status.success());
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(error["code"], "InvalidInput");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("--context-hash") && message.contains("without a sha256: prefix"));
    assert!(message.contains("work_context.context_hash"));
    assert!(!message.contains(&prefixed));
    assert_eq!(f.revision(), revision);
    args[5] = &hash;
    let saved = f.ok(&args);
    assert_eq!(
        saved["checkpoint"]["context_hash"],
        hash.to_ascii_lowercase()
    );
}

#[test]
fn compact_checkpoint_records_observed_changes_and_keeps_caller_claims_separate() {
    let f = Fixture::new();
    let a = f.start("executor", false);
    let sid = a["session"]["id"].as_str().unwrap();
    let b = f.start("other-agent", false);
    let other = b["session"]["id"].as_str().unwrap();
    let observed_event = {
        let mut store = Store::open(&f.0.join(".awr/state.db")).unwrap();
        let project = store.project_by_root(&f.0).unwrap();
        let mut event = EventDraft::new("work.progress", "Retain the implemented behavior");
        event.session_id = Some(sid.parse().unwrap());
        event.payload = serde_json::json!({"body":"RAW_PROCESS_BODY".repeat(10000),"changed_entities":["UNVERIFIED_EVENT_CLAIM"]});
        let first = store
            .append_event(project.id, project.project_revision, event)
            .unwrap();
        let mut second = EventDraft::new("work.progress", "OTHER_SESSION_HISTORY");
        second.session_id = Some(other.parse().unwrap());
        store
            .append_event(project.id, first.project_revision, second)
            .unwrap();
        first.id
    };
    fs::write(f.0.join("deliverable.txt"), "RAW_ARTIFACT_BODY").unwrap();
    let artifact = f.ok(&[
        "artifact",
        "add",
        "deliverable.txt",
        "--type",
        "report",
        "--mime",
        "text/plain",
        "--source-event",
        &observed_event.to_string(),
        "--expected-revision",
        &f.revision(),
    ]);
    let ledger = f.0.join("work-ledger.yaml");
    fs::write(
        &ledger,
        fs::read_to_string(&ledger)
            .unwrap()
            .replace("Wire session", "Continue the source change"),
    )
    .unwrap();
    let revision = f.ok(&["status"])["project_revision"].to_string();
    let cp = f.ok(&[
        "session",
        "checkpoint",
        "--session",
        sid,
        "--context-hash",
        &"b".repeat(64),
        "--digest",
        "Implemented a real source update",
        "--next-action",
        "Review saved progress",
        "--open-loop",
        "Retain the unfinished review",
        "--changed-entity",
        "CALLER_REPORTED_REFERENCE",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(cp["save_status"], "completed");
    assert_eq!(cp["context_hash_verified"], false);
    assert_eq!(cp["checkpoint"]["project_revision"].to_string(), revision);
    assert_eq!(
        cp["project_revision"].as_u64().unwrap(),
        revision.parse::<u64>().unwrap() + 2
    );
    let id = cp["checkpoint"]["id"].as_str().unwrap();
    let full = f.ok(&["object", "show", "checkpoint", id, "--full"]);
    let delta = &full["session_delta"];
    assert_eq!(delta["session_id"], sid);
    assert_eq!(
        delta["after_revision"],
        a["session"]["start_project_revision"]
    );
    assert_eq!(
        delta["through_revision"],
        cp["checkpoint"]["project_revision"]
    );
    assert!(
        delta["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["event"]["id"] == observed_event.to_string())
    );
    assert!(
        delta["session_events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["session_id"] == sid)
    );
    assert_eq!(
        delta["reported_changed_entities"],
        serde_json::json!(["CALLER_REPORTED_REFERENCE"])
    );
    assert_eq!(
        delta["observed_changed_entities"],
        cp["checkpoint"]["changed_entities"]
    );
    assert!(
        !delta["observed_changed_entities"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        delta["observed_changed_entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str()
                == Some(&format!(
                    "artifact:{}",
                    artifact["artifact"]["id"].as_str().unwrap()
                )))
    );
    for excluded in [
        "RAW_PROCESS_BODY",
        "RAW_ARTIFACT_BODY",
        "OTHER_SESSION_HISTORY",
        "UNVERIFIED_EVENT_CLAIM",
    ] {
        assert!(!full.to_string().contains(excluded));
    }
    assert!(
        !cp["checkpoint"]["changed_entities"]
            .to_string()
            .contains("CALLER_REPORTED_REFERENCE")
    );
    assert!(
        delta["source_observations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| !s["changes"].as_array().unwrap().is_empty())
    );
    let metadata = f.ok(&["object", "show", "checkpoint", id]);
    assert!(metadata.get("session_delta").is_none());
    assert_eq!(metadata["checkpoint_save"]["delta_recorded"], true);
    f.error(
        &[
            "object",
            "show",
            "checkpoint",
            id,
            "--full",
            "--max-bytes",
            "100",
        ],
        "InvalidInput",
    );
    let second = f.checkpoint(sid);
    let second_detail = f.ok(&[
        "object",
        "show",
        "checkpoint",
        second["checkpoint"]["id"].as_str().unwrap(),
        "--full",
    ]);
    assert_eq!(second_detail["session_delta"]["baseline_checkpoint_id"], id);
    assert_eq!(
        second_detail["session_delta"]["session_events"],
        serde_json::json!([])
    );
    assert_eq!(
        second_detail["session_delta"]["source_observations"],
        serde_json::json!([])
    );
    let unchanged = f.ok(&["object", "show", "checkpoint", id, "--full"]);
    for field in ["object", "session_delta", "checkpoint_save"] {
        assert_eq!(unchanged[field], full[field]);
    }
    let shown = f.ok(&["session", "show", sid]);
    assert_eq!(shown["checkpoint_saves"]["incomplete_count"], 0);
    assert_eq!(
        shown["checkpoint_saves"]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn interrupted_saves_remain_visible_and_intervening_changes_require_a_fresh_attempt() {
    let f = Fixture::new();
    let start = f.start("executor", false);
    let sid = start["session"]["id"].as_str().unwrap();
    let first = f.checkpoint(sid);
    let draft = CheckpointDraft {
        context_hash: "a".repeat(64),
        digest: "Progress before interruption".into(),
        next_action: "UNCOMMITTED_NEXT_ACTION".into(),
        open_loops: vec!["UNCOMMITTED_LOOP".into()],
        changed_entities: vec![],
    };
    let started = {
        let mut store = Store::open(&f.0.join(".awr/state.db")).unwrap();
        let p = store.project_by_root(&f.0).unwrap();
        store
            .begin_checkpoint_save(p.id, p.project_revision, sid.parse().unwrap(), draft)
            .unwrap()
    }; // A process can stop at this durable boundary without creating a checkpoint.
    let shown = f.ok(&["session", "show", sid]);
    assert_eq!(shown["checkpoint"]["id"], first["checkpoint"]["id"]);
    assert_eq!(shown["checkpoint_saves"]["incomplete_count"], 1);
    assert_eq!(
        shown["checkpoint_saves"]["attempts"][0]["id"],
        started.id.to_string()
    );
    assert_eq!(
        shown["checkpoint_saves"]["attempts"][0]["status"],
        "pending_or_interrupted"
    );
    assert!(
        !shown["checkpoint"]
            .to_string()
            .contains("UNCOMMITTED_NEXT_ACTION")
    );
    let attempt = f.ok(&["event", "show", &started.id.to_string(), "--full"]);
    assert_eq!(
        attempt["event"]["payload"]["draft"]["next_action"],
        "UNCOMMITTED_NEXT_ACTION"
    );
    {
        let mut store = Store::open(&f.0.join(".awr/state.db")).unwrap();
        let p = store.project_by_root(&f.0).unwrap();
        let event = store
            .append_event(
                p.id,
                p.project_revision,
                EventDraft::new("work.observed", "An intervening revision"),
            )
            .unwrap();
        assert!(matches!(
            store.finish_checkpoint_save(p.id, event.project_revision, started.id),
            Err(Error::RevisionConflict { .. })
        ));
        assert_eq!(
            store
                .latest_checkpoint(p.id, sid.parse().unwrap())
                .unwrap()
                .unwrap()
                .id
                .to_string(),
            first["checkpoint"]["id"].as_str().unwrap()
        );
    }
    let next = f.checkpoint(sid);
    let shown = f.ok(&["session", "show", sid]);
    assert_eq!(shown["checkpoint"]["id"], next["checkpoint"]["id"]);
    assert_eq!(shown["checkpoint_saves"]["incomplete_count"], 1);
    assert_eq!(
        shown["checkpoint_saves"]["attempts"][0]["status"],
        "completed"
    );
    assert_eq!(
        shown["checkpoint_saves"]["attempts"][1]["status"],
        "pending_or_interrupted"
    );
}

#[test]
fn cli_handoff_transfers_claim_and_preserves_both_histories_and_open_loops() {
    let f = Fixture::new();
    let source = fs::read(f.0.join("work-ledger.yaml")).unwrap();
    let first = f.start("executor", true);
    let sender = first["session"]["id"].as_str().unwrap();
    let old_claim = first["claim"]["id"].as_str().unwrap();
    let second = f.start("successor", false);
    let receiver = second["session"]["id"].as_str().unwrap();
    f.error(
        &[
            "session",
            "end",
            "--work",
            "W",
            "--expected-revision",
            &f.revision(),
        ],
        "InvalidInput",
    );
    f.error(
        &[
            "work",
            "handoff",
            "W",
            "--session",
            sender,
            "--to-session",
            receiver,
            "--expected-revision",
            &f.revision(),
        ],
        "ContextIncomplete",
    );
    let checkpoint = f.checkpoint(sender);
    let before = f.revision();
    f.error(
        &[
            "work",
            "handoff",
            "W",
            "--session",
            sender,
            "--to-session",
            sender,
            "--expected-revision",
            &before,
        ],
        "InvalidInput",
    );
    assert_eq!(f.revision(), before);
    f.error(
        &[
            "work",
            "handoff",
            "W",
            "--session",
            sender,
            "--to-session",
            receiver,
            "--expected-revision",
            "0",
        ],
        "RevisionConflict",
    );
    assert_eq!(f.revision(), before);
    let handed = f.ok(&[
        "work",
        "handoff",
        "W",
        "--session",
        sender,
        "--to-session",
        receiver,
        "--expected-revision",
        &before,
    ]);
    assert_eq!(handed["handoff"]["from_session"]["status"], "incomplete");
    assert_eq!(
        handed["handoff"]["transferred_claim"]["agent_id"],
        "successor"
    );
    assert_ne!(handed["handoff"]["transferred_claim"]["id"], old_claim);
    let shown = f.ok(&["session", "show", receiver]);
    assert!(shown["checkpoint"].is_null());
    assert_eq!(
        shown["inherited_checkpoint"]["id"],
        checkpoint["checkpoint"]["id"]
    );
    assert_eq!(
        shown["inherited_checkpoint"]["open_loops"][0],
        "Keep unfinished artifact review"
    );
    assert_eq!(shown["context_requires_refresh"], true);
    assert_eq!(
        f.ok(&["session", "show", sender])["claims"][0]["status"],
        "released"
    );
    // Both receipts share one revision. Cursor pagination must not lose the second receipt.
    let after = handed["project_revision"].as_u64().unwrap() - 1;
    let page = f.ok(&[
        "work",
        "history",
        "W",
        "--after-revision",
        &after.to_string(),
        "--limit",
        "1",
    ]);
    let cursor = page["next_cursor"].to_string();
    assert!(page["next_cursor"].is_object());
    assert!(page["events"][0].get("payload").is_none());
    let next = f.ok(&["work", "history", "W", "--cursor", &cursor, "--limit", "1"]);
    assert_ne!(page["events"][0]["id"], next["events"][0]["id"]);
    assert_eq!(
        page["events"][0]["project_revision"],
        next["events"][0]["project_revision"]
    );
    for id in [sender, receiver] {
        assert!(
            !f.ok(&[
                "work",
                "history",
                "W",
                "--session",
                id,
                "--after-revision",
                &after.to_string()
            ])["events"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    f.ok(&[
        "session",
        "end",
        "--session",
        receiver,
        "--expected-revision",
        &f.revision(),
    ]);
    assert!(
        f.ok(&["session", "list", "--active"])["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(f.0.join("work-ledger.yaml")).unwrap(), source);
    assert_eq!(f.ok(&["doctor"])["ok"], true);
}

#[test]
fn cleanup_and_retained_history_work_without_authoritative_sources() {
    let f = Fixture::new();
    let started = f.start("worker", false);
    let id = started["session"]["id"].as_str().unwrap();
    let acquired = f.ok(&[
        "work",
        "claim",
        "W",
        "--agent",
        "worker",
        "--expected-revision",
        &f.revision(),
    ]);
    let claim = acquired["claim"]["id"].as_str().unwrap();
    f.ok(&[
        "work",
        "release",
        "W",
        "--session",
        id,
        "--claim",
        claim,
        "--expected-revision",
        &f.revision(),
    ]);
    f.ok(&[
        "work",
        "claim",
        "W",
        "--session",
        id,
        "--expected-revision",
        &f.revision(),
    ]);
    fs::remove_file(f.0.join("work-ledger.yaml")).unwrap();
    let ended = f.ok(&[
        "session",
        "end",
        "--session",
        id,
        "--outcome",
        "interrupted",
        "--expected-revision",
        &f.revision(),
    ]);
    assert_eq!(ended["source_refresh_performed"], false);
    assert_eq!(ended["closed_claim_ids"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.ok(&["session", "show", id])["claims"][1]["status"],
        "released"
    );
    assert!(
        !f.ok(&["work", "history", "W"])["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unassigned_handoff_releases_work_and_keeps_checkpoint_for_later_pickup() {
    let f = Fixture::new();
    let started = f.start("worker", true);
    let id = started["session"]["id"].as_str().unwrap();
    f.checkpoint(id);
    let ended = f.ok(&[
        "work",
        "handoff",
        "W",
        "--session",
        id,
        "--expected-revision",
        &f.revision(),
    ]);
    assert!(ended["handoff"]["to_session"].is_null());
    assert!(ended["handoff"]["transferred_claim"].is_null());
    assert_eq!(
        f.ok(&["session", "show", id])["claims"][0]["status"],
        "released"
    );
    f.start("later-worker", true);
}

#[test]
fn session_start_conversation_binding_matches_the_mcp_surface() {
    let f = Fixture::new();
    let rev = f.revision();

    // A CLI start with --client/--conversation resolves for the MCP channel.
    let started = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--expected-revision",
        &rev,
    ]);
    let session_id = started["session"]["id"].as_str().unwrap().to_string();
    assert_eq!(started["binding_reused"], Value::Null);

    // The same binding resolves the session through the store, as the MCP
    // channel does, and a repeated start reuses it without a new session.
    let project_id: Id = started["session"]["project_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let store = Store::open(&f.0.join(".awr").join("state.db")).unwrap();
    let binding = awr_core::McpSessionBinding {
        client: "cli-host".into(),
        conversation: "conv-1".into(),
    };
    let resolved = store
        .mcp_bound_session(project_id, &binding)
        .unwrap()
        .unwrap();
    assert_eq!(resolved.id.to_string(), session_id);
    drop(store);

    let rev = f.revision();
    let reused = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--expected-revision",
        &rev,
    ]);
    assert_eq!(reused["binding_reused"], serde_json::json!(true));
    assert_eq!(reused["session"]["id"].as_str().unwrap(), session_id);

    // A different identity on the same conversation is refused.
    let rev = f.revision();
    f.error(
        &[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "other-agent",
            "--provider",
            "fixture",
            "--model",
            "test",
            "--client",
            "cli-host",
            "--conversation",
            "conv-1",
            "--expected-revision",
            &rev,
        ],
        "SourceConflict",
    );
}

#[test]
fn session_start_reuse_with_claim_acquires_the_claim() {
    let f = Fixture::new();
    let rev = f.revision();
    let started = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--expected-revision",
        &rev,
    ]);
    let session_id = started["session"]["id"].as_str().unwrap().to_string();

    // Reusing the binding with --claim must not silently drop the claim.
    let rev = f.revision();
    let reused = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--claim",
        "--ttl-ms",
        "60000",
        "--expected-revision",
        &rev,
    ]);
    assert_eq!(reused["binding_reused"], serde_json::json!(true));
    assert_eq!(reused["session"]["id"].as_str().unwrap(), session_id);
    let claim = reused["claim"]
        .as_object()
        .expect("claim acquired on reuse");
    assert!(claim["id"].as_str().is_some());
    assert!(claim["session_id"].as_str().is_some());

    // The claim is durable and owned by the reused session.
    let project_id: Id = started["session"]["project_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let store = Store::open(&f.0.join(".awr").join("state.db")).unwrap();
    let session: Id = session_id.parse().unwrap();
    let claims = store.session_claims(project_id, session).unwrap();
    assert!(
        claims.iter().any(|c| c.session_id == session),
        "reused session owns the acquired claim"
    );
}

#[test]
fn session_start_without_binding_flags_is_unchanged() {
    let f = Fixture::new();
    let rev = f.revision();
    let started = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "plain",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--expected-revision",
        &rev,
    ]);
    assert!(started["session"]["id"].as_str().is_some());
    assert_eq!(started["binding_reused"], Value::Null);
}

#[test]
fn session_start_reuse_enforces_the_expected_revision() {
    let f = Fixture::new();
    let rev = f.revision();
    let started = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--expected-revision",
        &rev,
    ]);
    // The revision at start is stale after the start event; the same command
    // with a new binding would reject it, and reuse must too.
    let stale = started["session"]["start_project_revision"]
        .as_i64()
        .unwrap()
        .to_string();
    f.error(
        &[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "host-agent",
            "--provider",
            "fixture",
            "--model",
            "test",
            "--client",
            "cli-host",
            "--conversation",
            "conv-1",
            "--expected-revision",
            &stale,
        ],
        "RevisionConflict",
    );
}

#[test]
fn session_start_reuse_rejects_a_closed_session_with_repair_guidance() {
    let f = Fixture::new();
    let rev = f.revision();
    let started = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--expected-revision",
        &rev,
    ]);
    let session_id = started["session"]["id"].as_str().unwrap().to_string();
    f.ok(&[
        "session",
        "end",
        "--session",
        &session_id,
        "--expected-revision",
        &f.revision(),
    ]);
    let result = f.run(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--expected-revision",
        &f.revision(),
    ]);
    assert!(!result.status.success());
    let payload: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(payload["code"], "InvalidTransition");
    let message = payload["message"].as_str().unwrap();
    assert!(message.contains("resume the session"), "{message}");
}

#[test]
fn session_start_reuse_never_steals_another_sessions_claim() {
    let f = Fixture::new();
    let rev = f.revision();
    let started = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--expected-revision",
        &rev,
    ]);
    // A different session acquires the work claim first.
    let other = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "other-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--expected-revision",
        &f.revision(),
    ]);
    let other_id = other["session"]["id"].as_str().unwrap().to_string();
    f.ok(&[
        "work",
        "claim",
        "W",
        "--session",
        &other_id,
        "--expected-revision",
        &f.revision(),
    ]);
    // Reuse with --claim must not steal it.
    f.error(
        &[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "host-agent",
            "--provider",
            "fixture",
            "--model",
            "test",
            "--client",
            "cli-host",
            "--conversation",
            "conv-1",
            "--claim",
            "--expected-revision",
            &f.revision(),
        ],
        "ClaimConflict",
    );
}

#[test]
fn session_start_reuse_reacquires_an_expired_claim() {
    let f = Fixture::new();
    let rev = f.revision();
    let started = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--claim",
        "--ttl-ms",
        "1",
        "--expected-revision",
        &rev,
    ]);
    let first_claim = started["claim"]["id"].as_str().unwrap().to_string();
    std::thread::sleep(std::time::Duration::from_millis(60));
    // The claim has expired; a repeated start reuses the session and the
    // reacquired claim is a new active one, not the expired record.
    let reused = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "cli-host",
        "--conversation",
        "conv-1",
        "--claim",
        "--expected-revision",
        &f.revision(),
    ]);
    assert_eq!(reused["binding_reused"], serde_json::json!(true));
    let reacquired = reused["claim"]["id"].as_str().unwrap();
    assert_ne!(reacquired, first_claim);
}

#[test]
fn malformed_work_history_cursor_is_reported_as_invalid_input() {
    let f = Fixture::new();
    let cursor = r#"{"secret_marker":"S3CR3T_CURSOR_VALUE"}"#;
    let output = f.run(&["work", "history", "W", "--cursor", cursor]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "InvalidInput");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("invalid event cursor JSON")
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("S3CR3T_CURSOR_VALUE"));
    assert!(!stderr.contains("secret_marker"));
}
