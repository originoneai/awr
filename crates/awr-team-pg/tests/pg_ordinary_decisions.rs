#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::{PgError, WorkstreamCommand, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};

async fn start(store: &WorkstreamReadStore, token: &str, name: &str) -> String {
    let prepared = prepare(store, token, "a").await;
    let result = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(
                &prepared,
                name,
                "session.start",
                json!({"conversation_id":name}),
            ),
        )
        .await
        .unwrap();
    result["receipt"]["data"]["session_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn claim(store: &WorkstreamReadStore) -> Value {
    let session = start(store, A, "question-author").await;
    let p = prepare(store, A, "a").await;
    let work_version = if p["data"]["runtime"].is_null() {
        "0"
    } else {
        p["data"]["runtime"]["work_version"].as_str().unwrap()
    };
    store.commands().execute(TENANT,PROJECT,A,command(&p,"take-question-work","task.claim_available",
        json!({"session_id":session,"expected_session_version":"1","expected_responsibility_version":p["data"]["responsibility"]["version"],
            "expected_work_version":work_version,"ttl_seconds":3600}))).await.unwrap()
}

async fn request(
    store: &WorkstreamReadStore,
    claim: &Value,
    key: &str,
    people: &[&str],
) -> WorkstreamCommand {
    command(
        &prepare(store, A, "a").await,
        &format!("request-{key}"),
        "decision.request",
        json!({"session_id":claim["receipt"]["data"]["session_id"],"expected_session_version":"1",
            "claim_id":claim["receipt"]["data"]["claim_id"],"expected_fence":claim["receipt"]["data"]["fence"],
            "expected_lease_version":claim["receipt"]["data"]["lease_version"],"request_key":key,
            "question":"Should the first release include the customer import?",
            "impact":"The answer determines what the team will deliver this week.","allowed_responder_person_ids":people}),
    )
}

async fn answer(
    store: &WorkstreamReadStore,
    token: &str,
    session: &str,
    decision: &str,
    key: &str,
) -> WorkstreamCommand {
    command(
        &prepare(store, token, "a").await,
        key,
        "decision.respond",
        json!({"session_id":session,"expected_session_version":"1","decision_id":decision,
            "expected_decision_version":"1","reply":"Deliver the customer import in the next release."}),
    )
}

async fn enable_reviewer(admin: &tokio_postgres::Client) {
    admin
        .batch_execute("UPDATE awr_team.credentials SET actor_id='reviewer' WHERE id='reader-b';")
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,
        authority_version,can_read,can_write) VALUES($1,$2,'reviewer','cli-b',$3,1,true,true)",
        &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
}

async fn catalog(store: &WorkstreamReadStore, token: &str) -> Value {
    let mut q = query("decisions.list");
    q.work_id = Some("a".into());
    q.decision_state = Some("all".into());
    store.query(TENANT, PROJECT, token, q).await.unwrap()
}

async fn change_source(admin: &tokio_postgres::Client, path: &str, text: &str) {
    let row=admin.query_one("SELECT s.id,s.source_ref_json FROM awr_team.source_snapshots s
        JOIN awr_team.projects p ON p.tenant_id=s.tenant_id AND p.id=s.project_id AND p.active_snapshot_id=s.id
        WHERE p.tenant_id=$1 AND p.id=$2",&[&TENANT,&PROJECT]).await.unwrap();
    let id: String = row.get(0);
    let mut source: Value = row.get(1);
    let file = source["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|file| file["path"] == path)
        .unwrap();
    file["text"] = json!(text);
    file["bytes"] = json!(text.len());
    file["sha256"] = json!(
        awr_source::fingerprint(text.as_bytes())
            .strip_prefix("sha256:")
            .unwrap()
    );
    admin.execute("UPDATE awr_team.source_snapshots SET source_ref_json=$4 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&TENANT,&PROJECT,&id,&source]).await.unwrap();
}

#[tokio::test]
async fn changed_verified_goal_material_rejects_an_answer_while_unrelated_material_preserves_it() {
    let (_guard, admin, _, store) = setup_with_specs(vec![
        awr_team_pg::SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"# Release goal\nDeliver the customer portal.".to_vec(),
        },
        awr_team_pg::SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"# Independent goal\nImprove the internal search.".to_vec(),
        },
    ])
    .await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let opened = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            request(&store, &c, "goal-binding", &["agent"]).await,
        )
        .await
        .unwrap();
    let id = opened["receipt"]["data"]["decision_id"].as_str().unwrap();
    change_source(
        &admin,
        "docs/private-beta.md",
        "# Independent goal\nImprove search and sorting.",
    )
    .await;
    admin
        .batch_execute("UPDATE awr_team.projects SET project_revision=project_revision+1")
        .await
        .unwrap();
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["baseline_matches_current"],
        true
    );
    change_source(
        &admin,
        "docs/alpha.md",
        "# Revised release goal\nDeliver the import before the customer portal.",
    )
    .await;
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["baseline_matches_current"],
        false
    );
    let reply = answer(
        &store,
        A,
        c["receipt"]["data"]["session_id"].as_str().unwrap(),
        id,
        "old-goal-answer",
    )
    .await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, reply).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["state"],
        "open"
    );
}

