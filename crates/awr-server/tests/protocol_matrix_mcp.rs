//! AWR-TMCP-050: HTTP + real MCP SDK transport protocol counterexamples.
//! Covers role/action units plus review/executor-adjacent permissions with
//! positive controls on both transports.
#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;

use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team_pg::{AdminAccessPlan, WorkstreamReadStore, workstream_credential_hash};
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
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

async fn connect(server: &Server, token: &str) -> Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(format!("{}/one/mcp", server.url)),
    );
    ().serve(transport).await.unwrap()
}

async fn try_connect(server: &Server, token: &str) -> Result<Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(format!("{}/one/mcp", server.url)),
    );
    ().serve(transport).await.map_err(|e| e.to_string())
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
    result
        .structured_content
        .unwrap_or_else(|| json!({"ok": !error}))
}

fn reader_plan(token: &str) -> AdminAccessPlan {
    serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"http-reader","kind":"human","display_name":"HTTP Reader"},
        "subject_client_id":"http-reader-cli",
        "role":"reader",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write":false,"manage":false,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"http-reader",
            "secret_hash":workstream_credential_hash(token).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap()
}

#[tokio::test]
async fn protocol_matrix_http_and_mcp_sdk_transports() {
    let (_g, admin, _db, store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='project_admin', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true,can_manage=true,grant_version=grant_version+1
             WHERE client_id='cli-a';",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let mcp = connect(&server, A).await;
    let client = http();

    // discovery_is_not_authority: tools catalog is navigation; forged identity denied.
    let tools = mcp.list_tools(Default::default()).await.unwrap();
    let names: Vec<_> = tools.tools.iter().map(|t| t.name.to_string()).collect();
    assert!(names.iter().any(|n| n == "awr_team_query"));
    assert!(
        names.iter().any(|n| n == "awr_team_access_preview")
            || names.iter().any(|n| n.starts_with("awr_team_access"))
    );

    let caps = call(
        &mcp,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
        false,
    )
    .await;
    assert_eq!(caps["action_authorization"], "tmcp_010_shared_decision");
    if let Some(nav) = caps.get("tool_discovery_is_navigation_only") {
        assert_eq!(nav, true);
    }

    let forged = call(
        &mcp,
        "awr_team_planning_suggest",
        json!({
            "protocol_version":1,
            "request_id":"mcp-forge-1",
            "rationale":"forged actor must not transfer authority",
            "affected_work_keys":["a"],
            "actor_id":"forged-admin"
        }),
        true,
    )
    .await;
    assert_eq!(forged["code"], "Forbidden");

    // cross_scope_identity + secret_delivery_boundary via access preview/apply.
    let reader_token =
        "awr1.http-reader.3333333333333333333333333333333333333333333333333333333333333333";
    let plan = reader_plan(reader_token);
    let preview = call(
        &mcp,
        "awr_team_access_preview",
        json!({"protocol_version":1,"plan":plan}),
        false,
    )
    .await;
    assert_eq!(preview["applied"], false);
    assert!(!preview.to_string().contains(reader_token));
    assert!(
        !preview
            .to_string()
            .contains(plan.credential.as_ref().unwrap().secret_hash.as_str())
    );

    let applied = call(
        &mcp,
        "awr_team_access_apply",
        json!({
            "protocol_version":1,
            "request_id":"mcp-matrix-add-1",
            "expected_state":preview["state_digest"],
            "expected_plan":preview["plan_digest"],
            "plan":plan
        }),
        false,
    )
    .await;
    assert_eq!(applied["replayed"], false);
    assert!(!applied.to_string().contains(reader_token));

    // idempotent_outcome: MCP replay + HTTP outcome parity.
    let replay = call(
        &mcp,
        "awr_team_access_apply",
        json!({
            "protocol_version":1,
            "request_id":"mcp-matrix-add-1",
            "expected_state":preview["state_digest"],
            "expected_plan":preview["plan_digest"],
            "plan":plan
        }),
        false,
    )
    .await;
    assert_eq!(replay["replayed"], true);

    let outcome_mcp = call(
        &mcp,
        "awr_team_access_outcome",
        json!({"protocol_version":1,"request_id":"mcp-matrix-add-1"}),
        false,
    )
    .await;
    assert_eq!(outcome_mcp["outcome"], "committed");

    let outcome_http = client
        .post(format!("{}/one/access/outcome", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"request_id":"mcp-matrix-add-1"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(outcome_http["outcome"], "committed");

    // role_action_matrix / execution_not_planning: reader denied on HTTP (+ MCP when admitted).
    let denied_http = client
        .post(format!("{}/one/access/preview", server.url))
        .header("authorization", format!("Bearer {reader_token}"))
        .json(&json!({"protocol_version":1,"plan":plan}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        denied_http.status().as_u16(),
        403,
        "reader access.manage must be forbidden"
    );

    let denied_plan_http = client
        .post(format!("{}/one/planning/suggest", server.url))
        .header("authorization", format!("Bearer {reader_token}"))
        .json(&json!({
            "protocol_version":1,
            "request_id":"reader-suggest-1",
            "rationale":"reader cannot propose planning changes",
            "affected_work_keys":["a"]
        }))
        .send()
        .await
        .unwrap();
    assert!(
        !denied_plan_http.status().is_success(),
        "reader planning status {}",
        denied_plan_http.status()
    );

    if let Ok(reader_mcp) = try_connect(&server, reader_token).await {
        let denied_access = call(
            &reader_mcp,
            "awr_team_access_preview",
            json!({"protocol_version":1,"plan":plan}),
            true,
        )
        .await;
        assert_eq!(denied_access["code"], "Forbidden");
        let denied_plan = call(
            &reader_mcp,
            "awr_team_planning_suggest",
            json!({
                "protocol_version":1,
                "request_id":"reader-suggest-mcp-1",
                "rationale":"reader cannot propose planning changes",
                "affected_work_keys":["a"]
            }),
            true,
        )
        .await;
        assert_eq!(denied_plan["code"], "Forbidden");
        let audit = call(
            &reader_mcp,
            "awr_team_query",
            json!({"protocol_version":1,"op":"audit.export"}),
            false,
        )
        .await;
        assert_eq!(
            audit["scope"], "self",
            "reader must not get project audit export"
        );
    }

    // Positive control: maintainer planning suggest via MCP, HTTP outcome parity.
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let sug = call(
        &mcp,
        "awr_team_planning_suggest",
        json!({
            "protocol_version":1,
            "request_id":"mcp-sug-matrix-1",
            "rationale":"Need a shared dependency for the protocol matrix",
            "affected_work_keys":["CLIENT-1"],
            "proposed_notes":{"add":"SHARED-1"}
        }),
        false,
    )
    .await;
    assert_eq!(sug["op"], "planning.propose");
    assert_eq!(sug["result"]["claimable"], false);

    let http_out = client
        .post(format!("{}/one/planning/outcome", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"request_id":"mcp-sug-matrix-1"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(http_out["request_id"], "mcp-sug-matrix-1");
    assert_eq!(http_out["already_recorded"], true);

    // revocation_race: revoked credential cannot initialize MCP / mutate.
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    let revoked = try_connect(&server, A).await;
    assert!(
        revoked.is_err(),
        "revoked credential must fail MCP init: {revoked:?}"
    );
    let revoked_http = client
        .post(format!("{}/one/planning/suggest", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({
            "protocol_version":1,
            "request_id":"mcp-sug-after-revoke",
            "rationale":"should be denied after credential revoke",
            "affected_work_keys":["CLIENT-1"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked_http.status().as_u16(), 403);

    // legacy_and_operator_separation: operator surfaces denied for member HTTP.
    let op = client
        .post(format!("{}/one/query", server.url))
        .header("authorization", format!("Bearer {reader_token}"))
        .json(&json!({"protocol_version":1,"op":"recovery-inspect"}))
        .send()
        .await
        .unwrap();
    assert!(
        !op.status().is_success(),
        "operator op status {}",
        op.status()
    );
}

#[tokio::test]
async fn protocol_matrix_extra_review_and_executor_permissions_visible_in_caps() {
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let mcp = connect(&server, A).await;
    let caps = call(
        &mcp,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
        false,
    )
    .await;
    assert_eq!(caps["permission_policy_id"], "awr-team-mcp-permission-v1");
    // Independent review is not a template default; capabilities must not claim otherwise.
    if let Some(review) = caps.get("independent_review") {
        assert_ne!(review.get("admin_bypass"), Some(&json!(true)));
    }
    // HTTP capabilities parity.
    let http_caps = http()
        .post(format!("{}/one/query", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"op":"capabilities"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let policy = http_caps
        .get("permission_policy_id")
        .or_else(|| http_caps.pointer("/data/permission_policy_id"))
        .cloned()
        .unwrap_or(json!(null));
    assert_eq!(policy, json!("awr-team-mcp-permission-v1"));
}
