mod support;
use awr_core::*;
use awr_store::Store;
use support::Fixture;

fn work(f: &Fixture, status: &str) -> WorkItem {
    WorkItem {
        spec_refs: vec![],
        ordinary_completion: None,
        archived: false,
        meta: f.meta("W"),
        title: "Implement runtime".into(),
        kind: None,
        owner: Some("source-team".into()),
        required: true,
        raw_status: status.into(),
        status: WorkStatus::normalize(status),
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: "Current source facts".into(),
        next_action: "Implement session".into(),
        blocker: None,
        acceptance: vec![],
        tags: vec![],
        paths: vec![],
    }
}
fn draft(agent: &str, claim: bool, ttl: Option<u64>) -> SessionDraft {
    SessionDraft {
        work_item_key: Some("W".into()),
        agent_id: agent.into(),
        provider: "fixture".into(),
        model: "test-model".into(),
        branch_id: None,
        claim,
        claim_ttl_ms: ttl,
    }
}

#[test]
fn session_claim_lifecycle_preserves_source_and_rejects_competing_writers() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![work(&f, "in_progress")],
        ..Default::default()
    });
    let baseline =
        serde_json::to_value(f.store.work_item(f.project.id, "W").unwrap().item).unwrap();
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    let (started, event) = f
        .store
        .start_session(f.project.id, rev, draft("executor-1", true, None))
        .unwrap();
    let session = started.session;
    let claim = started.claim.unwrap();
    assert_eq!(session.start_project_revision, rev);
    assert_eq!(event.project_revision, rev + 1);
    assert_eq!(session.agent_id, "executor-1");
    assert_eq!(session.provider, "fixture");
    assert_eq!(session.model, "test-model");
    assert_eq!(claim.session_id, session.id);
    assert_eq!(claim.agent_id, "executor-1");
    assert!(claim.active_at(now_millis().unwrap()));
    let mut other = Store::open(&f.root.join("state.db")).unwrap();
    assert!(matches!(
        other.start_session(f.project.id, rev, draft("executor-2", true, None)),
        Err(Error::RevisionConflict { .. })
    ));
    assert!(matches!(
        other.start_session(
            f.project.id,
            event.project_revision,
            draft("executor-2", true, None)
        ),
        Err(Error::ClaimConflict(_))
    ));
    assert_eq!(other.sessions(f.project.id, false, 100).unwrap().len(), 1);
    assert_eq!(
        other.project(f.project.id).unwrap().project_revision,
        event.project_revision
    );
    let (observer, observed) = other
        .start_session(
            f.project.id,
            event.project_revision,
            draft("observer", false, None),
        )
        .unwrap();
    assert!(matches!(
        other.release_claim(
            f.project.id,
            observed.project_revision,
            observer.session.id,
            claim.id
        ),
        Err(Error::ClaimConflict(_))
    ));
    let (released, released_event) = f
        .store
        .release_claim(
            f.project.id,
            observed.project_revision,
            session.id,
            claim.id,
        )
        .unwrap();
    assert_eq!(released.status, "released");
    assert!(released.released_at.is_some());
    let (reclaimed, reclaimed_event) = f
        .store
        .acquire_claim(
            f.project.id,
            released_event.project_revision,
            session.id,
            Some(60_000),
        )
        .unwrap();
    assert_ne!(reclaimed.id, claim.id);
    let (ended, ended_event) = f
        .store
        .end_session(
            f.project.id,
            reclaimed_event.project_revision,
            session.id,
            SessionOutcome::Incomplete,
        )
        .unwrap();
    assert_eq!(ended.status, "incomplete");
    assert_eq!(
        ended.end_project_revision,
        Some(ended_event.project_revision)
    );
    assert_eq!(
        f.store.claim(f.project.id, reclaimed.id).unwrap().status,
        "released"
    );
    assert!(matches!(
        f.store
            .acquire_claim(f.project.id, ended_event.project_revision, session.id, None),
        Err(Error::InvalidTransition(_))
    ));
    assert_eq!(
        serde_json::to_value(f.store.work_item(f.project.id, "W").unwrap().item).unwrap(),
        baseline
    );
    assert!(f.store.doctor().unwrap().ok);
}

#[test]
fn ttl_reclamation_is_atomic_and_cleanup_remains_possible_with_stale_sources() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![work(&f, "ready")],
        ..Default::default()
    });
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    assert!(matches!(
        f.store
            .start_session(f.project.id, rev, draft("a", true, Some(0))),
        Err(Error::InvalidInput(_))
    ));
    assert!(
        f.store
            .sessions(f.project.id, false, 10)
            .unwrap()
            .is_empty()
    );
    let (first, event) = f
        .store
        .start_session(f.project.id, rev, draft("a", true, Some(1)))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let (second, event) = f
        .store
        .start_session(f.project.id, event.project_revision, draft("b", true, None))
        .unwrap();
    assert_eq!(
        f.store
            .claim(f.project.id, first.claim.unwrap().id)
            .unwrap()
            .status,
        "expired"
    );
    assert_eq!(
        event.payload["expired_claim_ids"].as_array().unwrap().len(),
        1
    );
    f.source = f
        .store
        .mark_source_freshness(&f.source, Freshness::Unavailable)
        .unwrap();
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    assert!(matches!(
        f.store
            .start_session(f.project.id, rev, draft("c", false, None)),
        Err(Error::SourceStale(_))
    ));
    f.store
        .end_session(
            f.project.id,
            rev,
            second.session.id,
            SessionOutcome::Interrupted,
        )
        .unwrap();
    assert_eq!(
        f.store
            .claim(f.project.id, second.claim.unwrap().id)
            .unwrap()
            .status,
        "released"
    );
}

