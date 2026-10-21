#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::PgError;
use fixture::*;
use serde_json::{Value, json};

async fn counts(admin: &tokio_postgres::Client) -> Value {
    let r = admin.query_one("SELECT (SELECT count(*) FROM awr_team.sessions),
        (SELECT count(*) FROM awr_team.checkpoints),(SELECT count(*) FROM awr_team.operations),
        (SELECT count(*) FROM awr_team.events),project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
        &[&TENANT,&PROJECT]).await.unwrap();
    json!((0..5).map(|i| r.get::<_, i64>(i)).collect::<Vec<_>>())
}

async fn session_end_snapshot(admin: &tokio_postgres::Client, work: &str, session: &str) -> Value {
    admin
        .query_one(
            "SELECT jsonb_build_object(
                'session',(SELECT jsonb_build_object('state',state,'version',session_version)
                    FROM awr_team.sessions WHERE tenant_id=$1 AND project_id=$2 AND id=$3),
                'claims',(SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'state',state,'fence',fence,'lease_version',lease_version) ORDER BY id),'[]'::jsonb)
                    FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND work_id=$4),
                'runtime',(SELECT jsonb_build_object('state',state,'work_version',work_version,
                    'last_fence',last_fence,'recovery_blocked',recovery_blocked,
                    'recovery_execution_id',recovery_execution_id,'recovery_receipt_id',recovery_receipt_id)
                    FROM awr_team.work_runtime WHERE tenant_id=$1 AND project_id=$2
                      AND scope_id='main' AND work_id=$4),
                'executions',(SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'state',state,'version',execution_version,'terminal_reported',terminal_reported)
                    ORDER BY id),'[]'::jsonb) FROM awr_team.executions
                    WHERE tenant_id=$1 AND project_id=$2 AND work_id=$4),
                'waits',(SELECT COALESCE(jsonb_agg(jsonb_build_object('state',state) ORDER BY id),'[]'::jsonb)
                    FROM awr_team.wait_items WHERE tenant_id=$1 AND project_id=$2 AND work_id=$4),
                'reservations',(SELECT COALESCE(jsonb_agg(jsonb_build_object('state',state) ORDER BY id),'[]'::jsonb)
                    FROM awr_team.resource_reservations WHERE tenant_id=$1 AND project_id=$2 AND work_id=$4),
                'operation_count',(SELECT count(*) FROM awr_team.operations
                    WHERE tenant_id=$1 AND project_id=$2),
                'event_count',(SELECT count(*) FROM awr_team.events
                    WHERE tenant_id=$1 AND project_id=$2),
                'project_revision',(SELECT project_revision FROM awr_team.projects
                    WHERE tenant_id=$1 AND id=$2))",
            &[&TENANT, &PROJECT, &session, &work],
        )
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn authenticated_session_checkpoint_end_and_replay_commit_attribution_atomically() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let start = command(
        &prepared,
        "start-1",
        "session.start",
        json!({"conversation_id":"conversation-1"}),
    );
    let result = commands
        .execute(TENANT, PROJECT, A, start.clone())
        .await
        .unwrap();
    let id = result["receipt"]["data"]["session_id"].as_str().unwrap();
    assert_eq!(result["receipt"]["execution_authorized"], false);
    let once = counts(&admin).await;
    let replay = commands
        .execute(TENANT, PROJECT, A, start.clone())
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(result["receipt"], replay["receipt"]);
    assert_eq!(counts(&admin).await, once);
    let mut changed = start;
    changed.args["conversation_id"] = json!("different-intent");
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    let current = prepare(&store, A, "a").await;
    let checkpoint = command(
        &current,
        "checkpoint-1",
        "session.checkpoint",
        json!({"session_id":id,"expected_session_version":"1",
        "context_hash":current["data"]["context_hash"],"next_action":"Verify the implementation","open_loops":["review pending"]}),
    );
    let saved = commands
        .execute(TENANT, PROJECT, A, checkpoint.clone())
        .await
        .unwrap();
    assert_eq!(saved["receipt"]["data"]["session_version"], "2");
    assert_eq!(
        commands
            .execute(TENANT, PROJECT, A, checkpoint)
            .await
            .unwrap()["replayed"],
        true
    );
    let mut q = query("session.inspect");
    q.session_id = Some(id.into());
    let recovery = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(
        recovery["data"]["items"][0]["next_action"],
        "Verify the implementation"
    );
    assert_eq!(
        recovery["data"]["items"][0]["contract_matches_current"],
        true
    );
    let current = prepare(&store, A, "a").await;
    let end = command(
        &current,
        "end-1",
        "session.end",
        json!({"session_id":id,"expected_session_version":"2"}),
    );
    let ended = commands.execute(TENANT, PROJECT, A, end).await.unwrap();
    assert_eq!(ended["receipt"]["data"]["state"], "ended");
    let r = admin
        .query_one(
            "SELECT s.workstream_id,s.ownership_version,s.actor_id,s.client_id,
        (SELECT count(*) FROM awr_team.events WHERE work_id='a' AND workstream_id=s.workstream_id),
        (SELECT count(*) FROM awr_team.claims),(SELECT count(*) FROM awr_team.executions)
        FROM awr_team.sessions s WHERE id=$1",
            &[&id],
        )
        .await
        .unwrap();
    assert_eq!(r.get::<_, String>(0), awr_core::Id::from(1).to_string());
    assert_eq!(r.get::<_, i64>(1), 1);
    assert_eq!(r.get::<_, String>(2), "agent");
    assert_eq!(r.get::<_, String>(3), "cli-a");
    assert_eq!(r.get::<_, i64>(4), 4);
    assert_eq!(r.get::<_, i64>(5), 0);
    assert_eq!(r.get::<_, i64>(6), 0);
}

