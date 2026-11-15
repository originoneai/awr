//! Designed Team Web entry (WS-044).
//!
//! Browser clients authenticate by exchanging a bearer for an HttpOnly session
//! cookie on `/v1/web/*`. Classic `/v1/projects/*` and MCP routes continue to
//! reject any `Origin` header. This is not "open CORS + embed bearer in JS":
//! the allowlisted Origin is paired with cookie sessions, logout/revoke, and
//! the same server authorization + idempotent receipts used by CLI/MCP.

use super::{
    AccessOp, ProjectBinding, StateData, access_dispatch_authorized, denied, dispatch_authorized,
    error_response, response,
};
use awr_team_pg::{PgError, WorkstreamQuery};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub const COOKIE_NAME: &str = "awr_web_session";
pub const WEB_GUARD_HEADER: &str = "x-awr-web";
const SESSION_TTL_SECS: u64 = 8 * 60 * 60;
const MAX_SESSIONS: usize = 4096;

pub(crate) use crate::oauth::BrowserSession as WebSession;

#[derive(Default)]
pub struct WebSessionStore {
    inner: std::sync::Mutex<BTreeMap<String, WebSession>>,
    shared: Option<Arc<crate::oauth::OAuthStore>>,
}

impl WebSessionStore {
    pub fn new(shared: Option<Arc<crate::oauth::OAuthStore>>) -> Self {
        Self {
            shared,
            ..Self::default()
        }
    }

    fn access<T>(
        &self,
        persist: bool,
        operation: impl FnOnce(&mut BTreeMap<String, WebSession>) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        if let Some(shared) = &self.shared {
            return shared
                .browser_sessions(persist, |sessions| {
                    operation(sessions).map_err(|_| crate::oauth::OAuthError::Unavailable)
                })
                .map_err(|_| "session store unavailable");
        }
        let mut guard = self.inner.lock().map_err(|_| "session store unavailable")?;
        operation(&mut guard)
    }

    pub fn insert(&self, session: WebSession) -> Result<(), &'static str> {
        self.access(true, |guard| {
            if guard.len() >= MAX_SESSIONS {
                let now = now_ms();
                guard.retain(|_, s| !s.revoked && s.expires_at_ms > now);
                if guard.len() >= MAX_SESSIONS {
                    return Err("too many web sessions");
                }
            }
            guard.insert(session.id.clone(), session);
            Ok(())
        })
    }

    pub fn get_live(&self, id: &str) -> Option<WebSession> {
        self.access(false, |guard| {
            let now = now_ms();
            let Some(session) = guard.get_mut(id) else {
                return Ok(None);
            };
            if session.revoked || session.expires_at_ms <= now {
                return Ok(None);
            }
            session.last_seen_ms = now;
            Ok(Some(session.clone()))
        })
        .ok()
        .flatten()
    }

    pub fn revoke(&self, id: &str) -> Result<bool, &'static str> {
        self.access(true, |guard| {
            if let Some(session) = guard.get_mut(id) {
                session.revoked = true;
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }

    pub fn revoke_bearer_sessions(&self, bearer: &str) -> Result<usize, &'static str> {
        self.access(true, |guard| {
            let mut n = 0;
            for session in guard.values_mut() {
                if session.bearer == bearer && !session.revoked {
                    session.revoked = true;
                    n += 1;
                }
            }
            Ok(n)
        })
    }

    pub(super) fn connections(
        &self,
        allows: impl Fn(&str) -> bool,
    ) -> Result<Vec<Value>, &'static str> {
        self.access(false, |sessions| {
            let now = now_ms();
            Ok(sessions.values().filter(|s| s.expires_at_ms > now && allows(&s.bearer))
                .map(|s| json!({"connection_id":s.connection_id,"kind":"browser","created_at_ms":s.created_at_ms,
                    "expires_at_ms":s.expires_at_ms,"last_seen_ms":s.last_seen_ms,"state":if s.revoked {"revoked"} else {"active"},
                    "revocation_scope":"connection"})).collect())
        })
    }

    pub(super) fn revoke_connection(
        &self,
        connection: &str,
        allows: impl Fn(&str) -> bool,
    ) -> Result<bool, &'static str> {
        self.access(true, |sessions| {
            let now = now_ms();
            let session = sessions.values_mut().find(|s| {
                s.connection_id == connection && s.expires_at_ms > now && allows(&s.bearer)
            });
            match session {
                Some(session) => {
                    session.revoked = true;
                    Ok(true)
                }
                None => Ok(false),
            }
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn new_session_id() -> Result<String, Response> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| {
        response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"code":"Unavailable","message":"operating-system randomness unavailable"}),
        )
    })?;
    Ok(format!(
        "ws_{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}

pub fn router(state: Arc<StateData>) -> Router {
    Router::new()
        .route("/v1/web/login", post(login))
        .route("/v1/web/logout", post(logout))
        .route("/v1/web/session", get(session_inspect))
        .route("/v1/web/session/revoke", post(session_revoke))
        .route(
            "/v1/web/projects/{project}/connections/list",
            post(connections_list),
        )
        .route(
            "/v1/web/projects/{project}/connections/revoke",
            post(connections_revoke),
        )
        .route("/v1/web/projects", get(list_projects))
        .route("/v1/web/projects/{project}/query", post(web_query))
        .route("/v1/web/projects/{project}/command", post(web_command))
        .route(
            "/v1/web/projects/{project}/access/inspect",
            post(web_access_inspect),
        )
        .route(
            "/v1/web/projects/{project}/access/preview",
            post(web_access_preview),
        )
        .route(
            "/v1/web/projects/{project}/access/apply",
            post(web_access_apply),
        )
        .route(
            "/v1/web/projects/{project}/access/outcome",
            post(web_access_outcome),
        )
        .route("/v1/web/preflight", axum::routing::any(preflight))
        .with_state(state)
}

fn origin_allowed(state: &StateData, headers: &HeaderMap) -> Option<String> {
    let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
    if origin.is_empty() || origin == "null" {
        return None;
    }
    if state
        .web_origins
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(origin))
    {
        Some(origin.to_string())
    } else {
        None
    }
}

fn web_host_ok(state: &StateData, headers: &HeaderMap) -> bool {
    headers.get_all(header::HOST).iter().count() == 1
        && headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| {
                state
                    .hosts
                    .iter()
                    .any(|allowed| h.eq_ignore_ascii_case(allowed))
            })
}

