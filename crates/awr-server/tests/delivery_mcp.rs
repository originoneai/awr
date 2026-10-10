#![cfg(feature = "pg-tests")]
//! Synthetic SDK/HTTP regressions, not native team business acceptance.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;

use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team::delivery::*;
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
    tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .unwrap()
        .unwrap()
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
async fn request(
    server: &Server,
    alias: &str,
    token: &str,
    path: &str,
    args: &Value,
) -> (reqwest::StatusCode, Value) {
    let response = http()
        .post(format!("{}/{alias}/{path}", server.url))
        .bearer_auth(token)
        .json(args)
        .send()
        .await
        .unwrap();
    (response.status(), response.json().await.unwrap())
}
fn neutral(p: &Value, key: &str, op: &str, mut args: Value) -> Value {
    args["source_snapshot_id"] = p["source_snapshot_id"].clone();
    serde_json::to_value(command(p, key, op, args)).unwrap()
}
fn read(op: &str, key: Option<&str>) -> Value {
    let mut q = json!({"protocol_version":1,"op":op,"work_id":"a"});
    if let Some(key) = key {
        q["request_id"] = json!(key);
    }
    q
}
fn candidate(p: &Value) -> DeliveryCandidate {
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: "sdk-artifact".into(),
            sha256: "e".repeat(64),
            byte_length: "12".into(),
            locator: "fixture://artifact/report".into(),
        }],
    };
    serde_json::from_value(json!({"binding":{
        "tenant_id":TENANT,"project_id":PROJECT,"scope_id":"main","workstream_id":p["workstream_id"],
        "work_id":"a","candidate_id":"sdk-candidate","candidate_version":"1","contract_hash":p["data"]["contract_hash"],
        "manifest_digest":manifest.digest().unwrap(),"source_revision":null,"required_checks":["report"],
        "target":{"resource":"fixture://sdk-artifacts","reference":null,"precondition":{"kind":"missing"}}
    },"manifest":manifest})).unwrap()
}

