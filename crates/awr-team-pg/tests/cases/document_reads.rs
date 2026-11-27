use super::*;
use awr_team::SourceActivationPlan;
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

async fn publish_snapshot(
    source: &SourceStore,
    admin: &tokio_postgres::Client,
    actor_id: &str,
    files: Vec<SourceFile>,
    activate: bool,
) -> CandidateRecord {
    let mut bundle_bytes: Vec<u8> = admin
        .query_one(
            "SELECT elem->>'text'
             FROM awr_team.projects p
             JOIN awr_team.source_snapshots s
               ON s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id
             CROSS JOIN LATERAL jsonb_array_elements(s.source_ref_json->'files') elem
             WHERE p.tenant_id=$1 AND p.id=$2 AND elem->>'path'='workstreams.json'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get::<_, String>(0)
        .into_bytes();
    let mut all_files = files;
    if !all_files.iter().any(|f| f.path == "workstreams.json") {
        all_files.push(SourceFile {
            path: "workstreams.json".into(),
            bytes: std::mem::take(&mut bundle_bytes),
        });
    }
    let candidate = source
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: actor_id.into(),
            parser_version: "workstreams/1".into(),
            files: all_files,
        })
        .await
        .unwrap();
    if activate {
        let reviewer = if actor_id == "reviewer" {
            "agent"
        } else {
            "reviewer"
        };
        source
            .approve(
                TENANT,
                PROJECT,
                &candidate.proposal_id,
                reviewer,
                &candidate.manifest_digest,
            )
            .await
            .unwrap();
        source
            .activate_workstreams(
                TENANT,
                PROJECT,
                actor_id,
                &candidate.proposal_id,
                &SourceActivationPlan {
                    candidate_digest: candidate.manifest_digest.clone(),
                    parser_version: candidate.parser_version.clone(),
                    expected_authority_epoch: candidate.base_epoch.clone(),
                    approved_candidate_digest: candidate.manifest_digest.clone(),
                },
            )
            .await
            .unwrap();
    }
    candidate
}

