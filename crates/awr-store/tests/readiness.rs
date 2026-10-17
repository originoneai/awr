mod support;
use awr_core::*;
use awr_store::SourceRegistration;
use rusqlite::{Connection, params};
use support::Fixture;

fn work(f: &Fixture, key: &str, status: &str) -> WorkItem {
    WorkItem {
        spec_refs: vec![],
        ordinary_completion: None,
        archived: false,
        meta: f.meta(key),
        title: key.into(),
        kind: None,
        owner: Some("source-owner".into()),
        required: true,
        raw_status: status.into(),
        status: WorkStatus::normalize(status),
        priority: Some("P0".into()),
        milestone: None,
        score: None,
        evidence_level: None,
        summary: "summary".into(),
        next_action: "implement".into(),
        blocker: None,
        acceptance: vec!["usable".into()],
        tags: vec![],
        paths: vec![],
    }
}
fn edge(f: &Fixture, from: &str, to: &str, required: bool) -> Edge {
    Edge {
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
    }
}

#[test]
fn recursive_graph_handles_diamonds_missing_cycles_unknown_and_duplicate_sources() {
    let mut f = Fixture::new();
    let mut batch = ProjectionBatch::default();
    for (key, status) in [
        ("A", "planned"),
        ("B", "completed"),
        ("C", "completed"),
        ("D", "completed"),
        ("M", "ready"),
        ("X", "planned"),
        ("Y", "completed"),
        ("U", "vendor-paused"),
        ("Z", "planned"),
        ("BLOCK", "planned"),
    ] {
        batch.work_items.push(work(&f, key, status));
    }
    batch.work_items.last_mut().unwrap().blocker = Some("waiting for design".into());
    for (from, to, required) in [
        ("A", "B", true),
        ("A", "C", true),
        ("B", "D", true),
        ("C", "D", true),
        ("A", "optional-missing", false),
        ("M", "missing", true),
        ("X", "Y", true),
        ("Y", "X", true),
        ("Z", "U", true),
    ] {
        batch.edges.push(edge(&f, from, to, required));
    }
    let duplicate = batch.edges[0].clone();
    f.commit(batch);
    let report = f
        .store
        .work_readiness(f.project.id, "A", None, 100)
        .unwrap();
    assert!(report.ready); // The source owner does not create a runtime claim.
    assert_eq!(report.dependencies.dependencies.len(), 3);
    assert_eq!(report.dependencies.edges.len(), 4);
    let all = f
        .store
        .dependency_closure(f.project.id, "A", false)
        .unwrap();
    assert_eq!(all.missing_keys, ["optional-missing"]);
    let mut other = f
        .store
        .register_source(
            f.project.id,
            &SourceRegistration {
                domain: "ledger",
                role: "supporting",
                locator: "file:///other.yaml",
                format: "yaml",
                adapter: "yaml-ledger-v1",
            },
        )
        .unwrap();
    let mut extra = duplicate;
    extra.id = Id::new();
    extra.source_ref.source_id = other.id;
    extra.source_ref.locator = other.locator.clone();
    extra.source_ref.source_fingerprint = "other-1".into();
    other = f
        .store
        .commit_source_projection(
            &other,
            "other-1",
            ProjectionBatch {
                edges: vec![extra],
                ..Default::default()
            },
        )
        .unwrap();
    let duplicated = f
        .store
        .work_readiness(f.project.id, "A", None, 100)
        .unwrap();
    assert!(duplicated.ready);
    assert_eq!(duplicated.dependencies.dependencies.len(), 3);
    assert!(duplicated.diagnostics.is_empty());
    for (key, code) in [
        ("M", "missing_dependency"),
        ("X", "dependency_cycle"),
        ("U", "unknown_status"),
        ("Z", "unknown_status"),
        ("BLOCK", "active_blocker"),
    ] {
        let r = f
            .store
            .work_readiness(f.project.id, key, None, 100)
            .unwrap();
        assert!(!r.ready);
        assert!(
            r.diagnostics.iter().any(|d| d.code == code),
            "{key}: {:?}",
            r.diagnostics
        );
    }
    let cycles = f.store.dependency_closure(f.project.id, "X", true).unwrap();
    assert_eq!(cycles.cycle_keys, ["X", "Y"]);
    f.store
        .mark_source_freshness(&other, Freshness::Stale)
        .unwrap();
    let stale = f
        .store
        .work_readiness(f.project.id, "A", None, 100)
        .unwrap();
    assert!(!stale.ready);
    assert!(
        stale
            .diagnostics
            .iter()
            .any(|d| d.code == "source_not_fresh")
    );
    assert!(matches!(
        f.store.work_readiness(f.project.id, "missing", None, 100),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        f.store.ready_work(f.project.id, Some(Id::new()), 100),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn active_and_expired_claims_block_the_same_work_across_branches() {
    let mut f = Fixture::new();
    let w = work(&f, "A", "ready");
    let wid = w.meta.id;
    f.commit(ProjectionBatch {
        work_items: vec![w],
        ..Default::default()
    });
    // Seed runtime records directly: acquisition/release operations are a later milestone.
    let db = Connection::open(f.root.join("state.db")).unwrap();
    db.pragma_update(None, "foreign_keys", true).unwrap();
    let sid = Id::new();
    let cid = Id::new();
    let bid = Id::new();
    db.execute("INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision) VALUES(?1,?2,'experiment',0,'active',1)",params![bid.to_string(),f.project.id.to_string()]).unwrap();
    db.execute("INSERT INTO sessions(id,project_id,work_item_id,agent_id,provider,model,status,started_at,start_project_revision,revision) VALUES(?1,?2,?3,'runner','local','fixture','active',1,0,1)",params![sid.to_string(),f.project.id.to_string(),wid.to_string()]).unwrap();
    db.execute("INSERT INTO claims(id,project_id,work_item_id,session_id,agent_id,status,acquired_at,expires_at,revision) VALUES(?1,?2,?3,?4,'runner','active',1,200,1)",params![cid.to_string(),f.project.id.to_string(),wid.to_string(),sid.to_string()]).unwrap();
    let active = f
        .store
        .work_readiness(f.project.id, "A", None, 100)
        .unwrap();
    assert!(!active.ready);
    assert_eq!(active.active_claims[0].agent_id, "runner");
    let other = f
        .store
        .work_readiness(f.project.id, "A", Some(bid), 100)
        .unwrap();
    assert!(!other.ready);
    assert_eq!(other.active_claims[0].branch_id, None);
    assert!(
        f.store
            .work_readiness(f.project.id, "A", Some(bid), 200)
            .unwrap()
            .ready
    );
    assert!(
        f.store
            .work_readiness(f.project.id, "A", None, 200)
            .unwrap()
            .ready
    );
    let report = f.store.ready_work(f.project.id, None, 100).unwrap();
    assert!(report.ready.is_empty());
    assert_eq!(report.blocked.len(), 1);
    let expired = f.store.ready_work(f.project.id, None, 200).unwrap();
    assert_eq!(expired.ready.len(), 1);
    assert!(f.store.doctor().unwrap().ok);
}
