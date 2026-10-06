#![cfg(feature = "pg-tests")]
//! Synthetic SDK/HTTP transport checks; no native team business acceptance.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "../../awr-team-pg/tests/fixtures/delivery_integration.rs"]
mod integration_fixture;

use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team::delivery::*;
use awr_team_pg::*;
use fixture::*;
use integration_fixture::*;
use rmcp::{
    RoleClient, ServiceExt,
    model::CallToolRequestParams,
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use std::time::Duration;

type Client = RunningService<RoleClient, ()>;
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
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}
async fn connect(server: &Server, token: &str) -> Client {
    let transport = StreamableHttpClientTransport::with_client(
        http(),
        StreamableHttpClientTransportConfig::with_uri(format!("{}/one/mcp", server.url))
            .auth_header(token),
    );
    tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .unwrap()
        .unwrap()
}
async fn call(client: &Client, name: &str, args: Value, error: bool) -> Value {
    let result = client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error.unwrap_or(false), error, "{result:?}");
    result.structured_content.unwrap()
}
async fn request(
    server: &Server,
    alias: &str,
    token: &str,
    path: &str,
    args: &Value,
) -> (reqwest::StatusCode, Value) {
    let response = http()
        .post(format!("{}/{alias}/{path}", server.url))
        .bearer_auth(token)
        .json(args)
        .send()
        .await
        .unwrap();
    (response.status(), response.json().await.unwrap())
}
fn read(op: &str, id: Option<&str>) -> Value {
    let mut q = json!({"protocol_version":1,"op":op,"work_id":"a"});
    if let Some(id) = id {
        q["request_id"] = json!(id);
    }
    q
}
async fn public_prepare(client: &Client, key: &str, round_id: &str) -> Value {
    let p = call(client, "awr_team_query", read("work.prepare", None), false).await;
    let neutral = call(
        client,
        "awr_team_query",
        read("delivery.neutral.inspect", None),
        false,
    )
    .await;
    // The submitting member supplies the real review.open receipt's round ID.
    let mut review_query = read("review.inspect", None);
    review_query["review_round_id"] = json!(round_id);
    let review = call(client, "awr_team_query", review_query, false).await;
    let connector = neutral["data"]["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["enabled"] == true && c["current_epoch"] == true)
        .unwrap();
    let candidate: DeliveryCandidate =
        serde_json::from_value(neutral["data"]["candidate"].clone()).unwrap();
    serde_json::to_value(command(&p, key, "delivery.integration.prepare", json!({
        "source_snapshot_id":p["source_snapshot_id"],"connector_id":connector["connector_id"],
        "connector_version":connector["connector_version"],"candidate_digest":candidate.binding.digest().unwrap(),
        "selection_version":neutral["data"]["selection_version"],"evidence_id":review["data"]["review"]["evidence_id"],
        "review_round_id":review["data"]["review"]["round_id"],
        "review_decision_id":review["data"]["review"]["decisions"][0]["decision_id"],"operation":"fast_forward",
    }))).unwrap()
}
async fn public_reject(client: &Client, id: &str) -> Value {
    let p = call(client, "awr_team_query", read("work.prepare", None), false).await;
    serde_json::to_value(command(&p,"sdk-withdraw","delivery.integration.reject_prepared",json!({
        "source_snapshot_id":p["source_snapshot_id"],"integration_id":id,"reason":"Withdraw before dispatch."}))).unwrap()
}

#[tokio::test]
async fn supervisor_sdk_and_http_share_prepare_recovery_rejection_and_discovery() {
    let f = setup_integration().await;
    let server = start(WorkstreamReadStore::from_config(f.config.clone())).await;
    let client = connect(&server, SUPERVISOR).await;
    let tools = client.list_all_tools().await.unwrap();
    let command_tool = tools.iter().find(|t| t.name == "awr_team_command").unwrap();
    let query_tool = tools.iter().find(|t| t.name == "awr_team_query").unwrap();
    for op in [
        "delivery.integration.prepare",
        "delivery.integration.reject_prepared",
    ] {
        assert!(
            command_tool.input_schema["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(op))
        );
        let rule = command_tool.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["if"]["properties"]["op"]["const"] == op)
            .unwrap();
        assert!(
            rule["then"]["properties"]["args"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("source_snapshot_id"))
        );
    }
    for op in [
        "delivery.integration.lease",
        "delivery.integration.dispatch",
        "delivery.integration.confirm",
    ] {
        assert!(
            !command_tool.input_schema["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(op))
        );
    }
    assert!(
        query_tool.input_schema["properties"]["op"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("delivery.integration.inspect"))
    );
    assert!(
        query_tool.input_schema["properties"]["request_id"]["description"]
            .as_str()
            .unwrap()
            .contains("IntegrationRequest.request_id")
    );
    let capabilities = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
        false,
    )
    .await;
    assert_eq!(
        capabilities["neutral_delivery"]["integration"]["worker_dispatch_exposed"],
        false
    );
    let prepared_command =
        public_prepare(&client, "sdk-integration", &f.request.review_round_id).await;
    let prepared = call(&client, "awr_team_command", prepared_command.clone(), false).await;
    assert_eq!(prepared["receipt"]["protocol"], "awr-delivery-sync-v1");
    assert_eq!(prepared["execution_authorized"], false);
    let (status, replay) = request(&server, "one", SUPERVISOR, "command", &prepared_command).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"]["data"], prepared["receipt"]["data"]);
    let id = prepared["receipt"]["data"]["integration_id"]
        .as_str()
        .unwrap();
    let inspection = read("delivery.integration.inspect", Some(id));
    let via_sdk = call(&client, "awr_team_query", inspection.clone(), false).await;
    let (status, via_http) = request(&server, "one", SUPERVISOR, "query", &inspection).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(via_sdk, via_http);
    assert_eq!(via_sdk["data"]["integration_request"]["request_id"], id);
    client.cancel().await.unwrap();
    drop(server);
    let restarted = start(WorkstreamReadStore::from_config(f.config.clone())).await;
    let resumed = connect(&restarted, SUPERVISOR).await;
    let outcome = call(
        &resumed,
        "awr_team_query",
        read("delivery.neutral.outcome", Some("sdk-integration")),
        false,
    )
    .await;
    assert_eq!(outcome["data"]["receipt"], prepared["receipt"]);
    assert_eq!(
        call(&resumed, "awr_team_query", inspection, false).await["data"],
        via_sdk["data"]
    );
    let withdrawn_command = public_reject(&resumed, id).await;
    let withdrawn = call(
        &resumed,
        "awr_team_command",
        withdrawn_command.clone(),
        false,
    )
    .await;
    let (status, replay) =
        request(&restarted, "one", SUPERVISOR, "command", &withdrawn_command).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(replay["receipt"]["data"], withdrawn["receipt"]["data"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(withdrawn["receipt"]["data"]["before_dispatch"], true);
    assert_eq!(f.guards().await, 0);
    let row = f
        .admin
        .query_one(
            "SELECT (SELECT count(*) FROM awr_team.delivery_integration_intents),
        (SELECT count(*) FROM awr_team.completion_receipts)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, i64>(0), 1);
    assert_eq!(row.get::<_, i64>(1), 0);
    resumed.cancel().await.unwrap();
}

#[tokio::test]
async fn transport_refuses_forged_fields_cross_scope_and_revoked_replay() {
    let f = setup_integration().await;
    let server = start(WorkstreamReadStore::from_config(f.config.clone())).await;
    let client = connect(&server, SUPERVISOR).await;
    let valid = public_prepare(&client, "auth-integration", &f.request.review_round_id).await;
    for field in ["approved", "read_set", "actor_id", "repository_path"] {
        let mut forged = valid.clone();
        forged["args"][field] = json!("injected");
        let via_sdk = call(&client, "awr_team_command", forged.clone(), true).await;
        let (status, via_http) = request(&server, "one", SUPERVISOR, "command", &forged).await;
        assert!(!status.is_success());
        assert_eq!(via_sdk, via_http);
    }
    let developer = connect(&server, A).await;
    assert_eq!(
        call(&developer, "awr_team_command", valid.clone(), true).await["code"],
        "Forbidden"
    );
    developer.cancel().await.unwrap();
    let prepared = call(&client, "awr_team_command", valid.clone(), false).await;
    let id = prepared["receipt"]["data"]["integration_id"]
        .as_str()
        .unwrap();
    for mut q in [
        read("delivery.integration.inspect", None),
        read("delivery.integration.inspect", Some(id)),
    ] {
        q["session_id"] = json!("session-a");
        let via_sdk = call(&client, "awr_team_query", q.clone(), true).await;
        let (status, via_http) = request(&server, "one", SUPERVISOR, "query", &q).await;
        assert!(!status.is_success());
        assert_eq!(via_sdk, via_http);
    }
    let (status, _) = request(
        &server,
        "other",
        SUPERVISOR,
        "query",
        &read("delivery.integration.inspect", Some(id)),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'").await.unwrap();
    for (path, args) in [
        ("command", valid),
        ("query", read("delivery.integration.inspect", Some(id))),
    ] {
        assert_eq!(
            request(&server, "one", SUPERVISOR, path, &args).await.0,
            reqwest::StatusCode::FORBIDDEN
        );
    }
    assert_eq!(f.guards().await, 1);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn reconnected_client_can_inspect_unknown_original_attempt_but_cannot_cancel_it() {
    let f = setup_integration().await;
    let server = start(WorkstreamReadStore::from_config(f.config.clone())).await;
    let client = connect(&server, SUPERVISOR).await;
    let prepared = call(
        &client,
        "awr_team_command",
        public_prepare(&client, "unknown-prepare", &f.request.review_round_id).await,
        false,
    )
    .await;
    let id = prepared["receipt"]["data"]["integration_id"]
        .as_str()
        .unwrap();
    let lease = f.leased(id).await;
    drop(
        f.dispatched(id, lease["lease_id"].as_str().unwrap())
            .await
            .permit,
    );
    let fact = f
        .ingest_record(
            "unknown-routes",
            f.observation(id, IntegrationOutcome::Unknown),
        )
        .await;
    f.store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("unknown-confirm", id, &fact),
        )
        .await
        .unwrap();
    client.cancel().await.unwrap();
    let resumed = connect(&server, SUPERVISOR).await;
    let description = call(
        &resumed,
        "awr_team_query",
        read("delivery.integration.inspect", Some(id)),
        false,
    )
    .await;
    assert_eq!(description["data"]["state"], "unknown");
    assert_eq!(description["data"]["execution_authorized"], false);
    assert_eq!(
        description["data"]["candidate"],
        json!(f.selection.candidate)
    );
    let rejected = call(
        &resumed,
        "awr_team_command",
        public_reject(&resumed, id).await,
        true,
    )
    .await;
    assert_eq!(rejected["code"], "RecoveryBlocked");
    assert_eq!(f.guards().await, 1);
    resumed.cancel().await.unwrap();
}
