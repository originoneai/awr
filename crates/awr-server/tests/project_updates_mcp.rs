//! Real MCP coverage for project period update recording and reads.
#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;

use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
};
use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team_pg::{AuthorizationStore, WorkstreamReadStore};
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
use std::{collections::BTreeSet, time::Duration};

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

async fn connect(server: &Server, token: &str) -> Client {
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        http,
        StreamableHttpClientTransportConfig::with_uri(format!("{}/one/mcp", server.url))
            .auth_header(token),
    );
    ().serve(transport).await.unwrap()
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

async fn supervisor(admin: &tokio_postgres::Client, db: &str) {
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
    let authorization = AgentAuthorization {
        id: "update-project".into(),
        authorizer_person_id: PersonId::new("lead").unwrap(),
        responsible_person_id: PersonId::new("lead").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        actions: BTreeSet::from([AuthorizedAction::Inspect, AuthorizedAction::EditPlanning]),
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
                request_key: "issue-update-project".into(),
                authorization,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn update_record_tool_and_query_operations_work_on_the_real_mcp_service() {
    let (_guard, admin, db, store) = setup().await;
    supervisor(&admin, &db).await;
    let server = start(store).await;
    let client = connect(&server, A).await;
    let tools = client.list_all_tools().await.unwrap();
    let tool = tools
        .iter()
        .find(|tool| tool.name == "awr_team_update_record")
        .unwrap();
    assert!(
        tool.description
            .as_deref()
            .unwrap()
            .starts_with("Record the responsible supervisor's own status")
    );
    assert_eq!(
        tool.annotations.as_ref().unwrap().read_only_hint,
        Some(false)
    );
    let query = tools
        .iter()
        .find(|tool| tool.name == "awr_team_query")
        .unwrap();
    for op in ["updates.list", "update.inspect"] {
        assert!(
            query.input_schema["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(op))
        );
    }

    let input = json!({"protocol_version":1,"request_id":"mcp-week-47",
        "period_kind":"week","period_key":"2026-W47","starts_on":"2026-11-16",
        "ends_on":"2026-11-22","status":"risk","summary":"A sentence written by Jordan."});
    let recorded = call(&client, "awr_team_update_record", input.clone(), false).await;
    assert_eq!(recorded["receipt"]["op"], "update.record");
    assert!(recorded["receipt"]["fact_snapshot_ref"].is_string());
    let replay = call(&client, "awr_team_update_record", input, false).await;
    assert_eq!(replay["receipt"], recorded["receipt"]);
    let id = recorded["receipt"]["update_id"].as_str().unwrap();

    let listed = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"updates.list","limit":1}),
        false,
    )
    .await;
    assert_eq!(listed["data"]["items"][0]["update_id"], id);
    assert_eq!(
        listed["data"]["items"][0]["author"]["display_name"],
        "Jordan Lee"
    );
    let inspected = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"update.inspect","update_id":id}),
        false,
    )
    .await;
    assert_eq!(
        inspected["data"]["update"]["summary"],
        "A sentence written by Jordan."
    );
    assert_eq!(inspected["data"]["fact_snapshot"]["verified"]["count"], 0);
    assert_eq!(
        inspected["data"]["fact_snapshot_coverage"]["complete"],
        true
    );

    let refused = call(
        &client,
        "awr_team_update_record",
        json!({"protocol_version":1,"request_id":"mcp-invalid",
            "period_kind":"week","period_key":"2026-W47","starts_on":"2026-11-16",
            "ends_on":"2026-11-22","status":"risk","summary":"bad\nsummary"}),
        true,
    )
    .await;
    assert_eq!(refused["code"], "InvalidInput");
    assert!(!refused.to_string().contains("bad\nsummary"));
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.project_period_updates", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    client.cancel().await.unwrap();
}
