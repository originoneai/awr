//! WS-014 acceptance: real HTTP/MCP clients with provisioned workstream
//! credentials cannot reach schema-owner operator surfaces. Those remain on
//! `awr-server access` only (recovery-inspect, history migration, claim/
//! execution quarantine, execution attribution, backup/fencing/rebuild).
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
use std::process::Command;
use std::time::Duration;

type McpClient = RunningService<RoleClient, ()>;

/// Operator-only surfaces (CLI names + plausible dotted aliases).
const OPERATOR_OPS: &[&str] = &[
    "recovery-inspect",
    "recovery.inspect",
    "history-preview",
    "history-apply",
    "history-outcome",
    "history.preview",
    "history.apply",
    "history.outcome",
    "quarantine-preview",
    "quarantine-apply",
    "quarantine-outcome",
    "quarantine.preview",
    "quarantine.apply",
    "quarantine.outcome",
    "execution-attribution-preview",
    "execution-attribution-apply",
    "execution-attribution-outcome",
    "execution.attribution",
    "backup-create",
    "backup-inspect",
    "backup-restore-preview",
    "backup-restore-apply",
    "backup-restore-outcome",
    "backup-rebuild-preview",
    "backup-rebuild-apply",
    "backup-rebuild-outcome",
    "backup.create",
    "backup.restore",
    "backup.rebuild",
];

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
        projects: vec![ProjectBinding {
            key: "one".into(),
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
        }],
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

async fn post_query(server: &Server, token: &str, body: Value) -> reqwest::Response {
    http()
        .post(format!("{}/one/query", server.url))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
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

async fn connect(server: &Server, token: &str) -> McpClient {
    let transport = StreamableHttpClientTransport::with_client(
        http(),
        StreamableHttpClientTransportConfig::with_uri(format!("{}/one/mcp", server.url))
            .auth_header(token),
    );
    tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .expect("mcp connect timeout")
        .expect("mcp connect failed")
}

async fn mcp_call(client: &McpClient, name: &str, args: Value) -> Value {
    let result = client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error.unwrap_or(false), true, "{result:?}");
    result.structured_content.unwrap()
}

fn owner_url(db: &str) -> String {
    let raw = common::test_database_url_raw();
    if raw.starts_with("postgres://") || raw.starts_with("postgresql://") {
        let mut u = reqwest::Url::parse(&raw).unwrap();
        u.set_path(&format!("/{db}"));
        let pairs = u
            .query_pairs()
            .filter(|(k, _)| k != "dbname")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        u.set_query(None);
        if !pairs.is_empty() {
            u.query_pairs_mut().extend_pairs(pairs);
        }
        u.to_string()
    } else {
        format!("{raw} dbname={db}")
    }
}

