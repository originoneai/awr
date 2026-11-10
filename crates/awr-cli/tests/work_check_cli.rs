//! Direct checks use independent synthetic projects and the real CLI.
use awr_core::Id;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::Duration,
};

const C1: &str = "The saved draft remains available";
const C2: &str = "The delivery is independently reviewed";

struct Fixture {
    root: PathBuf,
    session: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    fn new(count: usize, unmapped: bool) -> Self {
        let mut f = Self {
            root: std::env::temp_dir().join(format!("awr-check-{}", Id::new())),
            session: String::new(),
        };
        fs::create_dir_all(&f.root).unwrap();
        fs::write(f.root.join("oracle.txt"), "original oracle\n").unwrap();
        fs::write(f.root.join("probe.json"), r#"{"mode":"pass"}"#).unwrap();
        let examples: Vec<_> = (0..count).map(|i| json!({
            "id":format!("draft-{i}"), "criterion":C1, "kind":if i % 2 == 0 {"positive"} else {"negative"},
            "given":"a pending draft", "when":"saving is checked", "then":"the draft remains",
            "check":{"run":[std::env::current_exe().unwrap().to_str().unwrap(),"--ignored","--exact","check_process_fixture","--nocapture"],"files":["oracle.txt"],"timeout_seconds":10}
        })).collect();
        fs::write(f.root.join("work.yaml"), serde_json::to_vec_pretty(&json!({
            "goals":[{"id":"G","title":"Keep drafts","status":"active","summary":"Retain the user's draft","success_criteria":[C1]}],
            "work_items":[{"id":"W","title":"Retain drafts","status":"in_progress","goal":"G","acceptance":if unmapped {vec![C1,C2]} else {vec![C1]},
                "definition":{"version":1,"purpose":"implementation","tier":"required", "examples":examples}},
                {"id":"OTHER","title":"Other work","status":"planned","goal":"G","acceptance":["Other result"]}]
        })).unwrap()).unwrap();
        fs::write(f.root.join("mapping.toml"), "[project]\nname='Checks fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        fs::write(
            f.root.join(".gitignore"),
            ".awr/\nprobe.json\nchild.pid\ncompletion.json\nmanual.json\n",
        )
        .unwrap();
        f.git(&["init"]);
        f.git(&["config", "user.email", "checks@example.invalid"]);
        f.git(&["config", "user.name", "Check fixture"]);
        f.git(&[
            "add",
            "work.yaml",
            "mapping.toml",
            "oracle.txt",
            ".gitignore",
        ]);
        f.git(&["commit", "-m", "Create check fixture"]);
        f.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        f.git(&["add", ".gitignore"]);
        f.git(&["commit", "-m", "Record initialized ignore rules"]);
        let start = f.ok(&[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "checker",
            "--provider",
            "fixture",
            "--model",
            "local",
            "--claim",
            "--expected-revision",
            &f.revision(),
        ]);
        f.session = start["session"]["id"].as_str().unwrap().into();
        f
    }
    fn git(&self, args: &[&str]) -> String {
        let o = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap().trim().into()
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .current_dir(&self.root)
            .env("AWR_LOCAL_WEB", "0")
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }
    fn check(&self, phase: &str, extra: &[&str]) -> Output {
        let r = self.revision();
        let mut a = vec![
            "work",
            "check",
            "W",
            "--session",
            &self.session,
            "--phase",
            phase,
            "--expected-revision",
            &r,
        ];
        a.extend_from_slice(extra);
        self.run(&a)
    }
    fn checked(&self, phase: &str, extra: &[&str]) -> Value {
        let o = self.check(phase, extra);
        assert!(
            o.status.success(),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn mode(&self, mode: &str) {
        fs::write(
            self.root.join("probe.json"),
            serde_json::to_vec(&json!({"mode":mode})).unwrap(),
        )
        .unwrap();
    }
    fn governed(&self) {
        let path = self.root.join(".awr/project.toml");
        let mut s = fs::read_to_string(&path).unwrap();
        s.push_str("\n[sources.options.admission_policy]\nversion=1\nmode='governed_enforce'\nscope='all'\n");
        fs::write(path, s).unwrap();
        self.ok(&["source", "reindex"]);
    }
    fn event(&self, result: &Value) -> Value {
        let store = awr_store::Store::open_existing(&self.root.join(".awr/state.db")).unwrap();
        let p = store
            .project_by_root(&self.root.canonicalize().unwrap())
            .unwrap();
        serde_json::to_value(
            store
                .event(p.id, result["event_id"].as_str().unwrap().parse().unwrap())
                .unwrap(),
        )
        .unwrap()
    }
}

#[test]
#[ignore = "subprocess helper selected by fixture argument vectors"]
fn check_process_fixture() {
    let mode: Value = serde_json::from_slice(&fs::read("probe.json").unwrap()).unwrap();
    println!("Observed draft output");
    eprintln!("Observed draft diagnostics");
    match mode["mode"].as_str().unwrap() {
        "fail" => panic!("The draft was lost"),
        "pass_once" => fs::write("probe.json", r#"{"mode":"fail"}"#).unwrap(),
        "rewrite" => fs::write("oracle.txt", "changed oracle\n").unwrap(),
        "sleep" => {
            fs::write("child.pid", std::process::id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(120));
        }
        _ => {}
    }
}

impl Fixture {
    fn discrimination(&self, method: &str, note: Option<&str>) {
        let path = self.root.join("work.yaml");
        let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        ledger["work_items"][0]["definition"]["discrimination"] =
            json!({"method":method,"note":note});
        fs::write(&path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
        self.ok(&["source", "reindex"]);
        self.git(&["add", "work.yaml"]);
        self.git(&["commit", "-m", "Declare the discrimination method"]);
    }

    fn mutation_report(&self, mutants: Value) {
        let report = json!({"version":1,"source_sha":self.git(&["rev-parse","HEAD"]),
            "tool":"synthetic-report 1","mutants":mutants});
        fs::write(
            self.root.join(".awr/mutation-report.json"),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
    }

    fn mutation_policy(&self) {
        let python = if cfg!(windows) { "python" } else { "python3" };
        let script = "import pathlib,sys; p=pathlib.Path('.awr/mutation-invocations'); p.write_text(p.read_text()+'x' if p.exists() else 'x'); sys.stdout.buffer.write(pathlib.Path('.awr/mutation-report.json').read_bytes())";
        self.gate_policy(
            "enforce",
            &format!(
                "[checks.mutation]\nrun = {}",
                serde_json::to_string(&json!([python, "-c", script])).unwrap()
            ),
        );
    }

    fn alternative_decision(&self, session: &str, proposal: Option<&str>) -> Output {
        let assessed = self.ok(&["work", "assess", "W"]);
        let mut request = json!({"work":"W","session":session,
            "expected_revision":self.revision().parse::<u64>().unwrap(),
            "request_key":format!("alternative-{}",Id::new()),
            "decision":if proposal.is_some() {"approved"} else {"proposed"},
            "contract_fingerprint":assessed["contract_fingerprint"]});
        if let Some(event) = proposal {
            request["proposal_event"] = json!(event);
        } else {
            request["judgment"] = json!({"contribution":"Keep the draft","necessity":"Preserve pending input",
                "alternatives":"Use a failure injection review","deliverable":"Safe local saving",
                "exit":"The reviewer accepts the alternative","responsibility":"draft-storage"});
        }
        fs::write(
            self.root.join(".awr/alternative-admission.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        self.run(&[
            "work",
            "admit",
            "--input",
            ".awr/alternative-admission.json",
        ])
    }
}

fn mutation_fixture() -> Fixture {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.discrimination("diff_mutation", None);
    f.gate_admit();
    f.mutation_policy();
    f
}

#[test]
fn mutation_criterion_1_all_killed_passes_records_counts_and_completes() {
    let f = mutation_fixture();
    f.mutation_report(json!([{"id":"m1","file":"src/save.rs","line":42,
        "status":"killed","killed_by":["draft-0"]}]));
    let run = f.checked("mutation", &["--require-met"]);
    assert_eq!(run["met"], true);
    assert_eq!(run["results"][0]["outcome"], "passed");
    let payload = &f.event(&run["results"][0])["payload"];
    assert_eq!(
        payload["mutants"],
        json!({"killed":1,"survived":0,"equivalent":0,"timeout":0,"skipped":0})
    );
    assert_eq!(
        fs::read_to_string(f.root.join(".awr/mutation-invocations")).unwrap(),
        "x"
    );
    f.checked("acceptance", &["--write-input", "completion.json"]);
    assert_eq!(f.gate_done()["check_gate"]["would_refuse"], false);
}

#[test]
fn mutation_criterion_1_survivor_refuses_completion_with_file_and_line() {
    let f = mutation_fixture();
    f.mutation_report(json!([{"id":"m-live","file":"src/save.rs","line":57,"status":"survived"}]));
    let run = f.checked("mutation", &[]);
    assert_eq!(run["met"], false);
    assert_eq!(run["results"][0]["outcome"], "failed");
    assert_eq!(
        run["results"][0]["survivors"],
        json!([{"id":"m-live","file":"src/save.rs","line":57}])
    );
    f.checked("acceptance", &["--write-input", "completion.json"]);
    let error = f.gate_error("mutants_survived");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("src/save.rs:57"),
        "{error}"
    );
}

#[test]
fn mutation_criterion_2_equivalent_with_note_is_counted_and_passes() {
    let f = mutation_fixture();
    f.mutation_report(json!([{"id":"m-eq","file":"src/save.rs","line":12,
        "status":"equivalent","note":"The earlier guard already excludes this branch"}]));
    let run = f.checked("mutation", &[]);
    assert_eq!(run["met"], true);
    assert_eq!(run["results"][0]["mutants"]["equivalent"], 1);
    f.checked("acceptance", &["--write-input", "completion.json"]);
    f.gate_done();
}

#[test]
fn mutation_criterion_2_equivalent_without_note_rejects_report_with_id() {
    let f = mutation_fixture();
    f.mutation_report(
        json!([{"id":"m-needs-note","file":"src/save.rs","line":12,"status":"equivalent"}]),
    );
    let output = f.check("mutation", &[]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["message"].as_str().unwrap().contains("m-needs-note"),
        "{error}"
    );
    let status = f.ok(&["work", "check", "W", "--status"]);
    assert!(status["examples"].as_array().unwrap().iter().all(|e| {
        e["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["phase"] != "mutation")
    }));
}

#[test]
fn mutation_criterion_4_missing_command_is_unsupported_and_explains_configuration() {
    let f = Fixture::new(1, false);
    let output = f.check("mutation", &[]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "Unsupported", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(".awr/gates.toml"),
        "{error}"
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("[checks.mutation]"),
        "{error}"
    );
}

#[test]
fn mutation_criterion_4_declared_example_command_passes() {
    let f = Fixture::new(1, false);
    let python = if cfg!(windows) { "python" } else { "python3" };
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/checks/mutants_example.py");
    f.gate_policy(
        "off",
        &format!(
            "[checks.mutation]\nrun = {}",
            serde_json::to_string(&json!([python, script.to_str().unwrap()])).unwrap()
        ),
    );
    let run = f.checked("mutation", &["--require-met"]);
    assert_eq!(run["met"], true);
    assert_eq!(run["results"][0]["mutants"]["killed"], 1);
    assert_eq!(run["results"][0]["mutants"]["equivalent"], 1);
}

#[test]
fn mutation_timeout_and_source_mismatch_cannot_satisfy_completion() {
    let f = mutation_fixture();
    f.mutation_report(json!([{"id":"slow","file":"src/save.rs","line":31,"status":"timeout"}]));
    assert_eq!(f.checked("mutation", &[])["met"], false);
    f.checked("acceptance", &["--write-input", "completion.json"]);
    f.gate_error("mutation_timeout");
    let path = f.root.join(".awr/mutation-report.json");
    let mut report: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    report["source_sha"] = json!("a".repeat(40));
    fs::write(path, serde_json::to_vec(&report).unwrap()).unwrap();
    let output = f.check("mutation", &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("source_sha"));
}

#[test]
fn mutation_criterion_3_independent_alternative_approval_allows_completion() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.discrimination(
        "justified_alternative",
        Some("Review a recorded storage failure injection"),
    );
    let output = f.alternative_decision(&f.session, None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let proposal: Value = serde_json::from_slice(&output.stdout).unwrap();
    let reviewer = f.ok(&[
        "session",
        "start",
        "--agent",
        "reviewer",
        "--provider",
        "fixture",
        "--model",
        "local",
        "--expected-revision",
        &f.revision(),
    ]);
    let output = f.alternative_decision(
        reviewer["session"]["id"].as_str().unwrap(),
        proposal["event"]["id"].as_str(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    f.gate_policy("enforce", "");
    f.checked("acceptance", &["--write-input", "completion.json"]);
    f.gate_done();
}

#[test]
fn mutation_criterion_3_self_approval_of_alternative_is_rejected_without_tags() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.discrimination(
        "justified_alternative",
        Some("Review a recorded storage failure injection"),
    );
    let output = f.alternative_decision(&f.session, None);
    assert!(output.status.success());
    let proposal: Value = serde_json::from_slice(&output.stdout).unwrap();
    let output = f.alternative_decision(&f.session, proposal["event"]["id"].as_str());
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "AdmissionRequired", "{error}");
    assert_eq!(
        error["details"]["reason"], "admission_review_not_independent",
        "{error}"
    );
}

#[test]
fn mutation_survivor_event_keeps_twenty_locations_and_the_full_count() {
    let f = mutation_fixture();
    f.mutation_report(json!((1..=25).map(|line|json!({"id":format!("m{line}"),"file":"src/save.rs","line":line,"status":"survived"})).collect::<Vec<_>>()));
    let run = f.checked("mutation", &[]);
    let event = f.event(&run["results"][0]);
    assert_eq!(event["payload"]["mutants"]["survived"], 25);
    assert_eq!(event["payload"]["survivors"].as_array().unwrap().len(), 20);
    assert_eq!(event["payload"]["survivors"][19]["line"], 20);
    assert!(serde_json::to_vec(&event["payload"]).unwrap().len() <= awr_core::CHECK_PAYLOAD_CAP);
}

#[test]
fn mutation_command_failure_and_timeout_remain_unsatisfied() {
    let python = if cfg!(windows) { "python" } else { "python3" };
    for (script, outcome, reason) in [
        ("import sys; sys.exit(7)", "failed", Value::Null),
        ("import time; time.sleep(120)", "unknown", json!("timeout")),
    ] {
        let f = Fixture::new(1, false);
        f.gate_policy(
            "off",
            &format!(
                "[checks.mutation]\nrun = {}",
                serde_json::to_string(&json!([python, "-c", script])).unwrap()
            ),
        );
        let run = f.checked("mutation", &["--timeout", "1"]);
        assert_eq!(run["met"], false);
        assert_eq!(run["results"][0]["outcome"], outcome);
        assert_eq!(run["results"][0]["reason"], reason);
        assert!(run["results"][0]["mutants"].is_null());
    }
}

#[test]
fn mutation_command_cannot_rebind_later_example_oracles_after_rewriting_files() {
    let f = Fixture::new(2, false);
    let python = if cfg!(windows) { "python" } else { "python3" };
    let script = "import pathlib,sys; pathlib.Path('oracle.txt').write_text('rewritten'); sys.stdout.buffer.write(pathlib.Path('.awr/mutation-report.json').read_bytes())";
    f.gate_policy(
        "off",
        &format!(
            "[checks.mutation]\nrun = {}",
            serde_json::to_string(&json!([python, "-c", script])).unwrap()
        ),
    );
    f.mutation_report(json!([{"id":"m1","file":"src/save.rs","line":1,"status":"killed"}]));
    let run = f.checked("mutation", &[]);
    assert_eq!(run["met"], false);
    for observation in run["results"].as_array().unwrap() {
        assert_eq!(observation["outcome"], "errored");
        assert_eq!(observation["reason"], "oracle_changed_during_run");
        assert_eq!(
            observation["oracle_files"][0]["sha256"],
            format!("{:x}", Sha256::digest(b"original oracle\n"))
        );
    }
}

#[test]
fn baseline_failure_and_acceptance_pass_record_exit_oracle_source_and_log_hashes() {
    let f = Fixture::new(1, false);
    f.mode("fail");
    let before = f.checked("baseline", &[]);
    assert_eq!(before["met"], true);
    let r = &before["results"][0];
    let e = f.event(r);
    assert_eq!(e["event_type"], "work.check");
    assert_eq!(e["session_id"], f.session);
    let p = &e["payload"];
    assert_eq!(p["outcome"], "failed");
    assert_eq!(p["exit_code"], 101);
    assert_eq!(p["source_sha"], f.git(&["rev-parse", "HEAD"]));
    assert_eq!(p["tree_state"], "clean");
    assert_eq!(
        p["criterion_sha256"],
        format!("{:x}", Sha256::digest(C1.as_bytes()))
    );
    assert!(p["example_oracle"].as_str().unwrap().starts_with("sha256:"));
    for (file, key) in [
        ("stdout.log", "stdout_sha256"),
        ("stderr.log", "stderr_sha256"),
    ] {
        let bytes = fs::read(f.root.join(p["log_dir"].as_str().unwrap()).join(file)).unwrap();
        assert_eq!(p[key], format!("{:x}", Sha256::digest(&bytes)));
    }
    f.mode("pass");
    let after = f.checked("acceptance", &[]);
    assert_eq!(after["met"], true);
    assert_eq!(after["results"][0]["outcome"], "passed");
    assert_eq!(
        f.event(&after["results"][0])["payload"]["phase"],
        "acceptance"
    );
    let status = f.ok(&["work", "check", "W", "--status"]);
    assert_eq!(status["examples"][0]["runs"].as_array().unwrap().len(), 2);
}

#[test]
fn oracle_changed_during_run_is_errored_even_when_the_child_exits_zero() {
    let f = Fixture::new(1, false);
    f.mode("rewrite");
    let v = f.checked("baseline", &[]);
    let p = &f.event(&v["results"][0])["payload"];
    assert_eq!(p["outcome"], "errored");
    assert_eq!(p["reason"], "oracle_changed_during_run");
    assert_eq!(p["exit_code"], 0);
    assert_eq!(v["met"], false);
}

#[test]
fn passing_acceptance_generates_registered_evidence_and_completes_without_manual_reports() {
    let f = Fixture::new(2, false);
    let v = f.checked("acceptance", &["--write-input", "completion.json"]);
    assert_eq!(v["met"], true);
    assert_eq!(v["unmapped"], json!([]));
    assert_eq!(v["evidence"]["trust"], "recorded");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(f.root.join("completion.json")).unwrap())
            .unwrap(),
        v["completion_input"]
    );
    let evidence = f.ok(&[
        "evidence",
        "show",
        v["evidence"]["external_key"].as_str().unwrap(),
        "--content",
    ]);
    assert_eq!(evidence["trust"], "recorded");
    let report: Value = serde_json::from_str(evidence["content"].as_str().unwrap()).unwrap();
    assert_eq!(report["checks"].as_array().unwrap().len(), 2);
    let doctor = f.ok(&["doctor"]);
    assert!(!doctor.to_string().contains("orphan_artifact"), "{doctor}");
    let done = f.ok(&[
        "work",
        "complete",
        "W",
        "--session",
        &f.session,
        "--reason",
        "Draft checks passed",
        "--input",
        "completion.json",
        "--expected-revision",
        &f.revision(),
        "--end-session",
    ]);
    assert_eq!(done["ok"], true);
}

#[test]
fn generated_check_evidence_trust_is_recorded_across_receipts_and_reads() {
    let f = Fixture::new(1, false);
    let generated = f.checked("acceptance", &[]);
    let key = generated["evidence"]["external_key"].as_str().unwrap();
    assert_eq!(generated["evidence"]["trust"], "recorded");
    let shown = f.ok(&["evidence", "show", key]);
    assert_eq!(shown["trust"], "recorded");
    assert_eq!(shown["evidence"]["trust"], "recorded");
    let work = f.ok(&["work", "show", "W"]);
    let record = work["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["external_key"] == key)
        .unwrap();
    assert_eq!(record["trust"], "recorded");
}

#[test]
fn completed_check_evidence_upgrades_to_verified_over_its_recorded_provenance() {
    let f = Fixture::new(1, false);
    let generated = f.checked("acceptance", &["--write-input", "completion.json"]);
    let key = generated["evidence"]["external_key"].as_str().unwrap();
    assert_eq!(generated["evidence"]["trust"], "recorded");
    f.ok(&[
        "work",
        "complete",
        "W",
        "--session",
        &f.session,
        "--reason",
        "The generated report passed byte verification",
        "--input",
        "completion.json",
        "--expected-revision",
        &f.revision(),
    ]);
    let shown = f.ok(&["evidence", "show", key]);
    assert_eq!(shown["trust"], "verified");
    assert_eq!(shown["evidence"]["trust"], "verified");
    let work = f.ok(&["work", "show", "W"]);
    let record = work["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["external_key"] == key)
        .unwrap();
    assert_eq!(record["trust"], "verified");
}

#[test]
fn failed_acceptance_does_not_register_evidence_and_points_at_failed_logs() {
    let f = Fixture::new(2, true);
    f.mode("pass_once");
    let v = f.checked("acceptance", &["--write-input", "completion.json"]);
    assert_eq!(v["results"][0]["outcome"], "passed");
    assert_eq!(v["results"][1]["outcome"], "failed");
    assert_eq!(v["met"], false);
    assert_eq!(v["unmapped"], json!([C2]));
    assert!(v["evidence"].is_null());
    assert!(v["completion_input"].is_null());
    assert!(!f.root.join("completion.json").exists());
    assert!(
        v["next_action"]
            .as_str()
            .unwrap()
            .contains(v["results"][1]["log_dir"].as_str().unwrap())
    );
}

#[test]
fn unchecked_criteria_are_unmapped_and_cannot_complete_from_generated_input() {
    let f = Fixture::new(2, true);
    let v = f.checked("acceptance", &["--write-input", "completion.json"]);
    assert_eq!(v["unmapped"], json!([C2]));
    let a = v["completion_input"]["acceptance"].as_array().unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(a[1]["criterion"], C2);
    assert_eq!(a[1]["evidence"], json!([]));
    let o = f.run(&[
        "work",
        "complete",
        "W",
        "--session",
        &f.session,
        "--reason",
        "Check the mapping",
        "--input",
        "completion.json",
        "--expected-revision",
        &f.revision(),
    ]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("EvidenceMissing"));
}

#[test]
fn governed_baseline_runs_without_admission() {
    let f = Fixture::new(1, false);
    f.governed();
    f.mode("fail");
    assert_eq!(
        f.checked("baseline", &[])["results"][0]["outcome"],
        "failed"
    );
}

#[test]
fn governed_acceptance_and_mutation_without_admission_are_refused_before_spawning() {
    let f = Fixture::new(1, false);
    f.governed();
    f.mode("sleep");
    for phase in ["acceptance", "mutation"] {
        let o = f.check(phase, &[]);
        assert!(!o.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&o.stderr).unwrap()["code"],
            "AdmissionRequired"
        );
    }
    assert!(!f.root.join("child.pid").exists());
}

#[test]
fn timeout_is_unknown_and_the_direct_child_has_been_reaped() {
    let f = Fixture::new(1, false);
    f.mode("sleep");
    let v = f.checked("baseline", &["--timeout", "1"]);
    let p = f.event(&v["results"][0])["payload"].clone();
    assert_eq!(p["outcome"], "unknown");
    assert_eq!(p["reason"], "timeout");
    assert_eq!(v["met"], false);
    let pid = fs::read_to_string(f.root.join("child.pid")).unwrap();
    #[cfg(unix)]
    assert!(
        !Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .unwrap()
            .success(),
        "Timed-out child survived"
    );
    #[cfg(windows)]
    {
        let o = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid.trim()), "/NH"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&o.stdout).contains(pid.trim()),
            "Timed-out child survived"
        );
    }
}

#[test]
fn timeout_never_counts_as_an_expected_baseline_failure() {
    let f = Fixture::new(1, false);
    f.mode("sleep");
    let o = f.check("baseline", &["--timeout", "1", "--require-met"]);
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["results"][0]["outcome"], "unknown");
    assert_ne!(v["results"][0]["outcome"], "failed");
}

#[test]
fn eight_example_text_is_compact_and_excludes_urls_and_millisecond_timestamps() {
    let f = Fixture::new(8, false);
    f.mode("fail");
    let o = Command::new(env!("CARGO_BIN_EXE_awr"))
        .env("AWR_LOCAL_WEB", "0")
        .arg("--project")
        .arg(&f.root)
        .args([
            "work",
            "check",
            "W",
            "--session",
            &f.session,
            "--phase",
            "baseline",
            "--expected-revision",
            &f.revision(),
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(o.stdout.len() <= 1024, "{} bytes", o.stdout.len());
    let s = String::from_utf8(o.stdout).unwrap();
    assert_eq!(s.lines().count(), 9);
    assert!(!s.contains("file://"));
    assert!(
        !s.split(|c: char| !c.is_ascii_digit())
            .any(|s| s.len() >= 13)
    );
    assert!(s.lines().last().unwrap().starts_with("Next:"));
}

#[test]
fn manually_registered_check_run_remains_reported() {
    let f = Fixture::new(1, false);
    fs::write(f.root.join("manual.json"),serde_json::to_vec(&json!({"external_key":"manual","work_item_key":"W","evidence_type":"check_run","level":"implemented","summary":"Caller supplied check claim","locator":"oracle.txt","scope":["W"]})).unwrap()).unwrap();
    let v = f.ok(&[
        "evidence",
        "add",
        "--input",
        "manual.json",
        "--expected-revision",
        &f.revision(),
    ]);
    assert_eq!(v["trust"], "reported");
    assert_eq!(f.ok(&["evidence", "show", "manual"])["trust"], "reported");
}

#[test]
fn missing_oracle_is_recorded_and_status_is_read_only() {
    let f = Fixture::new(1, false);
    fs::remove_file(f.root.join("oracle.txt")).unwrap();
    let v = f.checked("baseline", &[]);
    assert_eq!(v["results"][0]["reason"], "oracle_file_missing");
    assert_eq!(v["results"][0]["outcome"], "errored");
    let before = f.revision();
    let status = f.ok(&["work", "check", "W", "--status"]);
    assert_eq!(status["examples"].as_array().unwrap().len(), 1);
    assert_eq!(f.revision(), before);
}

#[test]
fn partial_selection_cannot_generate_a_completion_report_for_unrun_required_examples() {
    let f = Fixture::new(2, false);
    let v = f.checked("acceptance", &["--example", "draft-0"]);
    assert_eq!(v["met"], true);
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
    assert!(v["completion_input"].is_null());
    assert!(v["evidence"].is_null());
    assert!(
        v["next_action"]
            .as_str()
            .unwrap()
            .contains("all required examples")
    );
}

#[test]
fn stale_revision_unknown_example_and_wrong_session_do_not_start_checks() {
    let f = Fixture::new(1, false);
    f.mode("sleep");
    let start = f.ok(&[
        "session",
        "start",
        "--work",
        "OTHER",
        "--agent",
        "observer",
        "--provider",
        "fixture",
        "--model",
        "local",
        "--expected-revision",
        &f.revision(),
    ]);
    let other = start["session"]["id"].as_str().unwrap();
    let stale = f.run(&[
        "work",
        "check",
        "W",
        "--phase",
        "baseline",
        "--session",
        &f.session,
        "--expected-revision",
        "0",
    ]);
    assert_eq!(
        serde_json::from_slice::<Value>(&stale.stderr).unwrap()["code"],
        "RevisionConflict"
    );
    let unknown = f.check("baseline", &["--example", "not-declared"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&unknown.stderr).unwrap()["code"],
        "InvalidInput"
    );
    let wrong = f.run(&[
        "work",
        "check",
        "W",
        "--phase",
        "baseline",
        "--session",
        other,
        "--expected-revision",
        &f.revision(),
    ]);
    assert_eq!(
        serde_json::from_slice::<Value>(&wrong.stderr).unwrap()["code"],
        "InvalidInput"
    );
    assert!(!f.root.join("child.pid").exists());
}

#[test]
fn long_argument_vectors_store_only_a_digest_and_bounded_unicode_preview() {
    let f = Fixture::new(1, false);
    let path = f.root.join("work.yaml");
    let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let run = ledger["work_items"][0]["definition"]["examples"][0]["check"]["run"]
        .as_array_mut()
        .unwrap();
    for _ in 0..21 {
        run.push(json!("--skip"));
        run.push(json!("界".repeat(333)));
    }
    let argv = Value::Array(run.clone());
    fs::write(path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
    f.ok(&["source", "reindex"]);
    let v = f.checked("baseline", &[]);
    let e = f.event(&v["results"][0]);
    let p = &e["payload"];
    assert!(serde_json::to_vec(p).unwrap().len() <= 16 * 1024);
    assert!(p["run_preview"].as_str().unwrap().len() <= 256);
    assert_eq!(
        p["run_sha256"],
        format!("{:x}", Sha256::digest(serde_json::to_vec(&argv).unwrap()))
    );
    assert!(p.get("run").is_none());
}

#[test]
fn oversized_oracle_is_errored_and_dirty_tree_is_observed() {
    let f = Fixture::new(1, false);
    fs::write(f.root.join("oracle.txt"), vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
    f.mode("sleep");
    let v = f.checked("baseline", &[]);
    assert_eq!(v["results"][0]["outcome"], "errored");
    assert_eq!(v["results"][0]["tree_state"], "dirty");
    assert!(!f.root.join("child.pid").exists());
}

#[test]
fn passing_baseline_is_unmet_and_require_met_changes_only_the_exit_status() {
    let f = Fixture::new(1, false);
    let o = f.check("baseline", &["--require-met"]);
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["met"], false);
    assert_eq!(v["results"][0]["outcome"], "passed");
    assert_eq!(f.event(&v["results"][0])["event_type"], "work.check");
}

#[test]
fn non_git_projects_record_unknown_source_without_inventing_completion_bindings() {
    let f = Fixture::new(1, false);
    fs::remove_dir_all(f.root.join(".git")).unwrap();
    let v = f.checked("acceptance", &[]);
    assert_eq!(v["met"], true);
    assert!(v["results"][0]["source_sha"].is_null());
    assert_eq!(v["results"][0]["tree_state"], "unknown");
    assert!(v["completion_input"].is_null());
    assert!(v["next_action"].as_str().unwrap().contains("Git commit"));
}

#[test]
fn doctor_checks_log_integrity_and_does_not_hide_unrecorded_check_files() {
    let f = Fixture::new(1, false);
    f.mode("fail");
    let v = f.checked("baseline", &[]);
    let path = f.root.join(v["results"][0]["log_dir"].as_str().unwrap());
    fs::write(path.join("stdout.log"), "changed output").unwrap();
    fs::write(path.join("extra.txt"), "unregistered").unwrap();
    let output = f.run(&["doctor"]);
    assert!(!output.status.success());
    let d: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(d.to_string().contains("check_log_digest_mismatch"));
    assert!(d.to_string().contains("orphan_artifact"));
}

#[test]
fn governed_acceptance_needs_both_current_admission_and_the_calling_sessions_claim() {
    let f = Fixture::new(2, false);
    f.governed();
    fs::create_dir_all(f.root.join("decisions")).unwrap();
    let decision = "---\n{\"id\":\"draft-scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Draft scope\n\n## Decision\nPreserve the local draft on save.\n";
    fs::write(f.root.join("decisions/scope.md"), decision).unwrap();
    let manifest = f.root.join(".awr/project.toml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n");
    fs::write(manifest, text).unwrap();
    let path = f.root.join("work.yaml");
    let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    ledger["work_items"][0]["paths"] = json!(["oracle.txt"]);
    let d = &mut ledger["work_items"][0]["definition"];
    d["intent"] = json!([{"role":"user_decision","decision":"draft-scope","source_fingerprint":format!("sha256:{:x}",Sha256::digest(decision.as_bytes()))}]);
    d["contributes"] = json!([{"goal":"G","outcome":C1,"claim":"The original draft stays available","by":"checker"}]);
    d["granularity"] = json!({"judgment":"independent_result","reason":"One observable draft outcome","by":"checker"});
    d["responsibility"] = json!({"name":"draft-storage","kind":"delivery"});
    d["deliverable"] = json!("A save path that preserves the draft");
    d["evidence_method"] = json!("Compare direct checks before and after implementation");
    d["failure_recovery"] = json!("Keep the original draft available");
    d["discrimination"] = json!({"method":"fail_before_pass_after"});
    fs::write(path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
    f.ok(&["source", "reindex"]);
    f.mode("fail");
    assert_eq!(f.checked("baseline", &[])["met"], true);
    f.mode("pass");
    let assessed = f.ok(&["work", "assess", "W"]);
    let request = json!({"work":"W","session":f.session,"expected_revision":f.revision().parse::<u64>().unwrap(),"request_key":"draft-admission","decision":"approved","contract_fingerprint":assessed["contract_fingerprint"],"judgment":{"contribution":"Keep the draft","necessity":"A failed save can lose input","alternatives":"Retain the existing buffer","deliverable":"Safe local saving","exit":"All draft checks pass","responsibility":"draft-storage"}});
    fs::write(
        f.root.join("admission.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    f.ok(&["work", "admit", "--input", "admission.json"]);
    let v = f.checked("acceptance", &[]);
    assert_eq!(v["met"], true);
    let start = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "reviewer",
        "--provider",
        "fixture",
        "--model",
        "local",
        "--expected-revision",
        &f.revision(),
    ]);
    let session = start["session"]["id"].as_str().unwrap();
    let output = f.run(&[
        "work",
        "check",
        "W",
        "--session",
        session,
        "--phase",
        "acceptance",
        "--expected-revision",
        &f.revision(),
    ]);
    assert!(!output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "AdmissionRequired"
    );
}

impl Fixture {
    fn gate_policy(&self, mode: &str, extra: &str) {
        fs::write(
            self.root.join(".awr/gates.toml"),
            format!("version = 1\n[checks]\nmode = '{mode}'\nscope = 'all'\n{extra}\n"),
        )
        .unwrap();
    }
    fn gate_definition(&self, size: &str, purpose: &str) {
        let path = self.root.join("work.yaml");
        let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let d = &mut ledger["work_items"][0]["definition"];
        d["size"] = json!(size);
        d["purpose"] = json!(purpose);
        d["discrimination"] = json!({"method":"fail_before_pass_after"});
        ledger["work_items"][0]["summary"] = json!("Retain the original draft through saving.");
        ledger["work_items"][0]["next_action"] =
            json!("Run the registered draft checks and review their results.");
        fs::write(path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
        self.ok(&["source", "reindex"]);
        self.git(&["add", "work.yaml"]);
        self.git(&["commit", "-m", "Declare the completion checks"]);
    }
    fn gate_admit(&self) {
        self.governed();
        fs::create_dir_all(self.root.join("decisions")).unwrap();
        let decision = "---\n{\"id\":\"draft-scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Draft scope\n\n## Decision\nPreserve the local draft on save.\n";
        fs::write(self.root.join("decisions/scope.md"), decision).unwrap();
        let manifest = self.root.join(".awr/project.toml");
        let mut text = fs::read_to_string(&manifest).unwrap();
        text.push_str("\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n");
        fs::write(manifest, text).unwrap();
        let path = self.root.join("work.yaml");
        let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        ledger["work_items"][0]["paths"] = json!(["oracle.txt"]);
        let d = &mut ledger["work_items"][0]["definition"];
        d["intent"] = json!([{"role":"user_decision","decision":"draft-scope","source_fingerprint":format!("sha256:{:x}",Sha256::digest(decision.as_bytes()))}]);
        d["contributes"] = json!([{"goal":"G","outcome":C1,"claim":"The original draft stays available","by":"checker"}]);
        d["granularity"] = json!({"judgment":"independent_result","reason":"One observable draft outcome","by":"checker"});
        d["responsibility"] = json!({"name":"draft-storage","kind":"delivery"});
        d["deliverable"] = json!("A save path that preserves the draft");
        d["evidence_method"] = json!("Compare direct checks before and after implementation");
        d["failure_recovery"] = json!("Keep the original draft available");
        fs::write(path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
        self.ok(&["source", "reindex"]);
        self.git(&["add", "work.yaml", "decisions/scope.md"]);
        self.git(&["commit", "-m", "Adopt the draft contract"]);
        self.mode("fail");
        assert_eq!(self.checked("baseline", &[])["met"], true);
        self.mode("pass");
        let assessed = self.ok(&["work", "assess", "W"]);
        let request = json!({"work":"W","session":self.session,"expected_revision":self.revision().parse::<u64>().unwrap(),"request_key":"gate-admission","decision":"approved","contract_fingerprint":assessed["contract_fingerprint"],"judgment":{"contribution":"Keep the draft","necessity":"A failed save can lose input","alternatives":"Retain the existing buffer","deliverable":"Safe local saving","exit":"All draft checks pass","responsibility":"draft-storage"}});
        fs::write(
            self.root.join(".awr/admission.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        self.ok(&["work", "admit", "--input", ".awr/admission.json"]);
    }
    fn manual_completion(&self, waiver: Option<&str>) {
        let sha = self.git(&["rev-parse", "HEAD"]);
        fs::create_dir_all(self.root.join(".awr/artifacts")).unwrap();
        let report = json!({"version":1,"work_item":"W","source_sha":sha,"command":"Review draft retention","scope":["W"],"verified_at":0,"checks":[{"name":"draft-review","passed":true,"details":"Draft retained","criteria":[C1]}]});
        fs::write(
            self.root.join(".awr/artifacts/gate-report.json"),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        let prepared = self.ok(&[
            "work",
            "prepare-completion",
            "W",
            "--report",
            ".awr/artifacts/gate-report.json",
            "--evidence-key",
            "gate-review",
            "--source-sha",
            &sha,
            "--level",
            "locally_verified",
        ]);
        fs::write(
            self.root.join(".awr/gate-evidence.json"),
            serde_json::to_vec(&prepared["evidence"]).unwrap(),
        )
        .unwrap();
        self.ok(&[
            "evidence",
            "add",
            "--input",
            ".awr/gate-evidence.json",
            "--expected-revision",
            &self.revision(),
        ]);
        let mut input = prepared["completion"].clone();
        if let Some(reason) = waiver {
            input["check_waiver"] = json!(reason);
        }
        fs::write(
            self.root.join("completion.json"),
            serde_json::to_vec(&input).unwrap(),
        )
        .unwrap();
    }
    fn gate_complete(&self) -> Output {
        self.run(&[
            "work",
            "complete",
            "W",
            "--session",
            &self.session,
            "--reason",
            "Deliver the draft retention change",
            "--input",
            "completion.json",
            "--expected-revision",
            &self.revision(),
        ])
    }
    fn gate_done(&self) -> Value {
        let o = self.gate_complete();
        assert!(
            o.status.success(),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn gate_error(&self, reason: &str) -> Value {
        let o = self.gate_complete();
        assert!(
            !o.status.success(),
            "unexpected completion: {}",
            String::from_utf8_lossy(&o.stdout)
        );
        let v: Value = serde_json::from_slice(&o.stderr).unwrap();
        assert_eq!(v["code"], "EvidenceMissing", "{v}");
        assert_eq!(v["details"]["rule"], "completion.checks", "{v}");
        assert!(
            v["details"]["unmet"]
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u["reason"] == reason),
            "{v}"
        );
        v
    }
}

#[test]
fn criterion_1_and_2_enforce_accepts_all_checks_at_the_completed_commit() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.gate_policy("enforce", "");
    let runs = f.checked("acceptance", &["--write-input", "completion.json"]);
    assert_eq!(runs["met"], true);
    let done = f.gate_done();
    assert_eq!(done["check_gate"]["would_refuse"], false, "{done}");
    let event = f.ok(&[
        "event",
        "show",
        done["event"]["id"].as_str().unwrap(),
        "--full",
    ]);
    assert_eq!(
        event["event"]["payload"]["check_gate"]["would_refuse"], false,
        "{event}"
    );
}

#[test]
fn criterion_1_missing_acceptance_refuses_with_example_and_runnable_next_action() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.gate_policy("enforce", "");
    f.checked("acceptance", &["--example", "draft-0"]);
    f.manual_completion(None);
    let v = f.gate_error("acceptance_missing");
    assert!(
        v["details"]["unmet"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["example"] == "draft-1" && u["criterion"] == C1)
    );
    let next = v["details"]["next_action"].as_str().unwrap();
    assert!(
        next.contains("work check W")
            && next.contains(&f.session)
            && next.contains("--phase acceptance")
            && next.contains("--write-input")
            && next.contains(&f.revision()),
        "{next}"
    );
    let args: Vec<_> = next.split_whitespace().skip(1).collect();
    assert_eq!(f.ok(&args)["met"], true);
    assert!(f.root.join(".awr/completion-input.json").exists());
}

#[test]
fn criterion_2_acceptance_from_an_earlier_commit_is_refused() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.gate_policy("enforce", "");
    f.checked("acceptance", &[]);
    f.git(&[
        "commit",
        "--allow-empty",
        "-m",
        "Advance the implementation",
    ]);
    f.manual_completion(None);
    f.gate_error("acceptance_missing");
}

#[test]
fn criterion_3_observe_records_missing_checks_and_never_refuses_completion() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_policy("observe", "");
    f.manual_completion(None);
    let done = f.gate_done();
    assert_eq!(done["check_gate"]["would_refuse"], true, "{done}");
    let event = f.ok(&[
        "event",
        "show",
        done["event"]["id"].as_str().unwrap(),
        "--full",
    ]);
    assert_eq!(
        event["event"]["payload"]["check_gate"]["would_refuse"], true,
        "{event}"
    );
    assert_eq!(f.ok(&["work", "show", "W"])["work"]["status"], "completed");
}

#[test]
fn criterion_4_small_work_waiver_is_saved_in_the_completion_event() {
    let f = Fixture::new(2, false);
    f.gate_definition("small", "implementation");
    f.gate_policy("enforce", "small_work = 'reason'");
    f.manual_completion(Some(
        "The label-only change was reviewed against the draft screen.",
    ));
    let done = f.gate_done();
    let event = f.ok(&[
        "event",
        "show",
        done["event"]["id"].as_str().unwrap(),
        "--full",
    ]);
    assert_eq!(
        event["event"]["payload"]["work_action"]["completion"]["check_waiver"],
        "The label-only change was reviewed against the draft screen."
    );
    assert_eq!(done["check_gate"]["would_refuse"], false);
}

#[test]
fn criterion_4_medium_work_waiver_is_invalid_input() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_policy("enforce", "small_work = 'reason'");
    f.manual_completion(Some("Reviewed manually."));
    let o = f.gate_complete();
    assert!(!o.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stderr).unwrap()["code"],
        "InvalidInput"
    );
}

#[test]
fn criterion_5_absent_policy_preserves_existing_completion_behavior() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.manual_completion(None);
    let done = f.gate_done();
    assert!(done.get("check_gate").is_none());
}

#[test]
fn criterion_6_document_work_is_exempt_even_inside_enforced_scope() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "document");
    f.gate_policy("enforce", "");
    f.manual_completion(None);
    let done = f.gate_done();
    assert_eq!(done["check_gate"]["applies"], false, "{done}");
    assert_eq!(done["check_gate"]["would_refuse"], false, "{done}");
}

#[test]
fn criterion_7_dirty_acceptance_is_allowed_by_explicit_policy() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.gate_policy("enforce", "require_clean_tree = false");
    fs::write(f.root.join("mapping.toml"), "# pending implementation\n").unwrap();
    let runs = f.checked("acceptance", &["--write-input", "completion.json"]);
    assert_eq!(runs["results"][0]["tree_state"], "dirty");
    assert_eq!(f.gate_done()["check_gate"]["would_refuse"], false);
}

#[test]
fn criterion_7_dirty_acceptance_is_refused_by_default() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.gate_policy("enforce", "");
    fs::write(f.root.join("mapping.toml"), "# pending implementation\n").unwrap();
    f.checked("acceptance", &["--write-input", "completion.json"]);
    f.gate_error("acceptance_dirty_tree");
}

