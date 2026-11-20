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

#[allow(clippy::too_many_arguments)]
async fn event(
    admin: &tokio_postgres::Client,
    tenant: &str,
    id: &str,
    revision: i64,
    event_type: &str,
    work_id: &str,
    at: &str,
    payload: Value,
) {
    admin
        .execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,
                event_type,actor_id,work_id,payload_json,created_at)
             VALUES($1,$2,$3,$4,0,$5,'agent',$6,$7,$8::text::timestamptz)",
            &[
                &tenant,
                &PROJECT,
                &id,
                &revision,
                &event_type,
                &work_id,
                &payload,
                &at,
            ],
        )
        .await
        .unwrap();
}

async fn checkpoint(
    admin: &tokio_postgres::Client,
    id: &str,
    session: &str,
    phase: &str,
    at: &str,
) {
    admin
        .execute(
            "INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,
                contract_hash,observed_revision,next_action,open_loops_json,progress_json,created_at)
             VALUES($1,$2,$3,$4,'context','contract',1,'Synthetic checkpoint','[]',$5,$6::text::timestamptz)",
            &[
                &TENANT,
                &PROJECT,
                &id,
                &session,
                &json!({"phase":phase,"summary":"Raw synthetic report body","blockers":["Raw blocker"]}),
                &at,
            ],
        )
        .await
        .unwrap();
}

