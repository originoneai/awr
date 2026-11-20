#![cfg(feature = "pg-tests")]
//! AWR-PPD-060: the portfolio overview tool on the real service. The verbatim
//! replies below are saved as samples under tests/samples and re-parsed by
//! every run, so the bridge tests of the follow-up PR can consume genuine
//! service replies instead of hand-written shapes.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/oauth_http.rs"]
mod web;

use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
};
use awr_server::service::{OAuthConfig, ProjectBinding, ServiceConfig};
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
use std::collections::BTreeSet;

type Mcp = RunningService<RoleClient, ()>;

const CLONE: &str = "portfolio-project-001";

/// Write one verbatim reply into the evidence sample directory when
/// configured. The committed copies under tests/samples are compared in the
/// same test run.
fn save_verbatim(name: &str, value: &Value) {
    let Ok(directory) = std::env::var("AWR_PORTFOLIO_SAMPLE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    std::fs::write(directory.join(name), bytes).unwrap();
}

/// The fields of a member-page reply that stay stable between the committed
/// sample and a live reply; volatile facts (timestamps, ids) are compared by
/// shape only.
fn compare_member_reply(sample: &Value, live: &Value) {
    assert_eq!(sample["protocol_version"], 1);
    assert_eq!(sample["op"], "portfolio.overview");
    assert_eq!(sample["selection_basis"], "member");
    assert_eq!(
        sample["data"]["total_projects"],
        live["data"]["total_projects"]
    );
    assert_eq!(sample["data"]["truncated"], live["data"]["truncated"]);
    assert!(sample["data"]["execution_authorized"] == false);
    let sample_items = sample["data"]["projects"].as_array().unwrap();
    let live_items = live["data"]["projects"].as_array().unwrap();
    assert_eq!(sample_items.len(), live_items.len());
    for (sample_item, live_item) in sample_items.iter().zip(live_items) {
        assert_eq!(sample_item["project_id"], live_item["project_id"]);
        assert_eq!(sample_item["project_key"], live_item["project_key"]);
        assert_eq!(sample_item["project_status"], live_item["project_status"]);
        assert_eq!(sample_item["read"], live_item["read"]);
        assert_eq!(sample_item["works"], live_item["works"]);
        for kind in ["decision", "dependency", "review"] {
            assert_eq!(sample_item["waits"][kind], live_item["waits"][kind]);
        }
        assert_eq!(
            sample_item["waits"]["oldest_open_unix_ms"].is_null(),
            live_item["waits"]["oldest_open_unix_ms"].is_null()
        );
        // recorded_at is volatile too; the author and period stay compared.
        assert_eq!(
            sample_item["latest_update"]["update_id"],
            live_item["latest_update"]["update_id"]
        );
        assert_eq!(
            sample_item["latest_update"]["status"],
            live_item["latest_update"]["status"]
        );
        assert_eq!(
            sample_item["latest_update"]["author"],
            live_item["latest_update"]["author"]
        );
        assert_eq!(sample_item["directory"], live_item["directory"]);
        assert_eq!(sample_item["responsible_person"], Value::Null);
    }
    assert!(sample["data"]["as_of"]["at_unix_ms"].is_i64());
}

/// Start the real service with the base project and one cloned project
/// registered under the same tenant, so the member page spans projects.
/// Returns the base url and the spawned task; the caller awaits or drops it.
async fn start_multi_project(store: WorkstreamReadStore) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec!["awr.example".into()],
        allowed_web_origins: vec!["https://awr.example".into()],
        oauth: None::<OAuthConfig>,
        projects: vec![
            ProjectBinding {
                key: "one".into(),
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
            },
            ProjectBinding {
                key: "two".into(),
                tenant_id: TENANT.into(),
                project_id: CLONE.into(),
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
    (format!("http://{address}"), task)
}

async fn connect_at(server: &str, key: &str, bearer: &str) -> Mcp {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {bearer}").parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(format!("{server}/v1/projects/{key}/mcp")),
    );
    ().serve(transport).await.unwrap()
}

async fn connect(server: &str, bearer: &str) -> Mcp {
    connect_at(server, "one", bearer).await
}

async fn call(client: &Mcp, args: Value, error: bool) -> Value {
    let result = client
        .call_tool(
            CallToolRequestParams::new("awr_team_portfolio_overview".to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error.unwrap_or(false), error, "{result:?}");
    result.structured_content.unwrap()
}

/// Clone the fixture project with works, waits, completions and a period
/// update, exactly like the pg-suite scenario: the samples then carry every
/// field the bridge renders.
async fn seed_clone(admin: &tokio_postgres::Client) {
    let snapshot = "snapshot-portfolio-server";
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status,active_snapshot_id,project_revision)
             VALUES('{TENANT}','{CLONE}','pf-server','team','epoch-portfolio','active','{snapshot}',3);
             UPDATE awr_team.workstream_modes SET enabled=true WHERE tenant_id='{TENANT}' AND project_id='{CLONE}';
             INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                SELECT tenant_id,'{CLONE}',id,name,status FROM awr_team.work_scopes
                WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}';
             INSERT INTO awr_team.source_snapshots(tenant_id,project_id,id,manifest_digest,source_ref_json,parser_version,created_by)
                SELECT tenant_id,'{CLONE}','{snapshot}',manifest_digest,source_ref_json,parser_version,created_by
                FROM awr_team.source_snapshots s WHERE s.tenant_id='{TENANT}' AND s.project_id='{PROJECT}'
                  AND s.id=(SELECT active_snapshot_id FROM awr_team.projects p
                            WHERE p.tenant_id='{TENANT}' AND p.id='{PROJECT}');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                SELECT tenant_id,'{CLONE}',id,external_key FROM awr_team.work_items
                WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}';
             INSERT INTO awr_team.work_contracts(tenant_id,project_id,snapshot_id,scope_id,work_id,contract_hash,definition_state,title,contract_json)
                SELECT tenant_id,'{CLONE}','{snapshot}',scope_id,work_id,contract_hash,definition_state,title,contract_json
                FROM awr_team.work_contracts w WHERE w.tenant_id='{TENANT}' AND w.project_id='{PROJECT}'
                  AND w.snapshot_id=(SELECT active_snapshot_id FROM awr_team.projects p
                                     WHERE p.tenant_id='{TENANT}' AND p.id='{PROJECT}');
             INSERT INTO awr_team.workstream_catalogs(tenant_id,project_id,snapshot_id,catalog_json,projection_hash)
                SELECT tenant_id,'{CLONE}','{snapshot}',
                    jsonb_set(jsonb_set(catalog_json,'{{project_id}}',to_jsonb('{CLONE}'::text)),
                        '{{workstreams}}',
                        (SELECT jsonb_agg(jsonb_set(ws,'{{project_id}}',to_jsonb('{CLONE}'::text)))
                         FROM jsonb_array_elements(catalog_json->'workstreams') ws)),
                    projection_hash
                FROM awr_team.workstream_catalogs c WHERE c.tenant_id='{TENANT}' AND c.project_id='{PROJECT}'
                  AND c.snapshot_id=(SELECT active_snapshot_id FROM awr_team.projects p
                                     WHERE p.tenant_id='{TENANT}' AND p.id='{PROJECT}');
             INSERT INTO awr_team.workstream_ownership(tenant_id,project_id,work_id,workstream_id,ownership_version)
                SELECT tenant_id,'{CLONE}',work_id,workstream_id,ownership_version FROM awr_team.workstream_ownership
                WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}';
             INSERT INTO awr_team.workstream_snapshot_ownership(tenant_id,project_id,snapshot_id,scope_id,work_id,workstream_id,ownership_version)
                SELECT tenant_id,'{CLONE}','{snapshot}',scope_id,work_id,workstream_id,ownership_version
                FROM awr_team.workstream_snapshot_ownership o WHERE o.tenant_id='{TENANT}' AND o.project_id='{PROJECT}'
                  AND o.snapshot_id=(SELECT active_snapshot_id FROM awr_team.projects p
                                     WHERE p.tenant_id='{TENANT}' AND p.id='{PROJECT}');
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
                SELECT tenant_id,'{CLONE}',actor_id,role FROM awr_team.project_memberships
                WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}';
             INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,active,grant_version)
                SELECT tenant_id,'{CLONE}',actor_id,client_id,workstream_id,authority_version,can_read,true,1
                FROM awr_team.workstream_grants WHERE tenant_id='{TENANT}' AND project_id='{PROJECT}';
             INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
                VALUES('{TENANT}','{CLONE}','main','a','claimed',1,0);
             INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
                VALUES('{TENANT}','{CLONE}','session-clone','main','a','agent','cli-a','session-clone','active',
                    '00000000000000000000000001',1);
             INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json,progress_json)
                VALUES('{TENANT}','{CLONE}','cp-clone-1','session-clone','context','contract',1,'continue','[]',
                    '{{\"phase\":\"waiting_dependency\"}}'::jsonb);
             INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state,decision_key,decision_baseline_digest,decision_json,decision_version)
                VALUES('{TENANT}','{CLONE}','decision-clone','session-clone','a','Synthetic decision question','open','decision-key-clone',
                    repeat('0',64),
                    '{{\"basis\":{{}},\"request_digest\":\"synthetic\",\"impact\":\"Synthetic impact.\",\"allowed_responder_person_ids\":[\"reviewer\"],\"requester\":{{\"actor_id\":\"agent\",\"client_id\":\"cli-a\",\"person_id\":null}},\"source_snapshot_id\":\"synthetic\"}}'::jsonb,1);
             INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,independence_kind,author_actor_id,final_submitter_actor_id)
                VALUES('{TENANT}','{CLONE}','receipt-clone','c','main','contract','result','deps','evidence','review','{{}}','team_independent','agent','agent');
             INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json)
                VALUES('{TENANT}','{CLONE}','event-clone-completed',10,0,'work.completed','agent','c',
                    '{{\"receipt_id\":\"receipt-clone\",\"independence_kind\":\"team_independent\",\"team_independent_acceptance\":true}}'::jsonb);
             INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,selected_completion_id)
                VALUES('{TENANT}','{CLONE}','main','c','completed',1,0,'receipt-clone');
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
                VALUES('{TENANT}','{CLONE}','lead-portfolio','Portfolio Lead','active');
             INSERT INTO awr_team.project_period_updates(tenant_id,project_id,id,period_kind,period_key,starts_on,ends_on,status,summary,author_person_id,author_display_name,recorded_by_actor_id,recorded_by_client_id,request_id,version)
                VALUES('{TENANT}','{CLONE}','update-clone','week','2026-W47','2026-11-16','2026-11-22','risk',
                    'A synthetic supervisor sentence.','lead-portfolio','Portfolio Lead','agent','cli-a','update-clone',1);
             INSERT INTO awr_team.project_period_update_current(tenant_id,project_id,period_kind,period_key,update_id)
                VALUES('{TENANT}','{CLONE}','week','2026-W47','update-clone');
             INSERT INTO awr_team.project_directories(tenant_id,project_id,version,plan_json,provenance)
                VALUES('{TENANT}','{CLONE}',1,
                    '{{\"organization\":{{\"id\":\"example\",\"name\":\"Example company\"}},\"departments\":[{{\"id\":\"eng\",\"name\":\"Engineering\"}}],\"project_department_ids\":[\"eng\"],\"members\":[]}}'::jsonb,
                    '{{\"actor_id\":\"agent\",\"client_id\":\"cli-a\"}}'::jsonb);"
        ))
        .await
        .unwrap();
}

