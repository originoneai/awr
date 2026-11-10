use awr_core::*;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl Fixture {
    fn new(history: usize) -> Self {
        let root = std::env::temp_dir().join(format!("awr-action-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join(".awr/project.toml"), "[project]\nname='Daily work'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        let mut source = "goals:\n- id: G\n  title: Publish a useful guide\n  summary: Explain the complete workflow\n  status: active\nwork_items:\n".to_owned();
        for i in 0..history {
            source.push_str(&format!("- id: H{i:03}\n  title: Earlier section {i}\n  status: completed\n  goal: G\n  acceptance: [Reviewed section]\n"));
        }
        for (key, status, extra) in [
            ("W", "in_progress", ""),
            ("R", "ready", ""),
            ("D", "planned", "  depends_on: [W]\n"),
            ("B", "blocked", "  blocker: Needs an agreed scope\n"),
            ("U", "ready", "  depends_on: [MISSING]\n"),
        ] {
            source.push_str(&format!("- id: {key}\n  title: Write {key}\n  status: {status}\n  goal: G\n  acceptance: [Reviewed section]\n  next_action: Review section\n{extra}"));
        }
        fs::write(root.join("work.yaml"), source).unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let indexed =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(indexed.ok);
        Self {
            root,
            store,
            project: indexed.project_id,
        }
    }
    fn revision(&self) -> Revision {
        self.store.project(self.project).unwrap().project_revision
    }
    fn status(&self, args: &[&str]) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.root.to_str().unwrap(), "--json", "status"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn session(&mut self) -> Id {
        self.store
            .start_bound_session(
                self.project,
                self.revision(),
                SessionDraft {
                    work_item_key: Some("W".into()),
                    agent_id: "writer".into(),
                    provider: "fixture".into(),
                    model: "test".into(),
                    branch_id: None,
                    claim: true,
                    claim_ttl_ms: None,
                },
                McpSessionBinding {
                    client: "test".into(),
                    conversation: "daily".into(),
                },
            )
            .unwrap()
            .0
            .session
            .id
    }

    fn text(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .env("AWR_LOCAL_WEB", "0")
            .args(["--project", self.root.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn checkpoint(&mut self, key: &str, next_action: &str) {
        // Keep newer-checkpoint fixtures distinct at millisecond clock resolution.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let session = self
            .store
            .start_session(
                self.project,
                self.revision(),
                SessionDraft {
                    work_item_key: Some(key.into()),
                    agent_id: "writer".into(),
                    provider: "fixture".into(),
                    model: "test".into(),
                    branch_id: None,
                    claim: false,
                    claim_ttl_ms: None,
                },
            )
            .unwrap()
            .0
            .session
            .id;
        self.store
            .create_checkpoint(
                self.project,
                self.revision(),
                session,
                CheckpointDraft {
                    context_hash: "a".repeat(64),
                    digest: "Reviewed the draft".into(),
                    next_action: next_action.into(),
                    open_loops: vec![],
                    changed_entities: vec![],
                },
            )
            .unwrap();
    }

    fn replace_source(&mut self, source: &str) {
        fs::write(self.root.join("work.yaml"), source).unwrap();
        assert!(
            index_project(
                &mut self.store,
                &self.root,
                &Manifest::load(&self.root).unwrap(),
                false,
            )
            .unwrap()
            .ok
        );
    }

    fn legacy() -> Self {
        let mut fixture = Self::new(0);
        fixture.checkpoint("W", "Review the revised draft");
        fixture
    }

    // Only generated identities, source fingerprints, paths and clock readings vary.
    // All field names, business values, whitespace and diagnostic text stay comparable.
    fn normalize(&self, text: &str) -> String {
        let mut text = text.replace(self.root.to_str().unwrap(), "<project>");
        let status = self.status(&[]);
        let locator = status["current"][0]["progress"]["source_next_action"]["locator"]
            .as_str()
            .unwrap();
        text = text.replace(locator, "file://<project>/work.yaml");
        fn collect(value: &Value, replacements: &mut Vec<(String, String)>) {
            match value {
                Value::Object(fields) => {
                    for (key, value) in fields {
                        if matches!(key.as_str(), "projected_at" | "recorded_at")
                            && value.is_number()
                        {
                            replacements.push((value.to_string(), "0".into()));
                        } else {
                            collect(value, replacements);
                        }
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        collect(item, replacements);
                    }
                }
                Value::String(value) if value.parse::<Id>().is_ok() => {
                    replacements.push((value.clone(), "<id>".into()));
                }
                Value::String(value) if value.starts_with("sha256:") => {
                    replacements.push((value.clone(), "<fingerprint>".into()));
                }
                _ => {}
            }
        }
        let mut replacements = vec![];
        collect(&status, &mut replacements);
        for (from, to) in replacements {
            text = text.replace(&from, &to);
        }
        text
    }

    fn normalized_status(&self) -> String {
        self.normalize(&serde_json::to_string_pretty(&self.status(&[])).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn daily_queue_keeps_continuation_separate_from_claims_and_historical_noise() {
    let mut f = Fixture::new(140);
    let session = f.session();
    let before = fs::read(f.root.join("work.yaml")).unwrap();
    let revision = f.revision();
    let value = f.status(&[]);
    assert_eq!(value["view"], "action");
    assert_eq!(value["current_total"], 1);
    assert_eq!(value["ready_count"], 1);
    assert_eq!(value["waiting_count"], 1);
    assert_eq!(value["blocked_count"], 2);
    assert_eq!(
        value["current"][0]["claims"][0]["session"],
        session.to_string()
    );
    assert_eq!(value["history"]["not_checked"], 140);
    assert_eq!(value["history"]["verification_failed"], 0);
    assert_eq!(value["history"]["source_sha"], Value::Null);
    assert!(
        value["organization"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["code"] == "missing_dependency")
    );
    assert!(!value.to_string().contains("H139"));
    assert!(serde_json::to_vec(&value).unwrap().len() < 12_000);
    assert_eq!(f.revision(), revision);
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), before);
    // Navigation does not make an already claimed/in-progress task claimable.
    assert!(
        !f.store
            .work_readiness(f.project, "W", None, now_millis().unwrap())
            .unwrap()
            .ready
    );
    assert!(
        f.store
            .start_session(
                f.project,
                revision,
                SessionDraft {
                    work_item_key: Some("W".into()),
                    agent_id: "other".into(),
                    provider: "fixture".into(),
                    model: "test".into(),
                    branch_id: None,
                    claim: true,
                    claim_ttl_ms: None
                }
            )
            .is_err()
    );
    let scoped = f.status(&["--work", "W"]);
    assert_eq!(scoped["total"], 1);
    assert_eq!(scoped["blocked_count"], 0);
}

#[test]
fn unchecked_history_is_distinct_from_a_real_failed_check_and_keeps_sources_unchanged() {
    let f = Fixture::new(2);
    let before = fs::read(f.root.join("work.yaml")).unwrap();
    let unchecked = f.status(&[]);
    assert_eq!(unchecked["history"]["not_checked"], 2);
    let checked = f.status(&["--source-sha", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]);
    assert_eq!(checked["history"]["not_checked"], 0);
    assert_eq!(checked["history"]["verification_failed"], 2);
    assert_eq!(checked["current_total"], unchecked["current_total"]);
    let full = f.status(&["--view", "full"]);
    assert!(
        full["organization"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["code"] == "completion_not_checked")
    );
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), before);
}

#[test]
fn persistent_user_wait_and_unknown_execution_require_resolution_before_continuation() {
    let mut f = Fixture::new(0);
    let session = f.session();
    let prepared = awr_runtime::prepare_work(
        &mut f.store,
        &f.root,
        &awr_runtime::PrepareWorkRequest {
            work: "W".into(),
            session: Some(session),
            branch: None,
            goals: vec![],
            source_sha: None,
            budget: Some(7000),
        },
    )
    .unwrap();
    let checkpoint = f
        .store
        .create_checkpoint(
            f.project,
            f.revision(),
            session,
            CheckpointDraft {
                context_hash: prepared["context"]["work_context"]["context_hash"]
                    .as_str()
                    .unwrap()
                    .into(),
                digest: "Reviewed current context".into(),
                next_action: "Collect reply".into(),
                open_loops: vec![],
                changed_entities: vec![],
            },
        )
        .unwrap()
        .0;
    let wait = f
        .store
        .create_mcp_wait(
            f.project,
            f.revision(),
            "test",
            session,
            checkpoint.id,
            "Choose the audience".into(),
        )
        .unwrap()
        .0;
    let waiting = f.status(&["--work", "W"]);
    assert_eq!(waiting["current_total"], 0);
    assert_eq!(waiting["waiting_count"], 1);
    assert_eq!(waiting["waiting"][0]["wait_ids"][0], wait.id.to_string());
    f.store
        .reply_mcp_wait(
            f.project,
            f.revision(),
            "test",
            wait.id,
            "New users".into(),
            false,
        )
        .unwrap();
    assert_eq!(f.status(&["--work", "W"])["current_total"], 1);
    let execution = f
        .store
        .register_execution(
            f.project,
            f.revision(),
            session,
            ExecutionIntent {
                operation_key: "writing".into(),
                purpose: "Write section".into(),
                executor: ExecutorKind::External,
                command: vec![],
                cwd: f.root.to_string_lossy().into(),
                external_reference: Some("fixture-writing".into()),
            },
        )
        .unwrap()
        .0;
    let unknown = f.status(&["--work", "W"]);
    assert_eq!(unknown["waiting_count"], 1);
    assert_eq!(unknown["waiting"][0]["execution_total"], 1);
    assert_eq!(unknown["current_total"], 0);
    f.store
        .report_external_execution(
            f.project,
            f.revision(),
            ExternalExecutionReport {
                version: 1,
                request_key: "finished-writing".into(),
                execution_id: execution.id,
                host_id: "test".into(),
                host_work_key: "W".into(),
                native_session: "daily".into(),
                agent_id: "writer".into(),
                origin: ExternalReportOrigin::HostObserved,
                phase: ExternalReportPhase::Succeeded,
                observed_at: now_millis().unwrap(),
                summary: "Section written".into(),
                detail_references: vec![],
            },
        )
        .unwrap();
    assert_eq!(f.status(&["--work", "W"])["current_total"], 1);
    assert_eq!(
        f.store.work_item(f.project, "W").unwrap().item.status,
        WorkStatus::InProgress
    );
    assert_eq!(
        f.store.execution(f.project, execution.id).unwrap().state,
        ExecutionState::Registered
    );
}

#[test]
fn default_status_ten_actions_fit_2048_bytes() {
    let mut fixture = Fixture::new(0);
    let action = "a".repeat(90);
    let mut source = "goals:\n- id: G\n  title: Publish a guide\n  status: active\n  summary: Deliver useful sections\nwork_items:\n".to_owned();
    for i in 0..10 {
        let status = if i < 5 { "in_progress" } else { "ready" };
        source.push_str(&format!("- id: GUIDE-{i:03}\n  title: Write section {i}\n  status: {status}\n  goal: G\n  acceptance: [Reviewed section]\n  next_action: {action}\n"));
    }
    fixture.replace_source(&source);
    fixture.checkpoint("GUIDE-000", &action);
    let output = fixture.text(&["status"]);
    assert!(output.len() <= 2048, "{} bytes: {output}", output.len());
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("  GUIDE-"))
            .count(),
        10
    );
}

#[test]
fn compact_text_omits_runtime_diagnostics() {
    let fixture = Fixture::legacy();
    let status = fixture.status(&["--work", "W"]);
    let progress = &status["progress"];
    for args in [vec!["status"], vec!["status", "--work", "W"], vec!["ready"]] {
        let output = fixture.text(&args);
        for forbidden in [
            "file://".to_owned(),
            "Unix ms".to_owned(),
            "a".repeat(64),
            progress["source_next_action"]["projected_at"].to_string(),
            progress["latest_checkpoint_next_action"]["recorded_at"].to_string(),
            progress["latest_checkpoint_next_action"]["session_id"]
                .as_str()
                .unwrap()
                .into(),
            progress["latest_checkpoint_next_action"]["checkpoint_id"]
                .as_str()
                .unwrap()
                .into(),
        ] {
            assert!(
                !output.contains(&forbidden),
                "{args:?} leaked {forbidden}: {output}"
            );
        }
    }
}

const LEGACY_TEXT: [(&[&str], &str); 3] = [
    (
        &["status", "--view", "full"],
        include_str!("fixtures/status-text/full.json"),
    ),
    (
        &["status", "--view", "summary"],
        include_str!("fixtures/status-text/summary.json"),
    ),
    (
        &["status", "--view", "summary", "--work", "W"],
        include_str!("fixtures/status-text/summary-work.json"),
    ),
];

#[test]
fn legacy_full_and_summary_text_match_baseline_bytes() {
    let fixture = Fixture::legacy();
    for (args, expected) in LEGACY_TEXT {
        let expected: String = serde_json::from_str(expected).unwrap();
        assert_eq!(
            fixture.normalize(&fixture.text(args)).as_bytes(),
            expected.as_bytes(),
            "{args:?}"
        );
    }
}

#[test]
fn legacy_text_fixtures_reject_changed_output() {
    let fixture = Fixture::legacy();
    for (args, expected) in LEGACY_TEXT {
        let expected: String = serde_json::from_str(expected).unwrap();
        let changed = fixture.text(args).replacen("Project:", "Workspace:", 1);
        assert_ne!(
            fixture.normalize(&changed).as_bytes(),
            expected.as_bytes(),
            "{args:?}"
        );
    }
}

#[test]
fn status_json_matches_baseline_fields_and_values() {
    let fixture = Fixture::legacy();
    assert_eq!(
        fixture.normalized_status(),
        include_str!("fixtures/status-text/action.json")
    );
}

#[test]
fn status_json_fixture_rejects_removed_or_renamed_fields() {
    let fixture = Fixture::legacy();
    let expected = include_str!("fixtures/status-text/action.json");
    for field in [
        "current",
        "counts",
        "progress",
        "source_warnings",
        "snapshot",
    ] {
        let mut value = fixture.status(&[]);
        let removed = value.as_object_mut().unwrap().remove(field).unwrap();
        assert_ne!(
            fixture.normalize(&serde_json::to_string_pretty(&value).unwrap()),
            expected
        );
        value[format!("renamed_{field}")] = removed;
        assert_ne!(
            fixture.normalize(&serde_json::to_string_pretty(&value).unwrap()),
            expected
        );
    }
}

#[test]
fn compact_status_and_ready_omit_unused_legends() {
    let fixture = Fixture::new(0);
    for args in [vec!["status"], vec!["ready"]] {
        let output = fixture.text(&args);
        assert!(output.contains("[ledger"), "{output}");
        assert_eq!(
            output.matches(Trust::Source.legend()).count(),
            1,
            "{output}"
        );
        for trust in [
            Trust::Reported,
            Trust::Recorded,
            Trust::Verified,
            Trust::Reviewed,
        ] {
            assert!(!output.contains(trust.legend()), "{output}");
        }
        assert!(!output.contains("[note"), "{output}");
    }
}

#[test]
fn compact_legends_appear_once_for_used_markers() {
    let mut fixture = Fixture::legacy();
    fixture.checkpoint("R", "Review the ready draft");
    for args in [vec!["status"], vec!["ready"]] {
        let output = fixture.text(&args);
        assert_eq!(
            output.matches(Trust::Reported.legend()).count(),
            1,
            "{output}"
        );
        assert_eq!(output.matches("[note] =").count(), 1, "{output}");
    }
    let output = fixture.text(&["status"]);
    assert_eq!(
        output.matches(Trust::Source.legend()).count(),
        1,
        "{output}"
    );
    assert!(!fixture.text(&["ready"]).contains(Trust::Source.legend()));
}

#[test]
fn newer_checkpoint_action_is_marked_in_status_and_ready() {
    let mut fixture = Fixture::legacy();
    fixture.checkpoint("R", "Review the ready draft");
    for (key, action) in [
        ("W", "Review the revised draft"),
        ("R", "Review the ready draft"),
    ] {
        let progress = &fixture.status(&["--work", key])["progress"];
        assert!(
            progress["latest_checkpoint_next_action"]["recorded_at"]
                .as_i64()
                .unwrap()
                > progress["source_next_action"]["projected_at"]
                    .as_i64()
                    .unwrap()
        );
        let output = fixture.text(&["status", "--work", key]);
        let row = output
            .lines()
            .find(|line| line.starts_with(&format!("  {key} ")))
            .unwrap();
        assert!(row.contains(action) && row.contains("[note,"), "{row}");
        assert!(!row.contains("Review section"), "{row}");
    }
    let output = fixture.text(&["ready"]);
    assert!(
        output.lines().any(|line| line.starts_with("  R ")
            && line.contains("Review the ready draft")
            && line.contains("[note,")),
        "{output}"
    );
}

#[test]
fn older_checkpoint_does_not_hide_updated_source_action() {
    let mut fixture = Fixture::legacy();
    fixture.checkpoint("R", "Review the ready draft");
    std::thread::sleep(std::time::Duration::from_millis(2));
    let source = fs::read_to_string(fixture.root.join("work.yaml"))
        .unwrap()
        .replace("Review section", "Review the updated plan");
    fixture.replace_source(&source);
    for args in [vec!["status", "--work", "W"], vec!["ready"]] {
        let output = fixture.text(&args);
        assert!(
            output.contains("Review the updated plan") && output.contains("[ledger,"),
            "{output}"
        );
        assert!(
            !output.contains("Review the revised draft")
                && !output.contains("Review the ready draft"),
            "{output}"
        );
        assert!(!output.contains("[note"), "{output}");
    }
}

#[test]
fn compact_actions_fold_whitespace_and_cap_unicode_at_90_characters() {
    let mut fixture = Fixture::new(0);
    let action = format!("Review\\n  {}", "界".repeat(100));
    let source = fs::read_to_string(fixture.root.join("work.yaml"))
        .unwrap()
        .replace("Review section", &format!("\"{action}\""));
    fixture.replace_source(&source);
    for args in [vec!["status", "--work", "W"], vec!["ready"]] {
        let output = fixture.text(&args);
        let row = output
            .lines()
            .find(|line| line.starts_with("  W ") || line.starts_with("  R "))
            .unwrap();
        let action = row
            .split("  ")
            .filter(|part| !part.is_empty())
            .nth(1)
            .unwrap();
        assert_eq!(action.chars().count(), 90, "{row}");
        assert!(action.ends_with('…'), "{row}");
        assert!(!action.contains('\n'), "{row}");
    }
}
