//! Authenticated handoff regressions; these do not count as native business acceptance.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{PgError, WorkstreamCommand, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

async fn members(admin: &Client) {
    admin.batch_execute(r#"UPDATE awr_team.project_memberships SET role='worker' WHERE actor_id='agent';
        UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1 WHERE client_id='cli-a';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
          VALUES('reader-tenant','receiver','human','Successor member','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
          VALUES('reader-tenant','reader-project','receiver','worker');
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
          ('reader-tenant','reader-project','agent','Predecessor','active','{"kind":"simulated_member","controller_ref":"shared-controller"}'::jsonb),
          ('reader-tenant','reader-project','receiver','Successor','active','{"kind":"simulated_member","controller_ref":"shared-controller"}'::jsonb);
        UPDATE awr_team.credentials SET actor_id='receiver' WHERE id='reader-b';
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
          VALUES('reader-tenant','reader-project','receiver','cli-b','00000000000000000000000001',1,true,true);"#).await.unwrap();
}

async fn scoped(store: &WorkstreamReadStore, token: &str, session: &str) -> Value {
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    q.session_id = Some(session.into());
    q.max_context_bytes = Some(262144);
    store.query(TENANT, PROJECT, token, q).await.unwrap()
}

async fn start_session(store: &WorkstreamReadStore, token: &str, name: &str) -> String {
    let p = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&p, name, "session.start", json!({"conversation_id":name})),
        )
        .await
        .unwrap()["receipt"]["data"]["session_id"]
        .as_str()
        .unwrap()
        .into()
}

struct Trial {
    sender: String,
    receiver: String,
    claim: Value,
    execution: Option<Value>,
    handoff: Value,
    now: i64,
}

async fn trial(admin: &Client, store: &WorkstreamReadStore, live: bool) -> Trial {
    trial_options(admin, store, live, "execution", true).await
}

async fn trial_options(
    admin: &Client,
    store: &WorkstreamReadStore,
    live: bool,
    kind: &str,
    supplied_package: bool,
) -> Trial {
    members(admin).await;
    let sender = start_session(store, A, "predecessor-conversation").await;
    let receiver = start_session(store, B, "successor-conversation").await;
    let p = scoped(store, A, &sender).await;
    let claimed = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "take",
                "task.claim_available",
                json!({"session_id":sender,"expected_session_version":"1",
            "expected_responsibility_version":p["data"]["responsibility"]["version"],
            "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
            "ttl_seconds":600}),
            ),
        )
        .await
        .unwrap();
    let claim = claimed["receipt"]["data"].clone();
    let execution = if live {
        let p = scoped(store, A, &sender).await;
        let intent = store.commands().execute(TENANT, PROJECT, A, command(&p, "prepare-run", "execution.prepare",
            json!({"session_id":sender,"expected_session_version":"1","claim_id":claim["claim_id"],
                "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
                "expected_work_version":p["data"]["runtime"]["work_version"],
                "input_digest":"a".repeat(64),"declared_scope":["src/api"]}))).await.unwrap();
        let p = scoped(store, A, &sender).await;
        Some(store.commands().execute(TENANT, PROJECT, A, command(&p, "start-run", "execution.start",
            json!({"session_id":sender,"expected_session_version":"1",
                "execution_id":intent["receipt"]["data"]["execution_id"],"expected_execution_version":"1",
                "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
                "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"})))
            .await.unwrap()["receipt"]["data"].clone())
    } else {
        None
    };
    let p = scoped(store, A, &sender).await;
    let checkpoint = store.commands().execute(TENANT, PROJECT, A, command(&p, "save-checkpoint", "session.checkpoint",
        json!({"session_id":sender,"expected_session_version":"1","context_hash":p["data"]["context_hash"],
            "next_action":"Review the unfinished API implementation","open_loops":["Independent review pending"]})))
        .await.unwrap();
    let now: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let p = scoped(store, A, &sender).await;
    let mut args = json!({"session_id":sender,"expected_session_version":"2","handoff_id":"handoff",
            "kind":kind,"to_person_id":"receiver",
            "package":{"task_id":"a","contract_version":p["data"]["contract_hash"],"contract_hash":p["data"]["contract_hash"],
                "current_person_id":"agent","current_execution":{"kind":"person","person_id":"agent"},
                "consumed_context_digest":checkpoint["receipt"]["data"]["context_hash"],
                "checkpoint_ids":[checkpoint["receipt"]["data"]["checkpoint_id"]],"artifact_versions":[],"dependency_ids":[],
                "todos":["Review the unfinished API implementation","Independent review pending"],
                "awaiting_replies":[],"unknown_side_effects":[]},
            "proposed_successor":{"kind":"person","person_id":"receiver"},
            "proposer_execution_id":execution.as_ref().map(|e|e["execution_id"].clone()),
            "proposer_fence":claim["fence"],"now_ms":now,"expires_at_ms":now+60000});
    if !supplied_package {
        args.as_object_mut().unwrap().remove("package");
    }
    let handoff = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(&p, "propose", "handoff.propose", args),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    Trial {
        sender,
        receiver,
        claim,
        execution,
        handoff,
        now,
    }
}

