//! Synthetic authenticated command/read regressions against an isolated PostgreSQL.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::*;
use awr_team_pg::{AuthorizationStore, PgError, WorkstreamCommand, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};

async fn members(admin: &tokio_postgres::Client, db: &str) {
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent';
        UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]' WHERE actor_id='agent';
        UPDATE awr_team.project_memberships SET role='maintainer',assignment_grant=true,business_roles='[\"supervisor\"]' WHERE actor_id='reviewer';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
          ('reader-tenant','member-one','human','Member One','active'),('reader-tenant','member-two','human','Member Two','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
          ('reader-tenant','reader-project','member-one','developer'),('reader-tenant','reader-project','member-two','maintainer');
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
          ('reader-tenant','reader-project','member-one','Member One','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"fixture\"}'),
          ('reader-tenant','reader-project','member-two','Member Two','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"fixture\"}');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
          ('reader-tenant','reader-project','binding-one','member-one','agent','active'),
          ('reader-tenant','reader-project','binding-two','member-two','reviewer','active');
        UPDATE awr_team.credentials SET actor_id='reviewer' WHERE id='reader-b';").await.unwrap();
    enable_writes(admin).await;
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'reviewer','cli-b',$3,1,true,true)", &[&TENANT,&PROJECT,&Id::from(1).to_string()]).await.unwrap();
    let store = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db));
    for (person, actor, client, binding, actions) in [
        (
            "member-one",
            "agent",
            "cli-a",
            "binding-one",
            vec![AuthorizedAction::Inspect, AuthorizedAction::StartWork],
        ),
        (
            "member-two",
            "reviewer",
            "cli-b",
            "binding-two",
            vec![AuthorizedAction::Inspect, AuthorizedAction::AssignWork],
        ),
    ] {
        store
            .issue(
                TENANT,
                PROJECT,
                &IssueAuthorizationRequest {
                    request_key: format!("issue-{binding}"),
                    authorization: AgentAuthorization {
                        id: format!("grant-{binding}"),
                        authorizer_person_id: PersonId::new(person).unwrap(),
                        responsible_person_id: PersonId::new(person).unwrap(),
                        subject_kind: ExecutionSubjectKind::Agent,
                        subject_id: actor.into(),
                        client_id: client.into(),
                        session_id: None,
                        model_id: None,
                        scope: AuthorizationScope::Workstream {
                            project_id: PROJECT.into(),
                            workstream_id: Id::from(1).to_string(),
                        },
                        actions: actions.into_iter().collect(),
                        expires_at_ms: None,
                        status: AuthorizationStatus::Active,
                        revoked_at_ms: None,
                        revoked_by: None,
                        verifiable_capabilities: vec![],
                        self_reported_skill_hints: vec![],
                        parent_authorization_id: None,
                        maintainer_person_id: None,
                        created_at_ms: 1,
                        binding_id: Some(binding.into()),
                    },
                },
            )
            .await
            .unwrap();
    }
}

fn content() -> Value {
    json!({"title":"Keep retries stable","original_words":"  Reuse the first request.\n第二行。",
        "when_to_use":"A response was lost","not_applicable":"A new business intent","observed_result":"One recorded effect",
        "scope":{"path_prefixes":["src/service"],"task_tags":["retry"]},
        "dependencies":{"documents":[{"path":"docs/contract.md","sha256":"b".repeat(64)}],"decision_ids":["decision-ref"]}})
}

async fn start(store: &WorkstreamReadStore, token: &str, key: &str) -> String {
    let p = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&p, key, "session.start", json!({"conversation_id":key})),
        )
        .await
        .unwrap()["receipt"]["data"]["session_id"]
        .as_str()
        .unwrap()
        .into()
}

async fn proposal(store: &WorkstreamReadStore, session: &str, key: &str) -> WorkstreamCommand {
    command(
        &prepare(store, A, "a").await,
        key,
        "lesson.propose",
        json!({"session_id":session,"expected_session_version":"1","content":content()}),
    )
}

async fn change(
    store: &WorkstreamReadStore,
    token: &str,
    session: &str,
    op: &str,
    id: &str,
    v: &str,
    key: &str,
) -> WorkstreamCommand {
    command(
        &prepare(store, token, "a").await,
        key,
        op,
        json!({"session_id":session,"expected_session_version":"1",
        "lesson_id":id,"expected_lesson_version":v}),
    )
}