#[tokio::test]
async fn simultaneous_retries_create_one_question_and_lease_expiry_does_not_prevent_an_allowed_answer()
 {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    enable_reviewer(&admin).await;
    let c = claim(&store).await;
    let left = request(&store, &c, "concurrent-intake", &["reviewer"]).await;
    let mut right = left.clone();
    right.request_id = "concurrent-request-retry".into();
    let commands = store.commands();
    let (left, right) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, left),
        commands.execute(TENANT, PROJECT, A, right)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(
        left["receipt"]["data"]["decision_id"],
        right["receipt"]["data"]["decision_id"]
    );
    assert_ne!(
        left["receipt"]["data"]["deduplicated"],
        right["receipt"]["data"]["deduplicated"]
    );
    let id = left["receipt"]["data"]["decision_id"].as_str().unwrap();
    admin.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second' WHERE work_id='a'").await.unwrap();
    let b = start(&store, B, "answer-after-disconnect").await;
    let reply = answer(&store, B, &b, id, "answer-after-expiry").await;
    let result = commands.execute(TENANT, PROJECT, B, reply).await.unwrap();
    assert_eq!(result["receipt"]["data"]["applied"], true);
    assert_eq!(result["receipt"]["data"]["decision"]["state"], "replied");
}

#[tokio::test]
async fn ordinary_question_survives_sessions_and_records_one_attributed_answer() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    enable_reviewer(&admin).await;
    let c = claim(&store).await;
    let req = request(&store, &c, "release-scope", &["reviewer"]).await;
    let opened = store
        .commands()
        .execute(TENANT, PROJECT, A, req.clone())
        .await
        .unwrap();
    let id = opened["receipt"]["data"]["decision_id"].as_str().unwrap();
    assert_eq!(
        opened["receipt"]["data"]["decision"]["decision_version"],
        "1"
    );
    assert_eq!(
        opened["receipt"]["data"]["decision"]["baseline_matches_current"],
        true
    );
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, req)
            .await
            .unwrap()["replayed"],
        true
    );
    let before = prepare(&store, A, "a").await;
    assert_eq!(
        before["data"]["ordinary_decisions"]["items"][0]["decision_id"],
        id
    );
    assert_eq!(before["data"]["guidance"]["action"]["op"], "decisions.list");
    assert_eq!(
        before["data"]["guidance"]["action"]["query"]["decision_id"],
        id
    );
    let new_session = start(&store, B, "reviewer-new-session").await;
    let continued = prepare(&store, A, "a").await;
    assert_eq!(
        continued["data"]["context_hash"],
        before["data"]["context_hash"]
    );
    let reply = answer(&store, B, &new_session, id, "answer-release-scope").await;
    let resolved = store
        .commands()
        .execute(TENANT, PROJECT, B, reply.clone())
        .await
        .unwrap();
    let item = &resolved["receipt"]["data"]["decision"];
    assert_eq!(resolved["receipt"]["data"]["applied"], true);
    assert_eq!(item["state"], "replied");
    assert_eq!(item["decision_version"], "2");
    assert_eq!(item["resolution"]["person_id"], "reviewer");
    assert_eq!(item["resolution"]["display_name"], "Reviewer");
    assert_eq!(item["resolution"]["session_id"], new_session);
    assert_eq!(item["approval_granted"], false);
    assert_eq!(resolved["execution_authorized"], false);
    assert_eq!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, reply)
            .await
            .unwrap()["replayed"],
        true
    );
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["reply"],
        item["reply"]
    );
    let mut q = query("work.observe");
    q.work_id = Some("a".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["waiting_user"],
        false
    );
}

