//! `goal_coverage` aggregates work-to-goal supports edges into a read-only
//! coverage view: per-goal work sets with status counts, uncovered goals and
//! orphan work references. The three registered scenarios are pinned here:
//! uncovered goal, orphan work (dangling and dead goal) and normal coverage.

mod support;
use awr_core::*;
use support::Fixture;

fn goal(f: &Fixture, key: &str, status: &str) -> Goal {
    Goal {
        meta: f.meta(key),
        title: format!("Goal {key}"),
        status: status.into(),
        priority: None,
        success_criteria: vec![],
        summary: String::new(),
    }
}

fn work(f: &Fixture, key: &str, status: WorkStatus, archived: bool) -> WorkItem {
    WorkItem {
        archived,
        ordinary_completion: None,
        meta: f.meta(key),
        title: key.into(),
        kind: None,
        owner: None,
        required: true,
        raw_status: "planned".into(),
        status,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: String::new(),
        next_action: String::new(),
        blocker: None,
        acceptance: vec![],
        tags: vec![],
        paths: vec![],
        spec_refs: vec![],
        definition: None,
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
        required: true,
        revision: 1,
        source_ref: f.meta(work_key).source_ref,
    }
}

#[test]
fn uncovered_goal_has_no_live_work_reference() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        goals: vec![
            goal(&f, "G-ALIVE", "active"),
            goal(&f, "G-CONFIRMED", "confirmed"),
        ],
        work_items: vec![work(&f, "W1", WorkStatus::Completed, false)],
        edges: vec![supports(&f, "W1", "G-ALIVE")],
        ..Default::default()
    });
    let coverage = f.store.goal_coverage(f.project.id).unwrap();
    let keys: Vec<_> = coverage
        .uncovered_goal
        .iter()
        .map(|g| g.key.as_str())
        .collect();
    // G-ALIVE is referenced by a completed (still live) work; G-CONFIRMED has none.
    assert_eq!(keys, vec!["G-CONFIRMED"]);
    assert!(coverage.orphan_work.is_empty());
}

#[test]
fn orphan_work_reports_dangling_and_dead_goal_references() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        goals: vec![
            goal(&f, "G-OK", "active"),
            goal(&f, "G-CANCELLED", "cancelled"),
            goal(&f, "G-ARCHIVED", "archived"),
        ],
        work_items: vec![
            work(&f, "W-DANGLING", WorkStatus::Planned, false),
            work(&f, "W-CANCELLED-GOAL", WorkStatus::InProgress, false),
            work(&f, "W-ARCHIVED-GOAL", WorkStatus::Ready, false),
            work(&f, "W-CANCELLED", WorkStatus::Cancelled, false),
            work(&f, "W-ARCHIVED", WorkStatus::Planned, true),
            work(&f, "W-FINE", WorkStatus::InProgress, false),
        ],
        edges: vec![
            supports(&f, "W-DANGLING", "G-NOWHERE"),
            supports(&f, "W-CANCELLED-GOAL", "G-CANCELLED"),
            supports(&f, "W-ARCHIVED-GOAL", "G-ARCHIVED"),
            // Cancelled and archived works are out of the report.
            supports(&f, "W-CANCELLED", "G-NOWHERE"),
            supports(&f, "W-ARCHIVED", "G-NOWHERE"),
            supports(&f, "W-FINE", "G-OK"),
        ],
        ..Default::default()
    });
    let coverage = f.store.goal_coverage(f.project.id).unwrap();
    let mut orphans: Vec<_> = coverage
        .orphan_work
        .iter()
        .map(|o| (o.work.as_str(), o.goal.as_str(), o.reason.as_str()))
        .collect();
    orphans.sort();
    assert_eq!(
        orphans,
        vec![
            ("W-ARCHIVED-GOAL", "G-ARCHIVED", "archived_goal"),
            ("W-CANCELLED-GOAL", "G-CANCELLED", "cancelled_goal"),
            ("W-DANGLING", "G-NOWHERE", "unknown_goal"),
        ]
    );
    // A goal nobody declares stays out of the goal list but is reachable as a
    // dangling reference; unknown statuses never become orphan reasons.
    assert!(
        coverage
            .orphan_work
            .iter()
            .all(|o| o.reason != "unknown_status")
    );
}

#[test]
fn normal_coverage_lists_works_counts_and_stays_silent() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        goals: vec![goal(&f, "G-1", "active"), goal(&f, "G-OFF", "cancelled")],
        work_items: vec![
            work(&f, "W1", WorkStatus::Completed, false),
            work(&f, "W2", WorkStatus::Blocked, false),
            work(&f, "W3", WorkStatus::Cancelled, false),
        ],
        edges: vec![
            supports(&f, "W1", "G-1"),
            supports(&f, "W2", "G-1"),
            // A cancelled work still shows in the goal's work set but does not
            // count as coverage; the goal also keeps a live reference via W1/W2.
            supports(&f, "W3", "G-1"),
        ],
        ..Default::default()
    });
    let coverage = f.store.goal_coverage(f.project.id).unwrap();
    assert!(coverage.uncovered_goal.is_empty());
    assert!(coverage.orphan_work.is_empty());
    assert_eq!(coverage.goals.len(), 2, "every declared goal is listed");
    let g1 = coverage
        .goals
        .iter()
        .find(|g| g.key == "G-1")
        .expect("G-1 listed");
    assert_eq!(g1.works.len(), 3);
    assert_eq!(g1.status_counts.get("completed"), Some(&1));
    assert_eq!(g1.status_counts.get("blocked"), Some(&1));
    assert_eq!(g1.status_counts.get("cancelled"), Some(&1));
    assert!(
        g1.source_pointer.as_deref() == Some("/G-1"),
        "goal entry carries its source pointer, got {:?}",
        g1.source_pointer
    );
    let work_ref = g1.works.iter().find(|w| w.key == "W2").unwrap();
    assert_eq!(work_ref.status, "blocked");
}
