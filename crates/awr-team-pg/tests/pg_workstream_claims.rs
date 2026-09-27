#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::{PgError, WorkstreamCommand, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

async fn acquire(store: &WorkstreamReadStore, request: &str) -> WorkstreamCommand {
    let p = prepare(store, A, "a").await;
    command(
        &p,
        request,
        "claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1",
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":60}),
    )
}
fn change(p: &Value, claim: &Value, request: &str, op: &str) -> WorkstreamCommand {
    let mut args = json!({"session_id":claim["session_id"],"expected_session_version":"1",
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"]});
    if op == "claim.renew" {
        args["ttl_seconds"] = json!(120);
    }
    command(p, request, op, args)
}
async fn inspect(store: &WorkstreamReadStore, token: &str, claim: &Value) -> Value {
    let mut q = query("claim.inspect");
    q.work_id = Some("a".into());
    q.claim_id = Some(claim["claim_id"].as_str().unwrap().into());
    store.query(TENANT, PROJECT, token, q).await.unwrap()["data"].clone()
}
async fn snapshot(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'claims',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM awr_team.claims c),
        'work',(SELECT jsonb_agg(to_jsonb(w) ORDER BY work_id) FROM awr_team.work_runtime w),
        'executions',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.executions e),
        'resources',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.resource_reservations r),
        'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.events e),
        'operations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM awr_team.operations o),
        'revision',(SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2))",
        &[&TENANT,&PROJECT]).await.unwrap().get(0)
}

#[tokio::test]
async fn owned_leases_renew_release_and_replay_only_historical_receipts() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let request = acquire(&store, "take").await;
    let result = commands
        .execute(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    let claim = result["receipt"]["data"].clone();
    assert_eq!(claim["fence"], "1");
    assert_eq!(claim["work_version"], "1");
    assert_eq!(claim["lease_state_basis"], "at_commit");
    assert_eq!(result["receipt"]["execution_authorized"], false);
    assert_eq!(inspect(&store, A, &claim).await["lease_live"], true);
    let recorded = snapshot(&admin).await;
    assert_eq!(
        commands
            .execute(TENANT, PROJECT, A, request.clone())
            .await
            .unwrap()["receipt"],
        result["receipt"]
    );
    assert_eq!(snapshot(&admin).await, recorded);
    let renewed = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            change(
                &prepare(&store, A, "a").await,
                &claim,
                "renew",
                "claim.renew",
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    assert_eq!(renewed["lease_version"], "2");
    assert_eq!(renewed["fence"], "1");
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                change(
                    &prepare(&store, A, "a").await,
                    &claim,
                    "stale-renew",
                    "claim.renew"
                )
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let released = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            change(
                &prepare(&store, A, "a").await,
                &renewed,
                "release",
                "claim.release",
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    assert_eq!(released["state"], "released");
    assert_eq!(released["work_version"], "2");
    assert_eq!(released["resource_release_performed"], false);
    let before = snapshot(&admin).await;
    let replay = commands.execute(TENANT, PROJECT, A, request).await.unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], result["receipt"]);
    assert_eq!(inspect(&store, A, &claim).await["lease_live"], false);
    assert_eq!(snapshot(&admin).await, before);
    let r = admin
        .query_one(
            "SELECT workstream_id,ownership_version,coordinator_epoch FROM awr_team.claims",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(r.get::<_, String>(0), awr_core::Id::from(1).to_string());
    assert_eq!(r.get::<_, i64>(1), 1);
    assert_eq!(r.get::<_, String>(2), "epoch-a");
    assert_eq!(before["work"][0]["state"], "unclaimed");
}

#[tokio::test]
async fn same_actor_other_client_cannot_use_a_session_or_steal_its_live_claim() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'agent','cli-b',$3,1,true,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let commands = store.commands();
    let request = acquire(&store, "take").await;
    assert!(matches!(
        commands.execute(TENANT, PROJECT, B, request.clone()).await,
        Err(PgError::Forbidden)
    ));
    let claim =
        commands.execute(TENANT, PROJECT, A, request).await.unwrap()["receipt"]["data"].clone();
    assert_eq!(inspect(&store, B, &claim).await["owned_by_client"], false);
    for op in ["claim.renew", "claim.release"] {
        let before = snapshot(&admin).await;
        assert!(matches!(
            commands
                .execute(
                    TENANT,
                    PROJECT,
                    B,
                    change(&prepare(&store, B, "a").await, &claim, op, op)
                )
                .await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, before);
    }
    let start = command(
        &prepare(&store, B, "a").await,
        "other-session",
        "session.start",
        json!({"conversation_id":"colleague"}),
    );
    let other =
        commands.execute(TENANT, PROJECT, B, start).await.unwrap()["receipt"]["data"]["session_id"]
            .clone();
    let mut take = acquire(&store, "other-take").await;
    take.args["session_id"] = other;
    assert!(matches!(
        commands.execute(TENANT, PROJECT, B, take).await,
        Err(PgError::ClaimHeld)
    ));
    let mut q = query("claim.inspect");
    q.work_id = Some("b-private".into());
    q.claim_id = Some(claim["claim_id"].as_str().unwrap().into());
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn concurrent_claims_have_one_owner_and_exact_retries_do_not_advance_fences() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let request = acquire(&store, "once").await;
    let (one, two) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, request.clone()),
        commands.execute(TENANT, PROJECT, A, request)
    );
    let (one, two) = (one.unwrap(), two.unwrap());
    assert_eq!(one["receipt"], two["receipt"]);
    assert_ne!(one["replayed"], two["replayed"]);
    let claim = one["receipt"]["data"].clone();
    commands
        .execute(
            TENANT,
            PROJECT,
            A,
            change(
                &prepare(&store, A, "a").await,
                &claim,
                "release",
                "claim.release",
            ),
        )
        .await
        .unwrap();
    let first = acquire(&store, "first").await;
    let mut second = first.clone();
    second.request_id = "second".into();
    let (one, two) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, first),
        commands.execute(TENANT, PROJECT, A, second)
    );
    assert_ne!(one.is_ok(), two.is_ok());
    assert!(matches!(
        one.err().or(two.err()),
        Some(PgError::PreconditionsChanged)
    ));
    assert!(matches!(
        commands
            .execute(TENANT, PROJECT, A, acquire(&store, "third").await)
            .await,
        Err(PgError::ClaimHeld)
    ));
    let r=admin.query_one("SELECT last_fence,(SELECT count(*) FROM awr_team.claims WHERE state='active') FROM awr_team.work_runtime",&[]).await.unwrap();
    assert_eq!(r.get::<_, i64>(0), 2);
    assert_eq!(r.get::<_, i64>(1), 1);
}

