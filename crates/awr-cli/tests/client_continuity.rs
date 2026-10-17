use awr_core::{Id, McpSessionBinding};
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::Arc,
};
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr client {}", awr_core::Id::new())));
        fs::create_dir(&p.0).unwrap();
        p.ok(&[
            "init",
            "--goal",
            "Verify useful document search",
            "--accept",
        ]);
        p
    }
    fn run(&self, args: &[&str], input: Option<&Value>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
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
        decode(self.run(args, None))
    }
    fn error(&self, args: &[&str], code: &str) {
        let output = self.run(args, None);
        assert!(!output.status.success(), "{args:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            code,
            "{args:?}"
        );
    }
    fn hook(&self, session: &str, event: &str, turn: Option<&str>) -> Value {
        decode(self.run(&["client","hook","--client","codex","--work","INTAKE-001"],Some(&json!({"session_id":session,"hook_event_name":event,"cwd":self.0,"turn_id":turn,"model":"client-test"}))))
    }
    fn generic_hook(&self, session: &str, event: &str, turn: Option<&str>) -> Value {
        decode(self.run(
            &["client", "hook", "--client", "generic", "--work", "INTAKE-001"],
            Some(&json!({"session_id":session,"hook_event_name":event,"cwd":self.0,"turn_id":turn,"model":"client-test"})),
        ))
    }
}
fn decode(o: Output) -> Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn automatic_stop_and_compact_save_persisted_work_and_deduplicate() {
    let p = Project::new();
    let started = p.hook("conversation-a", "SessionStart", None);
    assert!(
        started["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("INTAKE-001")
    );
    p.ok(&[
        "client",
        "progress",
        "--client",
        "codex",
        "--external-session",
        "conversation-a",
        "--next-action",
        "Finish the reviewed search handler",
        "--open-loop",
        "Independent review is pending",
    ]);
    let saved = p.hook("conversation-a", "Stop", Some("turn-one"));
    assert_eq!(saved["awr"]["checkpoint_saved"], true);
    assert_eq!(
        saved["awr"]["binding"]["next_action"],
        "Finish the reviewed search handler"
    );
    let duplicate = p.hook("conversation-a", "Stop", Some("turn-one"));
    assert_eq!(duplicate["awr"]["duplicate"], true);
    assert_eq!(
        duplicate["awr"]["binding"]["checkpoint_id"],
        saved["awr"]["binding"]["checkpoint_id"]
    );
    p.ok(&[
        "client",
        "progress",
        "--client",
        "codex",
        "--external-session",
        "conversation-a",
        "--next-action",
        "Apply the reviewer corrections",
    ]);
    let revised = p.hook("conversation-a", "Stop", Some("turn-one"));
    assert_ne!(
        revised["awr"]["binding"]["checkpoint_id"],
        saved["awr"]["binding"]["checkpoint_id"]
    );
    let compact = p.hook("conversation-a", "PreCompact", None);
    assert_eq!(compact["awr"]["checkpoint_saved"], true);
    let resumed = p.hook("conversation-a", "SessionStart", None);
    assert!(
        resumed["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Independent review is pending")
    );
    assert_eq!(
        resumed["awr"]["binding"]["session_id"],
        started["awr"]["binding"]["session_id"]
    );
}

#[test]
fn post_compact_accepts_explicit_host_metrics_without_assuming_native_token_fields() {
    let p = Project::new();
    let started = p.hook("compact-conversation", "SessionStart", None);
    let input = json!({"session_id":"compact-conversation","hook_event_name":"PostCompact","cwd":p.0,"model":"client-test",
        "awr_compaction":{"observation":{"compaction_id":"native-1","sequence":1,"observed_at":awr_core::now_millis().unwrap(),"trigger":"automatic","model":"client-test","source":"host.full_request","measurement_scope":"full_request","measurement_basis":"host_reported","after_tokens":180000,"context_window_tokens":256000}}});
    let args = [
        "client",
        "hook",
        "--client",
        "codex",
        "--work",
        "INTAKE-001",
    ];
    let recorded = decode(p.run(&args, Some(&input)));
    assert_eq!(
        recorded["awr"]["binding"]["session_id"],
        started["awr"]["binding"]["session_id"]
    );
    assert_eq!(recorded["awr"]["compaction"]["state"], "handoff_candidate");
    assert!(
        recorded["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Action guidance:")
    );
    let again = decode(p.run(&args, Some(&input)));
    assert_eq!(again["awr"]["compaction"]["already_recorded"], true);
    assert_eq!(
        again["awr"]["compaction"]["recorded_event_id"],
        recorded["awr"]["compaction"]["recorded_event_id"]
    );
    let plain = p.hook("compact-conversation", "PostCompact", None);
    assert!(plain["awr"].get("compaction").is_none());
    let mut invalid = input;
    invalid["hook_event_name"] = json!("PreCompact");
    assert!(!p.run(&args, Some(&invalid)).status.success());
}
#[test]
fn conversations_keep_separate_next_actions_and_checkpoints() {
    let p = Project::new();
    let a = p.hook("conversation-a", "SessionStart", None);
    let b = p.hook("conversation-b", "SessionStart", None);
    assert_ne!(
        a["awr"]["binding"]["session_id"],
        b["awr"]["binding"]["session_id"]
    );
    p.ok(&[
        "client",
        "progress",
        "--client",
        "codex",
        "--external-session",
        "conversation-a",
        "--next-action",
        "Review the search implementation",
    ]);
    p.ok(&[
        "client",
        "progress",
        "--client",
        "codex",
        "--external-session",
        "conversation-b",
        "--next-action",
        "Review document import",
    ]);
    assert_eq!(
        p.hook("conversation-a", "Stop", Some("same-turn"))["awr"]["binding"]["next_action"],
        "Review the search implementation"
    );
    assert_eq!(
        p.hook("conversation-b", "Stop", Some("same-turn"))["awr"]["binding"]["next_action"],
        "Review document import"
    );
}
#[test]
fn concurrent_duplicate_deliveries_make_one_checkpoint() {
    let p = Arc::new(Project::new());
    p.hook("parallel-delivery", "SessionStart", None);
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let p = p.clone();
            std::thread::spawn(move || p.hook("parallel-delivery", "Stop", Some("one-turn")))
        })
        .collect();
    let values: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(
        values.iter().all(|v| v["awr"]["binding"]["checkpoint_id"]
            == values[0]["awr"]["binding"]["checkpoint_id"])
    );
    assert_eq!(
        values
            .iter()
            .filter(|v| v["awr"]["duplicate"] == false)
            .count(),
        1
    );
}
#[test]
fn installing_hooks_preserves_existing_entries_and_requires_no_global_edits() {
    let p = Project::new();
    fs::create_dir(p.0.join(".codex")).unwrap();
    let old = json!({"description":"existing project policy","hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"existing-policy"}]}]}});
    fs::write(
        p.0.join(".codex/hooks.json"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let preview = p.ok(&[
        "client",
        "install",
        "--client",
        "codex",
        "--work",
        "INTAKE-001",
    ]);
    assert_eq!(preview["installed"], false);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(p.0.join(".codex/hooks.json")).unwrap()).unwrap(),
        old
    );
    let installed = p.ok(&[
        "client",
        "install",
        "--client",
        "codex",
        "--work",
        "INTAKE-001",
        "--accept",
    ]);
    assert_eq!(
        installed["configuration"]["hooks"]["PreToolUse"],
        old["hooks"]["PreToolUse"]
    );
    assert_eq!(installed["activation_verified"], false);
    let repeated = p.ok(&[
        "client",
        "install",
        "--client",
        "codex",
        "--work",
        "INTAKE-001",
        "--accept",
    ]);
    assert_eq!(
        repeated["configuration"]["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn foreign_project_event_and_forged_generic_binding_are_rejected() {
    let p = Project::new();
    let other = Project::new();
    let event =
        json!({"session_id":"foreign","cwd":other.0,"hook_event_name":"Stop","turn_id":"one"});
    assert!(
        !p.run(
            &[
                "client",
                "hook",
                "--client",
                "codex",
                "--work",
                "INTAKE-001"
            ],
            Some(&event)
        )
        .status
        .success()
    );
    let rev = p.ok(&["status"])["project_revision"].to_string();
    let forged = p.run(
        &[
            "event",
            "append",
            "--type",
            "client.bound",
            "--summary",
            "Forged client binding",
            "--expected-revision",
            &rev,
        ],
        None,
    );
    assert!(!forged.status.success());
    assert!(String::from_utf8_lossy(&forged.stderr).contains("reserved"));
    assert!(
        p.ok(&[
            "client",
            "show",
            "--client",
            "codex",
            "--external-session",
            "foreign"
        ])["binding"]
            .is_null()
    );
}

#[test]
fn explicit_cross_client_handoff_carries_the_saved_next_action() {
    let p = Project::new();
    let first = p.hook("old-client", "SessionStart", None);
    p.ok(&[
        "client",
        "progress",
        "--client",
        "codex",
        "--external-session",
        "old-client",
        "--next-action",
        "Deliver the reviewed search result",
        "--open-loop",
        "Check the reviewer comments",
    ]);
    p.hook("old-client", "Stop", Some("done-for-now"));
    let next = p.ok(&[
        "client",
        "bind",
        "--client",
        "generic",
        "--external-session",
        "new-client",
        "--work",
        "INTAKE-001",
        "--from-session",
        first["awr"]["binding"]["session_id"].as_str().unwrap(),
    ]);
    assert_eq!(
        next["binding"]["next_action"],
        "Deliver the reviewed search result"
    );
    assert!(
        next["context"]
            .to_string()
            .contains("Check the reviewer comments")
    );
    assert_ne!(
        next["binding"]["session_id"],
        first["awr"]["binding"]["session_id"]
    );
}

#[cfg(unix)]
#[test]
fn generated_shell_hook_runs_with_documented_native_output() {
    let p = Project::new();
    let installed = p.ok(&[
        "client",
        "install",
        "--client",
        "codex",
        "--work",
        "INTAKE-001",
        "--accept",
    ]);
    let command = installed["configuration"]["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let mut child = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            &serde_json::to_vec(
                &json!({"session_id":"native-command","hook_event_name":"SessionStart","cwd":p.0}),
            )
            .unwrap(),
        )
        .unwrap();
    let output = decode(child.wait_with_output().unwrap());
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    assert!(output.get("awr").is_none());
    assert!(
        !p.ok(&[
            "client",
            "show",
            "--client",
            "codex",
            "--external-session",
            "native-command"
        ])["binding"]
            .is_null()
    );
}

#[test]
fn l2_install_rejects_generic_host_identity() {
    let p = Project::new();
    let failed = p.run(
        &[
            "client",
            "install",
            "--client",
            "generic",
            "--work",
            "INTAKE-001",
        ],
        None,
    );
    assert!(!failed.status.success());
    let error: Value = serde_json::from_slice(&failed.stderr).unwrap();
    assert_eq!(error["code"], "Unsupported");
    assert!(error["message"].as_str().unwrap().contains("Codex only"));
}

#[test]
fn l0_generic_host_completes_the_same_lifecycle() {
    let p = Project::new();
    let event = |session: &str, name: &str, turn: Option<&str>| {
        let mut e =
            json!({"session_id":session,"hook_event_name":name,"cwd":p.0,"model":"client-test"});
        if let Some(t) = turn {
            e["turn_id"] = json!(t);
        }
        decode(p.run(
            &[
                "client",
                "hook",
                "--client",
                "generic",
                "--work",
                "INTAKE-001",
            ],
            Some(&e),
        ))
    };
    let started = event("cursor:conversation-a", "SessionStart", None);
    assert!(
        started["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("INTAKE-001")
    );
    p.ok(&[
        "client",
        "progress",
        "--client",
        "generic",
        "--external-session",
        "cursor:conversation-a",
        "--next-action",
        "Finish the reviewed search handler",
        "--open-loop",
        "Independent review is pending",
    ]);
    let saved = event("cursor:conversation-a", "Stop", Some("turn-one"));
    assert_eq!(saved["awr"]["checkpoint_saved"], true);
    let shown = p.ok(&[
        "client",
        "show",
        "--client",
        "generic",
        "--external-session",
        "cursor:conversation-a",
    ]);
    assert_eq!(shown["binding"]["client"], "generic");
    assert_eq!(
        shown["binding"]["external_session"],
        "cursor:conversation-a"
    );
    // A namespaced host id keeps identities separate without a new client enum.
    let other = p.ok(&[
        "client",
        "show",
        "--client",
        "generic",
        "--external-session",
        "conversation-a",
    ]);
    assert!(other["binding"].is_null());
}

#[test]
fn explicit_client_bind_bridges_mcp_session_without_merging_namespaces() {
    let p = Project::new();
    let conversation = "cursor:bridge-conversation";
    let started = p.ok(&[
        "session",
        "start",
        "--work",
        "INTAKE-001",
        "--agent",
        "bridge-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "stdio",
        "--conversation",
        conversation,
        "--expected-revision",
        &p.ok(&["status"])["project_revision"].to_string(),
    ]);
    let session_text = started["session"]["id"].as_str().unwrap().to_owned();
    let session_id: Id = session_text.parse().unwrap();
    let project_id: Id = started["session"]["project_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let absent = p.ok(&[
        "client",
        "show",
        "--client",
        "generic",
        "--external-session",
        conversation,
    ]);
    assert!(absent["binding"].is_null());

    let bound = p.ok(&[
        "client",
        "bind",
        "--client",
        "generic",
        "--external-session",
        conversation,
        "--work",
        "INTAKE-001",
        "--session",
        &session_text,
    ]);
    assert_eq!(bound["binding"]["session_id"], session_text);
    assert_eq!(
        p.ok(&[
            "client",
            "show",
            "--client",
            "generic",
            "--external-session",
            conversation,
        ])["binding"]["session_id"],
        session_text
    );

    {
        let store = Store::open(&p.0.join(".awr/state.db")).unwrap();
        let mcp = store
            .mcp_bound_session(
                project_id,
                &McpSessionBinding {
                    client: "stdio".into(),
                    conversation: conversation.into(),
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(mcp.id, session_id);
        let local = store
            .client_binding(project_id, "generic", conversation)
            .unwrap()
            .unwrap();
        assert_eq!(local.session_id, session_id);
        assert!(
            store
                .mcp_bound_session(
                    project_id,
                    &McpSessionBinding {
                        client: "generic".into(),
                        conversation: conversation.into(),
                    },
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(store.sessions(project_id, true, 1000).unwrap().len(), 1);
        assert!(
            store
                .session_claims(project_id, session_id)
                .unwrap()
                .is_empty()
        );
    }

    p.ok(&[
        "client",
        "progress",
        "--client",
        "generic",
        "--external-session",
        conversation,
        "--next-action",
        "Review the bridge checkpoint",
        "--open-loop",
        "Confirm the MCP handoff",
    ]);
    let checkpointed = p.generic_hook(conversation, "Stop", Some("bridge-turn"));
    assert_eq!(checkpointed["awr"]["binding"]["session_id"], session_text);
    assert_eq!(checkpointed["awr"]["checkpoint_saved"], true);

    let store = Store::open(&p.0.join(".awr/state.db")).unwrap();
    let local = store
        .client_binding(project_id, "generic", conversation)
        .unwrap()
        .unwrap();
    assert_eq!(local.session_id, session_id);
    assert!(local.checkpoint_id.is_some());
    assert!(
        store
            .latest_checkpoint(project_id, session_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(store.sessions(project_id, true, 1000).unwrap().len(), 1);
    assert!(
        store
            .session_claims(project_id, session_id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn client_bind_never_implicitly_merges_mcp_identity_and_rejects_rebinding() {
    let p = Project::new();
    let conversation = "cursor:separate-conversation";
    let started = p.ok(&[
        "session",
        "start",
        "--work",
        "INTAKE-001",
        "--agent",
        "mcp-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--client",
        "stdio",
        "--conversation",
        conversation,
        "--expected-revision",
        &p.ok(&["status"])["project_revision"].to_string(),
    ]);
    let mcp_session = started["session"]["id"].as_str().unwrap().to_owned();
    let project_id: Id = started["session"]["project_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Matching text in the MCP namespace is not an implicit local bridge.
    let local = p.ok(&[
        "client",
        "bind",
        "--client",
        "generic",
        "--external-session",
        conversation,
        "--work",
        "INTAKE-001",
    ]);
    let local_session = local["binding"]["session_id"].as_str().unwrap();
    assert_ne!(local_session, mcp_session);

    let other = p.ok(&[
        "session",
        "start",
        "--work",
        "INTAKE-001",
        "--agent",
        "other-agent",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--expected-revision",
        &p.ok(&["status"])["project_revision"].to_string(),
    ]);
    let other_session = other["session"]["id"].as_str().unwrap();

    p.error(
        &[
            "client",
            "bind",
            "--client",
            "generic",
            "--external-session",
            conversation,
            "--work",
            "INTAKE-001",
            "--session",
            other_session,
        ],
        "SourceConflict",
    );
    p.error(
        &[
            "client",
            "bind",
            "--client",
            "generic",
            "--external-session",
            conversation,
            "--work",
            "OTHER-WORK",
            "--session",
            local_session,
        ],
        "SourceConflict",
    );

    let store = Store::open(&p.0.join(".awr/state.db")).unwrap();
    assert_eq!(
        store
            .mcp_bound_session(
                project_id,
                &McpSessionBinding {
                    client: "stdio".into(),
                    conversation: conversation.into(),
                },
            )
            .unwrap()
            .unwrap()
            .id
            .to_string(),
        mcp_session
    );
    assert_eq!(
        store
            .client_binding(project_id, "generic", conversation)
            .unwrap()
            .unwrap()
            .session_id
            .to_string(),
        local_session
    );
    assert_eq!(store.sessions(project_id, true, 1000).unwrap().len(), 3);
    assert!(
        store
            .session_claims(project_id, local_session.parse().unwrap())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn omitted_client_flag_is_rejected_loudly() {
    let p = Project::new();
    for args in [
        vec![
            "client",
            "bind",
            "--external-session",
            "x",
            "--work",
            "INTAKE-001",
        ],
        vec![
            "client",
            "progress",
            "--external-session",
            "x",
            "--next-action",
            "y",
        ],
        vec!["client", "show", "--external-session", "x"],
        vec!["client", "hook", "--work", "INTAKE-001"],
        vec!["client", "install", "--work", "INTAKE-001"],
    ] {
        let out = p.run(&args, None);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "InvalidInput", "{args:?}");
    }
}
