use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const WORK: &str = "work_items:\n- id: W\n  title: Current work\n  status: in_progress\n  next_action: Read the current inputs\n  acceptance: [Keep the exact criteria]\n- id: NEXT\n  title: Next work\n  status: ready\n  next_action: Read the current inputs\n  acceptance: [Keep the next criteria]\n";
const PLAN: &str =
    "[[sources]]\ndomain='plan'\nrole='primary'\npath='plan.md'\nadapter='markdown-heading-v1'\n";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-drill-cli-{}", Id::new()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(
            root.join("rules.md"),
            "# Rule {#rule severity=hard scope=project value=*}\n\nKeep required facts exactly.\n",
        )
        .unwrap();
        fs::write(
            root.join("goal.md"),
            "# Durable runtime\n\nRead the required context.\n",
        )
        .unwrap();
        fs::write(
            root.join("plan.md"),
            "# Implement the runtime\n\nRetain the source references.\n",
        )
        .unwrap();
        fs::write(root.join("sources.toml"),format!("[project]\nname='Drill down fixture'\nexternal_key='drill'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n{PLAN}")).unwrap();
        let f = Self(root);
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
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
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn fails(&self, args: &[&str], code: &str) {
        let output = self.run(args);
        assert!(
            !output.status.success() && output.stdout.is_empty(),
            "{args:?}"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            code
        );
    }
    fn revision(&self) -> String {
        self.ok(&["status"])["project_revision"].to_string()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn omitted_goal_resolves_by_id_and_version_and_source_content_stays_fingerprint_bound() {
    let f = Fixture::new();
    let body = format!(
        "# Durable runtime\n\n{}OPTIONAL_BODY_TAIL\n",
        "Historical context detail. ".repeat(10000)
    );
    fs::write(f.0.join("goal.md"), &body).unwrap();
    let report = f.ok(&["context", "compile", "--work", "W"]);
    let omitted = report["work_context"]["omitted_chunks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"].as_str().unwrap().starts_with("goal-body:"))
        .unwrap();
    let entity = &omitted["entities"][0];
    assert_eq!(entity["kind"], "goal");
    let id = entity["id"].as_str().unwrap();
    let revision = entity["revision"].to_string();
    assert!(!report.to_string().contains("OPTIONAL_BODY_TAIL"));
    let summary = f.ok(&["object", "show", "goal", id, "--entity-revision", &revision]);
    assert_eq!(summary["content_included"], false);
    assert!(!summary.to_string().contains("OPTIONAL_BODY_TAIL"));
    let full = f.ok(&[
        "object",
        "show",
        "goal",
        id,
        "--full",
        "--max-bytes",
        "524288",
        "--entity-revision",
        &revision,
    ]);
    assert!(
        full["object"]["summary"]
            .as_str()
            .unwrap()
            .ends_with("OPTIONAL_BODY_TAIL")
    );
    let source_id = full["source"]["id"].as_str().unwrap();
    let fp = full["source"]["fingerprint"].as_str().unwrap();
    let source_revision = full["source"]["revision"].to_string();
    let source = f.ok(&["source", "show", source_id]);
    assert_eq!(source["content_included"], false);
    let raw = f.ok(&[
        "source",
        "show",
        source_id,
        "--content",
        "--max-bytes",
        "524288",
        "--fingerprint",
        fp,
        "--source-revision",
        &source_revision,
    ]);
    assert_eq!(raw["content"], body);
    assert_eq!(raw["content_fingerprint_verified"], true);
    let lines = f.ok(&[
        "source",
        "show",
        source_id,
        "--content",
        "--max-bytes",
        "524288",
        "--start-line",
        "1",
        "--end-line",
        "2",
    ]);
    assert_eq!(lines["content"], "# Durable runtime\n\n");
    f.fails(
        &[
            "source",
            "show",
            source_id,
            "--content",
            "--max-bytes",
            "20",
        ],
        "InvalidInput",
    );
    fs::write(
        f.0.join("goal.md"),
        body.replace("OPTIONAL_BODY_TAIL", "REVISED_BODY_TAIL"),
    )
    .unwrap();
    f.fails(
        &[
            "source",
            "show",
            source_id,
            "--content",
            "--max-bytes",
            "524288",
        ],
        "SourceConflict",
    );
    f.fails(
        &[
            "object",
            "show",
            "goal",
            id,
            "--full",
            "--entity-revision",
            &revision,
        ],
        "RevisionConflict",
    );
    f.fails(
        &["source", "show", source_id, "--fingerprint", fp],
        "SourceConflict",
    );
    fs::remove_file(f.0.join(".awr/project.toml")).unwrap();
    let cached = f.ok(&[
        "object",
        "show",
        "goal",
        id,
        "--cached",
        "--full",
        "--max-bytes",
        "524288",
    ]);
    assert_eq!(cached["source_refresh_performed"], false);
    assert!(
        cached["object"]["summary"]
            .as_str()
            .unwrap()
            .contains("REVISED_BODY_TAIL")
    );
}

