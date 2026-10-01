#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team_pg::WorkstreamReadStore;
use fixture::*;
use serde_json::{Value, json};

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start(store: WorkstreamReadStore) -> Server {
    store.check_schema().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec![],
        allowed_web_origins: vec![],
        oauth: None,
        projects: vec![
            ProjectBinding {
                key: "one".into(),
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
            },
            ProjectBinding {
                key: "other".into(),
                tenant_id: "other-tenant".into(),
                project_id: PROJECT.into(),
            },
        ],
    };
    let router = awr_server::service::router(config, address, store).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        url: format!("http://{address}/v1/projects"),
        task,
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
async fn post(server: &Server, key: &str, token: &str, body: Value) -> reqwest::Response {
    http()
        .post(format!("{}/{key}/query", server.url))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn one_http_service_isolates_parallel_clients_and_projects_using_real_pg() {
    let (_guard, _, _, store) = setup().await;
    let server = start(store).await;
    let body = json!({"protocol_version":1,"op":"work.list"});
    let (a, b) = tokio::join!(
        post(&server, "one", A, body.clone()),
        post(&server, "one", B, body.clone())
    );
    assert_eq!(a.status(), 200);
    assert_eq!(b.status(), 200);
    let a: Value = a.json().await.unwrap();
    let b: Value = b.json().await.unwrap();
    assert_eq!(a["data"]["total"], 2);
    assert!(!a.to_string().contains("b-private"));
    assert_eq!(b["data"]["total"], 1);
    assert_eq!(b["data"]["items"][0]["work_id"], "b-private");
    assert_eq!(post(&server, "other", A, body.clone()).await.status(), 403);
    assert_eq!(post(&server, "not-configured", A, body).await.status(), 403);
    for work in ["b-private", "absent"] {
        let response = post(
            &server,
            "one",
            A,
            json!({"protocol_version":1,"op":"work.prepare","work_id":work}),
        )
        .await;
        assert_eq!(response.status(), 403);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"code":"Forbidden","message":"access denied"})
        );
    }
    let result = post(
        &server,
        "one",
        A,
        json!({"protocol_version":1,"op":"work.prepare","work_id":"c"}),
    )
    .await;
    let result: Value = result.json().await.unwrap();
    assert_eq!(result["data"]["dependency_export_unavailable"], true);
    assert!(!result.to_string().contains("b-private"));
}

#[tokio::test]
async fn http_requires_live_auth_and_never_accepts_grants_or_identity_from_a_body() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(store).await;
    let body = json!({"protocol_version":1,"op":"capabilities"});
    assert_eq!(
        http()
            .post(format!("{}/one/query", server.url))
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(post(&server, "one", NONE, body.clone()).await.status(), 403);
    for token in ["garbage", NONE] {
        assert_eq!(
            post(
                &server,
                "one",
                token,
                json!({"protocol_version":1,"op":"claim.acquire"}),
            )
            .await
            .status(),
            403
        );
    }
    let cap: Value = post(&server, "one", A, body.clone())
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        cap["commands"],
        json!(awr_team_pg::WorkstreamCommand::OPERATIONS)
    );
    assert_eq!(cap["execution_admission"], true);
    assert_eq!(
        cap["execution_modes"],
        json!(["caller_managed", "reference_write_v1"])
    );
    assert_eq!(cap["trusted_execution_results"], true);
    assert_eq!(cap["execution_reconciliation"], true);
    for field in ["tenant_id", "project_id", "actor_id", "client_id", "grants"] {
        let mut request = body.clone();
        request[field] = json!("forged");
        let response = post(&server, "one", A, request).await;
        assert_eq!(response.status(), 403);
        let text = response.text().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"code":"Forbidden","message":"access denied"})
        );
        assert!(!text.contains(A));
        assert!(!text.contains("reader-tenant"));
    }
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert_eq!(post(&server, "one", A, body.clone()).await.status(), 403);
    assert_eq!(post(&server, "one", B, body).await.status(), 200);
}