async fn inspect(store: &WorkstreamReadStore, trial: &mut Trial) {
    inspect_named(store, trial, "inspect").await;
}

async fn inspect_named(store: &WorkstreamReadStore, trial: &mut Trial, request: &str) {
    let p = scoped(store, B, &trial.receiver).await;
    trial.handoff = store.commands().execute(TENANT, PROJECT, B, command(&p, request, "handoff.inspect",
        json!({"session_id":trial.receiver,"expected_session_version":"1","handoff_id":"handoff",
            "expected_handoff_version":trial.handoff["version"],"inspector_person_id":"receiver","now_ms":trial.now})))
        .await.unwrap()["receipt"]["data"].clone();
}

async fn accept(store: &WorkstreamReadStore, trial: &Trial) -> WorkstreamCommand {
    let p = scoped(store, B, &trial.receiver).await;
    let mut args = json!({"session_id":trial.receiver,"expected_session_version":"1","handoff_id":"handoff",
        "expected_handoff_version":trial.handoff["version"],"acceptor_person_id":"receiver",
        "successor_execution":{"kind":"person","person_id":"receiver"},"prior_execution_stopped":true,
        "prior_reconciled":true,"context_reprepared":true,"expected_current_fence":trial.claim["fence"],"now_ms":trial.now});
    // New clients select the actual inspection; legacy responses have none.
    if !trial.handoff["consumption"].is_null() {
        args["inspection_request_id"] = trial.handoff["inspection_request_id"].clone();
    }
    command(&p, "accept", "handoff.accept", args)
}

#[tokio::test]
async fn inspected_transfer_preserves_owner_and_replay_never_grants_another_lease() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    assert_eq!(
        t.handoff["handoff"]["package"]["checkpoint_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        t.handoff["prepared_context"]["data"]["context_complete"],
        true
    );
    assert_eq!(t.handoff["consumption"]["member_person_id"], "receiver");
    let cmd = accept(&store, &t).await;
    let accepted = store
        .commands()
        .execute(TENANT, PROJECT, B, cmd.clone())
        .await
        .unwrap();
    assert_eq!(accepted["current_responsibility"]["owner"], "agent");
    assert_eq!(
        accepted["current_responsibility"]["current_executor"]["person_id"],
        "receiver"
    );
    assert_eq!(accepted["execution_authorized"], false);
    let p = scoped(&store, B, &t.receiver).await;
    let claimed = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            B,
            command(
                &p,
                "successor-claim",
                "claim.acquire",
                json!({"session_id":t.receiver,"expected_session_version":"1",
            "expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":600}),
            ),
        )
        .await
        .unwrap();
    assert_ne!(claimed["receipt"]["data"]["claim_id"], t.claim["claim_id"]);
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.claims", &[])
        .await
        .unwrap()
        .get(0);
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, B, cmd.clone())
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], accepted["receipt"]);
    assert_eq!(replay["execution_authorized"], false);
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.claims", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        count
    );
    let mut changed = cmd;
    changed.args["inspection_request_id"] = json!("different-inspection");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    let state_sql = "SELECT jsonb_build_object(
        'claims',(SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM awr_team.claims c),
        'runtime',(SELECT jsonb_agg(to_jsonb(r)) FROM awr_team.work_runtime r),
        'operations',(SELECT count(*) FROM awr_team.operations),
        'responsibility',(SELECT jsonb_agg(to_jsonb(r)) FROM awr_team.task_responsibilities r))";
    let before: Value = admin.query_one(state_sql, &[]).await.unwrap().get(0);
    let p = scoped(&store, A, &t.sender).await;
    let late_renew = store.commands().execute(TENANT, PROJECT, A, command(&p, "late-renew", "claim.renew",
        json!({"session_id":t.sender,"expected_session_version":"2","claim_id":t.claim["claim_id"],
            "expected_fence":t.claim["fence"],"expected_lease_version":t.claim["lease_version"],"ttl_seconds":600}))).await;
    assert!(
        matches!(late_renew, Err(PgError::StaleFence)),
        "Late predecessor renewal must be fenced: {late_renew:?}"
    );
    let after: Value = admin.query_one(state_sql, &[]).await.unwrap().get(0);
    assert_eq!(
        after, before,
        "Fenced renewal must not mutate claims, ownership, runtime or operations"
    );
}