async fn issue_read_delegation(db: &str) {
    let store = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db));
    store
        .issue(
            TENANT,
            CLONE,
            &IssueAuthorizationRequest {
                request_key: "issue-portfolio-agent".into(),
                authorization: AgentAuthorization {
                    id: "portfolio-agent-auth".into(),
                    authorizer_person_id: PersonId::new("lead-portfolio").unwrap(),
                    responsible_person_id: PersonId::new("lead-portfolio").unwrap(),
                    subject_kind: ExecutionSubjectKind::Agent,
                    subject_id: "agent".into(),
                    client_id: "cli-a".into(),
                    session_id: None,
                    model_id: None,
                    scope: AuthorizationScope::Project {
                        project_id: CLONE.into(),
                    },
                    actions: BTreeSet::from([AuthorizedAction::Inspect]),
                    expires_at_ms: None,
                    status: AuthorizationStatus::Active,
                    revoked_at_ms: None,
                    revoked_by: None,
                    verifiable_capabilities: vec![],
                    self_reported_skill_hints: vec![],
                    parent_authorization_id: None,
                    maintainer_person_id: None,
                    created_at_ms: 1,
                    binding_id: Some("portfolio-agent-binding".into()),
                },
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn portfolio_tool_replies_match_committed_samples() {
    let (_guard, admin, db, store) = fixture::setup().await;
    seed_clone(&admin).await;
    // Probe the same admission the middleware runs, to separate store failures
    // from service failures when debugging.
    store
        .authorize_project_entry(TENANT, PROJECT, A)
        .await
        .expect("base project entry must pass before the service starts");
    let (server, server_task) = start_multi_project(store).await;

    // Member page over both registered projects of the tenant.
    let client = connect(&server, A).await;
    let member = call(&client, json!({"protocol_version":1}), false).await;
    let items = member["data"]["projects"].as_array().unwrap();
    assert_eq!(
        items.len(),
        2,
        "both tenant projects, foreign tenant stays out"
    );
    let clone_item = items
        .iter()
        .find(|item| item["project_id"] == CLONE)
        .unwrap();
    assert_eq!(clone_item["read"]["status"], "complete");
    assert_eq!(clone_item["works"]["verified"], json!(1));
    assert_eq!(clone_item["waits"]["decision"], json!(1));
    assert_eq!(clone_item["waits"]["dependency"], json!(1));
    assert_eq!(
        clone_item["latest_update"]["author"]["display_name"],
        "Portfolio Lead"
    );
    assert_eq!(clone_item["directory"]["group"]["name"], "Engineering");
    let base_item = items
        .iter()
        .find(|item| item["project_id"] == PROJECT)
        .unwrap();
    assert_eq!(base_item["read"]["status"], "complete");
    assert_eq!(base_item["latest_update"], Value::Null);
    assert_eq!(base_item["directory"]["registered"], false);
    save_verbatim("portfolio-mcp-member.json", &member);

    // A bounded page with a resume cursor.
    let paged = call(&client, json!({"protocol_version":1,"limit":1}), false).await;
    assert_eq!(paged["data"]["projects"].as_array().unwrap().len(), 1);
    assert_eq!(paged["data"]["truncated"], true);
    assert!(paged["data"]["next_cursor"].is_string());
    save_verbatim("portfolio-mcp-paged.json", &paged);

    // Committed samples are re-parsed and compared against the live replies.
    for (name, live) in [
        ("portfolio-mcp-member.json", &member),
        ("portfolio-mcp-paged.json", &paged),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/samples")
            .join(name);
        let sample: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if name.contains("paged") {
            assert_eq!(sample["data"]["truncated"], true);
            assert!(sample["data"]["next_cursor"].is_string());
            assert_eq!(
                sample["data"]["total_projects"],
                live["data"]["total_projects"]
            );
        } else {
            compare_member_reply(&sample, live);
        }
        let text = sample.to_string();
        assert!(!text.contains("PRIVATE NEXT ACTION"));
        for banned in ["secret", "password", "bearer"] {
            assert!(!text.to_lowercase().contains(banned), "{banned} leaked");
        }
    }

    // Invalid inputs stay rejected through the real service.
    call(&client, json!({"protocol_version":2}), true).await;
    call(&client, json!({"protocol_version":1,"limit":0}), true).await;
    call(&client, json!({"protocol_version":1,"limit":201}), true).await;
    call(
        &client,
        json!({"protocol_version":1,"actor_id":"forged"}),
        true,
    )
    .await;
    drop(client);

    // An Agent reader with one live project-scope read delegation sees the
    // delegated project and nothing else.
    admin
        .batch_execute(&format!(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='{TENANT}' AND id='agent';
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('{TENANT}','{CLONE}','portfolio-agent-binding','lead-portfolio','agent','active');"
        ))
        .await
        .unwrap();
    issue_read_delegation(&db).await;
    let agent_client = connect_at(&server, "two", A).await;
    let agent_reply = call(&agent_client, json!({"protocol_version":1}), false).await;
    let agent_items = agent_reply["data"]["projects"].as_array().unwrap();
    assert_eq!(agent_items.len(), 2);
    let agent_clone = agent_items
        .iter()
        .find(|item| item["project_id"] == CLONE)
        .unwrap();
    assert_eq!(agent_clone["read"]["status"], "complete");
    let agent_base = agent_items
        .iter()
        .find(|item| item["project_id"] == PROJECT)
        .unwrap();
    assert_eq!(agent_base["read"]["reason"], "no_read_authority");
    save_verbatim("portfolio-mcp-agent.json", &agent_reply);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples/portfolio-mcp-agent.json");
    let sample: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        sample["data"]["total_projects"],
        agent_reply["data"]["total_projects"]
    );
    for (sample_item, live_item) in sample["data"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .zip(agent_reply["data"]["projects"].as_array().unwrap())
    {
        assert_eq!(sample_item["project_id"], live_item["project_id"]);
        assert_eq!(sample_item["read"], live_item["read"]);
    }
    drop(agent_client);
    server_task.abort();
}
