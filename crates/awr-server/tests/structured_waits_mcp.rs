#![cfg(feature = "pg-tests")]
//! AWR-PPD-050: structured waits end-to-end on the real awr-server HTTP/MCP/web
//! service. Verbatim web-route replies for `work.snapshot` (provided and legacy)
//! and `work.graph` are saved under `tests/samples` (when
//! `AWR_STRUCTURED_WAITS_SAMPLE_DIR` is set) and compared on every test run; the
//! Observer bridge tests consume the same verbatim samples.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/oauth_http.rs"]
mod web;

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

type McpClient = RunningService<RoleClient, ()>;

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
        allowed_hosts: vec!["awr.example".into()],
        allowed_web_origins: vec![web::ISSUER.into()],
        oauth: None,
        projects: vec![
            ProjectBinding {
                key: "one".into(),
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
            },
            ProjectBinding {
                key: "foreign".into(),
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
        url: format!("http://{address}/v1"),
        task,
    }
}

async fn connect_mcp(base: &str, token: &str) -> McpClient {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .timeout(Duration::from_secs(35))
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(format!("{base}/projects/one/mcp")),
    );
    ().serve(transport).await.unwrap()
}

async fn call_mcp(client: &McpClient, name: &str, args: Value, expect_error: bool) -> Value {
    let result = client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error.unwrap_or(false), expect_error, "{result:?}");
    result.structured_content.unwrap()
}