#[tokio::test]
async fn a_new_receiver_session_cannot_reuse_another_sessions_inspection() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    t.receiver = start_session(&store, B, "another-conversation").await;
    let result = store
        .commands()
        .execute(TENANT, PROJECT, B, accept(&store, &t).await)
        .await;
    assert!(
        matches!(result, Err(PgError::PreconditionsChanged)),
        "A new session must not reuse another session's inspection: {result:?}"
    );
}

#[tokio::test]
async fn checkpoint_change_requires_delivery_of_the_updated_package() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let old = t.handoff.clone();
    let p = scoped(&store, A, &t.sender).await;
    let saved = store.commands().execute(TENANT, PROJECT, A, command(&p, "updated-checkpoint", "session.checkpoint",
        json!({"session_id":t.sender,"expected_session_version":"2","context_hash":p["data"]["context_hash"],
            "next_action":"Verify revised API behavior","open_loops":["New follow-up from the customer"]}))).await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    inspect_named(&store, &mut t, "updated-inspect").await;
    assert_ne!(t.handoff["version"], old["version"]);
    assert_eq!(
        t.handoff["handoff"]["package"]["checkpoint_ids"][0],
        saved["receipt"]["data"]["checkpoint_id"]
    );
    assert_eq!(
        t.handoff["handoff"]["package"]["todos"][1],
        "New follow-up from the customer"
    );
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await
            .unwrap()["receipt"]["data"]["status"],
        "accepted"
    );
}

#[tokio::test]
async fn renewed_predecessor_lease_requires_a_current_inspection() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let p = scoped(&store, A, &t.sender).await;
    store.commands().execute(TENANT, PROJECT, A, command(&p, "renew-before-transfer", "claim.renew",
        json!({"session_id":t.sender,"expected_session_version":"2","claim_id":t.claim["claim_id"],
            "expected_fence":t.claim["fence"],"expected_lease_version":t.claim["lease_version"],"ttl_seconds":600}))).await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    inspect_named(&store, &mut t, "renewed-inspect").await;
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await
            .unwrap()["receipt"]["data"]["status"],
        "accepted"
    );
}

#[tokio::test]
async fn current_read_grant_is_required_even_with_an_existing_inspection() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let cmd = accept(&store, &t).await;
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_read=false,can_write=false,grant_version=grant_version+1
        WHERE actor_id='receiver' AND client_id='cli-b'").await.unwrap();
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, cmd).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn receiver_identity_and_retained_runtime_fence_are_required() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let p = scoped(&store, A, &t.sender).await;
    let mut args = accept(&store, &t).await.args;
    args["session_id"] = json!(t.sender);
    args["expected_session_version"] = json!("2");
    let error = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(&p, "forged-receiver", "handoff.accept", args),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, PgError::Forbidden));
    let mut cmd = accept(&store, &t).await;
    cmd.args
        .as_object_mut()
        .unwrap()
        .remove("expected_current_fence");
    assert!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, cmd)
            .await
            .unwrap_err()
            .is_missing_handoff_fence()
    );
    let p = scoped(&store, B, &t.receiver).await;
    let error = store.commands().execute(TENANT, PROJECT, B, command(&p, "inspect-missing", "handoff.inspect",
        json!({"session_id":t.receiver,"expected_session_version":"1","handoff_id":"missing",
            "expected_handoff_version":"1","inspector_person_id":"receiver"}))).await.unwrap_err();
    assert!(error.is_handoff_unavailable());
}

