//! Execution registration and start, stale writes and an unusable policy against a real indexed
//! project. The CLI tests cover claims; these cover the other gated entries and the store's own
//! guards, which no CLI flag reaches.
use awr_core::*;
use awr_runtime::admit_work;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    fs,
    path::{Path, PathBuf},
};

const ACC: &str = "Saving failure keeps the user's input";
const OUTCOME: &str = "Users keep their input when saving fails";

struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn definition(pin: &str, criterion: &str) -> Value {
    json!({
        "version": 1, "purpose": "implementation", "tier": "required",
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": pin}],
        "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge", "by": "agent-a"},
        "responsibility": {"name": "storage-module", "kind": "delivery"},
        "deliverable": "Local save with a failure fallback",
        "evidence_method": "Targeted tests compared before and after the change",
        "failure_recovery": "A failed save keeps the previous data",
        "examples": [
            {"criterion": criterion, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
            {"criterion": criterion, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"}
        ],
        "discrimination": {"method": "fail_before_pass_after"}
    })
}

fn pin(root: &Path) -> String {
    format!(
        "sha256:{:x}",
        sha2::Sha256::digest(fs::read(root.join("decisions/scope.md")).unwrap())
    )
}

fn write_ledger(root: &Path, criterion: &str) {
    let ledger = json!({
        "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
        "work_items": [{"id": "W", "title": "Save safely", "status": "planned", "goal": "G",
            "acceptance": [criterion], "paths": ["crates/storage/src"], "next_action": "Do the work",
            "definition": definition(&pin(root), criterion)}]
    });
    fs::write(
        root.join("work.yaml"),
        serde_json::to_vec_pretty(&ledger).unwrap(),
    )
    .unwrap();
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-admission-runtime-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::create_dir_all(root.join("decisions")).unwrap();
        fs::write(
            root.join(".awr/project.toml"),
            "[project]\nname = \"Admission runtime fixture\"\nexternal_key = \"admission-runtime-fixture\"\n\n[[sources]]\ndomain = \"ledger\"\nrole = \"primary\"\npath = \"work.yaml\"\nadapter = \"yaml-ledger-v1\"\n\n[sources.options.admission_policy]\nversion = 1\nmode = \"governed_enforce\"\nscope = \"all\"\n\n[[sources]]\ndomain = \"decisions\"\nrole = \"supporting\"\npath = \"decisions\"\nadapter = \"markdown-directory-v1\"\n",
        )
        .unwrap();
        fs::write(
            root.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        write_ledger(&root, ACC);
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let indexed =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(indexed.ok, "{indexed:?}");
        Self {
            root,
            store,
            project: indexed.project_id,
        }
    }

    fn write_ledger(&self, criterion: &str) {
        write_ledger(&self.root, criterion);
    }

    fn reindex(&mut self) {
        let indexed = index_project(
            &mut self.store,
            &self.root,
            &Manifest::load(&self.root).unwrap(),
            false,
        )
        .unwrap();
        assert!(indexed.ok, "{indexed:?}");
    }

    fn revision(&self) -> u64 {
        self.store.project(self.project).unwrap().project_revision
    }

    fn session(&mut self, agent: &str, work: Option<&str>) -> Id {
        let revision = self.revision();
        let draft = SessionDraft {
            work_item_key: work.map(String::from),
            agent_id: agent.into(),
            provider: "test".into(),
            model: "m".into(),
            branch_id: None,
            claim: false,
            claim_ttl_ms: None,
        };
        self.store
            .start_session(self.project, revision, draft)
            .unwrap()
            .0
            .session
            .id
    }

    fn request(&self, session: Id, key: &str) -> AdmissionRequest {
        AdmissionRequest {
            work: "W".into(),
            session: Some(session),
            agent: None,
            expected_revision: self.revision(),
            request_key: key.into(),
            decision: AdmissionDecision::Approved,
            contract_fingerprint: self
                .store
                .work_contract(self.project, "W")
                .unwrap()
                .fingerprint,
            judgment: Some(AdmissionJudgment {
                contribution: "Advances the saving outcome".into(),
                necessity: "The draft is lost today".into(),
                alternatives: "Reuse of the existing buffer was checked".into(),
                deliverable: "A fallback path".into(),
                exit: "Stop when the fallback is verified".into(),
                responsibility: "storage-module".into(),
            }),
            proposal_event: None,
            reason: None,
        }
    }

    fn approve(&mut self, reviewer: Id, key: &str) -> Value {
        let request = self.request(reviewer, key);
        admit_work(&mut self.store, &self.root.clone(), &request).unwrap()
    }

    fn intent(&self, key: &str) -> ExecutionIntent {
        ExecutionIntent {
            operation_key: key.into(),
            purpose: "Run the storage checks".into(),
            executor: ExecutorKind::ManagedLocal,
            command: vec!["true".into()],
            cwd: self.root.to_string_lossy().into(),
            external_reference: None,
        }
    }

    fn worker() -> WorkerIdentity {
        WorkerIdentity {
            nonce: Id::new(),
            pid: 4242,
            port: 4343,
            child_pid: None,
        }
    }
}

fn code(error: Error) -> String {
    match error {
        Error::Admission { code, .. } => code.to_string(),
        other => panic!("expected an admission refusal, got {other:?}"),
    }
}

fn edit_fixture_ledger(f: &mut Fixture, edit: impl FnOnce(&mut Value)) {
    let path = f.root.join("work.yaml");
    let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    edit(&mut ledger["work_items"][0]);
    fs::write(path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
    f.reindex();
}

fn bind_fixture_example_checks(f: &mut Fixture) {
    configure_fixture_example_checks(f);
    for id in ["save-0", "save-1"] {
        record_fixture_baseline(f, id, CheckOutcome::Failed);
    }
}

fn configure_fixture_example_checks(f: &mut Fixture) {
    fs::create_dir_all(f.root.join("tests")).unwrap();
    fs::write(f.root.join("tests/save.rs"), "original test\n").unwrap();
    edit_fixture_ledger(f, |work| {
        for (index, example) in work["definition"]["examples"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            example["id"] = json!(format!("save-{index}"));
            example["check"] = json!({"run": ["storage-check", "--case", index.to_string()], "files": ["tests/save.rs"]});
        }
    });
}

fn record_fixture_baseline(f: &mut Fixture, id: &str, outcome: CheckOutcome) -> Event {
    let session = f.session("baseline-runner", Some("W"));
    let work = f.store.work_item(f.project, "W").unwrap();
    let example = work
        .item
        .definition
        .as_ref()
        .unwrap()
        .examples
        .iter()
        .find(|e| e.id.as_deref() == Some(id))
        .unwrap();
    let check = example.check.as_ref().unwrap();
    let files: Vec<_> = check
        .files
        .iter()
        .map(|path| {
            let bytes = fs::read(f.root.join(path)).unwrap();
            OracleFile {
                path: path.clone(),
                sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
                size: bytes.len() as u64,
            }
        })
        .collect();
    f.store
        .record_check(
            f.project,
            &RecordCheckRequest {
                work: "W".into(),
                session,
                expected_revision: f.revision(),
                payload: CheckPayload {
                    version: 1,
                    example: id.into(),
                    criterion_sha256: format!("{:x}", sha2::Sha256::digest(ACC)),
                    phase: CheckPhase::Baseline,
                    outcome,
                    classification: None,
                    reason: None,
                    log_dir: format!(".awr/artifacts/checks/{session}/{id}"),
                    exit_code: Some(if outcome == CheckOutcome::Failed {
                        1
                    } else {
                        0
                    }),
                    example_oracle: Some(example_oracle(id, &check.run, &files).unwrap()),
                    oracle_files: files,
                    run_sha256: "a".repeat(64),
                    run_preview: "storage-check".into(),
                    source_sha: None,
                    tree_state: CheckTreeState::Unknown,
                    stdout_sha256: "b".repeat(64),
                    stderr_sha256: "c".repeat(64),
                    started_at: 1,
                    finished_at: 2,
                    mutants: None,
                    survivors: None,
                },
            },
        )
        .unwrap()
}

fn admission_value(f: &Fixture) -> Value {
    serde_json::to_value(f.store.admission_status(f.project, "W", None).unwrap()).unwrap()
}

#[test]
fn failed_baselines_for_both_examples_allow_approval_and_freeze_the_oracle() {
    let mut f = Fixture::new();
    bind_fixture_example_checks(&mut f);
    let reviewer = f.session("reviewer", None);
    let approved = f.approve(reviewer, "approve-baselines");
    assert!(
        approved.to_string().contains("oracle_fingerprint"),
        "{approved}"
    );
    let status = admission_value(&f);
    assert!(
        status["current"]["oracle_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(status["baselines"].as_array().unwrap().len(), 2);
    assert!(
        status["baselines"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["state"] == "baseline_recorded")
    );
    let worker = f.session("worker", Some("W"));
    f.store
        .acquire_claim(f.project, f.revision(), worker, None)
        .unwrap();
}

#[test]
fn missing_passing_and_inconclusive_baselines_refuse_approval_with_recovery_details() {
    for (outcome, reason) in [
        (None, "baseline_missing"),
        (Some(CheckOutcome::Passed), "baseline_passed"),
        (Some(CheckOutcome::Errored), "baseline_inconclusive"),
        (Some(CheckOutcome::Unknown), "baseline_inconclusive"),
    ] {
        let mut f = Fixture::new();
        configure_fixture_example_checks(&mut f);
        if let Some(outcome) = outcome {
            record_fixture_baseline(&mut f, "save-0", outcome);
        }
        let reviewer = f.session("reviewer", None);
        let request = f.request(reviewer, "refuse-baseline");
        let error = f.store.record_admission(f.project, &request).unwrap_err();
        let report = error.to_string();
        assert!(report.contains(reason), "{reason}: {report}");
        match outcome {
            None => assert!(
                report.contains("awr work check W --example save-0 --phase baseline --session"),
                "{report}"
            ),
            Some(CheckOutcome::Passed) => {
                assert!(report.contains("passed before the change"), "{report}")
            }
            _ => assert!(report.contains(".awr/artifacts/checks/"), "{report}"),
        }
        assert_eq!(code(error), "admission_obligations_unmet");
    }
}

#[test]
fn oracle_file_byte_change_refuses_claim_until_baselines_and_approval_are_renewed() {
    let mut f = Fixture::new();
    bind_fixture_example_checks(&mut f);
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "original-oracle");
    let worker = f.session("worker", Some("W"));
    fs::write(f.root.join("tests/save.rs"), "Original test\n").unwrap();
    let status = admission_value(&f);
    assert_eq!(status["state"], "stale");
    assert!(
        status["diagnostics"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("current oracle"),
        "{status}"
    );
    let error = f
        .store
        .acquire_claim(f.project, f.revision(), worker, None)
        .unwrap_err();
    assert_eq!(code(error), "admission_stale");
    let request = f.request(reviewer, "old-baselines");
    let error = f.store.record_admission(f.project, &request).unwrap_err();
    assert!(error.to_string().contains("baseline_missing"));
    for id in ["save-0", "save-1"] {
        record_fixture_baseline(&mut f, id, CheckOutcome::Failed);
    }
    f.approve(reviewer, "renewed-oracle");
    f.store
        .acquire_claim(f.project, f.revision(), worker, None)
        .unwrap();
}

#[test]
fn missing_oversized_and_non_file_oracles_make_a_frozen_approval_stale() {
    for kind in ["missing", "oversized", "directory"] {
        let mut f = Fixture::new();
        bind_fixture_example_checks(&mut f);
        let reviewer = f.session("reviewer", None);
        f.approve(reviewer, "readable-oracle");
        let path = f.root.join("tests/save.rs");
        fs::remove_file(&path).unwrap();
        match kind {
            "oversized" => fs::File::create(&path)
                .unwrap()
                .set_len(CHECK_FILE_CAP + 1)
                .unwrap(),
            "directory" => fs::create_dir(&path).unwrap(),
            _ => {}
        }
        let status = admission_value(&f);
        assert_eq!(status["state"], "stale", "{kind}: {status}");
        assert!(
            status["diagnostics"][0]["detail"]
                .as_str()
                .unwrap()
                .contains("oracle")
        );
    }
}

#[test]
fn observe_reports_missing_baselines_and_keeps_claims_and_decisions_available() {
    let mut f = Fixture::new();
    configure_fixture_example_checks(&mut f);
    let manifest = f.root.join(".awr/project.toml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("governed_enforce", "legacy_observe"),
    )
    .unwrap();
    f.reindex();
    let status = admission_value(&f);
    assert_eq!(status["enforced"], false);
    assert_eq!(status["baselines"][0]["state"], "baseline_missing");
    assert!(
        status["diagnostics"]
            .to_string()
            .contains("admission_obligations_unmet")
    );
    let worker = f.session("worker", Some("W"));
    f.store
        .acquire_claim(f.project, f.revision(), worker, None)
        .unwrap();
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "observe-baseline-gap");
}

#[test]
fn an_added_counterexample_needs_only_its_own_baseline() {
    let mut f = Fixture::new();
    bind_fixture_example_checks(&mut f);
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "two-examples");
    edit_fixture_ledger(&mut f, |work| {
        let mut extra = work["definition"]["examples"][1].clone();
        extra["id"] = json!("save-extra");
        extra["given"] = json!("a full disk");
        work["definition"]["examples"]
            .as_array_mut()
            .unwrap()
            .push(extra);
    });
    let status = admission_value(&f);
    assert_eq!(status["state"], "stale");
    assert_eq!(status["baselines"][0]["state"], "baseline_recorded");
    assert_eq!(status["baselines"][1]["state"], "baseline_recorded");
    assert_eq!(status["baselines"][2]["state"], "baseline_missing");
    let request = f.request(reviewer, "missing-extra");
    let error = f.store.record_admission(f.project, &request).unwrap_err();
    assert_eq!(code(error), "admission_obligations_unmet");
    record_fixture_baseline(&mut f, "save-extra", CheckOutcome::Failed);
    f.approve(reviewer, "three-examples");
    assert_eq!(f.store.check_events(f.project, "W", None).unwrap().len(), 3);
    assert_eq!(admission_value(&f)["facts"]["admitted"], true);
}

#[test]
fn inherited_fail_before_pass_after_requires_baselines_but_other_methods_do_not() {
    for method in ["fail_before_pass_after", "diff_mutation"] {
        let mut f = Fixture::new();
        configure_fixture_example_checks(&mut f);
        let path = f.root.join("work.yaml");
        let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut parent = ledger["work_items"][0].clone();
        parent["id"] = json!("P");
        parent["definition"]["discrimination"]["method"] = json!(method);
        ledger["work_items"].as_array_mut().unwrap().push(parent);
        let definition = &mut ledger["work_items"][0]["definition"];
        definition.as_object_mut().unwrap().remove("discrimination");
        definition["inherits"] = json!("P");
        fs::write(&path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
        f.reindex();
        let reviewer = f.session("reviewer", None);
        let request = f.request(reviewer, "inherited-method");
        let result = f.store.record_admission(f.project, &request);
        if method == "fail_before_pass_after" {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("baseline_missing"));
            assert_eq!(code(error), "admission_obligations_unmet");
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn alternative_approval_requires_a_distinct_proposal_session_in_store() {
    let mut f = Fixture::new();
    edit_fixture_ledger(&mut f, |work| {
        work["definition"]["discrimination"] =
            json!({"method":"justified_alternative","note":"Review recorded failure injection"});
    });
    let proposer = f.session("proposer", Some("W"));
    let reviewer = f.session("reviewer", None);
    let inline = f.request(reviewer, "inline-alternative");
    assert_eq!(
        code(f.store.record_admission(f.project, &inline).unwrap_err()),
        "admission_review_not_independent"
    );
    let mut proposal = f.request(proposer, "propose-alternative");
    proposal.decision = AdmissionDecision::Proposed;
    let (event, _) = f.store.record_admission(f.project, &proposal).unwrap();
    let mut own = f.request(proposer, "self-alternative");
    own.judgment = None;
    own.proposal_event = Some(event.id);
    assert_eq!(
        code(f.store.record_admission(f.project, &own).unwrap_err()),
        "admission_review_not_independent"
    );
    let mut independent = f.request(reviewer, "review-alternative");
    independent.judgment = None;
    independent.proposal_event = Some(event.id);
    f.store.record_admission(f.project, &independent).unwrap();
    assert_eq!(admission_value(&f)["state"], "approved");
    assert_eq!(admission_value(&f)["independent_review_required"], true);
}

#[test]
fn inherited_alternative_requires_independent_approval_without_policy_review_tags() {
    let mut f = Fixture::new();
    let path = f.root.join("work.yaml");
    let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut parent = ledger["work_items"][0].clone();
    parent["id"] = json!("P");
    parent["definition"]["discrimination"] =
        json!({"method":"justified_alternative","note":"Review injected failures"});
    ledger["work_items"].as_array_mut().unwrap().push(parent);
    let definition = &mut ledger["work_items"][0]["definition"];
    definition.as_object_mut().unwrap().remove("discrimination");
    definition["inherits"] = json!("P");
    fs::write(path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
    f.reindex();
    let proposer = f.session("proposer", Some("W"));
    let mut request = f.request(proposer, "inherited-alternative");
    assert_eq!(
        code(f.store.record_admission(f.project, &request).unwrap_err()),
        "admission_review_not_independent"
    );
    request.decision = AdmissionDecision::Proposed;
    f.store.record_admission(f.project, &request).unwrap();
    assert_eq!(admission_value(&f)["independent_review_required"], true);
}

#[test]
fn oracle_drift_blocks_execution_registration_and_start_without_source_reindexing() {
    let mut f = Fixture::new();
    bind_fixture_example_checks(&mut f);
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "execution-oracle");
    let worker = f.session("worker", Some("W"));
    let (execution, _) = f
        .store
        .register_execution(f.project, f.revision(), worker, f.intent("before-drift"))
        .unwrap();
    fs::write(f.root.join("tests/save.rs"), "changed test\n").unwrap();
    let error = f
        .store
        .register_execution(f.project, f.revision(), worker, f.intent("after-drift"))
        .unwrap_err();
    assert_eq!(code(error), "admission_stale");
    let error = f
        .store
        .start_execution(f.project, f.revision(), execution.id, Fixture::worker())
        .unwrap_err();
    assert_eq!(code(error), "admission_stale");
}

#[test]
fn legacy_approval_and_proposal_without_oracle_fields_remain_current() {
    for decision in [AdmissionDecision::Approved, AdmissionDecision::Proposed] {
        let mut f = Fixture::new();
        bind_fixture_example_checks(&mut f);
        let reviewer = f.session("reviewer", None);
        let mut request = f.request(reviewer, "legacy-decision");
        request.decision = decision;
        let (event, _) = f.store.record_admission(f.project, &request).unwrap();
        let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
        db.execute("DROP TRIGGER event_no_update", []).unwrap();
        db.execute("UPDATE events SET payload_json=json_remove(payload_json,'$.bound.oracle_fingerprint') WHERE id=?1", [event.id.to_string()]).unwrap();
        drop(db);
        if decision == AdmissionDecision::Proposed {
            let mut request = f.request(reviewer, "approve-legacy-proposal");
            request.proposal_event = Some(event.id);
            request.judgment = None;
            f.store.record_admission(f.project, &request).unwrap();
        }
        let status = admission_value(&f);
        assert_eq!(status["state"], "approved", "{status}");
        assert_eq!(status["facts"]["admitted"], true);
        assert!(status["current"].get("oracle_fingerprint").is_some());
        let worker = f.session("worker", Some("W"));
        f.store
            .acquire_claim(f.project, f.revision(), worker, None)
            .unwrap();
    }
}

#[test]
fn plain_examples_keep_legacy_approval_rules_without_baseline_obligations() {
    let mut f = Fixture::new();
    let reviewer = f.session("reviewer", None);
    let approval = f.approve(reviewer, "plain-examples");
    assert!(
        approval["event"]["payload"]["bound"]
            .get("oracle_fingerprint")
            .is_none()
    );
    let status = admission_value(&f);
    assert_eq!(status["facts"]["admitted"], true);
    assert!(status.get("baselines").is_none());
    let worker = f.session("worker", Some("W"));
    f.store
        .acquire_claim(f.project, f.revision(), worker, None)
        .unwrap();
}

#[test]
fn changing_a_check_argument_invalidates_the_examples_binding_and_refuses_a_claim() {
    let mut f = Fixture::new();
    bind_fixture_example_checks(&mut f);
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "approve-checks");
    let before = f.store.admission_status(f.project, "W", None).unwrap();
    assert_eq!(before.state, "approved");
    let worker = f.session("agent-w", Some("W"));
    edit_fixture_ledger(&mut f, |work| {
        work["definition"]["examples"][0]["check"]["run"][2] = json!("changed-case");
    });
    let after = f.store.admission_status(f.project, "W", None).unwrap();
    assert_eq!(after.state, "stale");
    assert_ne!(
        after.current.unwrap().examples_fingerprint,
        before.current.unwrap().examples_fingerprint
    );
    let revision = f.revision();
    let error = f
        .store
        .acquire_claim(f.project, revision, worker, None)
        .unwrap_err();
    assert_eq!(code(error), "admission_stale");
}

#[test]
fn next_action_and_priority_keep_admission_current_and_allow_a_claim() {
    for (field, value) in [
        ("next_action", json!("Review the saved draft behavior")),
        ("priority", json!("P1")),
    ] {
        let mut f = Fixture::new();
        bind_fixture_example_checks(&mut f);
        let reviewer = f.session("reviewer", None);
        f.approve(reviewer, "approve-checks");
        let before = f.store.admission_status(f.project, "W", None).unwrap();
        let worker = f.session("agent-w", Some("W"));
        edit_fixture_ledger(&mut f, |work| work[field] = value);
        let status = f.store.admission_status(f.project, "W", None).unwrap();
        assert_eq!(status.state, "approved", "{field}");
        assert_eq!(status.current, before.current, "{field}");
        let revision = f.revision();
        let (claim, _) = f
            .store
            .acquire_claim(f.project, revision, worker, None)
            .unwrap();
        assert_eq!(claim.status, "active", "{field}");
    }
}

#[test]
fn summary_edits_still_make_admission_stale_and_refuse_a_claim() {
    let mut f = Fixture::new();
    bind_fixture_example_checks(&mut f);
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "approve-checks");
    let before = f.store.admission_status(f.project, "W", None).unwrap();
    let worker = f.session("agent-w", Some("W"));
    edit_fixture_ledger(&mut f, |work| {
        work["summary"] = json!("The failure path is ready for review");
    });
    let status = f.store.admission_status(f.project, "W", None).unwrap();
    assert_eq!(status.state, "stale");
    assert_ne!(status.current, before.current);
    let revision = f.revision();
    let error = f
        .store
        .acquire_claim(f.project, revision, worker, None)
        .unwrap_err();
    assert_eq!(code(error), "admission_stale");
}

#[test]
fn executions_need_a_current_decision_to_register_and_again_to_start() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    // A session bound to the work but holding no claim still cannot register an execution.
    let revision = f.revision();
    let refused = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap_err();
    assert_eq!(code(refused), "admission_missing");

    f.approve(reviewer, "approve-1");
    let revision = f.revision();
    let (registered, _) = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap();
    assert_eq!(registered.state, ExecutionState::Registered);
    // A retry of the same request registers nothing twice: one operation key, one execution.
    let revision = f.revision();
    let again = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap_err();
    assert!(matches!(again, Error::SourceConflict(_)), "{again:?}");
    assert_eq!(f.store.executions(f.project, None).unwrap().len(), 1);

    // The scope moves between registration and start: the start is refused and nothing is dispatched.
    f.write_ledger("Saving failure keeps all of the user's input");
    f.reindex();
    let revision = f.revision();
    let refused = f
        .store
        .start_execution(f.project, revision, registered.id, Fixture::worker())
        .unwrap_err();
    assert_eq!(code(refused), "admission_stale");
    let still = f.store.execution(f.project, registered.id).unwrap();
    assert_eq!(
        still.state,
        ExecutionState::Registered,
        "a refused start dispatches nothing"
    );

    // A new decision on the current scope lets the same execution start once, and only once.
    f.approve(reviewer, "approve-2");
    let revision = f.revision();
    let (started, _) = f
        .store
        .start_execution(f.project, revision, registered.id, Fixture::worker())
        .unwrap();
    assert_eq!(started.state, ExecutionState::Starting);
    let revision = f.revision();
    assert!(
        f.store
            .start_execution(f.project, revision, registered.id, Fixture::worker())
            .is_err(),
        "no second dispatch"
    );
}

#[test]
fn execution_registration_checks_public_command_arguments_without_cli_preflight() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "approve-public-command");

    let mut public = f.intent("public-command");
    public.command = vec!["public-build".into(), "--config.ignore-scripts=true".into()];
    let revision = f.revision();
    let (registered, _) = f
        .store
        .register_execution(f.project, revision, worker, public.clone())
        .unwrap();
    assert_eq!(registered.state, ExecutionState::Registered);
    assert_eq!(registered.intent.command, public.command);
    assert_eq!(f.store.executions(f.project, None).unwrap().len(), 1);
    assert_eq!(f.revision(), revision + 1);

    for (key, command, category) in [
        (
            "standalone-environment",
            vec!["env", "PATH=/synthetic/bin", "public-build"],
            "environment_dump",
        ),
        (
            "synthetic-credential",
            vec!["public-build", "Basic YTpi"],
            "credential",
        ),
        (
            "synthetic-labelled-value",
            vec!["public-build", "SYNTHETIC_PASSWORD=fixture-labelled-value"],
            "labelled_value",
        ),
        (
            "environment-dump",
            vec![
                "public-build",
                "SYNTHETIC_HOME=/synthetic/home\nSYNTHETIC_BIN=/synthetic/bin",
            ],
            "environment_dump",
        ),
    ] {
        let mut intent = f.intent(key);
        intent.command = command.into_iter().map(String::from).collect();
        let revision = f.revision();
        let count = f.store.executions(f.project, None).unwrap().len();
        let error = f
            .store
            .register_execution(f.project, revision, worker, intent)
            .unwrap_err();
        assert!(
            matches!(&error, Error::RuleViolation(_)),
            "{key}: {error:?}"
        );
        let report = error.report();
        assert_eq!(report.code, "RuleViolation", "{key}");
        let details = report.details.unwrap();
        assert_eq!(details["category"], category, "{key}");
        assert_eq!(details["policy_version"], SECRET_POLICY_VERSION, "{key}");
        assert_eq!(
            f.store.executions(f.project, None).unwrap().len(),
            count,
            "{key}: a refusal must not register an execution"
        );
        assert_eq!(
            f.revision(),
            revision,
            "{key}: a refusal must not advance the project revision"
        );
    }
}

