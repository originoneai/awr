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
    let output = tokio::process::Command::new(binary)
        .env("AWR_TEAM_ENABLED", "1")
        .env("AWR_CLI_PARITY_TOKEN", A)
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
    assert!(!String::from_utf8_lossy(&bytes).contains(A));
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
        .auth_header(A),
    );
    let client = ().serve(transport).await.unwrap();
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