#[tokio::test]
async fn assertions_and_client_time_are_optional_and_cannot_supply_authority() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let mut cmd = accept(&store, &t).await;
    for key in [
        "prior_execution_stopped",
        "prior_reconciled",
        "context_reprepared",
    ] {
        cmd.args[key] = json!(false);
    }
    cmd.args["now_ms"] = json!(0);
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, cmd)
            .await
            .unwrap()["receipt"]["data"]["status"],
        "accepted"
    );
}

#[tokio::test]
async fn a_read_only_lookup_cannot_substitute_for_a_delivered_inspection() {
    let (_guard, admin, _, store) = setup().await;
    let t = trial(&admin, &store, false).await;
    let mut q = query("handoff.inspect");
    q.work_id = Some("a".into());
    q.handoff_id = Some("handoff".into());
    assert_eq!(
        store.query(TENANT, PROJECT, B, q).await.unwrap()["data"]["handoff"]["status"],
        "proposed"
    );
    let mut cmd = accept(&store, &t).await;
    cmd.args["inspection_request_id"] = json!("read-only-lookup");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, cmd).await,
        Err(PgError::PreconditionsChanged)
    ));
}

#[tokio::test]
async fn unknown_execution_requires_actual_reconciliation_and_new_inspection() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, true).await;
    let execution = t.execution.clone().unwrap();
    let p = scoped(&store, A, &t.sender).await;
    store.commands().execute(TENANT, PROJECT, A, command(&p, "report-unknown", "execution.report",
        json!({"session_id":t.sender,"expected_session_version":"2","execution_id":execution["execution_id"],
            "expected_execution_version":execution["execution_version"],"outcome":"unknown", "observed_paths":[],
            "note":"Connection interrupted before the result was observed"}))).await.unwrap();
    inspect(&store, &mut t).await;
    assert!(
        !t.handoff["handoff"]["package"]["unknown_side_effects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    const OP: &str =
        "awr1.recovery.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    admin.batch_execute("INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
        VALUES('reader-tenant','recovery','human','Recovery operator','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','reader-project','recovery','admin');
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,
          authority_version,can_read,can_write,can_manage,can_reconcile_execution)
        VALUES('reader-tenant','reader-project','recovery','recovery-client','00000000000000000000000001',1,true,true,true,true);").await.unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
        VALUES($1,'recovery','recovery','recovery-client',$2)",
            &[
                &TENANT,
                &awr_team_pg::workstream_credential_hash(OP).unwrap(),
            ],
        )
        .await
        .unwrap();
    let session = start_session(&store, OP, "operator-recovery").await;
    let mut q = query("execution.inspect");
    q.work_id = Some("a".into());
    q.execution_id = Some(execution["execution_id"].as_str().unwrap().into());
    let observed = store.query(TENANT, PROJECT, OP, q).await.unwrap()["data"].clone();
    let p = scoped(&store, OP, &session).await;
    store.commands().execute(TENANT, PROJECT, OP, command(&p, "reconcile-result", "execution.reconcile",
        json!({"session_id":session,"expected_session_version":"1","execution_id":execution["execution_id"],
            "expected_execution_version":observed["execution_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
            "reviewed_receipt_id":observed["latest_receipt"]["receipt_id"],"clear_recovery_block":true,
            "facts":{"outcome":"failed","input_digest":"a".repeat(64),"environment_digest":"c".repeat(64),
                "observed_paths":[],"note":"Inspected the stopped executor and resolved its effects","executor_stopped":true}})))
        .await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    inspect_named(&store, &mut t, "reconciled-inspect").await;
    assert!(
        t.handoff["handoff"]["package"]["unknown_side_effects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await
            .unwrap()["receipt"]["data"]["status"],
        "accepted"
    );
    let row = admin
        .query_one(
            "SELECT executor_actor_id,state FROM awr_team.executions WHERE id=$1",
            &[&execution["execution_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "agent");
    assert_eq!(row.get::<_, String>(1), "failed");
}

#[tokio::test]
async fn pending_shared_resource_blocks_transfer_even_without_a_running_execution() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    admin.execute("INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES($1,$2,'retained-resource','a','named','shared-output','reserved')", &[&TENANT,&PROJECT]).await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::RecoveryBlocked)
    ));
}