#[tokio::test]
async fn neutral_sdk_http_parity_reconnect_and_discovery_preserve_domain_receipts() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let client = connect(&server, A).await;
    let tools = client.list_all_tools().await.unwrap();
    let command_tool = tools
        .iter()
        .find(|tool| tool.name == "awr_team_command")
        .unwrap();
    let query_tool = tools
        .iter()
        .find(|tool| tool.name == "awr_team_query")
        .unwrap();
    for op in [
        "delivery.connector.configure",
        "delivery.candidate.select",
        "delivery.inspection.reserve",
        "delivery.facts.ingest",
        "delivery.source.prepare",
        "delivery.source.renew",
        "delivery.source.write",
        "delivery.source.confirm",
        "delivery.source.abandon",
    ] {
        assert!(
            command_tool.input_schema["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(op))
        );
        let rule = command_tool.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["if"]["properties"]["op"]["const"] == op)
            .unwrap();
        assert!(
            rule["then"]["properties"]["args"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("source_snapshot_id"))
        );
    }
    let fields = &command_tool.input_schema["properties"]["args"]["properties"];
    assert_eq!(
        fields["records"]["items"]["properties"]["protocol"]["const"],
        DELIVERY_PROTOCOL
    );
    assert_eq!(
        fields["candidate"]["required"],
        json!(["binding", "manifest"])
    );
    assert!(!fields.as_object().unwrap().contains_key("read_set"));
    for op in [
        "delivery.neutral.inspect",
        "delivery.submission.describe",
        "delivery.neutral.outcome",
        "delivery.source.status",
    ] {
        assert!(
            query_tool.input_schema["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(op))
        );
    }
    let query_rules = query_tool.input_schema["allOf"].as_array().unwrap();
    for op in ["delivery.neutral.outcome", "delivery.integration.inspect"] {
        assert!(
            query_rules.iter().any(|rule| {
                rule["if"]["properties"]["op"]["enum"]
                    .as_array()
                    .is_some_and(|ops| ops.contains(&json!(op)))
                    && rule["then"]["required"] == json!(["request_id"])
            }),
            "{op} must require request_id"
        );
    }
    let caps = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
        false,
    )
    .await;
    assert_eq!(caps["neutral_delivery"]["background_scheduling"], false);
    assert_eq!(
        caps["neutral_delivery"]["outcome_query"],
        "delivery.neutral.outcome"
    );
    let p = call(&client, "awr_team_query", read("work.prepare", None), false).await;
    let claim=call(&client,"awr_team_command",serde_json::to_value(command(&p,"sdk-claim","task.claim_available",json!({
        "session_id":"session-a","expected_session_version":"1","expected_work_version":"0",
        "expected_responsibility_version":p["data"]["responsibility"]["version"],"ttl_seconds":3600,
    }))).unwrap(),false).await["receipt"]["data"].clone();
    let p = call(&client, "awr_team_query", read("work.prepare", None), false).await;
    let candidate = candidate(&p);
    let selected=call(&client,"awr_team_command",neutral(&p,"sdk-select","delivery.candidate.select",json!({
        "expected_selected_digest":null,"session_id":"session-a","claim_id":claim["claim_id"],
        "fence":claim["fence"],"lease_version":claim["lease_version"],"candidate":candidate,
    })),false).await;
    assert_eq!(selected["execution_authorized"], false);
    assert_eq!(selected["receipt"]["protocol"], "awr-delivery-sync-v1");
    let configure = neutral(
        &p,
        "sdk-configure",
        "delivery.connector.configure",
        json!({
            "expected_connector_version":"0","mapping":{"connector_id":"sdk-connector","provider":"reference",
                "resource":"fixture://sdk-artifacts","principal_actor_id":"agent","principal_client_id":"cli-a",
                "fact_source":"caller_declared","enabled":true},
        }),
    );
    let (status, configured) = request(&server, "one", A, "command", &configure).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let replay = call(&client, "awr_team_command", configure, false).await;
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"]["data"], configured["receipt"]["data"]);
    let reserved=call(&client,"awr_team_command",neutral(&p,"sdk-reserve","delivery.inspection.reserve",json!({
        "connector_id":"sdk-connector","connector_version":"1","candidate_digest":candidate.binding.digest().unwrap(),"lease_seconds":60,
    })),false).await;
    let envelope = DeliveryEnvelope {
        protocol: DELIVERY_PROTOCOL.into(),
        protocol_version: DELIVERY_PROTOCOL_VERSION,
        record: DeliveryRecord::Verification(VerificationRun {
            binding: candidate.binding,
            run_id: "sdk-run".into(),
            check: "report".into(),
            outcome: VerificationOutcome::Unknown,
            result_artifact: None,
            provenance: FactProvenance {
                source: FactSource::CallerDeclared,
                reference: "fixture://sdk/result".into(),
                observed_at_unix_ms: None,
                recorded_at_unix_ms: 42,
            },
        }),
    };
    let ingest = neutral(
        &p,
        "sdk-ingest",
        "delivery.facts.ingest",
        json!({
            "connector_id":"sdk-connector","inspection_id":reserved["receipt"]["data"]["inspection_id"],"event_id":"sdk-event","records":[envelope],
        }),
    );
    let (status, ingested) = request(&server, "one", A, "command", &ingest).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let replay = call(&client, "awr_team_command", ingest, false).await;
    assert_eq!(replay["receipt"]["data"], ingested["receipt"]["data"]);
    let facts = call(
        &client,
        "awr_team_query",
        read("delivery.neutral.inspect", None),
        false,
    )
    .await;
    let (status, via_http) = request(
        &server,
        "one",
        A,
        "query",
        &read("delivery.neutral.inspect", None),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(facts, via_http);
    assert_eq!(facts["project_revision"], facts["data"]["project_revision"]);
    assert_eq!(facts["data"]["facts"][0]["current"], true);
    assert_eq!(facts["data"]["acceptance_ready"], false);
    client.cancel().await.unwrap();
    let resumed = connect(&server, A).await;
    let outcome = call(
        &resumed,
        "awr_team_query",
        read("delivery.neutral.outcome", Some("sdk-ingest")),
        false,
    )
    .await;
    assert_eq!(outcome["data"]["receipt"], ingested["receipt"]);
    assert_eq!(outcome["data"]["state_basis"], "at_commit");
    assert_eq!(outcome["data"]["execution_authorized"], false);
    let legacy = call(
        &resumed,
        "awr_team_query",
        read("command.inspect", Some("sdk-ingest")),
        false,
    )
    .await;
    assert_eq!(legacy["data"]["state"], "unknown");
    // This fixture has no configured source writer; do not fabricate a status.
    let unavailable = call(
        &resumed,
        "awr_team_query",
        read("delivery.source.status", None),
        true,
    )
    .await;
    let (status, via_http) = request(
        &server,
        "one",
        A,
        "query",
        &read("delivery.source.status", None),
    )
    .await;
    assert!(!status.is_success());
    assert_eq!(unavailable, via_http);
    assert_eq!(unavailable["code"], "Unavailable");
    let row=admin.query_one("SELECT (SELECT count(*) FROM awr_team.delivery_sync_requests),(SELECT count(*) FROM awr_team.operations),(SELECT count(*) FROM awr_team.claims WHERE state='active')",&[]).await.unwrap();
    assert_eq!(row.get::<_, i64>(0), 4);
    assert_eq!(row.get::<_, i64>(1), 1);
    assert_eq!(row.get::<_, i64>(2), 1);
    resumed.cancel().await.unwrap();
}