#[tokio::test]
async fn expired_lease_cannot_renew_and_safe_reacquisition_invalidates_old_fence() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let claim = commands
        .execute(TENANT, PROJECT, A, acquire(&store, "first").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                change(
                    &prepare(&store, A, "a").await,
                    &claim,
                    "late-renew",
                    "claim.renew"
                )
            )
            .await,
        Err(PgError::LeaseExpired)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let next = commands
        .execute(TENANT, PROJECT, A, acquire(&store, "next").await)
        .await
        .unwrap();
    assert_eq!(next["receipt"]["data"]["fence"], "2");
    assert_ne!(next["receipt"]["data"]["claim_id"], claim["claim_id"]);
    assert_eq!(inspect(&store, A, &claim).await["lease_live"], false);
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                change(
                    &prepare(&store, A, "a").await,
                    &claim,
                    "old-release",
                    "claim.release"
                )
            )
            .await,
        Err(PgError::StaleFence)
    ));
    let events=admin.query("SELECT event_index,event_type,workstream_id FROM awr_team.events WHERE project_revision=$1 ORDER BY event_index",
        &[&next["receipt"]["committed_project_revision"].as_str().unwrap().parse::<i64>().unwrap()]).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].get::<_, String>(1), "claim.expired");
    assert_eq!(events[1].get::<_, String>(1), "claim.acquire");
    assert_eq!(
        events[0].get::<_, String>(2),
        awr_core::Id::from(1).to_string()
    );
}

