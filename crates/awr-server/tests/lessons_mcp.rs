//! Synthetic lesson records exercised through the real loopback HTTP/MCP service.
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

async fn author(admin: &tokio_postgres::Client, db: &str) {
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
        UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]' WHERE actor_id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
          VALUES('reader-tenant','reader-project','member-one','Member One','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
          VALUES('reader-tenant','reader-project','binding-one','member-one','agent','active');").await.unwrap();
    enable_writes(admin).await;
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "issue-lesson".into(),
                authorization: AgentAuthorization {
                    id: "lesson-author".into(),
                    authorizer_person_id: PersonId::new("member-one").unwrap(),
                    responsible_person_id: PersonId::new("member-one").unwrap(),
                    subject_kind: ExecutionSubjectKind::Agent,
                    subject_id: "agent".into(),
                    client_id: "cli-a".into(),
                    session_id: None,
                    model_id: None,
                    scope: AuthorizationScope::Workstream {
                        project_id: PROJECT.into(),
                        workstream_id: awr_core::Id::from(1).to_string(),
                    },
                    actions: [AuthorizedAction::Inspect, AuthorizedAction::StartWork]
                        .into_iter()
                        .collect(),
                    expires_at_ms: None,
                    status: AuthorizationStatus::Active,
                    revoked_at_ms: None,
                    revoked_by: None,
                    verifiable_capabilities: vec![],
                    self_reported_skill_hints: vec![],
                    parent_authorization_id: None,
                    maintainer_person_id: None,
                    created_at_ms: 1,
                    binding_id: Some("binding-one".into()),
                },
            },
        )
        .await
        .unwrap();
}

async fn submit(client: &Client, key: &str, op: &str, args: Value, error: bool) -> Value {
    let prepared = call(
        client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let cmd = command(&prepared, key, op, args);
    call(
        client,
        "awr_team_command",
        serde_json::to_value(cmd).unwrap(),
        error,
    )
    .await
}

#[tokio::test]
async fn lesson_commands_and_confirmation_inbox_cross_the_real_mcp_transport() {
    let (_guard, admin, db, store) = setup().await;
    author(&admin, &db).await;
    let server = start(store).await;
    let client = connect(&server, A).await;
    let started = submit(
        &client,
        "start-lesson",
        "session.start",
        json!({"conversation_id":"lesson-conversation"}),
        false,
    )
    .await;
    let session = started["receipt"]["data"]["session_id"].as_str().unwrap();
    let content = json!({"title":"Keep retries stable","original_words":"  Reuse the first request.\n第二行。",
        "when_to_use":"A response was lost","not_applicable":"A new business intent","observed_result":"One recorded effect",
        "scope":{"path_prefixes":["src/service"],"task_tags":["retry"]},
        "dependencies":{"documents":[],"decision_ids":[]}});
    let args = json!({"session_id":session,"expected_session_version":"1","content":content});
    let mut forged = args.clone();
    forged["author_person_id"] = json!("member-two");
    let refused = submit(&client, "forged-lesson", "lesson.propose", forged, true).await;
    assert_eq!(refused["code"], "InvalidInput");
    let proposed = submit(&client, "propose-lesson", "lesson.propose", args, false).await;
    let id = proposed["receipt"]["data"]["lesson_id"].as_str().unwrap();
    let inbox = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.inbox"}),
        false,
    )
    .await;
    let item = inbox["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["guidance"]["code"] == "confirm_lesson")
        .unwrap();
    assert_eq!(item["guidance"]["type"], "确认经验");
    assert_eq!(item["guidance"]["lesson_id"], id);
    assert_eq!(item["next_query"]["op"], "work.observe");
    assert_eq!(item["title"], content["title"]);
    eprintln!(
        "LESSON_MCP_INBOX_SAMPLE={}",
        serde_json::to_string(item).unwrap()
    );
    let read = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"lessons.list","work_id":"a"}),
        false,
    )
    .await;
    assert_eq!(read["data"]["items"][0]["content"], content);
    assert_eq!(read["data"]["items"][0]["author_person_id"], "member-one");
    let change = json!({"session_id":session,"expected_session_version":"1","lesson_id":id,"expected_lesson_version":"1"});
    submit(
        &client,
        "confirm-lesson",
        "lesson.confirm",
        change.clone(),
        false,
    )
    .await;
    let inbox = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.inbox"}),
        false,
    )
    .await;
    assert!(
        !inbox["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["guidance"]["code"] == "confirm_lesson")
    );
    let mut revoke = change;
    revoke["expected_lesson_version"] = json!("2");
    submit(&client, "revoke-lesson", "lesson.revoke", revoke, false).await;
    let read = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"lessons.list","work_id":"a"}),
        false,
    )
    .await;
    assert_eq!(read["data"]["items"][0]["status"], "revoked");
    assert!(read["data"]["items"][0]["content"].is_null());
    client.cancel().await.unwrap();
}
