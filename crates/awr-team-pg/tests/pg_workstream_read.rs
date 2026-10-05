#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::Id;
use awr_team_pg::PgError;
use fixture::*;

#[tokio::test]
async fn work_snapshot_preserves_context_hash_scope_and_total_byte_budget() {
    let (_guard, _, _, store) = setup().await;
    let prepared = prepare(&store, A, "a").await;
    let mut q = query("work.snapshot");
    q.work_id = Some("a".into());
    let snapshot = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let data = &snapshot["data"];
    assert_eq!(data["context_hash"], prepared["data"]["context_hash"]);
    assert_eq!(snapshot["project_revision"], prepared["project_revision"]);
    assert_eq!(data["snapshot"]["consistency"], "repeatable_read");
    assert_eq!(
        data["snapshot"]["project_revision"],
        snapshot["project_revision"]
    );
    assert_eq!(
        data["snapshot"]["source_snapshot_id"],
        snapshot["source_snapshot_id"]
    );
    assert_eq!(
        data["snapshot"]["coordinator_epoch"],
        snapshot["coordinator_epoch"]
    );
    assert_eq!(data["observation"]["contract_hash"], data["contract_hash"]);
    assert_eq!(data["observation"]["execution_authorized"], false);
    assert_eq!(
        data["snapshot"]["queried_at_unix_ms"],
        data["observation"]["observed_at_unix_ms"]
    );
    let mut original = data.clone();
    original.as_object_mut().unwrap().remove("snapshot");
    original.as_object_mut().unwrap().remove("observation");
    assert_eq!(original, prepared["data"]);
    assert!(!snapshot.to_string().contains("PRIVATE"));
    let caps = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    assert!(
        caps["queries"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("work.snapshot"))
    );
    assert_eq!(caps["session_feedback"]["snapshot_read"], "work.snapshot");

    // The opt-in operation cannot silently drop feedback to meet a context budget.
    q.max_context_bytes = Some(serde_json::to_vec(&prepared["data"]).unwrap().len());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::ContextIncomplete)
    ));
    q.op = "work.prepare".into();
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["context_hash"],
        data["context_hash"]
    );
    q.op = "work.snapshot".into();
    q.max_context_bytes = None;
    for work in ["b-private", "missing"] {
        q.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q.clone()).await,
            Err(PgError::Forbidden)
        ));
    }
    q.work_id = Some("a".into());
    assert!(matches!(
        store.query("other-tenant", PROJECT, A, q.clone()).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q.clone()).await,
        Err(PgError::Forbidden)
    ));
    q.session_id = Some("session-b".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn incomplete_snapshot_keeps_one_restoration_action_when_duplicate_advice_is_omitted() {
    let (_guard, _, _, store) = setup_with_specs(vec![]).await;
    let mut q = query("work.snapshot");
    q.work_id = Some("a".into());
    let full = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let data = &full["data"];
    assert_eq!(data["context_complete"], false);
    assert_eq!(data["guidance"]["code"], "restore_context");
    assert_eq!(data["guidance"], data["observation"]["guidance"]);
    assert!(data["observation"]["guidance"].to_string().len() < 900);
    let mut mandatory = data.clone();
    mandatory.as_object_mut().unwrap().remove("guidance");
    let budget = serde_json::to_vec(&mandatory).unwrap().len();
    q.max_context_bytes = Some(budget);
    let compact = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert!(compact["data"].get("guidance").is_none());
    assert_eq!(compact["data"]["observation"]["guidance"], data["guidance"]);
    assert_eq!(compact["data"]["context_hash"], data["context_hash"]);
    assert_eq!(
        compact["data"]["completeness_reasons"],
        data["completeness_reasons"]
    );
    assert_eq!(
        compact["data"]["observation"]["execution_authorized"],
        false
    );
    assert!(serde_json::to_vec(&compact["data"]).unwrap().len() <= budget);
}

#[tokio::test]
async fn work_snapshot_does_not_mix_runtime_revisions_during_concurrent_writes() {
    let (_guard, admin, db, store) = setup().await;
    admin.execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
        VALUES($1,$2,'main','a','ready',3,3)", &[&TENANT,&PROJECT]).await.unwrap();
    let mut writer = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let writing = tokio::spawn(async move {
        for _ in 0..32 {
            let tx = writer.transaction().await.unwrap();
            tx.execute("UPDATE awr_team.work_runtime SET work_version=work_version+1,last_fence=last_fence+1
                WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'", &[&TENANT,&PROJECT]).await.unwrap();
            tx.execute(
                "UPDATE awr_team.projects SET project_revision=project_revision+1
                WHERE tenant_id=$1 AND id=$2",
                &[&TENANT, &PROJECT],
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });
    for _ in 0..24 {
        let mut q = query("work.snapshot");
        q.work_id = Some("a".into());
        let snapshot = store.query(TENANT, PROJECT, A, q).await.unwrap();
        let data = &snapshot["data"];
        assert_eq!(data["runtime"], data["observation"]["runtime"]);
        assert_eq!(
            data["runtime"]["work_version"],
            snapshot["project_revision"]
        );
        assert_eq!(data["runtime"]["last_fence"], snapshot["project_revision"]);
    }
    writing.await.unwrap();
    let mut q = query("work.snapshot");
    q.work_id = Some("a".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["project_revision"],
        "35"
    );
}

#[tokio::test]
async fn work_observation_is_scoped_current_and_does_not_change_context_or_authority() {
    let (_guard, admin, _, store) = setup().await;
    let prepared = prepare(&store, A, "a").await;
    let mut q = query("work.observe");
    q.work_id = Some("a".into());
    let observed = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let data = &observed["data"];
    assert_eq!(data["session"]["id"], "session-a");
    assert_eq!(data["session"]["client_id"], "cli-a");
    assert!(data["session"].get("conversation_id").is_none());
    assert_eq!(data["checkpoint"]["next_action"], "continue alpha");
    assert_eq!(data["checkpoint"]["contract_matches_current"], false);
    assert!(data["model"].is_null());
    assert!(data["usage"].is_null());
    assert_eq!(data["execution_authorized"], false);
    assert!(!observed.to_string().contains("PRIVATE"));
    assert_eq!(prepared["data"], prepare(&store, A, "a").await["data"]);
    assert_eq!(prepared["project_revision"], observed["project_revision"]);
    admin.batch_execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        SELECT tenant_id,project_id,'zz-newer-closed-supervisor',scope_id,work_id,actor_id,client_id,'closed-review','ended',workstream_id,ownership_version
        FROM awr_team.sessions WHERE id='session-a'").await.unwrap();
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["session"]["id"],
        "session-a"
    );
    for work in ["b-private", "missing"] {
        q.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q.clone()).await,
            Err(PgError::Forbidden)
        ));
    }
    q.work_id = Some("a".into());
    admin
        .batch_execute("UPDATE awr_team.sessions SET ownership_version=2 WHERE work_id='a'")
        .await
        .unwrap();
    let moved = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert!(moved["data"]["session"].is_null());
    assert!(moved["data"]["checkpoint"].is_null());
}

