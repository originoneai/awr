#[path = "../../awr-store/tests/support/mod.rs"]
mod support;
use awr_context::{DeltaBaseline, DeltaRequest, recent_delta};
use awr_core::*;
use support::Fixture;

fn work(f: &Fixture, key: &str) -> WorkItem {
    WorkItem {
        spec_refs: vec![],
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
        milestone: Some("M4".into()),
        score: None,
        evidence_level: None,
        summary: "FACT_BODY_NOT_A_DELTA".into(),
        next_action: "Continue".into(),
        blocker: None,
        acceptance: vec!["Deliver".into()],
        tags: vec![],
        paths: vec![],
    }
}
fn start(f: &mut Fixture, key: &str) -> Session {
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    f.store
        .start_session(
            f.project.id,
            rev,
            SessionDraft {
                work_item_key: Some(key.into()),
                agent_id: "agent".into(),
                provider: "test".into(),
                model: "fixture".into(),
                branch_id: None,
                claim: false,
                claim_ttl_ms: None,
            },
        )
        .unwrap()
        .0
        .session
}
fn checkpoint(f: &mut Fixture, session: Id) -> Checkpoint {
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    f.store
        .create_checkpoint(
            f.project.id,
            rev,
            session,
            CheckpointDraft {
                context_hash: "a".repeat(64),
                digest: "Checkpoint".into(),
                next_action: "Continue".into(),
                open_loops: vec![],
                changed_entities: vec![],
            },
        )
        .unwrap()
        .0
}
fn append(f: &mut Fixture, work: Option<Id>, importance: &str) -> Event {
    let mut draft = EventDraft::new("work.observed", "A concise observation. ".repeat(80));
    draft.work_item_id = work;
    draft.importance = importance.into();
    draft.payload = serde_json::json!({"body":"EVENT_BODY_NOT_A_DELTA".repeat(1000)});
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    f.store.append_event(f.project.id, rev, draft).unwrap()
}
fn edge(f: &Fixture, to: &str) -> Edge {
    Edge {
        id: Id::new(),
        project_id: f.project.id,
        from_kind: EntityKind::WorkItem,
        from_key: "W".into(),
        relation: "depends_on".into(),
        to_kind: EntityKind::WorkItem,
        to_key: to.into(),
        required: true,
        revision: 1,
        source_ref: f.meta(to).source_ref,
    }
}