#[tokio::test]
async fn reconnect_deduplicates_the_persisted_business_request_across_sessions() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let req = request(&store, &c, "same-question", &["agent"]).await;
    let first = store
        .commands()
        .execute(TENANT, PROJECT, A, req.clone())
        .await
        .unwrap();
    let successor = start(&store, A, "successor-author").await;
    let mut retry = request(&store, &c, "same-question", &["agent"]).await;
    retry.request_id = "successor-observed-request".into();
    retry.args["session_id"] = json!(successor);
    let second = store
        .commands()
        .execute(TENANT, PROJECT, A, retry.clone())
        .await
        .unwrap();
    assert_eq!(second["receipt"]["data"]["deduplicated"], true);
    assert_eq!(
        second["receipt"]["data"]["decision_id"],
        first["receipt"]["data"]["decision_id"]
    );
    assert_eq!(
        catalog(&store, A).await["data"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    retry.request_id = "changed-question-intent".into();
    retry.args["question"] = json!("Should the release be postponed?");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, retry).await,
        Err(PgError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn simultaneous_sessions_have_one_winning_answer_and_an_accurate_loser_receipt() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    enable_reviewer(&admin).await;
    let c = claim(&store).await;
    let opened = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            request(&store, &c, "race", &["agent", "reviewer"]).await,
        )
        .await
        .unwrap();
    let id = opened["receipt"]["data"]["decision_id"].as_str().unwrap();
    let b = start(&store, B, "second-decider").await;
    let a = c["receipt"]["data"]["session_id"].as_str().unwrap();
    let mut left = answer(&store, A, a, id, "left-answer").await;
    left.args["reply"] = json!("Deliver the import this week.");
    let right = answer(&store, B, &b, id, "right-answer").await;
    let commands = store.commands();
    let (left, right) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, left),
        commands.execute(TENANT, PROJECT, B, right)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    let applied = [&left, &right]
        .iter()
        .filter(|r| r["receipt"]["data"]["applied"] == true)
        .count();
    assert_eq!(applied, 1);
    let loser = if left["receipt"]["data"]["applied"] == true {
        &right
    } else {
        &left
    };
    assert_eq!(loser["receipt"]["data"]["already_resolved"], true);
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["decision_version"],
        "2"
    );
}

#[tokio::test]
async fn a_member_cannot_answer_another_members_question_without_explicit_responder_selection() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    enable_reviewer(&admin).await;
    let c = claim(&store).await;
    let opened = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            request(&store, &c, "restricted-reply", &["reviewer"]).await,
        )
        .await
        .unwrap();
    let id = opened["receipt"]["data"]["decision_id"].as_str().unwrap();
    let session = c["receipt"]["data"]["session_id"].as_str().unwrap();
    let wrong = answer(&store, A, session, id, "wrong-responder").await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, wrong).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["state"],
        "open"
    );
    let mut foreign = query("decisions.list");
    foreign.work_id = Some("b-private".into());
    foreign.decision_id = Some(id.into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, foreign).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn revoked_credentials_refuse_an_uncommitted_answer_and_preserve_the_question() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    enable_reviewer(&admin).await;
    let c = claim(&store).await;
    let opened = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            request(&store, &c, "revocation", &["reviewer"]).await,
        )
        .await
        .unwrap();
    let session = start(&store, B, "revoked-decider").await;
    let reply = answer(
        &store,
        B,
        &session,
        opened["receipt"]["data"]["decision_id"].as_str().unwrap(),
        "revoked-answer",
    )
    .await;
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-b'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, reply).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["state"],
        "open"
    );
}

#[tokio::test]
async fn a_changed_responsibility_baseline_rejects_the_old_answer_and_allows_withdrawal() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let opened = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            request(&store, &c, "changed-owner", &["agent"]).await,
        )
        .await
        .unwrap();
    let id = opened["receipt"]["data"]["decision_id"].as_str().unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.task_responsibilities SET version=version+1 WHERE work_id='a'",
        )
        .await
        .unwrap();
    let session = c["receipt"]["data"]["session_id"].as_str().unwrap();
    let mut reply = answer(&store, A, session, id, "outdated-answer").await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, reply.clone())
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["baseline_matches_current"],
        false
    );
    reply.op = "decision.cancel".into();
    reply.request_id = "withdraw-old-question".into();
    reply.args["reply"] =
        json!("The responsibility changed. A new owner will confirm the current release scope.");
    let cancelled = store
        .commands()
        .execute(TENANT, PROJECT, A, reply)
        .await
        .unwrap();
    assert_eq!(
        cancelled["receipt"]["data"]["decision"]["state"],
        "cancelled"
    );
}

