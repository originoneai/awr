#![cfg(feature = "pg-tests")]
//! AWR-PPD-061: capabilities publishes every business duty's action ceiling
//! from the shared `BusinessRole::action_ceiling` that the authorization path
//! intersects, so the displayed definitions and the enforced policy are one
//! value. The verbatim web reply is saved under tests/samples and re-parsed by
//! every run; the observer bridge tests consume the same committed sample.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/oauth_http.rs"]
mod web;

use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team_pg::{AdminAccessPlan, WorkstreamReadStore, workstream_credential_hash};
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

type McpClient = RunningService<RoleClient, ()>;

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
    store.check_schema().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec!["awr.example".into()],
        allowed_web_origins: vec![web::ISSUER.into()],
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
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        url: format!("http://{address}/v1"),
        task,
    }
}

async fn mcp(base: &str, token: &str) -> McpClient {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .timeout(Duration::from_secs(35))
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(format!("{base}/projects/one/mcp")),
    );
    ().serve(transport).await.unwrap()
}

async fn caps(client: &McpClient) -> Value {
    client
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"capabilities"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap()
}

fn member_plan() -> AdminAccessPlan {
    let token = "awr1.mcp-member.2222222222222222222222222222222222222222222222222222222222222222";
    serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"mcp-human","kind":"human","display_name":"MCP member"},
        "subject_client_id":"mcp-cli",
        "role":"developer",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write":true,"manage":false,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"mcp-member",
            "secret_hash":workstream_credential_hash(token).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap()
}

const MEMBER_TOKEN: &str =
    "awr1.mcp-member.2222222222222222222222222222222222222222222222222222222222222222";

/// The exact ceiling of each of the six duties, action by action, in the
/// serialized order of `BusinessRole::action_ceiling`. One extra or one
/// missing action in any list fails the assertions that compare against this
/// table, and so does any pairwise swap between two duties' lists.
fn hardcoded_ceilings() -> Vec<(&'static str, Value)> {
    vec![
        ("observer", json!(["work.read"])),
        (
            "developer",
            json!([
                "work.read",
                "session.maintain_own",
                "claim.manage_own",
                "execution.request_and_report_own",
                "planning.propose",
                "delivery.submit_and_request_review"
            ]),
        ),
        (
            "reviewer",
            json!(["work.read", "session.maintain_own", "review.decide"]),
        ),
        (
            "supervisor",
            json!([
                "work.read",
                "work.assign",
                "session.maintain_own",
                "planning.propose",
                "planning.edit_draft",
                "planning.approve",
                "planning.publish",
                "audit.read_project",
                "directory.manage_project"
            ]),
        ),
        (
            "deliverer",
            json!(["work.read", "session.maintain_own", "delivery.finalize"]),
        ),
        (
            "administrator",
            json!([
                "work.read",
                "access.manage_project",
                "audit.read_project",
                "directory.manage_project"
            ]),
        ),
    ]
}

fn assert_published_ceilings(presets: &Value) {
    assert_eq!(
        presets["role_names_grant_authority"],
        json!(false),
        "duty names stay descriptive"
    );
    assert_eq!(
        presets["roles"],
        json!([
            "observer",
            "developer",
            "reviewer",
            "supervisor",
            "deliverer",
            "administrator"
        ])
    );
    let ceilings = presets["action_ceilings"]
        .as_object()
        .expect("action_ceilings object");
    assert_eq!(ceilings.len(), 6, "exactly the six duties: {ceilings:?}");
    for (role, hardcoded) in hardcoded_ceilings() {
        let business_role: awr_team::BusinessRole =
            serde_json::from_value(json!(role)).expect("duty name parses");
        // The published list is the enforcement function's own value...
        assert_eq!(
            ceilings[role],
            json!(business_role.action_ceiling()),
            "{role} must publish its own action_ceiling"
        );
        // ...pinned action by action, so one extra or missing action fails.
        assert_eq!(ceilings[role], hardcoded, "{role} ceiling must stay exact");
    }
}

