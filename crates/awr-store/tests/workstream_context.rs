use awr_core::*;
#[allow(dead_code)]
#[path = "support/workstreams.rs"]
mod fixture;
use fixture::Fixture;

#[test]
fn dependency_boundaries_never_read_hidden_status_names_or_descendants() {
    let mut f = Fixture::new();
    f.dependency("W0", "W2", true);
    let hidden = f.dependency("W2", "W1", true);
    let missing = f.dependency("W0", "missing-private-key", true);
    f.dependency("W1", "W3", true);
    f.dependency("W3", "W0", true); // The private subgraph must not leak a cycle.
    f.dependency("W0", "optional-private-key", false);
    f.batch.work_items[1].status = WorkStatus::Completed;
    f.batch.work_items[1].raw_status = "completed".into();
    f.reproject();
    let read = f.read(0);
    let scoped = read.dependency_closure("W0", true).unwrap();
    assert_eq!(scoped.graph.dependencies.len(), 1);
    assert_eq!(scoped.graph.dependencies[0].item.meta.external_key, "W2");
    assert_eq!(scoped.graph.edges.len(), 1);
    assert!(scoped.graph.cycle_keys.is_empty());
    assert!(scoped.graph.missing_keys.is_empty());
    let ids = scoped
        .unavailable_dependencies
        .iter()
        .map(|d| d.edge_id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids, std::collections::BTreeSet::from([hidden, missing]));
    let output = serde_json::to_string(&scoped).unwrap();
    for private in [
        "\"W1\"",
        "\"W3\"",
        "missing-private-key",
        "optional-private-key",
        "Search component 1",
    ] {
        assert!(!output.contains(private), "leaked {private}");
    }
    assert_eq!(
        read.dependency_closure("W0", false)
            .unwrap()
            .unavailable_dependencies
            .len(),
        3
    );
    assert!(matches!(
        read.dependency_closure("W1", true),
        Err(Error::Workstream(WorkstreamError::AccessDenied))
    ));
    // Trusted legacy graph remains available and unchanged to project administrators.
    let all = f.store.dependency_closure(f.project, "W0", true).unwrap();
    assert_eq!(all.dependencies.len(), 3);
    assert!(!all.cycle_keys.is_empty());
}

#[test]
fn traversal_stops_at_a_hidden_bridge_and_keeps_local_cycles() {
    let mut f = Fixture::new();
    f.dependency("W0", "W1", true);
    f.dependency("W1", "W2", true);
    f.reproject();
    let graph = f.read(0).dependency_closure("W0", true).unwrap();
    assert!(graph.graph.dependencies.is_empty());
    assert_eq!(graph.unavailable_dependencies.len(), 1);
    f.dependency("W0", "W2", true);
    f.dependency("W2", "W0", true);
    f.reproject();
    let graph = f.read(0).dependency_closure("W0", true).unwrap();
    assert_eq!(graph.graph.dependencies.len(), 1);
    assert_eq!(graph.graph.cycle_keys, vec!["W0", "W2"]);
}