#[test]
fn a_decision_computed_against_an_older_revision_is_rejected_as_a_stale_write() {
    let mut f = Fixture::new();
    let reviewer = f.session("reviewer", None);
    let request = f.request(reviewer, "late");
    // The source moves after the decider read the contract: the revision guard refuses the write.
    f.write_ledger("Saving failure keeps all of the user's input");
    f.reindex();
    let root = f.root.clone();
    let error = admit_work(&mut f.store, &root, &request).unwrap_err();
    assert!(
        matches!(
            error,
            Error::RevisionConflict { .. } | Error::SourceConflict(_)
        ),
        "{error:?}"
    );
    // Nothing was recorded, so the work stays unadmitted.
    let status = f.store.admission_status(f.project, "W", None).unwrap();
    assert_eq!(status.state, "missing");
}

#[test]
fn an_unusable_stored_policy_fails_closed_for_claims_and_executions() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "approve-1");
    // The manifest validates policies, so corrupt the stored source configuration directly.
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    let corrupted = db
        .execute(
            "UPDATE sources SET config_json=json_set(config_json,'$.adapter_options.admission_policy.version',9) WHERE domain='ledger'",
            [],
        )
        .unwrap();
    assert_eq!(corrupted, 1);
    drop(db);
    let status = f.store.admission_status(f.project, "W", None).unwrap();
    assert_eq!((status.state, status.enforced), ("policy_invalid", true));
    let revision = f.revision();
    let refused = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap_err();
    assert_eq!(code(refused), "admission_policy_invalid");
}

#[test]
fn a_state_that_cannot_be_established_is_unknown_and_never_an_error_or_a_pass() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.approve(reviewer, "approve-1");
    // Events are append-only, so damaging one needs the guard removed first: this stands for a
    // corrupted store, the one way a recorded decision can fail to read back.
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    db.execute("DROP TRIGGER event_no_update", []).unwrap();
    let damaged = db
        .execute(
            "UPDATE events SET payload_json='{\"version\":1}' WHERE event_type='work.admission'",
            [],
        )
        .unwrap();
    assert_eq!(damaged, 1);
    drop(db);
    let revision = f.revision();
    let refused = f
        .store
        .register_execution(f.project, revision, worker, f.intent("op-1"))
        .unwrap_err();
    assert_eq!(code(refused), "admission_unknown");
    // Readiness reports the reason instead of failing, so every listing keeps answering.
    let readiness = f.store.work_readiness(f.project, "W", None, 0).unwrap();
    assert!(
        readiness
            .diagnostics
            .iter()
            .any(|d| d.code == "admission_unknown"),
        "{:?}",
        readiness.diagnostics
    );
    assert!(!readiness.ready);
}
