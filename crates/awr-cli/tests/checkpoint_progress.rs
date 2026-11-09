use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Project {
    fn new() -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-checkpoint-progress-{}", Id::new())));
        fs::create_dir(&p.0).unwrap();
        fs::write(p.0.join("mapping.toml"), "[project]\nname='Progress'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        fs::write(p.0.join("work.yaml"), "goals:\n- id: G\n  title: Ship a useful guide\n  status: active\n  summary: Deliver the reviewed guide\nwork_items:\n- id: W\n  title: Write guide\n  status: in_progress\n  goal: G\n  acceptance: [Review the guide]\n  next_action: Source action A\n- id: OTHER\n  title: Other task\n  status: ready\n  goal: G\n  acceptance: [Review other work]\n  next_action: Other source action\n").unwrap();
        p.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        p
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
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
    fn revision(&self) -> String {
        self.ok(&["session", "list"])["project_revision"].to_string()
    }
    fn start(&self, agent: &str) -> Value {
        self.ok(&[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            agent,
            "--provider",
            "fixture",
            "--model",
            "synthetic",
            "--expected-revision",
            &self.revision(),
        ])
    }
    fn checkpoint(&self, sid: &str, actor: Option<&str>) -> Value {
        let revision = self.revision();
        let hash = "a".repeat(64);
        let mut args = vec![
            "session",
            "checkpoint",
            "--session",
            sid,
            "--context-hash",
            &hash,
            "--digest",
            "Tests failed; investigation is incomplete",
            "--next-action",
            "Checkpoint action B",
            "--open-loop",
            "Retest the failed case",
            "--expected-revision",
            &revision,
        ];
        if let Some(actor) = actor {
            args.extend(["--agent", actor]);
        }
        self.ok(&args)
    }
}

#[test]
fn trust_progress_distinguishes_source_from_reported_next_actions() {
    let p = Project::new();
    let session = p.start("progress-reporter");
    let sid = session["session"]["id"].as_str().unwrap();
    p.checkpoint(sid, None);
    for view in ["summary", "action"] {
        let status = p.ok(&["status", "--view", view, "--work", "W"]);
        let progress = &status["current"][0]["progress"];
        assert_eq!(progress["source_next_action"]["trust"], "source");
        assert_eq!(
            progress["latest_checkpoint_next_action"]["trust"],
            "reported"
        );
    }
    let work = p.ok(&["work", "show", "W"]);
    assert_eq!(work["progress"]["source_next_action"]["trust"], "source");
    assert_eq!(
        work["progress"]["latest_checkpoint_next_action"]["trust"],
        "reported"
    );
}

#[test]
fn trust_progress_preserves_legacy_verification_and_boundary_fields() {
    let p = Project::new();
    let session = p.start("progress-reporter");
    let sid = session["session"]["id"].as_str().unwrap();
    p.checkpoint(sid, None);
    let work = p.ok(&["work", "show", "W"]);
    let progress = &work["progress"];
    assert_eq!(
        progress["latest_checkpoint_next_action"]["trust"],
        "reported"
    );
    assert_eq!(
        progress["latest_checkpoint_next_action"]["context_hash_verified"],
        false
    );
    assert_eq!(
        progress["boundary"],
        "Checkpoint progress does not modify the authoritative source or establish completion."
    );
    assert_eq!(
        progress["latest_checkpoint_next_action"]["session_label"]["is_caller_proof"],
        false
    );
    assert_eq!(
        progress["latest_checkpoint_next_action"]["actor"]["identity_verified"],
        false
    );
}

#[test]
fn trust_session_label_is_reported_without_replacing_session_fields() {
    let p = Project::new();
    let session = p.start("label-reporter");
    let sid = session["session"]["id"].as_str().unwrap();
    let shown = p.ok(&["session", "show", sid]);
    assert_eq!(shown["session_label"]["trust"], "reported");
    assert_eq!(shown["session_label"]["agent_id"], "label-reporter");
    assert_eq!(shown["session_label"]["is_caller_proof"], false);
    assert_eq!(shown["session"], session["session"]);
}

#[test]
fn checkpoint_progress_is_visible_without_rewriting_source_authority() {
    let p = Project::new();
    let source = fs::read(p.0.join("work.yaml")).unwrap();
    let before = p.ok(&["work", "show", "W"]);
    let s = p.start("agent-a");
    let sid = s["session"]["id"].as_str().unwrap();
    fs::write(
        p.0.join("code.txt"),
        "Changed code does not change the work source",
    )
    .unwrap();
    let scan = p.ok(&["source", "reindex"]);
    assert_eq!(scan["indexed"], 0);
    let cp = p.checkpoint(sid, None);
    let id = &cp["checkpoint"]["id"];
    for view in ["summary", "action"] {
        let v = p.ok(&["status", "--view", view, "--work", "W"]);
        let progress = &v["current"][0]["progress"];
        assert_eq!(progress["source_next_action"]["text"], "Source action A");
        assert_eq!(
            progress["latest_checkpoint_next_action"]["text"],
            "Checkpoint action B"
        );
        assert_eq!(
            &progress["latest_checkpoint_next_action"]["checkpoint_id"],
            id
        );
        assert_eq!(
            progress["latest_checkpoint_next_action"]["context_hash_verified"],
            false
        );
        assert_eq!(
            progress["latest_checkpoint_next_action"]["actor"]["agent_id"],
            Value::Null
        );
        assert_eq!(
            progress["latest_checkpoint_next_action"]["actor"]["identity_verified"],
            false
        );
        assert_eq!(progress["differs_from_source"], true);
        assert_eq!(v["current"][0]["status"], "in_progress");
    }
    let after = p.ok(&["work", "show", "W"]);
    assert_eq!(after["work"]["next_action"], "Source action A");
    assert_eq!(
        after["source_ref"]["source_revision"],
        before["source_ref"]["source_revision"]
    );
    assert_eq!(
        &after["progress"]["latest_checkpoint_next_action"]["checkpoint_id"],
        id
    );
    assert!(after["progress"]["source_next_action"]["projected_at"].is_i64());
    assert_eq!(fs::read(p.0.join("work.yaml")).unwrap(), source);
    assert!(
        p.ok(&["work", "show", "OTHER"])["progress"]["latest_checkpoint_next_action"].is_null()
    );
    // The ordinary, multi-work human summary must also show current progress.
    let human = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args([
            "--project",
            p.0.to_str().unwrap(),
            "status",
            "--view",
            "summary",
        ])
        .output()
        .unwrap();
    assert!(human.status.success());
    let output = String::from_utf8(human.stdout).unwrap();
    assert!(output.contains("Source next: Source action A"), "{output}");
    assert!(
        output.contains("Checkpoint next: Checkpoint action B"),
        "{output}"
    );
}

#[test]
fn declared_callers_are_recorded_and_mismatched_agents_need_resume() {
    let p = Project::new();
    let s = p.start("agent-a");
    let sid = s["session"]["id"].as_str().unwrap();
    let before = p.revision();
    let rejected = p.run(&[
        "session",
        "checkpoint",
        "--session",
        sid,
        "--agent",
        "agent-b",
        "--context-hash",
        &"a".repeat(64),
        "--digest",
        "Partial work",
        "--next-action",
        "Investigate",
        "--expected-project-revision",
        &before,
    ]);
    assert!(!rejected.status.success());
    let error: Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert_eq!(error["code"], "RuleViolation");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("session resume")
    );
    assert_eq!(p.revision(), before);
    assert!(p.ok(&["session", "show", sid])["checkpoint"].is_null());
    let cp = p.checkpoint(sid, Some("agent-a"));
    assert_eq!(cp["actor"]["agent_id"], "agent-a");
    assert_eq!(cp["actor"]["origin"], "caller_supplied_agent");
    assert_eq!(cp["actor"]["identity_verified"], false);
    let shown = p.ok(&["session", "show", sid]);
    assert_eq!(shown["checkpoint_save"]["actor"], cp["actor"]);
    assert_eq!(
        shown["checkpoint_write_revision"]["value"],
        shown["project_revision"]
    );
    assert_eq!(shown["checkpoint_write_revision"]["scope"], "project");
    let resumed = p.ok(&[
        "session",
        "resume",
        "--session",
        sid,
        "--agent",
        "agent-b",
        "--provider",
        "other-fixture",
        "--model",
        "synthetic",
        "--expected-revision",
        &p.revision(),
    ]);
    let next_sid = resumed["resumed"]["session"]["id"].as_str().unwrap();
    assert_ne!(next_sid, sid);
    let next = p.checkpoint(next_sid, Some("agent-b"));
    assert_eq!(next["actor"]["agent_id"], "agent-b");
    assert_eq!(
        p.ok(&["session", "show", sid])["checkpoint_save"]["actor"]["agent_id"],
        "agent-a"
    );
    let progress = p.ok(&["work", "show", "W"])["progress"].clone();
    assert_eq!(
        progress["latest_checkpoint_next_action"]["session_id"],
        next_sid
    );
}