#[tokio::test]
async fn credentials_and_project_membership_do_not_grant_other_clients_scopes() {
    let (_guard, _, _, store) = setup().await;
    let a = store
        .query(TENANT, PROJECT, A, query("workstreams.list"))
        .await
        .unwrap();
    assert_eq!(a["total"], 1);
    assert!(!a.to_string().contains("private-beta"));
    let b = store
        .query(TENANT, PROJECT, B, query("work.list"))
        .await
        .unwrap();
    assert_eq!(b["data"]["total"], 1);
    assert_eq!(b["data"]["items"][0]["work_id"], "b-private");
    for token in [NONE, "garbage", &A.replace('a', "d")] {
        assert!(matches!(
            store
                .query(TENANT, PROJECT, token, query("capabilities"))
                .await,
            Err(PgError::Forbidden)
        ));
    }
    assert!(matches!(
        store
            .query("other-tenant", PROJECT, A, query("capabilities"))
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn protocol_discovery_requires_authentication_and_a_readable_scope() {
    let (_guard, _, _, store) = setup().await;
    for token in ["garbage", NONE] {
        assert!(matches!(
            store
                .query(TENANT, PROJECT, token, query("claim.acquire"))
                .await,
            Err(PgError::Forbidden)
        ));
    }
    assert!(matches!(
        store
            .query(TENANT, PROJECT, A, query("claim.acquire"))
            .await,
        Err(PgError::Unsupported(_))
    ));
}

#[tokio::test]
async fn hidden_and_missing_work_or_session_have_the_same_denial() {
    let (_guard, _, _, store) = setup().await;
    for work in ["b-private", "missing"] {
        let mut q = query("work.prepare");
        q.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    for session in ["session-b", "absent"] {
        let mut q = query("session.inspect");
        q.session_id = Some(session.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    let mut mismatch = query("work.prepare");
    mismatch.work_id = Some("a".into());
    mismatch.workstream_id = Some(Id::from(2));
    assert!(store.query(TENANT, PROJECT, A, mismatch).await.is_err());
}

#[tokio::test]
async fn scoped_count_search_pagination_and_cursors_never_include_hidden_works() {
    let (_guard, admin, _, store) = setup().await;
    let mut q = query("work.list");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 2);
    q.cursor = Some(first["data"]["next_cursor"].as_str().unwrap().into());
    let next = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(next["data"]["items"][0]["work_id"], "c");
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    admin.batch_execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    let mut q = query("work.search");
    q.search = Some("private".into());
    let search = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(search["data"]["total"], 0);
}

#[tokio::test]
async fn dependency_exports_are_opaque_and_required_context_is_never_cut_to_budget() {
    let (_guard, _, _, store) = setup().await;
    let mut q = query("work.prepare");
    q.work_id = Some("c".into());
    let result = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(result["data"]["context_complete"], false);
    assert_eq!(result["data"]["dependency_export_unavailable"], true);
    assert!(!result.to_string().contains("b-private"));
    assert!(result.to_string().contains("preserve compatibility"));
    q.max_context_bytes = Some(1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ContextIncomplete)
    ));
}

#[tokio::test]
async fn events_and_recovery_respect_attribution_and_ownership_generation() {
    let (_guard, admin, _, store) = setup().await;
    let events = store
        .query(TENANT, PROJECT, A, query("events.list"))
        .await
        .unwrap();
    assert_eq!(events["data"]["items"].as_array().unwrap().len(), 1);
    assert!(!events.to_string().contains("session-b"));
    let mut q = query("work.recovery");
    q.work_id = Some("a".into());
    let recovery = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(
        recovery["data"]["items"][0]["next_action"],
        "continue alpha"
    );
    assert!(!recovery.to_string().contains("PRIVATE"));
    assert_eq!(
        recovery["data"]["items"][0]["contract_matches_current"],
        false
    );
    let current = recovery["data"]["current_contract_hash"].as_str().unwrap();
    admin
        .execute(
            "UPDATE awr_team.checkpoints SET contract_hash=$1 WHERE session_id='session-a'",
            &[&current],
        )
        .await
        .unwrap();
    let matched = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(
        matched["data"]["items"][0]["contract_matches_current"],
        true
    );
    // A matching contract is only one recovery fact, never permission to resume.
    assert_eq!(matched["data"]["automatic_resume"], false);
    admin
        .batch_execute("UPDATE awr_team.sessions SET ownership_version=2 WHERE id='session-a'")
        .await
        .unwrap();
    assert!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut q = query("session.inspect");
    q.session_id = Some("session-a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn semantic_context_tracks_selected_runtime_without_global_audit_churn() {
    let (_guard, admin, _, store) = setup().await;
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    let before = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(before["data"]["context_complete"], true);
    assert_eq!(
        before["data"]["context_hash_protocol"],
        "awr-team-workstream-context-v1"
    );
    assert!(before["data"]["runtime"].is_null());
    admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version)
        VALUES('reader-tenant','reader-project','main','b-private','pending',1);
        UPDATE awr_team.projects SET project_revision=project_revision+1 WHERE tenant_id='reader-tenant';").await.unwrap();
    let unrelated = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_ne!(before["project_revision"], unrelated["project_revision"]);
    assert_eq!(before["data"], unrelated["data"]);
    admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,recovery_blocked)
        VALUES('reader-tenant','reader-project','main','a','blocked',2,true)").await.unwrap();
    let changed = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_ne!(
        changed["data"]["context_hash"],
        before["data"]["context_hash"]
    );
    assert_eq!(changed["data"]["runtime"]["work_version"], "2");
    assert_eq!(changed["data"]["runtime"]["recovery_blocked"], true);
    assert_eq!(changed["data"]["execution_admission"], "not_evaluated");
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET status='frozen' WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    let frozen = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(frozen["project_status"], "frozen");
    assert_ne!(
        frozen["data"]["context_hash"],
        changed["data"]["context_hash"]
    );
}

#[tokio::test]
async fn mismatched_event_attribution_stale_grants_and_oversized_recovery_fail_closed() {
    let (_guard, admin, _, store) = setup().await;
    admin.execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json,workstream_id)
        VALUES($1,$2,'wrong-stream',4,0,'session.started','agent','b-private','{}',$3),
              ($1,$2,'unattributed',4,1,'session.started','agent','a','{}',NULL)",
        &[&TENANT,&PROJECT,&Id::from(1).to_string()]).await.unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET project_revision=4 WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    let events = store
        .query(TENANT, PROJECT, A, query("events.list"))
        .await
        .unwrap();
    assert_eq!(events["data"]["items"].as_array().unwrap().len(), 1);
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET authority_version=2 WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Forbidden)
    ));
    admin.batch_execute("UPDATE awr_team.workstream_grants SET authority_version=1 WHERE client_id='cli-a';
        UPDATE awr_team.checkpoints SET next_action=repeat('x',1048576) WHERE session_id='session-a'").await.unwrap();
    let mut q = query("work.recovery");
    q.work_id = Some("a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ResponseTooLarge)
    ));
}

