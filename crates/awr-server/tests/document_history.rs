#![cfg(feature = "pg-tests")]
//! AWR-PPD-053: document.history on the real HTTP/Web and MCP service entries.
//! Captures verbatim web-route replies under tests/samples so the Observer
//! bridge tests validate against real service output rather than handwritten
//! shapes.
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/oauth_http.rs"]
mod web;

use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team::SourceActivationPlan;
use awr_team_pg::{IngestRequest, SourceFile, SourceStore, WorkstreamReadStore};
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

fn save_verbatim(name: &str, value: &Value) {
    let Ok(directory) = std::env::var("AWR_DOCUMENT_HISTORY_SAMPLE_DIR") else {
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
        .json(&json!({"bearer": bearer}))
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

async fn web_query(server: &str, cookie: Option<&str>, key: &str, payload: Value) -> (u16, Value) {
    let request = web::http()
        .post(format!("{server}/web/projects/{key}/query"))
        .header("host", "awr.example")
        .header("origin", web::ISSUER)
        .header("x-awr-web", "1")
        .json(&payload);
    let request = match cookie {
        Some(cookie) => request.header("cookie", cookie),
        None => request,
    };
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

async fn publish_snapshot(
    source: &SourceStore,
    owner: &tokio_postgres::Client,
    actor_id: &str,
    files: Vec<SourceFile>,
) -> String {
    let bundle_bytes: Vec<u8> = owner
        .query_one(
            "SELECT elem->>'text'
             FROM awr_team.projects p
             JOIN awr_team.source_snapshots s
               ON s.tenant_id=p.tenant_id AND s.project_id=p.id AND s.id=p.active_snapshot_id
             CROSS JOIN LATERAL jsonb_array_elements(s.source_ref_json->'files') elem
             WHERE p.tenant_id=$1 AND p.id=$2 AND elem->>'path'='workstreams.json'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get::<_, String>(0)
        .into_bytes();
    let mut all_files = files;
    all_files.push(SourceFile {
        path: "workstreams.json".into(),
        bytes: bundle_bytes,
    });
    let candidate = source
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: actor_id.into(),
            parser_version: "workstreams/1".into(),
            files: all_files,
        })
        .await
        .unwrap();
    let reviewer = if actor_id == "reviewer" {
        "agent"
    } else {
        "reviewer"
    };
    source
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            reviewer,
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    source
        .activate_workstreams(
            TENANT,
            PROJECT,
            actor_id,
            &candidate.proposal_id,
            &SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                parser_version: candidate.parser_version.clone(),
                expected_authority_epoch: candidate.base_epoch.clone(),
                approved_candidate_digest: candidate.manifest_digest.clone(),
            },
        )
        .await
        .unwrap();
    candidate.snapshot_id
}

fn sha12(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))[..12].to_string()
}

fn assert_same_shape(sample: &Value, live: &Value) {
    assert_eq!(sample["protocol_version"], live["protocol_version"]);
    assert_eq!(sample["scope_id"], live["scope_id"]);
    assert_eq!(sample["selection_basis"], live["selection_basis"]);
    assert_eq!(sample["project_status"], live["project_status"]);
    assert_eq!(sample["data"]["path"], live["data"]["path"]);
    assert_eq!(sample["data"]["work_id"], live["data"]["work_id"]);
    assert_eq!(sample["data"]["total"], live["data"]["total"]);
    assert_eq!(sample["data"]["limit"], live["data"]["limit"]);
    assert_eq!(sample["data"]["truncated"], live["data"]["truncated"]);
    assert_eq!(sample["data"]["order"], live["data"]["order"]);
    assert_eq!(sample["data"]["source_kind"], live["data"]["source_kind"]);
    assert_eq!(
        sample["data"]["attribution_basis"],
        live["data"]["attribution_basis"]
    );
    assert_eq!(sample["data"]["read_only"], live["data"]["read_only"]);
    assert_eq!(
        sample["data"]["execution_authorized"],
        live["data"]["execution_authorized"]
    );
    let sample_items = sample["data"]["items"].as_array().unwrap();
    let live_items = live["data"]["items"].as_array().unwrap();
    assert_eq!(sample_items.len(), live_items.len());
    for (s_item, l_item) in sample_items.iter().zip(live_items.iter()) {
        assert_eq!(s_item["digest_prefix"], l_item["digest_prefix"]);
        assert_eq!(s_item["submitted_by"], l_item["submitted_by"]);
        assert!(s_item["source_snapshot_id"].is_string());
        assert!(s_item["recorded_at_unix_ms"].is_i64());
    }
}

