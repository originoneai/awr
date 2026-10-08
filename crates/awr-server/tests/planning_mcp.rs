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
    assert_eq!(
        result.is_error.unwrap_or(false),
        error,
        "{name} request {:?}: {result:?}",
        args.get("request_id")
    );
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
        dependency_acceptance: None,
        hard_rules: None,
        verification_requirements: None,
        execution_settlement: None,
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
#[expect(
    clippy::await_holding_lock,
    reason = "The guard serializes the shared PostgreSQL fixture for this entire async test."
)]
async fn reviewed_workspace_declarations_publish_through_authenticated_mcp_to_work_prepare() {
    use awr_team::{
        DependencyAcceptanceMode as Mode, ExecutionSettlementMode,
        ExecutionSettlementPolicy as Policy, PLANNING_CODEC_V4, PLANNING_CODEC_V5, WorkContract,
    };
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    admin.batch_execute("UPDATE awr_team.project_memberships SET role='maintainer',membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
    let root = std::env::temp_dir().join(format!("awr-planning-workspace-{}", common::nonce(0)));
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let definitions: Vec<_> = [
        ("00000000000000000000000001", "alpha"),
        ("00000000000000000000000002", "private-beta"),
    ]
    .into_iter()
    .map(|(id, key)| {
        json!({
            "id":id,"external_key":key,"title":key,"state":"active",
            "authority_version":1,"goal_keys":[key],"acceptance_contracts":[]
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
            "id":id,"title":id,"status":"planned","workstream":stream,
            "goals":[stream],"acceptance":["verified"],"paths":["src"],"depends_on":deps
        })
    })
    .collect();
    let ledger = root.join("ledger.yaml");
    std::fs::write(&ledger, serde_json::to_vec(&json!({
        "workstreams":{"version":1,"definitions":definitions},
        "goals":[{"id":"alpha","title":"Alpha","status":"active"},{"id":"private-beta","title":"Private","status":"active"}],
        "work_items":works
    })).unwrap()).unwrap();
    let binding =
        json!({"kind":"server_directory","locator":root,"ledger_relative_path":"ledger.yaml"});
    admin.execute("UPDATE awr_team.source_snapshots s SET source_ref_json=jsonb_set(s.source_ref_json,'{sole_source}',$3)
        FROM awr_team.projects p WHERE p.tenant_id=$1 AND p.id=$2 AND s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id",
        &[&TENANT,&PROJECT,&binding]).await.unwrap();
    let server = start(store).await;
    let client = connect(&server, A).await;
    let caps = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
        false,
    )
    .await;
    assert!(
        caps["planning"]["supported_candidate_codecs"]
            .as_array()
            .unwrap()
            .contains(&json!(PLANNING_CODEC_V4))
    );
    assert!(
        caps["planning"]["supported_candidate_codecs"]
            .as_array()
            .unwrap()
            .contains(&json!(PLANNING_CODEC_V5))
    );
    assert_eq!(
        caps["simulated_member_review"]["dependency_acceptance_mode"],
        "simulated_member_independent"
    );
    assert_eq!(
        caps["simulated_member_review"]["cross_workstream_adoption_supported"],
        true
    );
    assert_eq!(
        caps["planning"]["execution_settlement"]["declaration_only"],
        true
    );
    assert_eq!(
        caps["planning"]["execution_settlement"]["removal_supported"],
        false
    );
    let tools = client.list_tools(Default::default()).await.unwrap();
    let draft_tool = tools
        .tools
        .iter()
        .find(|t| t.name == "awr_team_planning_draft")
        .unwrap();
    let guidance = draft_tool.input_schema["properties"]["changes"]["description"]
        .as_str()
        .unwrap();
    assert!(guidance.contains("execution_settlement") && guidance.contains("exact prior"));

    for (key, policy) in [
        ("ORDINARY-1", Policy::COMPLETION_POLICY),
        ("SIMULATED-1", Policy::SIMULATED_MEMBER_COMPLETION_POLICY),
    ] {
        let mut task = draft(key);
        task.workstream = Some("alpha".into());
        task.definition_state = DraftDefinitionState::Enabled;
        task.completion_policy = policy.into();
        task.execution_settlement = Some(Policy {
            mode: ExecutionSettlementMode::IndependentWorkspaceV1,
            workspace_id: format!("workspace-{key}"),
        });
        task.hard_rules = Some(vec!["Preserve recorded history".into()]);
        task.verification_requirements = Some(vec!["Run persistence regressions".into()]);
        let mut omitted = task.clone();
        omitted.execution_settlement = None;
        omitted.hard_rules = None;
        omitted.verification_requirements = None;
        let mut renamed = omitted.clone();
        renamed.title = "Retain the execution contract".into();
        let mut replacement = task.clone();
        replacement
            .execution_settlement
            .as_mut()
            .unwrap()
            .workspace_id = format!("replacement-{key}");
        replacement.verification_requirements = Some(vec!["Verify API and process reload".into()]);
        for (index, change, expected) in [
            (
                0,
                DraftChange {
                    op: DraftOpKind::CreateTask,
                    before: None,
                    after: task.clone(),
                },
                task.clone(),
            ),
            (
                1,
                DraftChange {
                    op: DraftOpKind::EditFields,
                    before: Some(omitted),
                    after: renamed,
                },
                task.clone(),
            ),
            (
                2,
                DraftChange {
                    op: DraftOpKind::EditFields,
                    before: Some(task),
                    after: replacement.clone(),
                },
                replacement,
            ),
        ] {
            let created = call(&client, "awr_team_planning_draft", json!({
                "protocol_version":1,"request_id":format!("{key}-{index}-draft"),"mode":"create",
                "changes":[change.clone()],"allowed_spec_roots":["specs"],"project_goal_keys":["delivery"],
                "self_approve_policy":OrdinaryPlanningSelfApprovePolicy::ordinary_default()
            }), false).await;
            let candidate = &created["result"]["candidate_id"];
            let mut digest = created["result"]["candidate_digest"].clone();
            let preview = call(
                &client,
                "awr_team_planning_preview",
                json!({"protocol_version":1,"candidate_id":candidate}),
                false,
            )
            .await;
            assert_eq!(preview["diff"]["candidate_digest"], digest);
            assert_eq!(
                preview["diff"]["field_diffs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|d| d["field"] == "execution_settlement"),
                index != 1
            );
            call(&client,"awr_team_planning_approve",json!({
                "protocol_version":1,"request_id":format!("{key}-{index}-approve"),"candidate_id":candidate,"candidate_digest":digest
            }),false).await;
            if index == 0 {
                // Editing a reviewed candidate invalidates its approval even over the same authenticated transport.
                let mut edited_change = change;
                edited_change.after.title = "Reviewed execution workspace".into();
                let edited = call(
                    &client,
                    "awr_team_planning_draft",
                    json!({
                        "protocol_version":1,"request_id":format!("{key}-redraft"),"mode":"edit",
                        "candidate_id":candidate,"changes":[edited_change]
                    }),
                    false,
                )
                .await;
                let new_digest = edited["result"]["candidate_digest"].clone();
                assert_ne!(new_digest, digest);
                for (suffix, refused_digest) in
                    [("old", digest.clone()), ("unapproved", new_digest.clone())]
                {
                    call(&client,"awr_team_planning_publish",json!({
                        "protocol_version":1,"request_id":format!("{key}-{suffix}-publish"),"candidate_id":candidate,
                        "candidate_digest":refused_digest,"activate":true,"impact_proven":true
                    }),true).await;
                }
                digest = new_digest;
                call(&client,"awr_team_planning_approve",json!({
                    "protocol_version":1,"request_id":format!("{key}-reapprove"),"candidate_id":candidate,"candidate_digest":digest
                }),false).await;
            }
            let published = call(&client,"awr_team_planning_publish",json!({
                "protocol_version":1,"request_id":format!("{key}-{index}-publish"),"candidate_id":candidate,"candidate_digest":digest,
                "activate":true,"impact_proven":true
            }),false).await;
            assert!(published["result"]["activation"]["activated_snapshot_id"].is_string());
            let prepared = call(
                &client,
                "awr_team_query",
                json!({"protocol_version":1,"op":"work.prepare","work_id":key}),
                false,
            )
            .await;
            assert_eq!(prepared["data"]["context_complete"], true);
            assert_eq!(
                prepared["data"]["published_contract"]["completion_policy"],
                policy
            );
            assert_eq!(
                prepared["data"]["published_contract"]["execution_settlement"],
                json!(expected.execution_settlement)
            );
            assert_eq!(
                prepared["data"]["published_contract"]["verification_requirements"],
                json!(expected.verification_requirements)
            );
            assert_eq!(prepared["data"]["execution_admission"], "not_evaluated");
            assert!(prepared["data"]["runtime"].is_null());
            assert!(
                std::fs::read_to_string(&ledger)
                    .unwrap()
                    .contains(&expected.execution_settlement.as_ref().unwrap().workspace_id)
            );
        }
    }
    let mut dependent = draft("DEPENDENT-1");
    dependent.workstream = Some("alpha".into());
    dependent.definition_state = DraftDefinitionState::Enabled;
    dependent.hard_rules = Some(vec!["Preserve recorded history".into()]);
    dependent.verification_requirements = Some(vec!["Verify the integrated artifact".into()]);
    dependent.required_dependencies = vec!["SIMULATED-1".into()];
    dependent.dependency_acceptance = Some(std::collections::BTreeMap::from([(
        "SIMULATED-1".into(),
        Mode::SimulatedMemberIndependent,
    )]));
    let mut omitted = dependent.clone();
    omitted.dependency_acceptance = None;
    let mut renamed = omitted.clone();
    renamed.title = "Retain the reviewed input".into();
    let mut replacement = dependent.clone();
    replacement.dependency_acceptance.as_mut().unwrap().insert(
        "SIMULATED-1".into(),
        Mode::AgentReviewedCallerAssertedReconciled,
    );
    for (index, change, mode) in [
        (
            0,
            DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: dependent.clone(),
            },
            Mode::SimulatedMemberIndependent,
        ),
        (
            1,
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(omitted),
                after: renamed,
            },
            Mode::SimulatedMemberIndependent,
        ),
        (
            2,
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(dependent),
                after: replacement,
            },
            Mode::AgentReviewedCallerAssertedReconciled,
        ),
    ] {
        let created = call(&client, "awr_team_planning_draft", json!({
            "protocol_version":1,"request_id":format!("dependency-{index}-draft"),"mode":"create",
            "changes":[change],"allowed_spec_roots":["specs"],"project_goal_keys":["delivery"],
            "self_approve_policy":OrdinaryPlanningSelfApprovePolicy::ordinary_default(),
        }), false).await;
        let candidate = &created["result"]["candidate_id"];
        let digest = &created["result"]["candidate_digest"];
        let preview = call(
            &client,
            "awr_team_planning_preview",
            json!({
                "protocol_version":1,"candidate_id":candidate,
            }),
            false,
        )
        .await;
        let policy_diff = preview["diff"]["field_diffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["field"] == "dependency_acceptance");
        assert_eq!(policy_diff, index != 1);
        if policy_diff {
            assert!(
                preview["diff"]["review_requirements"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("explicit_dependency_assurance_review"))
            );
        }
        call(
            &client,
            "awr_team_planning_approve",
            json!({
                "protocol_version":1,"request_id":format!("dependency-{index}-approve"),
                "candidate_id":candidate,"candidate_digest":digest,
            }),
            false,
        )
        .await;
        call(&client, "awr_team_planning_publish", json!({
            "protocol_version":1,"request_id":format!("dependency-{index}-publish"),
            "candidate_id":candidate,"candidate_digest":digest,"activate":true,"impact_proven":true,
        }), false).await;
        let prepared = call(
            &client,
            "awr_team_query",
            json!({
                "protocol_version":1,"op":"work.prepare","work_id":"DEPENDENT-1",
            }),
            false,
        )
        .await;
        assert_eq!(prepared["data"]["context_complete"], true);
        assert_eq!(
            prepared["data"]["published_contract"]["dependency_acceptance"]["SIMULATED-1"],
            json!(mode)
        );
        assert_eq!(
            prepared["data"]["published_contract"]["completion_policy"],
            "independent_review"
        );
        assert_eq!(
            prepared["data"]["published_contract"]["codec"],
            if mode == Mode::SimulatedMemberIndependent {
                WorkContract::CODEC_V5
            } else {
                WorkContract::CODEC_V2
            }
        );
        let written = awr_source::prepare_publish_from_server_directory(
            &root,
            "ledger.yaml",
            PROJECT,
            &Default::default(),
        )
        .unwrap()
        .bundle()
        .unwrap();
        let contract = &written
            .contracts
            .iter()
            .find(|w| w.contract.external_key == "DEPENDENT-1")
            .unwrap()
            .contract;
        assert_eq!(contract.dependency_acceptance["SIMULATED-1"], mode);
    }
    client.cancel().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

async fn member_command(
    client: &Client,
    work: &str,
    request: &str,
    op: &str,
    args: Value,
    error: bool,
) -> Value {
    let prepared = call(
        client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":work}),
        false,
    )
    .await;
    call(
        client,
        "awr_team_command",
        serde_json::to_value(command(&prepared, request, op, args)).unwrap(),
        error,
    )
    .await
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "The guard serializes the shared PostgreSQL fixture for this entire async test."
)]
async fn simulated_dependency_supports_complete_ordinary_member_mcp_workflow() {
    Box::pin(ordinary_member_mcp_workflow(ConsumerWorkflow::SameStream)).await;
}