impl Fixture {
    fn stop_hook(&self, turn: &str) -> Value {
        use std::io::Write;
        use std::process::Stdio;
        let input = json!({"session_id":"gate-conversation","hook_event_name":"Stop","cwd":self.root,"turn_id":turn,"stop_hook_active":false});
        let mut child = Command::new(env!("CARGO_BIN_EXE_awr"))
            .current_dir(&self.root)
            .args([
                "--project",
                self.root.to_str().unwrap(),
                "--json",
                "client",
                "hook",
                "--client",
                "claude",
            ])
            .env("AWR_LOCAL_WEB", "0")
            .env_remove("AWR_HOOK_GATES")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&input).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn completion_reminder_uses_the_production_gate_and_only_prompts_after_checks_pass() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.gate_policy("observe", "");
    let path = f.root.join(".awr/gates.toml");
    let text = fs::read_to_string(&path).unwrap() + "\n[hooks]\ncompletion_reminder = true\n";
    fs::write(path, text).unwrap();
    f.ok(&[
        "client",
        "bind",
        "--client",
        "claude",
        "--external-session",
        "gate-conversation",
        "--work",
        "W",
        "--session",
        &f.session,
    ]);
    let missing = f.stop_hook("before-acceptance");
    assert_ne!(missing["decision"], "block", "{missing}");
    assert!(missing["awr"]["internal_error"].is_null(), "{missing}");
    f.checked("acceptance", &[]);
    let passing = f.stop_hook("after-acceptance");
    assert_eq!(passing["decision"], "block", "{passing}");
    assert!(
        passing["reason"]
            .as_str()
            .unwrap()
            .contains("awr work complete W"),
        "{passing}"
    );
    let repeated = f.stop_hook("after-acceptance");
    assert_ne!(repeated["decision"], "block", "{repeated}");
}