#[tokio::test]
async fn two_accepts_linearize_to_one_transfer_and_no_execution_start() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let first = accept(&store, &t).await;
    let mut second = first.clone();
    second.request_id = "accept-competitor".into();
    let one = store.commands();
    let two = store.commands();
    let (a, b) = tokio::join!(
        one.execute(TENANT, PROJECT, B, first),
        two.execute(TENANT, PROJECT, B, second)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(
        admin
            .query_one(
                "SELECT count(*) FROM awr_team.operations WHERE op='handoff.accept'",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.executions", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn cancel_and_accept_have_one_effective_outcome() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let receiver = accept(&store, &t).await;
    let p = scoped(&store, A, &t.sender).await;
    let cancel = command(
        &p,
        "cancel",
        "handoff.cancel",
        json!({"session_id":t.sender,"expected_session_version":"2",
        "handoff_id":"handoff","expected_handoff_version":t.handoff["version"],"by_person_id":"agent","reason":"Retain this work"}),
    );
    let one = store.commands();
    let two = store.commands();
    let (a, b) = tokio::join!(
        one.execute(TENANT, PROJECT, B, receiver),
        two.execute(TENANT, PROJECT, A, cancel)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let state: String = admin
        .query_one(
            "SELECT status FROM awr_team.team_handoffs WHERE id='handoff'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(matches!(state.as_str(), "accepted" | "cancelled"));
}

#[tokio::test]
async fn receiver_cannot_accept_without_actual_inspection() {
    let (_guard, admin, _, store) = setup().await;
    let trial = trial(&admin, &store, false).await;
    let result = store
        .commands()
        .execute(TENANT, PROJECT, B, accept(&store, &trial).await)
        .await;
    assert!(
        result.is_err(),
        "A receiver assertion must not substitute for actual inspection: {result:?}"
    );
}

#[tokio::test]
async fn caller_stop_flags_cannot_transfer_a_running_predecessor() {
    let (_guard, admin, _, store) = setup().await;
    let mut trial = trial(&admin, &store, true).await;
    inspect(&store, &mut trial).await;
    let result = store
        .commands()
        .execute(TENANT, PROJECT, B, accept(&store, &trial).await)
        .await;
    assert!(
        matches!(result, Err(PgError::RecoveryBlocked)),
        "Actual running execution must block handoff despite caller assurances: {result:?}"
    );
}

#[tokio::test]
async fn caller_clock_rollback_cannot_accept_an_expired_proposal() {
    let (_guard, admin, _, store) = setup().await;
    let mut trial = trial(&admin, &store, false).await;
    inspect(&store, &mut trial).await;
    // Advance only this isolated fixture's expiry; no timing-dependent sleep.
    admin
        .execute(
            "UPDATE awr_team.team_handoffs SET expires_at_ms=$1,
        body_json=jsonb_set(body_json,'{expires_at_ms}',to_jsonb($1::bigint)) WHERE id='handoff'",
            &[&(trial.now - 1)],
        )
        .await
        .unwrap();
    trial.now -= 1000;
    let result = store
        .commands()
        .execute(TENANT, PROJECT, B, accept(&store, &trial).await)
        .await;
    assert!(
        result.is_err(),
        "Server expiry must not follow rolled-back caller time: {result:?}"
    );
}

#[tokio::test]
async fn responsibility_transfer_derives_the_package_and_allows_the_new_owner_to_claim() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial_options(&admin, &store, false, "responsibility", false).await;
    inspect(&store, &mut t).await;
    assert_eq!(
        t.handoff["handoff"]["package"]["todos"][0],
        "Review the unfinished API implementation"
    );
    let accepted = store
        .commands()
        .execute(TENANT, PROJECT, B, accept(&store, &t).await)
        .await
        .unwrap();
    assert_eq!(accepted["current_responsibility"]["owner"], "receiver");
    assert!(accepted["current_responsibility"]["current_executor"].is_null());
    let p = scoped(&store, B, &t.receiver).await;
    let claimed = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            B,
            command(
                &p,
                "new-owner-claim",
                "claim.acquire",
                json!({"session_id":t.receiver,"expected_session_version":"1",
            "expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":600}),
            ),
        )
        .await
        .unwrap();
    assert_ne!(claimed["receipt"]["data"]["claim_id"], t.claim["claim_id"]);
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.executions", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn an_inspection_is_bound_to_the_actual_client_not_only_the_member() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    admin.batch_execute("UPDATE awr_team.credentials SET actor_id='receiver' WHERE id='no-grants';
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES('reader-tenant','reader-project','receiver','unscoped','00000000000000000000000001',1,true,true)").await.unwrap();
    let other_session = start_session(&store, NONE, "other-client").await;
    let p = scoped(&store, NONE, &other_session).await;
    let mut args = accept(&store, &t).await.args;
    args["session_id"] = json!(other_session);
    let attempted = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            NONE,
            command(&p, "borrowed-inspection", "handoff.accept", args),
        )
        .await;
    assert!(
        matches!(attempted, Err(PgError::PreconditionsChanged)),
        "Another client cannot reuse consumption: {attempted:?}"
    );
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await
            .unwrap()["receipt"]["data"]["status"],
        "accepted"
    );
}