async fn wait_for_lock(admin: &tokio_postgres::Client, query_fragment: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = admin
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)",
                    &[&format!("%{query_fragment}%")],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected real PostgreSQL row-lock wait");
}

#[tokio::test]
async fn revocation_and_reads_serialize_in_both_orders_without_stale_authority() {
    let (_guard, mut admin, db, store) = setup().await;
    let store = std::sync::Arc::new(store);
    let observer = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    // Revoke first. The reader began with an older repeatable-read snapshot
    // but must not return it after waiting on the credential row.
    let revoke = admin.transaction().await.unwrap();
    revoke
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    let reader = store.clone();
    let read =
        tokio::spawn(async move { reader.query(TENANT, PROJECT, A, query("work.list")).await });
    wait_for_lock(&observer, "FOR SHARE OF t,a,c,m").await;
    revoke.commit().await.unwrap();
    match read.await.unwrap() {
        Err(PgError::Forbidden) => {}
        Err(PgError::Db(e)) => assert_eq!(
            e.code(),
            Some(&tokio_postgres::error::SqlState::T_R_SERIALIZATION_FAILURE)
        ),
        other => panic!("revocation must prevent stale data: {other:?}"),
    }
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Forbidden)
    ));
    admin
        .batch_execute("UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'")
        .await
        .unwrap();
    // Read first. Hold a later grant row to observe the authenticated reader
    // retaining its credential lock until the whole query finishes.
    let hold = admin.transaction().await.unwrap();
    hold.query(
        "SELECT workstream_id FROM awr_team.workstream_grants WHERE client_id='cli-a' FOR UPDATE",
        &[],
    )
    .await
    .unwrap();
    let reader = store.clone();
    let read =
        tokio::spawn(async move { reader.query(TENANT, PROJECT, A, query("work.list")).await });
    wait_for_lock(&observer, "ORDER BY workstream_id FOR SHARE").await;
    let revoker = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let revoke = tokio::spawn(async move {
        revoker
            .batch_execute(
                "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            )
            .await
    });
    wait_for_lock(&observer, "UPDATE awr_team.credentials SET revoked_at").await;
    hold.rollback().await.unwrap();
    assert_eq!(read.await.unwrap().unwrap()["data"]["total"], 2);
    revoke.await.unwrap().unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn dynamic_revocation_expiry_actor_and_membership_changes_take_effect_on_next_read() {
    let (_guard, admin, _, store) = setup().await;
    assert!(
        store
            .query(TENANT, PROJECT, A, query("capabilities"))
            .await
            .is_ok()
    );
    for (disable, restore) in [
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'",
        ),
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='reader-a'",
        ),
        (
            "UPDATE awr_team.actors SET status='disabled' WHERE id='agent'",
            "UPDATE awr_team.actors SET status='active' WHERE id='agent'",
        ),
        (
            "UPDATE awr_team.tenants SET status='disabled' WHERE id='reader-tenant'",
            "UPDATE awr_team.tenants SET status='active' WHERE id='reader-tenant'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET active=false WHERE client_id='cli-a'",
            "UPDATE awr_team.workstream_grants SET active=true WHERE client_id='cli-a'",
        ),
    ] {
        admin.batch_execute(disable).await.unwrap();
        assert!(matches!(
            store.query(TENANT, PROJECT, A, query("capabilities")).await,
            Err(PgError::Forbidden)
        ));
        admin.batch_execute(restore).await.unwrap();
        assert!(
            store
                .query(TENANT, PROJECT, A, query("capabilities"))
                .await
                .is_ok()
        );
    }
    admin.batch_execute("DELETE FROM awr_team.workstream_grants WHERE actor_id='agent'; DELETE FROM awr_team.project_memberships WHERE actor_id='agent'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("capabilities")).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn multi_scope_choice_is_explicit_and_protocol_rejects_forged_authority_fields() {
    let (_guard, admin, _, store) = setup().await;
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-a',$3,1,true)",&[&TENANT,&PROJECT,&Id::from(2).to_string()]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.list")).await,
        Err(PgError::Workstream(
            awr_core::WorkstreamError::ScopeRequired
        ))
    ));
    for field in ["actor_id", "tenant_id", "project_id", "client_id", "grants"] {
        let mut value = serde_json::json!({"protocol_version":1,"op":"work.list"});
        value[field] = serde_json::json!("forged");
        assert!(serde_json::from_value::<awr_team_pg::WorkstreamQuery>(value).is_err());
    }
    let mut unsupported = query("claim.acquire");
    unsupported.protocol_version = 1;
    assert!(matches!(
        store.query(TENANT, PROJECT, A, unsupported).await,
        Err(PgError::Unsupported(_))
    ));
}

