mod support;
use awr_core::*;
use awr_store::{BranchFilter, EventQuery};
use support::Fixture;

fn work(f: &Fixture, key: &str) -> WorkItem {
    WorkItem {
        ordinary_completion: None,
        archived: false,
        meta: f.meta(key),
        title: key.into(),
        kind: None,
        owner: None,
        required: true,
        raw_status: "in_progress".into(),
        status: WorkStatus::InProgress,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: "Small current summary".into(),
        next_action: "Continue".into(),
        blocker: None,
        acceptance: vec![],
        tags: vec![],
        paths: vec![],
    }
}
fn session() -> SessionDraft {
    SessionDraft {
        work_item_key: Some("W".into()),
        agent_id: "executor".into(),
        provider: "fixture".into(),
        model: "test".into(),
        branch_id: None,
        claim: false,
        claim_ttl_ms: None,
    }
}

#[test]
fn scoped_append_and_cursor_queries_preserve_work_and_immutable_history() {
    let mut f = Fixture::new();
    let work = work(&f, "W");
    let wid = work.meta.id;
    f.commit(ProjectionBatch {
        work_items: vec![work],
        ..Default::default()
    });
    let revision = f.store.project(f.project.id).unwrap().project_revision;
    let (session, event) = f
        .store
        .start_session(f.project.id, revision, session())
        .unwrap();
    let start = event.project_revision;
    let mut forged = EventDraft::new("session.handoff_received", "Fabricated handoff");
    forged.session_id = Some(session.session.id);
    assert!(matches!(
        f.store.append_event(f.project.id, start, forged),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        start
    );
    let mut revision = start;
    let mut ids = Vec::new();
    for kind in ["test.started", "test.failed", "test.passed"] {
        let mut draft = EventDraft::new(kind, format!("Observed {kind}"));
        draft.session_id = Some(session.session.id);
        draft.importance = "high".into();
        draft.payload = serde_json::json!({"report":"reports/check.json"});
        let event = f.store.append_event(f.project.id, revision, draft).unwrap();
        assert_eq!(event.work_item_id, Some(wid));
        assert_eq!(event.branch_id, None);
        assert!(event.created_at > 0);
        revision = event.project_revision;
        ids.push(event.id);
    }
    let first = f
        .store
        .query_events(
            f.project.id,
            &EventQuery {
                work_item_id: Some(wid),
                session_id: Some(session.session.id),
                branch: BranchFilter::Main,
                importance: Some("high".into()),
                after_revision: start,
                limit: 2,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(first.events.len(), 2);
    assert_eq!(first.events[0].id, ids[0]);
    assert_eq!(first.events[1].id, ids[1]);
    let next = f
        .store
        .query_events(
            f.project.id,
            &EventQuery {
                work_item_id: Some(wid),
                cursor: first.next_cursor.clone(),
                limit: 2,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(next.events.len(), 1);
    assert_eq!(next.events[0].id, ids[2]);
    assert!(next.next_cursor.is_none());
    let failed = f
        .store
        .query_events(
            f.project.id,
            &EventQuery {
                event_type: Some("test.failed".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(failed.events.len(), 1);
    assert_eq!(failed.events[0].importance, "high");
    assert_eq!(
        f.store.event(f.project.id, ids[1]).unwrap().payload["report"],
        "reports/check.json"
    );
    assert_eq!(
        f.store.work_item(f.project.id, "W").unwrap().item.summary,
        "Small current summary"
    );
    let db = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    assert!(
        db.execute(
            "UPDATE events SET summary='tampered' WHERE id=?1",
            [ids[1].to_string()]
        )
        .is_err()
    );
    assert!(
        db.execute("DELETE FROM events WHERE id=?1", [ids[1].to_string()])
            .is_err()
    );
    let mut wrong = first.next_cursor.unwrap();
    wrong.project_id = Id::new();
    assert!(matches!(
        f.store.query_events(
            f.project.id,
            &EventQuery {
                cursor: Some(wrong),
                ..Default::default()
            }
        ),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        revision
    );
}

#[test]
fn legacy_source_events_keep_unknown_history_and_new_source_receipts_cannot_be_forged() {
    let mut f = Fixture::new();
    let work = work(&f, "W");
    let wid = work.meta.id;
    f.commit(ProjectionBatch {
        work_items: vec![work],
        ..Default::default()
    });
    let baseline = f.store.project(f.project.id).unwrap().project_revision;
    // Simulate opening an event log written by the version before source change receipts.
    let id = Id::new();
    let db = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    db.execute("INSERT INTO events(id,project_id,event_type,importance,summary,payload_json,project_revision,created_at)
        VALUES(?1,?2,'source.projected','normal','Legacy source projection',?3,?4,1)",
        rusqlite::params![id.to_string(), f.project.id.to_string(), serde_json::json!({"source_id":f.source.id,"fingerprint":"historical-value"}).to_string(), (baseline+1) as i64]).unwrap();
    db.execute(
        "UPDATE projects SET project_revision=project_revision+1 WHERE id=?1",
        [f.project.id.to_string()],
    )
    .unwrap();
    let delta = f
        .store
        .delta_events(f.project.id, baseline + 1, wid, None, baseline, 2, 2)
        .unwrap();
    assert_eq!(delta.source_changes[0].legacy_events, 1);
    assert!(!delta.source_changes[0].before_known);
    assert!(delta.source_changes[0].before.is_none());
    assert!(delta.source_changes[0].after.is_none());
    assert!(delta.source_changes[0].changed_entities.is_empty());
    assert_eq!(delta.source_changes[0].last_event.id, id);
    assert_eq!(
        f.store.event(f.project.id, id).unwrap().payload["fingerprint"],
        "historical-value"
    );
    for kind in [
        "source.projected",
        "source.retired",
        "source.registered",
        "source.configured",
        "source.freshness_changed",
    ] {
        let mut forged = EventDraft::new(kind, "Fabricated source change");
        forged.payload = serde_json::json!({"source_id":f.source.id});
        assert!(matches!(
            f.store.append_event(f.project.id, baseline + 1, forged),
            Err(Error::InvalidInput(_))
        ));
    }
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        baseline + 1
    );
    let all = f
        .store
        .delta_events(f.project.id, baseline + 1, wid, None, 0, 2, 2)
        .unwrap();
    assert!(!all.source_changes[0].before.is_some());
    assert!(all.source_changes[0].before_known); // The initial registration is known to have no predecessor.
    assert!(all.source_changes[0].event_count >= 3);
}

#[test]
fn event_scope_conflicts_and_ended_sessions_cannot_append() {
    let mut f = Fixture::new();
    let other = work(&f, "OTHER");
    let other_id = other.meta.id;
    f.commit(ProjectionBatch {
        work_items: vec![work(&f, "W"), other],
        ..Default::default()
    });
    let revision = f.store.project(f.project.id).unwrap().project_revision;
    let (session, event) = f
        .store
        .start_session(f.project.id, revision, session())
        .unwrap();
    let mut bad = EventDraft::new("work.progress", "Conflicting scope");
    bad.session_id = Some(session.session.id);
    bad.work_item_id = Some(other_id);
    assert!(matches!(
        f.store
            .append_event(f.project.id, event.project_revision, bad),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        event.project_revision
    );
    let (_, ended) = f
        .store
        .end_session(
            f.project.id,
            event.project_revision,
            session.session.id,
            SessionOutcome::Ended,
        )
        .unwrap();
    let mut late = EventDraft::new("work.progress", "Late append");
    late.session_id = Some(session.session.id);
    assert!(matches!(
        f.store
            .append_event(f.project.id, ended.project_revision, late),
        Err(Error::InvalidTransition(_))
    ));
    assert!(matches!(
        f.store.query_events(
            f.project.id,
            &EventQuery {
                session_id: Some(Id::new()),
                ..Default::default()
            }
        ),
        Err(Error::NotFound(_))
    ));
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        ended.project_revision
    );
}

#[test]
fn named_branch_filter_does_not_mix_unbranched_events() {
    let mut f = Fixture::new();
    let work = work(&f, "W");
    let wid = work.meta.id;
    f.commit(ProjectionBatch {
        work_items: vec![work],
        ..Default::default()
    });
    let branch = Id::new();
    let db = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    db.execute("INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision) VALUES(?1,?2,'isolated-events',0,'active',1)",rusqlite::params![branch.to_string(),f.project.id.to_string()]).unwrap();
    let mut revision = f.store.project(f.project.id).unwrap().project_revision;
    for scope in [None, Some(branch)] {
        let mut draft = EventDraft::new("work.observed", "Scoped observation");
        draft.work_item_id = Some(wid);
        draft.branch_id = scope;
        revision = f
            .store
            .append_event(f.project.id, revision, draft)
            .unwrap()
            .project_revision;
    }
    let named = f
        .store
        .query_events(
            f.project.id,
            &EventQuery {
                branch: BranchFilter::Branch(branch),
                work_item_id: Some(wid),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(named.events.len(), 1);
    assert_eq!(named.events[0].branch_id, Some(branch));
    let main = f
        .store
        .query_events(
            f.project.id,
            &EventQuery {
                branch: BranchFilter::Main,
                work_item_id: Some(wid),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(main.events.len(), 1);
    assert_eq!(main.events[0].branch_id, None);
    let all = f
        .store
        .query_events(
            f.project.id,
            &EventQuery {
                branch: BranchFilter::Any,
                work_item_id: Some(wid),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(all.events.len(), 2);
}

#[test]
fn newest_event_time_per_work_spans_sessions_and_branches_and_skips_unattributed_events() {
    let mut f = Fixture::new();
    let (a, b, quiet) = (work(&f, "A"), work(&f, "B"), work(&f, "QUIET"));
    let (aid, bid, quiet_id) = (a.meta.id, b.meta.id, quiet.meta.id);
    f.commit(ProjectionBatch {
        work_items: vec![a, b, quiet],
        ..Default::default()
    });
    // Projection receipts are not attributed to any work.
    assert!(f.store.last_event_times(f.project.id).unwrap().is_empty());

    let revision = f.store.project(f.project.id).unwrap().project_revision;
    let mut draft = session();
    draft.work_item_key = Some("A".into());
    let (_, started) = f
        .store
        .start_session(f.project.id, revision, draft)
        .unwrap();
    // A session event alone attributes the work and dates it.
    assert_eq!(
        f.store.last_event_times(f.project.id).unwrap()[&aid],
        started.created_at
    );

    let branch = Id::new();
    let db = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    db.execute("INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision) VALUES(?1,?2,'review',0,'active',1)",rusqlite::params![branch.to_string(),f.project.id.to_string()]).unwrap();
    let mut revision = started.project_revision;
    for (work, branch_id) in [(Some(aid), None), (Some(bid), Some(branch)), (None, None)] {
        let mut draft = EventDraft::new("work.observed", "Observation");
        draft.work_item_id = work;
        draft.branch_id = branch_id;
        revision = f
            .store
            .append_event(f.project.id, revision, draft)
            .unwrap()
            .project_revision;
    }

    // Every work equals the newest `created_at` that event history shows for it on any branch.
    let last = f.store.last_event_times(f.project.id).unwrap();
    for (id, name) in [(aid, "A"), (bid, "B")] {
        let newest = f
            .store
            .query_events(
                f.project.id,
                &EventQuery {
                    work_item_id: Some(id),
                    branch: BranchFilter::Any,
                    limit: 1000,
                    ..Default::default()
                },
            )
            .unwrap()
            .events
            .iter()
            .map(|e| e.created_at)
            .max()
            .unwrap();
        assert_eq!(last[&id], newest, "{name}");
    }
    // The branch event counts for B although the main line has none for it; the unattributed event counts for nobody.
    assert!(
        f.store
            .query_events(
                f.project.id,
                &EventQuery {
                    work_item_id: Some(bid),
                    branch: BranchFilter::Main,
                    ..Default::default()
                },
            )
            .unwrap()
            .events
            .is_empty()
    );
    assert!(last.contains_key(&bid));
    assert!(!last.contains_key(&quiet_id));
    assert_eq!(last.len(), 2);
}
