mod support;
use awr_core::*;
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
        raw_status: "planned".into(),
        status: WorkStatus::Planned,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: Some(EvidenceLevel::Designed),
        summary: "".into(),
        next_action: "build".into(),
        blocker: None,
        acceptance: vec![],
        tags: vec![],
        paths: vec!["crates/core/lib.rs".into()],
    }
}
fn draft(key: &str) -> EvidenceDraft {
    EvidenceDraft {
        external_key: key.into(),
        work_item_key: Some("W".into()),
        evidence_type: "command_report".into(),
        level: EvidenceLevel::LocallyVerified,
        summary: "Feature check passed".into(),
        locator: "reports/check.json".into(),
        sha256: Some("a".repeat(64)),
        source_sha: Some("b".repeat(40)),
        command: Some("cargo test -p example".into()),
        scope: vec!["W".into()],
        branch_id: None,
        verified_at: Some(100),
    }
}

#[test]
fn grouping_a_shared_locator_preserves_each_records_currency_and_missing_bindings() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![work(&f, "W")],
        ..Default::default()
    });
    let mut weak = draft("reference");
    weak.evidence_type = "source_reference".into();
    weak.level = EvidenceLevel::Unknown;
    weak.sha256 = None;
    weak.source_sha = None;
    weak.command = None;
    weak.verified_at = None;
    let mut old = draft("old-version");
    old.source_sha = Some("c".repeat(40));
    for input in [draft("current"), weak, old] {
        let rev = f.store.project(f.project.id).unwrap().project_revision;
        f.store.record_evidence(f.project.id, rev, input).unwrap();
    }
    let evidence = f
        .store
        .evidence_for_work(f.project.id, "W", Some(&"b".repeat(40)), None)
        .unwrap();
    let groups = evidence_groups(&evidence);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["locator"], "reports/check.json");
    let records = groups[0]["records"].as_array().unwrap();
    assert_eq!(records.len(), 3);
    let get = |key: &str| records.iter().find(|v| v["external_key"] == key).unwrap();
    assert_eq!(get("current")["currency"], "current");
    assert_eq!(get("reference")["level"], "unknown");
    assert_eq!(get("reference")["currency"], "unknown");
    assert!(get("reference")["sha256"].is_null());
    assert_eq!(
        get("reference")["missing_bindings"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(get("old-version")["currency"], "historical");
}

#[test]
fn decisions_require_acceptance_and_explicit_relevance_or_unknown_scope() {
    let mut f = Fixture::new();
    let mut batch = ProjectionBatch {
        work_items: vec![work(&f, "W")],
        ..Default::default()
    };
    for (key, status, keys, paths) in [
        ("accepted", DecisionStatus::Accepted, vec!["W"], vec![]),
        ("path", DecisionStatus::Accepted, vec![], vec!["crates/**"]),
        ("unscoped", DecisionStatus::Accepted, vec![], vec![]),
        ("unrelated", DecisionStatus::Accepted, vec!["OTHER"], vec![]),
        ("proposal", DecisionStatus::Proposed, vec!["W"], vec![]),
        ("superseded", DecisionStatus::Superseded, vec!["W"], vec![]),
    ] {
        batch.decisions.push(Decision {
            adoption: None,
            superseded_by: None,
            meta: f.meta(key),
            title: key.into(),
            status,
            raw_status: format!("{status:?}"),
            decision: "Choice".into(),
            rationale: "Reason".into(),
            affected_keys: keys.into_iter().map(String::from).collect(),
            paths: paths.into_iter().map(String::from).collect(),
        });
    }
    f.commit(batch);
    assert_eq!(f.store.decisions(f.project.id).unwrap().len(), 6);
    assert_eq!(
        f.store
            .decision(f.project.id, "proposal")
            .unwrap()
            .item
            .status,
        DecisionStatus::Proposed
    );
    let relevant = f.store.decisions_for_work(f.project.id, "W").unwrap();
    assert_eq!(relevant.len(), 3);
    assert_eq!(
        relevant
            .iter()
            .filter(|d| d.relevance == Applicability::Applicable)
            .count(),
        2
    );
    assert_eq!(
        relevant
            .iter()
            .find(|d| d.decision.item.meta.external_key == "unscoped")
            .unwrap()
            .relevance,
        Applicability::Unknown
    );
}

#[test]
fn unknown_work_paths_preserve_potential_decisions_and_explicit_scope_can_resolve_them() {
    let mut f = Fixture::new();
    let mut task = work(&f, "W");
    task.paths = vec!["crates/".into()];
    let decision = Decision {
        adoption: None,
        superseded_by: None,
        meta: f.meta("path-choice"),
        title: "Scoped choice".into(),
        status: DecisionStatus::Accepted,
        raw_status: "accepted".into(),
        decision: "Apply this source policy".into(),
        rationale: "Background".into(),
        affected_keys: vec![],
        paths: vec!["crates/core/**".into()],
    };
    f.commit(ProjectionBatch {
        work_items: vec![task],
        decisions: vec![decision],
        ..Default::default()
    });
    let unknown = f.store.decisions_for_work(f.project.id, "W").unwrap();
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].relevance, Applicability::Unknown);
    let paths = vec!["crates/core/lib.rs".into()];
    let selected = f
        .store
        .decisions_for_work_with_paths(f.project.id, "W", Some(&paths))
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].relevance, Applicability::Applicable);
    assert!(
        f.store
            .decisions_for_work_with_paths(f.project.id, "W", Some(&[]))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn evidence_is_explicit_version_bound_and_append_only_by_key() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![work(&f, "W")],
        ..Default::default()
    });
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    let event = f
        .store
        .append_event(
            f.project.id,
            rev,
            EventDraft::new("test_passed", "A test passed"),
        )
        .unwrap();
    assert!(
        f.store
            .evidence_for_work(f.project.id, "W", None, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.store
            .work_item(f.project.id, "W")
            .unwrap()
            .item
            .evidence_level,
        Some(EvidenceLevel::Designed)
    );
    let mut missing = draft("missing");
    missing.command = None;
    assert!(matches!(
        f.store
            .record_evidence(f.project.id, event.project_revision, missing),
        Err(Error::EvidenceMissing(_))
    ));
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        event.project_revision
    );
    assert!(matches!(
        f.store.record_evidence(f.project.id, rev, draft("stale")),
        Err(Error::RevisionConflict { .. })
    ));
    let (record, recorded) = f
        .store
        .record_evidence(f.project.id, event.project_revision, draft("E-1"))
        .unwrap();
    assert_eq!(recorded.event_type, "evidence.recorded");
    assert_eq!(
        record.work_item_id,
        f.store
            .work_item(f.project.id, "W")
            .ok()
            .map(|w| w.item.meta.id)
    );
    let current = f
        .store
        .evidence_for_work(f.project.id, "W", Some(&"b".repeat(40)), None)
        .unwrap();
    assert_eq!(current[0].currency, EvidenceCurrency::Current);
    assert!(current[0].missing_bindings.is_empty());
    assert_eq!(
        current[0].evidence.item.level,
        EvidenceLevel::LocallyVerified
    );
    assert_eq!(
        f.store
            .evidence_for_work(f.project.id, "W", Some(&"c".repeat(40)), None)
            .unwrap()[0]
            .currency,
        EvidenceCurrency::Historical
    );
    assert_eq!(
        f.store
            .evidence_for_work(f.project.id, "W", None, None)
            .unwrap()[0]
            .currency,
        EvidenceCurrency::Unknown
    );
    assert_eq!(
        f.store
            .evidence_for_work(f.project.id, "W", Some(&"b".repeat(40)), Some(Id::new()))
            .unwrap()[0]
            .currency,
        EvidenceCurrency::Historical
    );
    assert!(matches!(
        f.store
            .record_evidence(f.project.id, recorded.project_revision, draft("E-1")),
        Err(Error::SourceConflict(_))
    ));
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        recorded.project_revision
    );
    let mut old = draft("E-2");
    old.source_sha = Some("d".repeat(40));
    f.store
        .record_evidence(f.project.id, recorded.project_revision, old)
        .unwrap();
    assert_eq!(f.store.evidence_records(f.project.id).unwrap().len(), 2);
    assert_eq!(
        f.store
            .evidence(f.project.id, "E-1")
            .unwrap()
            .item
            .source_sha,
        Some("b".repeat(40))
    );
    assert_eq!(
        f.store
            .work_item(f.project.id, "W")
            .unwrap()
            .item
            .evidence_level,
        Some(EvidenceLevel::Designed)
    );
    assert!(f.store.doctor().unwrap().ok);
}