#[tokio::test]
async fn expiry_and_release_never_erase_nonterminal_effects_or_unknown_resources() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let claim = commands
        .execute(TENANT, PROJECT, A, acquire(&store, "take").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    admin.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second';
        INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,session_id,fence,contract_hash,executor_actor_id,state)
        VALUES('reader-tenant','reader-project','effect','a','session-a',1,'contract','agent','prepared')").await.unwrap();
    for state in ["prepared", "queued", "accepted", "running", "unknown"] {
        admin
            .execute("UPDATE awr_team.executions SET state=$1", &[&state])
            .await
            .unwrap();
        let before = snapshot(&admin).await;
        assert!(matches!(
            commands
                .execute(TENANT, PROJECT, A, acquire(&store, "takeover").await)
                .await,
            Err(PgError::RecoveryBlocked)
        ));
        assert!(matches!(
            commands
                .execute(
                    TENANT,
                    PROJECT,
                    A,
                    change(
                        &prepare(&store, A, "a").await,
                        &claim,
                        "release",
                        "claim.release"
                    )
                )
                .await,
            Err(PgError::RecoveryBlocked)
        ));
        assert_eq!(snapshot(&admin).await, before);
    }
    admin.batch_execute("UPDATE awr_team.executions SET state='succeeded';
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','resource','a','named','shared-fixture','unknown')").await.unwrap();
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                acquire(&store, "resource-takeover").await
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                change(
                    &prepare(&store, A, "a").await,
                    &claim,
                    "resource-release",
                    "claim.release"
                )
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    admin.batch_execute("UPDATE awr_team.resource_reservations SET state='reserved'; UPDATE awr_team.work_runtime SET recovery_blocked=true").await.unwrap();
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                acquire(&store, "blocked-takeover").await
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    admin
        .batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=false")
        .await
        .unwrap();
    commands
        .execute(
            TENANT,
            PROJECT,
            A,
            change(
                &prepare(&store, A, "a").await,
                &claim,
                "safe-release",
                "claim.release",
            ),
        )
        .await
        .unwrap();
    let r = admin
        .query_one("SELECT state FROM awr_team.resource_reservations", &[])
        .await
        .unwrap();
    assert_eq!(r.get::<_, String>(0), "reserved");
}

#[tokio::test]
async fn request_bounds_epoch_changes_and_completed_work_fail_without_reinterpreting_ownership() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let request = acquire(&store, "take").await;
    let before = snapshot(&admin).await;
    for ttl in [json!(0), json!(-1), json!(3601), json!("60"), json!(1.5)] {
        let mut c = request.clone();
        c.args["ttl_seconds"] = ttl;
        assert!(matches!(
            commands.execute(TENANT, PROJECT, A, c).await,
            Err(PgError::Protocol(_))
        ));
    }
    for (field, value) in [
        ("expected_work_version", json!("01")),
        ("expected_session_version", json!("0")),
        ("actor_id", json!("agent")),
        ("workstream_id", json!("forged")),
    ] {
        let mut c = request.clone();
        c.args[field] = value;
        assert!(matches!(
            commands.execute(TENANT, PROJECT, A, c).await,
            Err(PgError::Protocol(_))
        ));
    }
    assert_eq!(snapshot(&admin).await, before);
    let claim =
        commands.execute(TENANT, PROJECT, A, request).await.unwrap()["receipt"]["data"].clone();
    admin.batch_execute("UPDATE awr_team.projects SET coordinator_epoch='new-epoch' WHERE tenant_id='reader-tenant'").await.unwrap();
    assert_eq!(
        inspect(&store, A, &claim).await["epoch_matches_current"],
        false
    );
    assert_eq!(inspect(&store, A, &claim).await["lease_live"], false);
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                change(
                    &prepare(&store, A, "a").await,
                    &claim,
                    "new-epoch-renew",
                    "claim.renew"
                )
            )
            .await,
        Err(PgError::EpochChanged)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    assert!(matches!(
        commands
            .execute(TENANT, PROJECT, A, acquire(&store, "epoch-takeover").await)
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    admin.batch_execute("UPDATE awr_team.projects SET coordinator_epoch='epoch-a' WHERE tenant_id='reader-tenant';
        INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,
            contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json)
        SELECT tenant_id,project_id,'completed-receipt',work_id,scope_id,contract_hash,
            'synthetic-result','synthetic-dependencies','synthetic-evidence','synthetic-review','[]'::jsonb
        FROM awr_team.work_contracts WHERE tenant_id='reader-tenant' AND work_id='a' AND scope_id='main';
        UPDATE awr_team.work_runtime SET state='completed',selected_completion_id='completed-receipt' WHERE work_id='a'")
        .await.unwrap();
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                acquire(&store, "completed-takeover").await
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
}

#[tokio::test]
async fn waits_frozen_projects_and_paused_scopes_preserve_coordination_barriers() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    admin.batch_execute("INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state)
        VALUES('reader-tenant','reader-project','wait','session-a','a','Review the draft?','open')").await.unwrap();
    assert!(matches!(
        commands
            .execute(TENANT, PROJECT, A, acquire(&store, "waiting").await)
            .await,
        Err(PgError::WaitOpen)
    ));
    admin
        .batch_execute("UPDATE awr_team.wait_items SET state='replied'")
        .await
        .unwrap();
    let claim = commands
        .execute(TENANT, PROJECT, A, acquire(&store, "take").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET status='frozen' WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                change(
                    &prepare(&store, A, "a").await,
                    &claim,
                    "frozen-release",
                    "claim.release"
                )
            )
            .await,
        Err(PgError::ProjectNotAvailable)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET status='active' WHERE tenant_id='reader-tenant'",
        )
        .await
        .unwrap();
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
    catalog
        .workstreams
        .iter_mut()
        .find(|s| s.id == awr_core::Id::from(1))
        .unwrap()
        .state = awr_core::WorkstreamState::Paused;
    admin
        .execute(
            "UPDATE awr_team.workstream_catalogs SET catalog_json=$1 WHERE tenant_id=$2",
            &[&json!(catalog), &TENANT],
        )
        .await
        .unwrap();
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                change(
                    &prepare(&store, A, "a").await,
                    &claim,
                    "paused-renew",
                    "claim.renew"
                )
            )
            .await,
        Err(PgError::Workstream(awr_core::WorkstreamError::Inactive))
    ));
    commands
        .execute(
            TENANT,
            PROJECT,
            A,
            change(
                &prepare(&store, A, "a").await,
                &claim,
                "paused-release",
                "claim.release",
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        commands
            .execute(TENANT, PROJECT, A, acquire(&store, "paused-take").await)
            .await,
        Err(PgError::Workstream(awr_core::WorkstreamError::Inactive))
    ));
}