#[test]
fn incomplete_context_allows_a_truthful_checkpoint_and_revision_errors_name_project_scope() {
    let p = Project::new();
    let path = p.0.join("work.yaml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("goals:\n- id: G\n  title: Ship a useful guide\n  status: active\n  summary: Deliver the reviewed guide\n", "goals: []\n"),
    )
    .unwrap();
    p.ok(&["source", "reindex"]);
    let s = p.start("agent-a");
    let sid = s["session"]["id"].as_str().unwrap();
    let compiled = p.run(&["context", "compile", "--work", "W", "--session", sid]);
    assert!(!compiled.status.success());
    let error: Value = serde_json::from_slice(&compiled.stderr).unwrap();
    assert_eq!(error["code"], "ContextIncomplete");
    assert_eq!(error["details"]["incomplete_checkpoint_allowed"], true);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("incomplete checkpoint")
    );
    let context: Value = serde_json::from_slice(&compiled.stdout).unwrap();
    let hash = context["work_context"]["context_hash"].as_str().unwrap();
    let session_revision = s["session"]["revision"].to_string();
    let mut args = vec![
        "session",
        "checkpoint",
        "--session",
        sid,
        "--agent",
        "agent-a",
        "--context-hash",
        hash,
        "--digest",
        "Context has gaps and tests failed",
        "--next-action",
        "Investigate failing case",
        "--open-loop",
        "Resolve missing goal context",
        "--expected-project-revision",
        &session_revision,
    ];
    let rejected = p.run(&args);
    let error: Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert!(!rejected.status.success());
    assert_eq!(error["code"], "RevisionConflict");
    assert_eq!(error["details"]["revision_scope"], "project");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("not session.revision")
    );
    let rev = p.revision();
    *args.last_mut().unwrap() = &rev;
    let cp = p.ok(&args);
    assert_eq!(cp["context_hash_verified"], false);
    assert!(
        cp["warning"]
            .as_str()
            .unwrap()
            .contains("does not update source-backed work")
    );
    let status = p.ok(&["status", "--view", "summary", "--work", "W"]);
    assert_eq!(
        status["progress"]["latest_checkpoint_next_action"]["text"],
        "Investigate failing case"
    );
    assert_eq!(
        p.ok(&["work", "show", "W"])["work"]["status"],
        "in_progress"
    );
}

