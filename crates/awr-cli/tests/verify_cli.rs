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