#[tokio::test]
async fn future_code_paths_do_not_require_existing_source_documents() {
    let (_guard, _, _, store) = setup().await;
    let prepared = prepare(&store, A, "a").await;
    assert_eq!(
        prepared["data"]["visible_contract"]["scope_paths"],
        serde_json::json!(["src"])
    );
    assert_eq!(prepared["data"]["context_complete"], true);
    assert_eq!(prepared["data"]["required_specs"], serde_json::json!([]));
}

#[tokio::test]
async fn prepare_includes_only_the_selected_workstreams_required_specs() {
    let (_guard, _, _, store) = setup_with_specs(vec![
        awr_team_pg::SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"Public API contract".to_vec(),
        },
        awr_team_pg::SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"PRIVATE peer contract".to_vec(),
        },
    ])
    .await;
    let prepared = prepare(&store, A, "a").await;
    assert_eq!(prepared["data"]["context_complete"], true);
    assert_eq!(
        prepared["data"]["required_specs"][0]["path"],
        "docs/alpha.md"
    );
    assert_eq!(
        prepared["data"]["required_specs"][0]["text"],
        "Public API contract"
    );
    assert!(!prepared.to_string().contains("PRIVATE"));
    assert_eq!(
        prepared["data"]["visible_contract"]["scope_paths"],
        serde_json::json!(["src"])
    );
}

#[tokio::test]
async fn missing_declared_spec_still_blocks_context_even_when_code_is_not_created() {
    let (_guard, _, _, store) = setup_with_specs(vec![]).await;
    let prepared = prepare(&store, A, "a").await;
    assert_eq!(prepared["data"]["context_complete"], false);
    assert_eq!(
        prepared["data"]["completeness_reasons"],
        serde_json::json!(["required_spec_missing"])
    );
    assert_eq!(
        prepared["data"]["authorized_readable_refs"][0]["path"],
        "docs/alpha.md"
    );
}