async fn list(store: &WorkstreamReadStore, token: &str) -> Value {
    let mut q = query("lessons.list");
    q.work_id = Some("a".into());
    store.query(TENANT, PROJECT, token, q).await.unwrap()
}

async fn inbox(store: &WorkstreamReadStore, token: &str) -> Vec<Value> {
    let mut q = query("work.inbox");
    q.limit = Some(1);
    let mut items = Vec::new();
    for _ in 0..100 {
        let result = store
            .query(TENANT, PROJECT, token, q.clone())
            .await
            .unwrap();
        assert!(result["data"]["items"].as_array().unwrap().len() <= 1);
        items.extend(result["data"]["items"].as_array().unwrap().clone());
        match result["data"]["next_cursor"].as_str() {
            Some(cursor) => q.cursor = Some(cursor.into()),
            None => return items,
        }
    }
    panic!("inbox pagination failed to terminate")
}

#[tokio::test]
async fn lifecycle_attribution_private_candidates_confirmation_feedback_promotion_and_withdrawal() {
    let (_guard, admin, db, store) = setup().await;
    members(&admin, &db).await;
    let sa = start(&store, A, "start-author").await;
    let sb = start(&store, B, "start-supervisor").await;
    let p = proposal(&store, &sa, "propose").await;
    let created = store
        .commands()
        .execute(TENANT, PROJECT, A, p)
        .await
        .unwrap();
    let id = created["receipt"]["data"]["lesson_id"].as_str().unwrap();
    let own = list(&store, A).await;
    let record = &own["data"]["items"][0];
    assert_eq!(record["author_person_id"], "member-one");
    assert_eq!(record["source_session_id"], sa);
    assert_eq!(record["work_id"], "a");
    assert_eq!(record["content"], content());
    assert_eq!(
        record["original_words_provenance"],
        "recorded_by_authors_agent"
    );
    assert_eq!(record["use_for_personnel_evaluation"], false);
    // A member's observation identity receives inspection guidance for its own candidates.
    admin
        .execute(
            "UPDATE awr_team.credentials SET actor_id='member-one' WHERE id='no-grants'",
            &[],
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'member-one','unscoped',$3,1,true)",&[&TENANT,&PROJECT,&Id::from(1).to_string()]).await.unwrap();
    assert!(
        inbox(&store, NONE)
            .await
            .iter()
            .any(|x| x["guidance"]["code"] == "confirm_lesson")
    );
    assert!(record["created_at_unix_ms"].as_i64().unwrap() > 0);
    assert!(
        record["updated_at_unix_ms"].as_i64().unwrap()
            >= record["created_at_unix_ms"].as_i64().unwrap()
    );
    assert_eq!(list(&store, B).await["data"]["items"], json!([]));
    let sample = store
        .query(TENANT, PROJECT, A, query("work.inbox"))
        .await
        .unwrap();
    eprintln!(
        "LESSON_INBOX_SAMPLE={}",
        serde_json::to_string(&sample).unwrap()
    );
    let waiting = inbox(&store, A).await;
    let lesson = waiting
        .iter()
        .find(|x| x["guidance"]["code"] == "confirm_lesson")
        .unwrap();
    assert_eq!(lesson["guidance"]["type"], "确认经验");
    assert_eq!(lesson["guidance"]["lesson_id"], id);
    assert_eq!(lesson["next_query"]["op"], "work.observe");
    assert!(
        !inbox(&store, B)
            .await
            .iter()
            .any(|x| x["guidance"]["code"] == "confirm_lesson")
    );
    let mut c = change(&store, A, &sa, "lesson.confirm", id, "1", "confirm").await;
    let mut edited = content();
    edited["when_to_use"] = json!("A write response was lost");
    c.args["content"] = edited.clone();
    store
        .commands()
        .execute(TENANT, PROJECT, A, c)
        .await
        .unwrap();
    assert!(
        !inbox(&store, A)
            .await
            .iter()
            .any(|x| x["guidance"]["code"] == "confirm_lesson")
    );
    let shared = list(&store, B).await;
    assert_eq!(shared["data"]["items"][0]["stage"], "project");
    assert_eq!(shared["data"]["items"][0]["content"], edited);
    let mut feedback = change(
        &store,
        B,
        &sb,
        "lesson.mark_not_applicable",
        id,
        "2",
        "feedback",
    )
    .await;
    feedback.args["reason"] = json!("The operation creates a fresh intent.");
    let first = store
        .commands()
        .execute(TENANT, PROJECT, B, feedback.clone())
        .await
        .unwrap();
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, B, feedback)
        .await
        .unwrap();
    assert_eq!(first["receipt"], replay["receipt"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        list(&store, A).await["data"]["items"][0]["not_applicable_count"],
        1
    );
    let promote = change(&store, B, &sb, "lesson.promote", id, "3", "promote").await;
    store
        .commands()
        .execute(TENANT, PROJECT, B, promote)
        .await
        .unwrap();
    assert_eq!(
        list(&store, A).await["data"]["items"][0]["stage"],
        "team_rule"
    );
    let revoke = change(&store, A, &sa, "lesson.revoke", id, "4", "revoke").await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, revoke)
        .await
        .unwrap();
    for token in [A, B] {
        let read = list(&store, token).await;
        assert_eq!(read["data"]["items"][0]["status"], "revoked");
        assert!(read["data"]["items"][0]["content"].is_null());
    }
    let reconfirm = change(
        &store,
        A,
        &sa,
        "lesson.confirm",
        id,
        "5",
        "reconfirm-revoked",
    )
    .await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, reconfirm)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let rows = admin
        .query(
            "SELECT payload_json::text FROM awr_team.events WHERE event_type LIKE 'lesson.%'
        UNION ALL SELECT result_json::text FROM awr_team.operations WHERE op LIKE 'lesson.%'",
            &[],
        )
        .await
        .unwrap();
    for row in rows {
        let text: String = row.get(0);
        for private in [
            "Reuse the first request",
            "Keep retries stable",
            "fresh intent.",
            "when_to_use",
            "content_json",
            "original_words\"",
        ] {
            assert!(
                !text.contains(private),
                "audit/receipt disclosed content: {private}"
            );
        }
    }
}

