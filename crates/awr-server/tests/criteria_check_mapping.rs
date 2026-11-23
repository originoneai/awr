#![cfg(feature = "pg-tests")]
//! End-to-end real-server + PostgreSQL verification for AWR-PPD-040:
//! acceptance criteria ↔ examples ↔ Team verification checks and `work.snapshot`
//! `acceptance_evidence` evaluation.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;

use awr_core::ExampleKind;
use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team::delivery::*;
use awr_team::{
    ContractExample, DraftDefinitionState, OrdinaryPlanningSelfApprovePolicy, PLANNING_CODEC_V7,
    TaskDraft,
};
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
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;

type McpClient = RunningService<RoleClient, ()>;

const REVIEWER_TOKEN: &str =
    "awr1.reviewer-1.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

const CRITERION_BOUND: &str = "Filter matches status and keyword without mutating input";
const CRITERION_UNBOUND: &str = "Empty filter query returns all visible records";

struct Server {
    base: String,
    projects_url: String,
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
        projects: vec![
            ProjectBinding {
                key: "one".into(),
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
            },
            ProjectBinding {
                key: "other".into(),
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
        projects_url: format!("http://{address}/v1/projects"),
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

async fn connect(server: &Server, token: &str) -> McpClient {
    let transport = StreamableHttpClientTransport::with_client(
        http(),
        StreamableHttpClientTransportConfig::with_uri(format!("{}/one/mcp", server.projects_url))
            .auth_header(token),
    );
    tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .unwrap()
        .unwrap()
}

async fn call(client: &McpClient, name: &str, args: Value, error: bool) -> Value {
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
        "{name} op={:?} req={:?}: {result:?}",
        args.get("op"),
        args.get("request_id")
    );
    result.structured_content.unwrap()
}

async fn post_project(
    server: &Server,
    alias: &str,
    token: Option<&str>,
    path: &str,
    body: &Value,
) -> (reqwest::StatusCode, Value) {
    let mut req = http().post(format!("{}/{alias}/{path}", server.projects_url));
    if let Some(tok) = token {
        req = req.bearer_auth(tok);
    }
    let response = req.json(body).send().await.unwrap();
    (response.status(), response.json().await.unwrap())
}

fn neutral(p: &Value, key: &str, op: &str, mut args: Value) -> Value {
    args["source_snapshot_id"] = p["source_snapshot_id"].clone();
    serde_json::to_value(command(p, key, op, args)).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn make_candidate(
    p: &Value,
    version: &str,
    required_checks: Vec<&str>,
    artifact_body: &[u8],
) -> DeliveryCandidate {
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: format!("report-v{version}"),
            sha256: sha256_hex(artifact_body),
            byte_length: artifact_body.len().to_string(),
            locator: format!("fixture://artifact/report-v{version}"),
        }],
    };
    serde_json::from_value(json!({
        "binding": {
            "tenant_id": TENANT,
            "project_id": PROJECT,
            "scope_id": "main",
            "workstream_id": p["workstream_id"],
            "work_id": "a",
            "candidate_id": format!("cand-a-v{version}"),
            "candidate_version": version,
            "contract_hash": p["data"]["contract_hash"],
            "manifest_digest": manifest.digest().unwrap(),
            "source_revision": null,
            "required_checks": required_checks,
            "target": {
                "resource": "fixture://ppd040-artifacts",
                "reference": null,
                "precondition": {"kind": "missing"}
            }
        },
        "manifest": manifest
    }))
    .unwrap()
}

fn verification_envelope(
    binding: &CandidateBinding,
    run_id: &str,
    check: &str,
    outcome: VerificationOutcome,
) -> DeliveryEnvelope {
    let result_artifact = (outcome == VerificationOutcome::Passed).then(|| ArtifactEntry {
        artifact_id: format!("check-{run_id}"),
        sha256: sha256_hex(format!("passed:{run_id}:{check}").as_bytes()),
        byte_length: "16".into(),
        locator: format!("fixture://checks/{run_id}"),
    });
    DeliveryEnvelope {
        protocol: DELIVERY_PROTOCOL.into(),
        protocol_version: DELIVERY_PROTOCOL_VERSION,
        record: DeliveryRecord::Verification(VerificationRun {
            binding: binding.clone(),
            run_id: run_id.into(),
            check: check.into(),
            outcome,
            result_artifact,
            provenance: FactProvenance {
                source: FactSource::CallerDeclared,
                reference: format!("fixture://checks/{run_id}"),
                observed_at_unix_ms: Some(1_793_534_400_000),
                recorded_at_unix_ms: 1_793_534_400_100,
            },
        }),
    }
}

