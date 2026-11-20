#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/oauth_http.rs"]
mod web;
use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId, RevokeAuthorizationRequest,
};
use awr_team_pg::AuthorizationStore;
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

fn plan() -> Value {
    json!({"organization":{"id":"example","name":"Example company"},
        "departments":[{"id":"engineering","name":"Engineering"}],
        "project_department_ids":["engineering"],
        "members":[{"person_id":"directory-person","department_ids":["engineering"]}]})
}

/// A four-level chain; "scheduling" is the leaf group the project hangs under.
fn hierarchy_plan() -> Value {
    json!({"organization":{"id":"example","name":"Example company"},
        "departments":[{"id":"engineering","name":"Engineering"},
            {"id":"platform","name":"Platform","parent_id":"engineering"},
            {"id":"runtime","name":"Runtime","parent_id":"platform"},
            {"id":"scheduling","name":"Scheduling","parent_id":"runtime"}],
        "project_department_ids":["scheduling"],
        "members":[{"person_id":"directory-person","department_ids":["scheduling"]}]})
}

/// Write one verbatim reply into the evidence sample directory when configured.
/// The committed copies under tests/samples are compared in the same test run.
fn save_verbatim(name: &str, value: &Value) {
    let Ok(directory) = std::env::var("AWR_DIRECTORY_SAMPLE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    std::fs::write(directory.join(name), bytes).unwrap();
}

/// Fields that stay identical between a committed sample and a live reply.
/// Web-route samples wrap the reply under `data`; unwrap before comparing.
fn compare_directory_reply(sample: &Value, live: &Value) {
    let sample = sample.get("data").unwrap_or(sample);
    assert_eq!(sample["configured"], live["configured"]);
    assert_eq!(sample["version"], live["version"]);
    assert_eq!(sample["plan"], live["plan"]);
    assert_eq!(sample["members"], live["members"]);
    assert_eq!(sample["cross_project_person_merging"], false);
}

async fn connect(server: &web::Server) -> Mcp {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {A}").parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(format!(
            "{}/v1/projects/one/mcp",
            server.url
        )),
    );
    ().serve(transport).await.unwrap()
}

async fn call(client: &Mcp, name: &str, args: Value, error: bool) -> Value {
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

async fn seed_agent(admin: &tokio_postgres::Client) {
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','reader-project','directory-person','Project coordinator','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
        VALUES('reader-tenant','reader-project','directory-binding','directory-person','agent','active');
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage)
        SELECT tenant_id,project_id,actor_id,'cli-a',workstream_id,authority_version,true,true,true
        FROM awr_team.workstream_grants WHERE client_id='cli-b';
        UPDATE awr_team.workstream_grants SET can_read=true,can_write=true,can_manage=true WHERE client_id='cli-a';").await.unwrap();
}