#[test]
fn session_requires_known_work_branch_and_executable_claim_state() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![work(&f, "blocked")],
        ..Default::default()
    });
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    assert!(matches!(
        f.store
            .start_session(f.project.id, rev, draft("a", true, None)),
        Err(Error::DependencyBlocked(_))
    ));
    let mut invalid = draft("a", false, None);
    invalid.branch_id = Some(Id::new());
    assert!(matches!(
        f.store.start_session(f.project.id, rev, invalid),
        Err(Error::NotFound(_))
    ));
    let mut invalid = draft("a", false, None);
    invalid.work_item_key = Some("missing".into());
    assert!(matches!(
        f.store.start_session(f.project.id, rev, invalid),
        Err(Error::NotFound(_))
    ));
    let (session, _) = f
        .store
        .start_session(f.project.id, rev, draft("diagnostic-agent", false, None))
        .unwrap();
    assert!(session.claim.is_none());
    assert!(matches!(
        f.store.session(Id::new(), session.session.id),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn claims_are_work_exclusive_and_session_branch_provenance_is_preserved() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![work(&f, "ready")],
        ..Default::default()
    });
    let branch = Id::new();
    let db = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    db.execute("INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision) VALUES(?1,?2,'parallel-work',0,'active',1)",rusqlite::params![branch.to_string(),f.project.id.to_string()]).unwrap();
    let revision = f.store.project(f.project.id).unwrap().project_revision;
    let (unbranched, event) = f
        .store
        .start_session(f.project.id, revision, draft("main-agent", true, None))
        .unwrap();
    let mut request = draft("branch-agent", true, None);
    request.branch_id = Some(branch);
    assert!(matches!(
        f.store
            .start_session(f.project.id, event.project_revision, request.clone()),
        Err(Error::ClaimConflict(_))
    ));
    let (_, event) = f
        .store
        .end_session(
            f.project.id,
            event.project_revision,
            unbranched.session.id,
            SessionOutcome::Ended,
        )
        .unwrap();
    let (branched, event) = f
        .store
        .start_session(f.project.id, event.project_revision, request)
        .unwrap();
    assert_eq!(branched.session.branch_id, Some(branch));
    assert_eq!(branched.claim.as_ref().unwrap().branch_id, Some(branch));
    assert_eq!(event.branch_id, Some(branch));
    f.store
        .end_session(
            f.project.id,
            event.project_revision,
            branched.session.id,
            SessionOutcome::Ended,
        )
        .unwrap();
    assert_eq!(
        f.store
            .claim(f.project.id, unbranched.claim.unwrap().id)
            .unwrap()
            .status,
        "released"
    );
    assert_eq!(
        f.store
            .claim(f.project.id, branched.claim.unwrap().id)
            .unwrap()
            .status,
        "released"
    );
}

#[test]
fn handoff_rejects_other_branches_and_never_revives_expired_claims() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![work(&f, "ready")],
        ..Default::default()
    });
    let revision = f.store.project(f.project.id).unwrap().project_revision;
    let (sender, event) = f
        .store
        .start_session(f.project.id, revision, draft("sender", true, Some(1)))
        .unwrap();
    let (receiver, event) = f
        .store
        .start_session(
            f.project.id,
            event.project_revision,
            draft("receiver", false, None),
        )
        .unwrap();
    let branch = Id::new();
    let db = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    db.execute("INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision) VALUES(?1,?2,'other',0,'active',1)",rusqlite::params![branch.to_string(),f.project.id.to_string()]).unwrap();
    let mut request = draft("branch-agent", false, None);
    request.branch_id = Some(branch);
    let (other, event) = f
        .store
        .start_session(f.project.id, event.project_revision, request)
        .unwrap();
    assert!(matches!(
        f.store
            .select_active_session(f.project.id, None, None, None, None),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        f.store
            .select_active_session(f.project.id, None, None, None, Some(branch))
            .unwrap()
            .id,
        other.session.id
    );
    let (_, event) = f
        .store
        .create_checkpoint(
            f.project.id,
            event.project_revision,
            sender.session.id,
            CheckpointDraft {
                context_hash: "a".repeat(64),
                digest: "Handoff preparation".into(),
                next_action: "Continue work".into(),
                open_loops: vec!["unresolved input".into()],
                changed_entities: vec![],
            },
        )
        .unwrap();
    assert!(matches!(
        f.store.handoff(
            f.project.id,
            event.project_revision,
            sender.session.id,
            Some(other.session.id),
            None
        ),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        event.project_revision
    );
    std::thread::sleep(std::time::Duration::from_millis(10));
    let (handoff, _) = f
        .store
        .handoff(
            f.project.id,
            event.project_revision,
            sender.session.id,
            Some(receiver.session.id),
            Some(60_000),
        )
        .unwrap();
    assert!(handoff.transferred_claim.is_none());
    assert_eq!(
        f.store
            .claim(f.project.id, sender.claim.unwrap().id)
            .unwrap()
            .status,
        "expired"
    );
    assert!(
        f.store
            .session_claims(f.project.id, receiver.session.id)
            .unwrap()
            .is_empty()
    );
    assert!(f.store.doctor().unwrap().ok);
}
