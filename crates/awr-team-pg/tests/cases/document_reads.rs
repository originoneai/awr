use super::*;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn document_catalog_matches_work_metadata_and_preserves_controlled_content_reads() {
    let body = "# Delivery guide\n\nCheck the outcome before sharing it.\n";
    let (_guard, _admin, _, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: body.as_bytes().to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"PRIVATE peer guidance".to_vec(),
        },
    ])
    .await;
    let project = store
        .query(TENANT, PROJECT, A, query("documents.list"))
        .await
        .unwrap();
    let data = &project["data"];
    assert_eq!(data["total"], 1);
    assert_eq!(data["read_only"], true);
    assert_eq!(data["execution_authorized"], false);
    assert_eq!(data["scope"], "authorized_workstreams");
    assert!(data["as_of"]["at_unix_ms"].is_i64());
    let row = &data["items"][0];
    assert_eq!(row["name"], "Delivery guide");
    assert_eq!(row["summary"], "Check the outcome before sharing it.");
    assert_eq!(row["summary_kind"], "source_excerpt");
    assert_eq!(row["status"], "available");
    assert_eq!(row["readable"], true);
    assert!(!project.to_string().contains("PRIVATE"));
    assert!(!project.to_string().contains("private-beta"));
    let mut work = query("documents.list");
    work.work_id = Some("a".into());
    let selected = store.query(TENANT, PROJECT, A, work).await.unwrap();
    let mut selected_items = data["items"].clone();
    selected_items[0]["content_work_id"] = json!("a");
    assert_eq!(selected["data"]["items"], selected_items);
    assert_eq!(selected["data"]["scope"], "work");
    let prepared = prepare(&store, A, "a").await;
    let required = &prepared["data"]["required_specs"][0];
    assert_eq!(row["sha256"], required["sha256"]);
    assert_eq!(row["byte_length"], required["byte_length"]);
    assert_eq!(row["byte_length"], body.len());
    assert_eq!(prepared["project_revision"], project["project_revision"]);
    let mut content = query("source.content");
    content.source_path = Some("docs/alpha.md".into());
    content.expected_sha256 = Some(row["sha256"].as_str().unwrap().into());
    let read = store.query(TENANT, PROJECT, A, content).await.unwrap();
    assert_eq!(read["data"]["text"], body);
    assert_eq!(read["data"]["sha256"], row["sha256"]);
}

#[tokio::test]
async fn document_catalog_paging_search_and_cursors_follow_current_source_and_permission() {
    let (_guard, admin, _, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"# Delivery guide\n\nCheck the outcome.".to_vec(),
        },
        SourceFile {
            path: "docs/z-notes.md".into(),
            bytes: b"# Release notes\n\nShare the reviewed changes.".to_vec(),
        },
        SourceFile {
            path: "docs/unreferenced.md".into(),
            bytes: b"Unregistered visibility must stay private".to_vec(),
        },
    ])
    .await;
    add_required_source(&admin, 1, "docs/z-notes.md").await;
    let mut q = query("documents.list");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 2);
    assert_eq!(first["data"]["truncated"], true);
    assert_eq!(first["data"]["items"][0]["document_position"], 1);
    let cursor = first["data"]["next_cursor"].as_str().unwrap().to_string();
    assert!(!cursor.contains("docs/"));
    q.cursor = Some(cursor);
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(second["data"]["items"][0]["name"], "Release notes");
    assert_eq!(second["data"]["items"][0]["document_position"], 2);
    assert!(second["data"]["next_cursor"].is_null());
    let mut search = query("documents.list");
    search.search = Some("RELEASE".into());
    let filtered = store
        .query(TENANT, PROJECT, A, search.clone())
        .await
        .unwrap();
    assert_eq!(filtered["data"]["total"], 1);
    assert_eq!(filtered["data"]["items"][0]["path"], "docs/z-notes.md");
    search.cursor = q.cursor.clone();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, search).await,
        Err(PgError::CursorExpired)
    ));
    let (id, mut source) = source_record(&admin).await;
    source["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|file| file["path"] == "docs/alpha.md")
        .unwrap()["text"] = json!("Changed guidance");
    replace_source_record(&admin, &id, &source).await;
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    let mut small = query("documents.list");
    small.max_context_bytes = Some(10);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, small).await,
        Err(PgError::ResponseTooLarge)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn document_catalog_rechecks_shared_document_access_before_exposing_metadata() {
    let (_guard, admin, _, store) = setup_with_specs(vec![SourceFile {
        path: "docs/alpha.md".into(),
        bytes: b"# Private shared title\n\nPrivate shared excerpt.".to_vec(),
    }])
    .await;
    add_required_source(&admin, 2, "docs/alpha.md").await;
    let mut q = query("documents.list");
    q.work_id = Some("a".into());
    let restricted = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let row = &restricted["data"]["items"][0];
    assert_eq!(row["status"], "restricted");
    assert_eq!(row["readable"], false);
    for key in ["path", "name", "summary", "sha256", "byte_length"] {
        assert!(row[key].is_null(), "restricted {key} must remain hidden");
    }
    assert!(!restricted.to_string().contains("Private shared"));
    assert!(!restricted.to_string().contains("alpha.md"));
    for work in ["b-private", "missing"] {
        let mut selected = query("documents.list");
        selected.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, selected).await,
            Err(PgError::Forbidden)
        ));
    }
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
        workstream_id,authority_version,can_read) VALUES($1,$2,'agent','cli-a',$3,1,true)",
            &[&TENANT, &PROJECT, &Id::from(2).to_string()],
        )
        .await
        .unwrap();
    let allowed = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(allowed["data"]["items"][0]["name"], "Private shared title");
    assert_eq!(allowed["data"]["items"][0]["readable"], true);
}