#[tokio::test]
async fn approved_artifact_exports_work_over_authenticated_mcp_without_upstream_access() {
    Box::pin(ordinary_member_mcp_workflow(ConsumerWorkflow::Disclosure)).await;
}

#[tokio::test]
async fn current_cross_stream_adoption_supports_complete_pool_member_mcp_workflow() {
    Box::pin(ordinary_member_mcp_workflow(
        ConsumerWorkflow::PoolAdoption(awr_core::DeliveryVersionPolicy::CurrentContract),
    ))
    .await;
}

#[tokio::test]
async fn current_cross_stream_adoption_supports_complete_assigned_member_mcp_workflow() {
    Box::pin(ordinary_member_mcp_workflow(
        ConsumerWorkflow::AssignedAdoption(awr_core::DeliveryVersionPolicy::CurrentContract),
    ))
    .await;
}

#[tokio::test]
async fn fixed_cross_stream_adoption_supports_complete_pool_member_mcp_workflow() {
    Box::pin(ordinary_member_mcp_workflow(
        ConsumerWorkflow::PoolAdoption(awr_core::DeliveryVersionPolicy::FixedDelivery),
    ))
    .await;
}

#[tokio::test]
async fn fixed_cross_stream_adoption_supports_complete_assigned_member_mcp_workflow() {
    Box::pin(ordinary_member_mcp_workflow(
        ConsumerWorkflow::AssignedAdoption(awr_core::DeliveryVersionPolicy::FixedDelivery),
    ))
    .await;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsumerWorkflow {
    SameStream,
    Disclosure,
    PoolAdoption(awr_core::DeliveryVersionPolicy),
    AssignedAdoption(awr_core::DeliveryVersionPolicy),
}

#[expect(
    clippy::await_holding_lock,
    reason = "Serial isolated PostgreSQL fixture"
)]
async fn ordinary_member_mcp_workflow(mode: ConsumerWorkflow) {
    let disclose = mode != ConsumerWorkflow::SameStream;
    let adopt = matches!(
        mode,
        ConsumerWorkflow::PoolAdoption(_) | ConsumerWorkflow::AssignedAdoption(_)
    );
    let version_policy = match mode {
        ConsumerWorkflow::PoolAdoption(policy) | ConsumerWorkflow::AssignedAdoption(policy) => {
            policy
        }
        _ => awr_core::DeliveryVersionPolicy::CurrentContract,
    };
    const CONSUMER_AGENT: &str =
        "awr1.consumer-agent.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    use awr_core::{
        AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
        ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
    };
    use awr_team::{
        DependencyAcceptanceMode, ExecutionSettlementMode, ExecutionSettlementPolicy, WorkContract,
    };
    use awr_team_pg::AuthorizationStore;
    use std::collections::BTreeSet;

    // Heap-bound nested fixture futures preserve the ordinary test stack.
    let (_g, admin, db, store) = Box::pin(setup()).await;
    // Provision distinct simulated members before starting the loopback service.
    // Subsequent workflow mutations use authenticated MCP, never direct SQL.
    admin.batch_execute(r#"UPDATE awr_team.actors SET kind='agent' WHERE id IN ('agent','reviewer');
        UPDATE awr_team.project_memberships SET role='developer',agent_review=true,membership_version=membership_version+1;
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
          ('reader-tenant','member-author','agent','Author member','active'),
          ('reader-tenant','member-reviewer','agent','Reviewer member','active'),
          ('reader-tenant','supervisor','human','Supervisor','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
          ('reader-tenant','reader-project','member-author','developer'),
          ('reader-tenant','reader-project','member-reviewer','developer'),
          ('reader-tenant','reader-project','supervisor','admin');
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
          ('reader-tenant','reader-project','member-author','Author','active','{"kind":"simulated_member","controller_ref":"one-controller"}'),
          ('reader-tenant','reader-project','member-reviewer','Reviewer','active','{"kind":"simulated_member","controller_ref":"one-controller"}'),
          ('reader-tenant','reader-project','supervisor','Supervisor','active',NULL);
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
          ('reader-tenant','reader-project','author-binding','member-author','agent','active'),
          ('reader-tenant','reader-project','reviewer-binding','member-reviewer','reviewer','active');
        UPDATE awr_team.credentials SET actor_id='reviewer' WHERE client_id='cli-b';
        UPDATE awr_team.credentials SET actor_id='supervisor' WHERE client_id='unscoped';
        UPDATE awr_team.workstream_grants SET can_write=true WHERE client_id='cli-a';
        UPDATE awr_team.workstream_grants SET actor_id='reviewer' WHERE client_id='cli-b';"#).await.unwrap();
    let stream: String = admin
        .query_one(
            "SELECT workstream_id FROM awr_team.sessions WHERE id='session-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    for (actor, client) in [("reviewer", "cli-b"), ("supervisor", "unscoped")] {
        admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
            VALUES($1,$2,$3,$4,$5,1,true,true)", &[&TENANT,&PROJECT,&actor,&client,&stream]).await.unwrap();
    }
    let authorizations =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut delegated_members = vec![
        ("member-author", "agent", "cli-a", "author-binding"),
        ("member-reviewer", "reviewer", "cli-b", "reviewer-binding"),
    ];
    if adopt {
        admin.batch_execute("INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
            ('reader-tenant','member-consumer','agent','Consumer member','active'),
            ('reader-tenant','consumer-agent','agent','Consumer Agent','active');
            INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
            ('reader-tenant','reader-project','member-consumer','developer'),
            ('reader-tenant','reader-project','consumer-agent','developer');
            INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
            ('reader-tenant','reader-project','member-consumer','Consumer','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"one-controller\"}');
            INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
            ('reader-tenant','reader-project','consumer-binding','member-consumer','consumer-agent','active');
            UPDATE awr_team.project_memberships SET assignment_grant=true,membership_version=membership_version+1 WHERE actor_id='supervisor'").await.unwrap();
        admin.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,'consumer-agent','consumer-agent','consumer-cli',$2)",
            &[&TENANT,&awr_team_pg::workstream_credential_hash(CONSUMER_AGENT).unwrap()]).await.unwrap();
        for (actor, client) in [
            ("consumer-agent", "consumer-cli"),
            ("reviewer", "cli-b"),
            ("supervisor", "unscoped"),
        ] {
            admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
                VALUES($1,$2,$3,$4,$5,1,true,true)
                ON CONFLICT(tenant_id,project_id,actor_id,client_id,workstream_id)
                DO UPDATE SET can_read=true,can_write=true,grant_version=awr_team.workstream_grants.grant_version+1",
                &[&TENANT,&PROJECT,&actor,&client,&awr_core::Id::from(2).to_string()]).await.unwrap();
        }
        delegated_members.push((
            "member-consumer",
            "consumer-agent",
            "consumer-cli",
            "consumer-binding",
        ));
    }
    for (member, actor, client, binding) in delegated_members {
        authorizations
            .issue(
                TENANT,
                PROJECT,
                &IssueAuthorizationRequest {
                    request_key: format!("issue-{member}"),
                    authorization: AgentAuthorization {
                        id: format!("grant-{member}"),
                        authorizer_person_id: PersonId::new("supervisor").unwrap(),
                        responsible_person_id: PersonId::new(member).unwrap(),
                        subject_kind: ExecutionSubjectKind::Agent,
                        subject_id: actor.into(),
                        client_id: client.into(),
                        session_id: None,
                        model_id: None,
                        scope: AuthorizationScope::Project {
                            project_id: PROJECT.into(),
                        },
                        actions: BTreeSet::from([
                            AuthorizedAction::Inspect,
                            AuthorizedAction::StartWork,
                            AuthorizedAction::ClaimCoordination,
                            AuthorizedAction::Review,
                        ]),
                        expires_at_ms: None,
                        status: AuthorizationStatus::Active,
                        revoked_at_ms: None,
                        revoked_by: None,
                        verifiable_capabilities: vec![],
                        self_reported_skill_hints: vec![],
                        parent_authorization_id: None,
                        maintainer_person_id: None,
                        created_at_ms: 1000,
                        binding_id: Some(binding.into()),
                    },
                },
            )
            .await
            .unwrap();
    }
    for (work, dependency, scope) in [("a", None, "src/api"), ("c", Some("a"), "src/integration")] {
        let mut contract: WorkContract = serde_json::from_value(
            admin
                .query_one(
                    "SELECT contract_json FROM awr_team.work_contracts WHERE work_id=$1",
                    &[&work],
                )
                .await
                .unwrap()
                .get(0),
        )
        .unwrap();
        contract.codec = if dependency.is_some() {
            WorkContract::CODEC_V5
        } else {
            WorkContract::CODEC_V4
        }
        .into();
        contract.completion_policy =
            ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY.into();
        contract.execution_settlement = Some(ExecutionSettlementPolicy {
            mode: ExecutionSettlementMode::IndependentWorkspaceV1,
            workspace_id: format!("workspace-{work}"),
        });
        contract.scope_paths = vec![scope.into()];
        contract.required_dependencies = dependency.into_iter().map(Into::into).collect();
        contract.dependency_acceptance = dependency
            .into_iter()
            .map(|id| {
                (
                    id.into(),
                    DependencyAcceptanceMode::SimulatedMemberIndependent,
                )
            })
            .collect();
        admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id=$3",
            &[&json!(contract), &contract.hash().unwrap(), &work]).await.unwrap();
    }
    const EXPORT_READER: &str =
        "awr1.export-reader.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    if disclose {
        admin.batch_execute("INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES('reader-tenant','export-reader','human','Consumer','active');
            INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES('reader-tenant','reader-project','export-reader','reader')").await.unwrap();
        let hash = awr_team_pg::workstream_credential_hash(EXPORT_READER).unwrap();
        admin.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,'export-reader','export-reader','export-reader-cli',$2)", &[&TENANT,&hash]).await.unwrap();
        admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read) VALUES($1,$2,'export-reader','export-reader-cli',$3,1,true)", &[&TENANT,&PROJECT,&awr_core::Id::from(2).to_string()]).await.unwrap();
        let mut consumer: WorkContract = serde_json::from_value(
            admin
                .query_one(
                    "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='b-private'",
                    &[],
                )
                .await
                .unwrap()
                .get(0),
        )
        .unwrap();
        consumer.codec = WorkContract::CODEC_V6.into();
        consumer.required_dependencies = vec!["a".into()];
        consumer.dependency_acceptance.insert(
            "a".into(),
            DependencyAcceptanceMode::CrossWorkstream(awr_team::CrossWorkstreamDependencyPolicy {
                review_assurance:
                    awr_team::CrossWorkstreamReviewAssurance::SimulatedMemberIndependent,
                version_policy,
            }),
        );
        if adopt {
            consumer.completion_policy =
                ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY.into();
            consumer.scope_paths = vec!["src/integration".into()];
            consumer.verification_requirements = vec!["Verify the integrated artifact".into()];
            consumer.execution_settlement = Some(ExecutionSettlementPolicy {
                mode: ExecutionSettlementMode::IndependentWorkspaceV1,
                workspace_id: "workspace-b-private".into(),
            });
        }
        admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='b-private'",&[&json!(consumer),&consumer.hash().unwrap()]).await.unwrap();
    }
    let server = Box::pin(start(store)).await;
    let author = connect(&server, A).await;
    let reviewer = connect(&server, B).await;
    let supervisor = connect(&server, NONE).await;
    let adoption_client = if adopt {
        Some(connect(&server, CONSUMER_AGENT).await)
    } else {
        None
    };
    let cross_session = if let Some(client) = &adoption_client {
        member_command(
            client,
            "b-private",
            "cross-consumer-start",
            "session.start",
            json!({"conversation_id":"cross-consumer"}),
            false,
        )
        .await["receipt"]["data"]["session_id"]
            .clone()
    } else {
        Value::Null
    };
    let mut cross_claim = Value::Null;
    let consumer_session = if disclose {
        Value::Null
    } else {
        let consumer_session = member_command(
            &author,
            "c",
            "consumer-start",
            "session.start",
            json!({"conversation_id":"consumer"}),
            false,
        )
        .await["receipt"]["data"]["session_id"]
            .clone();
        let prepare = call(
            &author,
            "awr_team_query",
            json!({"protocol_version":1,"op":"work.prepare","work_id":"c"}),
            false,
        )
        .await;
        let claim_args = json!({"session_id":consumer_session,"expected_session_version":"1",
        "expected_responsibility_version":prepare["data"]["responsibility"]["version"],"expected_work_version":"0","ttl_seconds":600});
        let next = call(
            &author,
            "awr_team_query",
            json!({"protocol_version":1,"op":"work.next"}),
            false,
        )
        .await;
        assert_eq!(
            next["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["work_id"] == "c")
                .unwrap()["navigation"],
            "waiting_dependency"
        );
        assert_eq!(
            member_command(
                &author,
                "c",
                "blocked-consumer",
                "task.claim_available",
                claim_args,
                true
            )
            .await["code"],
            "Unavailable"
        );
        consumer_session
    };
    let mut upstream = Value::Null;
    let works = match mode {
        ConsumerWorkflow::Disclosure => vec![("a", "src/api")],
        ConsumerWorkflow::SameStream => vec![("a", "src/api"), ("c", "src/integration")],
        _ => vec![("a", "src/api"), ("b-private", "src/integration")],
    };
    for (work, scope) in works {
        let worker = if work == "b-private" {
            adoption_client.as_ref().unwrap()
        } else {
            &author
        };
        use sha2::Digest;
        let artifact = format!("Synthetic artifact {work}");
        let output: String = sha2::Sha256::digest(artifact.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let session = if work == "c" {
            consumer_session.clone()
        } else if work == "b-private" {
            cross_session.clone()
        } else {
            member_command(
                worker,
                work,
                "author-start",
                "session.start",
                json!({"conversation_id":work}),
                false,
            )
            .await["receipt"]["data"]["session_id"]
                .clone()
        };
        let mut sessions = Vec::new();
        for (client, suffix) in [(&reviewer, "reviewer"), (&supervisor, "supervisor")] {
            sessions.push(
                member_command(
                    client,
                    work,
                    &format!("{work}-{suffix}-start"),
                    "session.start",
                    json!({"conversation_id":format!("{work}-{suffix}")}),
                    false,
                )
                .await["receipt"]["data"]["session_id"]
                    .clone(),
            );
        }
        let p = call(
            worker,
            "awr_team_query",
            json!({"protocol_version":1,"op":"work.prepare","work_id":work}),
            false,
        )
        .await;
        assert_eq!(p["data"]["context_complete"], true);
        let claim = if work == "b-private" {
            cross_claim.clone()
        } else {
            member_command(worker,work,&format!("{work}-claim"),"task.claim_available",json!({
                "session_id":session,"expected_session_version":"1","expected_responsibility_version":p["data"]["responsibility"]["version"],
                "expected_work_version":"0","ttl_seconds":600,
            }),false).await["receipt"]["data"].clone()
        };
        assert_eq!(
            claim["responsibility"]["owner"],
            if work == "b-private" {
                "member-consumer"
            } else {
                "member-author"
            }
        );
        let p = call(
            worker,
            "awr_team_query",
            json!({"protocol_version":1,"op":"work.prepare","work_id":work}),
            false,
        )
        .await;
        let intent = member_command(worker,work,&format!("{work}-intent"),"execution.prepare",json!({
            "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],
            "expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
            "input_digest":"a".repeat(64),"declared_scope":[scope],
        }),false).await["receipt"]["data"].clone();
        let p = call(
            worker,
            "awr_team_query",
            json!({"protocol_version":1,"op":"work.prepare","work_id":work}),
            false,
        )
        .await;
        let started = member_command(worker,work,&format!("{work}-execute"),"execution.start",json!({
            "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],
            "expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
            "execution_id":intent["execution_id"],"expected_execution_version":intent["execution_version"],"execution_mode":"caller_managed",
        }),false).await["receipt"]["data"].clone();
        let reported = member_command(worker,work,&format!("{work}-report"),"execution.report",json!({
            "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],
            "expected_execution_version":started["execution_version"],"outcome":"succeeded","output_digest":output,
            "observed_paths":[format!("{scope}/result.json")],"note":"Observed the synthetic artifact",
            "workspace_settlement":{"workspace_id":format!("workspace-{work}"),"input_digest":"a".repeat(64),"environment_digest":"c".repeat(64),
                "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
                "executor_stopped":true,"no_external_effects":true},
        }),false).await["receipt"]["data"].clone();
        assert_eq!(reported["state"], "succeeded");
        let evidence = member_command(worker,work,&format!("{work}-evidence"),"evidence.submit",json!({
            "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],"input_digest":"a".repeat(64),
            "dirty_tree":false,"artifact_text":artifact,"payload":{"passed":true,"output_digest":output},
        }),false).await["receipt"]["data"].clone();
        let round = member_command(&supervisor,work,&format!("{work}-open"),"review.open",json!({
            "session_id":sessions[1],"expected_session_version":"1","evidence_id":evidence["evidence_id"],
        }),false).await["receipt"]["data"].clone();
        member_command(&reviewer,work,&format!("{work}-review"),"review.decide",json!({
            "session_id":sessions[0],"expected_session_version":"1","round_id":round["round_id"],
            "decision":"approve","reason":"Reviewed the exact synthetic artifact",
        }),false).await;
        let finalized = member_command(&supervisor,work,&format!("{work}-finalize"),"delivery.finalize",json!({
            "session_id":sessions[1],"expected_session_version":"1","evidence_id":evidence["evidence_id"],"context_complete":true,
        }),false).await["receipt"]["data"].clone();
        assert_eq!(
            finalized["independence_kind"],
            "simulated_member_independent"
        );
        assert_eq!(finalized["human_approval"], false);
        assert_eq!(finalized["team_independent_acceptance"], false);
        assert_eq!(
            finalized["execution_basis"],
            "caller_asserted_workspace_settled"
        );
        if disclose && work == "a" {
            let consumer = connect(&server, EXPORT_READER).await;
            let caps = call(
                &consumer,
                "awr_team_query",
                json!({"protocol_version":1,"op":"capabilities"}),
                false,
            )
            .await;
            assert_eq!(
                caps["approved_artifact_exports"]["adoption_available"],
                true
            );
            assert_eq!(
                caps["planning"]["cross_workstream_policy"]["adoption_version_policies"],
                json!(["current_contract", "fixed_delivery"])
            );
            assert_eq!(
                caps["approved_artifact_exports"]["historical_fixed_delivery_available"],
                true
            );
            let tools = consumer.list_all_tools().await.unwrap();
            assert_eq!(
                tools
                    .iter()
                    .find(|t| t.name == "awr_team_query")
                    .unwrap()
                    .input_schema["properties"]["export_id"]["type"],
                "string"
            );
            let cp = call(
                &consumer,
                "awr_team_query",
                json!({"protocol_version":1,"op":"work.prepare","work_id":"b-private"}),
                false,
            )
            .await;
            let publish_args = json!({"session_id":sessions[1],"expected_session_version":"1","consumer_work_id":"b-private",
                "expected_consumer_contract_hash":cp["data"]["contract_hash"],"expected_consumer_ownership_version":cp["data"]["ownership_version"],
                "receipt_id":finalized["receipt_id"],"expected_artifact_sha256":evidence["artifact_digest"]});
            let mut author_args = publish_args.clone();
            author_args["session_id"] = session.clone();
            assert_eq!(
                member_command(
                    worker,
                    "a",
                    "export-no-finalize-delegation",
                    "delivery.export.publish",
                    author_args,
                    true
                )
                .await["code"],
                "Forbidden"
            );
            let p = call(
                &supervisor,
                "awr_team_query",
                json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
                false,
            )
            .await;
            let request = serde_json::to_value(command(
                &p,
                "publish-approved-artifact",
                "delivery.export.publish",
                publish_args,
            ))
            .unwrap();
            let exported = call(&supervisor, "awr_team_command", request.clone(), false).await;
            let found = call(
                &consumer,
                "awr_team_query",
                json!({"protocol_version":1,"op":"delivery.exports","work_id":"b-private"}),
                false,
            )
            .await;
            assert_eq!(found["data"]["items"][0]["available"], true);
            assert_eq!(found["data"]["items"][0]["review"]["human_approval"], false);
            let args = json!({"protocol_version":1,"op":"artifact.content","work_id":"b-private",
                "export_id":exported["receipt"]["data"]["export_id"],"expected_sha256":evidence["artifact_digest"]});
            let result = call(&consumer, "awr_team_query", args.clone(), false).await;
            assert_eq!(result["data"]["text"], artifact);
            assert_eq!(result["data"]["execution_authorized"], false);
            for key in ["member_origins_json", "open_loops", "session_id"] {
                assert!(!result["data"].to_string().contains(key));
            }
            assert_eq!(
                call(
                    &consumer,
                    "awr_team_query",
                    json!({"protocol_version":1,"op":"work.snapshot","work_id":"a"}),
                    true
                )
                .await["code"],
                "Forbidden"
            );
            assert_eq!(call(&consumer,"awr_team_query",json!({"protocol_version":1,"op":"artifact.content","work_id":"b-private","artifact_id":evidence["artifact_id"]}),true).await["code"],"Forbidden");
            let observed=call(&supervisor,"awr_team_query",json!({"protocol_version":1,"op":"command.inspect","work_id":"a","request_id":"publish-approved-artifact"}),false).await;
            assert_eq!(observed["data"]["state"], "committed");
            assert_eq!(observed["data"]["receipt"], exported["receipt"]);
            if let Some(client) = &adoption_client {
                let assigned = matches!(mode, ConsumerWorkflow::AssignedAdoption(_));
                let mut p = call(
                    client,
                    "awr_team_query",
                    json!({"protocol_version":1,"op":"work.prepare","work_id":"b-private"}),
                    false,
                )
                .await;
                if assigned {
                    member_command(&supervisor,"b-private","cross-dispatch","task.assign",json!({
                        "assignee_person_id":"member-consumer","expected_responsibility_version":p["data"]["responsibility"]["version"],
                    }),false).await;
                    p = call(
                        client,
                        "awr_team_query",
                        json!({"protocol_version":1,"op":"work.prepare","work_id":"b-private"}),
                        false,
                    )
                    .await;
                }
                let mut take = json!({"session_id":cross_session,"expected_session_version":"1",
                    "expected_responsibility_version":p["data"]["responsibility"]["version"],"expected_work_version":"0","ttl_seconds":600});
                let op = if assigned {
                    take["assignment_request_key"] =
                        p["data"]["responsibility"]["pending"]["transfer_request_key"].clone();
                    "task.accept_assignment"
                } else {
                    "task.claim_available"
                };
                assert_eq!(
                    member_command(
                        client,
                        "b-private",
                        "cross-blocked-take",
                        op,
                        take.clone(),
                        true
                    )
                    .await["code"],
                    "Unavailable"
                );
                let adopt_args = json!({"session_id":cross_session,"expected_session_version":"1",
                    "expected_responsibility_version":p["data"]["responsibility"]["version"],
                    "export_id":exported["receipt"]["data"]["export_id"],"expected_export_version":exported["receipt"]["data"]["export_version"],
                    "expected_disclosure_sha256":exported["receipt"]["data"]["disclosure_sha256"],
                    "expected_adoption_version":p["data"]["adopted_dependencies"][0]["adoption_version"]});
                assert_eq!(
                    member_command(
                        &consumer,
                        "b-private",
                        "reader-cannot-adopt",
                        "delivery.adopt",
                        adopt_args.clone(),
                        true
                    )
                    .await["code"],
                    "Forbidden"
                );
                let request = serde_json::to_value(command(
                    &p,
                    "adopt-approved-artifact",
                    "delivery.adopt",
                    adopt_args,
                ))
                .unwrap();
                let adopted = call(client, "awr_team_command", request.clone(), false).await;
                assert_eq!(adopted["receipt"]["data"]["adopted"], true);
                assert_eq!(adopted["execution_authorized"], false);
                let inspected = call(client,"awr_team_query",json!({"protocol_version":1,"op":"command.inspect","work_id":"b-private","request_id":"adopt-approved-artifact"}),false).await;
                assert_eq!(inspected["data"]["receipt"], adopted["receipt"]);
                let replay = call(client, "awr_team_command", request, false).await;
                assert_eq!(replay["receipt"], adopted["receipt"]);
                assert_eq!(replay["execution_authorized"], false);
                let ready = call(
                    client,
                    "awr_team_query",
                    json!({"protocol_version":1,"op":"work.prepare","work_id":"b-private"}),
                    false,
                )
                .await;
                assert_eq!(ready["data"]["context_complete"], true);
                assert_eq!(
                    ready["data"]["adopted_dependencies"][0]["receipt_id"],
                    finalized["receipt_id"]
                );
                assert_eq!(
                    call(client, "awr_team_query", args.clone(), false).await["data"]["adopted"],
                    true
                );
                assert_eq!(
                    call(
                        client,
                        "awr_team_query",
                        json!({"protocol_version":1,"op":"work.snapshot","work_id":"a"}),
                        true
                    )
                    .await["code"],
                    "Forbidden"
                );
                cross_claim = member_command(client, "b-private", "cross-take", op, take, false)
                    .await["receipt"]["data"]
                    .clone();
            }
            if !adopt {
                let revoke=member_command(&supervisor,"a","revoke-approved-artifact","delivery.export.revoke",json!({"session_id":sessions[1],"expected_session_version":"1",
                "export_id":exported["receipt"]["data"]["export_id"],"expected_export_version":"1"}),false).await;
                assert_eq!(revoke["receipt"]["data"]["status"], "revoked");
                let replay = call(&supervisor, "awr_team_command", request, false).await;
                assert_eq!(replay["receipt"], exported["receipt"]);
                assert_eq!(replay["execution_authorized"], false);
                assert_eq!(
                    call(&consumer, "awr_team_query", args, true).await["code"],
                    "Forbidden"
                );
            }
            consumer.cancel().await.unwrap();
        }
        if work == "a" {
            upstream = finalized["receipt_id"].clone();
        } else {
            let links = admin.query("SELECT predecessor_work_id,predecessor_completion_id FROM awr_team.completion_dependencies WHERE completion_id=$1",
                &[&finalized["receipt_id"].as_str().unwrap()]).await.unwrap();
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].get::<_, String>(0), "a");
            assert_eq!(links[0].get::<_, String>(1), upstream.as_str().unwrap());
        }
    }
    if let Some(client) = adoption_client {
        client.cancel().await.unwrap();
    }
    author.cancel().await.unwrap();
    reviewer.cancel().await.unwrap();
    supervisor.cancel().await.unwrap();
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "The guard serializes the shared PostgreSQL fixture for this entire async test."
)]
async fn workspace_declaration_mcp_refuses_malformed_contracts_and_unprivileged_planners() {
    use awr_team::{ExecutionSettlementMode, ExecutionSettlementPolicy as Policy};
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    admin.batch_execute("UPDATE awr_team.project_memberships SET role='maintainer',membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
    let server = start(store).await;
    let planner = connect(&server, A).await;
    let employee = connect(&server, B).await;
    let mut task = draft("SIMULATED-1");
    task.workstream = Some("alpha".into());
    task.completion_policy = Policy::SIMULATED_MEMBER_COMPLETION_POLICY.into();
    task.execution_settlement = Some(Policy {
        mode: ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "workspace-a".into(),
    });
    task.verification_requirements = Some(vec!["Run persistence regressions".into()]);
    let change = json!(DraftChange {
        op: DraftOpKind::CreateTask,
        before: None,
        after: task
    });
    let body = json!({"protocol_version":1,"request_id":"workspace-employee-draft","mode":"create","changes":[change],
        "allowed_spec_roots":["specs"],"project_goal_keys":["delivery"],"self_approve_policy":OrdinaryPlanningSelfApprovePolicy::ordinary_default()});
    call(&employee, "awr_team_planning_draft", body.clone(), true).await;
    for (index, invalid) in [
        json!(null), json!({"mode":"unknown","workspace_id":"workspace-a"}),
        json!({"mode":"independent_workspace_v1","workspace_id":"/tmp/source"}),
        json!({"mode":"independent_workspace_v1","workspace_id":"workspace-a","review_authorized":true}),
    ].into_iter().enumerate() {
        let mut malformed = body.clone();
        malformed["request_id"] = json!(format!("workspace-invalid-{index}"));
        malformed["changes"][0]["after"]["execution_settlement"] = invalid;
        call(&planner,"awr_team_planning_draft",malformed,true).await;
    }
    for (index, missing) in [
        "execution_settlement",
        "scope_paths",
        "verification_requirements",
    ]
    .into_iter()
    .enumerate()
    {
        let mut incomplete = body.clone();
        incomplete["request_id"] = json!(format!("workspace-incomplete-{index}"));
        if missing == "scope_paths" {
            incomplete["changes"][0]["after"][missing] = json!([]);
        } else {
            incomplete["changes"][0]["after"]
                .as_object_mut()
                .unwrap()
                .remove(missing);
        }
        call(&planner, "awr_team_planning_draft", incomplete, true).await;
    }
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.planning_candidates", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
    planner.cancel().await.unwrap();
    employee.cancel().await.unwrap();
}

#[tokio::test]
async fn storage_failure_http_mcp_and_original_request_recovery() {
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let root = std::env::temp_dir().join(format!("awr-planning-storage-{}", common::nonce(0)));
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
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
    // The guarded writer uses a unique temporary name. Make the actual source
    // read-only to exercise its recoverable replacement failure on every host.
    let permissions = std::fs::metadata(&ledger).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&ledger, readonly).unwrap();
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
    std::fs::set_permissions(&ledger, permissions).unwrap();
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
