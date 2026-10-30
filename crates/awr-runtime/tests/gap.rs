//! Gaps against a real indexed project: the layer a gap belongs to decides which judgments it puts
//! in question, an experiment is closed only by the run, a choice only by someone authorized, and
//! nothing about a gap opens a task or holds another work.
use awr_core::*;
use awr_runtime::{admit_work, gap_work};
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

fn write_ledger(root: &Path) {
    let work = |key: &str| {
        json!({"id": key, "title": format!("Save safely {key}"), "status": "planned", "goal": "G",
            "acceptance": [ACC], "paths": ["crates/storage/src"], "next_action": "Do the work",
            "definition": definition(&pin(root))})
    };
    let ledger = json!({
        "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
        "work_items": [work("W"), work("V")]
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
    fn new() -> Self {
        Self::with_policy("scope = \"all\"", "")
    }

    /// `scope` is the TOML line that says which work the policy governs; `extra` is added to the
    /// policy table, such as the supervisors.
    fn with_policy(scope: &str, extra: &str) -> Self {
        let root = std::env::temp_dir().join(format!("awr-gap-runtime-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::create_dir_all(root.join("decisions")).unwrap();
        fs::write(
            root.join(".awr/project.toml"),
            format!("[project]\nname = \"Gap runtime fixture\"\nexternal_key = \"gap-runtime-fixture\"\n\n[[sources]]\ndomain = \"ledger\"\nrole = \"primary\"\npath = \"work.yaml\"\nadapter = \"yaml-ledger-v1\"\n\n[sources.options.admission_policy]\nversion = 1\nmode = \"governed_enforce\"\n{scope}\n{extra}\n[[sources]]\ndomain = \"decisions\"\nrole = \"supporting\"\npath = \"decisions\"\nadapter = \"markdown-directory-v1\"\n"),
        )
        .unwrap();
        fs::write(
            root.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        write_ledger(&root);
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

    fn session(&mut self, agent: &str, work: Option<&str>) -> Id {
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
                    branch_id: None,
                    claim: false,
                    claim_ttl_ms: None,
                },
            )
            .unwrap()
            .0
            .session
            .id
    }

    fn admit(&mut self, work: &str, reviewer: Id, key: &str) {
        let request = AdmissionRequest {
            work: work.into(),
            session: Some(reviewer),
            agent: None,
            expected_revision: self.revision(),
            request_key: key.into(),
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

    fn register(&mut self, session: Id, key: &str) -> Result<Execution> {
        let revision = self.revision();
        let intent = ExecutionIntent {
            operation_key: key.into(),
            purpose: "Run the storage checks".into(),
            executor: ExecutorKind::ManagedLocal,
            command: vec!["check".into()],
            cwd: self.root.to_string_lossy().into(),
            external_reference: None,
        };
        self.store
            .register_execution(self.project, revision, session, intent)
            .map(|(execution, _)| execution)
    }

    /// Registers an execution, starts it and ends it.
    fn run(&mut self, session: Id, key: &str, success: bool) -> Id {
        let registered = self.register(session, key).unwrap();
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
        let revision = self.revision();
        self.store
            .finish_execution(
                self.project,
                revision,
                ExecutionResult {
                    execution_id: registered.id,
                    nonce,
                    finished_at: now_millis().unwrap(),
                    success,
                    exit_code: Some(if success { 0 } else { 1 }),
                    signal: None,
                    error: None,
                },
            )
            .unwrap();
        registered.id
    }

    fn request(&self, work: &str, key: &str, gap: &str, action: GapAction) -> GapRequest {
        GapRequest {
            work: work.into(),
            session: None,
            agent: Some("claude-run".into()),
            expected_revision: self.revision(),
            request_key: key.into(),
            gap_key: gap.into(),
            action,
            route: None,
            resolution: None,
            subject: None,
            experiment: None,
            authority: vec![],
            waiting_for: None,
            outcome: None,
            sources: vec![],
            observed: None,
            reason: None,
        }
    }

    fn open(
        &mut self,
        work: &str,
        gap: &str,
        route: GapRoute,
        resolution: GapResolution,
        edit: impl FnOnce(&mut GapRequest),
    ) -> Result<Value> {
        let mut request = self.request(work, &format!("open-{work}-{gap}"), gap, GapAction::Open);
        request.route = Some(route);
        request.resolution = Some(resolution);
        request.subject = Some("The second criterion does not say what happens offline".into());
        edit(&mut request);
        gap_work(&mut self.store, &self.root.clone(), &request)
    }

    fn close(
        &mut self,
        work: &str,
        gap: &str,
        action: GapAction,
        edit: impl FnOnce(&mut GapRequest),
    ) -> Result<Value> {
        let mut request =
            self.request(work, &format!("close-{work}-{gap}-{action:?}"), gap, action);
        match action {
            GapAction::Withdraw => request.reason = Some("No longer relevant".into()),
            _ => request.outcome = Some("Concluded after looking".into()),
        }
        edit(&mut request);
        gap_work(&mut self.store, &self.root.clone(), &request)
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

    fn gaps(&self, work: &str) -> GapStatus {
        self.store.gap_status(self.project, work, None).unwrap()
    }
}

#[test]
fn an_open_gap_puts_in_question_only_the_judgment_that_reads_its_layer_on_that_work() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let other = f.session("agent-v", Some("V"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer, "admit-w");
    f.admit("V", reviewer, "admit-v");
    f.register(worker, "op-1").unwrap();
    // A gap in the design touches no judgment AWR keeps.
    f.open(
        "W",
        "design-1",
        GapRoute::Design,
        GapResolution::FactLookup,
        |_| {},
    )
    .unwrap();
    assert!(f.gaps("W").suspensions.is_empty());
    f.register(worker, "op-2").unwrap();
    // A gap in the acceptance puts the admission of this work in question ...
    let receipt = f
        .open(
            "W",
            "acceptance-1",
            GapRoute::Acceptance,
            GapResolution::FactLookup,
            |_| {},
        )
        .unwrap();
    assert_eq!(receipt["already_recorded"], false);
    assert_eq!(receipt["execution_started"], false);
    assert_eq!(receipt["gaps"]["open"], json!(["design-1", "acceptance-1"]));
    assert!(f.codes("W").contains(&"admission_suspended".to_string()));
    assert_eq!(
        code(f.register(worker, "op-3").unwrap_err()),
        "admission_suspended"
    );
    // ... and nothing else: not the other work, which keeps running.
    assert!(!f.codes("V").iter().any(|c| c.ends_with("_suspended")));
    f.register(other, "op-v1").unwrap();
    // Closing the gap lifts it. The admission was never decided again, because nothing changed.
    f.close("W", "acceptance-1", GapAction::Resolve, |r| {
        r.sources = vec!["docs/offline.md".into()]
    })
    .unwrap();
    assert!(!f.codes("W").iter().any(|c| c.ends_with("_suspended")));
    f.register(worker, "op-3").unwrap();
    // No task was opened by any of it.
    assert_eq!(f.store.executions(f.project, None).unwrap().len(), 4);
}

#[test]
fn an_experiment_is_closed_by_the_run_and_never_by_repairing_the_plan() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer, "admit-1");
    f.open(
        "W",
        "offline",
        GapRoute::Acceptance,
        GapResolution::FeasibilityExperiment,
        |r| r.experiment = Some("probe-offline-save".into()),
    )
    .unwrap();
    let refused = f
        .close("W", "offline", GapAction::Resolve, |_| {})
        .unwrap_err();
    assert_eq!(code(refused), "gap_experiment_not_run");
    // Repairing the plan around the question settles nothing about it: the contract moves, the
    // old admission lapses, a new one is decided, and the gap is still open.
    f.edit_ledger(|ledger| {
        ledger["work_items"][0]["paths"] = json!(["crates/storage/src/offline"]);
    });
    f.admit("W", reviewer, "admit-2");
    let refused = f
        .close("W", "offline", GapAction::Resolve, |_| {})
        .unwrap_err();
    assert_eq!(code(refused), "gap_experiment_not_run");
    assert_eq!(f.gaps("W").open, ["offline"]);
    // Any other run is held by the open gap, but the experiment itself is how the gap closes.
    assert_eq!(
        code(f.register(worker, "something-else").unwrap_err()),
        "admission_suspended"
    );
    f.run(worker, "probe-offline-save", false);
    // A failed run is a run, and it stays on record.
    let receipt = f
        .close("W", "offline", GapAction::Resolve, |r| {
            r.outcome = Some("Offline saving is not feasible with the current storage".into())
        })
        .unwrap();
    assert_eq!(receipt["gaps"]["gaps"][0]["state"], "resolved");
    assert_eq!(receipt["gaps"]["open"], json!([]));
    assert_eq!(
        f.store
            .executions(
                f.project,
                Some(f.store.work_item(f.project, "W").unwrap().item.meta.id)
            )
            .unwrap()
            .len(),
        1
    );
    f.register(worker, "op-after").unwrap();
}

#[test]
fn only_a_run_of_this_work_after_the_gap_was_opened_counts_as_its_experiment() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let other = f.session("agent-v", Some("V"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer, "admit-w");
    f.admit("V", reviewer, "admit-v");
    // A run that happened before the question was asked answers nothing.
    f.run(worker, "early-probe", true);
    f.open(
        "W",
        "early",
        GapRoute::Plan,
        GapResolution::FeasibilityExperiment,
        |r| r.experiment = Some("early-probe".into()),
    )
    .unwrap();
    assert_eq!(
        code(
            f.close("W", "early", GapAction::Resolve, |_| {})
                .unwrap_err()
        ),
        "gap_experiment_not_run"
    );
    // Nor does a run of another work, even under the key the gap names.
    f.open(
        "W",
        "foreign",
        GapRoute::Plan,
        GapResolution::FeasibilityExperiment,
        |r| r.experiment = Some("probe-in-v".into()),
    )
    .unwrap();
    f.run(other, "probe-in-v", true);
    assert_eq!(
        code(
            f.close("W", "foreign", GapAction::Resolve, |_| {})
                .unwrap_err()
        ),
        "gap_experiment_not_run"
    );
    // An execution that has not ended is not a result yet.
    f.open(
        "W",
        "running",
        GapRoute::Plan,
        GapResolution::FeasibilityExperiment,
        |r| r.experiment = Some("probe-running".into()),
    )
    .unwrap();
    let registered = f.register(worker, "probe-running").unwrap();
    assert_eq!(
        code(
            f.close("W", "running", GapAction::Resolve, |_| {})
                .unwrap_err()
        ),
        "gap_experiment_not_run"
    );
    let _ = registered;
}

#[test]
fn a_choice_is_made_by_those_authorized_and_the_other_kinds_by_what_they_need() {
    let mut f = Fixture::with_policy("scope = \"all\"", "supervisors = [\"lead\"]\n");
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer, "admit-w");
    // Named authority, else the policy's supervisors.
    f.open(
        "W",
        "named",
        GapRoute::Intent,
        GapResolution::AuthorizedChoice,
        |r| r.authority = vec!["owner".into()],
    )
    .unwrap();
    f.open(
        "W",
        "policy",
        GapRoute::Design,
        GapResolution::AuthorizedChoice,
        |_| {},
    )
    .unwrap();
    let as_agent = |agent: &str| {
        let agent = agent.to_string();
        move |r: &mut GapRequest| r.agent = Some(agent.clone())
    };
    assert_eq!(
        code(
            f.close("W", "named", GapAction::Resolve, as_agent("lead"))
                .unwrap_err()
        ),
        "gap_not_authorized"
    );
    assert_eq!(
        code(
            f.close("W", "policy", GapAction::Resolve, as_agent("intern"))
                .unwrap_err()
        ),
        "gap_not_authorized"
    );
    f.close("W", "named", GapAction::Resolve, as_agent("owner"))
        .unwrap();
    f.close("W", "policy", GapAction::Resolve, as_agent("lead"))
        .unwrap();
    // A fact lookup names its sources, and an environment wait what was observed.
    f.open(
        "W",
        "fact",
        GapRoute::Acceptance,
        GapResolution::FactLookup,
        |_| {},
    )
    .unwrap();
    assert_eq!(
        code(
            f.close("W", "fact", GapAction::Resolve, |_| {})
                .unwrap_err()
        ),
        "gap_sources_missing"
    );
    f.close("W", "fact", GapAction::Resolve, |r| {
        r.sources = vec!["docs/offline.md".into()]
    })
    .unwrap();
    f.open(
        "W",
        "env",
        GapRoute::Plan,
        GapResolution::EnvironmentWait,
        |r| r.waiting_for = Some("The staging database is restored".into()),
    )
    .unwrap();
    assert_eq!(
        code(f.close("W", "env", GapAction::Resolve, |_| {}).unwrap_err()),
        "gap_observation_missing"
    );
    f.close("W", "env", GapAction::Resolve, |r| {
        r.observed = Some("The database answers again".into())
    })
    .unwrap();
    assert!(f.gaps("W").open.is_empty());
}

#[test]
fn without_named_authority_or_supervisors_nobody_is_authorized_to_choose() {
    let mut f = Fixture::new();
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer, "admit-w");
    let refused = f
        .open(
            "W",
            "choice",
            GapRoute::Intent,
            GapResolution::AuthorizedChoice,
            |_| {},
        )
        .unwrap_err();
    assert_eq!(code(refused), "gap_authority_unknown");
    assert!(f.gaps("W").gaps.is_empty());
}

#[test]
fn history_is_kept_and_a_gap_key_names_one_gap_for_good() {
    let mut f = Fixture::new();
    f.open(
        "W",
        "g1",
        GapRoute::Acceptance,
        GapResolution::FactLookup,
        |_| {},
    )
    .unwrap();
    f.open(
        "W",
        "g2",
        GapRoute::Plan,
        GapResolution::EnvironmentWait,
        |r| r.waiting_for = Some("The vendor answers".into()),
    )
    .unwrap();
    f.close("W", "g1", GapAction::Resolve, |r| {
        r.sources = vec!["docs/offline.md".into()]
    })
    .unwrap();
    f.close("W", "g2", GapAction::Withdraw, |_| {}).unwrap();
    let status = f.gaps("W");
    assert_eq!(
        status
            .gaps
            .iter()
            .map(|g| (g.key.as_str(), g.state))
            .collect::<Vec<_>>(),
        [("g1", "resolved"), ("g2", "withdrawn")]
    );
    assert_eq!(status.gaps[1].reason.as_deref(), Some("No longer relevant"));
    let history = f
        .store
        .query_events(
            f.project,
            &EventQuery {
                event_type: Some(GAP_EVENT.into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        history.events.len(),
        4,
        "opening and closing are all on record"
    );
    // A closed gap cannot be closed again, a used key cannot be opened again.
    let error = f.close("W", "g1", GapAction::Withdraw, |_| {}).unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    let error = f
        .open("W", "g1", GapRoute::Plan, GapResolution::FactLookup, |r| {
            r.request_key = "again".into()
        })
        .unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    let error = f
        .close("W", "ghost", GapAction::Resolve, |_| {})
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
}

#[test]
fn requests_are_attributed_current_and_replay_without_effect() {
    let mut f = Fixture::new();
    let first = f
        .open(
            "W",
            "g1",
            GapRoute::Acceptance,
            GapResolution::FactLookup,
            |_| {},
        )
        .unwrap();
    // The same request again returns the original receipt; another request under the key is refused.
    let mut again = f.request("W", "open-W-g1", "g1", GapAction::Open);
    again.route = Some(GapRoute::Acceptance);
    again.resolution = Some(GapResolution::FactLookup);
    again.subject = Some("The second criterion does not say what happens offline".into());
    let replay = gap_work(&mut f.store, &f.root.clone(), &again).unwrap();
    assert_eq!(replay["already_recorded"], true);
    assert_eq!(replay["event"]["id"], first["event"]["id"]);
    again.route = Some(GapRoute::Plan);
    let error = gap_work(&mut f.store, &f.root.clone(), &again).unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    // A stale write is refused.
    let mut late = f.request("W", "late", "g9", GapAction::Open);
    late.route = Some(GapRoute::Design);
    late.resolution = Some(GapResolution::FactLookup);
    late.subject = Some("A question".into());
    late.expected_revision -= 1;
    let error = gap_work(&mut f.store, &f.root.clone(), &late).unwrap_err();
    assert!(matches!(error, Error::RevisionConflict { .. }), "{error:?}");
    // A session that has ended decides nothing; an active one decides under its own agent.
    let gone = f.session("agent-gone", None);
    let revision = f.revision();
    f.store
        .end_session(f.project, revision, gone, SessionOutcome::Ended)
        .unwrap();
    let mut by_session = f.request("W", "by-session", "g9", GapAction::Open);
    by_session.agent = None;
    by_session.session = Some(gone);
    by_session.route = Some(GapRoute::Design);
    by_session.resolution = Some(GapResolution::FactLookup);
    by_session.subject = Some("A question".into());
    let error = gap_work(&mut f.store, &f.root.clone(), &by_session).unwrap_err();
    assert!(matches!(error, Error::InvalidTransition(_)), "{error:?}");
    let active = f.session("agent-active", None);
    by_session.session = Some(active);
    by_session.expected_revision = f.revision();
    let receipt = gap_work(&mut f.store, &f.root.clone(), &by_session).unwrap();
    assert_eq!(receipt["event"]["payload"]["decider_agent"], "agent-active");
    assert_eq!(receipt["event"]["session_id"], json!(active));
}

#[test]
fn a_gap_is_recorded_only_for_governed_work() {
    let mut f = Fixture::new();
    let manifest = fs::read_to_string(f.root.join(".awr/project.toml")).unwrap();
    let plain = manifest.replace(
        "[sources.options.admission_policy]\nversion = 1\nmode = \"governed_enforce\"\nscope = \"all\"\n\n",
        "",
    );
    assert_ne!(manifest, plain);
    fs::write(f.root.join(".awr/project.toml"), plain).unwrap();
    f.reindex();
    let error = f
        .open(
            "W",
            "g1",
            GapRoute::Design,
            GapResolution::FactLookup,
            |_| {},
        )
        .unwrap_err();
    assert_eq!(code(error), "admission_not_governed");
    assert!(f.store.gap_block(f.project, "W", None).unwrap().is_none());
}

#[test]
fn a_withdrawn_gap_no_longer_exempts_its_experiment_from_a_gap_that_is_open() {
    let mut f = Fixture::new();
    let worker = f.session("agent-w", Some("W"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer, "admit-w");
    f.open(
        "W",
        "dropped",
        GapRoute::Plan,
        GapResolution::FeasibilityExperiment,
        |r| r.experiment = Some("probe-dropped".into()),
    )
    .unwrap();
    f.close("W", "dropped", GapAction::Withdraw, |_| {})
        .unwrap();
    f.open(
        "W",
        "open-one",
        GapRoute::Acceptance,
        GapResolution::FactLookup,
        |_| {},
    )
    .unwrap();
    // The probe of a withdrawn gap is the experiment of nothing, so the open gap holds it.
    assert_eq!(
        code(f.register(worker, "probe-dropped").unwrap_err()),
        "admission_suspended"
    );
}

#[test]
fn work_outside_the_scope_of_the_policy_records_no_gaps() {
    let mut f = Fixture::with_policy("scope = { tags = [\"governed\"] }", "");
    let error = f
        .open(
            "W",
            "g1",
            GapRoute::Design,
            GapResolution::FactLookup,
            |_| {},
        )
        .unwrap_err();
    assert_eq!(code(error), "admission_not_governed");
}

#[test]
fn gaps_recorded_under_a_policy_are_not_shown_once_the_policy_is_gone() {
    let mut f = Fixture::new();
    f.open(
        "W",
        "g1",
        GapRoute::Design,
        GapResolution::FactLookup,
        |_| {},
    )
    .unwrap();
    assert!(f.store.gap_block(f.project, "W", None).unwrap().is_some());
    let manifest = fs::read_to_string(f.root.join(".awr/project.toml")).unwrap();
    let plain = manifest.replace(
        "[sources.options.admission_policy]\nversion = 1\nmode = \"governed_enforce\"\nscope = \"all\"\n\n",
        "",
    );
    assert_ne!(manifest, plain);
    fs::write(f.root.join(".awr/project.toml"), plain).unwrap();
    f.reindex();
    // The record stays in the journal, but a project that governs nothing shows no block.
    assert!(f.store.gap_block(f.project, "W", None).unwrap().is_none());
    assert_eq!(f.gaps("W").gaps.len(), 1);
}