#[tokio::test]
async fn writes_require_live_write_grants_and_the_exact_credential_client() {
    let (_guard, admin, _, store) = setup().await;
    let commands = store.commands();
    let before = counts(&admin).await;
    let c = command(
        &prepare(&store, A, "a").await,
        "start",
        "session.start",
        json!({"conversation_id":"test"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, c.clone()).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        commands.execute(TENANT, PROJECT, B, c.clone()).await,
        Err(PgError::Forbidden)
    ));
    enable_writes(&admin).await;
    // A reader membership cannot turn a stored write bit into write authority.
    admin.batch_execute("UPDATE awr_team.project_memberships SET role='reader',membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, c.clone()).await,
        Err(PgError::Forbidden)
    ));
    admin.batch_execute("UPDATE awr_team.project_memberships SET role='worker',membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
    // Even an explicit grant on the same stream cannot mutate another client's session.
    let mut end = command(
        &prepare(&store, A, "a").await,
        "end",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"1"}),
    );
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'agent','cli-b',$3,1,true,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    assert!(matches!(
        commands.execute(TENANT, PROJECT, B, end.clone()).await,
        Err(PgError::Forbidden)
    ));
    end.args["session_id"] = json!("absent");
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, end).await,
        Err(PgError::Forbidden)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, c).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(counts(&admin).await, before);
}

