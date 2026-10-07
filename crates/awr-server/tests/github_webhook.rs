//! Signed loopback HTTP boundaries, without PG authority or provider access.
use awr_server::{
    config::GitHubWorkerConfig,
    service::{ProjectBinding, ServiceConfig, github_webhook::*},
};
use ring::hmac;
use serde_json::Value;
use std::time::Duration;

const KEY: &str = "synthetic-notification-key-for-tests";
const VARIABLE: &str = "SYNTHETIC_NOTIFICATION_SECRET";
fn service() -> ServiceConfig {
    ServiceConfig {
        version: 1,
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_hosts: vec![],
        allowed_web_origins: vec![],
        oauth: None,
        projects: vec![ProjectBinding {
            key: "team".into(),
            tenant_id: "example-tenant".into(),
            project_id: "example-project".into(),
        }],
    }
}
fn workers() -> GitHubWorkerConfig {
    let mut config: GitHubWorkerConfig = toml::from_str(include_str!(
        "../../../examples/github-delivery/worker.toml"
    ))
    .unwrap();
    config.workers[0].repository.report_directory = std::env::temp_dir();
    config
}
fn config() -> GitHubWebhookConfig {
    GitHubWebhookConfig {
        version: 1,
        max_body_bytes: 65536,
        hooks: vec![GitHubWebhookSpec {
            worker_id: "github-repository-worker".into(),
            secret_env: VARIABLE.into(),
        }],
    }
}
fn sign(body: &[u8], key: &str) -> String {
    let digest = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes()), body);
    let bytes: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={bytes}")
}
#[test]
fn absent_empty_and_invalid_configurations_do_not_resolve_secrets() {
    for config in [
        None,
        Some(GitHubWebhookConfig {
            version: 1,
            max_body_bytes: 65536,
            hooks: vec![],
        }),
    ] {
        GitHubWebhookRuntime::prepare(&service(), service().listen, config, None, |_| {
            panic!("absent lookup")
        })
        .unwrap();
    }
    for mutate in [
        |c: &mut GitHubWebhookConfig| c.version = 2,
        |c: &mut GitHubWebhookConfig| c.max_body_bytes = 1023,
        |c: &mut GitHubWebhookConfig| c.max_body_bytes = 262145,
        |c: &mut GitHubWebhookConfig| c.hooks[0].worker_id = "missing".into(),
        |c: &mut GitHubWebhookConfig| c.hooks[0].secret_env = "raw-synthetic-secret".into(),
        |c: &mut GitHubWebhookConfig| c.hooks[0].secret_env = "1INVALID".into(),
        |c: &mut GitHubWebhookConfig| c.hooks[0].secret_env = "AWR_GITHUB_WORKER_CREDENTIAL".into(),
        |c: &mut GitHubWebhookConfig| c.hooks[0].secret_env = "GITHUB_DELIVERY_CREDENTIAL".into(),
        |c: &mut GitHubWebhookConfig| c.hooks.push(c.hooks[0].clone()),
        |c: &mut GitHubWebhookConfig| c.hooks = vec![c.hooks[0].clone(); 17],
    ] {
        let mut c = config();
        mutate(&mut c);
        let result = GitHubWebhookRuntime::prepare(
            &service(),
            service().listen,
            Some(c),
            Some(&workers()),
            |_| panic!("invalid lookup"),
        );
        assert!(result.is_err());
    }
    assert!(
        GitHubWebhookRuntime::prepare(
            &service(),
            service().listen,
            Some(config()),
            None,
            |_| panic!("missing worker lookup")
        )
        .is_err()
    );
}
#[test]
fn strict_wire_configuration_rejects_raw_secrets_unknown_fields_and_duplicate_keys() {
    let text = "version=1\n[[hooks]]\nworker_id='github-repository-worker'\nsecret_env='SYNTHETIC_NOTIFICATION_SECRET'\n";
    let c: GitHubWebhookConfig = toml::from_str(text).unwrap();
    c.validate(&service(), Some(&workers())).unwrap();
    for changed in [
        format!("secret='synthetic'\n{text}"),
        format!("{text}transport='fixture'\n"),
        format!("{text}secret_env='ANOTHER'\n"),
        text.replace("secret_env=", "token="),
    ] {
        assert!(toml::from_str::<GitHubWebhookConfig>(&changed).is_err());
    }
}
#[test]
fn missing_invalid_or_oversized_secret_never_echoes_its_value() {
    for secret in [
        None,
        Some(String::new()),
        Some("x".repeat(31)),
        Some("x".repeat(4097)),
        Some(format!("{}\n", "x".repeat(32))),
    ] {
        let r = GitHubWebhookRuntime::prepare(
            &service(),
            service().listen,
            Some(config()),
            Some(&workers()),
            |name| {
                assert_eq!(name, VARIABLE);
                secret.clone()
            },
        );
        let error = r.err().unwrap();
        assert!(!error.contains(VARIABLE));
        if let Some(s) = secret.filter(|s| !s.is_empty()) {
            assert!(!error.contains(&s));
        }
    }
}
#[test]
fn bounded_configuration_reader_redacts_private_paths_and_invalid_source() {
    let path =
        std::env::temp_dir().join(format!("awr-notification-config-{}", awr_core::Id::new()));
    for bytes in [vec![b'x'; 65537], b"private-synthetic-secret".to_vec()] {
        std::fs::write(&path, bytes).unwrap();
        let error = GitHubWebhookConfig::read(&path, &service(), Some(&workers()))
            .err()
            .unwrap();
        assert!(!error.contains(path.to_str().unwrap()));
        assert!(!error.contains("private-synthetic-secret"));
    }
    std::fs::remove_file(path).unwrap();
}

