//! Optional signed notifications. Payloads only wake fresh authenticated queries.
use super::{
    ServiceConfig,
    github_worker::{GitHubWakeScope, GitHubWorkerMonitor, GitHubWorkerWakeup},
};
use crate::config::GitHubWorkerConfig;
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{Next, from_fn_with_state},
    response::Response,
    routing::post,
};
use ring::hmac;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::Read,
    net::SocketAddr,
    path::Path as FilePath,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

pub const GITHUB_WEBHOOK_CONFIG_ENV: &str = "AWR_TEAM_GITHUB_WEBHOOK_CONFIG";
const MAX_HOOKS: usize = 16;
const RECENT_DELIVERIES: usize = 512;
const RECENT_TTL: Duration = Duration::from_secs(600);
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
fn body_limit() -> usize {
    65536
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubWebhookConfig {
    pub version: u32,
    #[serde(default = "body_limit")]
    pub max_body_bytes: usize,
    #[serde(default)]
    pub hooks: Vec<GitHubWebhookSpec>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubWebhookSpec {
    pub worker_id: String,
    /// Variable name only. This is separate from AWR and provider credentials.
    pub secret_env: String,
}
impl GitHubWebhookConfig {
    pub fn from_environment(
        service: &ServiceConfig,
        workers: Option<&GitHubWorkerConfig>,
    ) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os(GITHUB_WEBHOOK_CONFIG_ENV) else {
            return Ok(None);
        };
        Self::read(FilePath::new(&path), service, workers).map(Some)
    }
    pub fn read(
        path: &FilePath,
        service: &ServiceConfig,
        workers: Option<&GitHubWorkerConfig>,
    ) -> Result<Self, String> {
        let mut text = String::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(65537).read_to_string(&mut text))
            .map_err(|_| "could not read GitHub notification configuration".to_string())?;
        if text.len() > 65536 {
            return Err("GitHub notification configuration is too large".into());
        }
        let config: Self = toml::from_str(&text)
            .map_err(|_| "invalid GitHub notification configuration".to_string())?;
        config.validate(service, workers)?;
        Ok(config)
    }
    pub fn validate(
        &self,
        service: &ServiceConfig,
        workers: Option<&GitHubWorkerConfig>,
    ) -> Result<(), String> {
        service.validate()?;
        if let Some(workers) = workers {
            workers.validate(service)?;
        }
        if self.version != 1
            || self.hooks.len() > MAX_HOOKS
            || !(1024..=262144).contains(&self.max_body_bytes)
        {
            return Err("unsupported GitHub notification version or limits".into());
        }
        let mut ids = BTreeSet::new();
        for hook in &self.hooks {
            let worker = workers
                .and_then(|c| c.workers.iter().find(|w| w.worker_id == hook.worker_id))
                .ok_or_else(|| "GitHub notification worker is unavailable".to_string())?;
            let name = hook.secret_env.as_bytes();
            if !ids.insert(&hook.worker_id)
                || name.is_empty()
                || name.len() > 128
                || !(name[0].is_ascii_uppercase() || name[0] == b'_')
                || !name
                    .iter()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
                || workers.is_some_and(|c| {
                    c.workers.iter().any(|w| {
                        w.credential_env == hook.secret_env
                            || w.provider_credential_env == hook.secret_env
                    })
                })
                || worker.repository.repository_id == 0
            {
                return Err(
                    "invalid GitHub notification identity or secret environment name".into(),
                );
            }
        }
        Ok(())
    }
}