#[tokio::test]
async fn document_history_tracks_ordered_changes_deduplicates_unchanged_and_caps_at_twenty() {
    let (_guard, admin, db, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"# Guide v1\n\nInitial version.\n".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"PRIVATE beta v1".to_vec(),
        },
    ])
    .await;
    // Register an active person for 'agent' and leave 'reviewer' without an
    // active persons row so both determinable and absent submitter paths run.
    admin
        .batch_execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('reader-tenant','reader-project','agent','Lin Yue','active');
             UPDATE awr_team.project_memberships SET role='admin'
             WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND actor_id='reviewer';",
        )
        .await
        .unwrap();
    let initial_snapshot: String = admin
        .query_one(
            "SELECT active_snapshot_id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let source = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));

    // Unchanged snapshot for docs/alpha.md (only private-beta changed): must NOT duplicate.
    let unchanged = publish_snapshot(
        &source,
        &admin,
        "reviewer",
        vec![
            SourceFile {
                path: "docs/alpha.md".into(),
                bytes: b"# Guide v1\n\nInitial version.\n".to_vec(),
            },
            SourceFile {
                path: "docs/private-beta.md".into(),
                bytes: b"PRIVATE beta v2".to_vec(),
            },
        ],
        true,
    )
    .await;

    // Pending (unactivated) candidate: must NEVER appear in history.
    let _pending = publish_snapshot(
        &source,
        &admin,
        "agent",
        vec![
            SourceFile {
                path: "docs/alpha.md".into(),
                bytes: b"# Guide PENDING UNACTIVATED\n".to_vec(),
            },
            SourceFile {
                path: "docs/private-beta.md".into(),
                bytes: b"PRIVATE beta v2".to_vec(),
            },
        ],
        false,
    )
    .await;

    // Second real change by 'reviewer' (no active persons row -> submitted_by: null).
    let v2 = publish_snapshot(
        &source,
        &admin,
        "reviewer",
        vec![
            SourceFile {
                path: "docs/alpha.md".into(),
                bytes: b"# Guide v2\n\nRevised policy.\n".to_vec(),
            },
            SourceFile {
                path: "docs/private-beta.md".into(),
                bytes: b"PRIVATE beta v2".to_vec(),
            },
        ],
        true,
    )
    .await;

    // Third real change: revert back to v1 bytes by 'agent' (active person -> Lin Yue).
    let v3_revert = publish_snapshot(
        &source,
        &admin,
        "agent",
        vec![
            SourceFile {
                path: "docs/alpha.md".into(),
                bytes: b"# Guide v1\n\nInitial version.\n".to_vec(),
            },
            SourceFile {
                path: "docs/private-beta.md".into(),
                bytes: b"PRIVATE beta v2".to_vec(),
            },
        ],
        true,
    )
    .await;

    // Another unchanged snapshot after revert: must NOT overwrite v3_revert's snapshot id.
    let _unchanged_tail = publish_snapshot(
        &source,
        &admin,
        "reviewer",
        vec![
            SourceFile {
                path: "docs/alpha.md".into(),
                bytes: b"# Guide v1\n\nInitial version.\n".to_vec(),
            },
            SourceFile {
                path: "docs/private-beta.md".into(),
                bytes: b"PRIVATE beta v3".to_vec(),
            },
        ],
        true,
    )
    .await;

    // Partial-stream reader A (stream 1 only) does not pass directory.read:
    // history reads cleanly, and submitter names stay redacted as null.
    let mut q = query("document.history");
    q.source_path = Some("docs/alpha.md".into());
    let partial_reply = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let partial_items = partial_reply["data"]["items"].as_array().unwrap();
    assert_eq!(partial_items.len(), 3);
    assert!(
        partial_items
            .iter()
            .all(|item| item["submitted_by"].is_null()),
        "readers without directory.read must receive null submitted_by"
    );

    // Grant stream 2 to reader A so directory.read passes; determinable active
    // persons now resolve while actors without an active persons row stay null.
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
             workstream_id,authority_version,can_read) VALUES($1,$2,'agent','cli-a',$3,1,true)",
            &[&TENANT, &PROJECT, &Id::from(2).to_string()],
        )
        .await
        .unwrap();

    let reply = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let data = &reply["data"];
    assert_eq!(data["path"], "docs/alpha.md");
    assert_eq!(data["work_id"], Value::Null);
    assert_eq!(data["total"], 3);
    assert_eq!(data["limit"], 20);
    assert_eq!(data["truncated"], false);
    assert_eq!(data["order"], "newest_first");
    assert_eq!(data["source_kind"], "registered_source_snapshots");
    assert_eq!(data["attribution_basis"], "snapshot_submitter");
    assert_eq!(data["read_only"], true);
    assert_eq!(data["execution_authorized"], false);
    assert_eq!(
        data["as_of"]["source_snapshot_id"],
        reply["source_snapshot_id"]
    );
    assert_eq!(data["as_of"]["project_revision"], reply["project_revision"]);
    assert!(!reply.to_string().contains("PRIVATE"));
    assert!(!reply.to_string().contains("Initial version"));
    assert!(!reply.to_string().contains("PENDING UNACTIVATED"));

    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let d1 = format!("{:x}", Sha256::digest(b"# Guide v1\n\nInitial version.\n"))[..12].to_string();
    let d2 = format!("{:x}", Sha256::digest(b"# Guide v2\n\nRevised policy.\n"))[..12].to_string();
    // Newest first: v3_revert -> v2 -> initial_snapshot (unchanged skipped).
    assert_eq!(items[0]["source_snapshot_id"], v3_revert.snapshot_id);
    assert_eq!(items[0]["digest_prefix"], d1);
    assert_eq!(
        items[0]["submitted_by"],
        json!({"person_id": "agent", "display_name": "Lin Yue"})
    );
    assert_eq!(items[1]["source_snapshot_id"], v2.snapshot_id);
    assert_eq!(items[1]["digest_prefix"], d2);
    assert_eq!(items[1]["submitted_by"], Value::Null);
    assert_eq!(items[2]["source_snapshot_id"], initial_snapshot);
    assert_eq!(items[2]["digest_prefix"], d1);
    assert_eq!(
        items[2]["submitted_by"],
        json!({"person_id": "agent", "display_name": "Lin Yue"})
    );
    assert!(
        items
            .iter()
            .all(|item| item["source_snapshot_id"] != unchanged.snapshot_id),
        "unchanged snapshots must never be duplicated"
    );
    assert!(
        items[0]["recorded_at_unix_ms"].as_i64().unwrap()
            >= items[1]["recorded_at_unix_ms"].as_i64().unwrap()
    );
    assert!(
        items[1]["recorded_at_unix_ms"].as_i64().unwrap()
            >= items[2]["recorded_at_unix_ms"].as_i64().unwrap()
    );

    // Push 20 more distinct changes (total = 23) and verify truncation to the latest 20.
    let mut created_ids = Vec::new();
    for version in 4..=23 {
        let body = format!("# Guide v{version}\n\nRevision {version}.\n");
        let c = publish_snapshot(
            &source,
            &admin,
            "agent",
            vec![
                SourceFile {
                    path: "docs/alpha.md".into(),
                    bytes: body.into_bytes(),
                },
                SourceFile {
                    path: "docs/private-beta.md".into(),
                    bytes: b"PRIVATE beta v3".to_vec(),
                },
            ],
            true,
        )
        .await;
        created_ids.push(c.snapshot_id);
    }
    let capped = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(capped["data"]["total"], 23);
    assert_eq!(capped["data"]["limit"], 20);
    assert_eq!(capped["data"]["truncated"], true);
    let capped_items = capped["data"]["items"].as_array().unwrap();
    assert_eq!(capped_items.len(), 20);
    assert_eq!(
        capped_items[0]["source_snapshot_id"],
        *created_ids.last().unwrap()
    );
    assert_eq!(capped_items[19]["source_snapshot_id"], created_ids[0]);
}