#[test]
fn delta_tracks_semantic_changes_and_folds_scoped_events_without_bodies() {
    let mut f = Fixture::new();
    let w = work(&f, "W");
    let unchanged = work(&f, "UNCHANGED");
    let removed = work(&f, "REMOVED");
    f.commit(ProjectionBatch {
        work_items: vec![w.clone(), unchanged.clone(), removed],
        edges: vec![edge(&f, "REMOVED")],
        ..Default::default()
    });
    append(&mut f, Some(w.meta.id), "critical");
    let session = start(&mut f, "W");
    let cp = checkpoint(&mut f, session.id);
    let mut changed = w.clone();
    changed.next_action = "Use new dependency".into();
    let mut batch = ProjectionBatch {
        work_items: vec![changed, unchanged.clone(), work(&f, "NEW")],
        edges: vec![edge(&f, "UNCHANGED")],
        ..Default::default()
    };
    for work in &mut batch.work_items {
        work.meta.source_ref.source_revision = f.source.revision + 1;
        work.meta.source_ref.source_fingerprint = "snapshot-2".into();
    }
    for edge in &mut batch.edges {
        edge.source_ref.source_revision = f.source.revision + 1;
        edge.source_ref.source_fingerprint = "snapshot-2".into();
    }
    f.source = f
        .store
        .commit_source_projection(&f.source, "snapshot-2", batch)
        .unwrap();
    for _ in 0..30 {
        append(&mut f, Some(w.meta.id), "normal");
    }
    append(&mut f, Some(w.meta.id), "high");
    let high = append(&mut f, Some(w.meta.id), "high");
    let critical = append(&mut f, None, "critical");
    append(&mut f, Some(unchanged.meta.id), "critical");
    let request = DeltaRequest {
        baseline: DeltaBaseline::Checkpoint { id: cp.id },
        event_limit: 2,
        ..Default::default()
    };
    let delta = recent_delta(&f.store, f.project.id, "W", None, &request).unwrap();
    assert_eq!(delta.after_revision, cp.project_revision);
    assert_eq!(delta.events.important_event_count, 3);
    assert_eq!(delta.events.omitted_important_events, 1);
    assert_eq!(delta.events.important_events[0].event.id, critical.id);
    assert_eq!(delta.events.important_events[1].event.id, high.id);
    assert!(
        delta
            .events
            .important_events
            .iter()
            .all(|e| e.summary.chars().count() <= 241)
    );
    let sources = &delta.events.source_changes;
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].changed_entity_count, 5);
    assert!(
        !sources[0]
            .changed_entities
            .iter()
            .any(|c| c.external_key == "UNCHANGED")
    );
    assert!(
        sources[0]
            .changed_entities
            .iter()
            .any(|c| c.external_key == "W"
                && c.latest_action == "updated"
                && c.before_revision == Some(1)
                && c.after_revision == Some(2))
    );
    assert!(
        sources[0]
            .changed_entities
            .iter()
            .any(|c| c.external_key == "REMOVED" && c.latest_action == "removed")
    );
    assert_eq!(
        sources[0].before.as_ref().unwrap().fingerprint,
        "snapshot-1"
    );
    assert_eq!(sources[0].after.as_ref().unwrap().fingerprint, "snapshot-2");
    assert_eq!(sources[0].legacy_events, 0);
    assert!(delta.gaps.is_empty());
    assert!(
        delta
            .events
            .process_history
            .iter()
            .any(|g| g.importance == "normal" && g.after_baseline == 30)
    );
    assert!(
        delta
            .events
            .process_history
            .iter()
            .any(|g| g.importance == "critical" && g.before_or_at_baseline == 1)
    );
    let text = serde_json::to_string(&delta).unwrap();
    assert!(!text.contains("EVENT_BODY_NOT_A_DELTA"));
    assert!(!text.contains("FACT_BODY_NOT_A_DELTA"));
    assert_eq!(
        text,
        serde_json::to_string(&recent_delta(&f.store, f.project.id, "W", None, &request).unwrap())
            .unwrap()
    );
    assert!(
        f.store.event(f.project.id, high.id).unwrap().payload["body"]
            .as_str()
            .unwrap()
            .contains("EVENT_BODY_NOT_A_DELTA")
    );
    assert_eq!(
        f.store.work_item(f.project.id, "W").unwrap().item.status,
        WorkStatus::InProgress
    );

    // A fingerprint-only refresh changes provenance, not every entity's semantic revision.
    let base = f.store.project(f.project.id).unwrap().project_revision;
    let mut batch = ProjectionBatch {
        work_items: f
            .store
            .work_items(f.project.id)
            .unwrap()
            .into_iter()
            .map(|w| w.item)
            .collect(),
        edges: vec![edge(&f, "UNCHANGED")],
        ..Default::default()
    };
    for work in &mut batch.work_items {
        work.meta.source_ref.source_revision = f.source.revision + 1;
        work.meta.source_ref.source_fingerprint = "snapshot-3".into();
    }
    batch.edges[0].source_ref.source_fingerprint = "snapshot-3".into();
    f.source = f
        .store
        .commit_source_projection(&f.source, "snapshot-3", batch)
        .unwrap();
    let delta = recent_delta(
        &f.store,
        f.project.id,
        "W",
        None,
        &DeltaRequest {
            baseline: DeltaBaseline::Revision { revision: base },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(delta.events.source_changes.len(), 1);
    assert_eq!(delta.events.source_changes[0].changed_entity_count, 0);
}

#[test]
fn baselines_reject_cross_work_and_future_revisions_and_retired_sources_remain_visible() {
    let mut f = Fixture::new();
    let w = work(&f, "W");
    f.commit(ProjectionBatch {
        work_items: vec![w.clone(), work(&f, "OTHER")],
        ..Default::default()
    });
    let own = start(&mut f, "W");
    let before_cp = recent_delta(
        &f.store,
        f.project.id,
        "W",
        None,
        &DeltaRequest {
            session_id: Some(own.id),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(before_cp.baseline_origin, "session_start");
    assert_eq!(before_cp.after_revision, own.start_project_revision);
    let cp = checkpoint(&mut f, own.id);
    let delta = recent_delta(
        &f.store,
        f.project.id,
        "W",
        None,
        &DeltaRequest {
            session_id: Some(own.id),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(delta.checkpoint_id, Some(cp.id));
    assert!(matches!(
        recent_delta(
            &f.store,
            f.project.id,
            "OTHER",
            None,
            &DeltaRequest {
                baseline: DeltaBaseline::Checkpoint { id: cp.id },
                ..Default::default()
            }
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        recent_delta(
            &f.store,
            f.project.id,
            "OTHER",
            None,
            &DeltaRequest {
                session_id: Some(own.id),
                ..Default::default()
            }
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        recent_delta(
            &f.store,
            f.project.id,
            "W",
            None,
            &DeltaRequest {
                baseline: DeltaBaseline::Revision { revision: 999999 },
                ..Default::default()
            }
        ),
        Err(Error::InvalidInput(_))
    ));
    let base = f.store.project(f.project.id).unwrap().project_revision;
    f.store.retire_source(&f.source).unwrap();
    let current = f.store.project(f.project.id).unwrap().project_revision;
    let delta = f
        .store
        .delta_events(f.project.id, current, w.meta.id, None, base, 2, 1)
        .unwrap();
    assert_eq!(delta.source_changes[0].latest_operation, "source.retired");
    assert!(!delta.source_changes[0].after.as_ref().unwrap().active);
    assert_eq!(delta.source_changes[0].changed_entity_count, 2);
    assert_eq!(delta.source_changes[0].omitted_entities, 1);
    assert!(
        delta.source_changes[0]
            .changed_entities
            .iter()
            .all(|c| c.latest_action == "removed")
    );
    assert!(
        f.store
            .event(f.project.id, delta.source_changes[0].last_event.id)
            .unwrap()
            .payload["changes"]
            .as_array()
            .unwrap()
            .len()
            == 2
    );
    assert!(matches!(
        f.store
            .delta_events(f.project.id, current - 1, w.meta.id, None, base, 2, 1),
        Err(Error::RevisionConflict { .. })
    ));
}