fn owner_cli(connection: &str, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_awr-server"))
        .env("AWR_TEAM_DATABASE_URL", connection)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owner CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fake_command(op: &str) -> Value {
    json!({
        "protocol_version": 1,
        "request_id": "operator-denied",
        "op": op,
        "workstream_id": awr_core::Id::from(1),
        "work_id": "a",
        "coordinator_epoch": "epoch-a",
        "expected_project_revision": "1",
        "expected_authority_version": "1",
        "expected_ownership_version": "1",
        "expected_contract_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "args": {}
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_and_mcp_clients_cannot_reach_operator_only_surfaces() {
    let (_guard, _admin, db, store) = setup().await;
    let server = start(store).await;

    // Capabilities must not advertise operator surfaces.
    let caps: Value = post_query(
        &server,
        A,
        json!({"protocol_version": 1, "op": "capabilities"}),
    )
    .await
    .json()
    .await
    .unwrap();
    let listed = format!("{}{}", caps["queries"], caps["commands"]);
    for op in OPERATOR_OPS {
        assert!(
            !listed.contains(op),
            "capabilities advertised operator op {op}: {listed}"
        );
    }

    // HTTP query path: authenticated client → Unsupported (501), never success.
    for op in OPERATOR_OPS {
        let response = post_query(&server, A, json!({"protocol_version": 1, "op": op})).await;
        assert_eq!(response.status().as_u16(), 501, "HTTP query op={op} status");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], "Unsupported", "HTTP query op={op}: {body}");
        assert!(body.get("data").is_none());
        assert!(!body.to_string().contains(A));
    }

    // HTTP command path: same denial for operator-shaped ops.
    for op in OPERATOR_OPS {
        let response = post_command(&server, A, fake_command(op)).await;
        assert_eq!(
            response.status().as_u16(),
            501,
            "HTTP command op={op} status"
        );
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], "Unsupported", "HTTP command op={op}: {body}");
    }

    // No dedicated HTTP operator routes under the project binding.
    for path in [
        "one/access/recovery-inspect",
        "one/access/history-preview",
        "one/access/quarantine-preview",
        "one/access/execution-attribution-preview",
        "one/access/backup-create",
        "one/operator/recovery-inspect",
    ] {
        let response = http()
            .post(format!("{}/{path}", server.url))
            .bearer_auth(A)
            .json(&json!({"protocol_version": 1}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            404,
            "unexpected operator HTTP route {path}"
        );
    }

    // MCP SDK client: tools enum omits owner-only operator ops; project-admin
    // access tools (TMCP-012) are separate and never expose recovery/SQL/owner CLI.
    let mcp = connect(&server, A).await;
    let tools = mcp.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
    assert!(names.contains(&"awr_team_query".into()));
    assert!(names.contains(&"awr_team_command".into()));
    assert!(names.contains(&"awr_team_access_preview".into()));
    assert_eq!(tools.len(), 6);
    for tool in &tools {
        let enum_ops = tool.input_schema["properties"]["op"]["enum"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let joined = serde_json::to_string(&enum_ops).unwrap();
        for op in OPERATOR_OPS {
            assert!(
                !joined.contains(op),
                "MCP tool {} enum contains operator op {op}",
                tool.name
            );
        }
        assert!(
            !tool.name.contains("recovery")
                && !tool.name.contains("quarantine")
                && !tool.name.contains("backup")
                && !tool.name.contains("history"),
            "owner operator tool exposed: {}",
            tool.name
        );
    }
    for op in [
        "recovery-inspect",
        "history-preview",
        "history-apply",
        "quarantine-preview",
        "quarantine-apply",
        "execution-attribution-preview",
        "execution-attribution-apply",
        "backup-create",
        "backup-restore-apply",
        "backup-rebuild-apply",
    ] {
        let denied = mcp_call(
            &mcp,
            "awr_team_query",
            json!({"protocol_version": 1, "op": op}),
        )
        .await;
        assert_eq!(denied["code"], "Unsupported", "MCP query op={op}: {denied}");
        let denied = mcp_call(&mcp, "awr_team_command", fake_command(op)).await;
        assert_eq!(
            denied["code"], "Unsupported",
            "MCP command op={op}: {denied}"
        );
    }
    // Invented operator tool names are also unavailable.
    let denied = mcp_call(
        &mcp,
        "awr_team_operator_recovery",
        json!({"protocol_version": 1, "op": "recovery-inspect"}),
    )
    .await;
    assert_eq!(denied["code"], "Unsupported");
    mcp.cancel().await.unwrap();

    // Cheap positive check: schema-owner CLI still reaches recovery-inspect /
    // history-preview on the same project the client was denied for.
    let owner = owner_url(&db);
    let report = owner_cli(
        &owner,
        &[
            "access",
            "recovery-inspect",
            "--tenant-id",
            TENANT,
            "--project-id",
            PROJECT,
        ],
    );
    assert_eq!(report["protocol"], "awr-operator-recovery-inspect-v1");
    assert_eq!(report["read_only"], true);
    let preview = owner_cli(
        &owner,
        &[
            "access",
            "history-preview",
            "--tenant-id",
            TENANT,
            "--project-id",
            PROJECT,
        ],
    );
    assert_eq!(preview["protocol"], "awr-operator-history-migration-v1");
    assert!(preview["state_digest"].as_str().is_some());
    assert!(preview["plan_digest"].as_str().is_some());
}