/// Write one verbatim reply into the sample directory when configured. The
/// committed copy under tests/samples is compared in the same test run.
fn save_verbatim(name: &str, value: &Value) {
    let Ok(directory) = std::env::var("AWR_CAPABILITIES_SAMPLE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    std::fs::write(directory.join(name), bytes).unwrap();
}

async fn login_web(server: &str, bearer: &str) -> String {
    let response = web::http()
        .post(format!("{server}/web/login"))
        .header("host", "awr.example")
        .header("origin", web::ISSUER)
        .header("x-awr-web", "1")
        .json(&json!({"bearer":bearer}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn web_query(server: &str, cookie: Option<&str>, key: &str, op: &str) -> (u16, Value) {
    let request = web::http()
        .post(format!("{server}/web/projects/{key}/query"))
        .header("host", "awr.example")
        .header("origin", web::ISSUER)
        .header("x-awr-web", "1")
        .json(&json!({"protocol_version":1,"op":op}));
    let request = match cookie {
        Some(cookie) => request.header("cookie", cookie),
        None => request,
    };
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

async fn web_caps(server: &str, cookie: Option<&str>, key: &str) -> (u16, Value) {
    web_query(server, cookie, key, "capabilities").await
}

#[tokio::test]
async fn capabilities_publish_duty_action_ceilings_from_the_shared_enforcement_function() {
    use awr_team::BusinessRole;
    use std::collections::BTreeSet;
    let (_g, owner, db, store) = setup().await;
    enable_writes(&owner).await;
    owner
        .batch_execute(
            "UPDATE awr_team.workstream_grants
             SET can_manage=true, grant_version=grant_version+1 WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let http = web::http();

    // A legacy member with no declared duties still reads the duty catalog:
    // the definitions are a public product description for every
    // authenticated project member.
    let admin = mcp(&server.url, A).await;
    let admin_caps = caps(&admin).await;
    assert!(
        admin_caps["identity"]["business_roles"].is_null(),
        "fixture admin carries no declared duty"
    );
    assert_published_ceilings(&admin_caps["business_role_presets"]);

    // Declare the observer duty for one member; the duty catalog itself stays
    // member-independent.
    let mut plan = member_plan();
    plan.business_roles = Some(BTreeSet::from([BusinessRole::Observer]));
    let preview = http
        .post(format!("{}/projects/one/access/preview", server.url))
        .header("host", "awr.example")
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"plan":plan}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview.status(), reqwest::StatusCode::OK);
    let preview: Value = preview.json().await.unwrap();
    let applied = http
        .post(format!("{}/projects/one/access/apply", server.url))
        .header("host", "awr.example")
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({
            "protocol_version":1,"request_id":"declare-observer-061",
            "expected_state":preview["state_digest"],
            "expected_plan":preview["plan_digest"],"plan":plan
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(applied.status(), reqwest::StatusCode::OK);
    owner
        .execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES($1,$2,'mcp-human','MCP member','active')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();

    let member = mcp(&server.url, MEMBER_TOKEN).await;
    let member_caps = caps(&member).await;
    assert_eq!(
        member_caps["identity"]["business_roles"],
        json!(["observer"])
    );
    assert_eq!(member_caps["identity"]["person_id"], "mcp-human");
    assert_eq!(member_caps["identity"]["person_display_name"], "MCP member");
    assert_eq!(
        member_caps["identity"]["membership_action_ceiling"],
        json!(["work.read"])
    );
    assert_published_ceilings(&member_caps["business_role_presets"]);
    assert_eq!(
        member_caps["business_role_presets"], admin_caps["business_role_presets"],
        "the duty catalog is member-independent"
    );

    // The web entry the observer bridge proxies returns the same flat reply.
    let cookie = login_web(&server.url, MEMBER_TOKEN).await;
    let (status, browser_caps) = web_caps(&server.url, Some(&cookie), "one").await;
    assert_eq!(status, 200, "{browser_caps}");
    assert_eq!(browser_caps, member_caps);
    assert_eq!(browser_caps["protocol"], "awr-team-workstream");
    assert_published_ceilings(&browser_caps["business_role_presets"]);
    save_verbatim("capabilities-web-read.json", &browser_caps);
    let (status, browser_next) = web_query(&server.url, Some(&cookie), "one", "work.next").await;
    assert_eq!(status, 200, "{browser_next}");
    assert_eq!(browser_next["data"]["identity"], browser_caps["identity"]);
    save_verbatim("work-next-web-read.json", &browser_next);

    // The committed samples are re-parsed and keep the live identity and catalog.
    let sample_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples/capabilities-web-read.json");
    let sample: Value = serde_json::from_slice(&std::fs::read(&sample_path).unwrap()).unwrap();
    assert_eq!(
        sample["business_role_presets"], browser_caps["business_role_presets"],
        "the committed sample matches the live reply"
    );
    assert_published_ceilings(&sample["business_role_presets"]);
    assert_eq!(sample["identity"], browser_caps["identity"]);
    assert_eq!(sample["identity"]["business_roles"], json!(["observer"]));
    assert_eq!(
        sample["identity"]["membership_action_ceiling"],
        json!(["work.read"])
    );
    let next_sample_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples/work-next-web-read.json");
    let next_sample: Value =
        serde_json::from_slice(&std::fs::read(next_sample_path).unwrap()).unwrap();
    assert_eq!(next_sample["data"]["identity"], browser_caps["identity"]);
    assert!(next_sample["data"]["items"].is_array());
    let sample_text = sample.to_string();
    for banned in ["secret", "password", "awr1."] {
        assert!(
            !sample_text.to_lowercase().contains(banned),
            "{banned} leaked into the sample"
        );
    }

    // An unauthenticated reader is refused before any duty data is built.
    let (status, unauthenticated) = web_caps(&server.url, None, "one").await;
    assert_eq!(status, 401, "{unauthenticated}");
    assert!(
        !unauthenticated
            .to_string()
            .contains("business_role_presets"),
        "refusals stay free of duty data"
    );

    // The same live cookie on another project binding authenticates the
    // member against that binding and is refused; cross-project reads stay
    // rejected.
    let (status, foreign) = web_caps(&server.url, Some(&cookie), "foreign").await;
    assert_eq!(status, 403, "{foreign}");
    assert!(!foreign.to_string().contains("business_role_presets"));

    // Revoking the member's credential closes the duty read on every entry.
    owner
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='mcp-member'",
            &[],
        )
        .await
        .unwrap();
    let (status, revoked) = web_caps(&server.url, Some(&cookie), "one").await;
    assert_eq!(status, 403, "{revoked}");
    assert!(!revoked.to_string().contains("business_role_presets"));
    let denied_mcp = member
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version":1,"op":"capabilities"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await;
    assert!(
        denied_mcp.is_err() || denied_mcp.as_ref().unwrap().is_error.unwrap_or(false),
        "a revoked member keeps no MCP duty read: {denied_mcp:?}"
    );
    // The shared read store itself refuses the revoked credential.
    let probe =
        WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    assert!(matches!(
        probe
            .query(TENANT, PROJECT, MEMBER_TOKEN, query("capabilities"))
            .await,
        Err(awr_team_pg::PgError::Forbidden)
    ));
}