#[tokio::test]
async fn document_catalog_supports_full_pages_of_bounded_unicode_metadata() {
    let text = format!("# {}\n\n{}", "🌍".repeat(256), "🌍".repeat(512));
    let mut files = vec![SourceFile {
        path: "docs/alpha.md".into(),
        bytes: text.as_bytes().to_vec(),
    }];
    for index in 1..20 {
        files.push(SourceFile {
            path: format!("docs/{}-{index}.md", "p".repeat(490)),
            bytes: text.as_bytes().to_vec(),
        });
    }
    let paths: Vec<_> = files.iter().skip(1).map(|file| file.path.clone()).collect();
    let (_guard, admin, _, store) = setup_with_specs(files).await;
    for path in paths {
        add_required_source(&admin, 1, &path).await;
    }
    let mut q = query("documents.list");
    q.max_context_bytes = Some(65_536);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::ResponseTooLarge)
    ));
    q.max_context_bytes = Some(262_144);
    let page = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(page["data"]["items"].as_array().unwrap().len(), 20);
    let bytes = serde_json::to_vec(&page).unwrap().len();
    assert!(bytes > 65_536 && bytes <= 262_144);
}

#[tokio::test]
async fn document_catalog_distinguishes_missing_integrity_and_content_size_limits() {
    let (_guard, admin, _, store) = setup_with_specs(vec![SourceFile {
        path: "docs/alpha.md".into(),
        bytes: b"# Delivery guide\n\nPublish safely.".to_vec(),
    }])
    .await;
    let (id, original) = source_record(&admin).await;
    for field in ["text", "sha256", "bytes"] {
        let mut source = original.clone();
        source["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|file| file["path"] == "docs/alpha.md")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        replace_source_record(&admin, &id, &source).await;
        let result = store
            .query(TENANT, PROJECT, A, query("documents.list"))
            .await
            .unwrap();
        let row = &result["data"]["items"][0];
        assert_eq!(row["status"], "unverified");
        assert_eq!(row["readable"], false);
        assert!(row["byte_length"].is_null());
        assert!(row["summary"].is_null());
    }
    let mut changed = original.clone();
    changed["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|file| file["path"] == "docs/alpha.md")
        .unwrap()["text"] = json!("Unverified changed text");
    replace_source_record(&admin, &id, &changed).await;
    let result = store
        .query(TENANT, PROJECT, A, query("documents.list"))
        .await
        .unwrap();
    assert_eq!(result["data"]["items"][0]["status"], "changed");
    assert!(!result.to_string().contains("Unverified changed text"));
    let mut missing = original.clone();
    missing["files"]
        .as_array_mut()
        .unwrap()
        .retain(|file| file["path"] != "docs/alpha.md");
    replace_source_record(&admin, &id, &missing).await;
    let result = store
        .query(TENANT, PROJECT, A, query("documents.list"))
        .await
        .unwrap();
    assert_eq!(result["data"]["items"][0]["status"], "missing");
    let mut oversized = original;
    let body = format!("# Large guide\n\n{}", "Guidance. ".repeat(30000));
    let file = oversized["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|file| file["path"] == "docs/alpha.md")
        .unwrap();
    file["text"] = json!(body);
    file["bytes"] = json!(body.len());
    file["sha256"] = json!(format!("{:x}", Sha256::digest(body.as_bytes())));
    replace_source_record(&admin, &id, &oversized).await;
    let result = store
        .query(TENANT, PROJECT, A, query("documents.list"))
        .await
        .unwrap();
    let row = &result["data"]["items"][0];
    assert_eq!(row["status"], "too_large");
    assert_eq!(row["readable"], false);
    assert_eq!(row["name"], "Large guide");
    assert_eq!(row["byte_length"], body.len());
    for size in [65_537, 262_144] {
        let text = "x".repeat(size);
        let file = oversized["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|file| file["path"] == "docs/alpha.md")
            .unwrap();
        file["text"] = json!(text);
        file["bytes"] = json!(size);
        file["sha256"] = json!(format!("{:x}", Sha256::digest(text.as_bytes())));
        replace_source_record(&admin, &id, &oversized).await;
        let catalog = store
            .query(TENANT, PROJECT, A, query("documents.list"))
            .await
            .unwrap();
        assert_eq!(catalog["data"]["items"][0]["readable"], true);
        let mut content = query("source.content");
        content.source_path = Some("docs/alpha.md".into());
        content.expected_sha256 = catalog["data"]["items"][0]["sha256"]
            .as_str()
            .map(str::to_owned);
        assert!(matches!(
            store.query(TENANT, PROJECT, A, content.clone()).await,
            Err(PgError::ContextIncomplete)
        ));
        content.max_context_bytes = Some(262_144);
        let full = store.query(TENANT, PROJECT, A, content).await.unwrap();
        assert_eq!(full["data"]["text"], text);
        assert_eq!(full["data"]["byte_length"], size);
    }
}