#[tokio::test]
async fn stale_preconditions_invalid_context_and_forged_fields_do_not_write() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let start = command(
        &prepared,
        "start",
        "session.start",
        json!({"conversation_id":"test"}),
    );
    // Audit cursor is not business CAS; stale project revision alone must not block.
    let mut stale_cursor = serde_json::to_value(&start).unwrap();
    stale_cursor["expected_project_revision"] = json!("0");
    assert!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                serde_json::from_value(stale_cursor).unwrap(),
            )
            .await
            .is_ok()
    );
    let before = counts(&admin).await;
    for field in [
        "expected_authority_version",
        "expected_ownership_version",
        "expected_contract_hash",
        "coordinator_epoch",
    ] {
        let mut value = serde_json::to_value(&start).unwrap();
        value["request_id"] = json!(format!("stale-{field}"));
        value[field] = if field == "expected_contract_hash" {
            json!("0".repeat(64))
        } else {
            json!("999")
        };
        assert!(
            commands
                .execute(TENANT, PROJECT, A, serde_json::from_value(value).unwrap())
                .await
                .is_err()
        );
    }
    for field in ["actor_id", "client_id", "tenant_id", "grants"] {
        let mut value = serde_json::to_value(&start).unwrap();
        value[field] = json!("forged");
        assert!(serde_json::from_value::<awr_team_pg::WorkstreamCommand>(value).is_err());
        let mut c = start.clone();
        c.args[field] = json!("forged");
        assert!(matches!(
            commands.execute(TENANT, PROJECT, A, c).await,
            Err(PgError::Protocol(_))
        ));
    }
    let wrong_context = command(
        &prepared,
        "cp",
        "session.checkpoint",
        json!({"session_id":"session-a","expected_session_version":"1",
        "context_hash":"0".repeat(64),"next_action":"continue","open_loops":[]}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, wrong_context).await,
        Err(PgError::PreconditionsChanged)
    ));
    let old_session = command(
        &prepared,
        "end",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"2"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, old_session).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(counts(&admin).await, before);
}

#[tokio::test]
async fn inspect_recovery_guidance_keeps_missing_unknown_and_receipts_identity_bound() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let start = command(
        &prepare(&store, A, "a").await,
        "lost",
        "session.start",
        json!({"conversation_id":"test"}),
    );
    commands.execute(TENANT, PROJECT, A, start).await.unwrap();
    let mut q = query("command.inspect");
    q.work_id = Some("a".into());
    q.request_id = Some("lost".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["state"],
        "committed"
    );
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-b',$3,1,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    assert_eq!(
        store.query(TENANT, PROJECT, B, q.clone()).await.unwrap()["data"]["state"],
        "unknown"
    );
    let mut missing = q.clone();
    missing.request_id = Some("not-submitted".into());
    let unknown = store
        .query(TENANT, PROJECT, A, missing.clone())
        .await
        .unwrap();
    assert_eq!(unknown["data"]["state"], "unknown");
    assert_eq!(unknown["data"]["retry"], "conditional");
    let guidance = &unknown["data"]["action_guidance"];
    assert!(
        guidance["when"]
            .as_str()
            .unwrap()
            .contains("missing or uncertain")
    );
    assert!(
        guidance["action"]
            .as_str()
            .unwrap()
            .contains("explicit input rejection")
    );
    assert!(
        guidance["action"]
            .as_str()
            .unwrap()
            .contains("Never repeat external effects")
    );
    assert!(!guidance["because"].is_null() && !guidance["recheck_on"].is_null());
    assert!(unknown["data"].to_string().len() < 800);

    let before = counts(&admin).await;
    let invalid = command(
        &prepare(&store, A, "a").await,
        "rejected",
        "session.start",
        json!({}),
    );
    assert!(
        commands
            .execute(TENANT, PROJECT, A, invalid)
            .await
            .unwrap_err()
            .is_missing_session_start_conversation_id()
    );
    missing.request_id = Some("rejected".into());
    let rejected = store.query(TENANT, PROJECT, A, missing).await.unwrap();
    assert_eq!(rejected["data"]["state"], "unknown");
    assert_eq!(rejected["data"]["action_guidance"], *guidance);
    assert_eq!(counts(&admin).await, before);
    admin.batch_execute("UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=2 WHERE work_id='a'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn concurrent_identical_requests_replay_and_conflicting_intents_do_not_lose_updates() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = std::sync::Arc::new(store.commands());
    let start = command(
        &prepare(&store, A, "a").await,
        "once",
        "session.start",
        json!({"conversation_id":"test"}),
    );
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, start.clone()),
        commands.execute(TENANT, PROJECT, A, start)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a["receipt"], b["receipt"]);
    assert_ne!(a["replayed"], b["replayed"]);
    let id = a["receipt"]["data"]["session_id"].as_str().unwrap();
    let prepared = prepare(&store, A, "a").await;
    let one = command(
        &prepared,
        "cp1",
        "session.checkpoint",
        json!({"session_id":id,"expected_session_version":"1","context_hash":prepared["data"]["context_hash"],"next_action":"first","open_loops":[]}),
    );
    let mut two = one.clone();
    two.request_id = "cp2".into();
    two.args["next_action"] = json!("second");
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, one),
        commands.execute(TENANT, PROJECT, A, two)
    );
    assert!(a.is_ok() != b.is_ok());
    assert!(matches!(
        a.err().or(b.err()),
        Some(PgError::PreconditionsChanged)
    ));
    let r = admin
        .query_one(
            "SELECT session_version,(SELECT count(*) FROM awr_team.checkpoints WHERE session_id=$1)
        FROM awr_team.sessions WHERE id=$1",
            &[&id],
        )
        .await
        .unwrap();
    assert_eq!(r.get::<_, i64>(0), 2);
    assert_eq!(r.get::<_, i64>(1), 1);
}