struct Hook {
    scope: GitHubWakeScope,
    secret: Zeroizing<String>,
    recent: Mutex<VecDeque<(String, Instant)>>,
}
struct Hooks {
    hosts: Vec<String>,
    hooks: BTreeMap<String, Hook>,
    wakeups: RwLock<BTreeMap<String, GitHubWorkerWakeup>>,
    permits: Semaphore,
}
/// Prepare all routes and secrets before spawning workers; bind their already
/// admitted wake handles before serving HTTP. This grants no worker authority.
pub struct GitHubWebhookRuntime {
    state: Arc<Hooks>,
    max_body_bytes: usize,
}
impl GitHubWebhookRuntime {
    pub fn prepare(
        service: &ServiceConfig,
        actual: SocketAddr,
        config: Option<GitHubWebhookConfig>,
        workers: Option<&GitHubWorkerConfig>,
        mut secret_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, String> {
        service.validate()?;
        if let Some(config) = &config {
            config.validate(service, workers)?;
        }
        let mut hooks = BTreeMap::new();
        let max_body_bytes = config.as_ref().map_or(body_limit(), |c| c.max_body_bytes);
        for spec in config.into_iter().flat_map(|c| c.hooks) {
            let worker = workers
                .and_then(|c| c.workers.iter().find(|w| w.worker_id == spec.worker_id))
                .ok_or_else(|| "GitHub notification worker is unavailable".to_string())?;
            let secret = Zeroizing::new(
                secret_lookup(&spec.secret_env)
                    .ok_or_else(|| "GitHub notification secret is unavailable".to_string())?,
            );
            if !(32..=4096).contains(&secret.len()) || secret.chars().any(char::is_control) {
                return Err("GitHub notification secret is invalid".into());
            }
            hooks.insert(
                spec.worker_id,
                Hook {
                    scope: GitHubWakeScope::from_spec(worker),
                    secret,
                    recent: Mutex::new(VecDeque::new()),
                },
            );
        }
        let mut hosts = service.allowed_hosts.clone();
        if actual.ip().is_loopback() {
            hosts.push(actual.to_string());
            hosts.push(format!("localhost:{}", actual.port()));
        }
        Ok(Self {
            state: Arc::new(Hooks {
                hosts,
                hooks,
                wakeups: RwLock::new(BTreeMap::new()),
                permits: Semaphore::new(16),
            }),
            max_body_bytes,
        })
    }
    pub fn router(&self) -> Router {
        if self.state.hooks.is_empty() {
            return Router::new();
        }
        Router::new()
            .route("/v1/hooks/github/{worker}", post(notification))
            .layer(DefaultBodyLimit::max(self.max_body_bytes))
            .layer(from_fn_with_state(self.state.clone(), bounded_request))
            .with_state(self.state.clone())
    }
    /// Only exact metadata from an admitted worker can fill a prepared route.
    /// Binding an unrelated monitor leaves that route unavailable.
    pub fn connect(&self, monitor: &GitHubWorkerMonitor) {
        let handles = monitor
            .wakeups()
            .into_iter()
            .filter(|h| {
                self.state
                    .hooks
                    .get(&h.scope().worker_id)
                    .is_some_and(|hook| hook.scope == *h.scope())
            })
            .map(|h| (h.scope().worker_id.clone(), h))
            .collect();
        *self
            .state
            .wakeups
            .write()
            .unwrap_or_else(|e| e.into_inner()) = handles;
    }
}

fn single<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    let mut all = headers.get_all(key).iter();
    let value = all.next()?.to_str().ok()?;
    if all.next().is_some() {
        return None;
    }
    Some(value)
}
fn signature(headers: &HeaderMap, body: &[u8], secret: &str) -> bool {
    let Some(value) =
        single(headers, "x-hub-signature-256").and_then(|v| v.strip_prefix("sha256="))
    else {
        return false;
    };
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return false;
    }
    let mut decoded = [0u8; 32];
    for (i, byte) in decoded.iter_mut().enumerate() {
        let Ok(parsed) = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16) else {
            return false;
        };
        *byte = parsed;
    }
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes()),
        body,
        &decoded,
    )
    .is_ok()
}
#[derive(Deserialize)]
struct Payload {
    repository: Repository,
}
#[derive(Deserialize)]
struct Repository {
    id: u64,
}

