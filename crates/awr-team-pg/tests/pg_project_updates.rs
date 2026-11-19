#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, Id, IssueAuthorizationRequest, PersonId, RevokeAuthorizationRequest,
};
use awr_team_pg::{
    AuthorizationStore, PgError, PgResult, UpdateRecordRequest, WorkstreamQuery,
    WorkstreamReadStore,
};
use fixture::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn request(id: &str, week: &str, summary: &str) -> UpdateRecordRequest {
    let (starts_on, ends_on) = match week {
        "2026-W46" => ("2026-11-09", "2026-11-15"),
        _ => ("2026-11-16", "2026-11-22"),
    };
    serde_json::from_value(json!({"protocol_version":1,"request_id":id,
        "period_kind":"week","period_key":week,"starts_on":starts_on,
        "ends_on":ends_on,"status":"risk","summary":summary}))
    .unwrap()
}

async fn supervisor_identity(admin: &tokio_postgres::Client) {
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
        UPDATE awr_team.project_memberships SET role='maintainer',business_roles='[\"supervisor\"]'::jsonb
          WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND actor_id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
          VALUES('reader-tenant','reader-project','lead','Jordan Lee','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
          VALUES('reader-tenant','reader-project','lead-binding','lead','agent','active');
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,
          authority_version,can_read,can_write,active)
          VALUES('reader-tenant','reader-project','agent','cli-a','00000000000000000000000002',1,true,true,true);
        UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1
          WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND actor_id='agent' AND client_id='cli-a';").await.unwrap();
}

async fn delegate(db: &str, id: &str, scope: AuthorizationScope, actions: &[AuthorizedAction]) {
    let authorization = AgentAuthorization {
        id: id.into(),
        authorizer_person_id: PersonId::new("lead").unwrap(),
        responsible_person_id: PersonId::new("lead").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope,
        actions: actions.iter().copied().collect::<BTreeSet<_>>(),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1000,
        binding_id: Some("lead-binding".into()),
    };
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: format!("issue-{id}"),
                authorization,
            },
        )
        .await
        .unwrap();
}

async fn supervisor(admin: &tokio_postgres::Client, db: &str) {
    supervisor_identity(admin).await;
    delegate(
        db,
        "update-project",
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        &[AuthorizedAction::Inspect, AuthorizedAction::EditPlanning],
    )
    .await;
}

async fn record(
    store: &WorkstreamReadStore,
    tenant: &str,
    token: &str,
    request: &UpdateRecordRequest,
) -> PgResult<Value> {
    store
        .commands()
        .record_update(tenant, PROJECT, token, request)
        .await
}

async fn read(store: &WorkstreamReadStore, token: &str, query: WorkstreamQuery) -> PgResult<Value> {
    store.query(TENANT, PROJECT, token, query).await
}

async fn update_rows(admin: &tokio_postgres::Client) -> i64 {
    admin
        .query_one("SELECT count(*) FROM awr_team.project_period_updates", &[])
        .await
        .unwrap()
        .get(0)
}