#[tokio::test]
async fn event_failure_rolls_back_session_checkpoint_revision_and_receipt() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let commands = store.commands();
    let c = command(
        &prepared,
        "cp",
        "session.checkpoint",
        json!({"session_id":"session-a","expected_session_version":"1",
        "context_hash":prepared["data"]["context_hash"],"next_action":"continue","open_loops":[]}),
    );
    let before = counts(&admin).await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_session_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic failure'; END $$;
        CREATE TRIGGER reject_session_event BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_session_event();").await.unwrap();
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, c.clone()).await,
        Err(PgError::Db(_))
    ));
    assert_eq!(counts(&admin).await, before);
    let session=admin.query_one("SELECT session_version,latest_checkpoint_id FROM awr_team.sessions WHERE id='session-a'",&[]).await.unwrap();
    assert_eq!(session.get::<_, i64>(0), 1);
    assert_eq!(session.get::<_, String>(1), "cp-session-a");
    admin
        .batch_execute("DROP TRIGGER reject_session_event ON awr_team.events")
        .await
        .unwrap();
    assert_eq!(
        commands.execute(TENANT, PROJECT, A, c).await.unwrap()["replayed"],
        false
    );
}

#[tokio::test]
async fn frozen_projects_block_new_writes_but_allow_reading_committed_receipts() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let c = command(
        &prepare(&store, A, "a").await,
        "start",
        "session.start",
        json!({"conversation_id":"test"}),
    );
    commands
        .execute(TENANT, PROJECT, A, c.clone())
        .await
        .unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET status='frozen' WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    assert_eq!(
        commands.execute(TENANT, PROJECT, A, c).await.unwrap()["replayed"],
        true
    );
    let c = command(
        &prepare(&store, A, "a").await,
        "new",
        "session.start",
        json!({"conversation_id":"another"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, c).await,
        Err(PgError::ProjectNotAvailable)
    ));
}

#[tokio::test]
async fn unknown_execution_blocks_session_end_without_erasing_recovery_responsibility() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    admin.batch_execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,session_id,fence,contract_hash,executor_actor_id,state)
        VALUES('reader-tenant','reader-project','unknown','a','session-a',1,'contract','agent','unknown')").await.unwrap();
    let c = command(
        &prepared,
        "end",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"1"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, c).await,
        Err(PgError::RecoveryBlocked)
    ));
    let r=admin.query_one("SELECT s.state,e.state FROM awr_team.sessions s JOIN awr_team.executions e ON e.session_id=s.id WHERE s.id='session-a'",&[]).await.unwrap();
    assert_eq!(r.get::<_, String>(0), "active");
    assert_eq!(r.get::<_, String>(1), "unknown");
}