/// A work item whose `source_ref` carries an identity+acceptance section
/// fingerprint, as produced by the yaml-ledger adapter (A2a).
fn sec_work(f: &Fixture, key: &str, section: &str) -> WorkItem {
    let mut w = work(f, key);
    w.meta.source_ref.section_fingerprint = Some(section.into());
    w
}

/// C1: completing one work item in a single ledger file must not roll back the
/// verified evidence of the other items (proves iron-rule #2 is fixed).
///
/// WB's completion rewrites the *whole file* (its source SHA changes), but WB's
/// and WA's identity+acceptance sections are untouched, so both stay Current.
#[test]
fn completing_one_work_item_keeps_other_items_evidence_current() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![sec_work(&f, "WA", "sec-A"), sec_work(&f, "WB", "sec-B")],
        ..Default::default()
    });
    let sha_before = "b".repeat(40);
    for key in ["WA", "WB"] {
        let mut d = draft(&format!("{key}-report"));
        d.work_item_key = Some(key.into());
        d.source_sha = Some(sha_before.clone());
        let rev = f.store.project(f.project.id).unwrap().project_revision;
        f.store.record_evidence(f.project.id, rev, d).unwrap();
    }
    // The whole-file SHA advances after WB is completed (status written by AWR).
    let sha_after = "c".repeat(40);
    for key in ["WA", "WB"] {
        let evs = f
            .store
            .evidence_for_work(f.project.id, key, Some(&sha_after), None)
            .unwrap();
        assert!(
            evs.iter().all(|a| a.currency == EvidenceCurrency::Current),
            "{key} evidence must stay Current when the whole-file SHA changes (section isolation)"
        );
    }
}