async fn post_command(server: &Server, token: &str, body: Value) -> reqwest::Response {
    http()
        .post(format!("{}/one/command", server.url))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn http_prepare(server: &Server) -> Value {
    post(
        server,
        "one",
        A,
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
    )
    .await
    .json()
    .await
    .unwrap()
}

#[tokio::test]
async fn http_claims_require_write_authority_and_report_live_lease_conflicts() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(store).await;
    let acquire_args = json!({"session_id":"session-a","expected_session_version":"1",
        "expected_work_version":"0","ttl_seconds":60});
    let denied = serde_json::to_value(command(
        &http_prepare(&server).await,
        "denied",
        "claim.acquire",
        acquire_args.clone(),
    ))
    .unwrap();
    assert_eq!(post_command(&server, A, denied).await.status(), 403);
    enable_writes(&admin).await;
    let request = serde_json::to_value(command(
        &http_prepare(&server).await,
        "take",
        "claim.acquire",
        acquire_args.clone(),
    ))
    .unwrap();
    let taken = post_command(&server, A, request.clone()).await;
    assert_eq!(taken.status(), 200);
    let taken: Value = taken.json().await.unwrap();
    let claim = &taken["receipt"]["data"];
    assert_eq!(taken["receipt"]["execution_authorized"], false);
    let query = json!({"protocol_version":1,"op":"claim.inspect","work_id":"a","claim_id":claim["claim_id"]});
    assert_eq!(post(&server, "one", B, query.clone()).await.status(), 403);
    let inspected: Value = post(&server, "one", A, query.clone())
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(inspected["data"]["lease_live"], true);
    let mut competing = acquire_args;
    competing["expected_work_version"] = json!("1");
    let conflict = post_command(
        &server,
        A,
        serde_json::to_value(command(
            &http_prepare(&server).await,
            "competing",
            "claim.acquire",
            competing,
        ))
        .unwrap(),
    )
    .await;
    assert_eq!(conflict.status(), 409);
    assert_eq!(conflict.json::<Value>().await.unwrap()["code"], "ClaimHeld");
    let mut args = json!({"session_id":"session-a","expected_session_version":"1",
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":"1","ttl_seconds":120});
    let renew = serde_json::to_value(command(
        &http_prepare(&server).await,
        "renew",
        "claim.renew",
        args.clone(),
    ))
    .unwrap();
    assert_eq!(post_command(&server, B, renew.clone()).await.status(), 403);
    let renewed = post_command(&server, A, renew).await;
    assert_eq!(renewed.status(), 200);
    assert_eq!(
        renewed.json::<Value>().await.unwrap()["receipt"]["data"]["lease_version"],
        "2"
    );
    args["expected_lease_version"] = json!("2");
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let expired = post_command(
        &server,
        A,
        serde_json::to_value(command(
            &http_prepare(&server).await,
            "expired",
            "claim.renew",
            args.clone(),
        ))
        .unwrap(),
    )
    .await;
    assert_eq!(expired.status(), 409);
    assert_eq!(
        expired.json::<Value>().await.unwrap()["code"],
        "LeaseExpired"
    );
    args.as_object_mut().unwrap().remove("ttl_seconds");
    let released = post_command(
        &server,
        A,
        serde_json::to_value(command(
            &http_prepare(&server).await,
            "release",
            "claim.release",
            args,
        ))
        .unwrap(),
    )
    .await;
    assert_eq!(released.status(), 200);
    assert_eq!(
        released.json::<Value>().await.unwrap()["receipt"]["data"]["state"],
        "released"
    );
    let replay: Value = post_command(&server, A, request)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], taken["receipt"]);
    let now: Value = post(&server, "one", A, query).await.json().await.unwrap();
    assert_eq!(now["data"]["lease_live"], false);
    assert_eq!(now["data"]["state"], "released");
}

#[tokio::test]
async fn http_session_journal_survives_an_unconsumed_reply_and_rejects_cross_client_writes() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let prepared = http_prepare(&server).await;
    let request = serde_json::to_value(command(
        &prepared,
        "http-start",
        "session.start",
        json!({"conversation_id":"http-conversation"}),
    ))
    .unwrap();
    let initial = post_command(&server, A, request.clone()).await;
    assert_eq!(initial.status(), 200);
    drop(initial); // The caller did not consume the committed result.
    let inspected:Value=post(&server,"one",A,json!({"protocol_version":1,"op":"command.inspect","work_id":"a","request_id":"http-start"})).await.json().await.unwrap();
    assert_eq!(inspected["data"]["state"], "committed");
    let replay: Value = post_command(&server, A, request)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], inspected["data"]["receipt"]);
    let id = replay["receipt"]["data"]["session_id"].as_str().unwrap();
    let prepared = http_prepare(&server).await;
    let checkpoint=serde_json::to_value(command(&prepared,"http-checkpoint","session.checkpoint",json!({"session_id":id,
        "expected_session_version":"1","context_hash":prepared["data"]["context_hash"],"next_action":"Review HTTP behavior","open_loops":["independent acceptance pending"]}))).unwrap();
    assert_eq!(
        post_command(&server, B, checkpoint.clone()).await.status(),
        403
    );
    let saved = post_command(&server, A, checkpoint).await;
    assert_eq!(saved.status(), 200);
    assert_eq!(saved.headers()["cache-control"], "no-store");
    let recovered: Value = post(
        &server,
        "one",
        A,
        json!({"protocol_version":1,"op":"session.inspect","session_id":id}),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(
        recovered["data"]["items"][0]["next_action"],
        "Review HTTP behavior"
    );
    let end = serde_json::to_value(command(
        &http_prepare(&server).await,
        "http-end",
        "session.end",
        json!({"session_id":id,"expected_session_version":"2"}),
    ))
    .unwrap();
    assert_eq!(post_command(&server, A, end).await.status(), 200);
    let rows: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.operations WHERE client_id='cli-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 3);
}