fn require_web_entry(state: &StateData, headers: &HeaderMap) -> Result<String, Response> {
    if state.web_origins.is_empty() {
        return Err(response(
            StatusCode::FORBIDDEN,
            json!({"code":"WebEntryDisabled","message":"web entry requires allowed_web_origins"}),
        ));
    }
    if !web_host_ok(state, headers) {
        return Err(denied());
    }
    let Some(origin) = origin_allowed(state, headers) else {
        return Err(response(
            StatusCode::FORBIDDEN,
            json!({"code":"ForbiddenOrigin","message":"origin is not allowlisted for the web entry"}),
        ));
    };
    Ok(origin)
}

fn require_guard(headers: &HeaderMap) -> Result<(), Response> {
    let ok = headers
        .get(WEB_GUARD_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "1");
    if ok {
        Ok(())
    } else {
        Err(response(
            StatusCode::FORBIDDEN,
            json!({"code":"Forbidden","message":"missing web guard header"}),
        ))
    }
}

fn with_cors(mut response: Response, origin: &str) -> Response {
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn set_session_cookie(response: &mut Response, session_id: &str, max_age: u64, secure: bool) {
    let secure_flag = if secure { "; Secure" } else { "" };
    let value = format!(
        "{COOKIE_NAME}={session_id}; HttpOnly; Path=/v1/web; SameSite=Strict; Max-Age={max_age}{secure_flag}"
    );
    if let Ok(v) = HeaderValue::from_str(&value) {
        response.headers_mut().append(header::SET_COOKIE, v);
    }
}

fn clear_session_cookie(response: &mut Response, secure: bool) {
    let secure_flag = if secure { "; Secure" } else { "" };
    let value =
        format!("{COOKIE_NAME}=; HttpOnly; Path=/v1/web; SameSite=Strict; Max-Age=0{secure_flag}");
    if let Ok(v) = HeaderValue::from_str(&value) {
        response.headers_mut().append(header::SET_COOKIE, v);
    }
}

fn cookie_session_id(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(&format!("{COOKIE_NAME}=")) {
            let id = rest.trim();
            if !id.is_empty()
                && id.len() <= 200
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn secure_cookie(state: &StateData) -> bool {
    !state
        .hosts
        .iter()
        .all(|h| h.starts_with("127.0.0.1") || h.starts_with("localhost") || h.starts_with("[::1]"))
}

fn capabilities_query() -> WorkstreamQuery {
    serde_json::from_value(json!({
        "protocol_version": 1,
        "op": "capabilities"
    }))
    .expect("capabilities query shape")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    bearer: String,
    /// Optional project key. When set, authenticate only against that binding.
    /// When omitted, discover any authorized binding without exposing others.
    #[serde(default)]
    project: Option<String>,
}

async fn login(State(state): State<Arc<StateData>>, headers: HeaderMap, body: Bytes) -> Response {
    let origin = match require_web_entry(&state, &headers) {
        Ok(o) => o,
        Err(r) => return r,
    };
    if let Err(r) = require_guard(&headers) {
        return with_cors(r, &origin);
    }
    let parsed: LoginBody = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return with_cors(
                response(
                    StatusCode::BAD_REQUEST,
                    json!({"code":"InvalidInput","message":"invalid login body"}),
                ),
                &origin,
            );
        }
    };
    if parsed.bearer.is_empty()
        || parsed.bearer.len() > 512
        || parsed.bearer.chars().any(char::is_control)
    {
        return with_cors(denied(), &origin);
    }
    let authorized = match authorized_project_bindings(
        &state,
        &parsed.bearer,
        parsed.project.as_deref(),
    )
    .await
    {
        Ok(list) if !list.is_empty() => list,
        Ok(_) => return with_cors(denied(), &origin),
        Err(PgError::Forbidden) => return with_cors(denied(), &origin),
        Err(error) => return with_cors(error_response(error), &origin),
    };
    let project_keys: Vec<String> = authorized.iter().map(|p| p.key.clone()).collect();
    let session_id = match new_session_id() {
        Ok(id) => id,
        Err(r) => return with_cors(r, &origin),
    };
    let now = now_ms();
    let session = WebSession {
        connection_id: WebSession::reference(&session_id),
        id: session_id.clone(),
        bearer: parsed.bearer,
        created_at_ms: now,
        expires_at_ms: now + SESSION_TTL_SECS * 1000,
        last_seen_ms: now,
        revoked: false,
    };
    if state.web_sessions.insert(session).is_err() {
        return with_cors(
            response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"code":"Busy","message":"web session capacity exceeded"}),
            ),
            &origin,
        );
    }
    let mut res = response(
        StatusCode::OK,
        json!({
            "ok": true,
            "protocol": "awr-team-web-entry",
            "protocol_version": 1,
            "session_id": session_id,
            "expires_at_ms": now + SESSION_TTL_SECS * 1000,
            "projects": project_keys,
            "auth": {
                "kind": "http_only_cookie",
                "cookie": COOKIE_NAME,
                "bearer_in_page": false,
                "logout": "/v1/web/logout",
                "revoke": "/v1/web/session/revoke"
            }
        }),
    );
    set_session_cookie(
        &mut res,
        &session_id,
        SESSION_TTL_SECS,
        secure_cookie(&state),
    );
    with_cors(res, &origin)
}

