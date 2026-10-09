//! Authenticated MCP handoff regressions; native business acceptance is separate.
#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team_pg::WorkstreamReadStore;
use fixture::*;
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
async fn connect(server: &Server, alias: &str, token: &str) -> Result<Client, String> {
    let transport = StreamableHttpClientTransport::with_client(
        http(),
        StreamableHttpClientTransportConfig::with_uri(format!("{}/{alias}/mcp", server.url))
            .auth_header(token),
    );
    tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .map_err(|_| "timeout".to_owned())?
        .map_err(|e| e.to_string())
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

async fn members(admin: &tokio_postgres::Client) {
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

async fn prepared(client: &Client, session: Option<&str>) -> Value {
    let mut q =
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a","max_context_bytes":262144});
    if let Some(session) = session {
        q["session_id"] = json!(session);
    }
    call(client, "awr_team_query", q, false).await
}

async fn execute(
    client: &Client,
    session: Option<&str>,
    key: &str,
    op: &str,
    args: Value,
) -> Value {
    let p = prepared(client, session).await;
    call(
        client,
        "awr_team_command",
        json!(command(&p, key, op, args)),
        false,
    )
    .await["receipt"]["data"]
        .clone()
}

struct Trial {
    sender: String,
    receiver: String,
    claim: Value,
    handoff: Value,
}

async fn trial(sender: &Client, receiver: &Client, running: bool) -> Trial {
    let sender_session = execute(
        sender,
        None,
        "start-sender",
        "session.start",
        json!({"conversation_id":"sender"}),
    )
    .await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let receiver_session = execute(
        receiver,
        None,
        "start-receiver",
        "session.start",
        json!({"conversation_id":"receiver"}),
    )
    .await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let p = prepared(sender, Some(&sender_session)).await;
    let claim = execute(sender, Some(&sender_session), "claim-sender", "task.claim_available", json!({
        "session_id":sender_session,"expected_session_version":"1","ttl_seconds":600,
        "expected_responsibility_version":p["data"]["responsibility"]["version"],"expected_work_version":"0"})).await;
    if running {
        let p = prepared(sender, Some(&sender_session)).await;
        let intent = execute(sender, Some(&sender_session), "prepare-run", "execution.prepare", json!({
            "session_id":sender_session,"expected_session_version":"1","claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":"a".repeat(64),"declared_scope":["src/api"]})).await;
        let p = prepared(sender, Some(&sender_session)).await;
        execute(sender, Some(&sender_session), "start-run", "execution.start", json!({
            "session_id":sender_session,"expected_session_version":"1","execution_id":intent["execution_id"],"expected_execution_version":"1",
            "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"})).await;
    }
    let p = prepared(sender, Some(&sender_session)).await;
    execute(sender, Some(&sender_session), "checkpoint-sender", "session.checkpoint", json!({
        "session_id":sender_session,"expected_session_version":"1","context_hash":p["data"]["context_hash"],
        "next_action":"Finish the remaining API checks","open_loops":["Independent validation pending"]})).await;
    let handoff = execute(sender, Some(&sender_session), "propose-handoff", "handoff.propose", json!({
        "session_id":sender_session,"expected_session_version":"2","handoff_id":"api-handoff","kind":"execution","to_person_id":"receiver"})).await;
    Trial {
        sender: sender_session,
        receiver: receiver_session,
        claim,
        handoff,
    }
}

async fn inspect(receiver: &Client, trial: &Trial) -> Value {
    execute(
        receiver,
        Some(&trial.receiver),
        "receive-package",
        "handoff.inspect",
        json!({
        "session_id":trial.receiver,"expected_session_version":"1","handoff_id":"api-handoff",
        "expected_handoff_version":trial.handoff["version"],"inspector_person_id":"receiver"}),
    )
    .await
}

async fn acceptance(receiver: &Client, trial: &Trial, inspection: &Value) -> Value {
    let p = prepared(receiver, Some(&trial.receiver)).await;
    json!(command(
        &p,
        "accept-handoff",
        "handoff.accept",
        json!({
        "session_id":trial.receiver,"expected_session_version":"1","handoff_id":"api-handoff",
        "expected_handoff_version":inspection["version"],"acceptor_person_id":"receiver",
        "inspection_request_id":inspection["inspection_request_id"],"expected_current_fence":trial.claim["fence"],
        "successor_execution":{"kind":"person","person_id":"receiver"}})
    ))
}

#[tokio::test]
async fn authenticated_handoff_survives_reconnect_and_requires_actual_package_delivery() {
    let (_guard, admin, _, store) = setup().await;
    members(&admin).await;
    let server = start(store).await;
    let sender = connect(&server, "one", A).await.unwrap();
    let receiver = connect(&server, "one", B).await.unwrap();
    let t = trial(&sender, &receiver, false).await;
    let query = json!({"protocol_version":1,"op":"handoff.inspect","work_id":"a","handoff_id":"api-handoff"});
    let looked_up = call(&receiver, "awr_team_query", query.clone(), false).await;
    assert_eq!(looked_up["data"]["handoff"]["status"], "proposed");
    let mut premature = acceptance(&receiver, &t, &t.handoff).await;
    premature["args"]["inspection_request_id"] = json!("read-only-lookup");
    assert_eq!(
        call(&receiver, "awr_team_command", premature, true).await["code"],
        "PreconditionsChanged"
    );
    let inspection = inspect(&receiver, &t).await;
    assert_eq!(
        inspection["prepared_context"]["data"]["context_complete"],
        true
    );
    assert_eq!(
        inspection["handoff"]["package"]["todos"][1],
        "Independent validation pending"
    );
    assert_eq!(inspection["consumption"]["client_id"], "cli-b");
    let request = acceptance(&receiver, &t, &inspection).await;
    receiver.cancel().await.unwrap();
    let reconnected = connect(&server, "one", B).await.unwrap();
    let accepted = call(&reconnected, "awr_team_command", request.clone(), false).await;
    assert_eq!(accepted["receipt"]["data"]["status"], "accepted");
    assert_eq!(accepted["current_responsibility"]["owner"], "agent");
    assert_eq!(
        accepted["current_responsibility"]["current_executor"]["person_id"],
        "receiver"
    );
    assert_eq!(accepted["execution_authorized"], false);
    let p = prepared(&reconnected, Some(&t.receiver)).await;
    execute(
        &reconnected,
        Some(&t.receiver),
        "claim-successor",
        "claim.acquire",
        json!({
        "session_id":t.receiver,"expected_session_version":"1","ttl_seconds":600,
        "expected_work_version":p["data"]["runtime"]["work_version"]}),
    )
    .await;
    let replay = call(&reconnected, "awr_team_command", request, false).await;
    assert_eq!(replay["receipt"], accepted["receipt"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["execution_authorized"], false);
    assert_eq!(
        call(&reconnected, "awr_team_query", query, false).await["data"]["handoff"]["status"],
        "accepted"
    );
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.executions", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    sender.cancel().await.unwrap();
    reconnected.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_caller_assurances_cannot_override_a_running_predecessor() {
    let (_guard, admin, _, store) = setup().await;
    members(&admin).await;
    let server = start(store).await;
    let sender = connect(&server, "one", A).await.unwrap();
    let receiver = connect(&server, "one", B).await.unwrap();
    let t = trial(&sender, &receiver, true).await;
    let inspection = inspect(&receiver, &t).await;
    let mut request = acceptance(&receiver, &t, &inspection).await;
    for key in [
        "prior_execution_stopped",
        "prior_reconciled",
        "context_reprepared",
    ] {
        request["args"][key] = json!(true);
    }
    request["args"]["now_ms"] = json!(0);
    assert_eq!(
        call(&receiver, "awr_team_command", request, true).await["code"],
        "RecoveryBlocked"
    );
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.executions", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "running"
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
    assert_eq!(
        inspection["consumption"]["predecessor"]["session_id"],
        t.sender
    );
    sender.cancel().await.unwrap();
    receiver.cancel().await.unwrap();
}
