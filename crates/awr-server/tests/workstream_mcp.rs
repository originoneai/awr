#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
use awr_server::service::{ProjectBinding, ServiceConfig};
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
async fn connect(server: &Server, alias: &str, token: &str) -> Result<Client, String> {
    let transport = StreamableHttpClientTransport::with_client(
        http(),
        StreamableHttpClientTransportConfig::with_uri(format!("{}/{alias}/mcp", server.url))
            .auth_header(token),
    );
    tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .map_err(|_| "timeout".to_owned())?
        .map_err(|e| e.to_string())
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
async fn prepared(client: &Client) -> Value {
    call(
        client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a"}),
        false,
    )
    .await
}
fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}
fn raw(server: &Server, token: &str) -> reqwest::RequestBuilder {
    http()
        .post(format!("{}/one/mcp", server.url))
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-03-26")
}

#[tokio::test]
async fn numeric_command_versions_are_rejected_without_mutation_and_strings_succeed() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let client = connect(&server, "one", A).await.unwrap();
    let before = prepared(&client).await;
    let request = serde_json::to_value(command(
        &before,
        "version-input-sentinel",
        "claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1",
            "expected_work_version":"0","ttl_seconds":60}),
    ))
    .unwrap();
    let counts_sql = "SELECT (SELECT count(*) FROM awr_team.claims),
        (SELECT count(*) FROM awr_team.operations),
        (SELECT count(*) FROM awr_team.checkpoints),
        (SELECT count(*) FROM awr_team.executions)";
    let baseline = admin.query_one(counts_sql, &[]).await.unwrap();
    for (nested, field, number) in [
        (true, "expected_session_version", 1),
        (true, "expected_work_version", 0),
        (false, "expected_project_revision", 1),
    ] {
        let mut invalid = request.clone();
        if nested {
            invalid["args"][field] = json!(number);
        } else {
            invalid[field] = json!(number);
        }
        let error = call(&client, "awr_team_command", invalid, true).await;
        assert_eq!(error["code"], "InvalidInput");
        assert_eq!(error["message"], "command fields or bounds are invalid");
        assert!(
            error["next_step"]
                .as_str()
                .unwrap()
                .contains("decimal strings")
        );
        for private in ["version-input-sentinel", "session-a", A] {
            assert!(!error.to_string().contains(private));
        }
        let audit = admin
            .query_one(
                "SELECT result, finished_at IS NOT NULL FROM awr_team.request_audit
                 WHERE action='claim.acquire' ORDER BY created_at DESC,id DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(audit.get::<_, String>(0), "failed");
        assert!(audit.get::<_, bool>(1));
        let after = admin.query_one(counts_sql, &[]).await.unwrap();
        for index in 0..4 {
            assert_eq!(after.get::<_, i64>(index), baseline.get::<_, i64>(index));
        }
        assert_eq!(
            prepared(&client).await["project_revision"],
            before["project_revision"]
        );
    }
    let receipt = call(&client, "awr_team_command", request, false).await;
    assert_eq!(receipt["receipt"]["data"]["state"], "active");
    assert_eq!(receipt["replayed"], false);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn discovered_review_and_evidence_selectors_reach_scoped_records() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let client = connect(&server, "one", A).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    let schema = &tools
        .iter()
        .find(|tool| tool.name == "awr_team_query")
        .unwrap()
        .input_schema;
    for field in [
        "evidence_id",
        "review_round_id",
        "change_id",
        "member_actor_id",
        "category",
    ] {
        assert_eq!(schema["properties"][field]["type"], "string");
        assert_eq!(schema["properties"][field]["maxLength"], 128);
    }
    assert_eq!(schema["properties"]["include_denies"]["type"], "boolean");
    let submitted = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(
            &prepared(&client).await,
            "selector-evidence",
            "evidence.submit",
            json!({"session_id":"session-a","expected_session_version":"1",
                "payload":{"passed":true},"artifact_hex":"73636f706564","dirty_tree":false}),
        ))
        .unwrap(),
        false,
    )
    .await;
    let evidence_id = &submitted["receipt"]["data"]["evidence_id"];
    let opened = call(
        &client,
        "awr_team_command",
        serde_json::to_value(command(&prepared(&client).await, "selector-review", "review.open",
            json!({"session_id":"session-a","expected_session_version":"1","evidence_id":evidence_id}))).unwrap(),
        false,
    ).await;
    let round_id = &opened["receipt"]["data"]["round_id"];
    for (op, field, id) in [
        ("evidence.inspect", "evidence_id", evidence_id),
        ("review.inspect", "review_round_id", round_id),
    ] {
        let mut args = json!({"protocol_version":1,"op":op,"work_id":"a"});
        assert_eq!(
            call(&client, "awr_team_query", args.clone(), true).await["code"],
            "InvalidInput"
        );
        args[field] = id.clone();
        let record = call(&client, "awr_team_query", args.clone(), false).await;
        assert!(record["data"].to_string().contains(id.as_str().unwrap()));
        args["work_id"] = json!("b-private");
        assert_eq!(
            call(&client, "awr_team_query", args, true).await["code"],
            "Forbidden"
        );
    }
    // Audit filters remain restricted to the operations that support them.
    for args in [
        json!({"protocol_version":1,"op":"audit.history","change_id":"change",
            "member_actor_id":"agent","category":"access","include_denies":false}),
        json!({"protocol_version":1,"op":"audit.requests","member_actor_id":"agent"}),
        json!({"protocol_version":1,"op":"audit.development","member_actor_id":"agent"}),
    ] {
        let query: awr_team_pg::WorkstreamQuery = serde_json::from_value(args).unwrap();
        query.validate().unwrap();
    }
    assert_eq!(
        call(
            &client,
            "awr_team_query",
            json!({"protocol_version":1,"op":"work.prepare","work_id":"a","include_denies":true}),
            true
        )
        .await["code"],
        "InvalidInput"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_feedback_is_discoverable_nonterminal_and_does_not_change_consumed_context() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let client = connect(&server, "one", A).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    let schema = &tools
        .iter()
        .find(|t| t.name == "awr_team_command")
        .unwrap()
        .input_schema;
    assert_eq!(
        schema["properties"]["args"]["properties"]["progress"]["required"],
        json!(["phase", "summary"])
    );
    assert_eq!(
        schema["properties"]["args"]["properties"]["usage"]["properties"]["scope"]["const"],
        "host_session"
    );
    let before = prepared(&client).await;
    assert_eq!(before["data"]["guidance"]["code"], "declare_client");
    let command=serde_json::to_value(command(&before,"mcp-feedback","session.checkpoint",json!({
        "session_id":"session-a","expected_session_version":"1","context_hash":before["data"]["context_hash"],
        "next_action":"Await design confirmation","open_loops":["User confirmation"],
        "client_info":{"product":"Example Agent","capabilities":{"model":"unsupported","usage":"unsupported","progress":"supported"}},
        "progress":{"phase":"waiting_user","summary":"Implementation options are ready for confirmation."}
    }))).unwrap();
    let receipt = call(&client, "awr_team_command", command.clone(), false).await;
    assert_eq!(receipt["execution_authorized"], false);
    assert_eq!(
        call(&client, "awr_team_command", command, false).await["replayed"],
        true
    );
    let after = prepared(&client).await;
    assert_eq!(
        after["data"]["context_hash"],
        before["data"]["context_hash"]
    );
    assert_eq!(after["data"]["guidance"]["code"], "wait_for_change");
    assert!(after["data"]["guidance"].to_string().len() < 900);
    let observation = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.observe","session_id":"session-a"}),
        false,
    )
    .await;
    assert_eq!(observation["data"]["progress"]["phase"], "waiting_user");
    assert_eq!(
        observation["data"]["missing"]["model"],
        "client_collection_unsupported"
    );
    let mut baseline = before["data"].clone();
    baseline.as_object_mut().unwrap().remove("guidance");
    let budget = serde_json::to_vec(&baseline).unwrap().len();
    let compact = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"a","max_context_bytes":budget}),
        false,
    )
    .await;
    assert!(compact["data"].get("guidance").is_none());
    assert_eq!(
        compact["data"]["context_hash"],
        before["data"]["context_hash"]
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn sdk_clients_negotiate_tools_and_isolate_workstreams_on_one_service() {
    let (_guard, _, _, store) = setup().await;
    let server = start(store).await;
    let (a, b) = tokio::join!(connect(&server, "one", A), connect(&server, "one", B));
    let (a, b) = (a.unwrap(), b.unwrap());
    let info = a.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "awr-team-mcp");
    assert!(info.capabilities.tools.is_some());
    let tools = a.list_all_tools().await.unwrap();
    // Query/command, member access and planning are discoverable; calls still
    // recheck the actor's effective authority.
    assert_eq!(tools.len(), 12);
    let caps = call(
        &a,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
        false,
    )
    .await;
    for (tool, ops, read) in [
        ("awr_team_query", "queries", true),
        ("awr_team_command", "commands", false),
    ] {
        let t = tools.iter().find(|t| t.name == tool).unwrap();
        assert_eq!(t.annotations.as_ref().unwrap().read_only_hint, Some(read));
        assert_eq!(t.input_schema["properties"]["op"]["enum"], caps[ops]);
    }
    assert_eq!(caps["execution_admission"], true);
    let observation = call(
        &a,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.observe","work_id":"a"}),
        false,
    )
    .await;
    assert_eq!(observation["data"]["session"]["id"], "session-a");
    assert_eq!(observation["data"]["execution_authorized"], false);
    let args = json!({"protocol_version":1,"op":"work.list"});
    let (ar, br) = tokio::join!(
        call(&a, "awr_team_query", args.clone(), false),
        call(&b, "awr_team_query", args, false)
    );
    assert_eq!(ar["data"]["total"], 2);
    assert!(!ar.to_string().contains("private"));
    assert_eq!(br["data"]["items"][0]["work_id"], "b-private");
    for work in ["b-private", "does-not-exist"] {
        let r = call(
            &a,
            "awr_team_query",
            json!({"protocol_version":1,"op":"work.prepare","work_id":work}),
            true,
        )
        .await;
        assert_eq!(r, json!({"code":"Forbidden","message":"access denied"}));
    }
    let dep = call(
        &a,
        "awr_team_query",
        json!({"protocol_version":1,"op":"work.prepare","work_id":"c"}),
        false,
    )
    .await;
    assert_eq!(dep["data"]["context_complete"], false);
    assert_eq!(dep["data"]["dependency_export_unavailable"], true);
    assert!(!dep.to_string().contains("b-private"));
    assert!(connect(&server, "other", A).await.is_err());
    assert!(connect(&server, "one", NONE).await.is_err());
    a.cancel().await.unwrap();
    b.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_claims_reconnect_and_share_live_state_and_historical_http_receipts() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let a = connect(&server, "one", A).await.unwrap();
    let b = connect(&server, "one", B).await.unwrap();
    let request = serde_json::to_value(command(&prepared(&a).await,"mcp-take","claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":"0","ttl_seconds":60}))).unwrap();
    let denied = call(&b, "awr_team_command", request.clone(), true).await;
    assert_eq!(denied["code"], "Forbidden");
    let taken = call(&a, "awr_team_command", request.clone(), false).await;
    assert_eq!(taken["receipt"]["execution_authorized"], false);
    let claim = &taken["receipt"]["data"];
    let query = json!({"protocol_version":1,"op":"claim.inspect","session_id":"session-a","claim_id":claim["claim_id"]});
    a.cancel().await.unwrap();
    let a = connect(&server, "one", A).await.unwrap();
    assert_eq!(
        call(&a, "awr_team_query", query.clone(), false).await["data"]["lease_live"],
        true
    );
    let hidden = call(&b, "awr_team_query", query.clone(), true).await;
    assert_eq!(hidden["code"], "Forbidden");
    let mut args = json!({"session_id":"session-a","expected_session_version":"1","claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":"1","ttl_seconds":120});
    let renew = serde_json::to_value(command(
        &prepared(&a).await,
        "http-renew",
        "claim.renew",
        args.clone(),
    ))
    .unwrap();
    let renewed: Value = http()
        .post(format!("{}/one/command", server.url))
        .bearer_auth(A)
        .json(&renew)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renewed["receipt"]["data"]["lease_version"], "2");
    let stale = serde_json::to_value(command(
        &prepared(&a).await,
        "stale-renew",
        "claim.renew",
        args.clone(),
    ))
    .unwrap();
    assert_eq!(
        call(&a, "awr_team_command", stale, true).await["code"],
        "PreconditionsChanged"
    );
    args["expected_lease_version"] = json!("2");
    args.as_object_mut().unwrap().remove("ttl_seconds");
    let release = serde_json::to_value(command(
        &prepared(&a).await,
        "mcp-release",
        "claim.release",
        args,
    ))
    .unwrap();
    let released = call(&a, "awr_team_command", release, false).await;
    assert_eq!(released["receipt"]["data"]["state"], "released");
    assert_eq!(
        released["receipt"]["data"]["resource_release_performed"],
        false
    );
    let replay = call(&a, "awr_team_command", request, false).await;
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], taken["receipt"]);
    assert_eq!(
        call(&a, "awr_team_query", query, false).await["data"]["lease_live"],
        false
    );
    let observed = call(
        &a,
        "awr_team_query",
        json!({"protocol_version":1,"op":"command.inspect",
        "work_id":"a","request_id":"http-renew"}),
        false,
    )
    .await;
    assert_eq!(observed["data"]["receipt"], renewed["receipt"]);
    a.cancel().await.unwrap();
    b.cancel().await.unwrap();
}