#[tokio::test]
async fn document_history_web_and_mcp_enforce_permissions_and_match_verbatim_samples() {
    let v1 = b"# Delivery guide\n\nInitial version.\n";
    let v2 = b"# Delivery guide\n\nSecond revision.\n";
    let (_guard, owner, db, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: v1.to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"# Private peer doc\n".to_vec(),
        },
    ])
    .await;
    // Register active person for 'agent'; leave 'reviewer' without an active persons row.
    owner
        .batch_execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('reader-tenant','reader-project','agent','Lin Yue','active');
             UPDATE awr_team.project_memberships SET role='admin'
             WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND actor_id='reviewer';",
        )
        .await
        .unwrap();
    let source = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let initial_snapshot: String = owner
        .query_one(
            "SELECT active_snapshot_id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    // Unchanged snapshot for docs/alpha.md (must be skipped).
    let unchanged_snapshot = publish_snapshot(
        &source,
        &owner,
        "reviewer",
        vec![
            SourceFile {
                path: "docs/alpha.md".into(),
                bytes: v1.to_vec(),
            },
            SourceFile {
                path: "docs/private-beta.md".into(),
                bytes: b"# Private peer doc v2\n".to_vec(),
            },
        ],
    )
    .await;
    // Second revision by 'reviewer' (no active person -> submitted_by: null).
    let v2_snapshot = publish_snapshot(
        &source,
        &owner,
        "reviewer",
        vec![
            SourceFile {
                path: "docs/alpha.md".into(),
                bytes: v2.to_vec(),
            },
            SourceFile {
                path: "docs/private-beta.md".into(),
                bytes: b"# Private peer doc v2\n".to_vec(),
            },
        ],
    )
    .await;
    // Register a missing path on stream 1 so reader A gets InvalidInput / source_unavailable.
    let mut catalog: Value = owner
        .query_one(
            "SELECT catalog_json FROM awr_team.workstream_catalogs
             WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
            &[&TENANT, &PROJECT, &v2_snapshot],
        )
        .await
        .unwrap()
        .get(0);
    catalog["workstreams"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|s| s["id"] == awr_core::Id::from(1).to_string())
        .unwrap()["acceptance_contracts"]
        .as_array_mut()
        .unwrap()
        .push(json!("docs/missing-alpha.md"));
    owner
        .execute(
            "UPDATE awr_team.workstream_catalogs SET catalog_json=$1
             WHERE tenant_id=$2 AND project_id=$3 AND snapshot_id=$4",
            &[&catalog, &TENANT, &PROJECT, &v2_snapshot],
        )
        .await
        .unwrap();

    let server = start(store).await;
    let cookie = login_web(&server.url, A).await;

    // 1. Real web route unauthorized-path error while reader A has stream 1 only (403 Forbidden).
    let (status, forbidden) = web_query(
        &server.url,
        Some(&cookie),
        "one",
        json!({"protocol_version": 1, "op": "document.history", "source_path": "docs/private-beta.md"}),
    )
    .await;
    assert_eq!(status, 403, "{forbidden}");
    assert_eq!(forbidden["code"], "Forbidden");
    save_verbatim("document-history-web-forbidden.json", &forbidden);

    // 2. Real web route missing-path error (400 InvalidInput / registered_source_required).
    let (status, not_found) = web_query(
        &server.url,
        Some(&cookie),
        "one",
        json!({"protocol_version": 1, "op": "document.history", "source_path": "docs/missing-alpha.md"}),
    )
    .await;
    assert_eq!(status, 400, "{not_found}");
    assert_eq!(not_found["code"], "InvalidInput");
    assert_eq!(not_found["invalid_field"], "/source_path");
    assert_eq!(not_found["constraint"], "registered_source_required");
    save_verbatim("document-history-web-not-found.json", &not_found);

    // Grant stream 2 to reader A so directory.read passes and the verbatim
    // success sample captures both a determinable submitter and a null submitter.
    owner
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,
             workstream_id,authority_version,can_read) VALUES($1,$2,'agent','cli-a',$3,1,true)",
            &[&TENANT, &PROJECT, &awr_core::Id::from(2).to_string()],
        )
        .await
        .unwrap();

    // 3. Real web route success reply.
    let (status, web_reply) = web_query(
        &server.url,
        Some(&cookie),
        "one",
        json!({"protocol_version": 1, "op": "document.history", "source_path": "docs/alpha.md"}),
    )
    .await;
    assert_eq!(status, 200, "{web_reply}");
    assert_eq!(web_reply["data"]["total"], 2);
    assert_eq!(web_reply["data"]["limit"], 20);
    assert_eq!(web_reply["data"]["truncated"], false);
    assert_eq!(web_reply["data"]["order"], "newest_first");
    let items = web_reply["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["source_snapshot_id"], v2_snapshot);
    assert_eq!(items[0]["digest_prefix"], sha12(v2));
    assert_eq!(items[0]["submitted_by"], Value::Null);
    assert_eq!(items[1]["source_snapshot_id"], initial_snapshot);
    assert_eq!(items[1]["digest_prefix"], sha12(v1));
    assert_eq!(
        items[1]["submitted_by"],
        json!({"person_id": "agent", "display_name": "Lin Yue"})
    );
    assert!(
        items
            .iter()
            .all(|item| item["source_snapshot_id"] != unchanged_snapshot)
    );
    let reply_text = web_reply.to_string();
    assert!(!reply_text.contains("Initial version"));
    assert!(!reply_text.contains("Private peer"));
    save_verbatim("document-history-web-read.json", &web_reply);

    // 4. Real MCP route returns the same history items.
    let mcp_client = mcp(&server.url, A).await;
    let mcp_reply = mcp_client
        .call_tool(
            CallToolRequestParams::new("awr_team_query").with_arguments(
                json!({"protocol_version": 1, "op": "document.history", "source_path": "docs/alpha.md"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(mcp_reply["data"]["items"], web_reply["data"]["items"]);

    // 5. Unauthenticated (401) and cross-project (403).
    let (status, unauth) = web_query(
        &server.url,
        None,
        "one",
        json!({"protocol_version": 1, "op": "document.history", "source_path": "docs/alpha.md"}),
    )
    .await;
    assert_eq!(status, 401, "{unauth}");
    let (status, foreign) = web_query(
        &server.url,
        Some(&cookie),
        "foreign",
        json!({"protocol_version": 1, "op": "document.history", "source_path": "docs/alpha.md"}),
    )
    .await;
    assert_eq!(status, 403, "{foreign}");

    // 6. Re-parse and compare committed samples against live replies (unless regenerating).
    if std::env::var_os("AWR_DOCUMENT_HISTORY_SAMPLE_DIR").is_none() {
        let samples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples");
        let sample_ok: Value = serde_json::from_slice(
            &std::fs::read(samples_dir.join("document-history-web-read.json")).unwrap(),
        )
        .unwrap();
        assert_same_shape(&sample_ok, &web_reply);
        let sample_missing: Value = serde_json::from_slice(
            &std::fs::read(samples_dir.join("document-history-web-not-found.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(sample_missing, not_found);
        let sample_forbidden: Value = serde_json::from_slice(
            &std::fs::read(samples_dir.join("document-history-web-forbidden.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(sample_forbidden, forbidden);
        for text in [
            sample_ok.to_string(),
            sample_missing.to_string(),
            sample_forbidden.to_string(),
        ] {
            for banned in ["secret", "password", "awr1."] {
                assert!(
                    !text.to_lowercase().contains(banned),
                    "{banned} leaked into sample"
                );
            }
        }
    }
}
