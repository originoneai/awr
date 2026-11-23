#![cfg(feature = "pg-tests")]
//! AWR-PPD-054: real service MCP + Web tests for `usage.summary`.
//! Saves verbatim synthetic replies as committed samples under tests/samples so
//! Observer bridge and department-home tests consume genuine service replies.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/oauth_http.rs"]
mod web;

use awr_server::service::{OAuthConfig, ProjectBinding, ServiceConfig};
use awr_team_pg::{WorkstreamReadStore, workstream_credential_hash};
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

type Mcp = RunningService<RoleClient, ()>;

const AUDIT_TOKEN: &str =
    "awr1.usage-audit.1111111111111111111111111111111111111111111111111111111111111111";
const PEER_TOKEN: &str =
    "awr1.usage-peer.2222222222222222222222222222222222222222222222222222222222222222";

struct Server {
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn save_verbatim(name: &str, value: &Value) {
    let Ok(directory) = std::env::var("AWR_USAGE_SAMPLE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    std::fs::write(directory.join(name), bytes).unwrap();
}

async fn start_service(store: WorkstreamReadStore) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec!["awr.example".into()],
        allowed_web_origins: vec![web::ISSUER.into()],
        oauth: None::<OAuthConfig>,
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
        base: format!("http://{address}"),
        task,
    }
}

async fn connect_mcp(server: &str, bearer: &str) -> Mcp {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {bearer}").parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(format!("{server}/v1/projects/one/mcp")),
    );
    ().serve(transport).await.unwrap()
}