#[tokio::test]
async fn missing_or_tampered_inspection_receipts_never_transfer_responsibility() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let mut missing = accept(&store, &t).await;
    missing.args["inspection_request_id"] = json!("not-committed");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, missing).await,
        Err(PgError::PreconditionsChanged)
    ));
    admin.batch_execute("UPDATE awr_team.operations SET result_json=jsonb_set(result_json,'{data,consumption,context_complete}','false')
        WHERE op='handoff.inspect'").await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        admin
            .query_one(
                "SELECT executor_person_id FROM awr_team.task_responsibilities WHERE work_id='a'",
                &[]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "agent"
    );
    assert_eq!(
        admin
            .query_one(
                "SELECT count(*) FROM awr_team.operations WHERE op='handoff.accept'",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn changed_artifact_versions_require_the_new_package_and_missing_artifacts_block_it() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    let p = scoped(&store, A, &t.sender).await;
    admin.execute("INSERT INTO awr_team.artifacts(tenant_id,project_id,id,object_key,sha256,byte_length,media_type,state,created_by)
        VALUES($1,$2,'handoff-artifact','handoff-artifact',$3,4,'text/plain','finalized','agent')",
        &[&TENANT,&PROJECT,&"a".repeat(64)]).await.unwrap();
    admin.execute("INSERT INTO awr_team.evidence(tenant_id,project_id,id,work_id,artifact_id,contract_hash,evidence_kind,trust_basis,digest,payload_json,created_by)
        VALUES($1,$2,'handoff-evidence','a','handoff-artifact',$3,'artifact','caller_asserted',$4,'{}','agent')",
        &[&TENANT,&PROJECT,&p["data"]["contract_hash"].as_str().unwrap(),&"a".repeat(64)]).await.unwrap();
    inspect(&store, &mut t).await;
    assert_eq!(
        t.handoff["handoff"]["package"]["artifact_versions"][0]["version"],
        "a".repeat(64)
    );
    admin
        .execute(
            "UPDATE awr_team.artifacts SET sha256=$1 WHERE id='handoff-artifact'",
            &[&"b".repeat(64)],
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .batch_execute("UPDATE awr_team.artifacts SET state='missing' WHERE id='handoff-artifact'")
        .await
        .unwrap();
    let p = scoped(&store, B, &t.receiver).await;
    let inspection = command(
        &p,
        "missing-artifact-inspect",
        "handoff.inspect",
        json!({"session_id":t.receiver,"expected_session_version":"1",
        "handoff_id":"handoff","expected_handoff_version":t.handoff["version"],"inspector_person_id":"receiver"}),
    );
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, inspection)
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.artifacts SET state='finalized' WHERE id='handoff-artifact'",
        )
        .await
        .unwrap();
    inspect_named(&store, &mut t, "changed-artifact-inspect").await;
    assert_eq!(
        t.handoff["handoff"]["package"]["artifact_versions"][0]["version"],
        "b".repeat(64)
    );
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await
            .unwrap()["receipt"]["data"]["status"],
        "accepted"
    );
}

