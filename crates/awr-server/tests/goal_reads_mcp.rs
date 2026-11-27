//! Goal reads exercised through the production MCP router with isolated source facts.
#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;

use awr_core::Id;
use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team_pg::{PgError, SourceFile};
use fixture::*;
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};

fn sources() -> Vec<SourceFile> {
    vec![
        SourceFile {path:"source_binding.json".into(), bytes:br#"{"kind":"server_directory","locator":"/synthetic","ledger_relative_path":"ledger.yaml"}"#.to_vec()},
        SourceFile {path:"ledger.yaml".into(), bytes:br#"
workstreams:
  version: 1
  definitions:
    - id: '00000000000000000000000001'
      external_key: delivery
      title: Delivery
      state: active
      authority_version: 1
      goal_keys: [ship, alpha]
      acceptance_contracts: []
goals:
  ship:
    title: Ship reviewed outcome
    status: confirmed
    success_criteria: [Reviewed delivery]
    summary: SOURCE BODY SENTINEL
  alpha:
    title: No supporting tasks
    status: active
  retired:
    title: Retired goal
    status: cancelled
  archived:
    title: Archived goal
    status: archived
  private:
    title: HIDDEN GOAL SENTINEL
    status: active
  unknown:
    title: Undecided status
    status: proposed
"#.to_vec()}]
}

async fn prepare_goals(admin: &tokio_postgres::Client) {
    use awr_team::{
        DraftDefinitionState, SourceActivationPlan, TaskDefinition, TaskDefinitions,
        WorkstreamBundle,
    };
    use awr_team_pg::{IngestRequest, SourceStore};
    let db: String = admin
        .query_one("SELECT current_database()", &[])
        .await
        .unwrap()
        .get(0);
    let stored: Value = admin.query_one("SELECT s.source_ref_json FROM awr_team.projects p
        JOIN awr_team.source_snapshots s ON s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id
        WHERE p.tenant_id=$1 AND p.id=$2",&[&TENANT,&PROJECT]).await.unwrap().get(0);
    let mut files: Vec<SourceFile> = stored["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| SourceFile {
            path: file["path"].as_str().unwrap().into(),
            bytes: file["text"].as_str().unwrap().as_bytes().to_vec(),
        })
        .collect();
    let bundle_file = files
        .iter_mut()
        .find(|file| file.path == "workstreams.json")
        .unwrap();
    let mut bundle: WorkstreamBundle = serde_json::from_slice(&bundle_file.bytes).unwrap();
    bundle
        .contracts
        .iter_mut()
        .find(|entry| entry.contract.work_id.as_str() == "a")
        .unwrap()
        .contract
        .goals = ["ship", "retired", "archived", "missing", "unknown"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    bundle_file.bytes = serde_json::to_vec(&bundle).unwrap();
    let definitions = TaskDefinitions {
        works: bundle
            .contracts
            .iter()
            .map(|entry| {
                let work = entry.contract.work_id.as_str();
                (
                    work.into(),
                    TaskDefinition {
                        title: match work {
                            "a" => "Visible task",
                            "b-private" => "HIDDEN TASK SENTINEL",
                            _ => "Supporting task",
                        }
                        .into(),
                        definition_state: DraftDefinitionState::Enabled,
                    },
                )
            })
            .collect(),
    };
    files.push(SourceFile {
        path: "task_definitions.json".into(),
        bytes: serde_json::to_vec(&definitions).unwrap(),
    });
    let source = SourceStore::from_config(common::with_db(&common::test_config(), &db));
    let candidate = source
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: "workstreams/1".into(),
            files,
        })
        .await
        .unwrap();
    source
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            "reviewer",
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    source
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &candidate.proposal_id,
            &SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                parser_version: candidate.parser_version,
                expected_authority_epoch: candidate.base_epoch,
                approved_candidate_digest: candidate.manifest_digest,
            },
        )
        .await
        .unwrap();
}