#[tokio::test]
async fn session_end_preserves_recovery_precedence_without_adding_recovery_only_guards() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let started = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepare(&store, A, "c").await,
                "recovery-only-session",
                "session.start",
                json!({"conversation_id":"recovery-only-session"}),
            ),
        )
        .await
        .unwrap();
    let session = started["receipt"]["data"]["session_id"].as_str().unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,session_id,fence,
                contract_hash,executor_actor_id,state,terminal_reported)
             VALUES($1,$2,'recovery-only-terminal','c',$3,1,'contract','agent','succeeded',true)",
            &[&TENANT, &PROJECT, &session],
        )
        .await
        .unwrap();
    admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,recovery_blocked)
        VALUES('reader-tenant','reader-project','main','c','unclaimed',true)
        ON CONFLICT (tenant_id,project_id,scope_id,work_id) DO UPDATE SET recovery_blocked=true;
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','recovery-only-reservation','c','named','recovery-only','unknown')").await.unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state)
             VALUES($1,$2,'recovery-priority-wait',$3,'c','wait','open')",
            &[&TENANT, &PROJECT, &session],
        )
        .await
        .unwrap();

    // The open wait is an original guard, so unresolved effects may safely take
    // diagnostic precedence without changing the command's allow/deny set.
    let before = session_end_snapshot(&admin, "c", session).await;
    let recovery = command(
        &prepare(&store, A, "c").await,
        "recovery-priority-end",
        "session.end",
        json!({"session_id":session,"expected_session_version":"1"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, recovery).await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(session_end_snapshot(&admin, "c", session).await, before);

    admin
        .batch_execute(
            "UPDATE awr_team.wait_items SET state='replied' WHERE id='recovery-priority-wait'",
        )
        .await
        .unwrap();

    // With the original wait guard gone, recovery-only facts must not become a
    // new reason to reject session.end.
    let ended = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepare(&store, A, "c").await,
                "recovery-only-end",
                "session.end",
                json!({"session_id":session,"expected_session_version":"1"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(ended["receipt"]["data"]["state"], "ended");
    let after = session_end_snapshot(&admin, "c", session).await;
    assert_eq!(after["session"], json!({"state":"ended","version":2}));
    assert_eq!(after["runtime"]["recovery_blocked"], true);
    assert_eq!(after["executions"][0]["state"], "succeeded");
    assert_eq!(after["reservations"][0]["state"], "unknown");
}

#[tokio::test]
async fn session_end_classifies_blockers_before_explicit_release_without_writes() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let acquired = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "end-guidance-acquire",
                "claim.acquire",
                json!({"session_id":"session-a","expected_session_version":"1",
                    "expected_work_version":"0","ttl_seconds":60}),
            ),
        )
        .await
        .unwrap();
    let claim = acquired["receipt"]["data"].clone();

    let before = session_end_snapshot(&admin, "a", "session-a").await;
    let active = command(
        &prepare(&store, A, "a").await,
        "end-active-claim",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"1"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, active).await,
        Err(PgError::ClaimHeld)
    ));
    assert_eq!(session_end_snapshot(&admin, "a", "session-a").await, before);

    admin.batch_execute("INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state)
        VALUES('reader-tenant','reader-project','end-wait','session-a','a','synthetic wait','open')").await.unwrap();
    let before = session_end_snapshot(&admin, "a", "session-a").await;
    let waiting = command(
        &prepare(&store, A, "a").await,
        "end-open-wait",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"1"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, waiting).await,
        Err(PgError::WaitOpen)
    ));
    assert_eq!(session_end_snapshot(&admin, "a", "session-a").await, before);

    admin.batch_execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,session_id,fence,contract_hash,executor_actor_id,state)
        VALUES('reader-tenant','reader-project','end-execution','a','session-a',1,'contract','agent','unknown')").await.unwrap();
    let before = session_end_snapshot(&admin, "a", "session-a").await;
    let executing = command(
        &prepare(&store, A, "a").await,
        "end-unresolved-execution",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"1"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, executing).await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(session_end_snapshot(&admin, "a", "session-a").await, before);

    admin.batch_execute("UPDATE awr_team.executions SET state='succeeded',terminal_reported=true WHERE id='end-execution';
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','end-unknown-reservation','a','named','end-unknown','unknown')").await.unwrap();
    let before = session_end_snapshot(&admin, "a", "session-a").await;
    let recovery = command(
        &prepare(&store, A, "a").await,
        "end-unknown-reservation",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"1"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, recovery).await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(session_end_snapshot(&admin, "a", "session-a").await, before);

    admin.batch_execute("UPDATE awr_team.resource_reservations SET state='released' WHERE id='end-unknown-reservation';
        UPDATE awr_team.work_runtime SET recovery_blocked=true WHERE work_id='a'").await.unwrap();
    let before = session_end_snapshot(&admin, "a", "session-a").await;
    let recovery = command(
        &prepare(&store, A, "a").await,
        "end-recovery-barrier",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"1"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, recovery).await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(session_end_snapshot(&admin, "a", "session-a").await, before);

    admin
        .batch_execute(
            "UPDATE awr_team.work_runtime SET recovery_blocked=false WHERE work_id='a';
        UPDATE awr_team.wait_items SET state='replied' WHERE id='end-wait'",
        )
        .await
        .unwrap();
    let before = session_end_snapshot(&admin, "a", "session-a").await;
    let active = command(
        &prepare(&store, A, "a").await,
        "end-after-settlement",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"1"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, active).await,
        Err(PgError::ClaimHeld)
    ));
    assert_eq!(session_end_snapshot(&admin, "a", "session-a").await, before);

    let released = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepare(&store, A, "a").await,
                "end-guidance-release",
                "claim.release",
                json!({"session_id":"session-a","expected_session_version":"1",
                    "claim_id":claim["claim_id"],"expected_fence":claim["fence"],
                    "expected_lease_version":claim["lease_version"]}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(released["receipt"]["data"]["state"], "released");
    let ended = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepare(&store, A, "a").await,
                "end-after-release",
                "session.end",
                json!({"session_id":"session-a","expected_session_version":"1"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(ended["receipt"]["data"]["state"], "ended");
    let states = admin
        .query_one(
            "SELECT s.state,c.state,(SELECT count(*) FROM awr_team.executions WHERE id='end-execution' AND state='succeeded')
             FROM awr_team.sessions s JOIN awr_team.claims c ON c.session_id=s.id
             WHERE s.id='session-a'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(states.get::<_, String>(0), "ended");
    assert_eq!(states.get::<_, String>(1), "released");
    assert_eq!(states.get::<_, i64>(2), 1);
}

#[tokio::test]
async fn paused_stream_can_save_recovery_and_end_but_cannot_start_new_sessions() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let mut catalog: awr_core::WorkstreamCatalog = serde_json::from_value(
        admin
            .query_one(
                "SELECT catalog_json FROM awr_team.workstream_catalogs WHERE tenant_id=$1",
                &[&TENANT],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    let alpha = catalog
        .workstreams
        .iter_mut()
        .find(|s| s.id == awr_core::Id::from(1))
        .unwrap();
    alpha.state = awr_core::WorkstreamState::Paused;
    alpha.authority_version = 2;
    admin
        .execute(
            "UPDATE awr_team.workstream_catalogs SET catalog_json=$1 WHERE tenant_id=$2",
            &[&serde_json::to_value(catalog).unwrap(), &TENANT],
        )
        .await
        .unwrap();
    admin.batch_execute("UPDATE awr_team.workstream_grants SET authority_version=2,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let start = command(
        &prepared,
        "paused-start",
        "session.start",
        json!({"conversation_id":"test"}),
    );
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, start).await,
        Err(PgError::Workstream(awr_core::WorkstreamError::Inactive))
    ));
    let checkpoint = command(
        &prepared,
        "paused-checkpoint",
        "session.checkpoint",
        json!({"session_id":"session-a","expected_session_version":"1",
        "context_hash":prepared["data"]["context_hash"],"next_action":"Preserve recovery notes until resumed","open_loops":["scope paused"]}),
    );
    commands
        .execute(TENANT, PROJECT, A, checkpoint)
        .await
        .unwrap();
    let end = command(
        &prepare(&store, A, "a").await,
        "paused-end",
        "session.end",
        json!({"session_id":"session-a","expected_session_version":"2"}),
    );
    assert_eq!(
        commands.execute(TENANT, PROJECT, A, end).await.unwrap()["receipt"]["data"]["state"],
        "ended"
    );
}

#[tokio::test]
async fn in_flight_write_waits_for_grant_revocation_and_cannot_use_previous_permission() {
    let (_guard, mut admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let command = command(
        &prepare(&store, A, "a").await,
        "racing",
        "session.start",
        json!({"conversation_id":"test"}),
    );
    let before = counts(&admin).await;
    let commands = store.commands();
    let observer = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let revoke = admin.transaction().await.unwrap();
    revoke.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    let write = tokio::spawn(async move { commands.execute(TENANT, PROJECT, A, command).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = observer
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND query LIKE '%ORDER BY workstream_id FOR SHARE%')",
                    &[],
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
    .expect("writer must wait on the real grant row");
    revoke.commit().await.unwrap();
    assert!(matches!(write.await.unwrap(), Err(PgError::Forbidden)));
    assert_eq!(counts(&admin).await, before);
}

#[tokio::test]
async fn unrelated_workstream_commands_do_not_conflict_on_audit_cursor() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1
             WHERE client_id IN ('cli-a','cli-b')",
        )
        .await
        .unwrap();
    // Grant B write on stream 2 (b-private).
    let commands = std::sync::Arc::new(store.commands());
    let prepared_a = prepare(&store, A, "a").await;
    let prepared_b = prepare(&store, B, "b-private").await;
    // Advance audit cursor with an unrelated write on work a.
    let first = command(
        &prepared_a,
        "a-start",
        "session.start",
        json!({"conversation_id":"alpha-1"}),
    );
    commands.execute(TENANT, PROJECT, A, first).await.unwrap();
    // Stale prepare for work b must still commit; project_revision moved.
    let stale_b = command(
        &prepared_b,
        "b-start",
        "session.start",
        json!({"conversation_id":"beta-1"}),
    );
    let result = commands
        .execute(TENANT, PROJECT, B, stale_b)
        .await
        .expect("unrelated audit cursor must not block work b");
    assert_eq!(result["replayed"], false);
    let revs = admin
        .query(
            "SELECT project_revision FROM awr_team.events
             WHERE tenant_id=$1 AND project_id=$2 ORDER BY project_revision,event_index,id",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    let mut cursor = 0i64;
    for row in revs {
        let rev: i64 = row.get(0);
        assert!(rev >= cursor, "audit cursors must remain ordered");
        cursor = rev;
    }
    // Concurrent same-task conflicting checkpoints: exactly one writer wins.
    let sid: String = admin
        .query_one(
            "SELECT id FROM awr_team.sessions WHERE conversation_id='alpha-1'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let prepared = prepare(&store, A, "a").await;
    let one = command(
        &prepared,
        "cp-race-1",
        "session.checkpoint",
        json!({"session_id":sid,"expected_session_version":"1",
            "context_hash":prepared["data"]["context_hash"],"next_action":"one","open_loops":[]}),
    );
    let mut two = one.clone();
    two.request_id = "cp-race-2".into();
    two.args["next_action"] = json!("two");
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, one),
        commands.execute(TENANT, PROJECT, A, two)
    );
    assert!(
        a.is_ok() != b.is_ok(),
        "same-task race must not lose both updates"
    );
    let row = admin
        .query_one(
            "SELECT session_version,(SELECT count(*) FROM awr_team.checkpoints WHERE session_id=$1)
             FROM awr_team.sessions WHERE id=$1",
            &[&sid],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, i64>(0), 2);
    assert_eq!(row.get::<_, i64>(1), 1);
}