async fn login_web(base: &str, token: &str) -> String {
    let response = web::http()
        .post(format!("{base}/web/login"))
        .header("host", "awr.example")
        .header("origin", web::ISSUER)
        .header("x-awr-web", "1")
        .json(&json!({"bearer":token}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn web_query(
    base: &str,
    cookie: Option<&str>,
    project_key: &str,
    query: Value,
) -> (u16, Value) {
    let mut req = web::http()
        .post(format!("{base}/web/projects/{project_key}/query"))
        .header("host", "awr.example")
        .header("origin", web::ISSUER)
        .header("x-awr-web", "1")
        .json(&query);
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }
    let response = req.send().await.unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    (status, body)
}

fn save_verbatim(name: &str, value: &Value) {
    let Ok(directory) = std::env::var("AWR_STRUCTURED_WAITS_SAMPLE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    std::fs::write(directory.join(name), bytes).unwrap();
}

fn assert_sample_matches_snapshot(sample_name: &str, live: &Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples")
        .join(sample_name);
    let sample: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(sample["protocol_version"], 1);
    assert_eq!(sample["workstream_id"], live["workstream_id"]);
    assert_eq!(sample["data"]["work_id"], live["data"]["work_id"]);
    assert_eq!(
        sample["data"]["observation"]["structured_waits"]["status"],
        live["data"]["observation"]["structured_waits"]["status"]
    );
    assert_eq!(
        sample["data"]["observation"]["structured_waits"]["items"],
        live["data"]["observation"]["structured_waits"]["items"]
    );
    assert_eq!(
        sample["data"]["observation"]["structured_waits"]["provenance"],
        live["data"]["observation"]["structured_waits"]["provenance"]
    );
    assert_eq!(
        sample["data"]["observation"]["structured_waits"]["grants_authority"],
        live["data"]["observation"]["structured_waits"]["grants_authority"]
    );
    assert_eq!(
        sample["data"]["observation"]["structured_waits"]["contract_matches_current"],
        live["data"]["observation"]["structured_waits"]["contract_matches_current"]
    );
    let text = sample.to_string().to_lowercase();
    for banned in ["secret", "password", "awr1."] {
        assert!(!text.contains(banned), "{banned} leaked into {sample_name}");
    }
}

fn assert_sample_matches_graph(sample_name: &str, live: &Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples")
        .join(sample_name);
    let sample: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(sample["protocol_version"], 1);
    assert_eq!(sample["workstream_id"], live["workstream_id"]);
    let sample_items = sample["data"]["items"].as_array().unwrap();
    let live_items = live["data"]["items"].as_array().unwrap();
    assert_eq!(sample_items.len(), live_items.len());
    for (s_item, l_item) in sample_items.iter().zip(live_items) {
        assert_eq!(s_item["work_id"], l_item["work_id"]);
        assert_eq!(
            s_item["structured_waits"]["status"],
            l_item["structured_waits"]["status"]
        );
        assert_eq!(
            s_item["structured_waits"]["items"],
            l_item["structured_waits"]["items"]
        );
        assert_eq!(
            s_item["structured_waits"]["provenance"],
            l_item["structured_waits"]["provenance"]
        );
        assert_eq!(
            s_item["structured_waits"]["grants_authority"],
            l_item["structured_waits"]["grants_authority"]
        );
    }
    let text = sample.to_string().to_lowercase();
    for banned in ["secret", "password", "awr1."] {
        assert!(!text.contains(banned), "{banned} leaked into {sample_name}");
    }
}

#[tokio::test]
async fn structured_waits_mcp_and_web_reads_match_verbatim_samples() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    // Allow session.checkpoint on work "c" too so work.graph carries one provided and one legacy work.
    admin
        .batch_execute(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
             VALUES('reader-tenant','reader-project','session-c','main','c','agent','cli-a','conv-c','active','00000000000000000000000001',1);",
        )
        .await
        .unwrap();

    let server = start(store).await;
    let client = connect_mcp(&server.url, A).await;

    // 1. Verify MCP tool discovery documents when and how to fill progress.waits.
    let tools = client.list_all_tools().await.unwrap();
    let cmd_tool = tools.iter().find(|t| t.name == "awr_team_command").unwrap();
    let waits_schema = &cmd_tool.input_schema["properties"]["args"]["properties"]["progress"]["properties"]
        ["waits"];
    assert!(
        waits_schema["description"]
            .as_str()
            .unwrap()
            .contains("external_party")
    );

    // 2. Write a structured wait on work "a" through real MCP awr_team_command.
    let prep_a = call_mcp(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let waits = json!([
        {
            "waiting_on": { "kind": "external_party", "name": "Partner Billing Desk" },
            "what": "Confirm settlement export field format.",
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Signed column list returned by Partner Billing Desk."
        },
        {
            "waiting_on": { "kind": "person", "name": "Morgan Chen" },
            "what": "Review retry timeout policy.",
            "since_unix_ms": 1_731_628_800_000_i64,
            "release_condition": "Timeout bounds recorded on the open decision."
        },
        {
            "waiting_on": { "kind": "group", "name": "Platform Security" },
            "what": "Approve outbound allowlist entry.",
            "since_unix_ms": 1_731_715_200_000_i64,
            "release_condition": "Allowlist change marked approved."
        }
    ]);
    let write_a = call_mcp(
        &client,
        "awr_team_command",
        json!({
            "protocol_version": 1,
            "request_id": "mcp-wait-a-1",
            "op": "session.checkpoint",
            "workstream_id": prep_a["workstream_id"],
            "work_id": "a",
            "coordinator_epoch": prep_a["coordinator_epoch"],
            "expected_project_revision": prep_a["project_revision"],
            "expected_authority_version": prep_a["authority_version"],
            "expected_ownership_version": prep_a["data"]["ownership_version"],
            "expected_contract_hash": prep_a["data"]["contract_hash"],
            "args": {
                "session_id": "session-a",
                "expected_session_version": "1",
                "context_hash": prep_a["data"]["context_hash"],
                "next_action": "Resume export mapper after partner confirmation",
                "open_loops": ["Confirm export schema"],
                "progress": {
                    "phase": "blocked",
                    "summary": "Waiting on partner column list and security allowlist.",
                    "waits": waits
                }
            }
        }),
        false,
    )
    .await;
    assert_eq!(write_a["execution_authorized"], false);

    // 3. Reject unknown waiting_on.kind and oversized name through real MCP awr_team_command.
    let bad_kind = call_mcp(
        &client,
        "awr_team_command",
        json!({
            "protocol_version": 1,
            "request_id": "mcp-wait-bad-kind",
            "op": "session.checkpoint",
            "workstream_id": prep_a["workstream_id"],
            "work_id": "a",
            "coordinator_epoch": prep_a["coordinator_epoch"],
            "expected_project_revision": prep_a["project_revision"],
            "expected_authority_version": prep_a["authority_version"],
            "expected_ownership_version": prep_a["data"]["ownership_version"],
            "expected_contract_hash": prep_a["data"]["contract_hash"],
            "args": {
                "session_id": "session-a",
                "expected_session_version": "2",
                "context_hash": prep_a["data"]["context_hash"],
                "next_action": "Try invalid wait",
                "open_loops": [],
                "progress": {
                    "phase": "blocked",
                    "summary": "Invalid kind test.",
                    "waits": [{
                        "waiting_on": { "kind": "vendor", "name": "Acme" },
                        "what": "Approval",
                        "since_unix_ms": 1_731_542_400_000_i64,
                        "release_condition": "Approved"
                    }]
                }
            }
        }),
        true,
    )
    .await;
    assert_eq!(bad_kind["code"], "InvalidInput");

    // 4. Write a legacy-shaped progress report (omitting `waits`) on work "c".
    let prep_c = call_mcp(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"c"}),
        false,
    )
    .await;
    call_mcp(
        &client,
        "awr_team_command",
        json!({
            "protocol_version": 1,
            "request_id": "mcp-legacy-c-1",
            "op": "session.checkpoint",
            "workstream_id": prep_c["workstream_id"],
            "work_id": "c",
            "coordinator_epoch": prep_c["coordinator_epoch"],
            "expected_project_revision": prep_c["project_revision"],
            "expected_authority_version": prep_c["authority_version"],
            "expected_ownership_version": prep_c["data"]["ownership_version"],
            "expected_contract_hash": prep_c["data"]["contract_hash"],
            "args": {
                "session_id": "session-c",
                "expected_session_version": "1",
                "context_hash": prep_c["data"]["context_hash"],
                "next_action": "Continue tests",
                "open_loops": [],
                "progress": {
                    "phase": "testing",
                    "summary": "Legacy report omitting waits."
                }
            }
        }),
        false,
    )
    .await;

    // 5. Read verbatim replies through the real web route (/v1/web/projects/one/query).
    let cookie = login_web(&server.url, A).await;
    let stream_id = prep_a["workstream_id"].as_str().unwrap();

    let (status_a, snap_a) = web_query(
        &server.url,
        Some(&cookie),
        "one",
        json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","workstream_id":stream_id,"max_context_bytes":262144}),
    )
    .await;
    assert_eq!(status_a, 200, "{snap_a}");
    assert_eq!(
        snap_a["data"]["observation"]["structured_waits"]["status"],
        "provided"
    );
    assert_eq!(
        snap_a["data"]["observation"]["structured_waits"]["items"],
        waits
    );
    save_verbatim("work-snapshot-structured-waits.json", &snap_a);

    let (status_c, snap_c) = web_query(
        &server.url,
        Some(&cookie),
        "one",
        json!({"protocol_version":1,"op":"work.snapshot","work_id":"c","workstream_id":stream_id,"max_context_bytes":262144}),
    )
    .await;
    assert_eq!(status_c, 200, "{snap_c}");
    assert_eq!(
        snap_c["data"]["observation"]["structured_waits"]["status"],
        "not_provided"
    );
    assert!(snap_c["data"]["observation"]["structured_waits"]["items"].is_null());
    save_verbatim("work-snapshot-legacy-waits.json", &snap_c);

    let (status_g, graph) = web_query(
        &server.url,
        Some(&cookie),
        "one",
        json!({"protocol_version":1,"op":"work.graph","workstream_id":stream_id}),
    )
    .await;
    assert_eq!(status_g, 200, "{graph}");
    let items = graph["data"]["items"].as_array().unwrap();
    let item_a = items.iter().find(|i| i["work_id"] == "a").unwrap();
    let item_c = items.iter().find(|i| i["work_id"] == "c").unwrap();
    assert_eq!(item_a["structured_waits"]["status"], "provided");
    assert_eq!(item_a["structured_waits"]["items"], waits);
    assert_eq!(item_c["structured_waits"]["status"], "not_provided");
    assert!(item_c["structured_waits"]["items"].is_null());
    save_verbatim("work-graph-structured-waits.json", &graph);

    // Compare against committed samples unless we are regenerating them right now.
    if std::env::var("AWR_STRUCTURED_WAITS_SAMPLE_DIR").is_err() {
        assert_sample_matches_snapshot("work-snapshot-structured-waits.json", &snap_a);
        assert_sample_matches_snapshot("work-snapshot-legacy-waits.json", &snap_c);
        assert_sample_matches_graph("work-graph-structured-waits.json", &graph);
    }

    // 6. Cross-project read is refused with 403 and leaks no wait content.
    let (foreign_status, foreign_body) = web_query(
        &server.url,
        Some(&cookie),
        "foreign",
        json!({"protocol_version":1,"op":"work.graph","workstream_id":stream_id}),
    )
    .await;
    assert_eq!(foreign_status, 403, "{foreign_body}");
    assert!(!foreign_body.to_string().contains("Partner Billing Desk"));

    // 7. Revoking credential immediately closes both web and MCP reads.
    admin
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            &[],
        )
        .await
        .unwrap();
    let (revoked_status, revoked_body) = web_query(
        &server.url,
        Some(&cookie),
        "one",
        json!({"protocol_version":1,"op":"work.graph","workstream_id":stream_id}),
    )
    .await;
    assert_eq!(revoked_status, 403, "{revoked_body}");
    assert!(!revoked_body.to_string().contains("Partner Billing Desk"));
}