#[tokio::test]
async fn live_claim_and_single_open_wait_guards_apply_to_new_questions() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let mut wrong = request(&store, &c, "wrong-lease", &["agent"]).await;
    wrong.args["expected_fence"] = json!("2");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, wrong).await,
        Err(PgError::StaleFence)
    ));
    let opened = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            request(&store, &c, "first-question", &["agent"]).await,
        )
        .await
        .unwrap();
    assert_eq!(opened["receipt"]["data"]["deduplicated"], false);
    let second = request(&store, &c, "second-question", &["agent"]).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, second).await,
        Err(PgError::WaitOpen)
    ));
    let p = prepare(&store, A, "a").await;
    let acquire = command(
        &p,
        "duplicate-claim",
        "claim.acquire",
        json!({"session_id":c["receipt"]["data"]["session_id"],
        "expected_session_version":"1","expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":30}),
    );
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, acquire).await,
        Err(PgError::WaitOpen)
    ));
    let execution = command(
        &p,
        "waiting-execution",
        "execution.prepare",
        json!({"session_id":c["receipt"]["data"]["session_id"],
        "expected_session_version":"1","claim_id":c["receipt"]["data"]["claim_id"],"expected_fence":c["receipt"]["data"]["fence"],
        "expected_lease_version":c["receipt"]["data"]["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "input_digest":"a".repeat(64),"declared_scope":["src"]}),
    );
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, execution)
            .await,
        Err(PgError::WaitOpen)
    ));
}

#[tokio::test]
async fn legacy_waits_keep_their_existing_behavior_and_cannot_overwrite_new_decisions() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let session = c["receipt"]["data"]["session_id"].as_str().unwrap();
    admin.execute("INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state)
        VALUES($1,$2,'legacy-question',$3,'a','Legacy question','open')",&[&TENANT,&PROJECT,&session]).await.unwrap();
    assert!(
        catalog(&store, A).await["data"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    admin.batch_execute("UPDATE awr_team.wait_items SET state='replied',reply='Legacy answer' WHERE id='legacy-question'").await.unwrap();
    let opened = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            request(&store, &c, "protected-question", &["agent"]).await,
        )
        .await
        .unwrap();
    let id = opened["receipt"]["data"]["decision_id"].as_str().unwrap();
    assert!(admin.execute("UPDATE awr_team.wait_items SET state='replied',reply='Unattributed answer' WHERE id=$1",&[&id]).await.is_err());
    assert_eq!(
        catalog(&store, A).await["data"]["items"][0]["state"],
        "open"
    );
    assert!(
        admin
            .execute("DELETE FROM awr_team.wait_items WHERE id=$1", &[&id])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn question_pages_are_bounded_and_bound_to_the_current_permissions() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = claim(&store).await;
    let session = c["receipt"]["data"]["session_id"].as_str().unwrap();
    for key in ["one", "two"] {
        let opened = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                request(&store, &c, key, &["agent"]).await,
            )
            .await
            .unwrap();
        let reply = answer(
            &store,
            A,
            session,
            opened["receipt"]["data"]["decision_id"].as_str().unwrap(),
            &format!("answer-{key}"),
        )
        .await;
        store
            .commands()
            .execute(TENANT, PROJECT, A, reply)
            .await
            .unwrap();
    }
    let mut q = query("decisions.list");
    q.work_id = Some("a".into());
    q.decision_state = Some("all".into());
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["items"].as_array().unwrap().len(), 1);
    q.cursor = first["data"]["next_cursor"].as_str().map(str::to_owned);
    assert!(q.cursor.is_some());
    let next = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_ne!(
        next["data"]["items"][0]["decision_id"],
        first["data"]["items"][0]["decision_id"]
    );
    admin.batch_execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
}

#[tokio::test]
async fn schema61_migration_is_atomic_repeatable_and_retains_wait_isolation() {
    let (_guard, admin, _) = common::historical_team_schema(60).await;
    let ddl = include_str!("../migrations/20261115000061_ordinary_decisions.sql");
    assert!(
        admin
            .batch_execute(&ddl.replace(
                "UPDATE awr_team.schema_state",
                "SELECT 1/0; UPDATE awr_team.schema_state"
            ))
            .await
            .is_err()
    );
    admin.batch_execute("ROLLBACK").await.unwrap();
    let columns: i64 = admin
        .query_one(
            "SELECT count(*) FROM information_schema.columns WHERE table_schema='awr_team'
        AND table_name='wait_items' AND column_name='decision_key'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(columns, 0);
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::migrate(&admin).await.unwrap();
    let row = admin
        .query_one(
            "SELECT relrowsecurity,relforcerowsecurity FROM pg_class
        WHERE oid='awr_team.wait_items'::regclass",
            &[],
        )
        .await
        .unwrap();
    assert!(row.get::<_, bool>(0));
    assert!(row.get::<_, bool>(1));
    awr_team_pg::check_schema(&admin).await.unwrap();
}
