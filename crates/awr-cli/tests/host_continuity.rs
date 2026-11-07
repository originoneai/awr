//! Synthetic host contract checks; these do not launch Kimi/Grok or claim native acceptance.
use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::Arc,
};

struct Host(PathBuf);
impl Host {
    fn new() -> Self {
        let h = Self(std::env::temp_dir().join(format!("awr 宿主续接 {}", Id::new())));
        fs::create_dir(&h.0).unwrap();
        h.write("work.yaml","goals:\n- id: G\n  title: Publish a guide\n  status: active\n  success_criteria: [A useful guide]\nwork_items:\n- id: W\n  title: Draft the guide\n  status: ready\n  goal: G\n  acceptance: [A useful guide]\n  next_action: Write the outline\n");
        h.write("mapping.toml","[project]\nname='Continuity'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n");
        h.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        h
    }
    fn write(&self, path: &str, body: &str) {
        fs::write(self.0.join(path), body).unwrap();
    }
    fn run(&self, args: &[&str], input: Option<Value>) -> Output {
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
                .write_all(&serde_json::to_vec(&input).unwrap())
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
        let r = self.run(args, None);
        assert!(!r.status.success(), "{args:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stderr).unwrap()["code"],
            code
        );
    }
    fn revision(&self) -> String {
        self.ok(&["session", "list"])["project_revision"].to_string()
    }
    fn bind(&self, external: &str) -> Value {
        self.ok(&[
            "client",
            "bind",
            "--client",
            "generic",
            "--external-session",
            external,
            "--work",
            "W",
            "--model",
            "fixture-model",
        ])
    }
    fn hook(&self, external: &str, event: &str, turn: &str) -> Value {
        decode(self.run(
            &["client", "hook", "--client", "generic", "--work", "W"],
            Some(
                json!({"session_id":external,"hook_event_name":event,"turn_id":turn,"cwd":self.0}),
            ),
        ))
    }
    fn register(&self, session: &str, key: &str) -> Value {
        self.ok(&[
            "execution",
            "register",
            "--session",
            session,
            "--key",
            key,
            "--purpose",
            "Draft the guide",
            "--reference",
            "host://fixture/execution/guide",
        ])
    }
    fn report(&self, execution: &Value, key: &str, phase: &str) -> Value {
        json!({"version":1,"request_key":key,"execution_id":execution["execution"]["id"],"host_id":"fixture-host","host_work_key":"workspace/guide","native_session":"fixture-provider/first-conversation","agent_id":"fixture-author","origin":"host_observed","phase":phase,"observed_at":now_millis().unwrap(),"summary":"Host observed a guide drafting stage","detail_references":["host://fixture/logs/guide","host://fixture/artifacts/guide"]})
    }
    fn submit(&self, report: &Value, rev: &str) -> Value {
        self.write("report.json", &report.to_string());
        self.ok(&[
            "execution",
            "report",
            "--input",
            "report.json",
            "--expected-revision",
            rev,
        ])
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn decode(r: Output) -> Value {
    assert!(
        r.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&r.stdout),
        String::from_utf8_lossy(&r.stderr)
    );
    serde_json::from_slice(&r.stdout).unwrap()
}

#[test]
fn external_reports_are_idempotent_queryable_and_never_promote_execution_success() {
    let h = Host::new();
    let b = h.bind("fixture-provider/first-conversation");
    let s = b["binding"]["session_id"].as_str().unwrap();
    let registered = h.register(s, "guide-execution");
    let id = registered["execution"]["id"].as_str().unwrap();
    assert_eq!(registered["outcome_verified"], false);
    let rev = h.revision();
    let report = h.report(&registered, "stage-1", "succeeded");
    let saved = h.submit(&report, &rev);
    assert_eq!(saved["created"], true);
    assert_eq!(saved["outcome_verified"], false);
    assert_eq!(saved["event"]["session_id"], s);
    assert_eq!(
        saved["event"]["work_item_id"],
        registered["execution"]["work_item_id"]
    );
    let replay = h.submit(&report, &rev);
    assert_eq!(replay["created"], false);
    assert_eq!(replay["event"], saved["event"]);
    assert_eq!(replay["project_revision"], saved["project_revision"]);
    let status = h.ok(&["execution", "report-status", "--key", "stage-1"]);
    assert_eq!(status["event"], saved["event"]);
    assert_eq!(status["side_effects_performed"], false);
    assert_eq!(
        h.ok(&["execution", "report-status", "--key", "not-recorded"])["found"],
        false
    );
    let inspected = h.ok(&["execution", "inspect", id]);
    assert_eq!(inspected["observation"]["state"], "unknown");
    assert_eq!(inspected["observation"]["verified"], false);
    assert_eq!(inspected["external_report"]["id"], saved["event"]["id"]);
    let shown = h.ok(&["execution", "show", id]);
    assert_eq!(shown["execution"], registered["execution"]);
    assert_eq!(shown["external_report"]["payload"]["report"], report);
    let mut changed = report.clone();
    changed["phase"] = json!("failed");
    h.write("report.json", &changed.to_string());
    h.error(
        &[
            "execution",
            "report",
            "--input",
            "report.json",
            "--expected-revision",
            &h.revision(),
        ],
        "SourceConflict",
    );
    h.error(
        &[
            "execution",
            "run",
            "--session",
            s,
            "--key",
            "guide-execution",
            "--purpose",
            "Draft the guide",
            "--",
            "forbidden-replay",
        ],
        "SourceConflict",
    );
    assert!(h.ok(&["execution", "show", id])["execution"]["worker"].is_null());
    assert_eq!(h.ok(&["work", "show", "W"])["work"]["status"], "ready");
    assert!(!h.0.join(".awr/executions").join(id).exists());
    h.ok(&[
        "session",
        "end",
        "--session",
        s,
        "--outcome",
        "ended",
        "--expected-revision",
        &h.revision(),
    ]);
    let ended = h.report(&registered, "stage-late", "interrupted");
    h.submit(&ended, &h.revision());
    assert_eq!(h.ok(&["session", "show", s])["session"]["status"], "ended");
}

#[test]
fn duplicate_concurrent_report_delivery_creates_one_receipt() {
    let h = Arc::new(Host::new());
    let b = h.bind("parallel-host");
    let r = h.register(
        b["binding"]["session_id"].as_str().unwrap(),
        "parallel-exec",
    );
    h.write(
        "report.json",
        &h.report(&r, "one-report", "progress").to_string(),
    );
    let revision = h.revision();
    let jobs = (0..4)
        .map(|_| {
            let h = h.clone();
            let revision = revision.clone();
            std::thread::spawn(move || {
                h.ok(&[
                    "execution",
                    "report",
                    "--input",
                    "report.json",
                    "--expected-revision",
                    &revision,
                ])
            })
        })
        .collect::<Vec<_>>();
    let values = jobs
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(values.iter().filter(|v| v["created"] == true).count(), 1);
    assert!(
        values
            .iter()
            .all(|v| v["event"]["id"] == values[0]["event"]["id"])
    );
}

#[test]
fn generic_host_handoff_uses_fresh_context_and_keeps_external_outcomes_unknown() {
    let h = Host::new();
    let bound = h.bind("provider-a/native-one");
    let sid = bound["binding"]["session_id"].as_str().unwrap();
    let registered = h.register(sid, "first-exec");
    h.ok(&[
        "client",
        "progress",
        "--client",
        "generic",
        "--external-session",
        "provider-a/native-one",
        "--digest",
        "Drafted the guide outline",
        "--next-action",
        "Review the guide examples",
        "--open-loop",
        "Check the external tool result before retrying",
    ]);
    let cp = h.hook("provider-a/native-one", "Stop", "stage-one");
    let duplicate = h.hook("provider-a/native-one", "Stop", "stage-one");
    assert_eq!(cp["awr"]["checkpoint_saved"], true);
    assert_eq!(duplicate["awr"]["duplicate"], true);
    assert_eq!(
        cp["awr"]["binding"]["checkpoint_id"],
        duplicate["awr"]["binding"]["checkpoint_id"]
    );
    let checkpoint = cp["awr"]["binding"]["checkpoint_id"].as_str().unwrap();
    let read = h.ok(&["object", "show", "checkpoint", checkpoint, "--full"]);
    assert!(read.to_string().contains("Review the guide examples"));
    let original = fs::read_to_string(h.0.join("work.yaml")).unwrap();
    h.write(
        "work.yaml",
        &original.replace("Write the outline", "Include revised examples"),
    );
    let revision = h.revision();
    let sessions = h.ok(&["session", "list"]);
    let inspect = h.ok(&["recovery", "inspect", "--session", sid]);
    assert_eq!(inspect["side_effects_performed"], false);
    assert_eq!(inspect["source_refresh_performed"], false);
    assert_eq!(inspect["checkpoint"]["id"], checkpoint);
    assert_eq!(inspect["executions"][0]["state"], "unknown");
    let diagnosis = h.run(&["doctor"], None);
    assert!(!diagnosis.status.success());
    let doctor: Value = serde_json::from_slice(&diagnosis.stdout).unwrap();
    assert_eq!(doctor["read_only"], true);
    assert!(
        doctor["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "source_fingerprint_changed")
    );
    assert_eq!(h.revision(), revision);
    assert_eq!(h.ok(&["session", "list"]), sessions);
    let successor = h.ok(&[
        "client",
        "bind",
        "--client",
        "generic",
        "--external-session",
        "provider-b/native-two",
        "--work",
        "W",
        "--from-session",
        sid,
        "--model",
        "fixture-successor",
    ]);
    assert_ne!(successor["binding"]["session_id"], sid);
    assert_eq!(
        successor["binding"]["next_action"],
        "Review the guide examples"
    );
    assert!(
        successor["context"]
            .to_string()
            .contains("Include revised examples")
    );
    let executions = h.ok(&["execution", "list", "--work", "W"]);
    assert_eq!(executions["executions"].as_array().unwrap().len(), 1);
    assert_eq!(executions["executions"][0], registered["execution"]);
    assert!(!h.0.join(".codex").exists());
    assert!(!h.0.join(".kimi").exists());
}

#[test]
fn source_failure_does_not_lose_reports_and_reports_cannot_adopt_foreign_or_managed_work() {
    let h = Host::new();
    let bound = h.bind("host-source-failure");
    let sid = bound["binding"]["session_id"].as_str().unwrap();
    let r = h.register(sid, "external-source-failure");
    let proposal = h.ok(&[
        "proposal",
        "create",
        "--kind",
        "work",
        "--target",
        "W",
        "--intent",
        "Revise next action",
        "--patch",
        r#"{"next_action":"Read the report"}"#,
        "--expected-revision",
        &h.revision(),
    ]);
    let inspected = h.ok(&["recovery", "inspect", "--session", sid]);
    assert!(
        inspected["runtime_findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |f| f["code"] == "pending_mutation" && f["object_id"] == proposal["proposal"]["id"]
            )
    );
    h.write("work.yaml", "work_items: [broken\n");
    let before = fs::read(h.0.join("work.yaml")).unwrap();
    h.submit(
        &h.report(&r, "after-source-failure", "unknown"),
        &h.revision(),
    );
    assert_eq!(fs::read(h.0.join("work.yaml")).unwrap(), before);
    let other = Host::new();
    other.write(
        "report.json",
        &h.report(&r, "foreign", "progress").to_string(),
    );
    other.error(
        &[
            "execution",
            "report",
            "--input",
            "report.json",
            "--expected-revision",
            &other.revision(),
        ],
        "NotFound",
    );
    let mut store = Store::open_existing(&h.0.join(".awr/state.db")).unwrap();
    let project = store.project_by_root(&h.0).unwrap();
    let managed = store
        .register_execution(
            project.id,
            project.project_revision,
            sid.parse().unwrap(),
            ExecutionIntent {
                operation_key: "managed-intent-only".into(),
                purpose: "A fixture intent; never dispatched".into(),
                executor: ExecutorKind::ManagedLocal,
                command: vec!["not-dispatched".into()],
                cwd: h.0.to_string_lossy().into(),
                external_reference: None,
                inputs: Vec::new(),
                input_binding: awr_core::InputBinding::None,
            },
        )
        .unwrap()
        .0;
    drop(store);
    let mut forged = h.report(&r, "managed-report", "succeeded");
    forged["execution_id"] = json!(managed.id);
    h.write("report.json", &forged.to_string());
    h.error(
        &[
            "execution",
            "report",
            "--input",
            "report.json",
            "--expected-revision",
            &h.revision(),
        ],
        "InvalidTransition",
    );
    forged["worker"] = json!({"pid":123});
    h.write("report.json", &forged.to_string());
    h.error(
        &[
            "execution",
            "report",
            "--input",
            "report.json",
            "--expected-revision",
            &h.revision(),
        ],
        "InvalidInput",
    );
}