#[tokio::test]
async fn forged_identity_unbound_foreign_and_ended_sessions_are_refused_without_writes() {
    let (_guard, admin, db, store) = setup().await;
    members(&admin, &db).await;
    let sa = start(&store, A, "own-session").await;
    let sb = start(&store, B, "other-session").await;
    for field in [
        "author_person_id",
        "person_id",
        "source_session_id",
        "source_work_id",
    ] {
        let mut p = proposal(&store, &sa, &format!("forged-{field}")).await;
        p.args[field] = json!("member-two");
        assert!(
            store
                .commands()
                .execute(TENANT, PROJECT, A, p)
                .await
                .is_err(),
            "{field}"
        );
    }
    for (sid, key) in [(sb.as_str(), "foreign"), ("absent", "missing")] {
        let p = proposal(&store, sid, key).await;
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, p).await,
            Err(PgError::Forbidden)
        ));
    }
    admin
        .execute(
            "UPDATE awr_team.sessions SET work_id='' WHERE id=$1",
            &[&sa],
        )
        .await
        .unwrap();
    let p = proposal(&store, &sa, "unbound").await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, p).await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.sessions SET work_id='a',state='ended' WHERE id=$1",
            &[&sa],
        )
        .await
        .unwrap();
    let p = proposal(&store, &sa, "ended").await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, p).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.lessons", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn other_agents_cannot_confirm_edit_or_revoke_and_promotion_requires_supervisor_duty() {
    let (_guard, admin, db, store) = setup().await;
    members(&admin, &db).await;
    let sa = start(&store, A, "own").await;
    let sb = start(&store, B, "other").await;
    let result = store
        .commands()
        .execute(TENANT, PROJECT, A, proposal(&store, &sa, "propose").await)
        .await
        .unwrap();
    let id = result["receipt"]["data"]["lesson_id"].as_str().unwrap();
    for (op, edit) in [
        ("lesson.confirm", false),
        ("lesson.confirm", true),
        ("lesson.revoke", false),
    ] {
        let mut c = change(&store, B, &sb, op, id, "1", &format!("foreign-{op}-{edit}")).await;
        if edit {
            c.args["content"] = content();
        }
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, B, c).await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(
            list(&store, A).await["data"]["items"][0]["lesson_version"],
            "1"
        );
    }
    let early = change(&store, B, &sb, "lesson.promote", id, "1", "early-promote").await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, early).await,
        Err(PgError::PreconditionsChanged)
    ));
    let confirm = change(&store, A, &sa, "lesson.confirm", id, "1", "confirm").await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, confirm)
        .await
        .unwrap();
    let denied = change(
        &store,
        A,
        &sa,
        "lesson.promote",
        id,
        "2",
        "developer-promote",
    )
    .await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, denied).await,
        Err(PgError::Forbidden)
    ));
    // A legacy management template alone is insufficient: the supervisor duty is explicit.
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET business_roles=NULL WHERE actor_id='reviewer'",
        )
        .await
        .unwrap();
    let denied = change(&store, B, &sb, "lesson.promote", id, "2", "admin-promote").await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, denied).await,
        Err(PgError::Forbidden)
    ));
    let c = change(&store, A, &sa, "lesson.revoke", id, "2", "revoke").await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, c)
        .await
        .unwrap();
    let mut c = change(&store, A, &sa, "lesson.confirm", id, "3", "edit-revoked").await;
    c.args["content"] = content();
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, c).await,
        Err(PgError::PreconditionsChanged)
    ));
}

