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
                inputs: Vec::new(),
                input_binding: awr_core::InputBinding::None,
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
