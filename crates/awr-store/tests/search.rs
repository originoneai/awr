mod support;
use awr_core::*;
use awr_store::SearchQuery;
use support::Fixture;

fn work(f: &Fixture, key: &str, title: &str, summary: &str) -> WorkItem {
    WorkItem {
        spec_refs: vec![],
        ordinary_completion: None,
        archived: false,
        meta: f.meta(key),
        title: title.into(),
        kind: None,
        owner: None,
        required: true,
        raw_status: "planned".into(),
        status: WorkStatus::Planned,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: summary.into(),
        next_action: "build".into(),
        blocker: None,
        acceptance: vec![],
        tags: vec![],
        paths: vec![],
    }
}
fn search(f: &mut Fixture, text: &str) -> Vec<awr_store::SearchHit> {
    f.store
        .search(
            f.project.id,
            &SearchQuery {
                text: Some(text.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .hits
}

#[test]
fn fts_and_structured_filters_return_ranked_versioned_summaries() {
    let mut f = Fixture::new();
    let w = work(&f, "W-1", "任务依赖分析", "Resolve dependency closure");
    let wid = w.meta.id;
    f.commit(ProjectionBatch {
        work_items: vec![w, work(&f, "W-2", "Build UI", "Draw work screen")],
        ..Default::default()
    });
    let revision = f.store.project(f.project.id).unwrap().project_revision;
    assert_eq!(search(&mut f, "依赖")[0].external_key, "W-1");
    assert_eq!(search(&mut f, "依赖分析")[0].external_key, "W-1");
    let english = search(&mut f, "dependency");
    assert_eq!(english[0].id, wid);
    assert!(english[0].rank < 0.0);
    assert_eq!(english[0].source_ref.as_ref().unwrap().source_revision, 1);
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        revision
    ); // Derived cache writes do not create work events/revisions.
    let structured = f
        .store
        .search(
            f.project.id,
            &SearchQuery {
                kind: Some("work_item".into()),
                status: Some("planned".into()),
                limit: 1,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(structured.hits.len(), 1);
    assert!(structured.truncated);
    let mut event = EventDraft::new("tool.failed", "Compiler dependency error");
    event.work_item_id = Some(wid);
    event.payload = serde_json::json!({"status":"failed","stdout":"RAW_STDOUT_SENTINEL"});
    f.store.append_event(f.project.id, revision, event).unwrap();
    let event = f
        .store
        .search(
            f.project.id,
            &SearchQuery {
                kind: Some("event".into()),
                status: Some("failed".into()),
                work_item_key: Some("W-1".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(event.hits.len(), 1);
    assert_eq!(event.hits[0].origin, "runtime");
    assert!(event.hits[0].source_ref.is_none());
    let mut progress = EventDraft::new("work.progress", "Continue the work");
    progress.payload = serde_json::json!({"status":"in_progress"});
    f.store
        .append_event(f.project.id, event.project_revision, progress)
        .unwrap();
    let progress = f
        .store
        .search(
            f.project.id,
            &SearchQuery {
                kind: Some("event".into()),
                status: Some("in_progress".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(progress.hits.len(), 1);
    assert_eq!(progress.hits[0].status.as_deref(), Some("in_progress"));
    assert!(search(&mut f, "RAW_STDOUT_SENTINEL").is_empty());
    f.source = f
        .store
        .mark_source_freshness(&f.source, Freshness::Stale)
        .unwrap();
    assert_eq!(
        search(&mut f, "依赖")[0].source_freshness,
        Some(Freshness::Stale)
    );
    f.store.retire_source(&f.source).unwrap();
    assert!(search(&mut f, "依赖").is_empty());
    assert!(f.store.doctor().unwrap().ok);
    assert_eq!(
        f.store.doctor().unwrap().schema_version,
        awr_store::SCHEMA_VERSION
    );
}

#[test]
fn index_excludes_source_bodies_credentials_code_and_long_tail_text() {
    let mut f = Fixture::new();
    let mut batch = ProjectionBatch::default();
    batch.goals.push(Goal {
        meta: f.meta("G"),
        title: "Readable goal".into(),
        status: "active".into(),
        priority: None,
        success_criteria: vec![],
        summary: "SOURCE_BODY_SENTINEL".into(),
    });
    batch.work_items = vec![
        work(&f, "W-1", "Credential check", "Review credential handling"),
        work(
            &f,
            "W-2",
            "Report",
            "Short useful summary\nFULL_BODY_SENTINEL",
        ),
        work(&f, "W-3", "Code", "```\nBINARY_CODE_SENTINEL"),
        work(
            &f,
            "W-4",
            "Long summary",
            &format!("{} LONG_TAIL_SENTINEL", "a".repeat(300)),
        ),
    ];
    f.commit(batch);
    // Current projection writes reject credentials. Seed only this disposable database
    // to exercise the read policy for records retained from before secret enforcement.
    let conn = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    conn.execute(
        "UPDATE work_items SET summary=?1 WHERE external_key='W-1'",
        ["api_key=RAW_CREDENTIAL_SENTINEL"],
    )
    .unwrap();
    for text in [
        "SOURCE_BODY_SENTINEL",
        "RAW_CREDENTIAL_SENTINEL",
        "FULL_BODY_SENTINEL",
        "BINARY_CODE_SENTINEL",
        "LONG_TAIL_SENTINEL",
    ] {
        assert!(search(&mut f, text).is_empty(), "{text} was indexed");
    }
    let docs = f
        .store
        .search(
            f.project.id,
            &SearchQuery {
                limit: 100,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(docs.hits.iter().all(|h| h.summary.chars().count() <= 280));
    assert!(docs.hits.iter().any(|h| h.summary == "[redacted]"));
    assert!(!docs.hits.iter().any(|h| h.summary.contains("SENTINEL")));
    assert!(matches!(
        f.store.search(
            f.project.id,
            &SearchQuery {
                text: Some("\"".into()),
                ..Default::default()
            }
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        f.store.search(
            f.project.id,
            &SearchQuery {
                kind: Some("arbitrary_table".into()),
                ..Default::default()
            }
        ),
        Err(Error::InvalidInput(_))
    ));
    let outsider = Id::new();
    assert!(matches!(
        f.store.search(outsider, &SearchQuery::default()),
        Err(Error::NotFound(_))
    ));
}