#[tokio::test]
async fn expired_accept_and_timeout_preserve_the_predecessors_admission() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    admin
        .execute(
            "UPDATE awr_team.team_handoffs SET expires_at_ms=$1,
        body_json=jsonb_set(body_json,'{expires_at_ms}',to_jsonb($1::bigint)) WHERE id='handoff'",
            &[&(t.now - 1)],
        )
        .await
        .unwrap();
    let receiver = accept(&store, &t).await;
    let p = scoped(&store, A, &t.sender).await;
    let timeout = command(
        &p,
        "timeout",
        "handoff.timeout",
        json!({"session_id":t.sender,"expected_session_version":"2",
        "handoff_id":"handoff","expected_handoff_version":t.handoff["version"],"now_ms":0}),
    );
    let one = store.commands();
    let two = store.commands();
    let (accepted, timed_out) = tokio::join!(
        one.execute(TENANT, PROJECT, B, receiver),
        two.execute(TENANT, PROJECT, A, timeout)
    );
    assert!(accepted.is_err());
    assert_eq!(timed_out.unwrap()["receipt"]["data"]["status"], "timed_out");
    assert_eq!(
        admin
            .query_one(
                "SELECT state FROM awr_team.claims WHERE id=$1",
                &[&t.claim["claim_id"].as_str().unwrap()]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "active"
    );
}

#[tokio::test]
async fn predecessor_renewal_and_acceptance_cannot_both_commit() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let receiver = accept(&store, &t).await;
    let p = scoped(&store, A, &t.sender).await;
    let renew = command(
        &p,
        "renew-race",
        "claim.renew",
        json!({"session_id":t.sender,"expected_session_version":"2",
        "claim_id":t.claim["claim_id"],"expected_fence":t.claim["fence"],"expected_lease_version":t.claim["lease_version"],"ttl_seconds":600}),
    );
    let one = store.commands();
    let two = store.commands();
    let (accepted, renewed) = tokio::join!(
        one.execute(TENANT, PROJECT, B, receiver),
        two.execute(TENANT, PROJECT, A, renew)
    );
    assert_eq!(
        usize::from(accepted.is_ok()) + usize::from(renewed.is_ok()),
        1,
        "Only one admission transition may commit: {accepted:?}, {renewed:?}"
    );
}

#[tokio::test]
async fn agent_coordination_cannot_borrow_read_authority_from_its_write_delegation() {
    use awr_core::*;
    use std::collections::BTreeSet;
    let (_guard, admin, db, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='receiver';
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
        VALUES('reader-tenant','reader-project','receiver-binding','receiver','receiver','active')").await.unwrap();
    let authorization = awr_team_pg::AuthorizationStore::from_config(common::with_app_role(
        &common::test_config(),
        &db,
    ));
    for (id, actions) in [
        ("receiver-read", BTreeSet::from([AuthorizedAction::Inspect])),
        (
            "receiver-coordinate",
            BTreeSet::from([
                AuthorizedAction::StartWork,
                AuthorizedAction::ClaimCoordination,
            ]),
        ),
    ] {
        let grant = AgentAuthorization {
            id: id.into(),
            authorizer_person_id: PersonId::new("receiver").unwrap(),
            responsible_person_id: PersonId::new("receiver").unwrap(),
            subject_kind: ExecutionSubjectKind::Agent,
            subject_id: "receiver".into(),
            client_id: "cli-b".into(),
            session_id: None,
            model_id: None,
            scope: AuthorizationScope::Task {
                project_id: PROJECT.into(),
                work_item_id: "a".into(),
            },
            actions,
            expires_at_ms: None,
            status: AuthorizationStatus::Active,
            revoked_at_ms: None,
            revoked_by: None,
            verifiable_capabilities: vec![],
            self_reported_skill_hints: vec![],
            parent_authorization_id: None,
            maintainer_person_id: None,
            created_at_ms: t.now,
            binding_id: Some("receiver-binding".into()),
        };
        authorization
            .issue(
                TENANT,
                PROJECT,
                &IssueAuthorizationRequest {
                    request_key: id.into(),
                    authorization: grant,
                },
            )
            .await
            .unwrap();
    }
    inspect(&store, &mut t).await;
    assert_eq!(
        t.handoff["consumption"]["command_delegation_id"],
        "receiver-coordinate"
    );
    assert_eq!(
        t.handoff["consumption"]["execution_instance"]["kind"],
        "agent_run"
    );
    let mut cmd = accept(&store, &t).await;
    cmd.args["successor_execution"] = t.handoff["consumption"]["execution_instance"].clone();
    admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='receiver-read'",
        )
        .await
        .unwrap();
    assert!(
        matches!(
            store.commands().execute(TENANT, PROJECT, B, cmd).await,
            Err(PgError::Forbidden)
        ),
        "A current write delegation does not imply a current context read grant"
    );
}

