use awr_core::*;
use awr_runtime::*;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
    session: Id,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-compaction-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join(".awr/project.toml"), "[project]\nname='Continuity fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        fs::write(root.join("work.yaml"), "goals:\n- id: G\n  title: Deliver a useful guide\n  status: active\nwork_items:\n- id: W\n  title: Write the guide\n  status: in_progress\n  goal: G\n  acceptance: [Reviewed guide]\n  next_action: Review the draft\n").unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let indexed =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(indexed.ok);
        let session = store
            .start_session(
                indexed.project_id,
                indexed.project_revision,
                SessionDraft {
                    work_item_key: Some("W".into()),
                    agent_id: "writer".into(),
                    provider: "fixture".into(),
                    model: "test".into(),
                    branch_id: None,
                    claim: true,
                    claim_ttl_ms: None,
                },
            )
            .unwrap()
            .0
            .session
            .id;
        Self {
            root,
            store,
            project: indexed.project_id,
            session,
        }
    }
    fn revision(&self) -> Revision {
        self.store.project(self.project).unwrap().project_revision
    }
    fn observe(&mut self, observation: CompactionObservation) -> Value {
        let request = ObserveCompactionRequest {
            session: self.session,
            expected_revision: self.revision(),
            observation,
            policy: CompactionPolicy::default(),
        };
        observe_compaction(&mut self.store, &self.root, &request).unwrap()
    }
    fn inspect(&self, details: bool) -> Value {
        inspect_compaction(
            &self.store,
            &self.root,
            &InspectCompactionRequest {
                session: self.session,
                include_observation: details,
            },
        )
        .unwrap()
    }
    fn prepare(&mut self, budget: usize) -> Value {
        prepare_work(
            &mut self.store,
            &self.root,
            &PrepareWorkRequest {
                work: "W".into(),
                session: Some(self.session),
                branch: None,
                goals: vec![],
                source_sha: None,
                budget: Some(budget),
            },
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn observation(sequence: u64, after: Option<u64>) -> CompactionObservation {
    CompactionObservation {
        compaction_id: format!("compact-{sequence}"),
        sequence,
        observed_at: now_millis().unwrap(),
        trigger: CompactionTrigger::Automatic,
        model: "fixture-model".into(),
        source: "fixture.post_compact.full_request".into(),
        measurement_scope: ContextMeasurementScope::FullRequest,
        measurement_basis: ContextMeasurementBasis::HostReported,
        before_tokens: Some(230_000),
        after_tokens: after,
        context_window_tokens: Some(256_000),
        duration_ms: None,
        usage: None,
    }
}

#[test]
fn threshold_and_real_metrics_are_kept_separate_from_cost_claims() {
    let mut f = Fixture::new();
    assert_eq!(f.inspect(false)["state"], "measurement_missing");
    let below = f.observe(observation(1, Some(127_999)));
    assert_eq!(below["state"], "continue");
    let at = f.observe(observation(2, Some(128_000)));
    assert_eq!(at["state"], "handoff_candidate");
    assert_eq!(at["post_compaction_percent"], 50.0);
    assert_eq!(at["native_compaction"], "unchanged");
    assert_eq!(at["session_switch_performed"], false);
    let detail = f.inspect(true);
    assert!(detail["observation"]["usage"].is_null());
    assert!(detail["observation"]["duration_ms"].is_null());
    let mut real = observation(3, Some(140_000));
    real.usage = Some(CompactionUsage {
        input_tokens: Some(230_000),
        output_tokens: Some(5_000),
        cached_input_tokens: Some(200_000),
        cost_usd: Some(0.0123),
    });
    real.duration_ms = Some(920);
    f.observe(real.clone());
    assert_eq!(f.inspect(true)["observation"], json!(real));
    assert_eq!(f.store.sessions(f.project, true, 20).unwrap().len(), 1);
    assert_eq!(
        f.store.work_item(f.project, "W").unwrap().item.status,
        WorkStatus::InProgress
    );
}

#[test]
fn replay_conflict_ordering_and_restart_preserve_the_latest_observation() {
    let mut f = Fixture::new();
    let first = observation(1, Some(100_000));
    let request = ObserveCompactionRequest {
        session: f.session,
        expected_revision: f.revision(),
        observation: first.clone(),
        policy: CompactionPolicy::default(),
    };
    let a = observe_compaction(&mut f.store, &f.root, &request).unwrap();
    let rev = f.revision();
    let b = observe_compaction(&mut f.store, &f.root, &request).unwrap();
    assert_eq!(a["recorded_event_id"], b["recorded_event_id"]);
    assert_eq!(b["already_recorded"], true);
    assert_eq!(rev, f.revision());
    let mut conflict = request.clone();
    conflict.observation.after_tokens = Some(100_001);
    assert!(matches!(
        observe_compaction(&mut f.store, &f.root, &conflict),
        Err(Error::SourceConflict(_))
    ));
    conflict = request.clone();
    conflict.policy.post_compaction_threshold_percent = 60;
    assert!(matches!(
        observe_compaction(&mut f.store, &f.root, &conflict),
        Err(Error::SourceConflict(_))
    ));
    let second = f.observe(observation(3, Some(160_000)));
    let mut old = observation(2, Some(10_000));
    for backwards in [false, true] {
        if backwards {
            old.sequence = 4;
            old.observed_at = 1;
        }
        let rev = f.revision();
        assert!(matches!(
            f.store.record_compaction(
                f.project,
                rev,
                f.session,
                &old,
                &CompactionPolicy::default()
            ),
            Err(Error::SourceConflict(_))
        ));
        assert_eq!(rev, f.revision());
    }
    let replay = observe_compaction(&mut f.store, &f.root, &request).unwrap();
    assert_eq!(
        replay["observation_event_id"],
        second["observation_event_id"]
    );
    let reopened = Store::open_existing(&f.root.join(".awr/state.db")).unwrap();
    let read = inspect_compaction(
        &reopened,
        &f.root,
        &InspectCompactionRequest {
            session: f.session,
            include_observation: true,
        },
    )
    .unwrap();
    assert_eq!(read["observation"]["sequence"], 3);
    let rev = f.revision();
    let mut other = Store::open_existing(&f.root.join(".awr/state.db")).unwrap();
    let next = observation(4, Some(160_000));
    f.store
        .record_compaction(
            f.project,
            rev,
            f.session,
            &next,
            &CompactionPolicy::default(),
        )
        .unwrap();
    assert!(matches!(
        other.record_compaction(
            f.project,
            rev,
            f.session,
            &observation(5, Some(100)),
            &CompactionPolicy::default()
        ),
        Err(Error::RevisionConflict { .. })
    ));
    assert!(
        other
            .record_compaction(
                f.project,
                rev,
                f.session,
                &next,
                &CompactionPolicy::default()
            )
            .unwrap()
            .1
    );
}

#[test]
fn missing_estimated_partial_and_changed_capacity_never_produce_false_trends() {
    let mut f = Fixture::new();
    for (i, scope, basis) in [
        (
            1,
            ContextMeasurementScope::HistoryOnly,
            ContextMeasurementBasis::HostReported,
        ),
        (
            2,
            ContextMeasurementScope::FullRequest,
            ContextMeasurementBasis::Estimated,
        ),
        (
            3,
            ContextMeasurementScope::Unknown,
            ContextMeasurementBasis::Unknown,
        ),
    ] {
        let mut o = observation(i, Some(250_000));
        o.measurement_scope = scope;
        o.measurement_basis = basis;
        let v = f.observe(o);
        assert_eq!(v["state"], "measurement_missing");
        assert!(v["post_compaction_percent"].is_null());
    }
    let unknown = f.observe(observation(4, None));
    assert!(unknown["post_compaction_percent"].is_null());
    f.observe(observation(5, Some(140_000)));
    let mut changed = observation(6, Some(140_000));
    changed.context_window_tokens = Some(512_000);
    let result = f.observe(changed);
    assert_eq!(result["state"], "continue");
    assert!(result["previous_delta_percentage_points"].is_null());
    let mut manual = observation(7, Some(250_000));
    manual.trigger = CompactionTrigger::Manual;
    assert_eq!(f.observe(manual)["state"], "manual_observation");
    let mut o = observation(8, Some(1));
    o.context_window_tokens = Some(0);
    assert!(o.validate(now_millis().unwrap()).is_err());
    o.context_window_tokens = None;
    o.observed_at = i64::MAX;
    assert!(o.validate(now_millis().unwrap()).is_err());
}

#[test]
fn deferral_is_observation_scoped_and_projects_sessions_and_domain_events_are_isolated() {
    let mut f = Fixture::new();
    let other = Fixture::new();
    let high = f.observe(observation(1, Some(180_000)));
    let request = DeferCompactionRequest {
        session: f.session,
        expected_revision: f.revision(),
        observation_event_id: high["observation_event_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    };
    let deferred = defer_compaction(&mut f.store, &f.root, &request).unwrap();
    assert_eq!(deferred["state"], "deferred");
    let rev = f.revision();
    assert_eq!(
        defer_compaction(&mut f.store, &f.root, &request).unwrap()["already_recorded"],
        true
    );
    assert_eq!(rev, f.revision());
    assert_eq!(
        f.observe(observation(2, Some(180_000)))["state"],
        "handoff_candidate"
    );
    assert_eq!(
        defer_compaction(&mut f.store, &f.root, &request).unwrap()["state"],
        "handoff_candidate"
    );
    let rev = f.revision();
    assert!(
        f.store
            .record_compaction(
                f.project,
                rev,
                other.session,
                &observation(3, Some(100)),
                &CompactionPolicy::default()
            )
            .is_err()
    );
    assert!(
        f.store
            .compaction_events(other.project, f.session, 2)
            .is_err()
    );
    let mut spoof = EventDraft::new("client.compaction_observed", "Spoofed observation");
    spoof.session_id = Some(f.session);
    spoof.payload =
        json!({"observation":observation(3,Some(1)),"policy":CompactionPolicy::default()});
    assert!(f.store.append_event(f.project, rev, spoof).is_err());
    assert_eq!(rev, f.revision());
}

#[test]
fn one_bounded_action_preserves_context_gates_and_is_smaller_than_summary() {
    let mut f = Fixture::new();
    let mut full = f.prepare(5000);
    full["ok"] = json!(true);
    let summary = summarize_work_response(full.clone());
    let action = guide_prepared_work(&f.store, &f.root, full.clone(), Some(f.session)).unwrap();
    assert_eq!(
        action["context"]["work_context"]["rendered_context"],
        full["context"]["work_context"]["rendered_context"]
    );
    assert_eq!(
        action["context"]["work_context"]["context_hash"],
        full["context"]["work_context"]["context_hash"]
    );
    assert_eq!(
        action["context"]["work_context"]["identity"]["source_versions"],
        full["context"]["completeness"]["source_versions"]
    );
    assert_eq!(
        action["management"]["decision"]["required_actions"],
        full["management"]["decision"]["required_actions"]
    );
    assert_eq!(action["diagnostics"], full["diagnostics"]);
    let mut different = full.clone();
    different["context"]["completeness"]["source_versions"][0]["fingerprint"] =
        json!("different-observation");
    let retained =
        guide_prepared_work(&f.store, &f.root, different.clone(), Some(f.session)).unwrap();
    assert_eq!(
        retained["context"]["completeness"]["source_versions"],
        different["context"]["completeness"]["source_versions"]
    );
    assert_eq!(action["guidance"].as_object().unwrap().len(), 4);
    assert!(serde_json::to_vec(&action["guidance"]).unwrap().len() <= ACTION_GUIDANCE_MAX_BYTES);
    assert!(
        serde_json::to_vec(&action).unwrap().len() < serde_json::to_vec(&summary).unwrap().len()
    );
    assert!(
        awr_context::token_count(&serde_json::to_string(&action).unwrap())
            < awr_context::token_count(&serde_json::to_string(&summary).unwrap())
    );
    assert!(
        action["guidance"]["next_action"]
            .as_str()
            .unwrap()
            .contains("record known observations")
    );
    f.observe(observation(1, Some(160_000)));
    let mut full = f.prepare(5000);
    full["ok"] = json!(true);
    let summary = summarize_work_response(full.clone());
    let action = guide_prepared_work(&f.store, &f.root, full, Some(f.session)).unwrap();
    assert!(
        serde_json::to_vec(&action).unwrap().len() < serde_json::to_vec(&summary).unwrap().len()
    );
    assert!(
        action["guidance"]["next_action"]
            .as_str()
            .unwrap()
            .contains("ask before opening")
    );
    let source = f.root.join("work.yaml");
    fs::write(
        &source,
        fs::read_to_string(&source)
            .unwrap()
            .replace("acceptance: [Reviewed guide]", "acceptance: []"),
    )
    .unwrap();
    index_project(
        &mut f.store,
        &f.root,
        &Manifest::load(&f.root).unwrap(),
        false,
    )
    .unwrap();
    let incomplete = f.prepare(5000);
    let action =
        guide_prepared_work(&f.store, &f.root, incomplete.clone(), Some(f.session)).unwrap();
    assert_eq!(action["context"], incomplete["context"]);
    assert_eq!(action["ok"], false);
    assert!(
        action["guidance"]["next_action"]
            .as_str()
            .unwrap()
            .contains("required gaps")
    );
}

#[test]
fn uncertain_execution_takes_precedence_over_switch_advice() {
    let mut f = Fixture::new();
    f.observe(observation(1, Some(160_000)));
    let rev = f.revision();
    let (execution, event) = f
        .store
        .register_execution(
            f.project,
            rev,
            f.session,
            ExecutionIntent {
                operation_key: "fixture-run".into(),
                purpose: "Verify the guide".into(),
                executor: ExecutorKind::ManagedLocal,
                command: vec!["fixture-only".into()],
                cwd: f.root.to_string_lossy().into(),
                external_reference: None,
                inputs: Vec::new(),
                input_binding: awr_core::InputBinding::None,
            },
        )
        .unwrap();
    f.store
        .start_execution(
            f.project,
            event.project_revision,
            execution.id,
            WorkerIdentity {
                nonce: Id::new(),
                pid: 1,
                port: 1,
                child_pid: None,
            },
        )
        .unwrap();
    let result = f.inspect(false);
    assert_eq!(result["state"], "handoff_candidate");
    assert!(
        result["guidance"]["next_action"]
            .as_str()
            .unwrap()
            .contains("Query execution status")
    );
    let prepared = f.prepare(5000);
    let action = guide_prepared_work(&f.store, &f.root, prepared, Some(f.session)).unwrap();
    assert_eq!(action["guidance"], result["guidance"]);

    let mut external = Fixture::new();
    external.observe(observation(1, Some(160_000)));
    let rev = external.revision();
    let (job, event) = external
        .store
        .register_execution(
            external.project,
            rev,
            external.session,
            ExecutionIntent {
                operation_key: "external-fixture".into(),
                purpose: "Check external outcome".into(),
                executor: ExecutorKind::External,
                command: vec![],
                cwd: external.root.to_string_lossy().into(),
                external_reference: Some("fixture://execution".into()),
                inputs: Vec::new(),
                input_binding: awr_core::InputBinding::None,
            },
        )
        .unwrap();
    external
        .store
        .report_external_execution(
            external.project,
            event.project_revision,
            ExternalExecutionReport {
                version: 1,
                request_key: "external-unknown".into(),
                execution_id: job.id,
                host_id: "fixture".into(),
                host_work_key: "W".into(),
                native_session: "fixture-native".into(),
                agent_id: "writer".into(),
                origin: ExternalReportOrigin::HostObserved,
                phase: ExternalReportPhase::Unknown,
                observed_at: now_millis().unwrap(),
                summary: "Host lost the result".into(),
                detail_references: vec![],
            },
        )
        .unwrap();
    assert!(
        external.inspect(false)["guidance"]["next_action"]
            .as_str()
            .unwrap()
            .contains("Query execution status")
    );
}