async fn issue(
    store: &AuthorizationStore,
    id: &str,
    scope: AuthorizationScope,
    actions: BTreeSet<AuthorizedAction>,
) {
    store
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: format!("issue-{id}"),
                authorization: AgentAuthorization {
                    id: id.into(),
                    authorizer_person_id: PersonId::new("directory-person").unwrap(),
                    responsible_person_id: PersonId::new("directory-person").unwrap(),
                    subject_kind: ExecutionSubjectKind::Agent,
                    subject_id: "agent".into(),
                    client_id: "cli-a".into(),
                    session_id: None,
                    model_id: None,
                    scope,
                    actions,
                    expires_at_ms: None,
                    status: AuthorizationStatus::Active,
                    revoked_at_ms: None,
                    revoked_by: None,
                    verifiable_capabilities: vec![],
                    self_reported_skill_hints: vec![],
                    parent_authorization_id: None,
                    maintainer_person_id: None,
                    created_at_ms: 1,
                    binding_id: Some("directory-binding".into()),
                },
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn an_explicit_agent_can_manage_grouping_without_member_administration() {
    let (_guard, admin, db, store) = setup().await;
    seed_agent(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET business_roles='[\"supervisor\"]'
        WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND actor_id='agent'",
        )
        .await
        .unwrap();
    let grants =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let project = AuthorizationScope::Project {
        project_id: PROJECT.into(),
    };
    issue(
        &grants,
        "directory-reader",
        project.clone(),
        BTreeSet::from([AuthorizedAction::Inspect]),
    )
    .await;
    let server = web::start(store, false).await;
    let client = connect(&server).await;
    let tools = client.list_all_tools().await.unwrap();
    for suffix in ["read", "preview", "apply", "outcome"] {
        assert!(
            tools
                .iter()
                .any(|tool| tool.name == format!("awr_team_directory_{suffix}"))
        );
    }
    let empty = call(
        &client,
        "awr_team_directory_read",
        json!({"protocol_version":1}),
        false,
    )
    .await;
    assert_eq!(empty["configured"], false);
    let preview_args = json!({"protocol_version":1,"plan":plan()});
    let denied = call(
        &client,
        "awr_team_directory_preview",
        preview_args.clone(),
        true,
    )
    .await;
    assert_eq!(denied["code"], "Forbidden");
    issue(
        &grants,
        "directory-manager",
        project,
        BTreeSet::from([AuthorizedAction::ManageDirectory]),
    )
    .await;
    let preview = call(
        &client,
        "awr_team_directory_preview",
        preview_args.clone(),
        false,
    )
    .await;
    let http = server
        .post("/v1/projects/one/directory/preview")
        .bearer_auth(A)
        .json(&preview_args)
        .send()
        .await
        .unwrap();
    assert_eq!(http.status(), 200);
    let http: Value = http.json().await.unwrap();
    assert_eq!(http, preview);
    let apply = json!({"protocol_version":1,"request_id":"register-project-grouping",
        "expected_version":preview["expected_version"],"expected_plan_digest":preview["plan_digest"],"plan":plan()});
    let receipt = call(&client, "awr_team_directory_apply", apply.clone(), false).await;
    assert_eq!(receipt["receipt"]["version"], "1");
    assert_eq!(
        receipt["receipt"]["provenance"]["delegation_id"],
        "directory-manager"
    );
    assert_eq!(receipt["receipt"]["access_grants_changed"], false);
    let replay = server
        .post("/v1/projects/one/directory/apply")
        .bearer_auth(A)
        .json(&apply)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    let replay: Value = replay.json().await.unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], receipt["receipt"]);
    let outcome = call(
        &client,
        "awr_team_directory_outcome",
        json!({"protocol_version":1,"request_id":"register-project-grouping"}),
        false,
    )
    .await;
    assert_eq!(outcome["receipt"], receipt["receipt"]);
    let access = call(
        &client,
        "awr_team_access_inspect",
        json!({"protocol_version":1}),
        true,
    )
    .await;
    assert_eq!(access["code"], "Forbidden");
    let forged = server
        .post("/v1/projects/one/directory/read")
        .bearer_auth(A)
        .json(&json!({"protocol_version":1,"project_id":"other"}))
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 403);
    grants
        .revoke(
            TENANT,
            PROJECT,
            &RevokeAuthorizationRequest {
                request_key: "revoke-directory-manager".into(),
                authorization_id: "directory-manager".into(),
                revoked_by: PersonId::new("directory-person").unwrap(),
                revoked_at_ms: 2,
                reason: "Management responsibility ended".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        call(&client, "awr_team_directory_apply", apply, true).await["code"],
        "Forbidden"
    );
    assert_eq!(
        call(
            &client,
            "awr_team_directory_read",
            json!({"protocol_version":1}),
            false
        )
        .await["version"],
        "1"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_task_delegation_cannot_disclose_or_change_project_grouping() {
    let (_guard, admin, db, store) = setup().await;
    seed_agent(&admin).await;
    let grants =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    issue(
        &grants,
        "task-only-grouping",
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
        BTreeSet::from([AuthorizedAction::Inspect, AuthorizedAction::ManageDirectory]),
    )
    .await;
    let server = web::start(store, false).await;
    let client = connect(&server).await;
    for (tool, args) in [
        ("awr_team_directory_read", json!({"protocol_version":1})),
        (
            "awr_team_directory_preview",
            json!({"protocol_version":1,"plan":plan()}),
        ),
    ] {
        let denied = call(&client, tool, args, true).await;
        assert_eq!(denied["code"], "Forbidden");
        assert!(!denied.to_string().contains("Project coordinator"));
    }
    let denied = server
        .post("/v1/projects/one/directory/read")
        .bearer_auth(A)
        .json(&json!({"protocol_version":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn browser_directory_reads_reauthorize_the_current_session_and_keep_writes_on_agent_entries()
{
    let (_guard, admin, db, store) = setup().await;
    seed_agent(&admin).await;
    let grants =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    issue(
        &grants,
        "browser-directory-reader",
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        BTreeSet::from([AuthorizedAction::Inspect]),
    )
    .await;
    let server = web::start(store, false).await;
    let login = server
        .post("/v1/web/login")
        .header("origin", web::ISSUER)
        .header("x-awr-web", "1")
        .json(&json!({"bearer":A}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let read = |path: &str, body: Value| {
        server
            .post(path)
            .header("origin", web::ISSUER)
            .header("x-awr-web", "1")
            .header("cookie", &cookie)
            .json(&body)
    };
    let path = "/v1/web/projects/one/directory/read";
    let result = read(path, json!({"protocol_version":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 200);
    assert_eq!(result.headers()["access-control-allow-origin"], web::ISSUER);
    let result: Value = result.json().await.unwrap();
    assert_eq!(result["data"]["configured"], false);
    assert_eq!(result["data"]["members"], json!([]));
    admin.execute(
        "INSERT INTO awr_team.project_directories(tenant_id,project_id,version,plan_json,provenance)
         VALUES($1,$2,1,$3,'{}'::jsonb)",
        &[&TENANT, &PROJECT, &plan()],
    ).await.unwrap();
    let recorded: Value = read(path, json!({"protocol_version":1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recorded["data"]["configured"], true);
    assert_eq!(
        recorded["data"]["members"][0]["display_name"],
        "Project coordinator"
    );
    assert_eq!(recorded["data"]["members"][0]["available"], true);
    for (route, body, expected) in [
        (
            path,
            json!({"protocol_version":1,"project_id":"another-project"}),
            403,
        ),
        (path, json!({"protocol_version":1,"plan":plan()}), 400),
        (
            "/v1/web/projects/other/directory/read",
            json!({"protocol_version":1}),
            403,
        ),
        (
            "/v1/web/projects/one/directory/apply",
            json!({"protocol_version":1}),
            404,
        ),
    ] {
        assert_eq!(read(route, body).send().await.unwrap().status(), expected);
    }
    let missing_guard = server
        .post(path)
        .header("origin", web::ISSUER)
        .header("cookie", &cookie)
        .json(&json!({"protocol_version":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_guard.status(), 403);
    admin.batch_execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='browser-directory-reader'")
        .await.unwrap();
    let denied = read(path, json!({"protocol_version":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let denied: Value = denied.json().await.unwrap();
    assert!(!denied.to_string().contains("Project coordinator"));
}

#[tokio::test]
async fn hierarchical_directories_round_trip_the_real_entries_and_feed_the_samples() {
    let (_guard, admin, db, store) = setup().await;
    seed_agent(&admin).await;
    let grants =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let project = AuthorizationScope::Project {
        project_id: PROJECT.into(),
    };
    issue(
        &grants,
        "directory-reader",
        project.clone(),
        BTreeSet::from([AuthorizedAction::Inspect]),
    )
    .await;
    issue(
        &grants,
        "directory-manager",
        project,
        BTreeSet::from([AuthorizedAction::ManageDirectory]),
    )
    .await;
    let server = web::start(store, false).await;
    let client = connect(&server).await;
    // The published tool contract documents the optional parent reference.
    let tools = client.list_all_tools().await.unwrap();
    let apply_tool = tools
        .iter()
        .find(|tool| tool.name == "awr_team_directory_apply")
        .unwrap();
    let published = serde_json::to_value(&apply_tool.input_schema).unwrap();
    assert!(published.to_string().contains("parent_id"));
    // Web session for the browser-shaped read.
    let login = server
        .post("/v1/web/login")
        .header("origin", web::ISSUER)
        .header("x-awr-web", "1")
        .json(&json!({"bearer":A}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let web_read = |body: Value| {
        server
            .post("/v1/web/projects/one/directory/read")
            .header("origin", web::ISSUER)
            .header("x-awr-web", "1")
            .header("cookie", &cookie)
            .json(&body)
    };
    // 1. Unconfigured read, exactly as the browser route serves it.
    let unconfigured: Value = web_read(json!({"protocol_version":1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unconfigured["ok"], true);
    assert_eq!(unconfigured["data"]["configured"], false);
    save_verbatim("directory-web-read-unconfigured.json", &unconfigured);
    // 2. A flat plan — the legacy shape the service recorded before the field.
    let flat_preview = call(
        &client,
        "awr_team_directory_preview",
        json!({"protocol_version":1,"plan":plan()}),
        false,
    )
    .await;
    server
        .post("/v1/projects/one/directory/apply")
        .bearer_auth(A)
        .json(
            &json!({"protocol_version":1,"request_id":"flat-directory-v1",
            "expected_version":flat_preview["expected_version"],
            "expected_plan_digest":flat_preview["plan_digest"],"plan":plan()}),
        )
        .send()
        .await
        .unwrap();
    let flat: Value = web_read(json!({"protocol_version":1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(flat["data"]["version"], "1");
    assert_eq!(
        flat["data"]["plan"]["departments"][0].get("parent_id"),
        None
    );
    save_verbatim("directory-web-read-flat.json", &flat);
    // 3. The hierarchy: preview, apply, then read through both real entries.
    let preview = call(
        &client,
        "awr_team_directory_preview",
        json!({"protocol_version":1,"plan":hierarchy_plan()}),
        false,
    )
    .await;
    assert_eq!(preview["expected_version"], "1");
    let receipt = call(
        &client,
        "awr_team_directory_apply",
        json!({"protocol_version":1,"request_id":"hierarchy-directory-v2",
            "expected_version":preview["expected_version"],
            "expected_plan_digest":preview["plan_digest"],"plan":hierarchy_plan()}),
        false,
    )
    .await;
    assert_eq!(receipt["receipt"]["version"], "2");
    let agent_http: Value = server
        .post("/v1/projects/one/directory/read")
        .bearer_auth(A)
        .json(&json!({"protocol_version":1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let agent_tool = call(
        &client,
        "awr_team_directory_read",
        json!({"protocol_version":1}),
        false,
    )
    .await;
    assert_eq!(agent_http, agent_tool, "HTTP and MCP serve one reply");
    assert_eq!(agent_http["version"], "2");
    assert_eq!(
        agent_http["plan"]["departments"].as_array().unwrap().len(),
        4
    );
    assert_eq!(agent_http["plan"]["departments"][3]["parent_id"], "runtime");
    assert_eq!(
        agent_http["plan"]["project_department_ids"],
        json!(["scheduling"])
    );
    assert_eq!(
        agent_http["members"][0]["department_ids"],
        json!(["scheduling"])
    );
    save_verbatim("directory-agent-read-hierarchy.json", &agent_http);
    let web_hierarchy: Value = web_read(json!({"protocol_version":1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(web_hierarchy["ok"], true);
    assert_eq!(web_hierarchy["data"], agent_http);
    save_verbatim("directory-web-read-hierarchy.json", &web_hierarchy);
    // The committed samples parse and keep the live deterministic shape.
    for (name, live) in [
        (
            "directory-web-read-unconfigured.json",
            &unconfigured["data"],
        ),
        ("directory-web-read-flat.json", &flat["data"]),
        ("directory-agent-read-hierarchy.json", &agent_http),
        ("directory-web-read-hierarchy.json", &web_hierarchy["data"]),
    ] {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/samples")
            .join(name);
        if !full.exists() {
            continue;
        }
        let sample: Value = serde_json::from_slice(&std::fs::read(full).unwrap()).unwrap();
        compare_directory_reply(&sample, live);
    }
    client.cancel().await.unwrap();
}