fn reply(status: StatusCode, code: &str) -> Response {
    super::response(status, json!({"code":code,"authoritative_fact":false}))
}
async fn bounded_request(
    State(state): State<Arc<Hooks>>,
    request: Request,
    next: Next,
) -> Response {
    // Acquire before body extraction so partial uploads cannot bypass the
    // concurrency bound by retaining an unbounded number of body buffers.
    let Ok(_permit) = state.permits.try_acquire() else {
        return reply(StatusCode::TOO_MANY_REQUESTS, "NotificationBusy");
    };
    match tokio::time::timeout(REQUEST_DEADLINE, next.run(request)).await {
        Ok(response) => response,
        Err(_) => reply(StatusCode::REQUEST_TIMEOUT, "NotificationTimedOut"),
    }
}
async fn notification(
    State(state): State<Arc<Hooks>>,
    Path(worker): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(hook) = state.hooks.get(&worker) else {
        return reply(StatusCode::NOT_FOUND, "NotificationUnavailable");
    };
    if headers.contains_key("origin")
        || !single(&headers, "host").is_some_and(|h| state.hosts.iter().any(|s| s == h))
        || !single(&headers, "content-type").is_some_and(|t| {
            t.split(';')
                .next()
                .is_some_and(|v| v.trim() == "application/json")
        })
        || !signature(&headers, &body, &hook.secret)
    {
        return reply(StatusCode::FORBIDDEN, "NotificationDenied");
    }
    let Some(delivery) = single(&headers, "x-github-delivery").filter(|s| {
        !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) else {
        return reply(StatusCode::BAD_REQUEST, "InvalidNotification");
    };
    let Some(event) = single(&headers, "x-github-event").filter(|s| {
        !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    }) else {
        return reply(StatusCode::BAD_REQUEST, "InvalidNotification");
    };
    let Ok(payload) = serde_json::from_slice::<Payload>(&body) else {
        return reply(StatusCode::BAD_REQUEST, "InvalidNotification");
    };
    if payload.repository.id != hook.scope.repository_id {
        return reply(StatusCode::FORBIDDEN, "NotificationDenied");
    }
    if !matches!(
        event,
        "push" | "pull_request" | "check_run" | "check_suite" | "status" | "workflow_run"
    ) {
        return reply(StatusCode::ACCEPTED, "NotificationIgnored");
    }
    let Some(wakeup) = state
        .wakeups
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&worker)
        .cloned()
    else {
        return reply(StatusCode::SERVICE_UNAVAILABLE, "WorkerUnavailable");
    };
    let now = Instant::now();
    let mut recent = hook.recent.lock().unwrap_or_else(|e| e.into_inner());
    while recent
        .front()
        .is_some_and(|(_, at)| now.duration_since(*at) >= RECENT_TTL)
    {
        recent.pop_front();
    }
    if recent.iter().any(|(id, _)| id == delivery) {
        return reply(StatusCode::ACCEPTED, "NotificationCoalesced");
    }
    if !wakeup.notify() {
        return reply(StatusCode::SERVICE_UNAVAILABLE, "WorkerUnavailable");
    }
    if recent.len() >= RECENT_DELIVERIES {
        recent.pop_front();
    }
    recent.push_back((delivery.to_owned(), now));
    reply(StatusCode::ACCEPTED, "QueryWakeupRequested")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    // GitHub's own documented example ("Validating webhook deliveries"). The tests
    // in tests/github_webhook.rs sign with the same HMAC code the verifier uses, so
    // only a value computed by GitHub shows that the two agree with the provider.
    const SECRET: &str = "It's a Secret to Everybody";
    const BODY: &[u8] = b"Hello, World!";
    const SIGNATURE: &str =
        "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

    fn headers(values: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in values {
            map.append("x-hub-signature-256", HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn the_signature_documented_by_github_verifies() {
        assert!(signature(&headers(&[SIGNATURE]), BODY, SECRET));
    }

    #[test]
    fn anything_other_than_that_exact_delivery_is_refused() {
        let valid = headers(&[SIGNATURE]);
        assert!(!signature(&valid, b"Hello, World?", SECRET), "changed body");
        assert!(
            !signature(&valid, BODY, "It's a Secret to Everybody!"),
            "changed secret"
        );
        assert!(!signature(&valid, b"", SECRET), "empty body");
        for refused in [
            // The legacy SHA-1 header format and a missing prefix are not accepted.
            SIGNATURE.replace("sha256=", "sha1="),
            SIGNATURE.replace("sha256=", ""),
            // GitHub sends lowercase hexadecimal only.
            SIGNATURE.to_uppercase().replace("SHA256=", "sha256="),
            // Wrong length, trailing text, non-hexadecimal digits.
            SIGNATURE[..SIGNATURE.len() - 1].to_owned(),
            format!("{SIGNATURE}0"),
            format!("{SIGNATURE} "),
            SIGNATURE.replace('7', "g"),
        ] {
            assert!(
                !signature(&headers(&[&refused]), BODY, SECRET),
                "must be refused: {refused}"
            );
        }
        assert!(
            !signature(&HeaderMap::new(), BODY, SECRET),
            "missing header"
        );
        assert!(
            !signature(&headers(&[SIGNATURE, SIGNATURE]), BODY, SECRET),
            "a repeated header is ambiguous"
        );
    }
}
