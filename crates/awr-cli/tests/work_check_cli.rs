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