#[tokio::test]
async fn stable_request_replay_changed_intent_stale_versions_and_revoked_delegation() {
    let (_guard, admin, db, store) = setup().await;
    members(&admin, &db).await;
    let sa = start(&store, A, "own").await;
    let p = proposal(&store, &sa, "stable-proposal").await;
    let first = store
        .commands()
        .execute(TENANT, PROJECT, A, p.clone())
        .await
        .unwrap();
    let repeated = store
        .commands()
        .execute(TENANT, PROJECT, A, p.clone())
        .await
        .unwrap();
    assert_eq!(first["receipt"], repeated["receipt"]);
    assert_eq!(repeated["replayed"], true);
    let id = first["receipt"]["data"]["lesson_id"].as_str().unwrap();
    let mut different = p.clone();
    different.args["content"]["original_words"] = json!("Different intent");
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, different)
            .await,
        Err(PgError::IdempotencyConflict)
    ));
    let stale = change(&store, A, &sa, "lesson.confirm", id, "2", "stale").await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, stale).await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .batch_execute("UPDATE awr_team.agent_authorizations SET status='revoked'")
        .await
        .unwrap();
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, p).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.lessons", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn reads_reject_cross_project_cross_stream_no_grant_and_mismatched_stream_and_are_read_only()
{
    let (_guard, admin, db, store) = setup().await;
    members(&admin, &db).await;
    let sa = start(&store, A, "own").await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, proposal(&store, &sa, "propose").await)
        .await
        .unwrap();
    let before = admin
        .query_one(
            "SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get::<_, i64>(0);
    let mut q = query("lessons.list");
    q.work_id = Some("a".into());
    for (tenant, project, token) in [
        (TENANT, "different-project", A),
        ("other-tenant", PROJECT, A),
        (TENANT, PROJECT, NONE),
    ] {
        assert!(matches!(
            store.query(tenant, project, token, q.clone()).await,
            Err(PgError::Forbidden)
        ));
    }
    let mut hidden = q.clone();
    hidden.work_id = Some("b-private".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, hidden).await,
        Err(PgError::Forbidden)
    ));
    let mut mismatch = q.clone();
    mismatch.workstream_id = Some(Id::from(2));
    assert!(matches!(
        store.query(TENANT, PROJECT, A, mismatch).await,
        Err(PgError::Forbidden)
    ));
    let mut tiny = q.clone();
    tiny.max_context_bytes = Some(1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, tiny).await,
        Err(PgError::ResponseTooLarge)
    ));
    list(&store, A).await;
    inbox(&store, A).await;
    assert_eq!(
        admin
            .query_one(
                "SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
                &[&TENANT, &PROJECT]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        before
    );
    let app = common::connect_config(&common::with_app_role(&common::test_config(), &db)).await;
    app.batch_execute("SET awr.tenant_id='other-tenant'; SET awr.project_id='reader-project'")
        .await
        .unwrap();
    assert_eq!(
        app.query_one("SELECT count(*) FROM awr_team.lessons", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let denied=app.execute("INSERT INTO awr_team.lessons(tenant_id,project_id,id,work_id,session_id,author_person_id,recorded_by_actor_id,recorded_by_client_id,content_json)
        VALUES($1,$2,'foreign-insert','a',$3,'member-one','agent','cli-a','{}')",&[&TENANT,&PROJECT,&sa]).await.unwrap_err();
    assert_eq!(
        denied.code(),
        Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE)
    );
}

#[tokio::test]
async fn inbox_and_lesson_pages_include_every_candidate_once() {
    let (_guard, admin, db, store) = setup().await;
    members(&admin, &db).await;
    let sa = start(&store, A, "own").await;
    for i in 0..22 {
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                proposal(&store, &sa, &format!("propose-{i}")).await,
            )
            .await
            .unwrap();
    }
    let items = inbox(&store, A).await;
    let candidates: Vec<_> = items
        .iter()
        .filter(|x| x["guidance"]["code"] == "confirm_lesson")
        .collect();
    assert_eq!(candidates.len(), 22);
    let keys: std::collections::BTreeSet<_> = candidates
        .iter()
        .map(|x| x["item_key"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), 22);
    let mut q = query("lessons.list");
    q.work_id = Some("a".into());
    q.limit = Some(3);
    let mut ids = std::collections::BTreeSet::new();
    loop {
        let page = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
        for item in page["data"]["items"].as_array().unwrap() {
            assert!(ids.insert(item["lesson_id"].as_str().unwrap().to_owned()));
        }
        match page["data"]["next_cursor"].as_str() {
            Some(c) => q.cursor = Some(c.into()),
            None => break,
        }
    }
    assert_eq!(ids.len(), 22);
    q.work_id = Some("c".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
}

#[tokio::test]
async fn migration67_is_atomic_repeatable_and_enforces_project_row_security() {
    let (_guard, admin, _) = common::historical_team_schema(66).await;
    let ddl = include_str!("../migrations/20261122000067_lessons.sql");
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
    assert_eq!(
        admin
            .query_one("SELECT version FROM awr_team.schema_state", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        66
    );
    assert!(
        admin
            .query_one("SELECT to_regclass('awr_team.lessons')::text", &[])
            .await
            .unwrap()
            .get::<_, Option<String>>(0)
            .is_none()
    );
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::migrate(&admin).await.unwrap();
    assert_eq!(
        admin
            .query_one("SELECT version FROM awr_team.schema_state", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        67
    );
    for table in ["lessons", "lesson_feedback"] {
        let r=admin.query_one("SELECT relrowsecurity,relforcerowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='awr_team' AND c.relname=$1",&[&table]).await.unwrap();
        assert!(r.get::<_, bool>(0));
        assert!(r.get::<_, bool>(1));
    }
}

#[tokio::test]
async fn a_withdrawn_candidate_stays_withdrawn_and_other_members_can_report_applicability() {
    let (_guard, admin, db, store) = setup().await;
    members(&admin, &db).await;
    let sa = start(&store, A, "own").await;
    let sb = start(&store, B, "other").await;
    let created = store
        .commands()
        .execute(TENANT, PROJECT, A, proposal(&store, &sa, "propose").await)
        .await
        .unwrap();
    let id = created["receipt"]["data"]["lesson_id"].as_str().unwrap();
    let mut feedback = change(
        &store,
        B,
        &sb,
        "lesson.mark_not_applicable",
        id,
        "1",
        "feedback",
    )
    .await;
    feedback.args["reason"] = json!("This guidance covers a different request pattern.");
    store
        .commands()
        .execute(TENANT, PROJECT, B, feedback)
        .await
        .unwrap();
    assert_eq!(
        list(&store, A).await["data"]["items"][0]["not_applicable_count"],
        1
    );
    assert_eq!(list(&store, B).await["data"]["items"], json!([]));
    let revoke = change(&store, A, &sa, "lesson.revoke", id, "2", "revoke-candidate").await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, revoke)
        .await
        .unwrap();
    let confirm = change(
        &store,
        A,
        &sa,
        "lesson.confirm",
        id,
        "3",
        "confirm-withdrawn-candidate",
    )
    .await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, confirm).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        list(&store, A).await["data"]["items"][0]["stage"],
        "candidate"
    );
    assert_eq!(
        list(&store, A).await["data"]["items"][0]["status"],
        "revoked"
    );
    assert!(
        !inbox(&store, A)
            .await
            .iter()
            .any(|x| x["guidance"]["code"] == "confirm_lesson")
    );
}