fn save_verbatim(name: &str, value: &Value) {
    let Ok(directory) = std::env::var("AWR_PROJECT_UPDATE_SAMPLE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    std::fs::write(directory.join(name), bytes).unwrap();
}

#[tokio::test]
async fn record_versions_replay_and_read_verbatim_pages() {
    let (_guard, admin, _db, store) = setup().await;
    supervisor(&admin, &_db).await;
    let first = record(
        &store,
        TENANT,
        A,
        &request(
            "week-47-v1",
            "2026-W47",
            "  The lead wrote this exact sentence.  ",
        ),
    )
    .await
    .unwrap();
    assert_eq!(first["replayed"], false);
    assert_eq!(first["receipt"]["version"], "1");
    assert_eq!(first["receipt"]["fact_snapshot_ref"], Value::Null);
    assert_eq!(first["receipt"]["author"]["display_name"], "Jordan Lee");
    assert_eq!(
        first["receipt"]["summary"],
        "  The lead wrote this exact sentence.  "
    );
    let first_id = first["receipt"]["update_id"].as_str().unwrap().to_owned();
    let old_row: Value = admin
        .query_one(
            "SELECT to_jsonb(u) FROM awr_team.project_period_updates u WHERE id=$1",
            &[&first_id],
        )
        .await
        .unwrap()
        .get(0);
    let replay = record(
        &store,
        TENANT,
        A,
        &request(
            "week-47-v1",
            "2026-W47",
            "  The lead wrote this exact sentence.  ",
        ),
    )
    .await
    .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], first["receipt"]);
    assert_eq!(update_rows(&admin).await, 1);
    let mut changed = request("week-47-v1", "2026-W47", "Changed intent");
    assert!(matches!(
        record(&store, TENANT, A, &changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(update_rows(&admin).await, 1);

    changed = request(
        "week-47-v2",
        "2026-W47",
        "A revised judgment, recorded as a new version.",
    );
    changed.status = "off".into();
    let second = record(&store, TENANT, A, &changed).await.unwrap();
    assert_eq!(second["receipt"]["version"], "2");
    assert_eq!(second["receipt"]["supersedes_update_id"], first_id);
    assert_eq!(update_rows(&admin).await, 2);
    let retained: Value = admin
        .query_one(
            "SELECT to_jsonb(u) FROM awr_team.project_period_updates u WHERE id=$1",
            &[&first_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        retained, old_row,
        "the old version must remain byte-for-byte unchanged"
    );

    let earlier = record(
        &store,
        TENANT,
        A,
        &request(
            "week-46-v1",
            "2026-W46",
            "The preceding week remains available.",
        ),
    )
    .await
    .unwrap();
    assert_eq!(update_rows(&admin).await, 3);

    let mut page_query = query("updates.list");
    page_query.limit = Some(1);
    let page_one = read(&store, A, page_query.clone()).await.unwrap();
    assert_eq!(page_one["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(page_one["data"]["items"][0]["version"], "2");
    assert_eq!(
        page_one["data"]["items"][0]["fact_snapshot_ref"],
        Value::Null
    );
    assert_eq!(page_one["data"]["truncated"], true);
    page_query.cursor = page_one["data"]["next_cursor"].as_str().map(str::to_owned);
    let page_two = read(&store, A, page_query.clone()).await.unwrap();
    assert_eq!(
        page_two["data"]["items"][0]["update_id"],
        earlier["receipt"]["update_id"]
    );
    assert_ne!(
        page_two["data"]["items"][0]["update_id"],
        page_one["data"]["items"][0]["update_id"]
    );
    assert_eq!(page_two["data"]["next_cursor"], Value::Null);
    let mut tampered = page_query.clone();
    let mut cursor: Value = serde_json::from_str(tampered.cursor.as_deref().unwrap()).unwrap();
    cursor["starts_on"] = json!("0000-00-00");
    tampered.cursor = Some(serde_json::to_string(&cursor).unwrap());
    assert!(matches!(
        read(&store, A, tampered).await,
        Err(PgError::CursorExpired)
    ));
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        read(&store, A, page_query).await,
        Err(PgError::CursorExpired)
    ));

    let mut inspect_query = query("update.inspect");
    inspect_query.update_id = Some(first_id);
    let inspected = read(&store, A, inspect_query).await.unwrap();
    assert_eq!(inspected["data"]["update"]["current"], false);
    assert_eq!(inspected["data"]["update"]["version"], "1");
    assert_eq!(
        inspected["data"]["update"]["author"]["display_name"],
        "Jordan Lee"
    );
    assert!(inspected["data"]["update"]["recorded_at_unix_ms"].is_i64());
    save_verbatim("updates-list.json", &page_one);
    save_verbatim("update-inspect.json", &inspected);

    // These committed samples are parsed from files produced by the same real read path.
    for (path, value) in [
        ("../tests/samples/updates-list.json", &page_one),
        ("../tests/samples/update-inspect.json", &inspected),
    ] {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        if full.exists() {
            let sample: Value = serde_json::from_slice(&std::fs::read(full).unwrap()).unwrap();
            assert_eq!(sample["protocol_version"], value["protocol_version"]);
            assert_eq!(sample["selection_basis"], "project");
            let (sample_record, live_record) = if path.contains("updates-list") {
                (&sample["data"]["items"][0], &value["data"]["items"][0])
            } else {
                (&sample["data"]["update"], &value["data"]["update"])
            };
            for field in [
                "period_kind",
                "period_key",
                "starts_on",
                "ends_on",
                "status",
                "summary",
                "version",
                "current",
                "fact_snapshot_ref",
            ] {
                assert_eq!(
                    sample_record[field], live_record[field],
                    "stale sample: {field}"
                );
            }
            assert_eq!(sample_record["author"]["display_name"], "Jordan Lee");
            assert!(sample_record["update_id"].is_string());
            assert!(sample_record["recorded_at_unix_ms"].is_i64());
        }
    }

    let month: UpdateRecordRequest = serde_json::from_value(json!({
        "protocol_version":1,"request_id":"month-12-v1","period_kind":"month",
        "period_key":"2026-12","starts_on":"2026-12-01","ends_on":"2026-12-31",
        "status":"ok","summary":"A monthly judgment supplied by the supervisor."}))
    .unwrap();
    let (left, right) = tokio::join!(
        record(&store, TENANT, A, &month),
        record(&store, TENANT, A, &month)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left["receipt"], right["receipt"]);
    assert_ne!(left["replayed"], right["replayed"]);
    let all = read(&store, A, query("updates.list")).await.unwrap();
    assert_eq!(all["data"]["items"][0]["period_kind"], "month");
    assert_eq!(all["data"]["items"][0]["period_key"], "2026-12");
    assert_eq!(all["data"]["items"][0]["starts_on"], "2026-12-01");
    assert_eq!(all["data"]["items"][0]["ends_on"], "2026-12-31");
    assert_eq!(update_rows(&admin).await, 4);
}

#[tokio::test]
async fn whitespace_only_summaries_are_invalid_input_and_write_nothing() {
    let (_guard, admin, db, store) = setup().await;
    supervisor(&admin, &db).await;
    for (index, summary) in ["\u{3000}", "\u{00a0}", "", "   ", "\t", "\n", "\t\n"]
        .into_iter()
        .enumerate()
    {
        let invalid = request(&format!("blank-update-{index}"), "2026-W47", summary);
        let error = record(&store, TENANT, A, &invalid).await.unwrap_err();
        assert!(
            error.is_invalid_command_fields(),
            "blank summary must be InvalidInput, got {error:?}"
        );
    }
    assert_eq!(update_rows(&admin).await, 0);
    let counts = admin
        .query_one(
            "SELECT
                (SELECT count(*) FROM awr_team.operations WHERE op='update.record'),
                (SELECT count(*) FROM awr_team.events WHERE event_type='update.recorded')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(counts.get::<_, i64>(0), 0);
    assert_eq!(counts.get::<_, i64>(1), 0);
}

#[tokio::test]
async fn every_write_refusal_leaves_no_update_operation_or_event() {
    let (_guard, admin, db, store) = setup().await;
    let req = request("denied-update", "2026-W47", "A synthetic status sentence.");

    let too_long = "雪".repeat(201);
    for (index, summary) in [" \t\n", too_long.as_str(), "bad\nsummary"]
        .into_iter()
        .enumerate()
    {
        let invalid = request(&format!("invalid-update-{index}"), "2026-W47", summary);
        let error = record(&store, TENANT, A, &invalid).await.unwrap_err();
        assert!(error.is_invalid_command_fields(), "{error:?}");
    }
    assert_eq!(update_rows(&admin).await, 0);

    // A supervisor member Agent still needs a live, project-scoped covering delegation.
    supervisor_identity(&admin).await;
    assert!(matches!(
        record(&store, TENANT, A, &req).await,
        Err(PgError::Forbidden)
    ));
    delegate(
        &db,
        "update-stream-only",
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
        &[AuthorizedAction::Inspect, AuthorizedAction::EditPlanning],
    )
    .await;
    assert!(matches!(
        record(&store, TENANT, A, &req).await,
        Err(PgError::Forbidden)
    ));

    // Catch a removed supervisor check specifically: every other write precondition is valid.
    delegate(
        &db,
        "update-project",
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        &[AuthorizedAction::Inspect, AuthorizedAction::EditPlanning],
    )
    .await;
    admin
        .execute(
            "UPDATE awr_team.project_memberships SET business_roles=NULL WHERE actor_id='agent'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        record(&store, TENANT, A, &req).await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.project_memberships SET business_roles='[\"supervisor\"]'::jsonb WHERE actor_id='agent'",
            &[],
        )
        .await
        .unwrap();

    // Even an explicit supervisor duty cannot turn a read-only member into a writer.
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET actor_id='reviewer' WHERE id='reader-b';
        UPDATE awr_team.project_memberships SET business_roles='[\"supervisor\"]'::jsonb
          WHERE actor_id='reviewer';",
        )
        .await
        .unwrap();
    assert!(matches!(
        record(&store, TENANT, B, &req).await,
        Err(PgError::Forbidden)
    ));

    // A valid project delegation and write grants never cross the URL's project boundary.
    assert!(matches!(
        record(&store, "other-tenant", A, &req).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(update_rows(&admin).await, 0);
    let counts = admin
        .query_one(
            "SELECT
        (SELECT count(*) FROM awr_team.operations WHERE op='update.record'),
        (SELECT count(*) FROM awr_team.events WHERE event_type='update.recorded')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(counts.get::<_, i64>(0), 0);
    assert_eq!(counts.get::<_, i64>(1), 0);
}

#[tokio::test]
async fn reads_require_project_visibility_and_never_promote_a_task_delegation() {
    let (_guard, admin, db, store) = setup().await;
    supervisor(&admin, &db).await;
    let recorded = record(
        &store,
        TENANT,
        A,
        &request(
            "read-scope-update",
            "2026-W47",
            "Only authorized project readers can see this.",
        ),
    )
    .await
    .unwrap();
    let update_id = recorded["receipt"]["update_id"].as_str().unwrap();

    // A read-only human member can read but cannot use the write tool.
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET actor_id='reviewer' WHERE id='reader-b';
        UPDATE awr_team.workstream_grants SET actor_id='reviewer' WHERE client_id='cli-b';
        UPDATE awr_team.project_memberships SET business_roles='[\"observer\"]'::jsonb
          WHERE actor_id='reviewer';",
        )
        .await
        .unwrap();
    let readonly = read(&store, B, query("updates.list")).await.unwrap();
    assert_eq!(readonly["data"]["items"].as_array().unwrap().len(), 1);
    assert!(matches!(
        record(
            &store,
            TENANT,
            B,
            &request("readonly-write", "2026-W46", "Not allowed")
        )
        .await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store
            .query("other-tenant", PROJECT, A, query("updates.list"))
            .await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        read(&store, NONE, query("updates.list")).await,
        Err(PgError::Forbidden)
    ));
    let mut missing = query("update.inspect");
    missing.update_id = Some("not-in-this-project".into());
    assert!(matches!(
        read(&store, A, missing).await,
        Err(PgError::Forbidden)
    ));

    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db))
        .revoke(
            TENANT,
            PROJECT,
            &RevokeAuthorizationRequest {
                request_key: "revoke-project-update".into(),
                authorization_id: "update-project".into(),
                revoked_by: PersonId::new("lead").unwrap(),
                revoked_at_ms: 2000,
                reason: "Synthetic scope test".into(),
            },
        )
        .await
        .unwrap();
    delegate(
        &db,
        "update-task-only",
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
        &[AuthorizedAction::Inspect],
    )
    .await;
    assert!(matches!(
        read(&store, A, query("updates.list")).await,
        Err(PgError::Forbidden)
    ));
    let mut exact = query("update.inspect");
    exact.update_id = Some(update_id.into());
    assert!(matches!(
        read(&store, A, exact).await,
        Err(PgError::Forbidden)
    ));

    delegate(
        &db,
        "update-stream-read",
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
        &[AuthorizedAction::Inspect],
    )
    .await;
    let scoped = read(&store, A, query("updates.list")).await.unwrap();
    assert_eq!(scoped["data"]["items"][0]["update_id"], update_id);
    assert!(!scoped.to_string().contains("PRIVATE NEXT ACTION"));
}