async fn logout(State(state): State<Arc<StateData>>, headers: HeaderMap) -> Response {
    let origin = match require_web_entry(&state, &headers) {
        Ok(o) => o,
        Err(r) => return r,
    };
    if let Err(r) = require_guard(&headers) {
        return with_cors(r, &origin);
    }
    if let Some(id) = cookie_session_id(&headers) {
        if state.web_sessions.revoke(&id).is_err() {
            return with_cors(super::unavailable(), &origin);
        }
    }
    let mut res = response(StatusCode::OK, json!({"ok": true, "logged_out": true}));
    clear_session_cookie(&mut res, secure_cookie(&state));
    with_cors(res, &origin)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeBody {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    all_mine: bool,
}

async fn session_revoke(
    State(state): State<Arc<StateData>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let origin = match require_web_entry(&state, &headers) {
        Ok(o) => o,
        Err(r) => return r,
    };
    if let Err(r) = require_guard(&headers) {
        return with_cors(r, &origin);
    }
    let Some(current_id) = cookie_session_id(&headers) else {
        return with_cors(denied(), &origin);
    };
    let Some(current) = state.web_sessions.get_live(&current_id) else {
        return with_cors(denied(), &origin);
    };
    let parsed: RevokeBody = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return with_cors(
                response(
                    StatusCode::BAD_REQUEST,
                    json!({"code":"InvalidInput","message":"invalid revoke body"}),
                ),
                &origin,
            );
        }
    };
    let mut revoked = Vec::new();
    if parsed.all_mine {
        let n = match state.web_sessions.revoke_bearer_sessions(&current.bearer) {
            Ok(n) => n,
            Err(_) => return with_cors(super::unavailable(), &origin),
        };
        revoked.push(json!({"scope":"all_mine","count": n}));
    } else {
        let target = parsed
            .session_id
            .as_deref()
            .unwrap_or(current_id.as_str())
            .to_string();
        if let Some(target_session) = state.web_sessions.get_live(&target) {
            if target_session.bearer != current.bearer {
                return with_cors(denied(), &origin);
            }
        }
        let ended = match state.web_sessions.revoke(&target) {
            Ok(ended) => ended,
            Err(_) => return with_cors(super::unavailable(), &origin),
        };
        if ended {
            revoked.push(json!({"session_id": target}));
        }
    }
    let clear_self = parsed.all_mine
        || parsed.session_id.as_deref() == Some(current_id.as_str())
        || parsed.session_id.is_none();
    let mut res = response(StatusCode::OK, json!({"ok": true, "revoked": revoked}));
    if clear_self {
        clear_session_cookie(&mut res, secure_cookie(&state));
    }
    with_cors(res, &origin)
}

