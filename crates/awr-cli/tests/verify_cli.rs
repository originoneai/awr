//! Repository verification uses real Git repositories, CLI observations and subprocess checks.
use awr_core::Id;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const CRITERION: &str = "Saved content matches the requested result";

struct Fixture {
    root: PathBuf,
    baseline: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    fn new() -> Self {
        let mut f = Self {
            root: std::env::temp_dir().join(format!("awr-verify-fixture-{}", Id::new())),
            baseline: String::new(),
        };
        fs::create_dir_all(f.root.join(".awr")).unwrap();
        fs::write(f.root.join(".awr/project.toml"), "[project]\nname='Verify fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        fs::write(f.root.join(".gitignore"), ".awr/*\n!.awr/project.toml\n").unwrap();
        fs::write(f.root.join("implementation.txt"), "before\n").unwrap();
        fs::write(f.root.join("oracle.txt"), "before\n").unwrap();
        f.write_ledger("in_progress");
        f.git(&["init"]);
        f.git(&["config", "user.name", "Verify fixture"]);
        f.git(&["config", "user.email", "verify@example.invalid"]);
        f.git(&["config", "core.autocrlf", "false"]);
        f.commit("Create initial behavior");
        f.baseline = f.git(&["rev-parse", "HEAD"]);
        fs::write(f.root.join("implementation.txt"), "after\n").unwrap();
        fs::write(f.root.join("oracle.txt"), "after\n").unwrap();
        f.write_ledger("completed");
        f.commit("Deliver saved content");
        f
    }

    fn ledger(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.join("work.yaml")).unwrap()).unwrap()
    }

    fn save_ledger(&self, ledger: &Value) {
        fs::write(
            self.root.join("work.yaml"),
            serde_json::to_vec_pretty(ledger).unwrap(),
        )
        .unwrap();
    }

    fn write_ledger(&self, status: &str) {
        let examples: Vec<_> = ["positive", "negative"].iter().map(|kind| json!({
            "id":format!("saved-{kind}"), "criterion":CRITERION, "kind":kind,
            "given":"a saved document", "when":"its content is checked", "then":"the requested content is retained",
            "check":{"run":[std::env::current_exe().unwrap().to_str().unwrap(),"--ignored","--exact","verify_process_fixture","--nocapture"],"files":["oracle.txt"],"timeout_seconds":10}
        })).collect();
        self.save_ledger(&json!({
            "goals":[{"id":"G","title":"Retain content","status":"active","success_criteria":[CRITERION]}],
            "work_items":[{"id":"W","title":"Save content","status":status,"goal":"G","acceptance":[CRITERION],
                "summary":"Preserve saved content","next_action":"Run and review the content checks",
                "definition":{"version":1,"purpose":"implementation","size":"medium","tier":"required",
                    "discrimination":{"method":"fail_before_pass_after"},"examples":examples}}]
        }));
    }

    fn git_output(&self, args: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        let o = self.git_output(args);
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap().trim().into()
    }

    fn commit(&self, message: &str) {
        self.git(&[
            "add",
            "work.yaml",
            "implementation.txt",
            "oracle.txt",
            ".gitignore",
            ".awr/project.toml",
        ]);
        self.git(&["commit", "-m", message]);
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_at(&self.root, args)
    }

    fn run_at(&self, root: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .current_dir(root)
            .env("AWR_LOCAL_WEB", "0")
            .args(["--project", root.to_str().unwrap(), "--json"])
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

    fn verify(&self, args: &[&str], expected: i32) -> Value {
        let mut a = vec!["verify"];
        a.extend_from_slice(args);
        let o = self.run(&a);
        assert_eq!(
            o.status.code(),
            Some(expected),
            "{a:?}: {} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }

    fn assert_no_database(&self) {
        assert!(
            !fs::read_dir(self.root.join(".awr")).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("state.db")),
            "rerun must leave state.db and its sidecars absent"
        );
    }

    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }

    fn local_completed(&self, acceptance: bool) {
        self.local_completed_with(acceptance, |_| {});
    }

    fn local_completed_with(&self, acceptance: bool, configure: impl FnOnce(&Self)) {
        self.write_ledger("in_progress");
        self.ok(&["source", "reindex"]);
        let session = self.ok(&[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "verifier",
            "--provider",
            "fixture",
            "--model",
            "local",
            "--claim",
            "--expected-revision",
            &self.revision(),
        ])["session"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        fs::create_dir_all(self.root.join("decisions")).unwrap();
        let decision = "---\n{\"id\":\"content-scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Content scope\n\n## Decision\nKeep saved content available.\n";
        fs::write(self.root.join("decisions/scope.md"), decision).unwrap();
        use sha2::{Digest, Sha256};
        let mut ledger = self.ledger();
        ledger["work_items"][0]["paths"] = json!(["implementation.txt"]);
        let d = &mut ledger["work_items"][0]["definition"];
        d["intent"] = json!([{"role":"user_decision","decision":"content-scope","source_fingerprint":format!("sha256:{:x}",Sha256::digest(decision.as_bytes()))}]);
        d["contributes"] = json!([{"goal":"G","outcome":CRITERION,"claim":"Saved content stays available","by":"verifier"}]);
        d["granularity"] = json!({"judgment":"independent_result","reason":"One observable saved result","by":"verifier"});
        d["responsibility"] = json!({"name":"content-storage","kind":"delivery"});
        d["deliverable"] = json!("A retained content result");
        d["evidence_method"] = json!("Compare the registered checks before and after the change");
        d["failure_recovery"] = json!("Retain the saved content");
        self.save_ledger(&ledger);
        let manifest = self.root.join(".awr/project.toml");
        let mut text = fs::read_to_string(&manifest).unwrap();
        text.push_str("\n[sources.options.admission_policy]\nversion=1\nmode='governed_enforce'\nscope='all'\n\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n");
        fs::write(manifest, text).unwrap();
        configure(self);
        self.git(&["add", "decisions/scope.md"]);
        self.commit("Prepare local completion");
        self.ok(&["source", "reindex"]);
        fs::write(self.root.join("implementation.txt"), "broken\n").unwrap();
        assert_eq!(
            self.ok(&[
                "work",
                "check",
                "W",
                "--session",
                &session,
                "--phase",
                "baseline",
                "--expected-revision",
                &self.revision()
            ])["met"],
            true
        );
        fs::write(self.root.join("implementation.txt"), "after\n").unwrap();
        let assessed = self.ok(&["work", "assess", "W"]);
        let request = json!({"work":"W","session":session,"expected_revision":self.revision().parse::<u64>().unwrap(),"request_key":"content-admission","decision":"approved","contract_fingerprint":assessed["contract_fingerprint"],"judgment":{"contribution":"Retain content","necessity":"Lost content interrupts work","alternatives":"Keep the original saved copy","deliverable":"Retained content","exit":"Content checks pass","responsibility":"content-storage"}});
        fs::write(
            self.root.join(".awr/admission.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        self.ok(&["work", "admit", "--input", ".awr/admission.json"]);
        if acceptance {
            self.ok(&[
                "work",
                "check",
                "W",
                "--session",
                &session,
                "--phase",
                "acceptance",
                "--write-input",
                ".awr/completion.json",
                "--expected-revision",
                &self.revision(),
            ]);
        } else {
            let sha = self.git(&["rev-parse", "HEAD"]);
            fs::create_dir_all(self.root.join(".awr/artifacts")).unwrap();
            let report = json!({"version":1,"work_item":"W","source_sha":sha,"command":"Review saved content","scope":["W"],"verified_at":0,"checks":[{"name":"content-review","passed":true,"details":"Reviewed saved content","criteria":[CRITERION]}]});
            fs::write(
                self.root.join(".awr/artifacts/report.json"),
                serde_json::to_vec(&report).unwrap(),
            )
            .unwrap();
            let prepared = self.ok(&[
                "work",
                "prepare-completion",
                "W",
                "--report",
                ".awr/artifacts/report.json",
                "--evidence-key",
                "content-review",
                "--source-sha",
                &sha,
                "--level",
                "locally_verified",
            ]);
            fs::write(
                self.root.join(".awr/evidence.json"),
                serde_json::to_vec(&prepared["evidence"]).unwrap(),
            )
            .unwrap();
            self.ok(&[
                "evidence",
                "add",
                "--input",
                ".awr/evidence.json",
                "--expected-revision",
                &self.revision(),
            ]);
            fs::write(
                self.root.join(".awr/completion.json"),
                serde_json::to_vec(&prepared["completion"]).unwrap(),
            )
            .unwrap();
        }
        self.ok(&[
            "work",
            "complete",
            "W",
            "--session",
            &session,
            "--reason",
            "Deliver verified content",
            "--input",
            ".awr/completion.json",
            "--expected-revision",
            &self.revision(),
        ]);
        // Observe the gate after completion so verification itself must find missing checks.
        fs::write(
            self.root.join(".awr/gates.toml"),
            "version=1\n[checks]\nmode='observe'\nscope='all'\n",
        )
        .unwrap();
    }

    fn assert_no_reuse(&self, v: &Value) {
        for result in v["results"].as_array().unwrap() {
            for field in [
                "bound_commit",
                "delta",
                "delta_paths",
                "delta_paths_omitted",
            ] {
                assert!(result.get(field).is_none(), "unexpected {field}: {v}");
            }
        }
    }

    fn install_push_target(&self, require_work: bool) -> PathBuf {
        let remote = self.root.join(".awr/push-target.git");
        self.git(&["init", "--bare", remote.to_str().unwrap()]);
        self.git(&["remote", "add", "validation", remote.to_str().unwrap()]);
        self.git(&["push", "--quiet", "validation", "HEAD:refs/heads/verified"]);
        self.ok(&["verify", "install-hook", "--accept"]);
        if require_work {
            self.require_work_policy();
        }
        remote
    }

    fn require_work_policy(&self) {
        let path = self.root.join(".awr/gates.toml");
        let mut policy = fs::read_to_string(&path).unwrap();
        policy.push_str("\n[verify]\nrequire_work=true\n");
        fs::write(path, policy).unwrap();
    }

    fn push(&self) -> Output {
        let mut paths = vec![
            PathBuf::from(env!("CARGO_BIN_EXE_awr"))
                .parent()
                .unwrap()
                .to_owned(),
        ];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        Command::new("git")
            .args(["push", "--quiet", "validation", "HEAD:refs/heads/verified"])
            .current_dir(&self.root)
            .env("AWR_LOCAL_WEB", "0")
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("AWR_VERIFY", "on")
            .output()
            .unwrap()
    }
}

#[test]
fn ledger_only_ac1_reuses_completion_observations_and_reports_the_delta() {
    let f = Fixture::new();
    f.local_completed(true);
    let bound = f.git(&["rev-parse", "HEAD"]);
    f.commit("Record completed work in the ledger");
    let requested = f.git(&["rev-parse", "HEAD"]);
    let v = f.verify(&["--work", "W"], 0);
    assert_eq!(v["commit"], requested);
    assert_eq!(v["results"][0]["work"], "W");
    assert_eq!(v["results"][0]["bound_commit"], bound);
    assert_eq!(v["results"][0]["delta"], "ledger_only");
    assert_eq!(v["results"][0]["delta_paths"], json!(["work.yaml"]));
    assert_eq!(v["results"][0]["unmet"], json!([]));
    assert_eq!(f.verify(&["--require-work"], 0)["results"], v["results"]);
    f.require_work_policy();
    assert_eq!(f.verify(&[], 0)["results"], v["results"]);
}

#[test]
fn ledger_only_ac1_reopened_work_cannot_reuse_its_previous_completion() {
    let f = Fixture::new();
    f.local_completed(true);
    let mut ledger = f.ledger();
    ledger["work_items"][0]["status"] = json!("in_progress");
    let criterion = "Retain the revised saved content";
    ledger["work_items"][0]["acceptance"] = json!([criterion]);
    for e in ledger["work_items"][0]["definition"]["examples"]
        .as_array_mut()
        .unwrap()
    {
        e["criterion"] = json!(criterion);
    }
    f.save_ledger(&ledger);
    f.commit("Reopen work with revised acceptance");
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("acceptance_missing"), "{v}");
    f.assert_no_reuse(&v);
    assert_eq!(
        f.verify(&["--require-work"], 1)["reason"],
        "no_associated_work"
    );
}

#[test]
fn ledger_only_ac2_code_changes_keep_acceptance_commit_bound() {
    let f = Fixture::new();
    f.local_completed(true);
    fs::write(f.root.join("implementation.txt"), "changed\n").unwrap();
    f.commit("Change code after acceptance");
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("acceptance_missing"), "{v}");
    f.assert_no_reuse(&v);
    assert_eq!(f.verify(&[], 0)["reason"], "no_associated_work");
    assert_eq!(
        f.verify(&["--require-work"], 1)["reason"],
        "no_associated_work"
    );
}

#[test]
fn ledger_only_ac2_ledger_mapping_matches_files_exactly() {
    let f = Fixture::new();
    f.local_completed(true);
    fs::write(f.root.join("work.yaml.rs"), "fn changed() {}\n").unwrap();
    f.git(&["add", "work.yaml.rs"]);
    f.commit("Change a file sharing the ledger name prefix");
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("acceptance_missing"), "{v}");
    f.assert_no_reuse(&v);
}

#[test]
fn ledger_only_ac3_check_file_changes_cannot_reuse_observations() {
    let f = Fixture::new();
    f.local_completed(true);
    fs::write(f.root.join("oracle.txt"), "changed\n").unwrap();
    f.commit("Change the registered check file");
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("acceptance_missing"), "{v}");
    assert!(v.to_string().contains("oracle_changed"), "{v}");
    f.assert_no_reuse(&v);
}

#[test]
fn ledger_only_ac3_changed_check_command_still_fails_the_frozen_oracle_gate() {
    let f = Fixture::new();
    f.local_completed(true);
    let mut ledger = f.ledger();
    ledger["work_items"][0]["definition"]["examples"][0]["check"]["run"]
        .as_array_mut()
        .unwrap()
        .push(json!("--quiet"));
    f.save_ledger(&ledger);
    f.commit("Change a check command inside the ledger");
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("oracle_changed"), "{v}");
    f.assert_no_reuse(&v);
}

#[test]
fn ledger_only_ac4_decision_and_policy_changes_are_not_ledger_deltas() {
    for path in [
        "decisions/scope.md",
        ".awr/gates.toml",
        "RULES.md",
        "other.yaml",
    ] {
        let f = Fixture::new();
        f.local_completed(true);
        let file = f.root.join(path);
        let mut content = fs::read_to_string(&file).unwrap_or_default();
        content.push_str("\n# Additional project information\n");
        fs::write(file, content).unwrap();
        f.git(&["add", "--force", path]);
        f.commit("Update a source outside the ledger mapping");
        let v = f.verify(&["--work", "W"], 1);
        assert!(v.to_string().contains("acceptance_missing"), "{path}: {v}");
        f.assert_no_reuse(&v);
    }
}

#[test]
fn ledger_only_ac5_exact_commit_output_keeps_its_original_bytes() {
    let f = Fixture::new();
    f.local_completed(true);
    let bound = f.git(&["rev-parse", "HEAD"]);
    let before = f.run(&["verify", "--commit", &bound]);
    assert!(before.status.success());
    f.commit("Record the completed ledger");
    let after = f.run(&["verify", "--commit", &bound]);
    assert!(after.status.success());
    assert_eq!(before.stdout, after.stdout);
    f.assert_no_reuse(&serde_json::from_slice(&after.stdout).unwrap());
}

#[test]
fn ledger_only_ac5_unassociated_code_commit_keeps_the_original_empty_selection() {
    let f = Fixture::new();
    f.local_completed(true);
    fs::write(
        f.root.join("implementation.txt"),
        "changed after completion\n",
    )
    .unwrap();
    f.commit("Change code without a completion bound to the new commit");
    let v = f.verify(&[], 0);
    assert_eq!(v["reason"], "no_associated_work");
    assert_eq!(v["results"], json!([]));
}

#[test]
fn ledger_only_ac5_unassociated_ledger_commit_keeps_the_original_empty_selection_bytes() {
    let f = Fixture::new();
    f.local_completed(true);
    f.commit("Record the completed ledger without requiring associated work");
    let revision: u64 = f.revision().parse().unwrap();
    let expected = json!({"mode":"local", "commit":f.git(&["rev-parse", "HEAD"]),
        "results":[], "project_revision":revision, "ok":true, "met":true,
        "require_work":false, "reason":"no_associated_work"});
    let output = f.run(&["verify"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    assert!(output.stderr.is_empty());
}

#[test]
fn ledger_only_ac5_rerun_and_baseline_keep_their_existing_receipt_shapes() {
    let f = Fixture::new();
    f.local_completed(true);
    f.commit("Record the completed ledger");
    for args in [vec!["--rerun"], vec!["--rerun", "--baseline", &f.baseline]] {
        let v = f.verify(&args, 0);
        assert_eq!(v["mode"], "rerun");
        f.assert_no_reuse(&v);
        for check in v["results"][0]["checks"].as_array().unwrap() {
            assert_eq!(check["acceptance"]["outcome"], "passed");
            if args.len() > 1 {
                assert_eq!(check["baseline"]["outcome"], "failed");
            }
        }
    }
}

#[test]
fn ledger_only_ac5_nonancestor_completion_is_not_reused() {
    let f = Fixture::new();
    f.local_completed(true);
    f.commit("Record the completed ledger");
    let completed = f.git(&["rev-parse", "HEAD"]);
    f.git(&["checkout", "--detach", &f.baseline]);
    f.git(&[
        "restore",
        "--source",
        &completed,
        "--staged",
        "--worktree",
        "--",
        ".",
    ]);
    f.git(&[
        "commit",
        "-m",
        "Record the same files on a different line of history",
    ]);
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("acceptance_missing"), "{v}");
    f.assert_no_reuse(&v);
}

#[test]
fn ledger_only_ac5_explicit_selection_and_mixed_results_remain_independent() {
    let f = Fixture::new();
    f.local_completed(true);
    let mut ledger = f.ledger();
    let mut unobserved = ledger["work_items"][0].clone();
    unobserved["id"] = json!("V");
    unobserved["title"] = json!("Unobserved saved content");
    ledger["work_items"]
        .as_array_mut()
        .unwrap()
        .push(unobserved);
    f.save_ledger(&ledger);
    f.commit("Record another source-completed work without observations");
    assert_eq!(f.verify(&[], 0)["results"], json!([]));
    assert_eq!(
        f.verify(&["--require-work"], 0)["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.verify(&["--work", "W", "--work", "W"], 0)["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let v = f.verify(&["--work", "V", "--work", "W"], 1);
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["work"], "V");
    assert_eq!(results[0]["met"], false);
    assert!(results[0].get("bound_commit").is_none());
    assert_eq!(results[1]["work"], "W");
    assert_eq!(results[1]["met"], true);
    assert_eq!(results[1]["delta"], "ledger_only");
}

#[test]
fn ledger_only_required_selection_keeps_exact_receipts_without_adding_ancestors() {
    let f = Fixture::new();
    f.local_completed(true);
    let ancestor = f.git(&["rev-parse", "HEAD"]);
    let mut ledger = f.ledger();
    let mut work = ledger["work_items"][0].clone();
    work["id"] = json!("V");
    work["status"] = json!("in_progress");
    for field in ["evidence", "verification", "completed_at"] {
        work.as_object_mut().unwrap().remove(field);
    }
    ledger["work_items"].as_array_mut().unwrap().push(work);
    f.save_ledger(&ledger);
    f.commit("Prepare an additional independently checked work");
    f.ok(&["source", "reindex"]);
    let session = f.ok(&[
        "session",
        "start",
        "--work",
        "V",
        "--agent",
        "additional-verifier",
        "--provider",
        "fixture",
        "--model",
        "local",
        "--expected-revision",
        &f.revision(),
    ])["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::write(f.root.join("implementation.txt"), "broken\n").unwrap();
    assert_eq!(
        f.ok(&[
            "work",
            "check",
            "V",
            "--session",
            &session,
            "--phase",
            "baseline",
            "--expected-revision",
            &f.revision()
        ])["met"],
        true
    );
    fs::write(f.root.join("implementation.txt"), "after\n").unwrap();
    let assessed = f.ok(&["work", "assess", "V"]);
    let request = json!({"work":"V", "session":session,
        "expected_revision":f.revision().parse::<u64>().unwrap(),
        "request_key":"additional-content-admission", "decision":"approved",
        "contract_fingerprint":assessed["contract_fingerprint"],
        "judgment":{"contribution":"Retain content", "necessity":"Lost content interrupts work",
            "alternatives":"Keep the original saved copy", "deliverable":"Retained content",
            "exit":"Content checks pass", "responsibility":"content-storage"}});
    fs::write(
        f.root.join(".awr/additional-admission.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    f.ok(&["work", "admit", "--input", ".awr/additional-admission.json"]);
    f.ok(&[
        "work",
        "claim",
        "V",
        "--session",
        &session,
        "--expected-revision",
        &f.revision(),
    ]);
    f.ok(&[
        "work",
        "check",
        "V",
        "--session",
        &session,
        "--phase",
        "acceptance",
        "--write-input",
        ".awr/additional-completion.json",
        "--expected-revision",
        &f.revision(),
    ]);
    f.ok(&[
        "work",
        "complete",
        "V",
        "--session",
        &session,
        "--reason",
        "Deliver checked content",
        "--input",
        ".awr/additional-completion.json",
        "--expected-revision",
        &f.revision(),
    ]);
    for args in [vec![], vec!["--require-work"]] {
        let v = f.verify(&args, 0);
        assert_eq!(v["results"].as_array().unwrap().len(), 1);
        assert_eq!(v["results"][0]["work"], "V");
        f.assert_no_reuse(&v);
    }
    assert_eq!(
        f.verify(&["--work", "W"], 0)["results"][0]["bound_commit"],
        ancestor
    );
}

#[test]
fn ledger_only_empty_tree_delta_is_eligible() {
    let f = Fixture::new();
    f.local_completed(true);
    let bound = f.git(&["rev-parse", "HEAD"]);
    f.git(&["commit", "--allow-empty", "-m", "Keep the accepted tree"]);
    let v = f.verify(&["--work", "W"], 0);
    assert_eq!(v["results"][0]["bound_commit"], bound);
    assert_eq!(v["results"][0]["delta_paths"], json!([]));
}

#[test]
fn ledger_only_deleted_nonledger_file_prevents_reuse() {
    let f = Fixture::new();
    f.local_completed_with(true, |f| {
        fs::write(f.root.join("obsolete.rs"), "fn obsolete() {}\n").unwrap();
        f.git(&["add", "obsolete.rs"]);
    });
    f.git(&["rm", "obsolete.rs"]);
    f.commit("Remove code after acceptance");
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("acceptance_missing"));
    f.assert_no_reuse(&v);
}

#[test]
fn ledger_only_rename_into_a_mapped_path_checks_the_old_path_too() {
    let f = Fixture::new();
    f.local_completed_with(true, |f| {
        fs::write(f.root.join("pending.yaml"), "work_items: []\n").unwrap();
        fs::write(f.root.join("bookkeeping.yaml"), "work_items: []\n# Old bookkeeping\n").unwrap();
        let manifest = f.root.join(".awr/project.toml");
        let mut text = fs::read_to_string(&manifest).unwrap();
        text.push_str("\n[[sources]]\ndomain='ledger'\nrole='supporting'\npath='bookkeeping.yaml'\nadapter='yaml-ledger-v1'\n");
        fs::write(manifest, text).unwrap();
        f.git(&["add", "pending.yaml", "bookkeeping.yaml"]);
    });
    fs::remove_file(f.root.join("bookkeeping.yaml")).unwrap();
    fs::rename(f.root.join("pending.yaml"), f.root.join("bookkeeping.yaml")).unwrap();
    f.git(&["add", "--update", "pending.yaml", "bookkeeping.yaml"]);
    f.commit("Move an unmapped file into a mapped ledger path");
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("acceptance_missing"));
    f.assert_no_reuse(&v);
}

#[test]
fn ledger_only_code_mode_change_prevents_reuse() {
    let f = Fixture::new();
    f.local_completed(true);
    f.git(&["add", "work.yaml"]);
    f.git(&["update-index", "--chmod=+x", "implementation.txt"]);
    f.git(&[
        "commit",
        "-m",
        "Change only the code file mode and ledger status",
    ]);
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("acceptance_missing"));
    f.assert_no_reuse(&v);
}

#[test]
fn ledger_only_mapped_file_mode_change_is_reported() {
    let f = Fixture::new();
    f.local_completed_with(true, |f| {
        fs::write(f.root.join("bookkeeping.yaml"), "work_items: []\n").unwrap();
        let manifest = f.root.join(".awr/project.toml");
        let mut text = fs::read_to_string(&manifest).unwrap();
        text.push_str("\n[[sources]]\ndomain='ledger'\nrole='supporting'\npath='bookkeeping.yaml'\nadapter='yaml-ledger-v1'\n");
        fs::write(manifest, text).unwrap();
        f.git(&["add", "bookkeeping.yaml"]);
    });
    f.git(&["add", "work.yaml"]);
    f.git(&["update-index", "--chmod=+x", "bookkeeping.yaml"]);
    f.git(&["commit", "-m", "Change a mapped ledger mode"]);
    let v = f.verify(&["--work", "W"], 0);
    assert_eq!(
        v["results"][0]["delta_paths"],
        json!(["bookkeeping.yaml", "work.yaml"])
    );
}

#[test]
fn ledger_only_reuse_keeps_the_independent_approver_gate() {
    let f = Fixture::new();
    f.local_completed(true);
    f.commit("Record the completed ledger");
    fs::write(
        f.root.join(".awr/gates.toml"),
        "version=1\n[checks]\nmode='observe'\nscope='all'\nseparate_approver=true\n",
    )
    .unwrap();
    let v = f.verify(&["--work", "W"], 1);
    assert!(v.to_string().contains("approver_is_implementer"), "{v}");
    assert_eq!(v["results"][0]["delta"], "ledger_only");
}

#[test]
fn ledger_only_multiple_mapped_files_use_nul_paths_and_bound_the_receipt() {
    let f = Fixture::new();
    let mut paths: Vec<_> = (0..16)
        .map(|n| format!("ledgers/part {n:02}.yaml"))
        .collect();
    #[cfg(unix)]
    paths.push("ledgers/part\twith\nseparators.yaml".into());
    f.local_completed_with(true, |f| {
        fs::create_dir(f.root.join("ledgers")).unwrap();
        let manifest = f.root.join(".awr/project.toml");
        let mut text = fs::read_to_string(&manifest).unwrap();
        for path in &paths {
            fs::write(f.root.join(path), "work_items: []\n").unwrap();
            text.push_str(&format!("\n[[sources]]\ndomain='ledger'\nrole='supporting'\npath={}\nadapter='yaml-ledger-v1'\n", serde_json::to_string(path).unwrap()));
        }
        fs::write(manifest, text).unwrap();
        f.git(&["add", "ledgers"]);
    });
    for path in &paths {
        fs::write(f.root.join(path), "work_items: []\n# Ledger bookkeeping\n").unwrap();
    }
    f.git(&["add", "ledgers"]);
    f.commit("Record all mapped ledger files");
    paths.push("work.yaml".into());
    paths.sort();
    let v = f.verify(&["--work", "W"], 0);
    assert_eq!(v["results"][0]["delta_paths"], json!(&paths[..16]));
    assert_eq!(v["results"][0]["delta_paths_omitted"], paths.len() - 16);
}

#[test]
fn ledger_only_nested_project_uses_repository_relative_paths() {
    let f = Fixture::new();
    let project = f.root.join("app");
    fs::create_dir_all(project.join(".awr")).unwrap();
    for path in [
        ".awr/project.toml",
        ".gitignore",
        "work.yaml",
        "implementation.txt",
        "oracle.txt",
    ] {
        fs::copy(f.root.join(path), project.join(path)).unwrap();
    }
    let nested = Fixture {
        root: project,
        baseline: f.baseline.clone(),
    };
    nested.local_completed(true);
    nested.commit("Record the nested completed ledger");
    f.git(&["config", "diff.relative", "true"]);
    let v = nested.verify(&["--work", "W"], 0);
    assert_eq!(v["results"][0]["delta_paths"], json!(["app/work.yaml"]));
    fs::write(f.root.join("outside.txt"), "outside this project\n").unwrap();
    f.git(&["add", "outside.txt"]);
    f.git(&["commit", "-m", "Change another part of the repository"]);
    let v = nested.verify(&["--work", "W"], 1);
    nested.assert_no_reuse(&v);
}

#[test]
fn ledger_only_ac6_installed_hook_allows_the_ledger_commit() {
    let f = Fixture::new();
    f.local_completed(true);
    let remote = f.install_push_target(true);
    f.commit("Record the completed ledger before pushing");
    let pushed = f.push();
    assert!(
        pushed.status.success(),
        "{} {}",
        String::from_utf8_lossy(&pushed.stdout),
        String::from_utf8_lossy(&pushed.stderr)
    );
    let target = Command::new("git")
        .arg("--git-dir")
        .arg(remote)
        .args(["rev-parse", "refs/heads/verified"])
        .output()
        .unwrap();
    assert!(target.status.success());
    assert_eq!(
        String::from_utf8(target.stdout).unwrap().trim(),
        f.git(&["rev-parse", "HEAD"])
    );
}

#[test]
fn ledger_only_ac6_installed_hook_refuses_code_changes() {
    let f = Fixture::new();
    f.local_completed(true);
    f.install_push_target(true);
    fs::write(
        f.root.join("implementation.txt"),
        "changed after completion\n",
    )
    .unwrap();
    f.commit("Change code after the verified delivery");
    let pushed = f.push();
    assert!(!pushed.status.success());
    assert!(String::from_utf8_lossy(&pushed.stdout).contains("No associated work"));
}

#[test]
fn ledger_only_ac6_default_hook_preserves_ledger_and_code_push_success() {
    let f = Fixture::new();
    f.local_completed(true);
    f.install_push_target(false);
    f.commit("Record the completed ledger with the default hook");
    assert!(f.push().status.success());
    fs::write(
        f.root.join("implementation.txt"),
        "changed after completion\n",
    )
    .unwrap();
    f.commit("Change code with the default hook");
    assert!(f.push().status.success());
}

#[test]
#[ignore = "subprocess helper selected by the ledger's argument vector"]
fn verify_process_fixture() {
    assert_eq!(
        fs::read_to_string("implementation.txt").unwrap(),
        fs::read_to_string("oracle.txt").unwrap(),
        "saved content differs"
    );
}

#[test]
fn ac1_local_reports_missing_acceptance_with_work_key() {
    let f = Fixture::new();
    f.local_completed(false);
    let v = f.verify(&["--work", "W"], 1);
    assert_eq!(v["results"][0]["work"], "W");
    assert!(v.to_string().contains("acceptance_missing"), "{v}");
}

#[test]
fn ac1_local_accepts_complete_checks_bound_to_the_selected_commit() {
    let f = Fixture::new();
    f.local_completed(true);
    let v = f.verify(&[], 0);
    assert_eq!(v["results"][0]["work"], "W");
    assert_eq!(v["results"][0]["check_gate"]["applies"], true);
    assert_eq!(v["results"][0]["unmet"], json!([]));
}

#[test]
fn local_gate_preserves_the_completion_session_for_independent_approval() {
    let f = Fixture::new();
    f.local_completed(true);
    fs::write(
        f.root.join(".awr/gates.toml"),
        "version=1\n[checks]\nmode='observe'\nscope='all'\nseparate_approver=true\n",
    )
    .unwrap();
    let v = f.verify(&[], 1);
    assert!(v.to_string().contains("approver_is_implementer"), "{v}");
}

#[test]
fn local_selection_uses_completion_source_sha_and_require_work() {
    let f = Fixture::new();
    f.local_completed(true);
    assert_eq!(
        f.verify(&["--commit", &f.baseline], 0)["reason"],
        "no_associated_work"
    );
    f.verify(&["--commit", &f.baseline, "--require-work"], 1);
}

#[test]
fn local_selection_retains_completed_work_after_its_check_definition_changes() {
    let f = Fixture::new();
    f.local_completed(true);
    let mut ledger = f.ledger();
    ledger["work_items"][0]["definition"]["examples"][0]["check"]["run"]
        .as_array_mut()
        .unwrap()
        .push(json!("--quiet"));
    f.save_ledger(&ledger);
    let v = f.verify(&[], 1);
    assert_eq!(v["results"][0]["work"], "W");
    assert!(v.to_string().contains("oracle_changed"), "{v}");
}

#[test]
fn rerun_skips_local_service_maintenance_and_ignores_an_existing_database() {
    let f = Fixture::new();
    let home = f.root.join(".awr/local-service");
    fs::write(f.root.join(".awr/state.db"), "unreadable fixture database").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_awr"))
        .env("AWR_LOCAL_WEB", "1")
        .env("AWR_LOCAL_HOME", &home)
        .args([
            "--project",
            f.root.to_str().unwrap(),
            "--json",
            "verify",
            "--rerun",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!home.exists());
    assert_eq!(
        fs::read_to_string(f.root.join(".awr/state.db")).unwrap(),
        "unreadable fixture database"
    );
    assert!(!f.root.join(".awr/state.db-wal").exists());
    assert!(!f.root.join(".awr/state.db-shm").exists());
}

#[test]
fn ac2_rerun_succeeds_in_a_fresh_checkout() {
    let f = Fixture::new();
    let v = f.verify(&["--rerun"], 0);
    assert_eq!(v["results"][0]["work"], "W");
    assert_eq!(v["results"][0]["met"], true);
    f.assert_no_database();
}

#[test]
fn ac2_rerun_failure_also_leaves_database_and_sidecars_absent() {
    let f = Fixture::new();
    fs::write(f.root.join("implementation.txt"), "broken\n").unwrap();
    f.verify(&["--rerun"], 1);
    f.assert_no_database();
}

#[test]
fn ac3_baseline_fails_with_current_oracle_and_current_commit_passes() {
    let f = Fixture::new();
    let v = f.verify(&["--rerun", "--baseline", &f.baseline], 0);
    assert_eq!(
        v["results"][0]["checks"][0]["baseline"]["outcome"],
        "failed"
    );
    f.assert_no_database();
}

#[cfg(unix)]
#[test]
fn baseline_copies_a_new_executable_check_with_its_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    fs::create_dir_all(f.root.join("checks")).unwrap();
    let check = f.root.join("checks/saved.sh");
    fs::write(
        &check,
        "#!/bin/sh\n[ \"$(cat implementation.txt)\" = after ]\n",
    )
    .unwrap();
    fs::set_permissions(&check, fs::Permissions::from_mode(0o755)).unwrap();
    let mut ledger = f.ledger();
    for e in ledger["work_items"][0]["definition"]["examples"]
        .as_array_mut()
        .unwrap()
    {
        e["check"]["run"] = json!(["./checks/saved.sh"]);
        e["check"]["files"] = json!(["checks/saved.sh"]);
        e["check"]["timeout_seconds"] = json!(30);
    }
    f.save_ledger(&ledger);
    f.git(&["add", "checks/saved.sh"]);
    f.commit("Register the executable check");
    f.verify(&["--rerun", "--baseline", &f.baseline], 0);
    f.assert_no_database();
}

#[test]
fn baseline_handles_nested_projects_and_reads_their_old_ledger() {
    let f = Fixture::new();
    let project = f.root.join("app");
    fs::create_dir_all(project.join(".awr")).unwrap();
    fs::copy(
        f.root.join(".awr/project.toml"),
        project.join(".awr/project.toml"),
    )
    .unwrap();
    let mut ledger = f.ledger();
    ledger["work_items"][0]["status"] = json!("in_progress");
    fs::write(
        project.join("work.yaml"),
        serde_json::to_vec(&ledger).unwrap(),
    )
    .unwrap();
    fs::write(project.join("implementation.txt"), "before\n").unwrap();
    fs::write(project.join("oracle.txt"), "before\n").unwrap();
    f.git(&["add", "app"]);
    f.git(&["commit", "-m", "Create the nested project"]);
    let baseline = f.git(&["rev-parse", "HEAD"]);
    ledger["work_items"][0]["status"] = json!("completed");
    fs::write(
        project.join("work.yaml"),
        serde_json::to_vec(&ledger).unwrap(),
    )
    .unwrap();
    fs::write(project.join("implementation.txt"), "after\n").unwrap();
    fs::write(project.join("oracle.txt"), "after\n").unwrap();
    f.git(&["add", "app"]);
    f.git(&["commit", "-m", "Deliver nested content"]);
    for (base, empty) in [(&baseline[..], false), ("HEAD", true)] {
        let o = f.run_at(&project, &["verify", "--rerun", "--baseline", base]);
        assert!(
            o.status.success(),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["results"].as_array().unwrap().is_empty(), empty, "{v}");
    }
    assert!(!project.join(".awr/state.db").exists());
}

#[cfg(unix)]
#[test]
fn baseline_refuses_a_nested_project_symlink_and_cleans_up_without_writing_outside() {
    let f = Fixture::new();
    let outside = f.root.join(".awr/protected");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("oracle.txt"), "protected content\n").unwrap();
    let project = f.root.join("app");
    std::os::unix::fs::symlink(&outside, &project).unwrap();
    f.git(&["add", "app"]);
    f.git(&["commit", "-m", "Record the older linked directory"]);
    let baseline = f.git(&["rev-parse", "HEAD"]);
    fs::remove_file(&project).unwrap();
    fs::create_dir_all(project.join(".awr")).unwrap();
    for path in [
        ".awr/project.toml",
        "work.yaml",
        "implementation.txt",
        "oracle.txt",
    ] {
        fs::copy(f.root.join(path), project.join(path)).unwrap();
    }
    f.git(&["add", "app"]);
    f.git(&["commit", "-m", "Create the current project directory"]);
    let before = f.git(&["worktree", "list", "--porcelain"]);
    let o = f.run_at(&project, &["verify", "--rerun", "--baseline", &baseline]);
    assert_eq!(
        fs::read_to_string(outside.join("oracle.txt")).unwrap(),
        "protected content\n"
    );
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(before, f.git(&["worktree", "list", "--porcelain"]));
}

#[test]
fn ac3_baseline_that_already_passed_is_rejected() {
    let f = Fixture::new();
    fs::write(f.root.join("oracle.txt"), "before\n").unwrap();
    fs::write(f.root.join("implementation.txt"), "before\n").unwrap();
    f.commit("Keep the original behavior");
    let v = f.verify(&["--rerun", "--baseline", &f.baseline], 1);
    assert!(v.to_string().contains("passed_before_change"), "{v}");
}

#[test]
fn ac4_success_removes_temporary_worktree_and_directory() {
    let f = Fixture::new();
    let before = f.git(&["worktree", "list", "--porcelain"]);
    let v = f.verify(&["--rerun", "--baseline", &f.baseline], 0);
    assert_eq!(before, f.git(&["worktree", "list", "--porcelain"]));
    let path = v["baseline_worktree"].as_str().unwrap();
    assert!(!PathBuf::from(path).exists(), "{path} survived cleanup");
}

#[test]
fn ac4_failed_check_removes_temporary_worktree_and_directory() {
    let f = Fixture::new();
    let before = f.git(&["worktree", "list", "--porcelain"]);
    fs::write(f.root.join("oracle.txt"), "before\n").unwrap();
    fs::write(f.root.join("implementation.txt"), "before\n").unwrap();
    let v = f.verify(&["--rerun", "--baseline", &f.baseline], 1);
    assert_eq!(before, f.git(&["worktree", "list", "--porcelain"]));
    assert!(!PathBuf::from(v["baseline_worktree"].as_str().unwrap()).exists());
}

#[test]
fn ac5_no_newly_completed_work_succeeds_with_an_explanation() {
    let f = Fixture::new();
    let v = f.verify(&["--rerun", "--baseline", "HEAD"], 0);
    assert_eq!(v["results"], json!([]));
    assert_eq!(v["reason"], "no_associated_work");
}

#[test]
fn ac5_require_work_rejects_an_empty_selection() {
    let f = Fixture::new();
    let v = f.verify(&["--rerun", "--baseline", "HEAD", "--require-work"], 1);
    assert_eq!(v["reason"], "no_associated_work");
}

#[test]
fn ac5_project_policy_requires_work_and_rejects_unknown_keys() {
    let f = Fixture::new();
    fs::write(
        f.root.join(".awr/gates.toml"),
        "version=1\n[verify]\nrequire_work=true\n",
    )
    .unwrap();
    f.verify(&["--rerun", "--baseline", "HEAD"], 1);
    fs::write(
        f.root.join(".awr/gates.toml"),
        "version=1\n[verify]\nrequire_wrok=true\n",
    )
    .unwrap();
    assert_eq!(f.run(&["verify", "--rerun"]).status.code(), Some(1));
    f.assert_no_database();
}

#[test]
fn explicit_work_selection_overrides_the_baseline_filter() {
    let f = Fixture::new();
    f.verify(&["--rerun", "--baseline", "HEAD", "--work", "W"], 1);
    assert_eq!(
        f.verify(&["--rerun", "--work", "W", "--work", "W"], 0)["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rerun_missing_work_and_unbound_examples_fail_explicitly() {
    let f = Fixture::new();
    assert_eq!(
        f.run(&["verify", "--rerun", "--work", "MISSING"])
            .status
            .code(),
        Some(1)
    );
    let mut ledger = f.ledger();
    ledger["work_items"][0]["definition"]["examples"][0]
        .as_object_mut()
        .unwrap()
        .remove("check");
    f.save_ledger(&ledger);
    let v = f.verify(&["--rerun"], 1);
    assert!(v.to_string().contains("check_unbound"), "{v}");
}

#[test]
fn baseline_requires_rerun_and_rerun_requires_the_selected_checkout() {
    let f = Fixture::new();
    assert_eq!(
        f.run(&["verify", "--baseline", &f.baseline]).status.code(),
        Some(2)
    );
    assert_eq!(
        f.run(&["verify", "--rerun", "--commit", &f.baseline])
            .status
            .code(),
        Some(1)
    );
    f.assert_no_database();
}

#[test]
fn baseline_spawn_errors_are_not_failed_baseline_proof() {
    let f = Fixture::new();
    let mut ledger = f.ledger();
    for e in ledger["work_items"][0]["definition"]["examples"]
        .as_array_mut()
        .unwrap()
    {
        e["check"]["run"] = json!(["awr-fixture-missing-executable"]);
    }
    f.save_ledger(&ledger);
    let before = f.git(&["worktree", "list", "--porcelain"]);
    f.verify(&["--rerun", "--baseline", &f.baseline], 1);
    assert_eq!(before, f.git(&["worktree", "list", "--porcelain"]));
}

#[test]
fn ac6_hook_preview_then_accept_installs_and_preserves_existing_bytes() {
    let f = Fixture::new();
    let hook = f.root.join(".git/hooks/pre-push");
    let preview = f.ok(&["verify", "install-hook"]);
    assert_eq!(preview["installed"], false);
    assert!(!hook.exists());
    f.ok(&["verify", "install-hook", "--accept"]);
    let bytes = fs::read(&hook).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("AWR_VERIFY"));
    fs::write(&hook, "#!/bin/sh\n# User-owned hook\nexit 7\n").unwrap();
    let existing = fs::read(&hook).unwrap();
    let v = f.ok(&["verify", "install-hook", "--accept"]);
    assert_eq!(v["installed"], false);
    assert!(v["manual_line"].as_str().unwrap().contains("awr verify"));
    assert_eq!(fs::read(&hook).unwrap(), existing);
    f.assert_no_database();
}

#[test]
fn ac6_hook_respects_core_hooks_path() {
    let f = Fixture::new();
    f.git(&["config", "core.hooksPath", "custom hooks"]);
    f.ok(&["verify", "install-hook", "--accept"]);
    assert!(f.root.join("custom hooks/pre-push").is_file());
    assert!(!f.root.join(".git/hooks/pre-push").exists());
}

#[test]
fn verify_is_discoverable_in_daily_help_and_capability_negotiation() {
    let f = Fixture::new();
    f.ok(&["capabilities", "--require", "verify.commit"]);
    let o = f.run(&["help", "daily"]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("verify"));
    f.assert_no_database();
}

#[test]
fn ac6_installed_hook_blocks_push_with_missing_acceptance_and_honors_opt_out() {
    let f = Fixture::new();
    f.local_completed(false);
    f.ok(&["verify", "install-hook", "--accept"]);
    let sha = f.git(&["rev-parse", "HEAD"]);
    let remote = f.root.join(".awr/push-target.git");
    f.git(&["init", "--bare", remote.to_str().unwrap()]);
    f.git(&["remote", "add", "validation", remote.to_str().unwrap()]);
    let mut paths = vec![
        PathBuf::from(env!("CARGO_BIN_EXE_awr"))
            .parent()
            .unwrap()
            .to_owned(),
    ];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(paths).unwrap();
    for (off, success) in [(false, false), (true, true)] {
        let o = Command::new("git")
            .args(["push", "--quiet", "validation", "HEAD:refs/heads/verified"])
            .current_dir(&f.root)
            .env("AWR_LOCAL_WEB", "0")
            .env("PATH", &path)
            .env("AWR_VERIFY", if off { "off" } else { "on" })
            .output()
            .unwrap();
        assert_eq!(
            o.status.success(),
            success,
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        let receipt = Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .args(["rev-parse", "--verify", "refs/heads/verified"])
            .output()
            .unwrap();
        assert_eq!(receipt.status.success(), success);
        if success {
            assert_eq!(String::from_utf8(receipt.stdout).unwrap().trim(), sha);
        }
    }
}
