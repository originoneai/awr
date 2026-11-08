//! Completion and reopening refuse to close a work whose supported goal section
//! changed after the work was claimed. The fingerprint snapshot is recorded by
//! the `work.claimed` event; re-claiming refreshes it as the lightweight
//! re-confirmation path. Works without goal references or without a recorded
//! snapshot are never blocked.
#[allow(dead_code)]
mod support;
use awr_core::*;
use serde_json::json;
use support::Fixture;

fn goal(f: &Fixture, key: &str, fingerprint: &str) -> Goal {
    let mut meta = f.meta(key);
    meta.source_ref.section_fingerprint = Some(fingerprint.into());
    Goal {
        meta,
        title: format!("Goal {key}"),
        status: "active".into(),
        priority: None,
        success_criteria: vec![],
        summary: String::new(),
    }
}

fn work(f: &Fixture, key: &str) -> WorkItem {
    WorkItem {
        spec_refs: vec![],
        definition: None,
        ordinary_completion: None,
        archived: false,
        meta: f.meta(key),
        title: format!("Work {key}"),
        kind: None,
        owner: None,
        required: true,
        raw_status: "in_progress".into(),
        status: WorkStatus::InProgress,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: "Current source facts".into(),
        next_action: "Go on".into(),
        blocker: None,
        acceptance: vec![],
        tags: vec![],
        paths: vec![],
    }
}

fn supports(f: &Fixture, work_key: &str, goal_key: &str) -> Edge {
    Edge {
        id: Id::new(),
        project_id: f.project.id,
        from_kind: EntityKind::WorkItem,
        from_key: work_key.into(),
        relation: "supports".into(),
        to_kind: EntityKind::Goal,
        to_key: goal_key.into(),
        required: false,
        revision: 1,
        source_ref: f.meta(work_key).source_ref,
    }
}

/// A work `W` supporting goal `G` with `fingerprint`, plus extra works/goals as given.
fn fixture(_fingerprint: &str, goals: &[(&str, &str)], edges: &[(&str, &str)]) -> Fixture {
    let mut f = Fixture::new();
    let mut batch = ProjectionBatch {
        work_items: vec![work(&f, "W")],
        ..Default::default()
    };
    for (key, fp) in goals {
        batch.goals.push(goal(&f, key, fp));
    }
    for (from, to) in edges {
        batch.edges.push(supports(&f, from, to));
    }
    f.commit(batch);
    f
}

/// Re-commits a projection batch under a fresh source fingerprint (a repeat of
/// the same fingerprint is treated as an unchanged source and skipped). Entity
/// provenance is aligned with the new snapshot, mirroring what a real re-index
/// of the changed source would produce.
fn recommit(f: &mut Fixture, build: impl FnOnce(&Fixture) -> ProjectionBatch) {
    let mut batch = build(f);
    let align = |reference: &mut SourceRef| {
        reference.source_fingerprint = "snapshot-2".into();
        reference.source_revision = f.source.revision + 1;
    };
    for goal in &mut batch.goals {
        align(&mut goal.meta.source_ref);
    }
    for item in &mut batch.work_items {
        align(&mut item.meta.source_ref);
    }
    for edge in &mut batch.edges {
        align(&mut edge.source_ref);
    }
    f.source = f
        .store
        .commit_source_projection(&f.source, "snapshot-2", batch)
        .unwrap();
}

