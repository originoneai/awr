mod support;
use awr_core::*;
use std::path::Path;
use support::Fixture;

/// HEAD sha of a real git repository (used by the real-worktree C1m test).
fn git_head(repo: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .expect("git must be available on PATH");
    assert!(out.status.success(), "git rev-parse failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

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

fn ledger_snapshot(f: &mut Fixture, sha: &str, section: &str) {
    let mut item = sec_work(f, "WA", section);
    if let Ok(previous) = f.store.work_item(f.project.id, "WA") {
        item.meta.id = previous.item.meta.id;
    }
    let fingerprint = format!("sha256:{sha}");
    item.meta.source_ref.source_fingerprint = fingerprint.clone();
    f.source = f
        .store
        .commit_source_projection(
            &f.source,
            &fingerprint,
            ProjectionBatch {
                work_items: vec![item],
                ..Default::default()
            },
        )
        .unwrap();
}

#[test]
fn ledger_snapshot_evidence_reuses_only_an_unchanged_stamped_contract() {
    let mut f = Fixture::new();
    let before = "a".repeat(64);
    let after = "b".repeat(64);
    ledger_snapshot(&mut f, &before, "sec-A");
    for (key, sha) in [("ledger", before.clone()), ("code", "c".repeat(40))] {
        let mut d = draft(key);
        d.work_item_key = Some("WA".into());
        d.source_sha = Some(sha);
        let rev = f.store.project(f.project.id).unwrap().project_revision;
        let (recorded, _) = f.store.record_evidence(f.project.id, rev, d).unwrap();
        assert_eq!(recorded.section_fingerprint.is_some(), key == "ledger");
    }
    // A refreshed ledger snapshot changes, while this task's contract stays put.
    ledger_snapshot(&mut f, &after, "sec-A");
    let assessments = f
        .store
        .evidence_for_work(f.project.id, "WA", Some(&after), None)
        .unwrap();
    let ledger = assessments
        .iter()
        .find(|a| a.evidence.item.external_key == "ledger")
        .unwrap();
    assert_eq!(ledger.currency, EvidenceCurrency::Current);
    assert!(ledger.missing_bindings.is_empty());
    assert_eq!(
        ledger.evidence.item.source_sha.as_deref(),
        Some(before.as_str())
    );
    assert_eq!(
        assessments
            .iter()
            .find(|a| a.evidence.item.external_key == "code")
            .unwrap()
            .currency,
        EvidenceCurrency::Historical
    );
    // A matching contract cannot bless an arbitrary requested code revision,
    // a different branch, or a generic read without the work source binding.
    for (requested, branch) in [(&"d".repeat(64), None), (&after, Some(Id::new()))] {
        assert!(
            f.store
                .evidence_for_work(f.project.id, "WA", Some(requested), branch)
                .unwrap()
                .iter()
                .all(|a| a.currency == EvidenceCurrency::Historical)
        );
    }
    assert_eq!(
        ledger
            .evidence
            .clone()
            .assess(Some(&after), Some("sec-A"), None)
            .currency,
        EvidenceCurrency::Historical
    );
    f.source = f
        .store
        .mark_source_freshness(&f.source, Freshness::Stale)
        .unwrap();
    let stale = f
        .store
        .evidence_for_work(f.project.id, "WA", Some(&after), None)
        .unwrap();
    assert_eq!(
        stale
            .iter()
            .find(|a| a.evidence.item.external_key == "ledger")
            .unwrap()
            .currency,
        EvidenceCurrency::Unknown
    );
    // Contract changes invalidate even a query for the original verification SHA.
    ledger_snapshot(&mut f, &"e".repeat(64), "sec-A-edited");
    for requested in [&before, &"e".repeat(64)] {
        assert!(
            f.store
                .evidence_for_work(f.project.id, "WA", Some(requested), None)
                .unwrap()
                .iter()
                .all(|a| a.currency == EvidenceCurrency::Historical)
        );
    }
}

#[test]
fn stale_projection_cannot_stamp_new_evidence_for_later_reuse() {
    let mut f = Fixture::new();
    let before = "a".repeat(64);
    let after = "b".repeat(64);
    ledger_snapshot(&mut f, &before, "sec-A");
    f.source = f
        .store
        .mark_source_freshness(&f.source, Freshness::Stale)
        .unwrap();
    let mut d = draft("stale-snapshot");
    d.work_item_key = Some("WA".into());
    d.source_sha = Some(before);
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    let (recorded, _) = f.store.record_evidence(f.project.id, rev, d).unwrap();
    assert!(recorded.section_fingerprint.is_none());
    // A later fresh projection must not promote the record by inferring that it
    // was registered against a verified ledger snapshot.
    ledger_snapshot(&mut f, &after, "sec-A");
    assert_eq!(
        f.store
            .evidence_for_work(f.project.id, "WA", Some(&after), None)
            .unwrap()[0]
            .currency,
        EvidenceCurrency::Historical
    );
}

#[test]
fn importing_old_ledger_evidence_does_not_stamp_the_current_contract() {
    let mut f = Fixture::new();
    let current = "b".repeat(64);
    ledger_snapshot(&mut f, &current, "sec-A");
    let mut d = draft("old-ledger");
    d.work_item_key = Some("WA".into());
    d.source_sha = Some("a".repeat(64));
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    let (recorded, _) = f.store.record_evidence(f.project.id, rev, d).unwrap();
    assert!(recorded.section_fingerprint.is_none());
    assert_eq!(
        f.store
            .evidence_for_work(f.project.id, "WA", Some(&current), None)
            .unwrap()[0]
            .currency,
        EvidenceCurrency::Historical
    );
    assert!(
        f.store
            .evidence(f.project.id, "old-ledger")
            .unwrap()
            .item
            .section_fingerprint
            .is_none()
    );
}

/// Git/code revision evidence has no ledger stamp. A contract match alone must
/// never allow it to cross an explicitly requested verification revision.
#[test]
fn evidence_is_bound_to_the_source_revision_it_was_recorded_against() {
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

    // At the revision the evidence was recorded against, it is Current.
    for key in ["WA", "WB"] {
        let evs = f
            .store
            .evidence_for_work(f.project.id, key, Some(&sha_before), None)
            .unwrap();
        assert_eq!(
            evs.len(),
            1,
            "{key} evidence must be linked to its work item"
        );
        assert!(
            evs.iter().all(|a| a.currency == EvidenceCurrency::Current),
            "{key} evidence must be Current at its own source revision"
        );
    }

    // The requested verification revision advances, without a ledger binding.
    let sha_after = "c".repeat(40);
    for key in ["WA", "WB"] {
        let evs = f
            .store
            .evidence_for_work(f.project.id, key, Some(&sha_after), None)
            .unwrap();
        assert_eq!(
            evs.len(),
            1,
            "{key} evidence must be linked to its work item"
        );
        assert!(
            evs.iter()
                .all(|a| a.currency == EvidenceCurrency::Historical),
            "{key} evidence recorded at an older source revision must be Historical, not Current"
        );
    }
}

/// Generic assessment lacks a current ledger binding, so a section fingerprint
/// that still matches cannot override a source-SHA mismatch.
#[test]
fn source_sha_mismatch_beats_a_matching_section_fingerprint() {
    let mut f = Fixture::new();
    f.commit(ProjectionBatch {
        work_items: vec![sec_work(&f, "WA", "sec-A")],
        ..Default::default()
    });
    let sha = "a".repeat(40);
    let mut d = draft("wa-report");
    d.work_item_key = Some("WA".into());
    d.source_sha = Some(sha.clone());
    let rev = f.store.project(f.project.id).unwrap().project_revision;
    f.store.record_evidence(f.project.id, rev, d).unwrap();

    let mut rec = f.store.evidence(f.project.id, "wa-report").unwrap();
    rec.item.section_fingerprint = Some("sec-A".into());
    assert_eq!(
        rec.clone().assess(Some(&sha), Some("sec-A"), None).currency,
        EvidenceCurrency::Current
    );
    // Same revision, edited contract -> Historical.
    assert_eq!(
        rec.clone()
            .assess(Some(&sha), Some("sec-A-edited"), None)
            .currency,
        EvidenceCurrency::Historical
    );
    // Older revision, identical contract -> Historical (never Current).
    assert_eq!(
        rec.assess(Some(&"d".repeat(40)), Some("sec-A"), None)
            .currency,
        EvidenceCurrency::Historical
    );
}

/// Git/code revision evidence transported between nodes has no ledger-snapshot
/// stamp. Identical task sections cannot establish that it verified a different
/// requested code revision.
#[test]
fn shared_ledger_cross_node_completion_marks_pulled_evidence_historical() {
    // Node A and Node B have identical sections for WA/WB, but request different
    // verification revisions rather than the ledger-snapshot fingerprints.
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
    let rev = node_a
        .store
        .project(node_a.project.id)
        .unwrap()
        .project_revision;
    node_a
        .store
        .record_evidence(node_a.project.id, rev, wa_report.clone())
        .unwrap();

    // Node B pulls WA's evidence (shared ledger) and completes WB, advancing its
    // own requested revision to sha_b (≠ sha_a). WA's evidence was recorded at sha_a,
    // so from Node B's view it is Historical until re-registered here.
    let sha_b = "b".repeat(40);
    let mut pulled = draft("wa-report");
    pulled.work_item_key = Some("WA".into());
    pulled.source_sha = Some(sha_a.clone());
    let rev = node_b
        .store
        .project(node_b.project.id)
        .unwrap()
        .project_revision;
    node_b
        .store
        .record_evidence(node_b.project.id, rev, pulled)
        .unwrap();
    let evs = node_b
        .store
        .evidence_for_work(node_b.project.id, "WA", Some(&sha_b), None)
        .unwrap();
    assert_eq!(evs.len(), 1, "pulled evidence must stay linked to WA");
    assert!(
        evs.iter()
            .all(|a| a.currency == EvidenceCurrency::Historical),
        "evidence recorded at another node's revision must not be Current here"
    );

    // Re-verifying at this node's revision restores Current.
    let mut revalidated = draft("wa-report-v2");
    revalidated.work_item_key = Some("WA".into());
    revalidated.source_sha = Some(sha_b.clone());
    let rev = node_b
        .store
        .project(node_b.project.id)
        .unwrap()
        .project_revision;
    node_b
        .store
        .record_evidence(node_b.project.id, rev, revalidated)
        .unwrap();
    let evs = node_b
        .store
        .evidence_for_work(node_b.project.id, "WA", Some(&sha_b), None)
        .unwrap();
    assert!(
        evs.iter().any(|a| a.currency == EvidenceCurrency::Current
            && a.evidence.item.external_key == "wa-report-v2"),
        "a record re-registered at this node's revision must be Current"
    );
}

/// C1m (real-environment): two *real* git worktrees of one shared ledger repo,
/// each holding an independent SQLite store. Node A completes item WA on its own
/// worktree (a real `git commit`), Node B fast-forwards to that commit and then
/// completes item WB on its copy. Each node therefore sees a different Git
/// revision, and records bound to an earlier code revision must be
/// Historical rather than silently Current; re-verifying at the current revision
/// restores Current.
///
/// This reproduces shared-ledger multi-agent collaboration over a real Git
/// transport, not merely two in-process fixtures.
#[test]
fn real_git_worktree_transport_marks_outdated_evidence_historical() {
    fn git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git must be available on PATH");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let base = std::env::temp_dir().join(format!("awr-c1m-real-{}", Id::new()));
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    // A shared ledger file keyed by Git commit, not its source fingerprint:
    // completion changes the repository revision, while the task contract stays put.
    // write_ledger writes into a specific worktree's own copy.
    let write_ledger = |dir: &Path, wa: bool, wb: bool| {
        std::fs::write(
            dir.join("work.yaml"),
            format!(
                "work_items:\n- id: WA\n  title: A\n  status: {}\n  acceptance: [render A]\n- id: WB\n  title: B\n  status: {}\n  acceptance: [render B]\n",
                if wa { "completed" } else { "planned" },
                if wb { "completed" } else { "planned" },
            ),
        )
        .unwrap();
    };
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.email=node@test",
            "-c",
            "user.name=node",
            "config",
            "--local",
            "user.email",
            "node@test",
        ],
    );
    git(&repo, &["config", "--local", "user.name", "node"]);
    write_ledger(&repo, false, false);
    git(&repo, &["add", "work.yaml"]);
    git(&repo, &["commit", "-q", "-m", "both planned"]);
    let sha_both_planned = git_head(&repo);

    // Two real worktrees of the shared repo, on independent branches (each node
    // completes its own ticket without colliding on the same branch).
    let node_a = base.join("node-a");
    let node_b = base.join("node-b");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "node-a",
            node_a.to_str().unwrap(),
            "main",
        ],
    );
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "node-b",
            node_b.to_str().unwrap(),
            "main",
        ],
    );

    // ---- Node A: store over worktree A, complete WA (real commit). ----
    let mut f_a = Fixture::new_under(&node_a, "file:///node-a/work.yaml");
    f_a.commit(ProjectionBatch {
        work_items: vec![sec_work(&f_a, "WA", "sec-A"), sec_work(&f_a, "WB", "sec-B")],
        ..Default::default()
    });
    let mut wa_ev = draft("wa-report");
    wa_ev.work_item_key = Some("WA".into());
    wa_ev.source_sha = Some(sha_both_planned.clone());
    let r = f_a.store.project(f_a.project.id).unwrap().project_revision;
    f_a.store.record_evidence(f_a.project.id, r, wa_ev).unwrap();
    write_ledger(&node_a, true, false); // node A completes WA
    git(&node_a, &["add", "work.yaml"]);
    git(&node_a, &["commit", "-q", "-m", "A completes WA"]);

    // ---- Node B fast-forwards to A's completed ledger, then completes WB. ----
    let sha_after_a = git_head(&node_a);
    // Two real worktrees share one git object store (the shared ledger): Node B
    // moves to A's latest commit through the real git transport, so its
    // Git revision now differs from the SHA Node A bound WA's evidence at.
    git(&node_b, &["reset", "-q", "--hard", &sha_after_a]);
    let sha_after_a = git_head(&node_b);
    assert_ne!(
        sha_after_a, sha_both_planned,
        "A's completion changed the whole file"
    );
    let mut f_b = Fixture::new_under(&node_b, "file:///node-b/work.yaml");
    f_b.commit(ProjectionBatch {
        work_items: vec![sec_work(&f_b, "WA", "sec-A"), sec_work(&f_b, "WB", "sec-B")],
        ..Default::default()
    });
    // Node B registers WA's shared report at *its* revision (sha_after_a) and
    // completes WB, which commits again and moves the ledger SHA to
    // sha_after_both.
    let mut wa_shared = draft("wa-report");
    wa_shared.work_item_key = Some("WA".into());
    wa_shared.source_sha = Some(sha_after_a.clone());
    let r = f_b.store.project(f_b.project.id).unwrap().project_revision;
    f_b.store
        .record_evidence(f_b.project.id, r, wa_shared)
        .unwrap();
    let mut wb_ev = draft("wb-report");
    wb_ev.work_item_key = Some("WB".into());
    wb_ev.source_sha = Some(sha_after_a.clone());
    let r = f_b.store.project(f_b.project.id).unwrap().project_revision;
    f_b.store.record_evidence(f_b.project.id, r, wb_ev).unwrap();
    write_ledger(&node_b, true, true); // node B completes WB
    git(&node_b, &["add", "work.yaml"]);
    git(&node_b, &["commit", "-q", "-m", "B completes WB"]);
    let sha_after_both = git_head(&node_b);
    assert_ne!(sha_after_both, sha_after_a);

    // Cross-node assertion: from Node B's final shared-ledger state, the records
    // that were bound to the earlier revision are Historical, because the real
    // transport moved the source out from under them.
    for (key, section) in [("WA", "sec-A"), ("WB", "sec-B")] {
        let _ = section;
        let evs = f_b
            .store
            .evidence_for_work(f_b.project.id, key, Some(&sha_after_both), None)
            .unwrap();
        assert!(
            !evs.is_empty(),
            "{key} evidence must stay linked over the real transport"
        );
        assert!(
            evs.iter().all(|a| a.currency != EvidenceCurrency::Current),
            "real-git transport left {key} evidence looking Current after the ledger moved",
        );
    }

    // Re-verifying both items at the final revision makes them Current again.
    for (key, report) in [("WA", "wa-report-v2"), ("WB", "wb-report-v2")] {
        let mut d = draft(report);
        d.work_item_key = Some(key.into());
        d.source_sha = Some(sha_after_both.clone());
        let r = f_b.store.project(f_b.project.id).unwrap().project_revision;
        f_b.store.record_evidence(f_b.project.id, r, d).unwrap();
        let evs = f_b
            .store
            .evidence_for_work(f_b.project.id, key, Some(&sha_after_both), None)
            .unwrap();
        assert!(
            evs.iter().any(|a| a.currency == EvidenceCurrency::Current
                && a.evidence.item.external_key == report),
            "{key} evidence re-verified at the current revision must be Current"
        );
    }
    let _ = std::fs::remove_dir_all(&base);
}
