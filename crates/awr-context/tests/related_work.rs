#[path = "../../awr-store/tests/support/mod.rs"]
mod support;
use awr_context::related_work;
use awr_core::*;
use support::Fixture;

fn work(f: &Fixture, key: &str, status: WorkStatus) -> WorkItem {
    WorkItem {
        spec_refs: vec![],
        ordinary_completion: None,
        archived: false,
        meta: f.meta(key),
        title: key.into(),
        kind: None,
        owner: None,
        required: true,
        raw_status: serde_json::to_value(status)
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
        status,
        priority: None,
        milestone: Some("M4".into()),
        score: None,
        evidence_level: None,
        summary: "UNRELATED_WORK_SUMMARY".into(),
        next_action: format!("Next for {key}"),
        blocker: if status == WorkStatus::Blocked {
            Some("Await input".into())
        } else {
            None
        },
        acceptance: vec!["Deliver".into()],
        tags: vec![],
        paths: vec!["src/main.rs".into()],
    }
}
fn fixture() -> Fixture {
    let mut f = Fixture::new();
    let mut batch = ProjectionBatch::default();
    for (key, status) in [
        ("W", WorkStatus::Planned),
        ("BASE", WorkStatus::Completed),
        ("DEP", WorkStatus::Blocked),
        ("A", WorkStatus::InProgress),
        ("B", WorkStatus::Completed),
        ("OPTIONAL", WorkStatus::Blocked),
    ] {
        batch.work_items.push(work(&f, key, status));
    }
    for (from, to, required) in [
        ("W", "BASE", true),
        ("W", "DEP", true),
        ("DEP", "BASE", true),
        ("W", "MISSING", true),
        ("W", "A", true),
        ("A", "B", true),
        ("B", "A", true),
        ("W", "OPTIONAL", false),
    ] {
        batch.edges.push(Edge {
            id: Id::new(),
            project_id: f.project.id,
            from_kind: EntityKind::WorkItem,
            from_key: from.into(),
            relation: "depends_on".into(),
            to_kind: EntityKind::WorkItem,
            to_key: to.into(),
            required,
            revision: 1,
            source_ref: f.meta(from).source_ref,
        });
    }
    for (key, status, affected) in [
        ("ACCEPTED", DecisionStatus::Accepted, vec!["W"]),
        ("UNSCOPED", DecisionStatus::Accepted, vec![]),
        ("PROPOSED", DecisionStatus::Proposed, vec!["W"]),
        ("OLD", DecisionStatus::Superseded, vec!["W"]),
        ("UNRELATED", DecisionStatus::Accepted, vec!["OTHER"]),
    ] {
        batch.decisions.push(Decision {
            specification: None,
            adoption: None,
            adoption_valid: None,
            superseded_by: None,
            meta: f.meta(key),
            title: key.into(),
            status,
            raw_status: format!("{status:?}"),
            decision: format!("Statement {key}"),
            rationale: "PRIVATE_RATIONALE_SENTINEL".into(),
            affected_keys: affected.into_iter().map(str::to_owned).collect(),
            paths: vec![],
        });
    }
    f.commit(batch);
    f
}
fn evidence(f: &mut Fixture, key: &str, verified: bool) {
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    f.store
        .record_evidence(
            f.project.id,
            rev,
            EvidenceDraft {
                external_key: key.into(),
                work_item_key: Some("W".into()),
                evidence_type: "report".into(),
                level: if verified {
                    EvidenceLevel::LocallyVerified
                } else {
                    EvidenceLevel::Implemented
                },
                summary: "A bounded evidence summary. ".repeat(40),
                locator: "report-does-not-exist.txt".into(),
                sha256: verified.then(|| "a".repeat(64)),
                source_sha: verified.then(|| "a".repeat(40)),
                command: verified.then(|| "verify feature".into()),
                scope: vec!["W".into()],
                branch_id: None,
                verified_at: verified.then_some(1),
            },
        )
        .unwrap();
}

#[test]
fn related_selection_preserves_required_gaps_without_history_or_rationale_bodies() {
    let mut f = fixture();
    evidence(&mut f, "OLD-REPORT", true);
    evidence(&mut f, "PARTIAL", false);
    let context = related_work(
        &f.store,
        f.project.id,
        "W",
        None,
        Some(&"b".repeat(40)),
        None,
    )
    .unwrap();
    assert_eq!(
        context
            .unresolved_dependencies
            .iter()
            .map(|d| d.meta.external_key.as_str())
            .collect::<Vec<_>>(),
        ["A", "DEP"]
    );
    assert_eq!(
        context
            .resolved_dependencies
            .iter()
            .filter(|d| d.meta.external_key == "BASE")
            .count(),
        1
    );
    assert_eq!(context.missing_dependencies, ["MISSING"]);
    assert_eq!(context.dependency_cycles, ["A", "B"]);
    assert!(
        context
            .unresolved_dependencies
            .iter()
            .all(|d| !d.next_action.is_empty())
    );
    assert!(context.required_edges.iter().all(|e| e.required));
    assert_eq!(context.accepted_decisions.len(), 1);
    assert_eq!(
        context.accepted_decisions[0].statement,
        "Statement ACCEPTED"
    );
    assert_eq!(context.accepted_decisions[0].raw_status, "Accepted");
    assert_eq!(context.accepted_decisions[0].affected_keys, ["W"]);
    assert_eq!(context.uncertain_decisions[0].meta.external_key, "UNSCOPED");
    assert_eq!(context.evidence.len(), 2);
    assert!(
        context
            .evidence
            .iter()
            .all(|e| e.summary.chars().count() <= 241)
    );
    assert!(
        context
            .evidence_gaps
            .iter()
            .any(|g| g.code == "historical_evidence")
    );
    assert!(
        context
            .evidence_gaps
            .iter()
            .any(|g| g.code == "missing_evidence_bindings")
    );
    assert!(
        context
            .evidence_gaps
            .iter()
            .any(|g| g.code == "evidence_level_unverified")
    );
    let encoded = serde_json::to_string(&context).unwrap();
    assert!(!encoded.contains("PRIVATE_RATIONALE_SENTINEL"));
    assert!(!encoded.contains("UNRELATED_WORK_SUMMARY"));
    assert!(!encoded.contains("OPTIONAL"));
    assert!(!encoded.contains("Statement PROPOSED"));
    assert!(!encoded.contains("Statement OLD"));
    assert_eq!(
        encoded,
        serde_json::to_string(
            &related_work(
                &f.store,
                f.project.id,
                "W",
                None,
                Some(&"b".repeat(40)),
                None
            )
            .unwrap()
        )
        .unwrap()
    );
    assert!(f.store.work_item(f.project.id, "W").unwrap().item.status == WorkStatus::Planned);
}

#[test]
fn stale_completion_facts_stay_unresolved_and_missing_evidence_is_reported() {
    let mut f = fixture();
    f.source = f
        .store
        .mark_source_freshness(&f.source, Freshness::Unavailable)
        .unwrap();
    let context = related_work(&f.store, f.project.id, "W", None, None, None).unwrap();
    assert!(context.resolved_dependencies.is_empty());
    assert!(
        context
            .unresolved_dependencies
            .iter()
            .any(|d| d.meta.external_key == "BASE"
                && d.status == WorkStatus::Completed
                && d.freshness == Freshness::Unavailable)
    );
    assert!(!context.source_issues.is_empty());
    assert!(context.accepted_decisions.is_empty());
    assert!(!context.uncertain_decisions.is_empty());
    assert_eq!(context.evidence_gaps[0].code, "no_evidence");
}