#[tokio::test]
async fn contract_and_same_stream_dependency_changes_require_a_current_inspection() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    inspect(&store, &mut t).await;
    let mut contract: awr_team::WorkContract = serde_json::from_value(
        admin
            .query_one(
                "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='a'",
                &[],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    contract.required_dependencies = vec!["c".into()];
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='a'",
        &[&json!(contract), &contract.hash().unwrap()]).await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let p = scoped(&store, A, &t.sender).await;
    store.commands().execute(TENANT, PROJECT, A, command(&p, "contract-checkpoint", "session.checkpoint", json!({
        "session_id":t.sender,"expected_session_version":"2","context_hash":p["data"]["context_hash"],
        "next_action":"Finish after the dependency is verified","open_loops":["Dependency still requires acceptance"]}))).await.unwrap();
    inspect_named(&store, &mut t, "contract-inspect").await;
    assert_eq!(
        t.handoff["handoff"]["package"]["dependency_ids"],
        json!(["c"])
    );
    admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,recovery_blocked)
        VALUES('reader-tenant','reader-project','main','c','unclaimed',1,0,false)").await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    inspect_named(&store, &mut t, "dependency-inspect").await;
    assert_eq!(
        t.handoff["consumption"]["predecessor"]["dependency_versions"][0]["work_version"],
        "1"
    );
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await
            .unwrap()["receipt"]["data"]["status"],
        "accepted"
    );
    let p = scoped(&store, B, &t.receiver).await;
    let attempted = store.commands().execute(TENANT, PROJECT, B, command(&p, "blocked-successor-claim", "claim.acquire", json!({
        "session_id":t.receiver,"expected_session_version":"1","expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":600}))).await;
    assert!(
        matches!(attempted, Err(PgError::MissingDependency)),
        "Handoff must preserve dependency admission: {attempted:?}"
    );
}

#[tokio::test]
async fn missing_cross_stream_adoption_returns_incomplete_context_and_blocks_acceptance() {
    let (_guard, admin, _, store) = setup().await;
    let mut t = trial(&admin, &store, false).await;
    let mut contract: awr_team::WorkContract = serde_json::from_value(
        admin
            .query_one(
                "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='a'",
                &[],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    contract.codec = awr_team::WorkContract::CODEC_V6.into();
    contract.required_dependencies = vec!["b-private".into()];
    contract.dependency_acceptance.insert(
        "b-private".into(),
        awr_team::DependencyAcceptanceMode::CrossWorkstream(
            awr_team::CrossWorkstreamDependencyPolicy {
                review_assurance:
                    awr_team::CrossWorkstreamReviewAssurance::SimulatedMemberIndependent,
                version_policy: awr_core::DeliveryVersionPolicy::CurrentContract,
            },
        ),
    );
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='a'",
        &[&json!(contract), &contract.hash().unwrap()]).await.unwrap();
    let p = scoped(&store, A, &t.sender).await;
    assert_eq!(p["data"]["context_complete"], false);
    store.commands().execute(TENANT, PROJECT, A, command(&p, "cross-checkpoint", "session.checkpoint", json!({
        "session_id":t.sender,"expected_session_version":"2","context_hash":p["data"]["context_hash"],
        "next_action":"Wait for an authorized upstream delivery","open_loops":["Upstream artifact adoption is pending"]}))).await.unwrap();
    inspect(&store, &mut t).await;
    assert_eq!(
        t.handoff["prepared_context"]["data"]["context_complete"],
        false
    );
    assert_eq!(
        t.handoff["consumption"]["predecessor"]["adopted_dependencies"][0]["valid"],
        false
    );
    assert!(
        t.handoff["consumption"]["predecessor"]["dependency_versions"]
            .as_array()
            .unwrap()
            .is_empty(),
        "Cross-stream source facts must not be read as same-stream dependencies"
    );
    assert!(!t.handoff.to_string().contains("PRIVATE NEXT ACTION"));
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, accept(&store, &t).await)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
}
