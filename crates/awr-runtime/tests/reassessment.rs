//! Reassessment against a real indexed project: executions that fail, outcomes nobody knows, host
//! observations and the decisions that answer them. The checks use the store and the runtime
//! directly, because a failing execution of a known command is easiest to set up there; the CLI
//! tests cover the command and the blocks it shows.
use awr_core::*;
use awr_runtime::{
    AssessManagementRequest, ManageWorkRequest, PrepareWorkRequest, admit_work, assess_management,
    manage_work, prepare_work, reassess_work,
};
use awr_source::{Manifest, index_project};
use awr_store::{EventQuery, Store};
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

fn definition(pin: &str) -> Value {
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
            {"criterion": ACC, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
            {"criterion": ACC, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"}
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

fn write_ledger(root: &Path, blocker: Option<&str>) {
    let work = |key: &str, blocker: Option<&str>| {
        let mut work = json!({"id": key, "title": format!("Save safely {key}"), "status": "planned", "goal": "G",
            "acceptance": [ACC], "paths": ["crates/storage/src"], "next_action": "Do the work",
            "definition": definition(&pin(root))});
        if let Some(blocker) = blocker {
            work["blocker"] = json!(blocker);
        }
        work
    };
    let ledger = json!({
        "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
        "work_items": [work("W", blocker), work("V", None)]
    });
    fs::write(
        root.join("work.yaml"),
        serde_json::to_vec_pretty(&ledger).unwrap(),
    )
    .unwrap();
}

fn code(error: Error) -> String {
    match error {
        Error::Admission { code, .. } => code.to_string(),
        other => panic!("expected an admission refusal, got {other:?}"),
    }
}

impl Fixture {
    fn new(mode: &str) -> Self {
        Self::with_scope(mode, "scope = \"all\"")
    }

    /// `scope` is the TOML line that says which work the policy governs.
    fn with_scope(mode: &str, scope: &str) -> Self {
        Self::with_policy(mode, scope, "")
    }

    /// `extra` is added to the policy table, such as the supervisors.
    fn with_policy(mode: &str, scope: &str, extra: &str) -> Self {
        let root = std::env::temp_dir().join(format!("awr-reassessment-runtime-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::create_dir_all(root.join("decisions")).unwrap();
        fs::write(
            root.join(".awr/project.toml"),
            format!("[project]\nname = \"Reassessment runtime fixture\"\nexternal_key = \"reassessment-runtime-fixture\"\n\n[[sources]]\ndomain = \"ledger\"\nrole = \"primary\"\npath = \"work.yaml\"\nadapter = \"yaml-ledger-v1\"\n\n[sources.options.admission_policy]\nversion = 1\nmode = \"{mode}\"\n{scope}\n{extra}\n[[sources]]\ndomain = \"decisions\"\nrole = \"supporting\"\npath = \"decisions\"\nadapter = \"markdown-directory-v1\"\n"),
        )
        .unwrap();
        fs::write(
            root.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        write_ledger(&root, None);
        let root = root.canonicalize().unwrap();
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

    fn edit_ledger(&mut self, edit: impl FnOnce(&mut Value)) {
        let path = self.root.join("work.yaml");
        let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        edit(&mut ledger);
        fs::write(&path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
        self.reindex();
    }

    fn branch(&mut self, name: &str) -> Id {
        let revision = self.revision();
        self.store
            .create_branch(
                self.project,
                revision,
                BranchDraft {
                    name: name.into(),
                    parent_branch_id: None,
                    git_binding: None,
                    actor: "claude-run".into(),
                    reason: "Try the other method on its own branch".into(),
                },
            )
            .unwrap()
            .0
            .id
    }

    fn session_on(&mut self, branch: Id, agent: &str, work: Option<&str>) -> Id {
        let revision = self.revision();
        self.store
            .start_session(
                self.project,
                revision,
                SessionDraft {
                    work_item_key: work.map(String::from),
                    agent_id: agent.into(),
                    provider: "test".into(),
                    model: "m".into(),
                    branch_id: Some(branch),
                    claim: false,
                    claim_ttl_ms: None,
                },
            )
            .unwrap()
            .0
            .session
            .id
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

    /// An approved admission decision for `work`, so that its executions can be registered.
    fn admit(&mut self, work: &str, reviewer: Id) {
        let request = AdmissionRequest {
            work: work.into(),
            session: Some(reviewer),
            agent: None,
            expected_revision: self.revision(),
            request_key: format!("admit-{work}"),
            decision: AdmissionDecision::Approved,
            contract_fingerprint: self
                .store
                .work_contract(self.project, work)
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
        };
        admit_work(&mut self.store, &self.root.clone(), &request).unwrap();
    }

    fn intent(&self, key: &str, command: &str) -> ExecutionIntent {
        ExecutionIntent {
            operation_key: key.into(),
            purpose: "Run the storage checks".into(),
            executor: ExecutorKind::ManagedLocal,
            command: vec![command.into()],
            cwd: self.root.to_string_lossy().into(),
            external_reference: None,
        }
    }

    /// One whole failed attempt of a command in a directory under the project.
    fn fail_in(&mut self, session: Id, key: &str, command: &str, directory: &str) {
        let revision = self.revision();
        let mut intent = self.intent(key, command);
        intent.cwd = self.root.join(directory).to_string_lossy().into();
        let (registered, _) = self
            .store
            .register_execution(self.project, revision, session, intent)
            .unwrap();
        let nonce = Id::new();
        let revision = self.revision();
        self.store
            .start_execution(
                self.project,
                revision,
                registered.id,
                WorkerIdentity {
                    nonce,
                    pid: 4242,
                    port: 4343,
                    child_pid: None,
                },
            )
            .unwrap();
        self.finish(registered.id, nonce, false);
    }

    fn register(&mut self, session: Id, key: &str, command: &str) -> Result<Execution> {
        let revision = self.revision();
        let intent = self.intent(key, command);
        self.store
            .register_execution(self.project, revision, session, intent)
            .map(|(execution, _)| execution)
    }

    /// Registers an execution and starts it; the run is left in flight.
    fn begin(&mut self, session: Id, key: &str, command: &str) -> (Id, Id) {
        let registered = self.register(session, key, command).unwrap();
        let nonce = Id::new();
        let revision = self.revision();
        self.store
            .start_execution(
                self.project,
                revision,
                registered.id,
                WorkerIdentity {
                    nonce,
                    pid: 4242,
                    port: 4343,
                    child_pid: None,
                },
            )
            .unwrap();
        (registered.id, nonce)
    }

    fn finish(&mut self, execution: Id, nonce: Id, success: bool) {
        let revision = self.revision();
        self.store
            .finish_execution(
                self.project,
                revision,
                ExecutionResult {
                    execution_id: execution,
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

    /// One whole attempt of a method, ending in success or failure.
    fn attempt(&mut self, session: Id, key: &str, command: &str, success: bool) -> Id {
        let (id, nonce) = self.begin(session, key, command);
        self.finish(id, nonce, success);
        id
    }

    fn status(&self, work: &str) -> ReassessmentStatus {
        self.store
            .reassessment_status(self.project, work, None)
            .unwrap()
    }

    fn request(&self, key: &str, choice: ReassessmentChoice) -> ReassessmentRequest {
        let status = self.status("W");
        ReassessmentRequest {
            work: "W".into(),
            session: None,
            agent: Some("claude-run".into()),
            expected_revision: self.revision(),
            request_key: key.into(),
            decision: choice,
            facts_fingerprint: status
                .current
                .expect("the facts are bound while a trigger holds")
                .facts_fingerprint,
            judgment: ReassessmentJudgment {
                reason: "The failures came from the test being fixed".into(),
                next_step: "Run the check again after the fix".into(),
                evidence_considered: "The output of the failed runs".into(),
            },
            root_cause: None,
            answers: ReassessmentSubject::Facts,
            known_issues: vec![],
            reduced_scope: None,
            escalated_to: None,
        }
    }

    fn decide(&mut self, key: &str, choice: ReassessmentChoice) -> Result<Value> {
        let request = self.request(key, choice);
        reassess_work(&mut self.store, &self.root.clone(), &request)
    }

    fn codes(&self, work: &str) -> Vec<String> {
        self.store
            .work_readiness(self.project, work, None, now_millis().unwrap())
            .unwrap()
            .diagnostics
            .into_iter()
            .map(|d| d.code)
            .collect()
    }
}

fn state(f: &Fixture) -> &'static str {
    f.status("W").state
}

#[test]
fn normal_progress_and_isolated_failures_never_ask_for_a_decision() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    assert_eq!(state(&f), "clear");
    assert!(f.status("W").deviations.is_empty());
    // Work with nothing to show gains no block.
    assert!(
        f.store
            .reassessment_block(f.project, "W", None)
            .unwrap()
            .is_none()
    );
    // Red, green, red, green, red: every failure is followed by a success of the same method.
    for (n, success) in [false, true, false, true, false].into_iter().enumerate() {
        f.attempt(worker, &format!("op-{n}"), "check", success);
    }
    let status = f.status("W");
    assert_eq!(status.state, "clear", "{status:?}");
    // The failures are classified and keep their evidence, yet nothing is asked for.
    assert_eq!(status.deviations.len(), 3);
    assert!(
        status
            .deviations
            .iter()
            .all(|d| d.kind == DeviationKind::FailedMethod && d.basis == "recorded")
    );
    assert!(status.diagnostics.is_empty());
    // What the records show is reported even though nothing is asked for.
    let block = f
        .store
        .reassessment_block(f.project, "W", None)
        .unwrap()
        .expect("deviations are shown");
    assert_eq!((block.state, block.deviations.len()), ("clear", 3));
    assert!(
        !f.codes("W").iter().any(|c| c.starts_with("reassessment_")),
        "{:?}",
        f.codes("W")
    );
    // Two failures in a row are still ordinary work.
    f.attempt(worker, "op-5", "check", true);
    f.attempt(worker, "op-6", "check", false);
    f.attempt(worker, "op-7", "check", false);
    assert_eq!(state(&f), "clear");
    f.register(worker, "op-8", "check").unwrap();
}

#[test]
fn three_failures_in_a_row_need_a_current_decision_and_a_further_failure_makes_it_stale() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    let failed: Vec<Id> = (1..=3)
        .map(|n| f.attempt(worker, &format!("op-{n}"), "check", false))
        .collect();
    let status = f.status("W");
    assert_eq!(status.state, "required");
    assert_eq!(status.triggers.len(), 1);
    assert_eq!(status.triggers[0].trigger, "repeated_failed_method");
    let mut expected: Vec<String> = failed.iter().map(|id| id.to_string()).collect();
    expected.sort();
    assert_eq!(status.triggers[0].references, expected);
    // The work itself is held: any new attempt, of this or another method, is refused ...
    assert!(f.codes("W").contains(&"reassessment_required".to_string()));
    for (key, command) in [("op-4", "check"), ("op-5", "another-check")] {
        let refused = f.register(worker, key, command).unwrap_err();
        assert_eq!(code(refused), "reassessment_required");
    }
    // ... until a decision on exactly these facts is recorded.
    let receipt = f.decide("decide-1", ReassessmentChoice::Continue).unwrap();
    assert_eq!(receipt["already_recorded"], false);
    assert_eq!(receipt["reassessment"]["state"], "current");
    assert_eq!(receipt["execution_started"], false);
    assert!(!f.codes("W").iter().any(|c| c.starts_with("reassessment_")));
    let fourth = f.attempt(worker, "op-4", "check", false);
    // The fourth failure contradicts "continue": the decision no longer covers the facts.
    let status = f.status("W");
    assert_eq!(status.state, "stale");
    assert!(
        status.diagnostics[0]
            .detail
            .contains(&format!("fact_added:{fourth}")),
        "{status:?}"
    );
    assert_eq!(
        code(f.register(worker, "op-5", "check").unwrap_err()),
        "reassessment_stale"
    );
    // Changing the method is a decision too, and it frees the work again.
    f.decide("decide-2", ReassessmentChoice::AlterMethod)
        .unwrap();
    assert_eq!(state(&f), "current");
    f.attempt(worker, "op-5", "a-different-check", true);
    // Every decision stays on record.
    let history = f
        .store
        .query_events(
            f.project,
            &EventQuery {
                event_type: Some(REASSESSMENT_EVENT.into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(history.events.len(), 2);
}

#[test]
fn a_success_ends_the_streak_and_a_decision_for_old_facts_does_not_answer_new_ones() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    for n in 1..=3 {
        f.attempt(worker, &format!("op-{n}"), "check", false);
    }
    f.decide("decide-1", ReassessmentChoice::Continue).unwrap();
    // The repair works: the method succeeds and nothing is asked for any more.
    f.attempt(worker, "op-4", "check", true);
    let status = f.status("W");
    assert_eq!(status.state, "clear");
    assert!(
        status.decision_event.is_some(),
        "the decision stays on record"
    );
    // Later trouble starts a new streak; the old decision answered other facts.
    for n in 5..=7 {
        f.attempt(worker, &format!("op-{n}"), "check", false);
    }
    let status = f.status("W");
    assert_eq!(status.state, "required", "{status:?}");
    assert!(status.diagnostics[0].detail.contains("other facts"));
}

#[test]
fn decisions_are_checked_against_the_current_facts_and_who_decides() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    // Nothing calls for a decision yet.
    let premature = ReassessmentRequest {
        facts_fingerprint: format!("sha256:{}", "a".repeat(64)),
        ..f.request_without_facts("early")
    };
    let error = reassess_work(&mut f.store, &f.root.clone(), &premature).unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    let ids: Vec<Id> = (1..=3)
        .map(|n| f.attempt(worker, &format!("op-{n}"), "check", false))
        .collect();
    // Facts other than the current ones are refused as a conflict.
    let wrong = ReassessmentRequest {
        facts_fingerprint: format!("sha256:{}", "a".repeat(64)),
        ..f.request("wrong", ReassessmentChoice::Continue)
    };
    let error = reassess_work(&mut f.store, &f.root.clone(), &wrong).unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    // A root cause may only group the records of this work.
    let mut foreign = f.request("foreign", ReassessmentChoice::Continue);
    foreign.root_cause = Some(RootCause {
        label: "The fixture is read-only".into(),
        references: vec![Id::new().to_string()],
    });
    let error = reassess_work(&mut f.store, &f.root.clone(), &foreign).unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    // A stale write is refused before anything is recorded.
    let mut late = f.request("late", ReassessmentChoice::Continue);
    late.expected_revision -= 1;
    let error = reassess_work(&mut f.store, &f.root.clone(), &late).unwrap_err();
    assert!(matches!(error, Error::RevisionConflict { .. }), "{error:?}");
    assert_eq!(state(&f), "required");
    // A session that has ended decides nothing.
    let gone = f.session("agent-gone", None);
    let revision = f.revision();
    f.store
        .end_session(f.project, revision, gone, SessionOutcome::Ended)
        .unwrap();
    let mut by_session = f.request("by-session", ReassessmentChoice::Continue);
    by_session.agent = None;
    by_session.session = Some(gone);
    let error = reassess_work(&mut f.store, &f.root.clone(), &by_session).unwrap_err();
    assert!(matches!(error, Error::InvalidTransition(_)), "{error:?}");
    // An attributed grouping is recorded with the decision.
    let mut grouped = f.request("grouped", ReassessmentChoice::AlterMethod);
    grouped.root_cause = Some(RootCause {
        label: "The fixture is read-only".into(),
        references: ids.iter().map(|id| id.to_string()).collect(),
    });
    let first = reassess_work(&mut f.store, &f.root.clone(), &grouped).unwrap();
    assert_eq!(
        first["event"]["payload"]["root_cause"]["label"],
        "The fixture is read-only"
    );
    assert_eq!(first["event"]["payload"]["decider_agent"], "claude-run");
    // The same request replays; a different one under the same key does not.
    let mut again = grouped.clone();
    again.expected_revision = f.revision();
    let replay = reassess_work(&mut f.store, &f.root.clone(), &again).unwrap();
    assert_eq!(replay["already_recorded"], true);
    assert_eq!(replay["event"]["id"], first["event"]["id"]);
    let mut other = grouped.clone();
    other.decision = ReassessmentChoice::Replan;
    let error = reassess_work(&mut f.store, &f.root.clone(), &other).unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    // A session that is active decides under its own agent.
    let reviewer_request = ReassessmentRequest {
        session: Some(reviewer),
        agent: None,
        request_key: "by-reviewer".into(),
        expected_revision: f.revision(),
        ..f.request("by-reviewer", ReassessmentChoice::Continue)
    };
    let receipt = reassess_work(&mut f.store, &f.root.clone(), &reviewer_request).unwrap();
    assert_eq!(receipt["event"]["payload"]["decider_agent"], "reviewer");
    assert_eq!(receipt["event"]["session_id"], json!(reviewer));
}

impl Fixture {
    /// A request for a moment when no fact is bound yet.
    fn request_without_facts(&self, key: &str) -> ReassessmentRequest {
        ReassessmentRequest {
            work: "W".into(),
            session: None,
            agent: Some("claude-run".into()),
            expected_revision: self.revision(),
            request_key: key.into(),
            decision: ReassessmentChoice::Continue,
            facts_fingerprint: format!("sha256:{}", "a".repeat(64)),
            judgment: ReassessmentJudgment {
                reason: "Looked at it".into(),
                next_step: "Go on".into(),
                evidence_considered: "Nothing yet".into(),
            },
            root_cause: None,
            answers: ReassessmentSubject::Facts,
            known_issues: vec![],
            reduced_scope: None,
            escalated_to: None,
        }
    }
}

#[test]
fn an_outcome_nobody_knows_holds_new_attempts_until_it_is_resolved() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    let (execution, nonce) = f.begin(worker, "op-1", "build");
    // While its session is active the run is simply in flight.
    assert_eq!(state(&f), "clear");
    let revision = f.revision();
    f.store
        .end_session(f.project, revision, worker, SessionOutcome::Incomplete)
        .unwrap();
    let status = f.status("W");
    assert_eq!(status.state, "required", "{status:?}");
    assert_eq!(status.triggers[0].trigger, "unknown_outcome_unresolved");
    assert_eq!(status.deviations[0].kind, DeviationKind::UnknownOutcome);
    assert_eq!(status.deviations[0].reference, execution.to_string());
    // Nobody may retry blindly ...
    let second = f.session("agent-2", Some("W"));
    assert_eq!(
        code(f.register(second, "op-2", "build").unwrap_err()),
        "reassessment_required"
    );
    // ... yet recovery is never held: the supervisor can still report how it ended.
    f.finish(execution, nonce, false);
    let status = f.status("W");
    assert_eq!(status.state, "clear", "{status:?}");
    assert_eq!(status.deviations[0].kind, DeviationKind::FailedMethod);
    f.register(second, "op-2", "build").unwrap();
}

fn observation(known: Option<bool>, at: i64) -> ManagementObservation {
    ManagementObservation {
        observed_at: at,
        note: "The host lost the connection while the check ran".into(),
        outcome_known: known,
        ..Default::default()
    }
}

#[test]
fn a_host_observation_counts_only_while_it_is_current() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    let record = |f: &mut Fixture, key: &str, known: Option<bool>, at: i64| {
        let fingerprint = assess_management(
            &f.store,
            &f.root,
            &AssessManagementRequest {
                work: "W".into(),
                branch: None,
            },
        )
        .unwrap()["contract_fingerprint"]
            .as_str()
            .unwrap()
            .to_string();
        let expected_revision = f.revision();
        manage_work(
            &mut f.store,
            &f.root.clone(),
            &ManageWorkRequest {
                work: "W".into(),
                session: worker,
                expected_revision,
                request_key: key.into(),
                contract_fingerprint: fingerprint,
                observation: observation(known, at),
            },
        )
        .unwrap();
    };
    record(&mut f, "unknown-1", Some(false), now_millis().unwrap());
    let status = f.status("W");
    assert_eq!(status.state, "required", "{status:?}");
    assert_eq!(status.deviations.len(), 1);
    assert_eq!(status.deviations[0].kind, DeviationKind::UnknownOutcome);
    assert_eq!(status.deviations[0].basis, "host_reported");
    assert_eq!(
        code(f.register(worker, "op-1", "build").unwrap_err()),
        "reassessment_required"
    );
    // The host finds out and says so: the newer observation replaces the old one.
    record(&mut f, "known-1", Some(true), now_millis().unwrap());
    assert_eq!(state(&f), "clear");
    assert!(f.status("W").deviations.is_empty());
    // An observation that says nothing about the outcome says nothing against it.
    record(&mut f, "silent-1", None, now_millis().unwrap());
    assert_eq!(state(&f), "clear");
    assert!(f.status("W").deviations.is_empty());
    // An observation older than a record it would speak about is stale and triggers nothing.
    record(
        &mut f,
        "unknown-2",
        Some(false),
        now_millis().unwrap() - 5_000,
    );
    assert_eq!(state(&f), "required");
    f.decide("decide-1", ReassessmentChoice::Wait).unwrap();
    f.attempt(worker, "op-1", "build", true);
    let status = f.status("W");
    assert_eq!(status.state, "clear", "{status:?}");
    assert_eq!(status.stale_observations.len(), 1);
    assert!(status.deviations.is_empty());
}

#[test]
fn a_declared_blocker_and_an_unanswered_wait_are_classified_and_hold_nothing() {
    let mut f = Fixture::new("governed_enforce");
    write_ledger(&f.root, Some("Waiting for the vendor API key"));
    f.reindex();
    let status = f.status("W");
    assert_eq!(status.state, "clear");
    assert_eq!(status.deviations.len(), 1);
    assert_eq!(status.deviations[0].kind, DeviationKind::ObjectiveBlocker);
    assert_eq!(status.deviations[0].basis, "source_declared");
    assert!(status.deviations[0].detail.contains("vendor API key"));
    write_ledger(&f.root, None);
    f.reindex();
    assert!(f.status("W").deviations.is_empty());
    // An unanswered wait for the user is a deviation of its own kind.
    let revision = f.revision();
    let session = f
        .store
        .start_bound_session(
            f.project,
            revision,
            SessionDraft {
                work_item_key: Some("W".into()),
                agent_id: "writer".into(),
                provider: "fixture".into(),
                model: "test".into(),
                branch_id: None,
                claim: false,
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
        .id;
    let prepared = prepare_work(
        &mut f.store,
        &f.root.clone(),
        &PrepareWorkRequest {
            work: "W".into(),
            session: Some(session),
            branch: None,
            goals: vec![],
            source_sha: None,
            budget: Some(7000),
        },
    )
    .unwrap();
    let revision = f.revision();
    let checkpoint = f
        .store
        .create_checkpoint(
            f.project,
            revision,
            session,
            CheckpointDraft {
                context_hash: prepared["context"]["work_context"]["context_hash"]
                    .as_str()
                    .unwrap()
                    .into(),
                digest: "Reviewed current context".into(),
                next_action: "Collect the reply".into(),
                open_loops: vec![],
                changed_entities: vec![],
            },
        )
        .unwrap()
        .0;
    let revision = f.revision();
    let wait = f
        .store
        .create_mcp_wait(
            f.project,
            revision,
            "test",
            session,
            checkpoint.id,
            "Choose the audience".into(),
        )
        .unwrap()
        .0;
    let status = f.status("W");
    assert_eq!(status.state, "clear", "{status:?}");
    assert_eq!(status.deviations.len(), 1);
    assert_eq!(status.deviations[0].kind, DeviationKind::ExternalWait);
    assert_eq!(status.deviations[0].reference, wait.id.to_string());
    assert!(status.deviations[0].detail.contains("Choose the audience"));
    // Answering it ends the deviation.
    let revision = f.revision();
    f.store
        .reply_mcp_wait(
            f.project,
            revision,
            "test",
            wait.id,
            "New users".into(),
            false,
        )
        .unwrap();
    assert!(f.status("W").deviations.is_empty());
}

#[test]
fn only_the_work_whose_records_show_the_trouble_is_held() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let other = f.session("agent-v", Some("V"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    f.admit("V", reviewer);
    for n in 1..=3 {
        f.attempt(worker, &format!("op-{n}"), "check", false);
    }
    assert_eq!(state(&f), "required");
    assert!(f.codes("W").contains(&"reassessment_required".to_string()));
    // The same command in another work is another work's record.
    assert_eq!(f.status("V").state, "clear");
    assert!(!f.codes("V").iter().any(|c| c.starts_with("reassessment_")));
    f.attempt(other, "op-v1", "check", false);
    f.register(other, "op-v2", "check").unwrap();
    // Nothing was created or changed by the trigger.
    assert_eq!(f.store.executions(f.project, None).unwrap().len(), 5);
}

#[test]
fn observing_reports_the_state_and_never_holds_anything() {
    let mut f = Fixture::new("legacy_observe");
    let worker = f.session("agent-w", Some("W"));
    for n in 1..=3 {
        f.attempt(worker, &format!("op-{n}"), "check", false);
    }
    let status = f.status("W");
    assert_eq!(status.state, "required");
    // The block is shown, but no entry is refused and no readiness diagnostic appears.
    let block = f
        .store
        .reassessment_block(f.project, "W", None)
        .unwrap()
        .expect("a project with a policy shows the state");
    assert_eq!(block.state, "required");
    assert!(!f.codes("W").iter().any(|c| c.starts_with("reassessment_")));
    f.register(worker, "op-4", "check").unwrap();
}

#[test]
fn work_in_a_project_without_a_policy_gains_no_block_and_no_decision() {
    let mut f = Fixture::new("governed_enforce");
    // Drop the policy from the manifest: the source no longer governs anything.
    let manifest = fs::read_to_string(f.root.join(".awr/project.toml")).unwrap();
    let plain = manifest.replace(
        "[sources.options.admission_policy]\nversion = 1\nmode = \"governed_enforce\"\nscope = \"all\"\n\n",
        "",
    );
    assert_ne!(manifest, plain);
    fs::write(f.root.join(".awr/project.toml"), plain).unwrap();
    f.reindex();
    let worker = f.session("agent-w", Some("W"));
    for n in 1..=3 {
        f.attempt(worker, &format!("op-{n}"), "check", false);
    }
    assert!(
        f.store
            .reassessment_block(f.project, "W", None)
            .unwrap()
            .is_none()
    );
    f.register(worker, "op-4", "check").unwrap();
    let request = f.request_without_facts("ungoverned");
    let error = reassess_work(&mut f.store, &f.root.clone(), &request).unwrap_err();
    assert_eq!(code(error), "admission_not_governed");
}

#[test]
fn facts_and_decisions_belong_to_one_branch() {
    let mut f = Fixture::new("governed_enforce");
    let feature = f.branch("feature");
    let worker = f.session_on(feature, "agent-w", Some("W"));
    let reviewer = f.session_on(feature, "reviewer", None);
    f.admit("W", reviewer);
    for n in 1..=3 {
        f.attempt(worker, &format!("op-{n}"), "check", false);
    }
    let on_branch = f
        .store
        .reassessment_status(f.project, "W", Some(feature))
        .unwrap();
    assert_eq!(on_branch.state, "required", "{on_branch:?}");
    // The baseline branch has recorded none of this.
    let on_main = f.status("W");
    assert_eq!(on_main.state, "clear", "{on_main:?}");
    assert!(on_main.deviations.is_empty());
    // A decision by a session of the branch is read on that branch only.
    let mut request = f.request_without_facts("on-branch");
    request.agent = None;
    request.session = Some(reviewer);
    request.facts_fingerprint = on_branch.current.unwrap().facts_fingerprint;
    let receipt = reassess_work(&mut f.store, &f.root.clone(), &request).unwrap();
    assert_eq!(receipt["event"]["branch_id"], json!(feature), "{receipt}");
    assert_eq!(receipt["reassessment"]["state"], "current");
    let after = f.status("W");
    assert_eq!(after.state, "clear");
    assert!(after.decision_event.is_none(), "{after:?}");
}

#[test]
fn a_state_that_cannot_be_established_is_unknown_and_never_an_error_or_a_pass() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    f.attempt(worker, "op-1", "check", false);
    // Events are append-only, so damaging one needs the guard removed first: this stands for a
    // corrupted store, the one way a recorded execution can fail to read back.
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    db.execute("DROP TRIGGER event_no_update", []).unwrap();
    let damaged = db
        .execute(
            "UPDATE events SET payload_json=json_object('execution', json_object('id', json_extract(payload_json,'$.execution.id'))) WHERE event_type='execution.finished'",
            [],
        )
        .unwrap();
    assert_eq!(damaged, 1);
    drop(db);
    assert_eq!(
        code(f.register(worker, "op-2", "check").unwrap_err()),
        "reassessment_unknown"
    );
    // Readiness reports the reason instead of failing, so every listing keeps answering.
    assert!(f.codes("W").contains(&"reassessment_unknown".to_string()));
}

#[test]
fn work_outside_the_scope_of_the_policy_is_told_nothing_and_held_by_nothing() {
    let mut f = Fixture::with_scope("governed_enforce", "scope = { tags = [\"governed\"] }");
    let worker = f.session("agent-w", Some("W"));
    for n in 1..=3 {
        f.attempt(worker, &format!("op-{n}"), "check", false);
    }
    // The records show the trouble, but this work is not governed: nothing is held ...
    assert_eq!(state(&f), "required");
    assert!(!f.codes("W").iter().any(|c| c.starts_with("reassessment_")));
    f.register(worker, "op-4", "check").unwrap();
    // ... and no decision can be recorded for it.
    let request = f.request("outside", ReassessmentChoice::Continue);
    let error = reassess_work(&mut f.store, &f.root.clone(), &request).unwrap_err();
    assert_eq!(code(error), "admission_not_governed");
}

#[test]
fn a_host_observation_describes_one_contract_and_goes_stale_when_it_moves() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    let fingerprint = assess_management(
        &f.store,
        &f.root,
        &AssessManagementRequest {
            work: "W".into(),
            branch: None,
        },
    )
    .unwrap()["contract_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    let expected_revision = f.revision();
    manage_work(
        &mut f.store,
        &f.root.clone(),
        &ManageWorkRequest {
            work: "W".into(),
            session: worker,
            expected_revision,
            request_key: "unknown-1".into(),
            contract_fingerprint: fingerprint,
            observation: observation(Some(false), now_millis().unwrap()),
        },
    )
    .unwrap();
    assert_eq!(state(&f), "required");
    // The work's scope moves: the host spoke about a contract that no longer exists.
    f.edit_ledger(|ledger| {
        ledger["work_items"][0]["paths"] = json!(["crates/storage/src/more"]);
    });
    let status = f.status("W");
    assert_eq!(status.state, "clear", "{status:?}");
    assert_eq!(status.stale_observations.len(), 1);
}

#[test]
fn a_method_is_a_command_in_a_directory_so_different_ones_never_add_up() {
    let mut f = Fixture::new("governed_enforce");
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    // Three commands failing once each are three methods.
    for (n, command) in ["check-a", "check-b", "check-c"].into_iter().enumerate() {
        f.fail_in(worker, &format!("cmd-{n}"), command, "");
    }
    assert_eq!(state(&f), "clear");
    // So is one command run in three directories.
    for n in 0..3 {
        f.fail_in(worker, &format!("dir-{n}"), "check", &format!("dir-{n}"));
    }
    let status = f.status("W");
    assert_eq!(status.state, "clear", "{status:?}");
    assert_eq!(status.deviations.len(), 6);
    // The same command in the same directory, three times (one is already there), is one.
    for n in 0..2 {
        f.fail_in(worker, &format!("same-{n}"), "check", "dir-0");
    }
    assert_eq!(state(&f), "required");
}

fn spent(rework: u32) -> ManagementObservation {
    ManagementObservation {
        observed_at: now_millis().unwrap(),
        note: "The host counted the cycles it sent the work back".into(),
        completed_rework_cycles: Some(rework),
        ..Default::default()
    }
}

impl Fixture {
    fn observe(&mut self, session: Id, key: &str, observation: ManagementObservation) {
        let fingerprint = assess_management(
            &self.store,
            &self.root,
            &AssessManagementRequest {
                work: "W".into(),
                branch: None,
            },
        )
        .unwrap()["contract_fingerprint"]
            .as_str()
            .unwrap()
            .to_string();
        let expected_revision = self.revision();
        manage_work(
            &mut self.store,
            &self.root.clone(),
            &ManageWorkRequest {
                work: "W".into(),
                session,
                expected_revision,
                request_key: key.into(),
                contract_fingerprint: fingerprint,
                observation,
            },
        )
        .unwrap();
    }

    fn budget(&self) -> BudgetStatus {
        self.status("W").budget.expect("the budget is reported")
    }

    fn budget_request(
        &self,
        key: &str,
        choice: ReassessmentChoice,
        agent: &str,
    ) -> ReassessmentRequest {
        ReassessmentRequest {
            agent: Some(agent.into()),
            answers: ReassessmentSubject::Budget,
            facts_fingerprint: self
                .budget()
                .current
                .expect("a spent budget is bound")
                .facts_fingerprint,
            ..self.request_without_facts(key)
        }
        .with_choice(choice)
    }

    fn decide_budget(&mut self, request: ReassessmentRequest) -> Result<Value> {
        reassess_work(&mut self.store, &self.root.clone(), &request)
    }
}

trait WithChoice {
    fn with_choice(self, choice: ReassessmentChoice) -> Self;
}
impl WithChoice for ReassessmentRequest {
    /// Fills in what a choice needs, so a test says only which choice it makes.
    fn with_choice(mut self, choice: ReassessmentChoice) -> Self {
        self.decision = choice;
        match choice {
            ReassessmentChoice::DeliverWithKnownIssues => {
                self.known_issues = vec!["The retry path is untested".into()]
            }
            ReassessmentChoice::ReduceScope => {
                self.reduced_scope = Some("Only the read path ships".into())
            }
            ReassessmentChoice::Escalate => self.escalated_to = Some("the owner".into()),
            _ => {}
        }
        self
    }
}

/// Small work with a tight rework budget: one cycle and an hour of effort are allowed.
fn small(f: &mut Fixture) {
    f.edit_ledger(|ledger| {
        ledger["work_items"][0]["definition"]["size"] = json!("small");
        ledger["work_items"][0]["definition"]["budget"] = json!({"rework_cycles": 1});
    });
}

#[test]
fn a_spent_budget_holds_the_work_until_it_is_dealt_with_and_a_further_cycle_asks_again() {
    let mut f = Fixture::new("governed_enforce");
    small(&mut f);
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    // Within the budget, nothing is shown and nothing is asked.
    assert_eq!(state(&f), "clear");
    assert!(
        f.store
            .reassessment_block(f.project, "W", None)
            .unwrap()
            .is_none()
    );
    f.observe(worker, "cycles-1", spent(1));
    assert_eq!(f.budget().state, "within");
    // A second cycle under a budget of one: AWR recorded none of its own, so the host's count is used.
    f.observe(worker, "cycles-2", spent(2));
    let budget = f.budget();
    assert_eq!(budget.state, "required");
    assert_eq!(
        (
            budget.usage.rework.used,
            budget.usage.rework.allowed,
            budget.usage.rework.counted_from
        ),
        (2, 1, "host_reported")
    );
    assert_eq!(budget.usage.rework.allowance, "declared");
    assert_eq!(budget.usage.effort_minutes.allowance, "size_default");
    assert_eq!(budget.usage.size, WorkSize::Small);
    assert!(
        f.codes("W")
            .contains(&"reassessment_budget_required".to_string())
    );
    assert_eq!(
        code(f.register(worker, "op-1", "check").unwrap_err()),
        "reassessment_budget_required"
    );
    // The facts are another matter: nothing failed here.
    assert_eq!(state(&f), "clear");
    // Continuing is not a way out of a spent budget, and a request with nothing spent is refused.
    let request = f.budget_request("not-continue", ReassessmentChoice::Continue, "claude-run");
    assert!(matches!(
        f.decide_budget(request),
        Err(Error::InvalidInput(_))
    ));
    // Delivering with known issues deals with it.
    let receipt = f
        .decide_budget(f.budget_request(
            "deliver",
            ReassessmentChoice::DeliverWithKnownIssues,
            "claude-run",
        ))
        .unwrap();
    assert_eq!(
        receipt["reassessment"]["budget"]["state"], "current",
        "{receipt}"
    );
    assert_eq!(
        receipt["event"]["payload"]["input"]["known_issues"][0],
        "The retry path is untested"
    );
    assert!(!f.codes("W").iter().any(|c| c.starts_with("reassessment_")));
    f.register(worker, "op-1", "check").unwrap();
    // A third cycle spends it again, and the old decision names what changed.
    f.observe(worker, "cycles-3", spent(3));
    let budget = f.budget();
    assert_eq!(budget.state, "stale");
    assert!(
        f.status("W").diagnostics[0]
            .detail
            .contains("fact_added:budget:rework:3>1")
    );
    assert_eq!(
        code(f.register(worker, "op-2", "check").unwrap_err()),
        "reassessment_budget_stale"
    );
    f.decide_budget(f.budget_request("reduce", ReassessmentChoice::ReduceScope, "claude-run"))
        .unwrap();
    assert_eq!(f.budget().state, "current");
    // Replanning the budget is a decision too, and a larger budget needs none.
    f.edit_ledger(|ledger| {
        ledger["work_items"][0]["definition"]["budget"] = json!({"rework_cycles": 5});
    });
    f.observe(worker, "cycles-3-again", spent(3));
    assert_eq!(f.budget().state, "within");
}

#[test]
fn a_budget_decision_answers_a_spent_budget_and_never_facts() {
    let mut f = Fixture::new("governed_enforce");
    small(&mut f);
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    // Nothing is spent yet.
    let mut early = f.request_without_facts("early");
    early.answers = ReassessmentSubject::Budget;
    early = early.with_choice(ReassessmentChoice::ReduceScope);
    let error = reassess_work(&mut f.store, &f.root.clone(), &early).unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    // Both a spent budget and repeated failures: each is answered by its own decision.
    for n in 1..=3 {
        f.attempt(worker, &format!("op-{n}"), "check", false);
    }
    f.observe(worker, "cycles", spent(2));
    assert_eq!(state(&f), "required");
    assert_eq!(f.budget().state, "required");
    // Answering the facts leaves the budget open ...
    f.decide("facts", ReassessmentChoice::AlterMethod).unwrap();
    assert_eq!(state(&f), "current");
    assert_eq!(f.budget().state, "required");
    assert!(
        f.codes("W")
            .contains(&"reassessment_budget_required".to_string())
    );
    // ... and answering the budget leaves the facts as they were.
    f.decide_budget(f.budget_request("budget", ReassessmentChoice::Replan, "claude-run"))
        .unwrap();
    assert_eq!(f.budget().state, "current");
    assert_eq!(state(&f), "current");
    assert!(!f.codes("W").iter().any(|c| c.starts_with("reassessment_")));
    // A wrong fingerprint is a conflict, and a root cause belongs to failed attempts only.
    let mut wrong = f.budget_request("wrong", ReassessmentChoice::Replan, "claude-run");
    wrong.facts_fingerprint = format!("sha256:{}", "a".repeat(64));
    assert!(matches!(
        f.decide_budget(wrong),
        Err(Error::SourceConflict(_))
    ));
    let mut grouped = f.budget_request("grouped", ReassessmentChoice::Replan, "claude-run");
    // A real record of the work: the grouping is refused for what it is, not for what it names.
    grouped.root_cause = Some(RootCause {
        label: "The fixture is read-only".into(),
        references: vec![f.status("W").deviations[0].reference.clone()],
    });
    assert!(matches!(
        f.decide_budget(grouped),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn an_escalated_budget_stays_held_until_someone_else_decides() {
    let mut f = Fixture::with_policy(
        "governed_enforce",
        "scope = \"all\"",
        "supervisors = [\"lead\"]\n",
    );
    small(&mut f);
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    f.observe(worker, "cycles", spent(2));
    f.decide_budget(f.budget_request("escalate", ReassessmentChoice::Escalate, "claude-run"))
        .unwrap();
    let budget = f.budget();
    assert_eq!(budget.state, "escalated");
    assert!(
        f.codes("W")
            .contains(&"reassessment_budget_escalated".to_string())
    );
    assert_eq!(
        code(f.register(worker, "op-1", "check").unwrap_err()),
        "reassessment_budget_escalated"
    );
    // The one who escalated cannot answer, and with supervisors only a supervisor can.
    let own = f.budget_request(
        "own",
        ReassessmentChoice::DeliverWithKnownIssues,
        "claude-run",
    );
    assert_eq!(
        code(f.decide_budget(own).unwrap_err()),
        "reassessment_budget_escalated"
    );
    let other = f.budget_request(
        "other",
        ReassessmentChoice::DeliverWithKnownIssues,
        "intern",
    );
    assert_eq!(
        code(f.decide_budget(other).unwrap_err()),
        "reassessment_not_authorized"
    );
    f.decide_budget(f.budget_request("lead", ReassessmentChoice::DeliverWithKnownIssues, "lead"))
        .unwrap();
    assert_eq!(f.budget().state, "current");
    f.register(worker, "op-1", "check").unwrap();
}

#[test]
fn effort_is_counted_from_the_claims_on_the_work_before_the_host_is_asked() {
    let mut f = Fixture::new("governed_enforce");
    small(&mut f);
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    let revision = f.revision();
    let worker = f
        .store
        .start_session(
            f.project,
            revision,
            SessionDraft {
                work_item_key: Some("W".into()),
                agent_id: "agent-w".into(),
                provider: "test".into(),
                model: "m".into(),
                branch_id: None,
                claim: true,
                claim_ttl_ms: Some(86_400_000),
            },
        )
        .unwrap()
        .0
        .session
        .id;
    let line = f.budget().usage.effort_minutes;
    assert_eq!(
        (line.counted_from, line.allowed, line.exhausted()),
        ("recorded", 60, false)
    );
    // The claim has been held for two hours.
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    db.execute("UPDATE claims SET acquired_at=acquired_at-7200000", [])
        .unwrap();
    drop(db);
    let line = f.budget().usage.effort_minutes;
    assert!(line.used >= 119 && line.exhausted(), "{line:?}");
    assert_eq!(line.counted_from, "recorded");
    // Even a host that says the work took ten minutes does not outweigh what AWR recorded.
    f.observe(
        worker,
        "ten-minutes",
        ManagementObservation {
            observed_at: now_millis().unwrap(),
            note: "The host counted ten active minutes".into(),
            active_elapsed_ms: Some(10 * 60_000),
            ..Default::default()
        },
    );
    let budget = f.budget();
    assert_eq!(budget.state, "required");
    assert!(budget.usage.effort_minutes.used >= 119);
    assert!(
        f.codes("W")
            .contains(&"reassessment_budget_required".to_string())
    );
    // Releasing the claim stops the clock; the time already held stays spent.
    f.decide_budget(f.budget_request(
        "deliver",
        ReassessmentChoice::DeliverWithKnownIssues,
        "claude-run",
    ))
    .unwrap();
    assert_eq!(f.budget().state, "current");
}

#[test]
fn a_released_claim_stops_the_clock_and_a_host_stands_in_only_where_awr_recorded_nothing() {
    let mut f = Fixture::new("governed_enforce");
    small(&mut f);
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    let worker = f.session("agent-w", Some("W"));
    // Nothing recorded by AWR: the host's minutes are used, and the block says so.
    f.observe(
        worker,
        "two-hours",
        ManagementObservation {
            observed_at: now_millis().unwrap(),
            note: "The host counted two active hours".into(),
            active_elapsed_ms: Some(120 * 60_000),
            ..Default::default()
        },
    );
    let line = f.budget().usage.effort_minutes;
    assert_eq!(
        (line.used, line.counted_from, line.exhausted()),
        (120, "host_reported", true)
    );
    // A claim that was held for half an hour and released counts exactly that, however long ago it was.
    let db = rusqlite::Connection::open(f.root.join(".awr/state.db")).unwrap();
    db.execute(
        "INSERT INTO claims(id,project_id,work_item_id,session_id,agent_id,branch_id,status,acquired_at,expires_at,released_at,revision) \
         SELECT ?1, project_id, id, ?2, 'agent-c', NULL, 'released', 1000000, 99000000000, 2800000, 1 FROM work_items WHERE external_key='W'",
        rusqlite::params![Id::new().to_string(), worker.to_string()],
    )
    .unwrap();
    drop(db);
    let line = f.budget().usage.effort_minutes;
    assert_eq!((line.used, line.counted_from), (30, "recorded"), "{line:?}");
}