struct Http {
    url: String,
    client: reqwest::Client,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}
impl Http {
    async fn start(enabled: bool) -> Self {
        let listener = tokio::net::TcpListener::bind(service().listen)
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let hooks = GitHubWebhookRuntime::prepare(
            &service(),
            address,
            enabled.then(config),
            Some(&workers()),
            |_| Some(KEY.into()),
        )
        .unwrap();
        let router = hooks.router();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            url: format!("http://{address}/v1/hooks/github/github-repository-worker"),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            stop,
            task,
        }
    }
    fn request(&self, body: &[u8]) -> reqwest::RequestBuilder {
        self.client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("x-github-delivery", "synthetic-delivery")
            .header("x-github-event", "push")
            .header("x-hub-signature-256", sign(body, KEY))
            .body(body.to_vec())
    }
    async fn close(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap();
    }
}
async fn assert_response(response: reqwest::Response, expected: u16) -> Value {
    assert_eq!(response.status().as_u16(), expected);
    let text = response.text().await.unwrap();
    assert!(!text.contains(KEY));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["authoritative_fact"], false);
    value
}
#[tokio::test]
async fn raw_body_hmac_is_required_and_a_valid_notification_never_creates_authority() {
    let http = Http::start(true).await;
    let body = br#"{ "repository": { "id": 123456 }, "approved": true, "status": "completed" }"#;
    assert_eq!(
        assert_response(http.request(body).send().await.unwrap(), 503).await["code"],
        "WorkerUnavailable"
    );
    let altered = br#"{"repository":{"id":123456},"approved":true,"status":"completed"}"#;
    // Semantically identical JSON with a signature for different original bytes is refused.
    assert_response(
        http.request(body)
            .body(altered.to_vec())
            .send()
            .await
            .unwrap(),
        403,
    )
    .await;
    assert_response(http.request(altered).send().await.unwrap(), 503).await;
    let mut request = http.request(body).build().unwrap();
    request.headers_mut().insert(
        "x-hub-signature-256",
        sign(body, "different-synthetic-key-for-tests")
            .parse()
            .unwrap(),
    );
    assert_response(http.client.execute(request).await.unwrap(), 403).await;
    request = http.request(body).build().unwrap();
    request.headers_mut().remove("x-hub-signature-256");
    assert_response(http.client.execute(request).await.unwrap(), 403).await;
    http.close().await;
}
#[tokio::test]
async fn exact_scope_headers_numeric_repository_and_payload_bounds_are_enforced() {
    let http = Http::start(true).await;
    let body = br#"{"repository":{"id":123456}}"#;
    for (header, value) in [
        ("origin", "https://example.invalid"),
        ("host", "example.invalid"),
        ("content-type", "text/plain"),
    ] {
        let mut request = http.request(body).build().unwrap();
        request.headers_mut().insert(header, value.parse().unwrap());
        assert_response(http.client.execute(request).await.unwrap(), 403).await;
    }
    for header in ["x-hub-signature-256", "x-github-delivery", "x-github-event"] {
        let mut request = http.request(body).build().unwrap();
        let value = request.headers()[header].clone();
        request.headers_mut().append(header, value);
        assert_response(
            http.client.execute(request).await.unwrap(),
            if header == "x-hub-signature-256" {
                403
            } else {
                400
            },
        )
        .await;
    }
    for invalid in [
        br#"{"repository":{"id":7}}"#.as_slice(),
        br#"{"repository":{"id":"123456"}}"#,
        br#"{"repository":{"id":123456,"id":7}}"#,
        br#"{"repository":{"id":123456},"repository":{"id":7}}"#,
        b"not JSON",
    ] {
        assert_response(
            http.request(invalid).send().await.unwrap(),
            if invalid == br#"{"repository":{"id":7}}"# {
                403
            } else {
                400
            },
        )
        .await;
    }
    let mut ping = http.request(body).build().unwrap();
    ping.headers_mut()
        .insert("x-github-event", "ping".parse().unwrap());
    assert_eq!(
        assert_response(http.client.execute(ping).await.unwrap(), 202).await["code"],
        "NotificationIgnored"
    );
    let large = vec![b'x'; 65537];
    assert_eq!(
        http.request(&large).send().await.unwrap().status().as_u16(),
        413
    );
    assert_eq!(
        http.client
            .post(format!("{}-missing", http.url))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        404
    );
    http.close().await;
}
#[tokio::test]
async fn absent_configuration_does_not_add_an_http_endpoint() {
    let http = Http::start(false).await;
    assert_eq!(
        http.request(br#"{"repository":{"id":123456}}"#)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        404
    );
    http.close().await;
}

#[tokio::test]
async fn concurrency_is_bounded_before_partial_request_bodies_are_buffered() {
    use tokio::io::AsyncWriteExt;
    let http = Http::start(true).await;
    let url = reqwest::Url::parse(&http.url).unwrap();
    let address = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
    let mut uploads = Vec::new();
    for _ in 0..16 {
        let mut socket = tokio::net::TcpStream::connect(&address).await.unwrap();
        let headers = format!(
            "POST {} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{",
            url.path()
        );
        socket.write_all(headers.as_bytes()).await.unwrap();
        uploads.push(socket);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let body = br#"{"repository":{"id":123456}}"#;
    assert_eq!(
        assert_response(http.request(body).send().await.unwrap(), 429).await["code"],
        "NotificationBusy"
    );
    drop(uploads);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let response = http.request(body).send().await.unwrap();
            if response.status().as_u16() == 503 {
                assert_eq!(
                    assert_response(response, 503).await["code"],
                    "WorkerUnavailable"
                );
                break;
            }
            assert_response(response, 429).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    http.close().await;
}

#[tokio::test]
async fn incomplete_request_body_times_out_and_releases_admission() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let http = Http::start(true).await;
    let url = reqwest::Url::parse(&http.url).unwrap();
    let address = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
    let mut socket = tokio::net::TcpStream::connect(&address).await.unwrap();
    let headers = format!(
        "POST {} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{",
        url.path()
    );
    let begun = std::time::Instant::now();
    socket.write_all(headers.as_bytes()).await.unwrap();
    let mut status = String::new();
    tokio::time::timeout(
        Duration::from_secs(13),
        BufReader::new(socket).read_line(&mut status),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(status.starts_with("HTTP/1.1 408 "));
    assert!(begun.elapsed() >= Duration::from_secs(9));
    assert_response(
        http.request(br#"{"repository":{"id":123456}}"#)
            .send()
            .await
            .unwrap(),
        503,
    )
    .await;
    http.close().await;
}