#[tokio::test]
async fn failed_expiration_event_rolls_back_fence_claims_revision_and_operation_together() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    commands
        .execute(TENANT, PROJECT, A, acquire(&store, "first").await)
        .await
        .unwrap();
    admin.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second';
        CREATE FUNCTION awr_team.reject_claim_expiration() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
          IF NEW.event_type='claim.expired' THEN RAISE EXCEPTION 'synthetic expiration event failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER reject_claim_expiration BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_claim_expiration()").await.unwrap();
    let next = acquire(&store, "retry").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        commands.execute(TENANT, PROJECT, A, next.clone()).await,
        Err(PgError::Db(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute("DROP TRIGGER reject_claim_expiration ON awr_team.events")
        .await
        .unwrap();
    assert_eq!(
        commands.execute(TENANT, PROJECT, A, next).await.unwrap()["receipt"]["data"]["fence"],
        "2"
    );
}

#[tokio::test]
async fn claim_waiting_for_a_revoked_grant_never_enters_with_old_authority() {
    let (_guard, mut admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let c = acquire(&store, "revoked").await;
    let before = snapshot(&admin).await;
    let commands = store.commands();
    let observer = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let revoke = admin.transaction().await.unwrap();
    revoke.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    let write = tokio::spawn(async move { commands.execute(TENANT, PROJECT, A, c).await });
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
    .expect("command must be blocked on the actual revoked grant row");
    revoke.commit().await.unwrap();
    assert!(matches!(write.await.unwrap(), Err(PgError::Forbidden)));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn legacy_or_reassigned_active_rows_cannot_be_silently_adopted_after_expiry() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let claim = commands
        .execute(TENANT, PROJECT, A, acquire(&store, "take").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    for attribution in [
        "workstream_id=NULL,ownership_version=NULL,coordinator_epoch=NULL",
        "workstream_id='00000000000000000000000002',ownership_version=1,coordinator_epoch='epoch-a'",
        "workstream_id='00000000000000000000000001',ownership_version=2,coordinator_epoch='epoch-a'",
    ] {
        admin
            .batch_execute(&format!("UPDATE awr_team.claims SET {attribution}"))
            .await
            .unwrap();
        let before = snapshot(&admin).await;
        assert!(matches!(
            commands
                .execute(TENANT, PROJECT, A, acquire(&store, "takeover").await)
                .await,
            Err(PgError::RecoveryBlocked)
        ));
        let mut q = query("claim.inspect");
        q.work_id = Some("a".into());
        q.claim_id = Some(claim["claim_id"].as_str().unwrap().into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
        for op in ["claim.renew", "claim.release"] {
            assert!(matches!(
                commands
                    .execute(
                        TENANT,
                        PROJECT,
                        A,
                        change(&prepare(&store, A, "a").await, &claim, op, op)
                    )
                    .await,
                Err(PgError::Forbidden)
            ));
        }
        assert_eq!(snapshot(&admin).await, before);
    }
}

#[tokio::test]
async fn schema_eleven_is_atomic_and_preserves_unattributed_claim_history() {
    let (_guard, admin, _) = common::historical_team_schema(10).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('reader-tenant','Readers','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status) VALUES('reader-tenant','reader-project','p','team','epoch-a','active');
        INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
          VALUES('reader-tenant','reader-project','session-a','main','a','agent','cli-a','conversation','active');
        INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,expires_at,state)
          VALUES('reader-tenant','reader-project','history','main','a','session-a','agent',4,clock_timestamp()-interval '1 day','expired')").await.unwrap();
    let migration = include_str!("../migrations/20260921000011_workstream_claims.sql");
    let injected = migration.replace(
        "UPDATE awr_team.schema_state",
        "SELECT 1/0; UPDATE awr_team.schema_state",
    );
    assert!(admin.batch_execute(&injected).await.is_err());
    admin.batch_execute("ROLLBACK").await.unwrap();
    let r=admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),(SELECT count(*) FROM information_schema.columns
        WHERE table_schema='awr_team' AND table_name='claims' AND column_name='workstream_id')",&[]).await.unwrap();
    assert_eq!(r.get::<_, i32>(0), 10);
    assert_eq!(r.get::<_, i64>(1), 0);
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::check_schema(&admin).await.unwrap();
    let r=admin.query_one("SELECT scope_id,fence,state,workstream_id,ownership_version,coordinator_epoch FROM awr_team.claims WHERE id='history'",&[]).await.unwrap();
    assert_eq!(r.get::<_, String>(0), "main");
    assert_eq!(r.get::<_, i64>(1), 4);
    assert_eq!(r.get::<_, String>(2), "expired");
    assert_eq!(r.get::<_, Option<String>>(3), None);
    assert_eq!(r.get::<_, Option<i64>>(4), None);
    assert_eq!(r.get::<_, Option<String>>(5), None);
    assert!(
        admin
            .batch_execute("UPDATE awr_team.claims SET ownership_version=1 WHERE id='history'")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn claim_inspect_never_sets_execution_authorized() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let result = commands
        .execute(TENANT, PROJECT, A, acquire(&store, "inspect-auth").await)
        .await
        .unwrap();
    let claim = result["receipt"]["data"].clone();

    // No executions yet — inspection must still report execution_authorized=false.
    let exec_count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.executions
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(exec_count, 0);

    let viewed = inspect(&store, A, &claim).await;
    assert_eq!(viewed["execution_authorized"], false);
    // Advisory eligibility may be true for an eligible holder, but is not permission.
    assert!(viewed.get("execution_eligibility_advisory").is_some());

    // Different client with read on the same stream still must not get start permission.
    admin
        .batch_execute(
            "INSERT INTO awr_team.workstream_grants(
                tenant_id,project_id,actor_id,client_id,workstream_id,
                authority_version,can_read,can_write,active)
             VALUES (
                'reader-tenant','reader-project','agent','cli-b',
                '00000000000000000000000001',1,true,false,true)
             ON CONFLICT (tenant_id,project_id,actor_id,client_id,workstream_id)
             DO UPDATE SET can_read=true, active=true,
               grant_version=awr_team.workstream_grants.grant_version+1;",
        )
        .await
        .unwrap();
    let other = inspect(&store, B, &claim).await;
    assert_eq!(other["execution_authorized"], false);
    assert_eq!(other["owned_by_client"], false);
}
