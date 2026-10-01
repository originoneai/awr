#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team::{
    DraftChange, DraftDefinitionState, DraftOpKind, OrdinaryPlanningSelfApprovePolicy, TaskDraft,
};
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
async fn connect(server: &Server, token: &str) -> Client {
    let transport = StreamableHttpClientTransport::with_client(
        http(),
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

fn draft(id: &str) -> TaskDraft {
    TaskDraft {
        work_id: id.into(),
        external_key: id.into(),
        title: format!("Task {id}"),
        goals: vec!["delivery".into()],
        scope_paths: vec!["specs/api.md".into()],
        acceptance: vec!["ok".into()],
        required_dependencies: vec![],
        completion_policy: "independent_review".into(),
        definition_state: DraftDefinitionState::Draft,
        workstream: None,
        split_from: None,
        split_children: vec![],
    }
}

#[tokio::test]
async fn planning_tools_catalog_and_suggest_idempotent_http_mcp_parity() {
    let (_g, admin, db, store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let client = connect(&server, A).await;

    let tools = client.list_tools(Default::default()).await.unwrap();
    let names: Vec<_> = tools.tools.iter().map(|t| t.name.to_string()).collect();
    for expected in [
        "awr_team_planning_suggest",
        "awr_team_planning_draft",
        "awr_team_planning_preview",
        "awr_team_planning_approve",
        "awr_team_planning_publish",
        "awr_team_planning_outcome",
    ] {
        assert!(names.iter().any(|n| n == expected), "{names:?}");
    }

    let caps = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
        false,
    )
    .await;
    assert_eq!(caps["artifact_content"], true);
    assert_eq!(caps["source_content"], true);
    assert_eq!(caps["planning_mcp"]["sql_tools"], false);
    assert_eq!(caps["planning_mcp"]["arbitrary_file_edit"], false);
    assert_eq!(caps["planning_mcp"]["direct_done"], false);

    // Forged identity refused.
    let denied = call(
        &client,
        "awr_team_planning_suggest",
        json!({
            "protocol_version":1,
            "request_id":"sug-1",
            "rationale":"x".repeat(8),
            "affected_work_keys":["a"],
            "actor_id":"forged"
        }),
        true,
    )
    .await;
    assert_eq!(denied["code"], "Forbidden");

    let sug = call(
        &client,
        "awr_team_planning_suggest",
        json!({
            "protocol_version":1,
            "request_id":"sug-stable-1",
            "rationale":"Need a shared dependency for the SDK",
            "affected_work_keys":["CLIENT-1"],
            "proposed_notes":{"add":"SHARED-1"}
        }),
        false,
    )
    .await;
    assert_eq!(sug["already_recorded"], false);
    assert_eq!(sug["op"], "planning.propose");
    assert_eq!(sug["result"]["claimable"], false);

    let replay = call(
        &client,
        "awr_team_planning_suggest",
        json!({
            "protocol_version":1,
            "request_id":"sug-stable-1",
            "rationale":"Need a shared dependency for the SDK",
            "affected_work_keys":["CLIENT-1"],
            "proposed_notes":{"add":"SHARED-1"}
        }),
        false,
    )
    .await;
    assert_eq!(replay["already_recorded"], true);

    let outcome = call(
        &client,
        "awr_team_planning_outcome",
        json!({"protocol_version":1,"request_id":"sug-stable-1"}),
        false,
    )
    .await;
    assert_eq!(outcome["already_recorded"], true);
    assert!(outcome["next_step"].as_str().unwrap().contains("reuse"));

    // HTTP parity for outcome
    let http_out = http()
        .post(format!("{}/one/planning/outcome", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"request_id":"sug-stable-1"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(http_out["request_id"], "sug-stable-1");
    assert_eq!(http_out["already_recorded"], true);

    // Query planning.outcome
    let q = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"planning.outcome","request_id":"sug-stable-1"}),
        false,
    )
    .await;
    assert_eq!(q["data"]["already_recorded"], true);

    // Path bypass refused on source.content
    let bad = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"source.content","source_path":"../secret"}),
        true,
    )
    .await;
    assert!(
        bad["code"] == "InvalidInput" || bad["code"] == "Forbidden" || bad["code"] == "Unsupported",
        "{bad}"
    );

    let _ = (admin, db);
}

