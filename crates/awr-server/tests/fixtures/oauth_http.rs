#![allow(dead_code)]
use awr_server::service::{OAuthConfig, ProjectBinding, ServiceConfig};
use awr_team_pg::WorkstreamReadStore;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const ISSUER: &str = "https://awr.example";
pub const RESOURCE: &str = "https://awr.example/v1/projects/one/mcp";
pub const CALLBACK: &str = "https://client.example/callback?channel=awr";
pub const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";

pub struct Server {
    pub url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub async fn start(store: WorkstreamReadStore, enabled: bool) -> Server {
    start_with_directory(store, enabled, None).await
}
pub async fn start_with_directory(
    store: WorkstreamReadStore,
    enabled: bool,
    directory: Option<std::path::PathBuf>,
) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec!["awr.example".into()],
        allowed_web_origins: vec![ISSUER.into()],
        oauth: enabled.then(|| OAuthConfig {
            issuer: ISSUER.into(),
            state_directory: directory,
        }),
        projects: vec![
            ProjectBinding {
                key: "one".into(),
                tenant_id: "reader-tenant".into(),
                project_id: "reader-project".into(),
            },
            ProjectBinding {
                key: "other".into(),
                tenant_id: "other-tenant".into(),
                project_id: "reader-project".into(),
            },
        ],
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
pub fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}
impl Server {
    pub async fn shutdown(mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        http()
            .get(format!("{}{path}", self.url))
            .header("host", "awr.example")
    }
    pub fn post(&self, path: &str) -> reqwest::RequestBuilder {
        http()
            .post(format!("{}{path}", self.url))
            .header("host", "awr.example")
    }
}
pub fn encode(params: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params.iter().copied())
        .finish()
}
pub struct Pending {
    pub client_id: String,
    pub transaction: String,
    pub cookie: String,
    pub html: String,
}
pub async fn register(server: &Server, name: &str) -> Value {
    let res = server
        .post("/oauth/register")
        .json(&json!({"client_name":name,"redirect_uris":[CALLBACK],
        "grant_types":["authorization_code","refresh_token"],"token_endpoint_auth_method":"none"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    res.json().await.unwrap()
}
pub fn auth_query(client_id: &str) -> String {
    encode(&[
        ("client_id", client_id),
        ("redirect_uri", CALLBACK),
        ("response_type", "code"),
        ("resource", RESOURCE),
        ("state", "client state & original"),
        ("scope", "awr.project"),
        (
            "code_challenge",
            &URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes())),
        ),
        ("code_challenge_method", "S256"),
    ])
}
pub async fn begin(server: &Server, name: &str) -> Pending {
    let client = register(server, name).await;
    let client_id = client["client_id"].as_str().unwrap().to_owned();
    let res = server
        .get(&format!("/oauth/authorize?{}", auth_query(&client_id)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert_eq!(res.headers()["referrer-policy"], "same-origin");
    assert!(
        res.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert_eq!(
        res.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("form-action")),
        Some("form-action 'self'")
    );
    let set_cookie = res.headers()["set-cookie"].to_str().unwrap();
    for flag in ["Secure", "HttpOnly", "SameSite=Lax", "Path=/"] {
        assert!(set_cookie.contains(flag));
    }
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    let html = res.text().await.unwrap();
    let transaction = html
        .split("name=\"transaction\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    Pending {
        client_id,
        transaction,
        cookie,
        html,
    }
}
pub fn consent(
    server: &Server,
    pending: &Pending,
    action: &str,
    credential: &str,
) -> reqwest::RequestBuilder {
    server
        .post("/oauth/consent")
        .header("origin", ISSUER)
        .header("cookie", &pending.cookie)
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(encode(&[
            ("transaction", &pending.transaction),
            ("action", action),
            ("credential", credential),
        ]))
}
pub async fn completion_callback(res: reqwest::Response) -> url::Url {
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    assert!(res.headers().get("location").is_none());
    assert_eq!(
        res.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("form-action")),
        Some("form-action 'self'")
    );
    assert!(
        res.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let html = res.text().await.unwrap();
    assert!(!html.contains("<form"));
    assert!(!html.contains("<script"));
    let target = html
        .split("http-equiv=\"refresh\" content=\"0;url=")
        .nth(1)
        .expect("automatic callback navigation")
        .split('"')
        .next()
        .unwrap();
    assert!(html.contains(&format!("href=\"{target}\" rel=\"noreferrer\"")));
    let target = target
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    url::Url::parse(&target).unwrap()
}
pub fn token_body(client_id: &str, code: &str, resource: &str, verifier: &str) -> String {
    encode(&[
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code),
        ("redirect_uri", CALLBACK),
        ("resource", resource),
        ("code_verifier", verifier),
    ])
}
pub fn exchange(server: &Server, body: String) -> reqwest::RequestBuilder {
    server
        .post("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
}