#[test]
fn latest_progress_respects_exact_branch_and_remains_visible_after_session_end() {
    let p = Project::new();
    let other = Project::new();
    let main = p.start("agent-main");
    let main_id = main["session"]["id"].as_str().unwrap();
    let main_cp = p.checkpoint(main_id, Some("agent-main"));
    p.ok(&[
        "branch",
        "create",
        "alternate",
        "--actor",
        "fixture",
        "--reason",
        "Explore independently",
        "--expected-revision",
        &p.revision(),
    ]);
    p.ok(&[
        "branch",
        "switch",
        "alternate",
        "--actor",
        "fixture",
        "--reason",
        "Explore independently",
        "--expected-revision",
        &p.revision(),
    ]);
    assert!(p.ok(&["work", "show", "W"])["progress"]["latest_checkpoint_next_action"].is_null());
    let branch = p.start("agent-branch");
    let branch_id = branch["session"]["id"].as_str().unwrap();
    let branch_cp = p.checkpoint(branch_id, Some("agent-branch"));
    p.ok(&[
        "session",
        "end",
        "--session",
        branch_id,
        "--outcome",
        "incomplete",
        "--expected-revision",
        &p.revision(),
    ]);
    assert_eq!(
        p.ok(&["work", "show", "W"])["progress"]["latest_checkpoint_next_action"]["checkpoint_id"],
        branch_cp["checkpoint"]["id"]
    );
    assert_eq!(
        p.ok(&["work", "show", "W", "--branch", "main"])["progress"]["latest_checkpoint_next_action"]
            ["checkpoint_id"],
        main_cp["checkpoint"]["id"]
    );
    assert!(
        other.ok(&["work", "show", "W"])["progress"]["latest_checkpoint_next_action"].is_null()
    );
}