async fn session_inspect(State(state): State<Arc<StateData>>, headers: HeaderMap) -> Response {
    let origin = match require_web_entry(&state, &headers) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let Some(id) = cookie_session_id(&headers) else {
        return with_cors(
            response(
                StatusCode::UNAUTHORIZED,
                json!({"code":"Unauthenticated","message":"no web session"}),
            ),
            &origin,
        );
    };
    let Some(session) = state.web_sessions.get_live(&id) else {
        return with_cors(
            response(
                StatusCode::UNAUTHORIZED,
                json!({"code":"SessionExpired","message":"web session expired or revoked"}),
            ),
            &origin,
        );
    };
    let projects = match authorized_project_keys(&state, &session.bearer).await {
        Ok(keys) => keys,
        Err(_) => Vec::<String>::new(),
    };
    with_cors(
        response(
            StatusCode::OK,
            json!({
                "ok": true,
                "session_id": session.id,
                "created_at_ms": session.created_at_ms,
                "expires_at_ms": session.expires_at_ms,
                "last_seen_ms": session.last_seen_ms,
                "projects": projects,
            }),
        ),
        &origin,
    )
}

async fn list_projects(State(state): State<Arc<StateData>>, headers: HeaderMap) -> Response {
    let origin = match require_web_entry(&state, &headers) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let Some(id) = cookie_session_id(&headers) else {
        return with_cors(denied(), &origin);
    };
    let Some(session) = state.web_sessions.get_live(&id) else {
        return with_cors(
            response(
                StatusCode::UNAUTHORIZED,
                json!({"code":"SessionExpired","message":"web session expired or revoked"}),
            ),
            &origin,
        );
    };
    let projects = match authorized_project_bindings(&state, &session.bearer, None).await {
        Ok(list) => list
            .into_iter()
            .map(|p| {
                json!({
                    "key": p.key,
                    "tenant_id": p.tenant_id,
                    "project_id": p.project_id,
                })
            })
            .collect::<Vec<Value>>(),
        Err(PgError::Forbidden) => Vec::new(),
        Err(error) => return with_cors(error_response(error), &origin),
    };
    with_cors(
        response(
            StatusCode::OK,
            json!({"ok": true, "projects": projects, "view": "my_projects"}),
        ),
        &origin,
    )
}

async fn preflight(State(state): State<Arc<StateData>>, headers: HeaderMap) -> Response {
    let origin = match require_web_entry(&state, &headers) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type, x-awr-web"),
    );
    res.headers_mut().insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    with_cors(res, &origin)
}