#[test]
fn checkpoint_delta_source_history_and_immutable_events_form_a_traceable_chain() {
    let f = Fixture::new();
    let session = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "executor",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--expected-revision",
        &f.revision(),
    ]);
    let sid = session["session"]["id"].as_str().unwrap();
    let context = f.ok(&["context", "compile", "--session", sid]);
    let cp = f.ok(&[
        "session",
        "checkpoint",
        "--session",
        sid,
        "--context-hash",
        context["work_context"]["context_hash"].as_str().unwrap(),
        "--digest",
        "Progress worth retaining",
        "--next-action",
        "Read the updated input",
        "--open-loop",
        "Inspect remaining source changes",
        "--expected-revision",
        &f.revision(),
    ]);
    let cpid = cp["checkpoint"]["id"].as_str().unwrap();
    let cpread = f.ok(&[
        "object",
        "show",
        "checkpoint",
        cpid,
        "--full",
        "--entity-revision",
        &cp["checkpoint"]["revision"].to_string(),
    ]);
    assert_eq!(cpread["object"], cp["checkpoint"]);
    fs::write(
        f.0.join("work.yaml"),
        WORK.replace("Read the current inputs", "Read the revised inputs"),
    )
    .unwrap();
    f.revision();
    let event_id = {
        let mut store = Store::open(&f.0.join(".awr/state.db")).unwrap();
        let project = store.project_by_root(&f.0).unwrap();
        let mut event = EventDraft::new("work.observed", "Review the changed input");
        event.session_id = Some(sid.parse().unwrap());
        event.importance = "critical".into();
        event.payload = json!({"body":"RAW_EVENT_BODY".repeat(1000)});
        store
            .append_event(project.id, project.project_revision, event)
            .unwrap()
            .id
    };
    let delta = f.ok(&[
        "context",
        "delta",
        "--session",
        sid,
        "--checkpoint",
        cpid,
        "--entity-limit",
        "1",
    ]);
    assert_eq!(delta["source_refresh_ok"], true);
    assert_eq!(delta["delta"]["checkpoint_id"], cpid);
    assert!(!delta.to_string().contains("RAW_EVENT_BODY"));
    let changes = delta["delta"]["events"]["source_changes"]
        .as_array()
        .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["changed_entity_count"], 2);
    assert_eq!(changes[0]["omitted_entities"], 1);
    let source_id = changes[0]["source_id"].as_str().unwrap();
    let after = delta["delta"]["after_revision"].to_string();
    let through = delta["delta"]["events"]["project_revision"].to_string();
    let mut page = f.ok(&[
        "source",
        "history",
        source_id,
        "--after-revision",
        &after,
        "--through-revision",
        &through,
        "--limit",
        "1",
    ]);
    let mut events = Vec::new();
    loop {
        for e in page["events"].as_array().unwrap() {
            assert_eq!(e["source_id"], source_id);
            assert!(e.get("payload").is_none());
            let full = f.ok(&["event", "show", e["id"].as_str().unwrap(), "--full"]);
            assert_eq!(full["event"]["payload"]["source_id"], source_id);
            assert_eq!(full["event"]["payload"]["change_schema"], 1);
            events.push(e["id"].clone());
        }
        if page["next_cursor"].is_null() {
            break;
        }
        page = f.ok(&[
            "source",
            "history",
            source_id,
            "--cursor",
            &page["next_cursor"].to_string(),
            "--through-revision",
            &through,
            "--limit",
            "1",
        ]);
    }
    assert!(events.contains(&changes[0]["last_event"]["id"]));
    assert!(events.len() >= 2);
    let event_id = event_id.to_string();
    let brief = f.ok(&["event", "show", &event_id]);
    assert!(!brief.to_string().contains("RAW_EVENT_BODY"));
    assert!(brief["event"]["source_id"].is_null());
    f.fails(
        &["event", "show", &event_id, "--full", "--max-bytes", "100"],
        "InvalidInput",
    );
    let full = f.ok(&["event", "show", &event_id, "--full"]);
    assert_eq!(
        full["event"]["payload"]["body"],
        "RAW_EVENT_BODY".repeat(1000)
    );
    let work_history = f.ok(&[
        "event",
        "history",
        "--work",
        "W",
        "--session",
        sid,
        "--after-revision",
        &after,
        "--through-revision",
        &through,
    ]);
    assert!(
        work_history["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == event_id)
    );
    assert!(!work_history.to_string().contains("RAW_EVENT_BODY"));
    f.fails(
        &[
            "source",
            "history",
            source_id,
            "--through-revision",
            "999999",
        ],
        "InvalidInput",
    );
    f.fails(
        &["context", "delta", "--work", "ABSENT"],
        "ContextIncomplete",
    );
    fs::remove_file(f.0.join("rules.md")).unwrap();
    let failed = f.run(&["context", "delta", "--session", sid]);
    assert!(!failed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stdout).unwrap()["source_refresh_ok"],
        false
    );
}

#[test]
fn retired_source_metadata_and_history_remain_readable_without_the_manifest() {
    let f = Fixture::new();
    let plan = f.ok(&["source", "show", "plan"]);
    let id = plan["source"]["id"].as_str().unwrap();
    let source_revision = plan["source"]["revision"].to_string();
    let manifest = f.0.join(".awr/project.toml");
    fs::write(
        &manifest,
        fs::read_to_string(f.0.join("sources.toml"))
            .unwrap()
            .replace(PLAN, ""),
    )
    .unwrap();
    f.ok(&["source", "reindex"]);
    let retired = f.ok(&["source", "show", id]);
    assert_eq!(retired["active"], false);
    f.fails(
        &["source", "show", id, "--source-revision", &source_revision],
        "RevisionConflict",
    );
    fs::remove_file(manifest).unwrap();
    let history = f.ok(&["source", "history", id]);
    assert!(
        history["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "source.retired")
    );
    assert_eq!(f.ok(&["source", "show", id])["active"], false);
    f.fails(&["source", "show", "plan"], "NotFound");
    f.fails(&["source", "show", id, "--content"], "SourceUnavailable");
}

#[test]
fn malformed_event_history_cursor_is_reported_as_invalid_input() {
    let f = Fixture::new();
    let cursor = r#"{"secret_marker":"S3CR3T_CURSOR_VALUE"}"#;
    let output = f.run(&["event", "history", "--work", "W", "--cursor", cursor]);
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