#[test]
fn check_obligations_and_preflight_share_the_current_gate_summary() {
    let f = Fixture::new(2, false);
    f.gate_definition("medium", "implementation");
    f.gate_admit();
    f.gate_policy("enforce", "");
    let prepared = f.ok(&[
        "work",
        "prepare",
        "W",
        "--session",
        &f.session,
        "--budget",
        "9000",
    ]);
    let obligations = prepared["check_obligations"].as_array().unwrap();
    assert_eq!(obligations.len(), 2);
    for obligation in obligations {
        assert_eq!(obligation["state"], "unmet");
        assert!(
            obligation["baseline_command"]
                .as_str()
                .unwrap()
                .contains(&f.session)
        );
        assert!(
            obligation["acceptance_command"]
                .as_str()
                .unwrap()
                .contains("--write-input")
        );
    }
    let shown = f.ok(&["work", "show", "W"]);
    assert_eq!(shown["checks"]["would_refuse"], true);
    f.manual_completion(None);
    let preflight = f.ok(&[
        "work",
        "prepare-completion",
        "W",
        "--report",
        ".awr/artifacts/gate-report.json",
        "--evidence-key",
        "preflight-only",
        "--source-sha",
        &f.git(&["rev-parse", "HEAD"]),
        "--level",
        "locally_verified",
    ]);
    assert_eq!(preflight["checks"]["unmet"], shown["checks"]["unmet"]);
    f.checked("acceptance", &[]);
    assert_eq!(
        f.ok(&["work", "show", "W"])["checks"]["would_refuse"],
        false
    );
}

#[test]
fn doctor_reports_unreadable_check_policy_without_silently_using_off() {
    let f = Fixture::new(1, false);
    fs::write(
        f.root.join(".awr/gates.toml"),
        "version = 1\n[checks]\nmode = 'enforce'\n",
    )
    .unwrap();
    let output = f.run(&["doctor"]);
    assert!(!output.status.success());
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "gates_policy_unreadable" && f["severity"] == "error"),
        "{v}"
    );
    f.gate_policy("observe", "");
    let output = f.run(&["doctor"]);
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!v.to_string().contains("gates_policy_unreadable"), "{v}");
}

#[test]
fn check_waiver_rejects_blank_and_checks_only_policy() {
    for reason in ["", "Reviewed."] {
        let f = Fixture::new(2, false);
        f.gate_definition("small", "implementation");
        f.gate_policy(
            "enforce",
            if reason.is_empty() {
                "small_work = 'reason'"
            } else {
                "small_work = 'checks'"
            },
        );
        f.manual_completion(Some(reason));
        let o = f.gate_complete();
        assert!(!o.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&o.stderr).unwrap()["code"],
            "InvalidInput"
        );
    }
}
