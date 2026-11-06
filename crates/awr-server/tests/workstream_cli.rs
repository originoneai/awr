#![cfg(feature = "pg-tests")]
//! Native CLI versus authenticated MCP, using the process-owned PostgreSQL fixture.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;

use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
};
use awr_server::service::{ProjectBinding, ServiceConfig};
use fixture::*;
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn connect(
    server: &Server,
    token: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let transport = StreamableHttpClientTransport::with_client(
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap(),
        StreamableHttpClientTransportConfig::with_uri(format!(
            "{}/v1/projects/fixture/mcp",
            server.url
        ))
        .auth_header(token),
    );
    ().serve(transport).await.unwrap()
}

fn build_cli() -> PathBuf {
    let output =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args([
                "build",
                "-p",
                "awr-cli",
                "--locked",
                "--message-format=json",
            ])
            .current_dir(format!("{}/../..", env!("CARGO_MANIFEST_DIR")))
            .output()
            .expect("build the native CLI for this fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|message| {
            (message["reason"] == "compiler-artifact" && message["target"]["name"] == "awr")
                .then(|| message["executable"].as_str().map(PathBuf::from))
                .flatten()
        })
        .expect("Cargo must report the executable, including custom target directories")
}

async fn start(store: awr_team_pg::WorkstreamReadStore) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec![],
        allowed_web_origins: vec![],
        oauth: None,
        projects: vec![ProjectBinding {
            key: "fixture".into(),
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
        }],
    };
    let router = awr_server::service::router(config, address, store).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        url: format!("http://{address}"),
        task,
    }
}

fn project(server: &Server) -> Project {
    let root = std::env::temp_dir().join(format!("awr-cli-pg-{}", common::nonce(0)));
    std::fs::create_dir_all(root.join(".awr/team-remotes")).unwrap();
    let profile = awr_team::RemoteProfile {
        name: "fixture".into(),
        endpoint: server.url.clone(),
        project_key: "fixture".into(),
        credential_env: "AWR_CLI_PARITY_TOKEN".into(),
        protocol_version: 1,
    };
    std::fs::write(
        root.join(".awr/team-remotes/fixture.toml"),
        toml::to_string(&profile).unwrap(),
    )
    .unwrap();
    Project(root)
}

async fn cli(binary: &PathBuf, project: &Project, args: &[&str], expected_exit: i32) -> Value {
    cli_as(binary, project, A, args, expected_exit).await
}

async fn cli_as(
    binary: &PathBuf,
    project: &Project,
    token: &str,
    args: &[&str],
    expected_exit: i32,
) -> Value {
    let output = tokio::process::Command::new(binary)
        .env("AWR_TEAM_ENABLED", "1")
        .env("AWR_CLI_PARITY_TOKEN", token)
        .env("NO_PROXY", "*")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .arg("--project")
        .arg(&project.0)
        .args(["--json", "team"])
        .args(args)
        .args(["--remote", "fixture"])
        .output()
        .await
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(expected_exit),
        "{args:?}: {output:?}"
    );
    let bytes = if expected_exit == 0 {
        assert!(output.stderr.is_empty());
        output.stdout
    } else {
        assert!(output.stdout.is_empty());
        output.stderr
    };
    let result: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(token));
    assert!(!project.0.join(".awr/state.db").exists());
    result
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn assert_same_facts(mut cli: Value, mut mcp: Value, start: i64, end: i64) {
    // Only these measured read times vary. Never remove contracts, hashes,
    // revisions, ownership, permissions, context gaps or entire observation blocks.
    for path in [
        "/data/observed_at_unix_ms",
        "/data/collaboration/source_state/query_started_at_unix_ms",
        "/data/snapshot/queried_at_unix_ms",
        "/data/observation/observed_at_unix_ms",
        "/data/observation/collaboration/source_state/query_started_at_unix_ms",
    ] {
        assert_eq!(
            cli.pointer(path).is_some(),
            mcp.pointer(path).is_some(),
            "{path}"
        );
        for result in [&mut cli, &mut mcp] {
            if let Some(time) = result.pointer_mut(path) {
                let measured = time.as_i64().expect("observed read time");
                assert!(
                    (start - 2000..=end + 2000).contains(&measured),
                    "{path}: {measured}"
                );
                *time = json!(0);
            }
        }
    }
    assert_eq!(cli, mcp);
}