/// The current projection id of an entity, so a re-commit preserves identity.
fn existing_id(f: &Fixture, kind: EntityKind, key: &str) -> Id {
    f.store
        .object_projection(f.project.id, kind, key)
        .unwrap()
        .item["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn revision(f: &Fixture) -> Revision {
    f.store.project(f.project.id).unwrap().project_revision
}

/// A session on `W` holding a claim, so the claim event records the snapshot.
fn claimed(f: &mut Fixture, agent: &str) -> Session {
    let expected = revision(f);
    let (started, _) = f
        .store
        .start_session(
            f.project.id,
            expected,
            SessionDraft {
                work_item_key: Some("W".into()),
                agent_id: agent.into(),
                provider: "fixture".into(),
                model: "test".into(),
                branch_id: None,
                claim: true,
                claim_ttl_ms: None,
            },
        )
        .unwrap();
    started.session
}

/// Re-claims `W` from a fresh session after releasing the old claim.
fn reclaim(f: &mut Fixture, previous: Session) -> Session {
    let expected = revision(f);
    f.store
        .release_claim(f.project.id, expected, previous.id, {
            let claim = f.store.session_claims(f.project.id, previous.id).unwrap();
            claim[0].id
        })
        .unwrap();
    claimed(f, "executor-2")
}

#[test]
fn an_unchanged_goal_never_blocks_the_check() {
    let mut f = fixture("fp-1", &[("G", "fp-1")], &[("W", "G")]);
    claimed(&mut f, "executor");
    f.store
        .check_goal_currency(f.project.id, "W", None)
        .unwrap();
}

#[test]
fn a_changed_goal_section_is_rejected_with_both_fingerprints() {
    let mut f = fixture("fp-1", &[("G", "fp-1")], &[("W", "G")]);
    claimed(&mut f, "executor");
    // The goal section was rewritten while the work was in flight.
    recommit(&mut f, |f| ProjectionBatch {
        goals: vec![{
            let mut g = goal(f, "G", "fp-2");
            g.meta.id = existing_id(f, EntityKind::Goal, "G");
            g
        }],
        work_items: vec![{
            let mut w = work(f, "W");
            w.meta.id = existing_id(f, EntityKind::WorkItem, "W");
            w
        }],
        edges: vec![supports(f, "W", "G")],
        ..Default::default()
    });
    let error = f
        .store
        .check_goal_currency(f.project.id, "W", None)
        .unwrap_err();
    let Error::GoalDrift { work, drifts } = &error else {
        panic!("expected GoalDrift, got {error:?}");
    };
    assert_eq!(work, "W");
    assert_eq!(drifts, &[("G".into(), "fp-1".into(), "fp-2".into())]);
    let message = error.to_string();
    assert!(message.contains("fp-1") && message.contains("fp-2"), "{message}");
    assert!(message.contains("reassess"), "{message}");
}

#[test]
fn reclaiming_the_work_refreshes_the_snapshot_and_clears_the_rejection() {
    let mut f = fixture("fp-1", &[("G", "fp-1")], &[("W", "G")]);
    let session = claimed(&mut f, "executor");
    recommit(&mut f, |f| ProjectionBatch {
        goals: vec![{
            let mut g = goal(f, "G", "fp-2");
            g.meta.id = existing_id(f, EntityKind::Goal, "G");
            g
        }],
        work_items: vec![{
            let mut w = work(f, "W");
            w.meta.id = existing_id(f, EntityKind::WorkItem, "W");
            w
        }],
        edges: vec![supports(f, "W", "G")],
        ..Default::default()
    });
    assert!(f.store.check_goal_currency(f.project.id, "W", None).is_err());
    reclaim(&mut f, session);
    f.store
        .check_goal_currency(f.project.id, "W", None)
        .unwrap();
}

#[test]
fn a_removed_goal_reference_is_also_drift() {
    let mut f = fixture("fp-1", &[("G", "fp-1")], &[("W", "G")]);
    claimed(&mut f, "executor");
    // The goal section (and with it the work's reference) disappeared.
    recommit(&mut f, |f| ProjectionBatch {
        work_items: vec![{
            let mut w = work(f, "W");
            w.meta.id = existing_id(f, EntityKind::WorkItem, "W");
            w
        }],
        ..Default::default()
    });
    let error = f
        .store
        .check_goal_currency(f.project.id, "W", None)
        .unwrap_err();
    let Error::GoalDrift { drifts, .. } = &error else {
        panic!("expected GoalDrift, got {error:?}");
    };
    assert_eq!(drifts.len(), 1);
    assert_eq!(drifts[0].0, "G");
}

#[test]
fn works_without_goals_or_without_a_claim_are_never_blocked() {
    let mut f = fixture("fp-1", &[("G", "fp-1")], &[]);
    // No goal reference and no claim yet.
    f.store
        .check_goal_currency(f.project.id, "W", None)
        .unwrap();
    claimed(&mut f, "executor");
    // Still fine after a claim: the snapshot is empty, so nothing can drift.
    f.store
        .check_goal_currency(f.project.id, "W", None)
        .unwrap();
    // A goal that changed but was never referenced by the work stays irrelevant.
    recommit(&mut f, |f| ProjectionBatch {
        work_items: vec![{
            let mut w = work(f, "W");
            w.meta.id = existing_id(f, EntityKind::WorkItem, "W");
            w
        }],
        goals: vec![{
            let mut g = goal(f, "G", "fp-2");
            g.meta.id = existing_id(f, EntityKind::Goal, "G");
            g
        }],
        ..Default::default()
    });
    f.store
        .check_goal_currency(f.project.id, "W", None)
        .unwrap();
}

#[test]
fn the_claim_event_payload_carries_the_fingerprint_map() {
    let mut f = fixture("fp-1", &[("G", "fp-1")], &[("W", "G")]);
    let session = claimed(&mut f, "executor");
    let conn = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    // A claim on session start rides the session.started event.
    let payload: String = conn
        .query_row(
            "SELECT payload_json FROM events WHERE project_id=?1 AND work_item_id IS NOT NULL AND event_type IN ('work.claimed','session.started') ORDER BY project_revision DESC LIMIT 1",
            [f.project.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        value["goal_fingerprints"],
        json!({"G": "fp-1"}),
        "claim payload: {value}"
    );
    let _ = session;
}