#[test]
fn legacy_receipts_and_shortened_text_do_not_imply_verified_identity() {
    let p = Project::new();
    let s = p.start("old-session-label");
    let sid = s["session"]["id"].as_str().unwrap();
    let cp = p.checkpoint(sid, None);
    let conn = rusqlite::Connection::open(p.0.join(".awr/state.db")).unwrap();
    // Construct the old receipt shape only in this disposable synthetic database.
    // Restore the append-only guard before exercising the application's read path.
    conn.execute_batch("BEGIN; DROP TRIGGER event_no_update;")
        .unwrap();
    conn.execute("UPDATE events SET payload_json=json_remove(payload_json,'$.actor') WHERE event_type='checkpoint.created'",[]).unwrap();
    conn.execute_batch("CREATE TRIGGER event_no_update BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT,'events are append-only'); END; COMMIT;").unwrap();
    drop(conn);
    let shown = p.ok(&["session", "show", sid]);
    assert_eq!(shown["checkpoint"]["id"], cp["checkpoint"]["id"]);
    assert_eq!(shown["checkpoint_save"]["actor"]["origin"], "not_recorded");
    assert!(shown["checkpoint_save"]["actor"]["agent_id"].is_null());
    let text = "Historical source details ".repeat(30) + "Current source target";
    let path = p.0.join("work.yaml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("Source action A", &text),
    )
    .unwrap();
    let status = p.ok(&["status", "--view", "summary", "--work", "W"]);
    assert_eq!(status["progress"]["source_next_action"]["truncated"], true);
    assert!(
        status["progress"]["source_next_action"]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= 240
    );
    assert_eq!(p.ok(&["work", "show", "W"])["work"]["next_action"], text);
    let human = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args([
            "--project",
            p.0.to_str().unwrap(),
            "status",
            "--view",
            "summary",
            "--work",
            "W",
        ])
        .output()
        .unwrap();
    assert!(human.status.success());
    let output = String::from_utf8(human.stdout).unwrap();
    for expected in [
        "Source next:",
        "Checkpoint next:",
        "not declared",
        "unverified",
        "Text shortened",
    ] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    let human = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["--project", p.0.to_str().unwrap(), "work", "show", "W"])
        .output()
        .unwrap();
    assert!(human.status.success());
    let output = String::from_utf8(human.stdout).unwrap();
    assert!(
        output.contains(&text),
        "work detail must preserve the full source action"
    );
    assert_eq!(output.matches("Source next:").count(), 1);
}
