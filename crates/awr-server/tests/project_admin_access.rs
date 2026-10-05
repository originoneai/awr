//! TMCP-012: project-admin MCP/HTTP access management; non-admins denied;
//! raw secrets never appear in tool responses.
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
        .timeout(Duration::from_secs(35))
        .build()
        .unwrap()
}

async fn mcp(base: &str, token: &str) -> McpClient {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(format!("{base}/one/mcp")),
    );
    ().serve(transport).await.unwrap()
}

fn member_plan() -> AdminAccessPlan {
    let token = "awr1.mcp-member.2222222222222222222222222222222222222222222222222222222222222222";
    serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"mcp-human","kind":"human","display_name":"MCP member"},
        "subject_client_id":"mcp-cli",
        "role":"developer",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write":true,"manage":false,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"mcp-member",
            "secret_hash":workstream_credential_hash(token).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap()
}

#[tokio::test]
async fn declared_duties_roundtrip_through_http_and_mcp_with_matching_action_denials() {
    use awr_team::BusinessRole;
    use std::collections::BTreeSet;
    let (_g, owner, _db, store) = setup().await;
    enable_writes(&owner).await;
    owner
        .batch_execute(
            "UPDATE awr_team.workstream_grants
             SET can_manage=true, grant_version=grant_version+1 WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let http = http();
    let admin = mcp(&server.url, A).await;
    let mut plan = member_plan();
    plan.business_roles = Some(BTreeSet::from([BusinessRole::Observer]));
    let preview = http
        .post(format!("{}/one/access/preview", server.url))
        .bearer_auth(A)
        .json(&json!({"protocol_version":1,"plan":plan}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview.status(), reqwest::StatusCode::OK);
    let preview: Value = preview.json().await.unwrap();
    assert_eq!(preview["desired"]["business_roles"], json!(["observer"]));
    let applied = admin
        .call_tool(
            CallToolRequestParams::new("awr_team_access_apply").with_arguments(
                json!({"protocol_version":1,"request_id":"declare-observer",
                    "expected_state":preview["state_digest"],
                    "expected_plan":preview["plan_digest"],"plan":plan})
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap();
    assert!(!applied.is_error.unwrap_or(false), "{applied:?}");
    assert_eq!(
        applied.structured_content.unwrap()["receipt"]["current_policy"]["membership"]["business_roles"],
        json!(["observer"])
    );
    let token = "awr1.mcp-member.2222222222222222222222222222222222222222222222222222222222222222";
    let member = mcp(&server.url, token).await;
    let caps = member
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"capabilities"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(caps["identity"]["business_roles"], json!(["observer"]));
    assert_eq!(
        caps["identity"]["membership_action_ceiling"],
        json!(["work.read"])
    );
    let prepared = member
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"work.prepare","work_id":"a"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let body = serde_json::to_value(command(
        &prepared,
        "observer-cannot-start",
        "session.start",
        json!({"conversation_id":"observer"}),
    ))
    .unwrap();
    let denied = http
        .post(format!("{}/one/command", server.url))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
    let http_error: Value = denied.json().await.unwrap();
    let denied = member
        .call_tool(
            CallToolRequestParams::new("awr_team_command")
                .with_arguments(body.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert!(denied.is_error.unwrap_or(false));
    assert_eq!(denied.structured_content.unwrap(), http_error);
}

#[tokio::test]
async fn admin_can_preview_apply_via_mcp_and_http_non_admin_denied_no_raw_secrets() {
    let (_g, owner, _db, store) = setup().await;
    owner
        .batch_execute(
            "UPDATE awr_team.workstream_grants
             SET can_write=true, can_manage=true, grant_version=grant_version+1
             WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let client = http();

    // Non-admin: create a reader via admin HTTP first would require admin. Use NONE
    // which has membership but no grants — capabilities middleware will deny MCP
    // session entirely. Prefer HTTP denial with a reader token after admin adds them.
    let admin_mcp = mcp(&server.url, A).await;
    let tools = admin_mcp.list_tools(Default::default()).await.unwrap();
    let names: Vec<_> = tools.tools.iter().map(|t| t.name.to_string()).collect();
    for expected in [
        "awr_team_access_inspect",
        "awr_team_access_preview",
        "awr_team_access_apply",
        "awr_team_access_outcome",
    ] {
        assert!(names.iter().any(|n| n == expected), "missing {expected}");
    }

    let plan = member_plan();
    let preview_args = json!({"protocol_version":1,"plan":plan})
        .as_object()
        .unwrap()
        .clone();
    let preview = admin_mcp
        .call_tool(
            CallToolRequestParams::new("awr_team_access_preview".to_owned())
                .with_arguments(preview_args),
        )
        .await
        .unwrap();
    let preview_val: Value = preview.structured_content.clone().unwrap();
    assert_eq!(preview_val["applied"], false);
    assert!(!preview_val.to_string().contains("awr1.mcp-member."));
    assert!(
        !preview_val
            .to_string()
            .contains(plan.credential.as_ref().unwrap().secret_hash.as_str())
    );

    let apply_args = json!({
        "protocol_version":1,
        "request_id":"mcp-add-1",
        "expected_state":preview_val["state_digest"],
        "expected_plan":preview_val["plan_digest"],
        "plan":plan
    })
    .as_object()
    .unwrap()
    .clone();
    let applied = admin_mcp
        .call_tool(
            CallToolRequestParams::new("awr_team_access_apply".to_owned())
                .with_arguments(apply_args),
        )
        .await
        .unwrap();
    let applied_val: Value = applied.structured_content.clone().unwrap();
    assert_eq!(applied_val["replayed"], false);
    assert!(!applied_val.to_string().contains("awr1.mcp-member."));

    let directory = admin_mcp
        .call_tool(
            CallToolRequestParams::new("awr_team_access_inspect".to_owned()).with_arguments(
                json!({"protocol_version":1,"limit":50})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(
        directory["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["actor_id"] == "mcp-human")
    );
    assert!(!directory.to_string().contains("secret_hash"));

    let outcome = admin_mcp
        .call_tool(
            CallToolRequestParams::new("awr_team_access_outcome".to_owned()).with_arguments(
                json!({"protocol_version":1,"request_id":"mcp-add-1"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(outcome.structured_content.unwrap()["outcome"], "committed");

    // HTTP admin preview works.
    let http_preview = client
        .post(format!("{}/one/access/preview", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"plan":plan}))
        .send()
        .await
        .unwrap();
    // Stale because already applied — expect conflict or ok with changed state.
    assert!(
        http_preview.status().is_success() || http_preview.status().as_u16() == 409,
        "status {}",
        http_preview.status()
    );

    // Non-admin HTTP: add reader and deny.
    let reader_token =
        "awr1.http-reader.3333333333333333333333333333333333333333333333333333333333333333";
    let reader_plan: AdminAccessPlan = serde_json::from_value(json!({
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
            "secret_hash":workstream_credential_hash(reader_token).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap();
    let p = client
        .post(format!("{}/one/access/preview", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"plan":reader_plan}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let applied = client
        .post(format!("{}/one/access/apply", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({
            "protocol_version":1,
            "request_id":"add-http-reader",
            "expected_state":p["state_digest"],
            "expected_plan":p["plan_digest"],
            "plan":reader_plan
        }))
        .send()
        .await
        .unwrap();
    assert!(applied.status().is_success(), "{}", applied.status());

    let denied = client
        .post(format!("{}/one/access/preview", server.url))
        .header("authorization", format!("Bearer {reader_token}"))
        .json(&json!({"protocol_version":1,"plan":plan}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);

    // Same MCP entry exposes bounded project navigation and access history.
    let next = admin_mcp
        .call_tool(
            CallToolRequestParams::new("awr_team_query".to_owned()).with_arguments(
                json!({"protocol_version":1,"op":"work.next"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(next["data"]["identity"]["can_manage_members"], true);
    let audit = client
        .post(format!("{}/one/query", server.url))
        .bearer_auth(A)
        .json(&json!({"protocol_version":1,"op":"audit.requests"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(audit["data"]["scope"], "project");
    let records = audit["data"]["items"].as_array().unwrap();
    assert!(
        records
            .iter()
            .any(|r| r["action"] == "awr_team_access_apply" && r["result"] == "succeeded")
    );
    assert!(records.iter().any(|r| r["actor_id"] == "http-reader"
        && r["action"] == "access.preview"
        && r["result"] == "denied"));
    assert!(!audit.to_string().contains(reader_token));
    let denied_audit = client
        .post(format!("{}/one/query", server.url))
        .bearer_auth(reader_token)
        .json(&json!({"protocol_version":1,"op":"audit.requests","member_actor_id":"agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied_audit.status(), reqwest::StatusCode::FORBIDDEN);

    // Raw bearer in body is refused.
    let forged = client
        .post(format!("{}/one/access/preview", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"bearer":"awr1.leak","plan":plan}))
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), reqwest::StatusCode::FORBIDDEN);
}