#[tokio::test]
async fn planning_draft_create_via_mcp_uses_business_entrypoint() {
    let (_g, admin, _db, store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
               ('reader-tenant','reader-project','API-1','API-1')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let client = connect(&server, A).await;
    let after = draft("SHARED-1");
    let changes = vec![DraftChange {
        op: DraftOpKind::CreateTask,
        before: None,
        after,
    }];
    let body = json!({
        "protocol_version":1,
        "request_id":"draft-1",
        "mode":"create",
        "changes": changes,
        "allowed_spec_roots":["specs"],
        "project_goal_keys":["delivery"],
        "self_approve_policy": OrdinaryPlanningSelfApprovePolicy::ordinary_default(),
    });
    let created = call(&client, "awr_team_planning_draft", body, false).await;
    assert_eq!(created["op"], "planning.edit_draft");
    assert!(created["result"]["candidate_id"].as_str().unwrap().len() > 10);
    let preview = call(
        &client,
        "awr_team_planning_preview",
        json!({
            "protocol_version":1,
            "candidate_id": created["result"]["candidate_id"]
        }),
        false,
    )
    .await;
    assert!(
        preview.get("diff").is_some() || preview.get("candidate_id").is_some(),
        "{preview}"
    );
}

#[tokio::test]
async fn storage_failure_http_mcp_and_original_request_recovery() {
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let root = std::env::temp_dir().join(format!("awr-planning-storage-{}", common::nonce(0)));
    std::fs::create_dir(&root).unwrap();
    let definitions: Vec<_> = [
        ("00000000000000000000000001", "alpha"),
        ("00000000000000000000000002", "private-beta"),
    ]
    .into_iter()
    .map(|(id, key)| {
        json!({
            "id": id, "external_key": key, "title": key, "state": "active",
            "authority_version": 1, "goal_keys": [key], "acceptance_contracts": []
        })
    })
    .collect();
    let works: Vec<_> = [
        ("a", "alpha", vec![]),
        ("b-private", "private-beta", vec![]),
        ("c", "alpha", vec!["b-private"]),
    ]
    .into_iter()
    .map(|(id, stream, deps)| {
        json!({
            "id": id, "title": id, "status": "planned", "workstream": stream,
            "goals": [stream], "acceptance": ["verified"], "paths": ["src"], "depends_on": deps
        })
    })
    .collect();
    let before = serde_json::to_vec(&json!({
        "workstreams": {"version": 1, "definitions": definitions},
        "goals": [{"id":"alpha","title":"Alpha","status":"active"},
                  {"id":"private-beta","title":"Private","status":"active"}],
        "work_items": works
    }))
    .unwrap();
    let ledger = root.join("ledger.yaml");
    std::fs::write(&ledger, &before).unwrap();
    // Register only this test's temporary source against its isolated fixture.
    let binding = json!({"kind":"server_directory","locator":root,
                         "ledger_relative_path":"ledger.yaml"});
    admin
        .execute(
            "UPDATE awr_team.source_snapshots s
         SET source_ref_json=jsonb_set(s.source_ref_json,'{sole_source}',$3)
         FROM awr_team.projects p
         WHERE p.tenant_id=$1 AND p.id=$2 AND s.tenant_id=p.tenant_id
           AND s.project_id=p.id AND s.id=p.active_snapshot_id",
            &[&TENANT, &PROJECT, &binding],
        )
        .await
        .unwrap();
    let server = start(store).await;
    let client = connect(&server, A).await;
    let mut after = draft("NEW-1");
    after.workstream = Some("alpha".into());
    let created = call(
        &client,
        "awr_team_planning_draft",
        json!({
            "protocol_version":1, "request_id":"storage-draft", "mode":"create",
            "changes":[DraftChange {op:DraftOpKind::CreateTask,before:None,after}],
            "allowed_spec_roots":["specs"], "project_goal_keys":["delivery"],
            "self_approve_policy":OrdinaryPlanningSelfApprovePolicy::ordinary_default()
        }),
        false,
    )
    .await;
    let candidate = &created["result"]["candidate_id"];
    let digest = &created["result"]["candidate_digest"];
    call(
        &client,
        "awr_team_planning_approve",
        json!({
            "protocol_version":1,"request_id":"storage-approve",
            "candidate_id":candidate,"candidate_digest":digest
        }),
        false,
    )
    .await;
    let body = json!({
        "protocol_version":1,"request_id":"storage-activate",
        "candidate_id":candidate,"candidate_digest":digest,
        "activate":true,"impact_proven":true
    });
    let obstruction = root.join(".ledger.yaml.tmcp022.tmp");
    std::fs::create_dir(&obstruction).unwrap();
    let failed = call(&client, "awr_team_planning_publish", body.clone(), true).await;
    assert_eq!(failed["code"], "SourceStorageUnavailable");
    assert!(!failed.to_string().contains(root.to_str().unwrap()));
    assert!(
        failed["next_step"]
            .as_str()
            .unwrap()
            .contains("planning.outcome")
    );
    // HTTP uses 503; MCP uses isError with the same bounded structured body.
    let http_result = http()
        .post(format!("{}/one/planning/publish", server.url))
        .bearer_auth(A)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        http_result.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(http_result.json::<Value>().await.unwrap(), failed);
    assert_eq!(std::fs::read(&ledger).unwrap(), before);
    let outcome_args = json!({"protocol_version":1,"request_id":"storage-activate"});
    let unknown = call(
        &client,
        "awr_team_planning_outcome",
        outcome_args.clone(),
        false,
    )
    .await;
    assert_eq!(unknown["already_recorded"], false);
    assert!(unknown["result"].is_null());
    assert!(unknown["next_step"].as_str().unwrap().contains("unknown"));
    let reserved = admin
        .query_one(
            "SELECT c.request_hash,c.status,j.phase FROM awr_team.planning_command_receipts c
         JOIN awr_team.planning_writeback_journals j USING(tenant_id,project_id,request_id)
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.request_id='storage-activate'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    let original_hash: String = reserved.get(0);
    assert_eq!(reserved.get::<_, String>(1), "reserved");
    assert_eq!(reserved.get::<_, String>(2), "validated");
    let mut changed = body.clone();
    changed["impact_proven"] = json!(false);
    let conflict = call(&client, "awr_team_planning_publish", changed, true).await;
    assert_eq!(conflict["code"], "IdempotencyConflict");
    std::fs::remove_dir(&obstruction).unwrap();
    let resumed = call(&client, "awr_team_planning_publish", body.clone(), false).await;
    assert_eq!(resumed["already_recorded"], false);
    assert_eq!(resumed["request_hash"], original_hash);
    assert!(resumed["result"]["activation"]["activated_snapshot_id"].is_string());
    let replay = call(&client, "awr_team_planning_publish", body, false).await;
    assert_eq!(replay["already_recorded"], true);
    let completed = call(&client, "awr_team_planning_outcome", outcome_args, false).await;
    assert_eq!(completed["already_recorded"], true);
    assert_eq!(completed["request_hash"], original_hash);
    assert_eq!(completed["result"]["result"], resumed["result"]);
    let source = std::fs::read_to_string(&ledger).unwrap();
    assert_eq!(source.matches("id: NEW-1").count(), 1);
    let receipts: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_publish_receipts
         WHERE tenant_id=$1 AND project_id=$2 AND candidate_id=$3",
            &[&TENANT, &PROJECT, &candidate.as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(receipts, 1);
    std::fs::remove_dir_all(root).unwrap();
}
