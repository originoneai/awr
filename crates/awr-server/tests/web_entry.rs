//! WS-044 web entry: per-project authorization on login/discovery (CR #143).
#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;

use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team_pg::WorkstreamReadStore;
use fixture::*;
use serde_json::{Value, json};
use std::time::Duration;

struct Server {
    base: String,
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start(store: WorkstreamReadStore, projects: Vec<ProjectBinding>) -> Server {
    store.check_schema().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://127.0.0.1:7381");
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec![],
        allowed_web_origins: vec![origin.clone()],
        oauth: None,
        projects,
    };
    let router = awr_server::service::router(config, address, store).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        base: format!("http://{address}"),
        origin,
        task,
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

fn web_headers(origin: &str, cookie: Option<&str>) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("origin", origin.parse().unwrap());
    h.insert("x-awr-web", "1".parse().unwrap());
    h.insert("content-type", "application/json".parse().unwrap());
    if let Some(c) = cookie {
        h.insert("cookie", c.parse().unwrap());
    }
    h
}

fn session_cookie(set_cookie: &reqwest::header::HeaderValue) -> String {
    let raw = set_cookie.to_str().unwrap();
    raw.split(';')
        .next()
        .expect("cookie pair")
        .trim()
        .to_string()
}

/// Lexicographically first binding is unauthorized; member only on later project.
#[tokio::test]
async fn login_discovers_second_project_only_member_without_exposing_first() {
    let (_guard, _, _, store) = setup().await;
    // "aaa" sorts before "zzz"; token A is only valid on reader-tenant (zzz).
    let server = start(
        store,
        vec![
            ProjectBinding {
                key: "aaa".into(),
                tenant_id: "other-tenant".into(),
                project_id: PROJECT.into(),
            },
            ProjectBinding {
                key: "zzz".into(),
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
            },
        ],
    )
    .await;

    let res = http()
        .post(format!("{}/v1/web/login", server.base))
        .headers(web_headers(&server.origin, None))
        .json(&json!({"bearer": A}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    let set = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .next()
        .cloned()
        .expect("set-cookie");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);
    let projects = body["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0], "zzz");
    assert!(!body.to_string().contains("aaa"));

    let cookie = session_cookie(&set);
    let listed = http()
        .get(format!("{}/v1/web/projects", server.base))
        .headers(web_headers(&server.origin, Some(&cookie)))
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);
    let listed: Value = listed.json().await.unwrap();
    let keys: Vec<&str> = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, vec!["zzz"]);
    assert!(!listed.to_string().contains("other-tenant"));
}

#[tokio::test]
async fn login_with_explicit_unauthorized_project_is_denied() {
    let (_guard, _, _, store) = setup().await;
    let server = start(
        store,
        vec![
            ProjectBinding {
                key: "aaa".into(),
                tenant_id: "other-tenant".into(),
                project_id: PROJECT.into(),
            },
            ProjectBinding {
                key: "zzz".into(),
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
            },
        ],
    )
    .await;

    let res = http()
        .post(format!("{}/v1/web/login", server.base))
        .headers(web_headers(&server.origin, None))
        .json(&json!({"bearer": A, "project": "aaa"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn project_discovery_drops_revoked_credential_access() {
    let (_guard, admin, _, store) = setup().await;
    let server = start(
        store,
        vec![
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
    )
    .await;

    let login = http()
        .post(format!("{}/v1/web/login", server.base))
        .headers(web_headers(&server.origin, None))
        .json(&json!({"bearer": A}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let set = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .next()
        .cloned()
        .expect("set-cookie");
    let cookie = session_cookie(&set);
    let before: Value = http()
        .get(format!("{}/v1/web/projects", server.base))
        .headers(web_headers(&server.origin, Some(&cookie)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["projects"].as_array().unwrap().len(), 1);
    assert_eq!(before["projects"][0]["key"], "one");

    // Revoke the underlying credential — discovery must re-check authorization.
    admin
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            &[],
        )
        .await
        .unwrap();

    let after: Value = http()
        .get(format!("{}/v1/web/projects", server.base))
        .headers(web_headers(&server.origin, Some(&cookie)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["ok"], true);
    assert!(
        after["projects"].as_array().unwrap().is_empty(),
        "revoked credential must not keep project discovery: {after}"
    );

    let session: Value = http()
        .get(format!("{}/v1/web/session", server.base))
        .headers(web_headers(&server.origin, Some(&cookie)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(session["projects"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn disjoint_project_credentials_do_not_enumerate_peer_projects() {
    let (_guard, _, _, store) = setup().await;
    let server = start(
        store,
        vec![
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
    )
    .await;

    let login = http()
        .post(format!("{}/v1/web/login", server.base))
        .headers(web_headers(&server.origin, None))
        .json(&json!({"bearer": A}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let body: Value = login.json().await.unwrap();
    assert_eq!(body["projects"], json!(["one"]));
    assert!(!body.to_string().contains("other"));
}
