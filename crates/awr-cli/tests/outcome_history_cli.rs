//! DEC-041 against real receipts: the history is read from what the program itself wrote (a completion,
//! a cancellation, a reopening, an ordinary confirmation, supervised executions and usage), so a change in the
//! shape of those receipts shows up here and not as a quietly emptier report.
use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WORK: &str = "goals:\n- id: G\n  title: Deliver the work\n  status: active\n  summary: Deliver the work of this project.\nwork_items:\n- id: W1\n  title: First delivery\n  status: in_progress\n  next_action: Continue the delivery\n  acceptance: [A1]\n- id: W2\n  title: Second delivery\n  status: in_progress\n  next_action: Continue the delivery\n  acceptance: [A2]\n- id: W3\n  title: Third delivery\n  status: in_progress\n  next_action: Continue the delivery\n  acceptance: [A3]\n- id: W4\n  title: Fourth delivery\n  status: in_progress\n  next_action: Continue the delivery\n  acceptance: [A4]\n- id: W5\n  title: Reading\n  status: planned\n  goal: G\n  next_action: Continue the delivery\n  acceptance: [A5]\n- id: W6\n  title: Sixth delivery\n  status: in_progress\n  next_action: Continue the delivery\n  acceptance: [A6]\n";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            root: std::env::temp_dir().join(format!("awr-outcome-history-{}", Id::new())),
        };
        fs::create_dir(&f.root).unwrap();
        fs::write(f.root.join("work.yaml"), WORK).unwrap();
        fs::write(f.root.join("sources.toml"), "[project]\nname='Outcome history'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .env_clear()
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }
    fn store(&self) -> (Store, Id) {
        let store = Store::open_existing(&self.root.join(".awr/state.db")).unwrap();
        let project = store
            .project_by_root(&self.root.canonicalize().unwrap())
            .unwrap()
            .id;
        (store, project)
    }
    /// A session on `work` of this model, holding the claim.
    fn start(&self, work: &str, model: &str) -> String {
        self.ok(&[
            "session",
            "start",
            "--work",
            work,
            "--agent",
            &format!("agent-{model}"),
            "--provider",
            "fixture",
            "--model",
            model,
            "--claim",
            "--expected-revision",
            &self.revision(),
        ])["session"]["id"]
            .as_str()
            .unwrap()
            .into()
    }
    fn evidence(&self, key: &str, work: &str, criterion: &str, level: EvidenceLevel) {
        let report = json!({"version":1,"work_item":work,"source_sha":SHA,"command":"verify","scope":[work],"verified_at":now_millis().unwrap(),"checks":[{"name":"delivery","passed":true,"details":"Checked the delivery","criteria":[criterion]}]});
        let bytes = serde_json::to_vec_pretty(&report).unwrap();
        let locator = format!("report-{key}.json");
        fs::write(self.root.join(&locator), &bytes).unwrap();
        let draft = EvidenceDraft {
            external_key: key.into(),
            work_item_key: Some(work.into()),
            evidence_type: "completion_report".into(),
            level,
            summary: "Checked the delivery".into(),
            locator,
            sha256: Some(
                awr_source::fingerprint(&bytes)
                    .trim_start_matches("sha256:")
                    .into(),
            ),
            source_sha: Some(SHA.into()),
            command: Some("verify".into()),
            scope: vec![work.into()],
            branch_id: None,
            verified_at: report["verified_at"].as_i64(),
        };
        let (mut store, project) = self.store();
        let revision = store.project(project).unwrap().project_revision;
        store.record_evidence(project, revision, draft).unwrap();
    }
    fn complete(&self, work: &str, session: &str, criterion: &str, evidence: &[&str]) {
        let input = json!({"version":1,"source_sha":SHA,"acceptance":[{"criterion":criterion,"evidence":evidence}]});
        let path = self.root.join(format!("completion-{work}.json"));
        fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
        self.ok(&[
            "work",
            "complete",
            work,
            "--session",
            session,
            "--reason",
            "Reviewed the evidence",
            "--input",
            path.to_str().unwrap(),
            "--expected-revision",
            &self.revision(),
        ]);
    }
    fn execution(&self, work_session: &str, key: &str, finish: Option<bool>) {
        let (mut store, project) = self.store();
        let rev = |s: &Store| s.project(project).unwrap().project_revision;
        let revision = rev(&store);
        let (run, _) = store
            .register_execution(
                project,
                revision,
                work_session.parse().unwrap(),
                ExecutionIntent {
                    operation_key: key.into(),
                    purpose: "A supervised command".into(),
                    executor: ExecutorKind::ManagedLocal,
                    command: vec!["worker".into()],
                    cwd: self.root.to_string_lossy().into_owned(),
                    external_reference: None,
                },
            )
            .unwrap();
        let Some(success) = finish else { return };
        let nonce = Id::new();
        let revision = rev(&store);
        store
            .start_execution(
                project,
                revision,
                run.id,
                WorkerIdentity {
                    nonce,
                    pid: 1,
                    port: 1,
                    child_pid: None,
                },
            )
            .unwrap();
        let revision = rev(&store);
        store
            .finish_execution(
                project,
                revision,
                ExecutionResult {
                    execution_id: run.id,
                    nonce,
                    finished_at: now_millis().unwrap(),
                    success,
                    exit_code: Some(if success { 0 } else { 1 }),
                    signal: None,
                    error: None,
                },
            )
            .unwrap();
    }
    /// An ordinary confirmation by a person, saved through the host entry as the program has it.
    fn confirm_ordinary(&self, work: &str) {
        let policy = OrdinaryWorkPolicy {
            version: 1,
            policy_id: "reading-v1".into(),
            authorized_by: "fixture-owner".into(),
            authorized_at: 1,
            reason: "Use ordinary confirmations for the reading work".into(),
            work_items: vec![work.into()],
        };
        let manifest = fs::read_to_string(self.root.join(".awr/project.toml")).unwrap();
        let mut mapping: toml::Value = toml::from_str(&manifest).unwrap();
        mapping["sources"][0]["options"] =
            toml::Value::try_from(json!({"ordinary_work_policy": policy})).unwrap();
        fs::write(
            self.root.join(".awr/project.toml"),
            toml::to_string(&mapping).unwrap(),
        )
        .unwrap();
        self.ok(&["source", "reindex"]);
        let shown = self.ok(&["work", "show", work]);
        let envelope = json!({"version":1,"request_key":"confirm-reading","actor":{"host":"fixture-desktop","subject":"fixture-user","origin":"human"},"reason":"Save the explicit confirmation","change":{"operation":"confirm_ordinary","work":work,"source_fingerprint":shown["source_ref"]["source_fingerprint"],"policy_fingerprint":policy.fingerprint().unwrap(),"kind":"user_confirmation","basis":"Read the note and confirmed the useful ideas are captured","confirmed_at":now_millis().unwrap(),"artifacts":[]}});
        fs::write(self.root.join("save.json"), envelope.to_string()).unwrap();
        self.ok(&[
            "host",
            "save",
            "--input",
            "save.json",
            "--expected-revision",
            &self.revision(),
        ]);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn history(
    f: &Fixture,
    as_of_ms: i64,
    cohort_by: CohortBy,
    keys: Option<Vec<String>>,
) -> OutcomeHistory {
    let (store, project) = f.store();
    let input = store.outcome_history_input(project, None).unwrap();
    aggregate_outcome_history(
        &input,
        &HistoryRequest {
            as_of_ms,
            cohort_by,
            work_keys: keys,
        },
    )
    .unwrap()
}

#[test]
fn the_history_of_real_receipts_counts_each_signal_on_its_own() {
    let f = Fixture::new();
    let (s1, s2, s3, s4, s6) = (
        f.start("W1", "m-a"),
        f.start("W2", "m-a"),
        f.start("W3", "m-b"),
        f.start("W4", "m-b"),
        f.start("W6", "m-c"),
    );
    // W1 and W2 are completed; W2's acceptance is then revised, so its completion is of an earlier standard.
    f.evidence("E1", "W1", "A1", EvidenceLevel::LocallyVerified);
    f.evidence("E1b", "W1", "A1", EvidenceLevel::RealEnvironmentValidated);
    f.complete("W1", &s1, "A1", &["E1", "E1b"]);
    f.evidence("E2", "W2", "A2", EvidenceLevel::LocallyVerified);
    f.complete("W2", &s2, "A2", &["E2"]);
    let text = fs::read_to_string(f.root.join("work.yaml")).unwrap();
    fs::write(
        f.root.join("work.yaml"),
        text.replace("acceptance: [A2]", "acceptance: [A2 revised]"),
    )
    .unwrap();
    f.ok(&["source", "reindex"]);
    // W3 is cancelled; W4 is completed and reopened.
    f.ok(&[
        "work",
        "cancel",
        "W3",
        "--session",
        &s3,
        "--reason",
        "No longer needed",
        "--expected-revision",
        &f.revision(),
    ]);
    f.evidence("E4", "W4", "A4", EvidenceLevel::LocallyVerified);
    f.complete("W4", &s4, "A4", &["E4"]);
    f.ok(&[
        "work",
        "reopen",
        "W4",
        "--session",
        &s4,
        "--reason",
        "A new requirement",
        "--next-action",
        "Replan it",
        "--expected-revision",
        &f.revision(),
    ]);
    // W5 is confirmed by a person; W6 only has a command still running.
    f.confirm_ordinary("W5");
    f.execution(&s1, "run-w1", Some(true));
    f.execution(&s3, "run-w3", Some(false));
    f.execution(&s6, "run-w6", None);

    let as_of = now_millis().unwrap() + 1;
    let report = history(&f, as_of, CohortBy::ProviderModel, None);
    let value = serde_json::to_value(&report).unwrap();
    // Every receipt the program wrote is read, each against a version of its work.
    assert_eq!(report.sample.works_in_scope, 6, "{value:#}");
    assert_eq!(report.sample.receipts.used, 6, "{value:#}");
    assert_eq!(report.sample.receipts.without_version, 0, "{value:#}");
    assert_eq!(report.sample.executions.used, 3, "{value:#}");
    assert_eq!(report.cohorts.len(), 4, "{value:#}");

    let model = |name: &str| {
        report
            .cohorts
            .iter()
            .find(|c| c.key.model.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no cohort {name}: {value:#}"))
    };
    let a = model("m-a");
    assert_eq!(a.works, 2);
    assert_eq!(a.signals.completion_reported, 1, "{value:#}");
    assert_eq!(a.signals.completion_under_earlier_standard, 1, "{value:#}");
    // The highest level a completion bound, per completion: W1 bound two records, W2 one.
    assert_eq!(
        a.signals.completion_by_evidence_level["real_environment_validated"],
        1
    );
    assert_eq!(
        a.signals.completion_by_evidence_level["locally_verified"],
        1
    );
    assert_eq!(a.executions.succeeded, 1);
    let b = model("m-b");
    assert_eq!(b.signals.cancelled, 1, "{value:#}");
    assert_eq!(b.signals.open, 1, "{value:#}");
    assert_eq!(b.signals.reopened_works, 1);
    assert_eq!(b.signals.withdrawn_completions, 1);
    assert_eq!(b.signals.completion_reported, 0);
    assert_eq!(b.executions.failed, 1);
    let c = model("m-c");
    assert_eq!(
        (c.signals.open, c.executions.unfinished),
        (1, 1),
        "{value:#}"
    );
    let nobody = report.cohorts.iter().find(|c| c.unattributed).unwrap();
    assert_eq!(nobody.signals.accepted_by_human, 1, "{value:#}");
    assert_eq!(nobody.signals.completion_reported, 0);

    // Not one number of the report is a rate, and what is not derived is said.
    assert_eq!(report.coverage.works_with_outcome, (4, 6), "{value:#}");
    assert_eq!(report.coverage.works_with_execution, (3, 6));
    assert_eq!(report.coverage.works_with_usage, (0, 6));
    assert_eq!(
        report.not_derived,
        [
            "model_success_rate",
            "dollar_savings",
            "semantic_understanding"
        ]
    );
    assert!(
        report
            .cohorts
            .iter()
            .all(|c| c.cost.actual_micros.is_empty() && c.cost.works_without_usage == c.works)
    );

    // The same question asked again gives the same report; a filter limits the works and says what it did not find.
    assert_eq!(
        report.history_hash,
        history(&f, as_of, CohortBy::ProviderModel, None).history_hash
    );
    let only = history(
        &f,
        as_of,
        CohortBy::None,
        Some(vec!["W1".into(), "W9".into()]),
    );
    assert_eq!(only.sample.works_in_scope, 1);
    assert_eq!(only.scope.unknown_work_keys, ["W9"]);
    assert_eq!(only.cohorts[0].signals.completion_reported, 1);
    assert_eq!(only.sample.receipts.outside_filter, 5);
}

#[test]
fn a_branch_has_its_own_history_and_the_main_line_does_not_borrow_it() {
    let f = Fixture::new();
    let session = f.start("W2", "m-a");
    f.ok(&[
        "work",
        "cancel",
        "W2",
        "--session",
        &session,
        "--reason",
        "No longer needed",
        "--expected-revision",
        &f.revision(),
    ]);
    f.execution(&session, "run-w2", Some(true));
    let branch: Id = f.ok(&[
        "branch",
        "create",
        "review",
        "--actor",
        "fixture",
        "--reason",
        "Review the work",
        "--expected-revision",
        &f.revision(),
    ])["branch"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // A receipt written on the branch, as a session working there would leave it.
    rusqlite::Connection::open(f.root.join(".awr/state.db"))
        .unwrap()
        .execute(
            "INSERT INTO events(id,project_id,work_item_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at)
             SELECT ?1, w.project_id, w.id, ?2, 'work.cancelled', 'normal', 'synthetic', '{}',
                    (SELECT project_revision FROM projects WHERE id=w.project_id), ?3
             FROM work_items w WHERE w.external_key='W3'",
            rusqlite::params![Id::new().to_string(), branch.to_string(), now_millis().unwrap()],
        )
        .unwrap();
    let (store, project) = f.store();
    let main = store.outcome_history_input(project, None).unwrap();
    let side = store.outcome_history_input(project, Some(branch)).unwrap();
    let keys = |input: &HistoryInput| -> Vec<Id> {
        input.receipts.iter().map(|r| r.work_item_id).collect()
    };
    let id_of = |key: &str| store.work_item(project, key).unwrap().item.meta.id;
    assert_eq!(keys(&main), [id_of("W2")]);
    assert_eq!(keys(&side), [id_of("W3")]);
    assert_eq!((main.executions.len(), side.executions.len()), (1, 0));
}
