use awr_core::Id;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const WORK_KEY: &str = "W";
const OTHER_KEY: &str = "OTHER";
const FIRST_CRITERION: &str = "The report covers the first requirement";
const SECOND_CRITERION: &str = "The report covers the second requirement";
const ALTERNATE_CODE_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(label: &str, with_other: bool) -> Self {
        let root =
            std::env::temp_dir().join(format!("awr-evidence-currency-{label}-{}", Id::new()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("work.yaml"), work_yaml(with_other)).unwrap();
        fs::write(
            root.join("manifest.toml"),
            "[project]\nname = 'Evidence currency CLI fixture'\ncontext_profile = 'minimal'\n\n[[sources]]\ndomain = 'ledger'\nrole = 'primary'\npath = 'work.yaml'\nadapter = 'yaml-ledger-v1'\n",
        )
        .unwrap();
        let project = Self { root };
        project.ok(&["init", "--manifest", "manifest.toml", "--accept"]);
        project
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }

    fn run_owned(&self, args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        Self::success(&self.run(args), args)
    }

    fn ok_owned(&self, args: &[String]) -> Value {
        Self::success(&self.run_owned(args), &[])
    }

    fn success(output: &Output, args: &[&str]) -> Value {
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{args:?}: invalid JSON stdout ({error})\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    fn error(&self, args: &[&str], code: &str) -> Value {
        let output = self.run(args);
        Self::error_output(&output, args, code)
    }

    fn error_owned(&self, args: &[String], code: &str) -> Value {
        let output = self.run_owned(args);
        Self::error_output(&output, &[], code)
    }

    fn error_output(output: &Output, args: &[&str], code: &str) -> Value {
        assert!(
            !output.status.success(),
            "{args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            output.stdout.is_empty(),
            "{args:?} wrote success output on failure: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
            panic!(
                "{args:?}: invalid JSON stderr ({error})\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(value["code"], code, "{args:?}: {value}");
        value
    }

    fn revision(&self) -> String {
        self.ok(&["session", "list"])["project_revision"]
            .as_u64()
            .unwrap()
            .to_string()
    }

    fn current_sha(&self) -> String {
        let value = self.ok(&["work", "show", WORK_KEY, "--cached"]);
        let fingerprint = value["source_ref"]["source_fingerprint"].as_str().unwrap();
        fingerprint
            .strip_prefix("sha256:")
            .unwrap_or(fingerprint)
            .to_owned()
    }

    fn start_session(&self, work: &str, agent: &str, claim: bool) -> String {
        let mut args = vec![
            "session".to_owned(),
            "start".to_owned(),
            "--work".to_owned(),
            work.to_owned(),
            "--agent".to_owned(),
            agent.to_owned(),
            "--provider".to_owned(),
            "fixture".to_owned(),
            "--model".to_owned(),
            "cli-test".to_owned(),
            "--expected-revision".to_owned(),
            self.revision(),
        ];
        if claim {
            args.push("--claim".to_owned());
        }
        self.ok_owned(&args)["session"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn write_report(&self, name: &str, work: &str, source_sha: &str) -> PathBuf {
        let criteria = vec![FIRST_CRITERION, SECOND_CRITERION];
        let report = json!({
            "version": 1,
            "work_item": work,
            "source_sha": source_sha,
            "command": "synthetic CLI verification",
            "scope": [work],
            "verified_at": 1,
            "checks": [{
                "name": "acceptance coverage",
                "passed": true,
                "details": "Synthetic report covers the declared acceptance criteria.",
                "criteria": criteria,
            }]
        });
        let path = self.root.join(name);
        fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        path
    }

    fn prepare(&self, work: &str, report: &Path, evidence_key: &str, source_sha: &str) -> Value {
        let report = report.file_name().unwrap().to_str().unwrap();
        let args = [
            "work",
            "prepare-completion",
            work,
            "--report",
            report,
            "--evidence-key",
            evidence_key,
            "--source-sha",
            source_sha,
        ];
        self.ok(&args)
    }

    fn write_prepared_evidence(&self, prepared: &Value, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(
            &path,
            serde_json::to_vec_pretty(&prepared["evidence"]).unwrap(),
        )
        .unwrap();
        path
    }

    fn add_prepared_evidence(&self, prepared: &Value, evidence_file: &Path) -> Value {
        let expected = prepared["project_revision"].as_u64().unwrap().to_string();
        let input = evidence_file.file_name().unwrap().to_str().unwrap();
        let args = vec![
            "evidence".to_owned(),
            "add".to_owned(),
            "--input".to_owned(),
            input.to_owned(),
            "--expected-revision".to_owned(),
            expected,
        ];
        self.ok_owned(&args)
    }

    fn complete(&self, work: &str, session: &str, prepared: &Value) -> Value {
        let completion_file = self.root.join(format!("completion-{work}.json"));
        fs::write(
            &completion_file,
            serde_json::to_vec_pretty(&prepared["completion"]).unwrap(),
        )
        .unwrap();
        let input = completion_file.to_str().unwrap();
        let expected = self.revision();
        let args = vec![
            "work".to_owned(),
            "complete".to_owned(),
            work.to_owned(),
            "--session".to_owned(),
            session.to_owned(),
            "--reason".to_owned(),
            "Verified the synthetic completion report".to_owned(),
            "--input".to_owned(),
            input.to_owned(),
            "--expected-revision".to_owned(),
            expected,
        ];
        self.ok_owned(&args)
    }

    fn evidence_show(&self, key: &str, source_sha: &str) -> Value {
        let args = ["evidence", "show", key, "--source-sha", source_sha];
        self.ok(&args)
    }

    fn work_show(&self, work: &str, source_sha: &str) -> Value {
        let args = ["work", "show", work, "--source-sha", source_sha, "--cached"];
        self.ok(&args)
    }

    fn append_other_work(&self) {
        let mut body = fs::read_to_string(self.root.join("work.yaml")).unwrap();
        body.push_str(&format!(
            "\n- id: {OTHER_KEY}\n  title: Review a second report\n  status: in_progress\n  owner: worker-b\n  acceptance: [{FIRST_CRITERION}, {SECOND_CRITERION}]\n"
        ));
        fs::write(self.root.join("work.yaml"), body).unwrap();
        self.ok(&["source", "reindex"]);
    }

    fn change_acceptance(&self) {
        let body = fs::read_to_string(self.root.join("work.yaml")).unwrap();
        fs::write(
            self.root.join("work.yaml"),
            body.replacen(
                FIRST_CRITERION,
                "The report covers an edited first requirement",
                1,
            ),
        )
        .unwrap();
        self.ok(&["source", "reindex"]);
    }

    fn organize(&self) {
        let body = fs::read_to_string(self.root.join("work.yaml")).unwrap();
        let body = body.replacen(
            "  title: Deliver the report\n",
            "  title: Deliver the report\n  goal: G\n",
            1,
        );
        fs::write(
            self.root.join("work.yaml"),
            format!(
                "goals:\n- id: G\n  title: Deliver the report\n  status: active\n  summary: The user requested the report\n  success_criteria: [The report is delivered]\n{body}"
            ),
        )
        .unwrap();
        self.ok(&["source", "reindex"]);
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn work_yaml(with_other: bool) -> String {
    let other = if with_other {
        format!(
            "\n- id: {OTHER_KEY}\n  title: Review a second report\n  status: in_progress\n  owner: worker-b\n  acceptance: [{FIRST_CRITERION}, {SECOND_CRITERION}]\n"
        )
    } else {
        String::new()
    };
    format!(
        "work_items:\n- id: {WORK_KEY}\n  title: Deliver the report\n  status: in_progress\n  owner: worker-a\n  acceptance: [{FIRST_CRITERION}, {SECOND_CRITERION}]\n  next_action: Verify the report\n{other}"
    )
}

fn completion_gap(value: &Value) -> &Value {
    value["organization"]["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gap| gap["target"] == WORK_KEY && gap["code"] == "completion_verification_failed")
        .unwrap_or_else(|| panic!("missing completion verification gap: {value}"))
}

#[test]
fn cli_completion_receipt_cannot_be_replaced_by_a_passing_report_for_another_sha() {
    let project = Project::new("receipt-sha-replacement", false);
    project.organize();
    let session = project.start_session(WORK_KEY, "worker-a", true);
    let original_sha = project.current_sha();
    let report = project.write_report("selected-report.json", WORK_KEY, &original_sha);
    let prepared = project.prepare(WORK_KEY, &report, "selected", &original_sha);
    let evidence_file = project.write_prepared_evidence(&prepared, "selected.json");
    project.add_prepared_evidence(&prepared, &evidence_file);
    let completed = project.complete(WORK_KEY, &session, &prepared);
    assert_eq!(completed["event"]["event_type"], "work.completed");
    let completed_sha = project.current_sha();
    let baseline = project.ok(&["intake", "inspect", "--source-sha", &completed_sha]);
    assert_eq!(baseline["organization"]["state"], "completed");
    assert_eq!(baseline["organization"]["verified_completed"], 1);

    // This is a different Git/code revision, so the original selected proof is
    // historical for this query. A fresh passing report must not replace it.
    let replacement = project.write_report("replacement-report.json", WORK_KEY, ALTERNATE_CODE_SHA);
    let prepared_replacement =
        project.prepare(WORK_KEY, &replacement, "replacement", ALTERNATE_CODE_SHA);
    let replacement_file =
        project.write_prepared_evidence(&prepared_replacement, "replacement.json");
    project.add_prepared_evidence(&prepared_replacement, &replacement_file);

    let inspect = project.ok(&["intake", "inspect", "--source-sha", ALTERNATE_CODE_SHA]);
    assert_eq!(inspect["organization"]["state"], "awaiting_verification");
    assert_eq!(inspect["organization"]["verified_completed"], 0);
    assert_eq!(inspect["organization"]["completion_verification_failed"], 1);
    assert!(
        completion_gap(&inspect)["detail"]
            .as_str()
            .unwrap()
            .contains(
                "selected evidence selected no longer meets the current verification requirements"
            )
    );

    let status = project.ok(&[
        "status",
        "--view",
        "full",
        "--source-sha",
        ALTERNATE_CODE_SHA,
    ]);
    assert_eq!(status["organization"]["state"], "awaiting_verification");
    assert_eq!(status["organization"]["verified_completed"], 0);
    assert_eq!(status["organization"]["completion_verification_failed"], 1);
    assert!(
        completion_gap(&status)["detail"]
            .as_str()
            .unwrap()
            .contains(
                "selected evidence selected no longer meets the current verification requirements"
            )
    );
}

#[test]
fn cli_multiple_selected_evidence_cannot_be_replaced_after_partial_or_total_damage() {
    let project = Project::new("multiple-selected-damage", false);
    project.organize();
    let session = project.start_session(WORK_KEY, "worker-a", true);
    let original_sha = project.current_sha();

    let report_a = project.write_report("selected-a-report.json", WORK_KEY, &original_sha);
    let prepared_a = project.prepare(WORK_KEY, &report_a, "selected-a", &original_sha);
    let evidence_a_file = project.write_prepared_evidence(&prepared_a, "selected-a.json");
    project.add_prepared_evidence(&prepared_a, &evidence_a_file);

    let report_b = project.write_report("selected-b-report.json", WORK_KEY, &original_sha);
    let prepared_b = project.prepare(WORK_KEY, &report_b, "selected-b", &original_sha);
    let evidence_b_file = project.write_prepared_evidence(&prepared_b, "selected-b.json");
    project.add_prepared_evidence(&prepared_b, &evidence_b_file);

    let mut completion = prepared_a.clone();
    for mapping in completion["completion"]["acceptance"]
        .as_array_mut()
        .unwrap()
    {
        mapping["evidence"] = json!(["selected-a", "selected-b"]);
    }
    let completed = project.complete(WORK_KEY, &session, &completion);
    assert_eq!(completed["event"]["event_type"], "work.completed");
    let completed_sha = project.current_sha();
    let baseline = project.ok(&["intake", "inspect", "--source-sha", &completed_sha]);
    assert_eq!(baseline["organization"]["state"], "completed");
    assert_eq!(baseline["organization"]["verified_completed"], 1);

    // A passing report at the requested SHA cannot replace either selected
    // record when the receipt's two-record selection is checked as a whole.
    let alternate = project.write_report("alternate-report.json", WORK_KEY, ALTERNATE_CODE_SHA);
    let prepared_alternate = project.prepare(WORK_KEY, &alternate, "alternate", ALTERNATE_CODE_SHA);
    let alternate_file = project.write_prepared_evidence(&prepared_alternate, "alternate.json");
    project.add_prepared_evidence(&prepared_alternate, &alternate_file);
    let different_sha = project.ok(&[
        "status",
        "--view",
        "full",
        "--source-sha",
        ALTERNATE_CODE_SHA,
    ]);
    assert_eq!(
        different_sha["organization"]["state"],
        "awaiting_verification"
    );
    assert_eq!(different_sha["organization"]["verified_completed"], 0);
    assert!(
        completion_gap(&different_sha)["detail"]
            .as_str()
            .unwrap()
            .contains(
                "selected evidence selected-a no longer meets the current verification requirements"
            )
    );

    // Register a replacement at the current ledger SHA and corrupt one selected
    // report. The remaining selected report must not make the damaged one
    // replaceable; repeat with both selected reports damaged.
    let current_sha = project.current_sha();
    let current_replacement =
        project.write_report("current-replacement-report.json", WORK_KEY, &current_sha);
    let prepared_current = project.prepare(
        WORK_KEY,
        &current_replacement,
        "current-replacement",
        &current_sha,
    );
    let current_replacement_file =
        project.write_prepared_evidence(&prepared_current, "current-replacement.json");
    project.add_prepared_evidence(&prepared_current, &current_replacement_file);

    let selected_a_bytes = fs::read(project.root.join("selected-a-report.json")).unwrap();
    let selected_b_bytes = fs::read(project.root.join("selected-b-report.json")).unwrap();
    let corrupt = |bytes: &[u8], details: &str| {
        let mut report: Value = serde_json::from_slice(bytes).unwrap();
        report["checks"][0]["details"] = json!(details);
        serde_json::to_vec_pretty(&report).unwrap()
    };

    fs::write(
        project.root.join("selected-a-report.json"),
        corrupt(
            &selected_a_bytes,
            "The first selected observation was altered",
        ),
    )
    .unwrap();
    let partial = project.ok(&["intake", "inspect", "--source-sha", &current_sha]);
    assert_eq!(partial["organization"]["state"], "awaiting_verification");
    assert_eq!(partial["organization"]["verified_completed"], 0);
    assert!(
        completion_gap(&partial)["detail"]
            .as_str()
            .unwrap()
            .contains("registered content bytes no longer match the SHA256 bound at registration")
    );

    fs::write(
        project.root.join("selected-b-report.json"),
        corrupt(
            &selected_b_bytes,
            "The second selected observation was altered",
        ),
    )
    .unwrap();
    let all = project.ok(&["status", "--view", "full", "--source-sha", &current_sha]);
    assert_eq!(all["organization"]["state"], "awaiting_verification");
    assert_eq!(all["organization"]["verified_completed"], 0);
    assert!(
        completion_gap(&all)["detail"]
            .as_str()
            .unwrap()
            .contains("registered content bytes no longer match the SHA256 bound at registration")
    );
}

#[test]
fn cli_completion_reuses_stamped_ledger_evidence_across_two_sessions() {
    let project = Project::new("two-sessions", true);
    let session_a = project.start_session(WORK_KEY, "worker-a", true);
    let session_b = project.start_session(OTHER_KEY, "worker-b", true);

    let initial_sha = project.current_sha();
    let report_a = project.write_report("report-a.json", WORK_KEY, &initial_sha);
    let prepared_a = project.prepare(WORK_KEY, &report_a, "evidence-a", &initial_sha);
    let evidence_a_file = project.write_prepared_evidence(&prepared_a, "evidence-a.json");
    let recorded_a = project.add_prepared_evidence(&prepared_a, &evidence_a_file);
    assert_eq!(recorded_a["evidence"]["external_key"], "evidence-a");
    project.complete(WORK_KEY, &session_a, &prepared_a);

    let after_a_sha = project.current_sha();
    assert_ne!(after_a_sha, initial_sha);
    // Generic evidence inspection remains SHA-strict; the work-aware query
    // carries the ledger snapshot and section bindings needed for reuse.
    assert_eq!(
        project.evidence_show("evidence-a", &after_a_sha)["currency"],
        "historical"
    );
    let after_a = project.work_show(WORK_KEY, &after_a_sha);
    let evidence_a = after_a["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["external_key"] == "evidence-a")
        .unwrap();
    assert_eq!(evidence_a["currency"], "current");
    assert!(
        evidence_a["reasons"].as_array().unwrap().iter().any(
            |reason| reason == "ledger snapshot changed; the stamped work section is unchanged"
        )
    );

    let report_b = project.write_report("report-b.json", OTHER_KEY, &after_a_sha);
    let prepared_b = project.prepare(OTHER_KEY, &report_b, "evidence-b", &after_a_sha);
    let evidence_b_file = project.write_prepared_evidence(&prepared_b, "evidence-b.json");
    project.add_prepared_evidence(&prepared_b, &evidence_b_file);
    project.complete(OTHER_KEY, &session_b, &prepared_b);

    let after_b_sha = project.current_sha();
    assert_ne!(after_b_sha, after_a_sha);
    let inspected = project.work_show(WORK_KEY, &after_b_sha);
    let evidence = inspected["evidence"].as_array().unwrap();
    assert_eq!(
        evidence
            .iter()
            .find(|item| item["external_key"] == "evidence-a")
            .unwrap()["currency"],
        "current"
    );
    let other_inspected = project.work_show(OTHER_KEY, &after_b_sha);
    assert_eq!(
        other_inspected["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["external_key"] == "evidence-b")
            .unwrap()["currency"],
        "current"
    );

    project.change_acceptance();
    let changed_sha = project.current_sha();
    assert_ne!(changed_sha, after_b_sha);
    let changed = project.work_show(WORK_KEY, &changed_sha);
    assert_eq!(
        changed["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["external_key"] == "evidence-a")
            .unwrap()["currency"],
        "historical"
    );
}

#[test]
fn cli_imported_old_report_is_historical_until_fresh_reverification() {
    let producer = Project::new("transport-producer", false);
    let consumer = Project::new("transport-consumer", false);
    let original_sha = producer.current_sha();
    assert_eq!(consumer.current_sha(), original_sha);

    let old_report = producer.write_report("old-report.json", WORK_KEY, &original_sha);
    let old_preflight = producer.prepare(WORK_KEY, &old_report, "producer-only", &original_sha);
    assert_eq!(old_preflight["stage"], "report_validated");
    fs::copy(&old_report, consumer.root.join("old-report.json")).unwrap();

    consumer.append_other_work();
    let drifted_sha = consumer.current_sha();
    assert_ne!(drifted_sha, original_sha);
    let session = consumer.start_session(WORK_KEY, "consumer", true);

    let imported = consumer.prepare(
        WORK_KEY,
        &consumer.root.join("old-report.json"),
        "imported-old",
        &original_sha,
    );
    let imported_file = consumer.write_prepared_evidence(&imported, "imported-old.json");
    consumer.add_prepared_evidence(&imported, &imported_file);
    let imported_inspection = consumer.work_show(WORK_KEY, &drifted_sha);
    assert_eq!(
        imported_inspection["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["external_key"] == "imported-old")
            .unwrap()["currency"],
        "historical"
    );

    let fresh_report = consumer.write_report("fresh-report.json", WORK_KEY, &drifted_sha);
    let fresh = consumer.prepare(WORK_KEY, &fresh_report, "fresh", &drifted_sha);
    let fresh_file = consumer.write_prepared_evidence(&fresh, "fresh.json");
    consumer.add_prepared_evidence(&fresh, &fresh_file);
    consumer.complete(WORK_KEY, &session, &fresh);

    let current_sha = consumer.current_sha();
    assert_ne!(current_sha, drifted_sha);
    let after = consumer.work_show(WORK_KEY, &current_sha);
    let evidence = after["evidence"].as_array().unwrap();
    assert_eq!(
        evidence
            .iter()
            .find(|item| item["external_key"] == "imported-old")
            .unwrap()["currency"],
        "historical"
    );
    assert_eq!(
        evidence
            .iter()
            .find(|item| item["external_key"] == "fresh")
            .unwrap()["currency"],
        "current"
    );
}

#[test]
fn cli_stale_revision_rejects_evidence_without_stamp_or_side_effect() {
    let project = Project::new("revision-conflict", false);
    let source_sha = project.current_sha();
    let report = project.write_report("report.json", WORK_KEY, &source_sha);
    let prepared = project.prepare(WORK_KEY, &report, "interleaved", &source_sha);
    let evidence_file = project.write_prepared_evidence(&prepared, "interleaved.json");
    let prepared_revision = prepared["project_revision"].as_u64().unwrap();
    let source_before = fs::read(project.root.join("work.yaml")).unwrap();

    // A separate CLI process advances the revision between preparation and the
    // write. The stale writer must fail before record_evidence can stamp or insert.
    let session = project.start_session(WORK_KEY, "interleaving-writer", false);
    assert!(!session.is_empty());
    let stale_expected = prepared_revision.to_string();
    let input = evidence_file.file_name().unwrap().to_str().unwrap();
    let args = vec![
        "evidence".to_owned(),
        "add".to_owned(),
        "--input".to_owned(),
        input.to_owned(),
        "--expected-revision".to_owned(),
        stale_expected,
    ];
    let conflict = project.error_owned(&args, "RevisionConflict");
    assert!(conflict["details"]["actual"].as_u64().unwrap() > prepared_revision);
    assert_eq!(
        fs::read(project.root.join("work.yaml")).unwrap(),
        source_before
    );
    assert_eq!(project.current_sha(), source_sha);
    project.error(&["evidence", "show", "interleaved"], "NotFound");
    assert_eq!(
        project.work_show(WORK_KEY, &source_sha)["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["external_key"] == "interleaved")
            .count(),
        0
    );
}

#[test]
fn concurrent_cli_evidence_writers_cannot_both_commit_the_same_revision() {
    for round in 0..8 {
        let project = Project::new(&format!("concurrent-writers-{round}"), false);
        let sha = project.current_sha();
        let report = project.write_report("concurrent-report.json", WORK_KEY, &sha);
        let before = fs::read(project.root.join("work.yaml")).unwrap();
        let revision = project.revision();
        let mut inputs = Vec::new();
        for key in ["writer-a", "writer-b"] {
            let prepared = project.prepare(WORK_KEY, &report, key, &sha);
            let input = project.write_prepared_evidence(&prepared, &format!("{key}.json"));
            inputs.push((key, input));
        }
        let mut children = Vec::new();
        for (key, input) in inputs {
            let child = Command::new(env!("CARGO_BIN_EXE_awr"))
                .args([
                    "--project",
                    project.root.to_str().unwrap(),
                    "--json",
                    "evidence",
                    "add",
                    "--input",
                    input.to_str().unwrap(),
                    "--expected-revision",
                    &revision,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            children.push((key, child));
        }
        let mut successes = 0;
        for (key, child) in children {
            let output = child.wait_with_output().unwrap();
            if output.status.success() {
                successes += 1;
                let recorded = Project::success(&output, &[]);
                assert!(recorded["evidence"]["section_fingerprint"].is_string());
                assert_eq!(recorded["evidence"]["external_key"], key);
            } else {
                Project::error_output(&output, &[], "RevisionConflict");
                project.error(&["evidence", "show", key], "NotFound");
            }
        }
        assert_eq!(successes, 1, "exactly one writer may commit this revision");
        assert_eq!(
            project.revision().parse::<u64>().unwrap(),
            revision.parse::<u64>().unwrap() + 1
        );
        assert_eq!(fs::read(project.root.join("work.yaml")).unwrap(), before);
    }
}

#[test]
fn legacy_payload_without_section_fingerprint_reopens_and_remains_strict() {
    let project = Project::new("legacy-payload", false);
    let original_sha = project.current_sha();
    let report = project.write_report("legacy-report.json", WORK_KEY, &original_sha);
    let prepared = project.prepare(WORK_KEY, &report, "legacy", &original_sha);
    let evidence_file = project.write_prepared_evidence(&prepared, "legacy.json");
    project.add_prepared_evidence(&prepared, &evidence_file);
    assert_eq!(
        project.evidence_show("legacy", &original_sha)["currency"],
        "current"
    );

    // Model an older SQLite payload written before Evidence.section_fingerprint
    // existed. The CLI registration above remains the public write path.
    let database = project.root.join(".awr/state.db");
    let connection = Connection::open(&database).unwrap();
    let payload: String = connection
        .query_row(
            "SELECT payload_json FROM evidence WHERE external_key=?1",
            params!["legacy"],
            |row| row.get(0),
        )
        .unwrap();
    let mut legacy: Value = serde_json::from_str(&payload).unwrap();
    assert!(legacy.get("section_fingerprint").is_some());
    legacy
        .as_object_mut()
        .unwrap()
        .remove("section_fingerprint");
    connection
        .execute(
            "UPDATE evidence SET payload_json=?1 WHERE external_key=?2",
            params![serde_json::to_string(&legacy).unwrap(), "legacy"],
        )
        .unwrap();
    drop(connection);

    // Every CLI invocation reopens the store, so this checks deserialization of
    // a pre-stamp payload as well as its currency behavior after ledger drift.
    assert_eq!(
        project.evidence_show("legacy", &original_sha)["currency"],
        "current"
    );
    project.append_other_work();
    let drifted_sha = project.current_sha();
    let inspected = project.work_show(WORK_KEY, &drifted_sha);
    assert_eq!(
        inspected["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["external_key"] == "legacy")
            .unwrap()["currency"],
        "historical"
    );
    assert_eq!(
        project.evidence_show("legacy", &original_sha)["currency"],
        "current"
    );
}