#[tokio::test]
async fn execution_intents_share_http_mcp_identity_without_dispatching_effects() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let a = connect(&server, "one", A).await.unwrap();
    let b = connect(&server, "one", B).await.unwrap();
    let take=serde_json::to_value(command(&prepared(&a).await,"take","claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":"0","ttl_seconds":60}))).unwrap();
    let taken = call(&a, "awr_team_command", take, false).await;
    let claim = &taken["receipt"]["data"];
    for prepare_via_http in [true, false] {
        let p = prepared(&a).await;
        let id = if prepare_via_http {
            "http-intent"
        } else {
            "mcp-intent"
        };
        let request=serde_json::to_value(command(&p,id,"execution.prepare",json!({"session_id":"session-a",
            "expected_session_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],
            "expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
            "input_digest":"a".repeat(64),"declared_scope":["src"]}))).unwrap();
        let begun: Value = if prepare_via_http {
            http()
                .post(format!("{}/one/command", server.url))
                .bearer_auth(A)
                .json(&request)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap()
        } else {
            call(&a, "awr_team_command", request.clone(), false).await
        };
        let e = &begun["receipt"]["data"];
        assert_eq!(begun["receipt"]["execution_authorized"], false);
        assert_eq!(e["dispatched"], false);
        let q = json!({"protocol_version":1,"op":"execution.inspect","work_id":"a","execution_id":e["execution_id"]});
        assert_eq!(
            call(&b, "awr_team_query", q.clone(), true).await["code"],
            "Forbidden"
        );
        assert_eq!(
            call(&a, "awr_team_query", q.clone(), false).await["data"]["state"],
            "prepared"
        );
        let stop=serde_json::to_value(command(&prepared(&a).await,&format!("cancel-{id}"),"execution.cancel",
            json!({"session_id":"session-a","expected_session_version":"1","execution_id":e["execution_id"],"expected_execution_version":"1"}))).unwrap();
        let denied = http()
            .post(format!("{}/one/command", server.url))
            .bearer_auth(B)
            .json(&stop)
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), 403);
        let cancelled: Value = if prepare_via_http {
            call(&a, "awr_team_command", stop, false).await
        } else {
            http()
                .post(format!("{}/one/command", server.url))
                .bearer_auth(A)
                .json(&stop)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap()
        };
        assert_eq!(cancelled["receipt"]["data"]["stop_confirmed"], true);
        let replay = call(&a, "awr_team_command", request, false).await;
        assert_eq!(replay["receipt"], begun["receipt"]);
        assert_eq!(replay["replayed"], true);
        let now: Value = http()
            .post(format!("{}/one/query", server.url))
            .bearer_auth(A)
            .json(&q)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(now["data"]["state"], "cancelled");
        assert_eq!(now["data"]["execution_authorized"], false);
    }
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.outbox", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    a.cancel().await.unwrap();
    b.cancel().await.unwrap();
}