/// C1m: shared-ledger multi-node isolation. Two logical nodes share one git
/// ledger, each completing a different ticket on its own copy, so each node
/// observes a different whole-file SHA. A node's own completion must not drop
/// the other node's evidence from Current to Historical.
#[test]
fn shared_ledger_cross_node_completions_do_not_cross_invalidate() {
    // Node A and Node B have identical sections for WA/WB, but each copy's
    // whole-file SHA differs because each node completed a different ticket.
    let mut node_a = Fixture::new();
    node_a.commit(ProjectionBatch {
        work_items: vec![
            sec_work(&node_a, "WA", "sec-A"),
            sec_work(&node_a, "WB", "sec-B"),
        ],
        ..Default::default()
    });
    let mut node_b = Fixture::new();
    node_b.commit(ProjectionBatch {
        work_items: vec![
            sec_work(&node_b, "WA", "sec-A"),
            sec_work(&node_b, "WB", "sec-B"),
        ],
        ..Default::default()
    });

    // Node A completes WA against its own file SHA.
    let sha_a = "a".repeat(40);
    let mut wa_report = draft("wa-report");
    wa_report.work_item_key = Some("WA".into());
    wa_report.source_sha = Some(sha_a.clone());
    let rev = node_a.store.project(node_a.project.id).unwrap().project_revision;
    node_a
        .store
        .record_evidence(node_a.project.id, rev, wa_report.clone())
        .unwrap();

    // Node B pulls WA's evidence (shared ledger) and completes WB, advancing its
    // own whole-file SHA to sha_b (≠ sha_a). WA's identity+acceptance section is
    // unchanged, so its evidence must remain Current under sha_b.
    let sha_b = "b".repeat(40);
    let mut pulled = draft("wa-report");
    pulled.work_item_key = Some("WA".into());
    pulled.source_sha = Some(sha_a.clone());
    let rev = node_b.store.project(node_b.project.id).unwrap().project_revision;
    node_b
        .store
        .record_evidence(node_b.project.id, rev, pulled)
        .unwrap();
    let evs = node_b
        .store
        .evidence_for_work(node_b.project.id, "WA", Some(&sha_b), None)
        .unwrap();
    assert!(
        evs.iter().all(|a| a.currency == EvidenceCurrency::Current),
        "cross-node completion must not drop the other node's evidence to Historical"
    );
}