#[tokio::test]
async fn document_history_distinguishes_missing_paths_from_unauthorized_and_cross_project_reads() {
    let (_guard, admin, _, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"# Alpha guide\n".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"# Beta secret\n".to_vec(),
        },
    ])
    .await;
    // Register a path in stream 1 that has never been stored in any snapshot.
    add_required_source(&admin, 1, "docs/missing-alpha.md").await;

    // 1. Authorized registered path absent from snapshots -> InvalidInput (source_unavailable), NOT empty list.
    let mut missing = query("document.history");
    missing.source_path = Some("docs/missing-alpha.md".into());
    let err = store.query(TENANT, PROJECT, A, missing).await.unwrap_err();
    let diag = err
        .query_input_diagnostic()
        .expect("typed InvalidInput diagnostic");
    assert_eq!(diag["code"], "InvalidInput");
    assert_eq!(diag["invalid_field"], "/source_path");
    assert_eq!(diag["constraint"], "registered_source_required");

    // 2. Peer-stream document -> Forbidden.
    let mut peer = query("document.history");
    peer.source_path = Some("docs/private-beta.md".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, peer).await,
        Err(PgError::Forbidden)
    ));

    // 3. Partial-stream reader asking for completely unregistered path -> Forbidden (no existence oracle).
    let mut unknown = query("document.history");
    unknown.source_path = Some("docs/never-existed.md".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, unknown.clone()).await,
        Err(PgError::Forbidden)
    ));

    // 4. Shared document (in stream 1 + stream 2) -> Forbidden for stream-1-only reader A.
    add_required_source(&admin, 2, "docs/alpha.md").await;
    let mut shared = query("document.history");
    shared.source_path = Some("docs/alpha.md".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, shared.clone()).await,
        Err(PgError::Forbidden)
    ));

    // 5. Grant stream 2 to reader A -> now A has full-project read access:
    //    - shared doc succeeds
    //    - unregistered path returns InvalidInput (source_unavailable), NOT empty list and NOT Forbidden.
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
             workstream_id,authority_version,can_read) VALUES($1,$2,'agent','cli-a',$3,1,true)",
            &[&TENANT, &PROJECT, &Id::from(2).to_string()],
        )
        .await
        .unwrap();
    let shared_ok = store
        .query(TENANT, PROJECT, A, shared.clone())
        .await
        .unwrap();
    assert_eq!(shared_ok["data"]["items"].as_array().unwrap().len(), 1);
    let err = store.query(TENANT, PROJECT, A, unknown).await.unwrap_err();
    let diag = err
        .query_input_diagnostic()
        .expect("typed InvalidInput diagnostic");
    assert_eq!(diag["code"], "InvalidInput");
    assert_eq!(diag["constraint"], "registered_source_required");

    // 6. Unscoped credential and cross-tenant request -> Forbidden.
    assert!(matches!(
        store.query(TENANT, PROJECT, NONE, shared.clone()).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store
            .query("other-tenant", PROJECT, A, shared.clone())
            .await,
        Err(PgError::Forbidden)
    ));

    // 7. Revoked credential -> Forbidden.
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, shared).await,
        Err(PgError::Forbidden)
    ));
}