#[tokio::test]
async fn http_command_conflicts_and_forged_authority_return_explicit_errors_without_writes() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let request = serde_json::to_value(command(
        &http_prepare(&server).await,
        "http-invalid",
        "session.start",
        json!({"conversation_id":"test"}),
    ))
    .unwrap();
    let mut forged = request.clone();
    forged["actor_id"] = json!("operator");
    let denied = post_command(&server, A, forged).await;
    assert_eq!(denied.status(), 403);
    assert_eq!(
        denied.json::<Value>().await.unwrap(),
        json!({"code":"Forbidden","message":"access denied"})
    );
    let mut stale = request.clone();
    stale["expected_authority_version"] = json!("999");
    let result = post_command(&server, A, stale).await;
    assert_eq!(result.status(), 409);
    assert_eq!(
        result.json::<Value>().await.unwrap()["code"],
        "PreconditionsChanged"
    );
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    assert_eq!(post_command(&server, A, request).await.status(), 403);
    let rows: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.operations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn transport_bounds_hosts_origins_cursor_replay_and_unknown_operations() {
    let (_guard, _, _, store) = setup().await;
    let server = start(store).await;
    let body = json!({"protocol_version":1,"op":"work.list","limit":1});
    let response = post(&server, "one", A, body).await;
    assert_eq!(response.headers()["cache-control"], "no-store");
    let first: Value = response.json().await.unwrap();
    assert_eq!(post(&server,"one",B,json!({"protocol_version":1,"op":"work.list","limit":1,"cursor":first["data"]["next_cursor"]})).await.status(),409);
    let body = json!({"protocol_version":1,"op":"capabilities"});
    for (header, value) in [
        ("origin", "https://untrusted.invalid"),
        ("host", "untrusted.invalid"),
    ] {
        assert_eq!(
            http()
                .post(format!("{}/one/query", server.url))
                .bearer_auth(A)
                .header(header, value)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(
        post(
            &server,
            "one",
            A,
            json!({"protocol_version":2,"op":"capabilities"})
        )
        .await
        .status(),
        501
    );
    assert_eq!(
        post(
            &server,
            "one",
            A,
            json!({"protocol_version":1,"op":"claim.acquire"})
        )
        .await
        .status(),
        501
    );
    assert_eq!(
        http()
            .post(format!("{}/one/query", server.url))
            .bearer_auth(A)
            .body("x".repeat(65537))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
}

#[tokio::test]
async fn http_preserves_context_limits_and_labels_stale_recovery() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(store).await;
    let response = post(
        &server,
        "one",
        A,
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a","max_context_bytes":1}),
    )
    .await;
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "ContextIncomplete"
    );
    let recovery: Value = post(
        &server,
        "one",
        A,
        json!({"protocol_version":1,"op":"work.recovery","work_id":"a"}),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(
        recovery["data"]["items"][0]["contract_matches_current"],
        false
    );
    assert_eq!(recovery["data"]["automatic_resume"], false);
    admin.batch_execute("UPDATE awr_team.checkpoints SET next_action=repeat('x',1048576) WHERE session_id='session-a'").await.unwrap();
    let response = post(
        &server,
        "one",
        A,
        json!({"protocol_version":1,"op":"work.recovery","work_id":"a"}),
    )
    .await;
    assert_eq!(response.status(), 409);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "ResponseTooLarge");
    assert!(body.get("data").is_none());
}