async fn call_query(client: &Mcp, args: Value, error: bool) -> Value {
    let result = client
        .call_tool(
            CallToolRequestParams::new("awr_team_query".to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error.unwrap_or(false), error, "{result:?}");
    result.structured_content.unwrap()
}

async fn login_web(server: &str, bearer: &str) -> String {
    let response = web::http()
        .post(format!("{server}/v1/web/login"))
        .header("host", "awr.example")
        .header("origin", web::ISSUER)
        .header("x-awr-web", "1")
        .json(&json!({"bearer":bearer}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn web_query(
    server: &str,
    project_key: &str,
    cookie: Option<&str>,
    origin: &str,
    args: Value,
) -> (u16, Value) {
    let mut req = web::http()
        .post(format!("{server}/v1/web/projects/{project_key}/query"))
        .header("host", "awr.example")
        .header("origin", origin)
        .header("x-awr-web", "1")
        .json(&args);
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }
    let response = req.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

fn assert_same_usage_reply(mcp: &Value, browser: &Value) {
    assert!(browser["data"]["as_of"]["at_unix_ms"].is_i64());
    let mut expected = mcp.clone();
    expected["data"]["as_of"]["at_unix_ms"] = browser["data"]["as_of"]["at_unix_ms"].clone();
    assert_eq!(browser, &expected);
}

fn assert_clean_sample(sample: &Value, live: &Value) {
    assert_eq!(sample["protocol_version"], 1);
    assert_eq!(sample["data"]["execution_authorized"], false);
    assert_eq!(sample["data"]["collection"], live["data"]["collection"]);
    assert_eq!(sample["data"]["project"], live["data"]["project"]);
    assert_eq!(sample["data"]["by_task"], live["data"]["by_task"]);
    assert_eq!(sample["data"]["by_member"], live["data"]["by_member"]);
    assert_eq!(sample["data"]["member_scope"], live["data"]["member_scope"]);
    assert_eq!(
        sample["data"]["member_ordering"],
        live["data"]["member_ordering"]
    );
    assert_eq!(
        sample["data"]["personal_comparison_ranking"],
        live["data"]["personal_comparison_ranking"]
    );
    assert_eq!(sample["data"]["coverage"], live["data"]["coverage"]);
    let text = sample.to_string().to_lowercase();
    for banned in ["secret", "password", "bearer", "awr1."] {
        assert!(!text.contains(banned), "{banned} leaked in sample");
    }
}

async fn seed_service_fixture(admin: &tokio_postgres::Client) {
    let audit_hash = workstream_credential_hash(AUDIT_TOKEN).unwrap();
    let peer_hash = workstream_credential_hash(PEER_TOKEN).unwrap();
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
             VALUES('{TENANT}','audit-lead','human','Audit Lead','active'),
                   ('{TENANT}','peer-dev','human','Peer Builder','active');
             INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
             VALUES('{TENANT}','usage-audit','audit-lead','cli-audit','{audit_hash}'),
                   ('{TENANT}','usage-peer','peer-dev','cli-peer','{peer_hash}');
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role,business_roles)
             VALUES('{TENANT}','{PROJECT}','audit-lead','project_admin','[\"administrator\"]'::jsonb),
                   ('{TENANT}','{PROJECT}','peer-dev','developer','[\"developer\"]'::jsonb);
             UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]'::jsonb
             WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}' AND actor_id='agent';
             INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,active)
             VALUES('{TENANT}','{PROJECT}','audit-lead','cli-audit','00000000000000000000000001',1,true,false,true),
                   ('{TENANT}','{PROJECT}','audit-lead','cli-audit','00000000000000000000000002',1,true,false,true),
                   ('{TENANT}','{PROJECT}','peer-dev','cli-peer','00000000000000000000000001',1,true,true,true),
                   ('{TENANT}','{PROJECT}','peer-dev','cli-peer','00000000000000000000000002',1,true,true,true);
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('{TENANT}','{PROJECT}','audit-lead','Audit Lead','active'),
                   ('{TENANT}','{PROJECT}','member-app','App Builder','active'),
                   ('{TENANT}','{PROJECT}','peer-dev','Peer Builder','active'),
                   ('{TENANT}','{PROJECT}','member-delivery','Delivery Builder','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('{TENANT}','{PROJECT}','bind-app','member-app','agent','active');
             INSERT INTO awr_team.project_directories(tenant_id,project_id,version,plan_json,provenance)
             VALUES('{TENANT}','{PROJECT}',1,
                 '{{\"organization\":{{\"id\":\"org-1\",\"name\":\"Example Org\"}},
                    \"departments\":[
                      {{\"id\":\"dept-eng\",\"name\":\"Engineering\"}},
                      {{\"id\":\"grp-app\",\"name\":\"App Group\",\"parent_id\":\"dept-eng\"}},
                      {{\"id\":\"grp-platform\",\"name\":\"Platform Group\",\"parent_id\":\"dept-eng\"}},
                      {{\"id\":\"grp-delivery\",\"name\":\"Delivery Group\",\"parent_id\":\"dept-eng\"}}
                    ],
                    \"project_department_ids\":[\"grp-app\"],
                    \"members\":[
                      {{\"person_id\":\"member-app\",\"department_ids\":[\"grp-app\"]}},
                      {{\"person_id\":\"peer-dev\",\"department_ids\":[\"grp-platform\"]}},
                      {{\"person_id\":\"member-delivery\",\"department_ids\":[\"grp-delivery\"]}}
                    ]}}'::jsonb,
                 '{{\"actor_id\":\"audit-lead\",\"client_id\":\"cli-audit\"}}'::jsonb);"
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn usage_summary_works_on_real_mcp_and_web_service_and_matches_committed_samples() {
    let (_guard, admin, _db, store) = setup().await;
    seed_service_fixture(&admin).await;
    let server = start_service(store).await;

    // 1. Before any usage or session timestamps are recorded: all not_collected (never 0).
    let audit_cookie = login_web(&server.base, AUDIT_TOKEN).await;
    let (status, uncollected) = web_query(
        &server.base,
        "one",
        Some(&audit_cookie),
        web::ISSUER,
        json!({"protocol_version":1,"op":"usage.summary"}),
    )
    .await;
    assert_eq!(status, 200, "{uncollected}");
    assert_eq!(uncollected["data"]["coverage"]["status"], "not_collected");
    assert_eq!(
        uncollected["data"]["project"]["duration"]["status"],
        "not_collected"
    );
    assert!(uncollected["data"]["project"]["duration"]["summed_ms"].is_null());
    assert_eq!(
        uncollected["data"]["project"]["tokens"]["status"],
        "not_collected"
    );
    assert!(uncollected["data"]["project"]["tokens"]["input_tokens"].is_null());
    assert_eq!(
        uncollected["data"]["project"]["cost"]["status"],
        "not_collected"
    );
    assert!(uncollected["data"]["project"]["cost"]["amount_micros"].is_null());
    assert_eq!(
        uncollected["data"]["coverage"]["groups"]["missing"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    save_verbatim("usage-summary-web-uncollected.json", &uncollected);

    // 2. Record ended session + host usage for member-app (App Group) while Platform & Delivery remain missing.
    admin
        .batch_execute(&format!(
            "UPDATE awr_team.sessions SET state='ended'
             WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}' AND id='session-a';
             INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,workstream_id,payload_json,created_at)
             VALUES('{TENANT}','{PROJECT}','ev-svc-start',30,0,'session.start','agent','a','00000000000000000000000001',
                    '{{\"session_id\":\"session-a\"}}'::jsonb,'2026-11-19T09:00:00.000Z'::timestamptz),
                   ('{TENANT}','{PROJECT}','ev-svc-end',31,0,'session.end','agent','a','00000000000000000000000001',
                    '{{\"session_id\":\"session-a\"}}'::jsonb,'2026-11-19T09:30:00.000Z'::timestamptz);
             INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json,usage_json)
             VALUES('{TENANT}','{PROJECT}','cp-svc-1','session-a','ctx','ctr',31,'complete','[]',
                    '{{\"source\":\"native_host_event\",\"source_ref\":\"logs/session.jsonl#4\",\"counter_id\":\"host-1\",\"scope\":\"host_session\",\"coverage\":\"complete\",\"observed_at_unix_ms\":1795080600000,\"input_tokens\":6400,\"output_tokens\":1200,\"cached_input_tokens\":800}}'::jsonb);"
        ))
        .await
        .unwrap();

    // Verify MCP tool discovery lists usage.summary and returns identical data to Web.
    let audit_mcp = connect_mcp(&server.base, AUDIT_TOKEN).await;
    let tools = audit_mcp.list_all_tools().await.unwrap();
    let query_tool = tools.iter().find(|t| t.name == "awr_team_query").unwrap();
    assert!(
        query_tool.input_schema["properties"]["op"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("usage.summary"))
    );
    let mcp_audit = call_query(
        &audit_mcp,
        json!({"protocol_version":1,"op":"usage.summary"}),
        false,
    )
    .await;
    let (status, web_audit) = web_query(
        &server.base,
        "one",
        Some(&audit_cookie),
        web::ISSUER,
        json!({"protocol_version":1,"op":"usage.summary"}),
    )
    .await;
    assert_eq!(status, 200, "{web_audit}");
    assert_same_usage_reply(&mcp_audit, &web_audit);
    assert_eq!(web_audit["data"]["member_scope"], "project");
    assert_eq!(
        web_audit["data"]["project"]["duration"]["summed_ms"],
        1_800_000
    );
    assert_eq!(
        web_audit["data"]["project"]["tokens"]["input_tokens"],
        6_400
    );
    assert_eq!(
        web_audit["data"]["project"]["cost"]["status"],
        "not_collected"
    );
    assert_eq!(web_audit["data"]["coverage"]["status"], "partial");
    assert_eq!(
        web_audit["data"]["coverage"]["groups"]["collected"],
        json!([{"id":"grp-app","name":"App Group"}])
    );
    assert_eq!(
        web_audit["data"]["coverage"]["groups"]["missing"],
        json!([
            {"id":"grp-delivery","name":"Delivery Group"},
            {"id":"grp-platform","name":"Platform Group"}
        ])
    );
    save_verbatim("usage-summary-web-audit.json", &web_audit);

    // 3. Non-audit member (peer-dev) sees ONLY self; requesting another member is 403 Forbidden.
    let peer_cookie = login_web(&server.base, PEER_TOKEN).await;
    let (status, web_self) = web_query(
        &server.base,
        "one",
        Some(&peer_cookie),
        web::ISSUER,
        json!({"protocol_version":1,"op":"usage.summary"}),
    )
    .await;
    assert_eq!(status, 200, "{web_self}");
    assert_eq!(web_self["data"]["member_scope"], "self");
    assert_eq!(web_self["data"]["by_member"].as_array().unwrap().len(), 1);
    assert_eq!(web_self["data"]["by_member"][0]["member_id"], "peer-dev");
    assert_eq!(
        web_self["data"]["by_member"][0]["tokens"]["status"],
        "not_collected"
    );
    assert!(web_self["data"]["by_member"][0]["tokens"]["input_tokens"].is_null());
    assert!(!web_self.to_string().contains("member-app"));
    save_verbatim("usage-summary-web-self.json", &web_self);

    let (status, forbidden_other) = web_query(
        &server.base,
        "one",
        Some(&peer_cookie),
        web::ISSUER,
        json!({"protocol_version":1,"op":"usage.summary","member_actor_id":"agent"}),
    )
    .await;
    assert_eq!(status, 403, "{forbidden_other}");
    assert!(!forbidden_other.to_string().contains("6400"));

    // 4. Cross-project, unauthenticated, forbidden origin, invalid sort_by all refused.
    let (status, _) = web_query(
        &server.base,
        "foreign",
        Some(&audit_cookie),
        web::ISSUER,
        json!({"protocol_version":1,"op":"usage.summary"}),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = web_query(
        &server.base,
        "one",
        None,
        web::ISSUER,
        json!({"protocol_version":1,"op":"usage.summary"}),
    )
    .await;
    assert_eq!(status, 401);
    let (status, _) = web_query(
        &server.base,
        "one",
        Some(&audit_cookie),
        "https://not-allowed.example",
        json!({"protocol_version":1,"op":"usage.summary"}),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = web_query(
        &server.base,
        "one",
        Some(&audit_cookie),
        web::ISSUER,
        json!({"protocol_version":1,"op":"usage.summary","sort_by":"tokens_desc"}),
    )
    .await;
    assert_eq!(status, 400);

    // 5. Committed verbatim samples match live service replies (skipped only while regenerating).
    if std::env::var("AWR_USAGE_SAMPLE_DIR").is_err() {
        for (name, live) in [
            ("usage-summary-web-uncollected.json", &uncollected),
            ("usage-summary-web-audit.json", &web_audit),
            ("usage-summary-web-self.json", &web_self),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/samples")
                .join(name);
            let sample: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_clean_sample(&sample, live);
        }
    }

    audit_mcp.cancel().await.unwrap();
}