async fn sample(name: &str, value: &Value) {
    if std::env::var_os("AWR_RECORD_GOAL_SAMPLES").is_some() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/samples")
            .join(name);
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
        )
        .unwrap();
    }
}

async fn business_state(admin: &tokio_postgres::Client) -> Value {
    admin
        .query_one(
            "SELECT jsonb_build_object(
        'projects',(SELECT jsonb_agg(to_jsonb(p)) FROM awr_team.projects p),
        'sessions',(SELECT jsonb_agg(to_jsonb(s)) FROM awr_team.sessions s),
        'runtime',(SELECT jsonb_agg(to_jsonb(r)) FROM awr_team.work_runtime r),
        'claims',(SELECT jsonb_agg(to_jsonb(c)) FROM awr_team.claims c))",
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn goals_are_source_backed_scoped_bounded_and_read_only_on_the_real_service() {
    let (_guard, admin, _db, store) = setup_with_specs(sources()).await;
    prepare_goals(&admin).await;
    let before = business_state(&admin).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = "http://127.0.0.1:7381";
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec![],
        allowed_web_origins: vec![origin.into()],
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
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        http.clone(),
        StreamableHttpClientTransportConfig::with_uri(format!(
            "http://{address}/v1/projects/one/mcp"
        ))
        .auth_header(A),
    );
    let client = ().serve(transport).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    let tool = tools.iter().find(|t| t.name == "awr_team_query").unwrap();
    assert!(
        tool.description
            .as_ref()
            .unwrap()
            .starts_with("Read current Team work")
    );
    for op in ["goals.list", "goal.inspect"] {
        assert!(
            tool.input_schema["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(op))
        );
    }
    let mut replies = Vec::new();
    for (op, selector) in [
        ("goals.list", json!({})),
        ("goal.inspect", json!({"goal_id":"ship"})),
    ] {
        let mut input = json!({"protocol_version":1,"op":op});
        input
            .as_object_mut()
            .unwrap()
            .extend(selector.as_object().unwrap().clone());
        let result = client
            .call_tool(
                CallToolRequestParams::new("awr_team_query")
                    .with_arguments(input.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let value = result.structured_content.unwrap();
        sample(
            if op == "goals.list" {
                "goals-list-mcp.json"
            } else {
                "goal-inspect-mcp.json"
            },
            &value,
        )
        .await;
        let wire = value.to_string();
        for secret in [
            "HIDDEN TASK SENTINEL",
            "HIDDEN GOAL SENTINEL",
            "b-private",
            "SOURCE BODY SENTINEL",
            "PRIVATE NEXT ACTION",
            "secret_hash",
        ] {
            assert!(!wire.contains(secret), "{secret}");
        }
        replies.push(value);
    }
    let list = &replies[0]["data"]["items"];
    let list = list.as_array().unwrap();
    let goal = list
        .iter()
        .find(|g| g["goal_id"] == "ship" && g["kind"] == "goal")
        .unwrap();
    assert_eq!(goal["status"], "confirmed");
    assert_eq!(goal["success_criteria"], json!(["Reviewed delivery"]));
    assert_eq!(goal["departments"], json!([]));
    assert_eq!(goal["department_reason"], "goal_department_not_registered");
    assert_eq!(goal["hidden_task_count"], 1);
    assert_eq!(goal["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(goal["tasks"][0]["title"], "Visible task");
    assert_eq!(goal["uncovered"], false);
    let uncovered = list.iter().find(|g| g["goal_id"] == "alpha").unwrap();
    assert_eq!(uncovered["uncovered"], true);
    assert_eq!(
        list.iter()
            .find(|g| g["goal_id"] == "unknown" && g["kind"] == "goal")
            .unwrap()["uncovered"],
        false
    );
    let reasons: Vec<_> = list
        .iter()
        .filter(|g| g["kind"] == "orphan_work")
        .map(|g| g["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        reasons,
        vec!["archived_goal", "unknown_goal", "cancelled_goal"]
    );
    assert_eq!(replies[1]["data"]["goal"]["status"], "confirmed");
    assert_eq!(replies[1]["data"]["read_only"], true);
    assert_eq!(replies[1]["data"]["execution_authorized"], false);
    let retired = client
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"goal.inspect","goal_id":"retired"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert!(!retired.is_error.unwrap_or(false));
    let retired = retired.structured_content.unwrap();
    assert_eq!(
        retired["data"]["orphan_work"][0]["reason"],
        "cancelled_goal"
    );
    sample("goal-retired-inspect-mcp.json", &retired).await;
    let login = http
        .post(format!("http://{address}/v1/web/login"))
        .header("origin", origin)
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
    let web = |project: &str, input: Value| {
        http.post(format!("http://{address}/v1/web/projects/{project}/query"))
            .header("origin", origin)
            .header("x-awr-web", "1")
            .header("cookie", &cookie)
            .json(&input)
    };
    for (input, name, expected) in [
        (
            json!({"protocol_version":1,"op":"goals.list"}),
            "goals-list.json",
            &replies[0],
        ),
        (
            json!({"protocol_version":1,"op":"goal.inspect","goal_id":"ship"}),
            "goal-inspect.json",
            &replies[1],
        ),
        (
            json!({"protocol_version":1,"op":"goal.inspect","goal_id":"retired"}),
            "goal-retired-inspect.json",
            &retired,
        ),
    ] {
        let response = web("one", input).send().await.unwrap();
        assert_eq!(response.status(), 200);
        let mut body: Value = response.json().await.unwrap();
        sample(name, &body).await;
        body["data"]["as_of"] = expected["data"]["as_of"].clone();
        assert_eq!(&body, expected);
    }
    for op in ["goals.list", "goal.inspect"] {
        let mut input = json!({"protocol_version":1,"op":op});
        if op == "goal.inspect" {
            input["goal_id"] = json!("ship");
        }
        assert_eq!(
            web("foreign", input.clone()).send().await.unwrap().status(),
            403
        );
        input["workstream_id"] = json!(Id::from(2));
        assert_eq!(web("one", input).send().await.unwrap().status(), 403);
    }
    assert_eq!(business_state(&admin).await, before);
    // Reauthorization happens for each actual request on this same connection.
    admin.batch_execute("UPDATE awr_team.workstream_grants SET active=false,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    let result = client
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"goals.list"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await;
    match result {
        Ok(result) => assert!(result.is_error.unwrap_or(false)),
        Err(error) => assert!(error.to_string().contains("HTTP 403 Forbidden"), "{error}"),
    }
    assert_eq!(
        web("one", json!({"protocol_version":1,"op":"goals.list"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    client.cancel().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn goal_pagination_permissions_drift_and_coverage_rules_are_enforced() {
    let (_guard, admin, _db, store) = setup_with_specs(sources()).await;
    prepare_goals(&admin).await;
    let mut q = query("goals.list");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["items"].as_array().unwrap().len(), 1);
    q.cursor = first["data"]["next_cursor"].as_str().map(str::to_owned);
    assert!(q.cursor.is_some());
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_ne!(first["data"]["items"], second["data"]["items"]);
    let mut inspect = query("goal.inspect");
    inspect.goal_id = Some("ship".into());
    inspect.limit = Some(1);
    let one = store
        .query(TENANT, PROJECT, A, inspect.clone())
        .await
        .unwrap();
    assert_eq!(one["data"]["total"], 2);
    inspect.cursor = one["data"]["next_cursor"].as_str().map(str::to_owned);
    let two = store
        .query(TENANT, PROJECT, A, inspect.clone())
        .await
        .unwrap();
    assert_ne!(one["data"]["goal"]["tasks"], two["data"]["goal"]["tasks"]);
    assert!(two["data"]["next_cursor"].is_null());
    for goal in ["private", "missing"] {
        let mut denied = query("goal.inspect");
        denied.goal_id = Some(goal.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, denied).await,
            Err(PgError::Forbidden)
        ));
    }
    let mut denied = query("goals.list");
    denied.workstream_id = Some(Id::from(2));
    assert!(matches!(
        store.query(TENANT, PROJECT, A, denied).await,
        Err(PgError::Forbidden)
    ));
    for op in ["goals.list", "goal.inspect"] {
        let mut denied = query(op);
        if op == "goal.inspect" {
            denied.goal_id = Some("ship".into());
        }
        assert!(
            store
                .query("other-tenant", PROJECT, A, denied.clone())
                .await
                .is_err()
        );
        assert!(
            store
                .query(TENANT, "other-project", A, denied.clone())
                .await
                .is_err()
        );
        assert!(
            store
                .query(TENANT, PROJECT, NONE, denied.clone())
                .await
                .is_err()
        );
        denied.max_context_bytes = Some(1);
        assert!(matches!(
            store.query(TENANT, PROJECT, A, denied).await,
            Err(PgError::ResponseTooLarge)
        ));
    }
    admin
        .batch_execute(
            "UPDATE awr_team.work_contracts SET definition_state='cancelled' WHERE work_id='a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    assert!(matches!(
        store.query(TENANT, PROJECT, A, inspect).await,
        Err(PgError::CursorExpired)
    ));
    let updated = store
        .query(TENANT, PROJECT, A, query("goals.list"))
        .await
        .unwrap();
    assert!(
        updated["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["kind"] != "orphan_work")
    );
    assert_eq!(
        updated["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["goal_id"] == "ship")
            .unwrap()["status"],
        "confirmed"
    );
    // Snapshot bytes are verified, and arbitrary other files never supply goals.
    admin.batch_execute(r#"UPDATE awr_team.source_snapshots SET source_ref_json=jsonb_set(source_ref_json,'{files,1,text}','"tampered"'::jsonb)"#).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("goals.list")).await,
        Err(PgError::SnapshotDrift(_))
    ));
}

#[test]
fn goal_query_selectors_are_closed_and_diagnostics_are_bounded() {
    for value in [
        json!({"op":"goal.inspect"}),
        json!({"op":"goals.list","goal_id":"private"}),
        json!({"op":"goals.list","session_id":"private"}),
        json!({"op":"goals.list","work_id":"private"}),
        json!({"op":"goal.inspect","goal_id":""}),
        json!({"op":"goals.list","limit":101}),
    ] {
        let mut value = value;
        value["protocol_version"] = json!(1);
        let q: awr_team_pg::WorkstreamQuery = serde_json::from_value(value).unwrap();
        let error = q.validate().unwrap_err();
        assert!(error.query_input_diagnostic().is_some());
        assert!(!error.to_string().contains("private"));
    }
}

#[tokio::test]
async fn task_delegation_never_promotes_a_peer_task_or_an_unrelated_goal() {
    use awr_core::*;
    use awr_team_pg::AuthorizationStore;
    use std::collections::BTreeSet;
    let (_guard, admin, db, store) = setup_with_specs(sources()).await;
    prepare_goals(&admin).await;
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES('reader-tenant','reader-project','owner','Owner','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','goal-binding','owner','agent','active')").await.unwrap();
    let authorization = AgentAuthorization {
        id: "goal-authorization".into(),
        authorizer_person_id: PersonId::new("owner").unwrap(),
        responsible_person_id: PersonId::new("owner").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
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
        created_at_ms: 1000,
        binding_id: Some("goal-binding".into()),
    };
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "goal-task-grant".into(),
                authorization,
            },
        )
        .await
        .unwrap();
    let result = store
        .query(TENANT, PROJECT, A, query("goals.list"))
        .await
        .unwrap();
    assert!(!result.to_string().contains("No supporting tasks"));
    let goal = result["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["goal_id"] == "ship")
        .unwrap();
    assert_eq!(goal["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(goal["tasks"][0]["work_id"], "a");
    assert_eq!(goal["hidden_task_count"], 2);
    let mut q = query("goal.inspect");
    q.goal_id = Some("ship".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["total"],
        1
    );
    admin.batch_execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='goal-authorization'").await.unwrap();
    assert!(store.query(TENANT, PROJECT, A, q).await.is_err());
}

#[tokio::test]
async fn cancelled_and_archived_tasks_and_long_task_lists_keep_the_coverage_contract() {
    let (_guard, admin, _db, store) = setup_with_specs(sources()).await;
    prepare_goals(&admin).await;
    // Duplicate only synthetic task contracts/ownership to exercise the preview
    // continuation independently of the outer goal page size.
    admin.batch_execute("INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
        SELECT tenant_id,project_id,'extra-'||n,'extra-'||n FROM awr_team.work_items CROSS JOIN generate_series(1,22) n WHERE id='c';
        INSERT INTO awr_team.work_contracts(tenant_id,project_id,snapshot_id,scope_id,work_id,contract_hash,definition_state,title,contract_json)
        SELECT tenant_id,project_id,snapshot_id,scope_id,'extra-'||n,contract_hash,'enabled','Extra task',
          jsonb_set(jsonb_set(contract_json,'{work_id}',to_jsonb('extra-'||n)),'{external_key}',to_jsonb('extra-'||n))
        FROM awr_team.work_contracts CROSS JOIN generate_series(1,22) n WHERE work_id='c';
        INSERT INTO awr_team.workstream_snapshot_ownership(tenant_id,project_id,snapshot_id,scope_id,work_id,workstream_id,ownership_version)
        SELECT tenant_id,project_id,snapshot_id,scope_id,'extra-'||n,workstream_id,ownership_version
        FROM awr_team.workstream_snapshot_ownership CROSS JOIN generate_series(1,22) n WHERE work_id='c';
        UPDATE awr_team.work_contracts SET definition_state='archived' WHERE work_id='a';").await.unwrap();
    let result = store
        .query(TENANT, PROJECT, A, query("goals.list"))
        .await
        .unwrap();
    let goal = result["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["goal_id"] == "ship")
        .unwrap();
    assert_eq!(goal["tasks"].as_array().unwrap().len(), 20);
    assert_eq!(goal["task_total"], 23);
    let mut q = query("goal.inspect");
    q.goal_id = Some("ship".into());
    q.cursor = goal["tasks_next_cursor"].as_str().map(str::to_owned);
    let continued = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(
        continued["data"]["goal"]["tasks"].as_array().unwrap().len(),
        3
    );
    assert!(!result.to_string().contains("Visible task"));
    assert!(
        result["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["kind"] != "orphan_work")
    );
    admin.batch_execute("UPDATE awr_team.work_contracts SET definition_state='cancelled' WHERE work_id='c' OR work_id LIKE 'extra-%';").await.unwrap();
    let mut q = query("goal.inspect");
    q.goal_id = Some("ship".into());
    q.workstream_id = Some(Id::from(1));
    let cancelled = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(cancelled["data"]["goal"]["uncovered"], true);
    assert_eq!(cancelled["data"]["goal"]["status"], "confirmed");
    assert_eq!(cancelled["data"]["goal"]["tasks"][0]["status"], "cancelled");
    assert_eq!(cancelled["data"]["goal"]["hidden_task_count"], 0);
}

#[tokio::test]
async fn full_project_discovery_includes_unlinked_goals_and_uses_definition_state_for_drafts() {
    let (_guard, admin, _db, store) = setup_with_specs(sources()).await;
    admin.batch_execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES('reader-tenant','reader-project','agent','cli-a','00000000000000000000000002',1,true);
        UPDATE awr_team.work_contracts SET definition_state='draft' WHERE work_id='a';").await.unwrap();
    let mut q = query("goal.inspect");
    q.goal_id = Some("private".into());
    let result = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(result["data"]["goal"]["uncovered"], true);
    assert_eq!(result["data"]["goal"]["tasks"], json!([]));
    let mut q = query("goal.inspect");
    q.goal_id = Some("ship".into());
    let result = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(result["data"]["goal"]["hidden_task_count"], 0);
    assert_eq!(result["data"]["goal"]["tasks"][0]["status"], "draft");
}