#[tokio::test]
async fn lazy_submission_contract_has_http_mcp_parity_and_preserves_command_authority() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let client = connect(&server, A).await;
    let input = read("delivery.submission.describe", None);
    let (status, http_contract) = request(&server, "one", A, "query", &input).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let mcp_contract = call(&client, "awr_team_query", input.clone(), false).await;
    assert_eq!(http_contract["data"], mcp_contract["data"]);
    assert_eq!(mcp_contract["data"]["read_only"], true);
    assert_eq!(mcp_contract["data"]["stored_selection"], Value::Null);
    assert_eq!(mcp_contract["data"]["execution_authorized"], false);
    assert_eq!(mcp_contract["data"]["acceptance_ready"], false);
    assert!(serde_json::to_vec(&mcp_contract).unwrap().len() < 16384);
    let tools = client.list_all_tools().await.unwrap();
    let tool = tools
        .iter()
        .find(|tool| tool.name == "awr_team_query")
        .unwrap();
    assert!(
        tool.input_schema["properties"]["op"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("delivery.submission.describe"))
    );
    assert!(
        tool.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["if"]["properties"]["op"]["enum"]
                .as_array()
                .is_some_and(|ops| ops.contains(&json!("delivery.submission.describe")))
                && rule["then"]["required"] == json!(["work_id"]))
    );
    let ordinary = call(
        &client,
        "awr_team_query",
        read("delivery.neutral.inspect", None),
        false,
    )
    .await;
    assert_eq!(ordinary["data"]["submission"]["describe_query"], input);
    assert!(ordinary["data"]["submission"].get("args_schema").is_none());
    let p = call(&client, "awr_team_query", read("work.prepare", None), false).await;
    let claim = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(
            &p,
            "describe-claim",
            "task.claim_available",
            json!({"session_id":"session-a",
            "expected_session_version":"1","expected_work_version":"0",
            "expected_responsibility_version":p["data"]["responsibility"]["version"],
            "ttl_seconds":3600}),
        ))
        .unwrap(),
        false,
    )
    .await["receipt"]["data"]
        .clone();
    let p = call(&client, "awr_team_query", read("work.prepare", None), false).await;
    let mut selected = serde_json::to_value(candidate(&p)).unwrap();
    let mut binding = mcp_contract["data"]["binding_values"]
        .as_object()
        .unwrap()
        .clone();
    for key in [
        "candidate_id",
        "candidate_version",
        "manifest_digest",
        "source_revision",
        "target",
    ] {
        binding.insert(key.into(), selected["binding"][key].clone());
    }
    selected["binding"] = Value::Object(binding);
    let selection_args = json!({
        "session_id":"session-a","claim_id":claim["claim_id"],"fence":claim["fence"],
        "lease_version":claim["lease_version"],"expected_selected_digest":null,
        "candidate":selected});
    let selected = call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "describe-select",
            mcp_contract["data"]["command"]["op"].as_str().unwrap(),
            selection_args.clone(),
        ),
        false,
    )
    .await;
    let state = call(&client, "awr_team_query", input.clone(), false).await;
    assert_eq!(
        state["data"]["stored_selection"]["binding_digest"],
        selected["receipt"]["data"]["candidate_digest"]
    );
    assert_eq!(state["data"]["stored_selection"]["current"], true);
    let other = connect(&server, B).await;
    let denied = call(&other, "awr_team_query", input.clone(), true).await;
    assert_eq!(
        denied,
        json!({"code":"Forbidden","message":"access denied"})
    );
    assert_eq!(
        request(&server, "other", A, "query", &input).await.0,
        reqwest::StatusCode::FORBIDDEN
    );
    let mut bounded = input.clone();
    bounded["max_context_bytes"] = json!(64);
    let (status, error) = request(&server, "one", A, "query", &bounded).await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT);
    assert_eq!(error, call(&client, "awr_team_query", bounded, true).await);
    // Read permission remains sufficient to describe, and insufficient to select.
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert_eq!(
        call(&client, "awr_team_query", input, false).await["data"]["read_only"],
        true
    );
    let p = call(&client, "awr_team_query", read("work.prepare", None), false).await;
    let mut denied_args = selection_args;
    denied_args["expected_selected_digest"] =
        selected["receipt"]["data"]["candidate_digest"].clone();
    let denied = call(
        &client,
        "awr_team_command",
        neutral(
            &p,
            "readonly-select",
            "delivery.candidate.select",
            denied_args,
        ),
        true,
    )
    .await;
    assert_eq!(
        denied,
        json!({"code":"Forbidden","message":"access denied"})
    );
    other.cancel().await.unwrap();
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn neutral_transport_refuses_injection_cross_scope_and_revoked_clients() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let client = connect(&server, A).await;
    let p = call(&client, "awr_team_query", read("work.prepare", None), false).await;
    let valid = neutral(
        &p,
        "auth-configure",
        "delivery.connector.configure",
        json!({
            "expected_connector_version":"0","mapping":{"connector_id":"auth-connector","provider":"reference",
                "resource":"fixture://sdk-artifacts","principal_actor_id":"agent","principal_client_id":"cli-a",
                "fact_source":"caller_declared","enabled":true},
        }),
    );
    call(&client, "awr_team_command", valid.clone(), false).await;
    for field in ["read_set", "request_id", "actor_id", "source_path"] {
        let mut forged = valid.clone();
        forged["request_id"] = json!(format!("forged-{field}"));
        forged["args"][field] = json!("injected");
        let mcp = call(&client, "awr_team_command", forged.clone(), true).await;
        let (status, http) = request(&server, "one", A, "command", &forged).await;
        assert!(!status.is_success());
        assert_eq!(mcp, http);
    }
    let mut wrong = read("delivery.neutral.outcome", Some("auth-configure"));
    wrong["work_id"] = json!("c");
    let mcp = call(&client, "awr_team_query", wrong.clone(), true).await;
    let (status, http) = request(&server, "one", A, "query", &wrong).await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    assert_eq!(mcp, http);
    let (status, _) = request(
        &server,
        "other",
        A,
        "query",
        &read("delivery.neutral.inspect", None),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    let other = connect(&server, B).await;
    assert_eq!(
        call(&other, "awr_team_command", valid, true).await["code"],
        "Forbidden"
    );
    let mut missing = read("delivery.neutral.outcome", Some("auth-configure"));
    missing.as_object_mut().unwrap().remove("work_id");
    assert!(
        call(&client, "awr_team_query", missing, true)
            .await
            .get("code")
            .is_some()
    );
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(
        client
            .call_tool(
                CallToolRequestParams::new("awr_team_query").with_arguments(
                    read("delivery.neutral.outcome", Some("auth-configure"))
                        .as_object()
                        .unwrap()
                        .clone()
                )
            )
            .await
            .is_err()
    );
    assert!(client.list_all_tools().await.is_err());
    let (status, _) = request(
        &server,
        "one",
        A,
        "query",
        &read("delivery.neutral.outcome", Some("auth-configure")),
    )
    .await;
    assert!(!status.is_success());
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.delivery_sync_requests", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    other.cancel().await.unwrap();
    client.cancel().await.unwrap();
}