async fn business_state(admin: &tokio_postgres::Client) -> BTreeMap<String, Value> {
    let mut state = BTreeMap::new();
    for row in admin.query("SELECT tablename FROM pg_tables WHERE schemaname='awr_team' AND tablename <> 'request_audit' ORDER BY tablename",&[]).await.unwrap() {
        let table: String = row.get(0);
        assert!(table.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        let value: Value = admin.query_one(&format!("SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb) FROM awr_team.\"{table}\" t"),&[]).await.unwrap().get(0);
        state.insert(table,value);
    }
    state
}

async fn task_read_authorization(admin: &tokio_postgres::Client, db: &str) {
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES('reader-tenant','reader-project','alice','Alice','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','bind-agent','alice','agent','active')").await.unwrap();
    let authorization = AgentAuthorization {
        id: "cli-task-read".into(),
        authorizer_person_id: PersonId::new("alice").unwrap(),
        responsible_person_id: PersonId::new("alice").unwrap(),
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
        binding_id: Some("bind-agent".into()),
    };
    awr_team_pg::AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "cli-task-read".into(),
                authorization,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "The process-owned database fixture stays guarded for this entire single-thread test runtime."
)]
async fn native_team_queries_match_mcp_without_business_mutations() {
    let binary = build_cli();
    let (_guard, admin, db, store) = setup().await;
    let commands = store.commands();
    let server = start(store).await;
    let project = project(&server);
    let client = connect(&server, A).await;
    let before = business_state(&admin).await;
    let cases: [(&str, Vec<&str>, Value); 9] = [
        ("capabilities", vec!["capabilities"], json!({})),
        ("workstreams.list", vec!["workstream", "list"], json!({})),
        ("work.next", vec!["work", "next"], json!({})),
        (
            "work.inbox",
            vec!["work", "inbox", "--limit", "1"],
            json!({"limit":1}),
        ),
        (
            "work.list",
            vec!["work", "list", "--limit", "1"],
            json!({"limit":1}),
        ),
        (
            "work.search",
            vec!["work", "search", "a"],
            json!({"search":"a"}),
        ),
        (
            "work.prepare",
            vec!["work", "prepare", "a"],
            json!({"work_id":"a"}),
        ),
        (
            "work.snapshot",
            vec!["work", "snapshot", "c"],
            json!({"work_id":"c"}),
        ),
        (
            "work.observe",
            vec!["work", "observe", "a", "--session", "session-a"],
            json!({"work_id":"a","session_id":"session-a"}),
        ),
    ];
    for (op, args, extra) in cases {
        let mut query = json!({"protocol_version":1,"op":op});
        query
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let start = now_ms();
        let actual = cli(&binary, &project, &args, 0).await;
        let result = client
            .call_tool(
                CallToolRequestParams::new("awr_team_query")
                    .with_arguments(query.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{op}: {result:?}");
        if op == "work.prepare" {
            assert_eq!(actual["data"]["context_complete"], true);
        }
        if op == "work.snapshot" {
            assert_eq!(actual["data"]["context_complete"], false);
            assert!(actual["data"]["observation"]["runtime"].is_null());
            assert!(actual["data"]["observation"]["session"].is_null());
        }
        if op == "work.observe" {
            assert_eq!(
                actual["data"]["checkpoint"]["next_action"],
                "continue alpha"
            );
            assert!(actual["data"]["runtime"].is_null());
        }
        assert_same_facts(actual, result.structured_content.unwrap(), start, now_ms());
    }
    let empty = cli(&binary, &project, &["work", "search", "no-such-title"], 0).await;
    assert_eq!(empty["data"]["items"], json!([]));
    assert_eq!(empty["data"]["total"], 0);
    assert!(empty["data"]["next_cursor"].is_null());
    assert_eq!(
        business_state(&admin).await,
        before,
        "queries must only add access audit metadata"
    );
    let audits: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.request_audit", &[])
        .await
        .unwrap()
        .get(0);
    assert!(audits >= 18, "both transports must be audited");

    for (args, query) in [
        (
            vec!["work", "prepare", "b-private"],
            json!({"protocol_version":1,"op":"work.prepare","work_id":"b-private"}),
        ),
        (
            vec!["work", "prepare", "absent"],
            json!({"protocol_version":1,"op":"work.prepare","work_id":"absent"}),
        ),
        (
            vec!["work", "prepare", "a", "--max-context-bytes", "1"],
            json!({"protocol_version":1,"op":"work.prepare","work_id":"a","max_context_bytes":1}),
        ),
        (
            vec!["work", "observe", "a", "--session", "session-b"],
            json!({"protocol_version":1,"op":"work.observe","work_id":"a","session_id":"session-b"}),
        ),
    ] {
        let actual = cli(&binary, &project, &args, 1).await;
        let result = client
            .call_tool(
                CallToolRequestParams::new("awr_team_query")
                    .with_arguments(query.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(actual, result.structured_content.unwrap());
    }

    let stream = awr_core::Id::from(1).to_string();
    let first = cli(
        &binary,
        &project,
        &["work", "list", "--workstream", &stream, "--limit", "1"],
        0,
    )
    .await;
    let cursor = first["data"]["next_cursor"].as_str().unwrap();
    let second = cli(
        &binary,
        &project,
        &[
            "work",
            "list",
            "--workstream",
            &stream,
            "--limit",
            "1",
            "--cursor",
            cursor,
        ],
        0,
    )
    .await;
    assert_ne!(first["data"]["items"], second["data"]["items"]);
    assert!(second["data"]["next_cursor"].is_null());
    admin.execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'",&[]).await.unwrap();
    let expired = cli(
        &binary,
        &project,
        &[
            "work",
            "list",
            "--workstream",
            &stream,
            "--limit",
            "1",
            "--cursor",
            cursor,
        ],
        1,
    )
    .await;
    assert_eq!(expired["code"], "CursorExpired");

    // A real claim makes the first scanned task produce no inbox item. The CLI
    // must return that empty page and its cursor rather than inventing an end.
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET assignment_grant=true WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let prepared = cli(&binary, &project, &["work", "prepare", "a"], 0).await;
    commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "cli-inbox-claim",
                "task.claim_available",
                json!({"session_id":"session-a","expected_session_version":"1",
                    "expected_responsibility_version":prepared["data"]["responsibility"]["version"],
                    "expected_work_version":"0","ttl_seconds":3600}),
            ),
        )
        .await
        .unwrap();
    let claimed = business_state(&admin).await;
    let first = cli(&binary, &project, &["work", "inbox", "--limit", "1"], 0).await;
    assert_eq!(first["data"]["items"], json!([]));
    let cursor = first["data"]["next_cursor"].as_str().unwrap();
    let second = cli(
        &binary,
        &project,
        &["work", "inbox", "--limit", "1", "--cursor", cursor],
        0,
    )
    .await;
    assert_eq!(second["data"]["items"][0]["work_id"], "c");
    assert!(second["data"]["next_cursor"].is_null());
    let recorded = cli(&binary, &project, &["work", "observe", "a"], 0).await;
    assert!(recorded["data"]["runtime"].is_object());
    assert_eq!(recorded["data"]["claim"]["lease_live"], true);
    assert_eq!(
        recorded["data"]["checkpoint"]["next_action"],
        "continue alpha"
    );
    assert_eq!(business_state(&admin).await, claimed);

    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read) VALUES($1,$2,'agent','cli-a',$3,1,true)",
        &[&TENANT,&PROJECT,&awr_core::Id::from(2).to_string()]).await.unwrap();
    let ambiguous = cli(&binary, &project, &["work", "list"], 1).await;
    assert_eq!(ambiguous["code"], "WorkstreamScopeRequired");
    assert_eq!(
        cli(
            &binary,
            &project,
            &["work", "list", "--workstream", &stream],
            0
        )
        .await["data"]["total"],
        2
    );

    // Broad credential access cannot turn a single-task Agent delegation into
    // stream discovery or sibling access, and the CLI adds no discovery preflight.
    task_read_authorization(&admin, &db).await;
    let scoped = business_state(&admin).await;
    let next = cli(&binary, &project, &["work", "next"], 0).await;
    assert_eq!(next["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["data"]["items"][0]["work_id"], "a");
    for args in [
        vec!["capabilities"],
        vec!["workstream", "list"],
        vec!["work", "list", "--workstream", &stream],
        vec!["work", "prepare", "c"],
    ] {
        assert_eq!(cli(&binary, &project, &args, 1).await["code"], "Forbidden");
    }
    assert_eq!(
        cli(&binary, &project, &["work", "prepare", "a"], 0).await["data"]["work_id"],
        "a"
    );
    assert_eq!(business_state(&admin).await, scoped);
    assert!(!project.0.join(".awr/state.db").exists());
    client.cancel().await.unwrap();
}

async fn runtime_command(
    store: &awr_team_pg::WorkstreamReadStore,
    request: &str,
    op: &str,
    args: Value,
) -> Value {
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(&prepare(store, A, "a").await, request, op, args),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "The process-owned database fixture stays guarded for this entire single-thread test runtime."
)]
async fn native_runtime_inspections_match_mcp_for_active_terminal_unknown_and_mismatched_records() {
    let binary = build_cli();
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let started = runtime_command(
        &store,
        "cli-session-start",
        "session.start",
        json!({"conversation_id":"cli-runtime-reads"}),
    )
    .await;
    let session = started["receipt"]["data"]["session_id"].as_str().unwrap();
    let taken = runtime_command(&store, "cli-claim", "claim.acquire", json!({
        "session_id":session,"expected_session_version":"1","expected_work_version":"0","ttl_seconds":3600
    })).await;
    let claim = &taken["receipt"]["data"];
    let claim_id = claim["claim_id"].as_str().unwrap();
    let intent = runtime_command(&store, "cli-execution-prepare", "execution.prepare", json!({
        "session_id":session,"expected_session_version":"1","claim_id":claim_id,
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":claim["work_version"],"input_digest":"a".repeat(64),"declared_scope":["src"]
    })).await;
    let execution = intent["receipt"]["data"]["execution_id"].as_str().unwrap();
    let server = start(awr_team_pg::WorkstreamReadStore::from_config(
        common::with_app_role(&common::test_config(), &db),
    ))
    .await;
    let project = project(&server);
    let client = connect(&server, A).await;

    for terminal in [false, true] {
        if terminal {
            let cancelled = runtime_command(&store, "cli-cancel", "execution.cancel", json!({
                "session_id":session,"expected_session_version":"1","execution_id":execution,"expected_execution_version":"1"
            })).await;
            assert_eq!(cancelled["receipt"]["data"]["stop_confirmed"], true);
            runtime_command(
                &store,
                "cli-release",
                "claim.release",
                json!({
                    "session_id":session,"expected_session_version":"1","claim_id":claim_id,
                    "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"]
                }),
            )
            .await;
            runtime_command(
                &store,
                "cli-session-end",
                "session.end",
                json!({
                    "session_id":session,"expected_session_version":"1"
                }),
            )
            .await;
        }
        let before = business_state(&admin).await;
        for (op, args, extra) in [
            (
                "session.inspect",
                vec!["session", "inspect", session],
                json!({"session_id":session}),
            ),
            (
                "claim.inspect",
                vec![
                    "claim",
                    "inspect",
                    claim_id,
                    "--work",
                    "a",
                    "--session",
                    session,
                ],
                json!({"work_id":"a","session_id":session,"claim_id":claim_id}),
            ),
            (
                "execution.inspect",
                vec!["execution", "inspect", execution, "--session", session],
                json!({"session_id":session,"execution_id":execution}),
            ),
            (
                "command.inspect",
                vec!["command", "inspect", "cli-session-start", "--work", "a"],
                json!({"work_id":"a","request_id":"cli-session-start"}),
            ),
            (
                "work.recovery",
                vec!["work", "recovery", "a", "--session", session],
                json!({"work_id":"a","session_id":session}),
            ),
            (
                "work.recovery",
                vec!["work", "recovery", "c"],
                json!({"work_id":"c"}),
            ),
            (
                "command.inspect",
                vec!["command", "inspect", "missing-request", "--work", "a"],
                json!({"work_id":"a","request_id":"missing-request"}),
            ),
        ] {
            let mut query = json!({"protocol_version":1,"op":op});
            query
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let start = now_ms();
            let actual = cli(&binary, &project, &args, 0).await;
            let result = client
                .call_tool(
                    CallToolRequestParams::new("awr_team_query")
                        .with_arguments(query.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            assert!(!result.is_error.unwrap_or(false), "{op}: {result:?}");
            match op {
                "session.inspect" => {
                    assert_eq!(actual["data"]["items"][0]["session_id"], session);
                    assert_eq!(
                        actual["data"]["items"][0]["state"],
                        if terminal { "ended" } else { "active" }
                    );
                    assert!(actual["data"]["items"][0]["checkpoint_id"].is_null());
                }
                "claim.inspect" => {
                    assert_eq!(
                        actual["data"]["state"],
                        if terminal { "released" } else { "active" }
                    );
                    assert_eq!(actual["data"]["lease_live"], !terminal);
                    assert_eq!(actual["data"]["execution_authorized"], false);
                }
                "execution.inspect" => {
                    assert_eq!(
                        actual["data"]["state"],
                        if terminal { "cancelled" } else { "prepared" }
                    );
                    assert_eq!(actual["data"]["artifact_verified"], false);
                    assert!(actual["data"]["latest_receipt"].is_null());
                }
                "command.inspect" => {
                    if extra["request_id"] == "missing-request" {
                        assert_eq!(actual["data"]["state"], "unknown");
                    } else {
                        assert_eq!(actual["data"]["state"], "committed");
                        assert_eq!(actual["data"]["receipt"], started["receipt"]);
                    }
                }
                "work.recovery" => {
                    assert_eq!(actual["data"]["automatic_resume"], false);
                    assert_eq!(actual["data"]["checkpoint_actions_are_historical"], true);
                    assert_eq!(
                        actual["data"]["items"].as_array().unwrap().len(),
                        if extra["work_id"] == "c" { 0 } else { 1 }
                    );
                }
                _ => unreachable!(),
            }
            assert_same_facts(actual, result.structured_content.unwrap(), start, now_ms());
        }
        assert_eq!(
            business_state(&admin).await,
            before,
            "runtime inspections must only add audit metadata"
        );
    }
    let before = business_state(&admin).await;
    for (op, args, extra) in [
        (
            "session.inspect",
            vec!["session", "inspect", session, "--work", "c"],
            json!({"work_id":"c","session_id":session}),
        ),
        (
            "claim.inspect",
            vec!["claim", "inspect", claim_id, "--work", "c"],
            json!({"work_id":"c","claim_id":claim_id}),
        ),
        (
            "execution.inspect",
            vec![
                "execution",
                "inspect",
                execution,
                "--work",
                "a",
                "--session",
                "session-b",
            ],
            json!({"work_id":"a","session_id":"session-b","execution_id":execution}),
        ),
        (
            "command.inspect",
            vec!["command", "inspect", "cli-session-start", "--work", "c"],
            json!({"work_id":"c","request_id":"cli-session-start"}),
        ),
        (
            "work.recovery",
            vec!["work", "recovery", "c", "--session", session],
            json!({"work_id":"c","session_id":session}),
        ),
    ] {
        let mut query = json!({"protocol_version":1,"op":op});
        query
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let actual = cli(&binary, &project, &args, 1).await;
        let result = client
            .call_tool(
                CallToolRequestParams::new("awr_team_query")
                    .with_arguments(query.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{op}: {result:?}");
        assert!(actual["code"].is_string());
        assert_eq!(actual, result.structured_content.unwrap());
    }
    assert_eq!(business_state(&admin).await, before);
    client.cancel().await.unwrap();
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "The process-owned database fixture stays guarded for this entire single-thread test runtime."
)]
async fn native_runtime_reads_match_mcp_for_receipt_visibility_and_unauthorized_clients() {
    let binary = build_cli();
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let started = runtime_command(
        &store,
        "cli-permission-session",
        "session.start",
        json!({"conversation_id":"cli-runtime-permissions"}),
    )
    .await;
    let session = started["receipt"]["data"]["session_id"].as_str().unwrap();
    let taken = runtime_command(&store, "cli-permission-claim", "claim.acquire", json!({
        "session_id":session,"expected_session_version":"1","expected_work_version":"0","ttl_seconds":3600
    })).await;
    let claim = &taken["receipt"]["data"];
    let claim_id = claim["claim_id"].as_str().unwrap();
    let intent = runtime_command(&store, "cli-permission-prepare", "execution.prepare", json!({
        "session_id":session,"expected_session_version":"1","claim_id":claim_id,
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":claim["work_version"],"input_digest":"a".repeat(64),"declared_scope":["src"]
    })).await;
    let execution = intent["receipt"]["data"]["execution_id"].as_str().unwrap();
    let running = runtime_command(&store, "cli-permission-start", "execution.start", json!({
        "session_id":session,"expected_session_version":"1","execution_id":execution,
        "expected_execution_version":intent["receipt"]["data"]["execution_version"],
        "claim_id":claim_id,"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":prepare(&store, A, "a").await["data"]["runtime"]["work_version"],
        "execution_mode":"caller_managed"
    })).await;
    assert_eq!(running["execution_authorized"], true);
    let private_note = "Synthetic private execution receipt for CLI permission checks.";
    let reported = runtime_command(&store, "cli-permission-report", "execution.report", json!({
        "session_id":session,"expected_session_version":"1","execution_id":execution,
        "expected_execution_version":running["receipt"]["data"]["execution_version"],
        "outcome":"succeeded","output_digest":"b".repeat(64),"observed_paths":["src/result.json"],
        "note":private_note
    })).await;
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-b',$3,1,true)", &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let server = start(awr_team_pg::WorkstreamReadStore::from_config(
        common::with_app_role(&common::test_config(), &db),
    ))
    .await;
    let project = project(&server);
    let owner = connect(&server, A).await;
    let peer = connect(&server, B).await;
    for (token, client, allowed) in [(A, &owner, true), (B, &peer, true), (B, &peer, false)] {
        if !allowed {
            // Keep the peer's unrelated stream grant so MCP remains connected;
            // the established connection must recheck access to this work.
            let revoked = admin
                .execute(
                    "DELETE FROM awr_team.workstream_grants WHERE tenant_id=$1 AND project_id=$2
                     AND client_id='cli-b' AND workstream_id=$3",
                    &[&TENANT, &PROJECT, &awr_core::Id::from(1).to_string()],
                )
                .await
                .unwrap();
            assert_eq!(revoked, 1);
        }
        let before = business_state(&admin).await;
        for (op, args, extra) in [
            (
                "session.inspect",
                vec!["session", "inspect", session, "--work", "a"],
                json!({"session_id":session}),
            ),
            (
                "claim.inspect",
                vec!["claim", "inspect", claim_id, "--work", "a"],
                json!({"claim_id":claim_id}),
            ),
            (
                "execution.inspect",
                vec!["execution", "inspect", execution, "--work", "a"],
                json!({"execution_id":execution}),
            ),
            (
                "command.inspect",
                vec!["command", "inspect", "cli-permission-report", "--work", "a"],
                json!({"request_id":"cli-permission-report"}),
            ),
            ("work.recovery", vec!["work", "recovery", "a"], json!({})),
        ] {
            let mut query = json!({"protocol_version":1,"op":op,"work_id":"a"});
            query
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let start = now_ms();
            let actual = cli_as(&binary, &project, token, &args, i32::from(!allowed)).await;
            let result = client
                .call_tool(
                    CallToolRequestParams::new("awr_team_query")
                        .with_arguments(query.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            if !allowed {
                assert_eq!(result.is_error, Some(true), "{op}: {result:?}");
                assert_eq!(actual["code"], "Forbidden");
                assert!(actual.get("data").is_none());
                assert!(!actual.to_string().contains(private_note));
                assert_eq!(actual, result.structured_content.unwrap());
                continue;
            }
            assert!(!result.is_error.unwrap_or(false), "{op}: {result:?}");
            match op {
                "claim.inspect" => {
                    assert_eq!(actual["data"]["owned_by_client"], token == A);
                }
                "execution.inspect" => {
                    assert_eq!(actual["data"]["owned_by_client"], token == A);
                    assert_eq!(actual["data"]["receipt_details_available"], token == A);
                    assert_eq!(actual["data"]["terminal_reported"], true);
                    if token == A {
                        assert_eq!(
                            actual["data"]["latest_receipt"]["receipt_id"],
                            reported["receipt"]["data"]["receipt_id"]
                        );
                        assert_eq!(
                            actual["data"]["latest_receipt"]["payload"]["note"],
                            private_note
                        );
                    } else {
                        assert_eq!(actual["data"].get("latest_receipt"), Some(&Value::Null));
                    }
                }
                "command.inspect" => {
                    if token == A {
                        assert_eq!(actual["data"]["state"], "committed");
                        assert_eq!(actual["data"]["receipt"], reported["receipt"]);
                    } else {
                        assert_eq!(actual["data"]["state"], "unknown");
                        assert_eq!(actual["data"]["request_id"], "cli-permission-report");
                        assert!(actual["data"].get("receipt").is_none());
                    }
                }
                _ => {}
            }
            if token == B {
                assert!(!actual.to_string().contains(private_note));
            }
            assert_same_facts(actual, result.structured_content.unwrap(), start, now_ms());
        }
        assert_eq!(
            business_state(&admin).await,
            before,
            "permission checks and receipt inspections must only add audit metadata"
        );
    }
    owner.cancel().await.unwrap();
    peer.cancel().await.unwrap();
}