fn save_verbatim(name: &str, body: &Value) {
    if std::env::var("AWR_SAVE_VERBATIM_SAMPLES").as_deref() != Ok("1") {
        return;
    }
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/observer/test/fixtures/ppd040");
    std::fs::create_dir_all(&dir).unwrap();
    let pretty = serde_json::to_string_pretty(body).unwrap();
    for banned in ["awr1.", "secret", "password"] {
        assert!(
            !pretty.to_lowercase().contains(banned),
            "{banned} must never appear in verbatim sample {name}"
        );
    }
    std::fs::write(dir.join(name), format!("{pretty}\n")).unwrap();
}

#[tokio::test]
async fn criteria_examples_planning_publish_and_snapshot_evidence_lifecycle() {
    let (_guard, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    // Allow agent to plan/manage and give independent reviewer a credential + stream grant.
    let reviewer_hash = workstream_credential_hash(REVIEWER_TOKEN).unwrap();
    admin
        .batch_execute(&format!(
            "UPDATE awr_team.workstream_grants SET can_manage=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             UPDATE awr_team.project_memberships SET role='developer', independent_review=true, membership_version=membership_version+1
             WHERE actor_id='reviewer';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
               ('{TENANT}','{PROJECT}','person-author','Author','active'),
               ('{TENANT}','{PROJECT}','reviewer','Reviewer','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
               ('{TENANT}','{PROJECT}','bind-agent','person-author','agent','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
             VALUES ('{TENANT}','reviewer-1','reviewer','cli-reviewer','{reviewer_hash}');
             INSERT INTO awr_team.workstream_grants(
                tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage,grant_version)
             VALUES ('{TENANT}','{PROJECT}','reviewer','cli-reviewer','00000000000000000000000001',1,true,true,false,1);"
        ))
        .await
        .unwrap();

    // Register a disposable server_directory sole source matching the active snapshot.
    let tmp_dir = std::env::temp_dir().join(format!(
        "awr-ppd040-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let tmp_dir = std::fs::canonicalize(&tmp_dir).unwrap();
    let definitions = json!([
        {"id":"00000000000000000000000001","external_key":"alpha","title":"alpha","state":"active","authority_version":1,"goal_keys":["alpha"],"acceptance_contracts":[]},
        {"id":"00000000000000000000000002","external_key":"private-beta","title":"private-beta","state":"active","authority_version":1,"goal_keys":["private-beta"],"acceptance_contracts":[]}
    ]);
    let ledger_json = json!({
        "workstreams": {"version": 1, "definitions": definitions},
        "goals": [
            {"id":"alpha","title":"Alpha","status":"active"},
            {"id":"private-beta","title":"Private","status":"active"}
        ],
        "work_items": [
            {"id":"a","title":"a","status":"planned","workstream":"alpha","goals":["alpha"],"acceptance":["verified"],"paths":["src"],"depends_on":[],"completion_policy":"independent_review"},
            {"id":"b-private","title":"b-private","status":"planned","workstream":"private-beta","goals":["private-beta"],"acceptance":["verified"],"paths":["src"],"depends_on":[],"completion_policy":"independent_review"},
            {"id":"c","title":"c","status":"planned","workstream":"alpha","goals":["alpha"],"acceptance":["verified"],"paths":["src"],"depends_on":["b-private"],"completion_policy":"independent_review"}
        ]
    });
    std::fs::write(
        tmp_dir.join("ledger.yaml"),
        serde_json::to_vec(&ledger_json).unwrap(),
    )
    .unwrap();
    let binding = json!({
        "kind": "server_directory",
        "locator": tmp_dir.to_string_lossy(),
        "ledger_relative_path": "ledger.yaml"
    });
    admin
        .execute(
            "UPDATE awr_team.source_snapshots s
             SET source_ref_json=jsonb_set(s.source_ref_json,'{sole_source}',$3)
             FROM awr_team.projects p
             WHERE p.tenant_id=$1 AND p.id=$2
               AND s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id",
            &[&TENANT, &PROJECT, &binding],
        )
        .await
        .unwrap();

    let server = start(store).await;
    let client = connect(&server, A).await;
    let reviewer = connect(&server, REVIEWER_TOKEN).await;

    // 0. Capabilities advertise planning V7.
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
            .contains(&json!(PLANNING_CODEC_V7))
    );

    // 1. Legacy V1 task (no examples in contract): acceptance_evidence is status="not_provided", items=null.
    let (legacy_status, legacy_snap) = post_project(
        &server,
        "one",
        Some(A),
        "query",
        &json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
    )
    .await;
    assert_eq!(legacy_status, reqwest::StatusCode::OK, "{legacy_snap}");
    assert_eq!(
        legacy_snap["data"]["acceptance_evidence"]["status"],
        "not_provided"
    );
    assert!(
        legacy_snap["data"]["acceptance_evidence"]["items"].is_null(),
        "legacy task must report items=null, not an empty list"
    );
    save_verbatim("work-snapshot-legacy.json", &legacy_snap);

    // 2. Planning draft adds V7 examples (positive + negative with check_id on CRITERION_BOUND;
    //    positive without check_id on CRITERION_UNBOUND), then preview -> approve -> publish & activate.
    let before_a = TaskDraft {
        work_id: "a".into(),
        external_key: "a".into(),
        title: "a".into(),
        goals: vec!["alpha".into()],
        scope_paths: vec!["src".into()],
        acceptance: vec!["verified".into()],
        required_dependencies: vec![],
        completion_policy: "independent_review".into(),
        dependency_acceptance: None,
        hard_rules: None,
        verification_requirements: None,
        execution_settlement: None,
        examples: None,
        definition_state: DraftDefinitionState::Enabled,
        workstream: Some("alpha".into()),
        split_from: None,
        split_children: vec![],
    };
    let examples_v1 = vec![
        ContractExample {
            id: Some("filter-pos".into()),
            criterion: CRITERION_BOUND.into(),
            kind: ExampleKind::Positive,
            given: "issues exist with status open".into(),
            when: "filtering by status open and keyword".into(),
            then: "matching issues return and originals stay unchanged".into(),
            check_id: Some("unit.filter.pos".into()),
        },
        ContractExample {
            id: Some("filter-neg".into()),
            criterion: CRITERION_BOUND.into(),
            kind: ExampleKind::Negative,
            given: "issues exist with mismatched keyword".into(),
            when: "filtering with unmatched keyword".into(),
            then: "empty list returns without throwing".into(),
            check_id: Some("unit.filter.neg".into()),
        },
        ContractExample {
            id: Some("empty-query".into()),
            criterion: CRITERION_UNBOUND.into(),
            kind: ExampleKind::Positive,
            given: "visible issues exist".into(),
            when: "query parameters are blank".into(),
            then: "all visible issues return".into(),
            check_id: None,
        },
    ];
    let after_a = TaskDraft {
        acceptance: vec![CRITERION_BOUND.into(), CRITERION_UNBOUND.into()],
        verification_requirements: Some(vec!["unit.filter.pos".into(), "unit.filter.neg".into()]),
        examples: Some(examples_v1.clone()),
        ..before_a.clone()
    };
    let drafted = call(
        &client,
        "awr_team_planning_draft",
        json!({
            "protocol_version": 1,
            "request_id": "ppd040-draft-1",
            "mode": "create",
            "changes": [{"op": "edit_fields", "before": before_a, "after": after_a}],
            "allowed_spec_roots": ["src"],
            "project_goal_keys": ["alpha"],
            "self_approve_policy": OrdinaryPlanningSelfApprovePolicy::ordinary_default()
        }),
        false,
    )
    .await;
    let cand_id = drafted["result"]["candidate_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cand_digest = drafted["result"]["candidate_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let preview = call(
        &client,
        "awr_team_planning_preview",
        json!({"protocol_version":1,"candidate_id":cand_id}),
        false,
    )
    .await;
    assert!(
        preview["diff"]["field_diffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["field"] == "examples"),
        "planning preview must surface examples diff: {preview}"
    );
    call(
        &client,
        "awr_team_planning_approve",
        json!({
            "protocol_version": 1,
            "request_id": "ppd040-approve-1",
            "candidate_id": cand_id,
            "candidate_digest": cand_digest
        }),
        false,
    )
    .await;
    let published = call(
        &client,
        "awr_team_planning_publish",
        json!({
            "protocol_version": 1,
            "request_id": "ppd040-publish-1",
            "candidate_id": cand_id,
            "candidate_digest": cand_digest,
            "activate": true,
            "impact_proven": true
        }),
        false,
    )
    .await;
    assert!(
        published["result"]["activation"]["activated_snapshot_id"].is_string(),
        "V7 planning candidate must activate: {published}"
    );

    // 3. Before any work starts: bound criterion is not_started, unbound criterion is no_bound_check,
    //    and work.snapshot preserves work.prepare context_hash.
    let prepared = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let snap_not_started = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
        false,
    )
    .await;
    assert_eq!(
        snap_not_started["data"]["context_hash"], prepared["data"]["context_hash"],
        "work.snapshot must preserve work.prepare context_hash"
    );
    assert_eq!(
        snap_not_started["data"]["acceptance_evidence"]["status"],
        "provided"
    );
    assert_eq!(
        snap_not_started["data"]["acceptance_evidence"]["items"][0]["status"],
        "not_started"
    );
    assert_eq!(
        snap_not_started["data"]["acceptance_evidence"]["items"][1]["status"],
        "no_bound_check"
    );

    // 4. Claim, configure connector, select candidate v1 -> bound criterion is waiting before checks.
    let claim = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(
            &prepared,
            "ppd040-claim-1",
            "task.claim_available",
            json!({
                "session_id": "session-a",
                "expected_session_version": "1",
                "expected_work_version": "0",
                "expected_responsibility_version": prepared["data"]["responsibility"]["version"],
                "ttl_seconds": 3600
            }),
        ))
        .unwrap(),
        false,
    )
    .await["receipt"]["data"]
        .clone();

    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let body_v1 = b"candidate v1 report";
    let cand_v1 = make_candidate(&p, "1", vec!["unit.filter.pos", "unit.filter.neg"], body_v1);
    let digest_v1 = cand_v1.binding.digest().unwrap();
    call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-select-v1",
            "delivery.candidate.select",
            json!({
                "expected_selected_digest": null,
                "session_id": "session-a",
                "claim_id": claim["claim_id"],
                "fence": claim["fence"],
                "lease_version": claim["lease_version"],
                "candidate": cand_v1
            }),
        ),
        false,
    )
    .await;
    call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-conn-1",
            "delivery.connector.configure",
            json!({
                "expected_connector_version": "0",
                "mapping": {
                    "connector_id": "ci-connector",
                    "provider": "reference",
                    "resource": "fixture://ppd040-artifacts",
                    "principal_actor_id": "agent",
                    "principal_client_id": "cli-a",
                    "fact_source": "caller_declared",
                    "enabled": true
                }
            }),
        ),
        false,
    )
    .await;

    let snap_waiting_checks = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
        false,
    )
    .await;
    assert_eq!(
        snap_waiting_checks["data"]["acceptance_evidence"]["items"][0]["status"],
        "waiting"
    );
    assert_eq!(
        snap_waiting_checks["data"]["acceptance_evidence"]["items"][1]["status"],
        "no_bound_check"
    );

    // 5. Ingest one passing + one failing check on v1 -> status becomes failed.
    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let res_v1_fail = call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-reserve-v1-fail",
            "delivery.inspection.reserve",
            json!({
                "connector_id": "ci-connector",
                "connector_version": "1",
                "candidate_digest": digest_v1,
                "lease_seconds": 60
            }),
        ),
        false,
    )
    .await;
    call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-ingest-v1-fail",
            "delivery.facts.ingest",
            json!({
                "connector_id": "ci-connector",
                "inspection_id": res_v1_fail["receipt"]["data"]["inspection_id"],
                "event_id": "evt-v1-fail",
                "records": [
                    verification_envelope(&cand_v1.binding, "run-v1-pos", "unit.filter.pos", VerificationOutcome::Passed),
                    verification_envelope(&cand_v1.binding, "run-v1-neg-fail", "unit.filter.neg", VerificationOutcome::Failed)
                ]
            }),
        ),
        false,
    )
    .await;
    let snap_failed = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
        false,
    )
    .await;
    assert_eq!(
        snap_failed["data"]["acceptance_evidence"]["items"][0]["status"],
        "failed"
    );
    assert_eq!(
        snap_failed["data"]["acceptance_evidence"]["items"][0]["reason"],
        "bound_check_failed"
    );

    // 6. Both bound checks pass on v1, evidence submitted, review opened (not yet approved) -> waiting.
    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let res_v1_pass = call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-reserve-v1-pass",
            "delivery.inspection.reserve",
            json!({
                "connector_id": "ci-connector",
                "connector_version": "1",
                "candidate_digest": digest_v1,
                "lease_seconds": 60
            }),
        ),
        false,
    )
    .await;
    call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-ingest-v1-pass",
            "delivery.facts.ingest",
            json!({
                "connector_id": "ci-connector",
                "inspection_id": res_v1_pass["receipt"]["data"]["inspection_id"],
                "event_id": "evt-v1-pass",
                "records": [
                    verification_envelope(&cand_v1.binding, "run-v1-neg-pass", "unit.filter.neg", VerificationOutcome::Passed)
                ]
            }),
        ),
        false,
    )
    .await;

    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let ev_v1 = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(
            &p,
            "ppd040-ev-v1",
            "delivery.evidence.submit",
            json!({
                "session_id": "session-a",
                "expected_session_version": "1",
                "dirty_tree": false,
                "artifact_text": std::str::from_utf8(body_v1).unwrap(),
                "delivery_payload": {
                    "delivery_candidate_digest": digest_v1,
                    "passed": true,
                    "output_digest": sha256_hex(body_v1)
                }
            }),
        ))
        .unwrap(),
        false,
    )
    .await;
    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let round_v1 = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(
            &p,
            "ppd040-open-v1",
            "review.open",
            json!({
                "session_id": "session-a",
                "expected_session_version": "1",
                "evidence_id": ev_v1["receipt"]["data"]["evidence_id"]
            }),
        ))
        .unwrap(),
        false,
    )
    .await["receipt"]["data"]["round_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let snap_open_review = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
        false,
    )
    .await;
    assert_eq!(
        snap_open_review["data"]["acceptance_evidence"]["items"][0]["status"], "waiting",
        "passing checks without review approval must stay waiting"
    );

    // 7. Independent reviewer starts session and approves round 1 on v1 -> supported!
    //    Unbound criterion stays no_bound_check (never inferred).
    let p_rev = call(
        &reviewer,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let rev_session = call(
        &reviewer,
        "awr_team_command",
        serde_json::to_value(command(
            &p_rev,
            "ppd040-rev-session",
            "session.start",
            json!({"conversation_id": "ppd040-review"}),
        ))
        .unwrap(),
        false,
    )
    .await["receipt"]["data"]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let p_rev = call(
        &reviewer,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    call(
        &reviewer,
        "awr_team_command",
        serde_json::to_value(command(
            &p_rev,
            "ppd040-approve-v1",
            "review.decide",
            json!({
                "session_id": rev_session,
                "expected_session_version": "1",
                "round_id": round_v1,
                "decision": "approve",
                "reason": "Both bound checks pass on candidate v1"
            }),
        ))
        .unwrap(),
        false,
    )
    .await;

    let (sup_status, snap_supported) = post_project(
        &server,
        "one",
        Some(A),
        "query",
        &json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
    )
    .await;
    assert_eq!(sup_status, reqwest::StatusCode::OK);
    assert_eq!(
        snap_supported["data"]["acceptance_evidence"]["status"],
        "provided"
    );
    assert_eq!(
        snap_supported["data"]["acceptance_evidence"]["items"][0]["status"],
        "supported"
    );
    assert_eq!(
        snap_supported["data"]["acceptance_evidence"]["items"][1]["status"],
        "no_bound_check"
    );
    save_verbatim("work-snapshot-supported.json", &snap_supported);

    // 8. Candidate version change (v1 -> v2) immediately invalidates v1's supported status!
    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let body_v2 = b"candidate v2 report";
    let cand_v2 = make_candidate(&p, "2", vec!["unit.filter.pos", "unit.filter.neg"], body_v2);
    let digest_v2 = cand_v2.binding.digest().unwrap();
    call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-select-v2",
            "delivery.candidate.select",
            json!({
                "expected_selected_digest": digest_v1,
                "session_id": "session-a",
                "claim_id": claim["claim_id"],
                "fence": claim["fence"],
                "lease_version": claim["lease_version"],
                "candidate": cand_v2
            }),
        ),
        false,
    )
    .await;

    let (cand_status, snap_v2_waiting) = post_project(
        &server,
        "one",
        Some(A),
        "query",
        &json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
    )
    .await;
    assert_eq!(cand_status, reqwest::StatusCode::OK);
    assert_eq!(
        snap_v2_waiting["data"]["acceptance_evidence"]["items"][0]["status"], "waiting",
        "candidate version change must invalidate prior candidate's supported status"
    );
    assert_eq!(
        snap_v2_waiting["data"]["acceptance_evidence"]["items"][0]["checks"][0]["outcome"],
        "not_recorded"
    );
    assert_eq!(
        snap_v2_waiting["data"]["acceptance_evidence"]["review_current_candidate"],
        false
    );
    save_verbatim("work-snapshot-candidate-changed.json", &snap_v2_waiting);

    // 9. Review returned on v2 (after both bound checks pass on v2) -> status becomes failed (review_returned)!
    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let res_v2 = call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-reserve-v2",
            "delivery.inspection.reserve",
            json!({
                "connector_id": "ci-connector",
                "connector_version": "1",
                "candidate_digest": digest_v2,
                "lease_seconds": 60
            }),
        ),
        false,
    )
    .await;
    call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "ppd040-ingest-v2",
            "delivery.facts.ingest",
            json!({
                "connector_id": "ci-connector",
                "inspection_id": res_v2["receipt"]["data"]["inspection_id"],
                "event_id": "evt-v2-pass",
                "records": [
                    verification_envelope(&cand_v2.binding, "run-v2-pos", "unit.filter.pos", VerificationOutcome::Passed),
                    verification_envelope(&cand_v2.binding, "run-v2-neg", "unit.filter.neg", VerificationOutcome::Passed)
                ]
            }),
        ),
        false,
    )
    .await;
    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let ev_v2 = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(
            &p,
            "ppd040-ev-v2",
            "delivery.evidence.submit",
            json!({
                "session_id": "session-a",
                "expected_session_version": "1",
                "dirty_tree": false,
                "artifact_text": std::str::from_utf8(body_v2).unwrap(),
                "delivery_payload": {
                    "delivery_candidate_digest": digest_v2,
                    "passed": true,
                    "output_digest": sha256_hex(body_v2)
                }
            }),
        ))
        .unwrap(),
        false,
    )
    .await;
    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let round_v2 = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(
            &p,
            "ppd040-open-v2",
            "review.open",
            json!({
                "session_id": "session-a",
                "expected_session_version": "1",
                "evidence_id": ev_v2["receipt"]["data"]["evidence_id"]
            }),
        ))
        .unwrap(),
        false,
    )
    .await["receipt"]["data"]["round_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let p_rev = call(
        &reviewer,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    call(
        &reviewer,
        "awr_team_command",
        serde_json::to_value(command(
            &p_rev,
            "ppd040-reject-v2",
            "review.decide",
            json!({
                "session_id": rev_session,
                "expected_session_version": "1",
                "round_id": round_v2,
                "decision": "reject",
                "reason": "Edge case missing in filter implementation"
            }),
        ))
        .unwrap(),
        false,
    )
    .await;

    let (ret_status, snap_returned) = post_project(
        &server,
        "one",
        Some(A),
        "query",
        &json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
    )
    .await;
    assert_eq!(ret_status, reqwest::StatusCode::OK);
    assert_eq!(
        snap_returned["data"]["acceptance_evidence"]["items"][0]["status"], "failed",
        "returned review must turn criterion status to failed, never supported"
    );
    assert_eq!(
        snap_returned["data"]["acceptance_evidence"]["items"][0]["reason"],
        "review_returned"
    );
    save_verbatim("work-snapshot-review-returned.json", &snap_returned);

    // 10. Binding change via planning draft: re-bind CRITERION_BOUND to `unit.filter.strict`.
    //     First approve a fresh round on v2 so v2 is supported, then activate the binding change
    //     and confirm the old supported evidence is immediately invalidated.
    let p = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    let round_v2b = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(
            &p,
            "ppd040-open-v2b",
            "review.open",
            json!({
                "session_id": "session-a",
                "expected_session_version": "1",
                "evidence_id": ev_v2["receipt"]["data"]["evidence_id"]
            }),
        ))
        .unwrap(),
        false,
    )
    .await["receipt"]["data"]["round_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let p_rev = call(
        &reviewer,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await;
    call(
        &reviewer,
        "awr_team_command",
        serde_json::to_value(command(
            &p_rev,
            "ppd040-approve-v2b",
            "review.decide",
            json!({
                "session_id": rev_session,
                "expected_session_version": "1",
                "round_id": round_v2b,
                "decision": "approve",
                "reason": "Verified on v2"
            }),
        ))
        .unwrap(),
        false,
    )
    .await;
    let snap_v2_supported = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
        false,
    )
    .await;
    assert_eq!(
        snap_v2_supported["data"]["acceptance_evidence"]["items"][0]["status"],
        "supported"
    );

    let mut examples_v2 = examples_v1.clone();
    examples_v2[1].check_id = Some("unit.filter.strict".into());
    let before_rebind = after_a.clone();
    let after_rebind = TaskDraft {
        verification_requirements: Some(vec![
            "unit.filter.pos".into(),
            "unit.filter.strict".into(),
        ]),
        examples: Some(examples_v2),
        ..before_rebind.clone()
    };
    let drafted2 = call(
        &client,
        "awr_team_planning_draft",
        json!({
            "protocol_version": 1,
            "request_id": "ppd040-draft-rebind",
            "mode": "create",
            "changes": [{"op": "edit_fields", "before": before_rebind, "after": after_rebind}],
            "allowed_spec_roots": ["src"],
            "project_goal_keys": ["alpha"],
            "self_approve_policy": OrdinaryPlanningSelfApprovePolicy::ordinary_default()
        }),
        false,
    )
    .await;
    let cand2_id = drafted2["result"]["candidate_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cand2_digest = drafted2["result"]["candidate_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &client,
        "awr_team_planning_approve",
        json!({
            "protocol_version": 1,
            "request_id": "ppd040-approve-rebind",
            "candidate_id": cand2_id,
            "candidate_digest": cand2_digest
        }),
        false,
    )
    .await;
    let pub2 = call(
        &client,
        "awr_team_planning_publish",
        json!({
            "protocol_version": 1,
            "request_id": "ppd040-publish-rebind",
            "candidate_id": cand2_id,
            "candidate_digest": cand2_digest,
            "activate": true,
            "impact_proven": true
        }),
        false,
    )
    .await;
    assert!(pub2["result"]["activation"]["activated_snapshot_id"].is_string());

    let snap_after_rebind = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.snapshot","work_id":"a","max_context_bytes":262144}),
        false,
    )
    .await;
    assert_eq!(
        snap_after_rebind["data"]["acceptance_evidence"]["candidate_current"], false,
        "changing a check binding changes contract_hash and invalidates the old candidate"
    );
    assert_ne!(
        snap_after_rebind["data"]["acceptance_evidence"]["items"][0]["status"],
        "supported"
    );
    assert_eq!(
        snap_after_rebind["data"]["acceptance_evidence"]["items"][0]["bound_checks"],
        json!(["unit.filter.pos", "unit.filter.strict"])
    );
    assert_eq!(
        snap_after_rebind["data"]["acceptance_evidence"]["items"][0]["checks"][1]["outcome"],
        "not_recorded"
    );

    // 11. Permission & cross-project isolation: unauthenticated, unscoped, cross-project, revoked.
    let (status, unauth) = post_project(
        &server,
        "one",
        None,
        "query",
        &json!({"protocol_version":1,"op":"work.snapshot","work_id":"a"}),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    assert!(!unauth.to_string().contains("acceptance_evidence"));

    let web_unauth = http()
        .post(format!("{}/v1/web/projects/one/query", server.base))
        .json(&json!({"protocol_version":1,"op":"work.snapshot","work_id":"a"}))
        .send()
        .await
        .unwrap();
    assert_eq!(web_unauth.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(
        !web_unauth
            .text()
            .await
            .unwrap()
            .contains("acceptance_evidence")
    );

    let (status, unscoped) = post_project(
        &server,
        "one",
        Some(NONE),
        "query",
        &json!({"protocol_version":1,"op":"work.snapshot","work_id":"a"}),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    assert!(!unscoped.to_string().contains("acceptance_evidence"));

    let (status, cross_project) = post_project(
        &server,
        "other",
        Some(A),
        "query",
        &json!({"protocol_version":1,"op":"work.snapshot","work_id":"a"}),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    assert!(!cross_project.to_string().contains("acceptance_evidence"));

    admin
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            &[],
        )
        .await
        .unwrap();
    let (status, revoked) = post_project(
        &server,
        "one",
        Some(A),
        "query",
        &json!({"protocol_version":1,"op":"work.snapshot","work_id":"a"}),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    assert!(!revoked.to_string().contains("acceptance_evidence"));

    let _ = std::fs::remove_dir_all(&tmp_dir);
    let _ = server.base;
}