#[tokio::test]
async fn admission_observation_and_authorized_recovery_share_transport_identity() {
    for trusted in [false, true] {
        recovery_over_transports(trusted, "caller_managed", false).await;
    }
    recovery_over_transports(true, "reference_write_v1", false).await;
}

#[tokio::test]
async fn previous_epoch_recovery_requires_explicit_review_over_http_and_replays_over_mcp() {
    recovery_over_transports(true, "reference_write_v1", true).await;
}

async fn recovery_over_transports(trusted: bool, mode: &str, previous_epoch: bool) {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    if trusted {
        admin.batch_execute("UPDATE awr_team.actors SET kind='system' WHERE id='agent';
            UPDATE awr_team.workstream_grants SET can_attest_execution=true WHERE client_id='cli-a'").await.unwrap();
    }
    let server = start(store).await;
    let a = connect(&server, "one", A).await.unwrap();
    let taken = call(&a,"awr_team_command",serde_json::to_value(command(&prepared(&a).await,"take","claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":"0","ttl_seconds":60}))).unwrap(),false).await;
    let claim = &taken["receipt"]["data"];
    let p = prepared(&a).await;
    let intent=call(&a,"awr_team_command",serde_json::to_value(command(&p,"intent","execution.prepare",
        json!({"session_id":"session-a","expected_session_version":"1","claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":"a".repeat(64),"declared_scope":["src"]}))).unwrap(),false).await;
    let e = &intent["receipt"]["data"];
    let p = prepared(&a).await;
    let request=serde_json::to_value(command(&p,"start","execution.start",json!({"session_id":"session-a",
        "expected_session_version":"1","execution_id":e["execution_id"],"expected_execution_version":"1",
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":mode,
        "expected_input_digest":"a".repeat(64)}))).unwrap();
    if !trusted {
        let mut denied = request.clone();
        denied["request_id"] = json!("denied-reference-mode");
        denied["args"]["execution_mode"] = json!("reference_write_v1");
        let response = http()
            .post(format!("{}/one/command", server.url))
            .bearer_auth(A)
            .json(&denied)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    let started: Value = http()
        .post(format!("{}/one/command", server.url))
        .bearer_auth(A)
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(started["execution_authorized"], true);
    assert_eq!(started["receipt"]["data"]["execution_mode"], mode);
    assert_eq!(started["receipt"]["data"]["input_digest"], "a".repeat(64));
    let replay = call(&a, "awr_team_command", request, false).await;
    assert_eq!(replay["receipt"], started["receipt"]);
    assert_eq!(replay["execution_authorized"], false);
    let report=serde_json::to_value(command(&prepared(&a).await,"report","execution.report",json!({"session_id":"session-a",
        "expected_session_version":"1","execution_id":e["execution_id"],"expected_execution_version":"2",
        "outcome":"succeeded","output_digest":"b".repeat(64),"observed_paths":["src/output"],"note":"Caller observed an output."}))).unwrap();
    let observed = call(&a, "awr_team_command", report.clone(), false).await;
    assert_eq!(observed["receipt"]["data"]["state"], "unknown");
    let retried: Value = http()
        .post(format!("{}/one/command", server.url))
        .bearer_auth(A)
        .json(&report)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retried["receipt"], observed["receipt"]);
    assert_eq!(retried["execution_authorized"], false);
    let inspect = call(
        &a,
        "awr_team_query",
        json!({"protocol_version":1,"op":"execution.inspect",
        "work_id":"a","execution_id":e["execution_id"]}),
        false,
    )
    .await;
    assert_eq!(inspect["data"]["recovery_blocked"], true);
    assert_eq!(inspect["data"]["automatic_resume"], false);
    let r=admin.query_one("SELECT (SELECT count(*) FROM awr_team.execution_receipts),
        (SELECT state FROM awr_team.resource_reservations LIMIT 1),(SELECT count(*) FROM awr_team.outbox)",&[]).await.unwrap();
    assert_eq!(r.get::<_, i64>(0), 1);
    assert_eq!(r.get::<_, String>(1), "unknown");
    assert_eq!(r.get::<_, i64>(2), 0);
    let facts = json!({"outcome":"succeeded","input_digest":"a".repeat(64),"output_digest":"b".repeat(64),
        "environment_digest":"c".repeat(64),"observed_paths":["src/output"],"note":"Reviewed the actual effect and its output."});
    let attestation = serde_json::to_value(command(&prepared(&a).await, "attest", "execution.attest",
        json!({"session_id":"session-a","expected_session_version":"1","execution_id":e["execution_id"],
        "expected_execution_version":"3","facts":facts}))).unwrap();
    let attested = call(&a, "awr_team_command", attestation.clone(), !trusted).await;
    if trusted {
        assert_eq!(
            attested["receipt"]["data"]["receipt_kind"],
            "trusted_executor"
        );
        assert_eq!(attested["receipt"]["data"]["recovery_blocked"], true);
        let replay: Value = http()
            .post(format!("{}/one/command", server.url))
            .bearer_auth(A)
            .json(&attestation)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(replay["receipt"], attested["receipt"]);
        assert_eq!(replay["execution_authorized"], false);
    }
    let inspected = call(
        &a,
        "awr_team_query",
        json!({"protocol_version":1,"op":"execution.inspect",
        "work_id":"a","execution_id":e["execution_id"]}),
        false,
    )
    .await;
    let p = prepared(&a).await;
    let denied = serde_json::to_value(command(&p,"unprivileged-recovery","execution.reconcile",
        json!({"session_id":"session-a","expected_session_version":"1","execution_id":e["execution_id"],
        "expected_execution_version":inspected["data"]["execution_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "reviewed_receipt_id":inspected["data"]["latest_receipt"]["receipt_id"],"clear_recovery_block":true,"facts":facts}))).unwrap();
    call(&a, "awr_team_command", denied, true).await;
    if previous_epoch {
        // Synthetic recovery boundary; this does not exercise physical database restore.
        admin.batch_execute("UPDATE awr_team.projects SET coordinator_epoch='restored-generation',project_revision=project_revision+1
            WHERE tenant_id='reader-tenant' AND id='reader-project';
            UPDATE awr_team.sessions SET state='interrupted',session_version=session_version+1 WHERE state='active';
            UPDATE awr_team.claims SET state='revoked',lease_version=lease_version+1 WHERE state='active';
            UPDATE awr_team.executions SET state='unknown',cancel_requested=true,execution_version=execution_version+1;
            UPDATE awr_team.work_runtime SET recovery_blocked=true,last_fence=last_fence+1,work_version=work_version+1;
            UPDATE awr_team.resource_reservations SET state='unknown' WHERE state='reserved'").await.unwrap();
    }
    const OP: &str =
        "awr1.operator.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
        VALUES('reader-tenant','operator','human','Operator','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','reader-project','operator','admin')",
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
        VALUES($1,'operator','operator','operator-cli',$2)",
            &[
                &TENANT,
                &awr_team_pg::workstream_credential_hash(OP).unwrap(),
            ],
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,
        can_read,can_write,can_manage,can_reconcile_execution) VALUES($1,$2,'operator','operator-cli',$3,1,true,true,true,true)",
        &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let op = connect(&server, "one", OP).await.unwrap();
    let session = call(
        &op,
        "awr_team_command",
        serde_json::to_value(command(
            &prepared(&op).await,
            "operator-session",
            "session.start",
            json!({"conversation_id":"recovery"}),
        ))
        .unwrap(),
        false,
    )
    .await;
    let viewed = call(
        &op,
        "awr_team_query",
        json!({"protocol_version":1,"op":"execution.inspect",
        "work_id":"a","execution_id":e["execution_id"]}),
        false,
    )
    .await;
    assert_eq!(viewed["data"]["reconciliation_authority"], true);
    assert_eq!(
        viewed["data"]["previous_epoch_review_required"],
        previous_epoch
    );
    assert_eq!(
        viewed["data"]["previous_epoch_recovery_available"],
        previous_epoch
    );
    assert_eq!(viewed["data"]["execution_coordinator_epoch"], "epoch-a");
    let p = prepared(&op).await;
    let mut request = serde_json::to_value(command(&p,"operator-recovery","execution.reconcile",
        json!({"session_id":session["receipt"]["data"]["session_id"],"expected_session_version":"1","execution_id":e["execution_id"],
        "expected_execution_version":viewed["data"]["execution_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "reviewed_receipt_id":viewed["data"]["latest_receipt"]["receipt_id"],"clear_recovery_block":true,"facts":facts}))).unwrap();
    if previous_epoch {
        let denied = http()
            .post(format!("{}/one/command", server.url))
            .bearer_auth(OP)
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), 409);
        assert_eq!(
            denied.json::<Value>().await.unwrap()["code"],
            "EpochChanged"
        );
        request["args"]["previous_epoch_recovery"] = json!({
            "execution_epoch":viewed["data"]["execution_coordinator_epoch"],
            "executor_stopped":true,"review_reference":"fixture:reviewed-restore-barrier"});
    }
    let settled: Value = http()
        .post(format!("{}/one/command", server.url))
        .bearer_auth(OP)
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settled["receipt"]["data"]["receipt_kind"], "reconcile");
    assert_eq!(settled["receipt"]["data"]["recovery_blocked"], false);
    assert_eq!(settled["receipt"]["data"]["work_completed"], false);
    assert_eq!(
        settled["receipt"]["data"]["previous_epoch_reconciled"],
        previous_epoch
    );
    assert_eq!(
        settled["receipt"]["data"]["execution_coordinator_epoch"],
        "epoch-a"
    );
    assert_eq!(
        settled["receipt"]["data"]["reporting_coordinator_epoch"],
        if previous_epoch {
            "restored-generation"
        } else {
            "epoch-a"
        }
    );
    let replay = call(&op, "awr_team_command", request, false).await;
    assert_eq!(replay["receipt"], settled["receipt"]);
    assert_eq!(replay["execution_authorized"], false);
    op.cancel().await.unwrap();
    a.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_session_journal_reconnects_replays_and_shares_http_outcomes() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let a = connect(&server, "one", A).await.unwrap();
    let b = connect(&server, "one", B).await.unwrap();
    let prep = prepared(&a).await;
    let request = serde_json::to_value(command(
        &prep,
        "mcp-start",
        "session.start",
        json!({"conversation_id":"sdk-conversation"}),
    ))
    .unwrap();
    let started = call(&a, "awr_team_command", request.clone(), false).await;
    let id = started["receipt"]["data"]["session_id"].as_str().unwrap();
    a.cancel().await.unwrap(); // MCP connection lifetime is not durable session lifetime.
    let a = connect(&server, "one", A).await.unwrap();
    let inspected = call(
        &a,
        "awr_team_query",
        json!({"protocol_version":1,"op":"command.inspect","work_id":"a","request_id":"mcp-start"}),
        false,
    )
    .await;
    assert_eq!(inspected["data"]["receipt"], started["receipt"]);
    let replay = call(&a, "awr_team_command", request.clone(), false).await;
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], started["receipt"]);
    let http_replay: Value = http()
        .post(format!("{}/one/command", server.url))
        .bearer_auth(A)
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(http_replay, replay);
    let mut changed = request.clone();
    changed["args"]["conversation_id"] = json!("different-intent");
    assert_eq!(
        call(&a, "awr_team_command", changed, true).await["code"],
        "IdempotencyConflict"
    );
    assert_eq!(
        call(&b, "awr_team_command", request, true).await["code"],
        "Forbidden"
    );
    let prep = prepared(&a).await;
    let checkpoint=serde_json::to_value(command(&prep,"mcp-checkpoint","session.checkpoint",json!({"session_id":id,
        "expected_session_version":"1","context_hash":prep["data"]["context_hash"],"next_action":"Review the SDK integration","open_loops":["business acceptance pending"]}))).unwrap();
    let mut mismatch = checkpoint.clone();
    mismatch["args"]["context_hash"] = json!("0".repeat(64));
    assert_eq!(
        call(&a, "awr_team_command", mismatch, true).await["code"],
        "PreconditionsChanged"
    );
    call(&a, "awr_team_command", checkpoint, false).await;
    let session = call(
        &a,
        "awr_team_query",
        json!({"protocol_version":1,"op":"session.inspect","session_id":id}),
        false,
    )
    .await;
    assert_eq!(
        session["data"]["items"][0]["next_action"],
        "Review the SDK integration"
    );
    let end = serde_json::to_value(command(
        &prepared(&a).await,
        "mcp-end",
        "session.end",
        json!({"session_id":id,"expected_session_version":"2"}),
    ))
    .unwrap();
    call(&a, "awr_team_command", end, false).await;
    let unknown=call(&a,"awr_team_query",json!({"protocol_version":1,"op":"command.inspect","work_id":"a","request_id":"not-submitted"}),false).await;
    assert_eq!(unknown["data"]["state"], "unknown");
    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.operations WHERE client_id='cli-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 3);
    a.cancel().await.unwrap();
    b.cancel().await.unwrap();
}

#[tokio::test]
async fn initialized_clients_recheck_revocation_for_tools_and_discovery() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(store).await;
    let a = connect(&server, "one", A).await.unwrap();
    let b = connect(&server, "one", B).await.unwrap();
    let request = serde_json::to_value(command(
        &prepared(&a).await,
        "reader-cannot-write",
        "session.start",
        json!({"conversation_id":"reader"}),
    ))
    .unwrap();
    assert_eq!(
        call(&a, "awr_team_command", request, true).await["code"],
        "Forbidden"
    );
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(a.list_all_tools().await.is_err());
    assert!(
        a.call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"work.list"})
                    .as_object()
                    .unwrap()
                    .clone()
            )
        )
        .await
        .is_err()
    );
    assert!(b.list_all_tools().await.is_ok());
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_read=false,grant_version=grant_version+1 WHERE client_id='cli-b'").await.unwrap();
    assert!(b.list_all_tools().await.is_err());
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.operations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
    a.cancel().await.unwrap();
    b.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_rejects_forged_identity_unsupported_operations_and_context_truncation() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let server = start(store).await;
    let a = connect(&server, "one", A).await.unwrap();
    for field in [
        "tenant_id",
        "project_id",
        "actor_id",
        "client_id",
        "grants",
        "project",
    ] {
        let mut query = json!({"protocol_version":1,"op":"capabilities"});
        query[field] = json!("forged");
        // Authority forgeries are denied by the perimeter before strict decoding;
        // an unrelated unknown field still fails the input schema.
        let expected = if field == "project" {
            "InvalidInput"
        } else {
            "Forbidden"
        };
        assert_eq!(
            call(&a, "awr_team_query", query, true).await["code"],
            expected
        );
        let mut c = serde_json::to_value(command(
            &prepared(&a).await,
            "forged",
            "session.start",
            json!({"conversation_id":"fake"}),
        ))
        .unwrap();
        c[field] = json!("forged");
        assert_eq!(
            call(&a, "awr_team_command", c, true).await["code"],
            expected
        );
    }
    for op in ["execution.dispatch", "work.unimplemented", "claim.transfer"] {
        let c = serde_json::to_value(command(&prepared(&a).await, "unimplemented", op, json!({})))
            .unwrap();
        assert_eq!(
            call(&a, "awr_team_command", c, true).await["code"],
            "Unsupported"
        );
    }
    assert_eq!(
        call(
            &a,
            "awr_team_query",
            json!({"protocol_version":2,"op":"capabilities"}),
            true
        )
        .await["code"],
        "Unsupported"
    );
    assert_eq!(
        call(
            &a,
            "awr_team_query",
            json!({"protocol_version":1,"op":"work.prepare","work_id":"a","max_context_bytes":1}),
            true
        )
        .await["code"],
        "ContextIncomplete"
    );
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.operations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
    a.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_http_perimeter_bounds_bodies_and_refuses_sticky_transport_authority() {
    let (_guard, _, _, store) = setup().await;
    let server = start(store).await;
    let request = rpc("tools/list", json!({}));
    let response = raw(&server, A).json(&request).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.headers().get("mcp-session-id").is_none());
    for (header, value) in [("host", "attacker.invalid"), ("origin", "http://localhost")] {
        assert_eq!(
            raw(&server, A)
                .header(header, value)
                .json(&request)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(
        raw(&server, A)
            .header("authorization", format!("Bearer {B}"))
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        raw(&server, "invalid")
            .header("mcp-session-id", "copied-session")
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        raw(&server, A)
            .header("content-type", "application/json")
            .body("x".repeat(65537))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    let bad = raw(&server, A)
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .unwrap();
    // RMCP reports an undecodable JSON body as unsupported media (415).
    assert_eq!(bad.status(), 415);
    assert!(!bad.text().await.unwrap().contains(A));
    let methods = raw(&server, A)
        .json(&rpc(
            "tools/call",
            json!({"name":"awr_work_complete","arguments":{}}),
        ))
        .send()
        .await
        .unwrap();
    let result: Value = methods.json().await.unwrap();
    assert!(result.get("error").is_some() || result["result"]["isError"] == true);
}

#[tokio::test]
async fn mcp_envelope_limit_never_returns_a_partial_large_checkpoint() {
    let (_guard, admin, _, store) = setup().await;
    // Operator/imported historical data can exceed today's journal write limit.
    let long = "PRIVATE_LARGE_CONTENT".repeat(30000);
    admin
        .execute(
            "UPDATE awr_team.checkpoints SET next_action=$1 WHERE id='cp-session-a'",
            &[&long],
        )
        .await
        .unwrap();
    let server = start(store).await;
    let result=raw(&server,A).json(&rpc("tools/call",json!({"name":"awr_team_query","arguments":{"protocol_version":1,"op":"session.inspect","session_id":"session-a"}}))).send().await.unwrap();
    assert_eq!(result.status(), 409);
    assert_eq!(result.headers()["cache-control"], "no-store");
    let body = result.text().await.unwrap();
    assert!(body.contains("ResponseTooLarge"));
    assert!(!body.contains("PRIVATE_LARGE_CONTENT"));
}