async fn stored_snapshot(admin: &tokio_postgres::Client, update_id: &str) -> Value {
    admin
        .query_one(
            "SELECT fact_snapshot_json FROM awr_team.project_period_updates WHERE id=$1",
            &[&update_id],
        )
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
async fn fact_snapshots_follow_the_reference_table_window_and_immutability() {
    let (_guard, admin, db, store) = setup().await;
    supervisor(&admin, &db).await;
    admin
        .execute(
            "UPDATE awr_team.projects SET settings_json='{\"time_zone\":\"America/New_York\"}'
             WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();

    for (id, revision, kind, work, at, payload) in [
        (
            "verified-before",
            100,
            "work.completed",
            "c",
            "2026-11-16T04:59:59Z",
            json!({"team_independent_acceptance":true}),
        ),
        (
            "verified-start",
            101,
            "work.completed",
            "a",
            "2026-11-16T05:00:00Z",
            json!({"team_independent_acceptance":true}),
        ),
        (
            "verified-end",
            102,
            "work.completed",
            "d",
            "2026-11-23T05:00:00Z",
            json!({"team_independent_acceptance":true}),
        ),
        (
            "legacy-open",
            103,
            "wait.opened",
            "a",
            "2026-11-16T06:00:00Z",
            json!({}),
        ),
        (
            "decision-open",
            104,
            "decision.request",
            "c",
            "2026-11-17T06:00:00Z",
            json!({"deduplicated":false}),
        ),
        (
            "decision-dedup",
            105,
            "decision.request",
            "a",
            "2026-11-17T07:00:00Z",
            json!({"deduplicated":true}),
        ),
        (
            "review-open",
            106,
            "review.opened",
            "a",
            "2026-11-17T08:00:00Z",
            json!({}),
        ),
        (
            "review-reject",
            107,
            "review.decided",
            "a",
            "2026-11-18T08:00:00Z",
            json!({"decision":"reject"}),
        ),
        (
            "review-approve",
            108,
            "review.decided",
            "c",
            "2026-11-18T09:00:00Z",
            json!({"decision":"approve"}),
        ),
        (
            "decision-answer",
            109,
            "decision.respond",
            "c",
            "2026-11-18T10:00:00Z",
            json!({"applied":true}),
        ),
        (
            "decision-noop",
            110,
            "decision.respond",
            "a",
            "2026-11-18T10:30:00Z",
            json!({"applied":false}),
        ),
        (
            "decision-cancel",
            111,
            "decision.cancel",
            "c",
            "2026-11-18T11:00:00Z",
            json!({"applied":true}),
        ),
        (
            "legacy-reply",
            112,
            "wait.replied",
            "a",
            "2026-11-18T12:00:00Z",
            json!({}),
        ),
        (
            "external-open",
            113,
            "wait.opened",
            "b-private",
            "2026-11-18T13:00:00Z",
            json!({"kind":"external"}),
        ),
        (
            "external-reply",
            114,
            "wait.replied",
            "b-private",
            "2026-11-18T14:00:00Z",
            json!({"kind":"external"}),
        ),
    ] {
        event(&admin, TENANT, id, revision, kind, work, at, payload).await;
    }
    event(
        &admin,
        "other-tenant",
        "foreign-completion",
        115,
        "work.completed",
        "foreign-private-work",
        "2026-11-18T15:00:00Z",
        json!({}),
    )
    .await;

    checkpoint(
        &admin,
        "phase-a-before",
        "session-a",
        "implementing",
        "2026-11-16T04:00:00Z",
    )
    .await;
    checkpoint(
        &admin,
        "phase-a-wait",
        "session-a",
        "waiting_dependency",
        "2026-11-17T06:00:00Z",
    )
    .await;
    checkpoint(
        &admin,
        "phase-a-repeat",
        "session-a",
        "waiting_dependency",
        "2026-11-17T07:00:00Z",
    )
    .await;
    checkpoint(
        &admin,
        "phase-a-clear",
        "session-a",
        "testing",
        "2026-11-18T06:00:00Z",
    )
    .await;
    checkpoint(
        &admin,
        "phase-a-new-claim-after-old-verification",
        "session-a",
        "delivered",
        "2026-11-18T07:00:00Z",
    )
    .await;
    checkpoint(
        &admin,
        "phase-b-before",
        "session-b",
        "implementing",
        "2026-11-16T04:00:00Z",
    )
    .await;
    checkpoint(
        &admin,
        "phase-b-claim",
        "session-b",
        "delivered",
        "2026-11-18T15:00:00Z",
    )
    .await;
    checkpoint(
        &admin,
        "phase-b-repeat",
        "session-b",
        "delivered",
        "2026-11-18T16:00:00Z",
    )
    .await;

    let req = request(
        "facts-week-47-v1",
        "2026-W47",
        "A private caller sentence that must never enter the facts.",
    );
    let first = record(&store, TENANT, A, &req).await.unwrap();
    let first_id = first["receipt"]["update_id"].as_str().unwrap().to_owned();
    let first_ref = first["receipt"]["fact_snapshot_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let expected = json!({
        "verified":{"count":1,"work_ids":["a"]},
        "completed_without_independent_review":{"count":0,"work_ids":[]},
        "waits_opened":{"count":5,"kinds":[
            {"kind":"decision","count":1,"work_ids":["c"]},
            {"kind":"dependency","count":1,"work_ids":["a"]},
            {"kind":"review","count":1,"work_ids":["a"]},
            {"kind":"external","count":1,"work_ids":["b-private"]},
            {"kind":"unknown","count":1,"work_ids":["a"]}
        ]},
        "waits_cleared":{"count":7,"kinds":[
            {"kind":"decision","count":2,"work_ids":["c","c"]},
            {"kind":"dependency","count":1,"work_ids":["a"]},
            {"kind":"review","count":2,"work_ids":["a","c"]},
            {"kind":"external","count":1,"work_ids":["b-private"]},
            {"kind":"unknown","count":1,"work_ids":["a"]}
        ]},
        "review_returned":{"count":1,"work_ids":["a"]},
        "delivered_unverified":{"count":2,"work_ids":["a","b-private"]},
        "decisions_made":{"count":1,"work_ids":["c"]}
    });
    assert_eq!(stored_snapshot(&admin, &first_id).await, expected);
    let text = expected.to_string();
    assert!(!text.contains("private caller sentence"));
    assert!(!text.contains("Raw synthetic report body"));
    assert!(!text.contains("Raw blocker"));
    assert!(!text.contains("foreign-private-work"));
    assert!(!text.contains("lead"));
    assert!(!text.contains("Jordan"));

    let mut inspect_query = query("update.inspect");
    inspect_query.update_id = Some(first_id.clone());
    let inspected = read(&store, A, inspect_query.clone()).await.unwrap();
    assert_eq!(inspected["data"]["fact_snapshot"], expected);
    assert_eq!(
        inspected["data"]["fact_snapshot_coverage"],
        json!({"complete":true,"hidden_work_count":0})
    );
    let window = admin
        .query_one(
            "SELECT fact_window_start_unix_ms,fact_window_end_unix_ms,fact_time_zone,
                fact_window_start_unix_ms=(extract(epoch FROM '2026-11-16T05:00:00Z'::timestamptz)*1000)::bigint,
                fact_window_end_unix_ms=(extract(epoch FROM '2026-11-23T05:00:00Z'::timestamptz)*1000)::bigint
             FROM awr_team.project_period_updates WHERE id=$1",
            &[&first_id],
        )
        .await
        .unwrap();
    assert!(window.get::<_, bool>(3) && window.get::<_, bool>(4));
    assert_eq!(window.get::<_, String>(2), "America/New_York");

    let original_row: Value = admin
        .query_one(
            "SELECT to_jsonb(u) FROM awr_team.project_period_updates u WHERE id=$1",
            &[&first_id],
        )
        .await
        .unwrap()
        .get(0);
    event(
        &admin,
        TENANT,
        "verified-after-first-snapshot",
        500,
        "work.completed",
        "b-private",
        "2026-11-19T05:00:00Z",
        json!({"team_independent_acceptance":true}),
    )
    .await;
    let replay = record(&store, TENANT, A, &req).await.unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"]["fact_snapshot_ref"], first_ref);
    assert_eq!(stored_snapshot(&admin, &first_id).await, expected);
    assert_eq!(update_rows(&admin).await, 1);
    let after_replay = read(&store, A, inspect_query.clone()).await.unwrap();
    assert_eq!(after_replay["data"]["fact_snapshot"], expected);
    assert!(
        admin
            .execute(
                "UPDATE awr_team.project_period_updates SET fact_snapshot_json='{}' WHERE id=$1",
                &[&first_id],
            )
            .await
            .is_err()
    );
    admin.batch_execute("ROLLBACK").await.unwrap();

    let second = record(
        &store,
        TENANT,
        A,
        &request("facts-week-47-v2", "2026-W47", "A revised judgment."),
    )
    .await
    .unwrap();
    let second_id = second["receipt"]["update_id"].as_str().unwrap();
    assert_ne!(second["receipt"]["fact_snapshot_ref"], first_ref);
    assert_eq!(
        stored_snapshot(&admin, second_id).await["verified"]["count"],
        2
    );
    assert_eq!(
        stored_snapshot(&admin, second_id).await["delivered_unverified"],
        json!({"count":1,"work_ids":["a"]})
    );
    assert_eq!(stored_snapshot(&admin, &first_id).await, expected);
    let retained: Value = admin
        .query_one(
            "SELECT to_jsonb(u) FROM awr_team.project_period_updates u WHERE id=$1",
            &[&first_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(retained, original_row);
}

#[tokio::test]
async fn fact_snapshots_default_to_utc_and_refuse_unknown_stored_zones() {
    let (_guard, admin, db, store) = setup().await;
    supervisor(&admin, &db).await;
    event(
        &admin,
        TENANT,
        "utc-start",
        100,
        "work.completed",
        "a",
        "2026-11-16T00:00:00Z",
        json!({"team_independent_acceptance":true}),
    )
    .await;
    let first = record(
        &store,
        TENANT,
        A,
        &request("utc-week", "2026-W47", "UTC is the default."),
    )
    .await
    .unwrap();
    let id = first["receipt"]["update_id"].as_str().unwrap();
    assert_eq!(stored_snapshot(&admin, id).await["verified"]["count"], 1);
    let zone = admin
        .query_one(
            "SELECT fact_time_zone,fact_window_start_unix_ms=(extract(epoch FROM '2026-11-16T00:00:00Z'::timestamptz)*1000)::bigint
             FROM awr_team.project_period_updates WHERE id=$1",
            &[&id],
        )
        .await
        .unwrap();
    assert_eq!(zone.get::<_, String>(0), "UTC");
    assert!(zone.get::<_, bool>(1));

    admin
        .execute(
            "UPDATE awr_team.projects SET settings_json='{\"time_zone\":\"Not/A_Real_Zone\"}'
             WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    assert!(matches!(
        record(
            &store,
            TENANT,
            A,
            &request("bad-zone", "2026-W46", "Do not silently use UTC."),
        )
        .await,
        Err(PgError::SourceDivergence)
    ));
    assert_eq!(update_rows(&admin).await, 1);
}

#[tokio::test]
async fn verified_requires_independent_acceptance_evidence() {
    let (_guard, admin, db, store) = setup().await;
    supervisor(&admin, &db).await;
    // Payloads copied verbatim from the two producers that write work.completed:
    // the current shape from workstream_command/reviews.rs (independence_kind
    // plus the derived team_independent_acceptance, no policy key) and the
    // older receipt shape from review.rs (policy only). One row per basis a
    // producer can actually write; combinations like an acceptance flag beside
    // an ordinary policy are not written by anyone and stay untested.
    let current_shape = |kind: &str| {
        json!({"receipt_id":"receipt-x","evidence_id":"evidence-x","execution_id":"execution-x",
            "independence_kind":kind,
            "team_independent_acceptance":kind == "team_independent",
            "approval_basis":"basis-x","execution_basis":"execution-basis-x",
            "human_approval":false,"approved_by_person_id":"person-x"})
    };
    for (index, (id, work, payload)) in [
        ("independence-team", "a", current_shape("team_independent")),
        ("independence-agent", "b", current_shape("agent_review")),
        (
            "independence-simulated",
            "c",
            current_shape("simulated_member_independent"),
        ),
        (
            "independence-self",
            "d",
            current_shape("personal_self_review"),
        ),
        (
            "independence-ordinary",
            "e",
            current_shape("ordinary_confirm"),
        ),
        (
            "old-agent-policy",
            "f",
            json!({"receipt_id":"receipt-y","evidence_id":"evidence-y",
                "policy":"caller_managed_execution_and_agent_review"}),
        ),
        (
            "old-simulated-policy",
            "g",
            json!({"receipt_id":"receipt-y","evidence_id":"evidence-y",
                "policy":"caller_managed_execution_and_simulated_member_review"}),
        ),
        (
            "old-ordinary-policy",
            "h",
            json!({"receipt_id":"receipt-y","evidence_id":"evidence-y",
                "policy":"ordinary_confirm"}),
        ),
        ("no-basis", "i", json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        event(
            &admin,
            TENANT,
            id,
            100 + index as i64,
            "work.completed",
            work,
            "2026-11-18T00:00:00Z",
            payload,
        )
        .await;
    }
    let recorded = record(
        &store,
        TENANT,
        A,
        &request("verified-basis", "2026-W47", "Basis decides."),
    )
    .await
    .unwrap();
    let id = recorded["receipt"]["update_id"].as_str().unwrap();
    let snapshot = stored_snapshot(&admin, id).await;
    assert_eq!(
        snapshot["verified"],
        json!({"count":5,"work_ids":["a","b","c","f","g"]}),
        "team_independent, agent_review and simulated_member_independent independence kinds, \
         plus the two old independent review policies, verify"
    );
    assert_eq!(
        snapshot["completed_without_independent_review"],
        json!({"count":4,"work_ids":["d","e","h","i"]}),
        "personal_self_review, ordinary_confirm in either shape and an empty basis stay unverified"
    );
    let mut inspect_query = query("update.inspect");
    inspect_query.update_id = Some(id.to_owned());
    let inspected = read(&store, A, inspect_query).await.unwrap();
    assert_eq!(
        inspected["data"]["fact_snapshot"]["verified"],
        json!({"count":2,"work_ids":["a","c"]}),
        "the projection keeps only the reader's visible work"
    );
    assert_eq!(
        inspected["data"]["fact_snapshot"]["completed_without_independent_review"],
        json!({"count":0,"work_ids":[]})
    );
    assert_eq!(
        inspected["data"]["fact_snapshot_coverage"],
        json!({"complete":false,"hidden_work_count":7})
    );
    assert!(
        !inspected["data"]["fact_snapshot"]
            .to_string()
            .contains("\"b\"")
    );
    let keys: Vec<&str> = inspected["data"]["fact_snapshot"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(keys.contains(&"delivered_unverified"));
    assert!(!keys.contains(&"claimed_unverified"));
}

#[tokio::test]
async fn untyped_legacy_waits_count_as_unknown_not_decisions() {
    let (_guard, admin, db, store) = setup().await;
    supervisor(&admin, &db).await;
    for (index, (id, kind, work, payload)) in [
        ("untyped-open", "wait.opened", "u", json!({})),
        ("typed-open", "wait.opened", "t", json!({"kind":"decision"})),
        ("untyped-reply", "wait.replied", "u", json!({})),
        (
            "typed-reply",
            "wait.replied",
            "t",
            json!({"kind":"decision"}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        event(
            &admin,
            TENANT,
            id,
            100 + index as i64,
            kind,
            work,
            "2026-11-18T00:00:00Z",
            payload,
        )
        .await;
    }
    let recorded = record(
        &store,
        TENANT,
        A,
        &request("untyped-waits", "2026-W47", "Kinds must match the payload."),
    )
    .await
    .unwrap();
    let snapshot =
        stored_snapshot(&admin, recorded["receipt"]["update_id"].as_str().unwrap()).await;
    assert_eq!(
        snapshot["waits_opened"],
        json!({"count":2,"kinds":[
            {"kind":"decision","count":1,"work_ids":["t"]},
            {"kind":"unknown","count":1,"work_ids":["u"]}
        ]})
    );
    assert_eq!(
        snapshot["waits_cleared"],
        json!({"count":2,"kinds":[
            {"kind":"decision","count":1,"work_ids":["t"]},
            {"kind":"unknown","count":1,"work_ids":["u"]}
        ]})
    );
    assert_eq!(
        snapshot["decisions_made"],
        json!({"count":1,"work_ids":["t"]}),
        "an untyped legacy reply is not a decision made"
    );
}

#[tokio::test]
async fn legacy_rows_without_a_snapshot_read_as_unknown_not_zero() {
    let (_guard, admin, db, store) = setup().await;
    supervisor(&admin, &db).await;
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.project_period_updates(tenant_id,project_id,id,period_kind,
               period_key,starts_on,ends_on,status,summary,author_person_id,author_display_name,
               recorded_by_actor_id,recorded_by_client_id,request_id,version)
             VALUES('{TENANT}','{PROJECT}','legacy-no-facts','week','2026-W47','2026-11-16',
               '2026-11-22','ok','A schema-64 sentence without facts.','lead','Synthetic lead',
               'agent','cli-a','legacy',1);
             INSERT INTO awr_team.project_period_update_current(tenant_id,project_id,period_kind,
               period_key,update_id)
             VALUES('{TENANT}','{PROJECT}','week','2026-W47','legacy-no-facts')"
        ))
        .await
        .unwrap();

    let listed = read(&store, A, query("updates.list")).await.unwrap();
    assert_eq!(
        listed["data"]["items"].as_array().unwrap().len(),
        1,
        "the schema-64 row is still the current version"
    );
    assert_eq!(listed["data"]["items"][0]["fact_snapshot_ref"], Value::Null);
    let mut inspect_query = query("update.inspect");
    inspect_query.update_id = Some("legacy-no-facts".into());
    let inspected = read(&store, A, inspect_query.clone()).await.unwrap();
    assert_eq!(
        inspected["data"]["update"]["fact_snapshot_ref"],
        Value::Null
    );
    assert!(
        inspected["data"]["fact_snapshot"].is_null(),
        "a row without a snapshot is unknown, not zero counts: {}",
        inspected["data"]["fact_snapshot"]
    );
    assert!(inspected["data"]["fact_snapshot_coverage"].is_null());

    let successor = record(
        &store,
        TENANT,
        A,
        &request("legacy-week", "2026-W47", "A newer judged version."),
    )
    .await
    .unwrap();
    assert_eq!(successor["receipt"]["version"], "2");
    assert!(successor["receipt"]["fact_snapshot_ref"].is_string());
    assert_eq!(update_rows(&admin).await, 2);
    let listed = read(&store, A, query("updates.list")).await.unwrap();
    assert_eq!(listed["data"]["items"].as_array().unwrap().len(), 1);
    assert!(listed["data"]["items"][0]["fact_snapshot_ref"].is_string());
    let legacy = admin
        .query_one(
            "SELECT fact_snapshot_ref,fact_snapshot_json,fact_window_start_unix_ms,
                fact_window_end_unix_ms,fact_time_zone
             FROM awr_team.project_period_updates WHERE id='legacy-no-facts'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(legacy.get::<_, Option<String>>(0), None);
    assert_eq!(legacy.get::<_, Option<Value>>(1), None);
    assert_eq!(legacy.get::<_, Option<i64>>(2), None);
    assert_eq!(legacy.get::<_, Option<i64>>(3), None);
    assert_eq!(legacy.get::<_, Option<String>>(4), None);
    let reinspected = read(&store, A, inspect_query).await.unwrap();
    assert!(reinspected["data"]["fact_snapshot"].is_null());
    assert!(reinspected["data"]["update"]["current"].is_boolean());
}

#[tokio::test]
async fn record_versions_replay_and_read_verbatim_pages() {
    let (_guard, admin, _db, store) = setup().await;
    supervisor(&admin, &_db).await;
    event(
        &admin,
        TENANT,
        "sample-verified-work",
        100,
        "work.completed",
        "a",
        "2026-11-18T00:00:00Z",
        json!({"team_independent_acceptance":true}),
    )
    .await;
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
    assert!(first["receipt"]["fact_snapshot_ref"].is_string());
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
        second["receipt"]["fact_snapshot_ref"]
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
            ] {
                assert_eq!(
                    sample_record[field], live_record[field],
                    "stale sample: {field}"
                );
            }
            assert!(sample_record["fact_snapshot_ref"].is_string());
            assert!(live_record["fact_snapshot_ref"].is_string());
            assert_eq!(sample_record["author"]["display_name"], "Jordan Lee");
            assert!(sample_record["update_id"].is_string());
            assert!(sample_record["recorded_at_unix_ms"].is_i64());
            if path.contains("update-inspect") {
                assert_eq!(
                    sample["data"]["fact_snapshot"],
                    value["data"]["fact_snapshot"]
                );
                assert_eq!(
                    sample["data"]["fact_snapshot_coverage"],
                    value["data"]["fact_snapshot_coverage"]
                );
            }
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
    for (index, (id, work)) in [("visible-work", "a"), ("hidden-work", "b-private")]
        .into_iter()
        .enumerate()
    {
        event(
            &admin,
            TENANT,
            id,
            100 + index as i64,
            "work.completed",
            work,
            "2026-11-18T00:00:00Z",
            json!({"team_independent_acceptance":true}),
        )
        .await;
    }
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
    let mut readonly_exact = query("update.inspect");
    readonly_exact.update_id = Some(update_id.into());
    let readonly_facts = read(&store, B, readonly_exact).await.unwrap();
    assert_eq!(
        readonly_facts["data"]["fact_snapshot"]["verified"],
        json!({"count":1,"work_ids":["b-private"]})
    );
    assert_eq!(
        readonly_facts["data"]["fact_snapshot_coverage"],
        json!({"complete":false,"hidden_work_count":1})
    );
    assert!(
        !readonly_facts["data"]["fact_snapshot"]
            .to_string()
            .contains("\"a\"")
    );
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
    let mut foreign = query("update.inspect");
    foreign.update_id = Some(update_id.into());
    assert!(matches!(
        store.query("other-tenant", PROJECT, A, foreign).await,
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
    let mut scoped_exact = query("update.inspect");
    scoped_exact.update_id = Some(update_id.into());
    let scoped_facts = read(&store, A, scoped_exact).await.unwrap();
    assert_eq!(
        scoped_facts["data"]["fact_snapshot"]["verified"],
        json!({"count":1,"work_ids":["a"]})
    );
    assert_eq!(
        scoped_facts["data"]["fact_snapshot_coverage"],
        json!({"complete":false,"hidden_work_count":1})
    );
    assert!(
        !scoped_facts["data"]["fact_snapshot"]
            .to_string()
            .contains("b-private")
    );
    let counts = admin
        .query_one(
            "SELECT
                (SELECT count(*) FROM awr_team.operations WHERE op='update.record'),
                (SELECT count(*) FROM awr_team.events WHERE event_type='update.recorded')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(counts.get::<_, i64>(0), 1);
    assert_eq!(counts.get::<_, i64>(1), 1);
}