#[test]
fn typed_goals_rules_and_session_selection_use_the_same_visibility() {
    let mut f = Fixture::new();
    f.start(1);
    f.start(3);
    let own = f.start(0);
    let read = f.read(0);
    assert_eq!(
        read.goals()
            .unwrap()
            .iter()
            .map(|g| g.item.meta.external_key.as_str())
            .collect::<Vec<_>>(),
        vec!["G0"]
    );
    assert_eq!(read.rules().unwrap()[0].item.meta.external_key, "SHARED");
    assert_eq!(read.work_items().unwrap().len(), 2);
    assert_eq!(
        read.select_active_session(None, None, None).unwrap().id,
        own.id
    );
    assert!(matches!(
        read.select_active_session(Some(f.works[1]), None, None),
        Err(Error::Workstream(WorkstreamError::AccessDenied))
    ));
    f.start(2);
    assert!(matches!(
        f.read(0).select_active_session(None, None, None),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        f.read(0)
            .select_active_session(Some(f.works[0]), None, None)
            .unwrap()
            .id,
        own.id
    );
}

#[test]
fn moved_back_sessions_keep_history_but_cannot_supply_current_context() {
    let mut f = Fixture::new();
    let old = f.start(0);
    let cp = f.checkpoint(old.id);
    assert_eq!(
        f.read(0).context_checkpoint(cp.id, "W0", None).unwrap().id,
        cp.id
    );
    f.store
        .end_session(f.project, f.rev(), old.id, SessionOutcome::Ended)
        .unwrap();
    f.move_work(0, 1);
    f.move_work(0, 0);
    let read = f.read(0);
    assert_eq!(read.session(old.id).unwrap().id, old.id);
    assert_eq!(read.checkpoint(cp.id).unwrap().id, cp.id);
    assert!(matches!(
        read.context_session(old.id),
        Err(Error::Workstream(WorkstreamError::BindingMismatch))
    ));
    assert!(matches!(
        read.context_checkpoint(cp.id, "W0", None),
        Err(Error::Workstream(WorkstreamError::BindingMismatch))
    ));
    assert!(matches!(
        read.context_recovery_checkpoint(old.id),
        Err(Error::Workstream(WorkstreamError::BindingMismatch))
    ));
    assert!(read.latest_work_checkpoint("W0", None).unwrap().is_none());
    let current = f.start(0);
    assert_eq!(
        f.read(0).context_session(current.id).unwrap().id,
        current.id
    );
}

#[test]
fn execution_context_keeps_all_current_results_and_drops_old_ownership_generations() {
    let mut f = Fixture::new();
    let session = f.start(0);
    let other = f.start(1);
    fn register(f: &mut Fixture, session: Id, key: &str) -> Execution {
        f.store
            .register_execution(
                f.project,
                f.rev(),
                session,
                ExecutionIntent {
                    operation_key: key.into(),
                    purpose: "Synthetic scoped execution".into(),
                    executor: ExecutorKind::ManagedLocal,
                    command: vec!["synthetic-worker".into()],
                    cwd: f.root.to_string_lossy().into_owned(),
                    external_reference: None,
                    inputs: Vec::new(),
                    input_binding: awr_core::InputBinding::None,
                },
            )
            .unwrap()
            .0
    }
    let old = register(&mut f, session.id, "old");
    register(&mut f, other.id, "other");
    assert_eq!(
        f.read(0)
            .context_executions("W0", None)
            .unwrap()
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        vec![old.id]
    );
    let nonce = Id::new();
    f.store
        .start_execution(
            f.project,
            f.rev(),
            old.id,
            WorkerIdentity {
                nonce,
                pid: 1,
                port: 1,
                child_pid: None,
            },
        )
        .unwrap();
    f.store
        .finish_execution(
            f.project,
            f.rev(),
            ExecutionResult {
                execution_id: old.id,
                nonce,
                finished_at: now_millis().unwrap(),
                success: true,
                exit_code: Some(0),
                signal: None,
                error: None,
            },
        )
        .unwrap();
    assert_eq!(
        f.read(0).context_executions("W0", None).unwrap()[0].state,
        ExecutionState::Succeeded
    );
    f.store
        .end_session(f.project, f.rev(), session.id, SessionOutcome::Ended)
        .unwrap();
    f.move_work(0, 1);
    assert!(f.read(1).context_executions("W0", None).unwrap().is_empty());
    f.move_work(0, 0);
    assert!(f.read(0).context_executions("W0", None).unwrap().is_empty());
    let current = f.start(0);
    let new = register(&mut f, current.id, "current");
    assert_eq!(
        f.read(0)
            .context_executions("W0", None)
            .unwrap()
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        vec![new.id]
    );
    assert_eq!(
        f.store
            .executions(f.project, Some(f.works[0]))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn old_evidence_keeps_historical_attribution_without_becoming_current_after_round_trip() {
    let mut f = Fixture::new();
    let old = f.evidence(0);
    assert_eq!(
        f.read(0)
            .context_evidence_for_work("W0", None, None)
            .unwrap()
            .len(),
        1
    );
    f.move_work(0, 1);
    f.move_work(0, 0);
    let read = f.read(0);
    assert_eq!(read.evidence(&old.id.to_string()).unwrap().item.id, old.id);
    assert!(
        read.context_evidence_for_work("W0", None, None)
            .unwrap()
            .is_empty()
    );
    let mut newer = EvidenceDraft {
        work_item_key: Some("W0".into()),
        external_key: "E0-current".into(),
        evidence_type: "report".into(),
        level: EvidenceLevel::Designed,
        summary: "Current synthetic evidence".into(),
        locator: "new.txt".into(),
        sha256: None,
        source_sha: None,
        command: None,
        scope: vec!["W0".into()],
        branch_id: None,
        verified_at: None,
    };
    let current = f
        .store
        .record_evidence(f.project, f.rev(), newer.clone())
        .unwrap()
        .0;
    assert_eq!(
        f.read(0)
            .context_evidence_for_work("W0", None, None)
            .unwrap()[0]
            .evidence
            .item
            .id,
        current.id
    );
    // Unrelated stream evidence cannot enter this packet or change its coverage.
    newer.work_item_key = Some("W1".into());
    newer.external_key = "E1-current".into();
    newer.scope = vec!["W1".into()];
    f.store.record_evidence(f.project, f.rev(), newer).unwrap();
    assert_eq!(
        f.read(0)
            .context_evidence_for_work("W0", None, None)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn legacy_dependency_context_preserves_missing_references_and_graph_shape() {
    let mut f = Fixture::legacy();
    f.dependency("W0", "W1", true);
    f.dependency("W1", "not-yet-declared", true);
    f.reproject();
    let before = f.rev();
    let scoped = f.read(0).dependency_closure("W0", true).unwrap();
    let legacy = f.store.dependency_closure(f.project, "W0", true).unwrap();
    assert!(scoped.unavailable_dependencies.is_empty());
    assert_eq!(
        serde_json::to_value(scoped.graph).unwrap(),
        serde_json::to_value(legacy).unwrap()
    );
    assert_eq!(f.rev(), before);
    assert!(f.store.doctor().unwrap().ok);
}