/// Projects the bearer is currently authorized for (capabilities probe).
/// `only_key` selects one binding; otherwise all configured projects are checked.
/// Unauthorized bindings are omitted — never enumerated to the client.
async fn authorized_project_bindings(
    state: &StateData,
    bearer: &str,
    only_key: Option<&str>,
) -> Result<Vec<ProjectBinding>, PgError> {
    let candidates: Vec<&ProjectBinding> = if let Some(key) = only_key {
        match state.projects.get(key) {
            Some(p) => vec![p],
            None => return Ok(Vec::new()),
        }
    } else {
        state.projects.values().collect()
    };
    let mut out = Vec::new();
    for project in candidates {
        match state
            .store
            .query(
                &project.tenant_id,
                &project.project_id,
                bearer,
                capabilities_query(),
            )
            .await
        {
            Ok(_) => out.push(project.clone()),
            Err(PgError::Forbidden) => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(out)
}

async fn authorized_project_keys(state: &StateData, bearer: &str) -> Result<Vec<String>, PgError> {
    Ok(authorized_project_bindings(state, bearer, None)
        .await?
        .into_iter()
        .map(|p| p.key)
        .collect())
}

async fn require_session(
    state: &StateData,
    headers: &HeaderMap,
) -> Result<(String, WebSession), Response> {
    let origin = require_web_entry(state, headers)?;
    require_guard(headers).map_err(|r| with_cors(r, &origin))?;
    let Some(session_id) = cookie_session_id(headers) else {
        return Err(with_cors(
            response(
                StatusCode::UNAUTHORIZED,
                json!({"code":"Unauthenticated","message":"no web session"}),
            ),
            &origin,
        ));
    };
    let Some(session) = state.web_sessions.get_live(&session_id) else {
        return Err(with_cors(
            response(
                StatusCode::UNAUTHORIZED,
                json!({"code":"SessionExpired","message":"web session expired or revoked"}),
            ),
            &origin,
        ));
    };
    Ok((origin, session))
}

async fn web_query(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (origin, session) = match require_session(&state, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let result = dispatch_authorized(state, key, &session.bearer, body, false).await;
    with_cors(result, &origin)
}

async fn connections_list(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    connections_dispatch(state, key, headers, body, false).await
}

async fn connections_revoke(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    connections_dispatch(state, key, headers, body, true).await
}

async fn connections_dispatch(
    state: Arc<StateData>,
    key: String,
    headers: HeaderMap,
    body: Bytes,
    revoke: bool,
) -> Response {
    let (origin, session) = match require_session(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return error,
    };
    let Some(project) = state.projects.get(&key) else {
        return with_cors(denied(), &origin);
    };
    let request = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return with_cors(
                response(StatusCode::BAD_REQUEST, json!({"code":"InvalidInput"})),
                &origin,
            );
        }
    };
    let action = if revoke {
        "connections.revoke"
    } else {
        "connections.list"
    };
    let Ok(_permit) = state.permits.try_acquire() else {
        return with_cors(super::unavailable(), &origin);
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        super::audited(
            &state,
            project,
            &session.bearer,
            action,
            None,
            super::connections::operate(
                &state,
                project,
                &session.bearer,
                request,
                revoke,
                Some(&session.connection_id),
            ),
        ),
    )
    .await
    .unwrap_or_else(|_| {
        Err(PgError::Unsupported(
            "connection result is unconfirmed".into(),
        ))
    });
    let res = match result {
        Ok(data) => response(StatusCode::OK, json!({"ok":true,"data":data})),
        Err(error) => error_response(error),
    };
    with_cors(res, &origin)
}

async fn web_command(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (origin, session) = match require_session(&state, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let result = dispatch_authorized(state, key, &session.bearer, body, true).await;
    with_cors(result, &origin)
}

async fn web_access_inspect(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    web_access(state, key, headers, body, AccessOp::Inspect).await
}
async fn web_access_preview(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    web_access(state, key, headers, body, AccessOp::Preview).await
}
async fn web_access_apply(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    web_access(state, key, headers, body, AccessOp::Apply).await
}
async fn web_access_outcome(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    web_access(state, key, headers, body, AccessOp::Outcome).await
}

async fn web_access(
    state: Arc<StateData>,
    key: String,
    headers: HeaderMap,
    body: Bytes,
    op: AccessOp,
) -> Response {
    let (origin, session) = match require_session(&state, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let result = access_dispatch_authorized(state, key, &session.bearer, body, op).await;
    with_cors(result, &origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_store_expires_and_revokes() {
        let store = WebSessionStore::default();
        let now = now_ms();
        store
            .insert(WebSession {
                connection_id: WebSession::reference("ws_a"),
                id: "ws_a".into(),
                bearer: "tok".into(),
                created_at_ms: now,
                expires_at_ms: now + 60_000,
                last_seen_ms: now,
                revoked: false,
            })
            .unwrap();
        assert!(store.get_live("ws_a").is_some());
        assert!(store.revoke("ws_a").unwrap());
        assert!(store.get_live("ws_a").is_none());
    }

    #[test]
    fn cookie_parser_accepts_only_safe_ids() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("awr_web_session=ws_abc123; other=1"),
        );
        assert_eq!(cookie_session_id(&headers).as_deref(), Some("ws_abc123"));
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("awr_web_session=../evil"),
        );
        assert!(cookie_session_id(&headers).is_none());
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("awr_web_session=bad value"),
        );
        assert!(cookie_session_id(&headers).is_none());
    }
}
