//! Operator-bound multi-project HTTP/MCP service. Every request authenticates
//! inside PostgreSQL; tenant/actor/client/grants are never taken from its JSON.
mod action_auth;
mod mcp;
mod web;

pub use crate::named_agent_host::{
    named_agent_host_capabilities, negotiate_named_adapter, usable_named_clients,
};
pub use action_auth::{
    action_authorization_capabilities, command_action_name, query_action_name,
    reject_access_management_forgeries, reject_forged_authority_fields,
};

use awr_team_pg::{
    AdminAccessPlan, PgError, PlanningApproveRequest, PlanningDraftRequest, PlanningPublishRequest,
    PlanningSuggestRequest, WorkstreamCommand, WorkstreamCommandStore, WorkstreamQuery,
    WorkstreamReadStore,
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap, net::SocketAddr, path::Path as FilePath, sync::Arc, time::Duration,
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectBinding {
    pub key: String,
    pub tenant_id: String,
    pub project_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub version: u32,
    pub listen: SocketAddr,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Exact browser Origins allowed to use the designed `/v1/web/*` entry (WS-044).
    /// Classic `/v1/projects/*` and MCP continue to reject any Origin.
    #[serde(default)]
    pub allowed_web_origins: Vec<String>,
    pub projects: Vec<ProjectBinding>,
}

impl ServiceConfig {
    pub fn read(path: &FilePath) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|_| "could not read service configuration".to_string())?;
        if text.len() > 262144 {
            return Err("service configuration is too large".into());
        }
        let config: Self =
            toml::from_str(&text).map_err(|_| "invalid service configuration".to_string())?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.projects.is_empty() || self.projects.len() > 1024 {
            return Err("unsupported version or project count".into());
        }
        let mut keys = std::collections::BTreeSet::new();
        for p in &self.projects {
            if p.key.is_empty()
                || p.key.len() > 128
                || !p
                    .key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                || !keys.insert(&p.key)
                || [&p.tenant_id, &p.project_id]
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
            {
                return Err("invalid or duplicate project binding".into());
            }
        }
        if self.allowed_hosts.len() > 64
            || self.allowed_hosts.iter().any(|s| {
                s.is_empty()
                    || s.len() > 255
                    || s.chars().any(|c| c.is_whitespace())
                    || s.contains(['*', '/', '@'])
            })
        {
            return Err("allowed_hosts must be exact host authorities".into());
        }
        if !self.listen.ip().is_loopback() && self.allowed_hosts.is_empty() {
            return Err(
                "remote listeners require explicit allowed_hosts and operator TLS termination"
                    .into(),
            );
        }
        if self.allowed_web_origins.len() > 64
            || self.allowed_web_origins.iter().any(|s| {
                s.is_empty()
                    || s.len() > 255
                    || !(s.starts_with("http://") || s.starts_with("https://"))
                    || s.chars().any(|c| c.is_whitespace())
                    || s.contains('*')
            })
        {
            return Err("allowed_web_origins must be exact http(s) Origins".into());
        }
        Ok(())
    }
}

pub(crate) struct StateData {
    store: WorkstreamReadStore,
    commands: WorkstreamCommandStore,
    projects: BTreeMap<String, ProjectBinding>,
    hosts: Vec<String>,
    web_origins: Vec<String>,
    web_sessions: Arc<web::WebSessionStore>,
    permits: Arc<tokio::sync::Semaphore>,
}

pub fn router(
    config: ServiceConfig,
    actual: SocketAddr,
    store: WorkstreamReadStore,
) -> Result<Router, String> {
    config.validate()?;
    let mut hosts = config.allowed_hosts;
    if actual.ip().is_loopback() {
        hosts.push(actual.to_string());
        hosts.push(format!("localhost:{}", actual.port()));
    }
    let web_origins = config.allowed_web_origins;
    let state = Arc::new(StateData {
        commands: store.commands(),
        store,
        projects: config
            .projects
            .into_iter()
            .map(|p| (p.key.clone(), p))
            .collect(),
        hosts,
        web_origins,
        web_sessions: Arc::new(web::WebSessionStore::default()),
        permits: Arc::new(tokio::sync::Semaphore::new(64)),
    });
    let mut router = Router::new()
        .route("/v1/projects/{project}/query", post(query))
        .route("/v1/projects/{project}/command", post(command))
        .route(
            "/v1/projects/{project}/access/inspect",
            post(access_inspect),
        )
        .route(
            "/v1/projects/{project}/access/preview",
            post(access_preview),
        )
        .route("/v1/projects/{project}/access/apply", post(access_apply))
        .route(
            "/v1/projects/{project}/access/outcome",
            post(access_outcome),
        )
        .route(
            "/v1/projects/{project}/planning/suggest",
            post(planning_suggest),
        )
        .route(
            "/v1/projects/{project}/planning/draft",
            post(planning_draft),
        )
        .route(
            "/v1/projects/{project}/planning/preview",
            post(planning_preview),
        )
        .route(
            "/v1/projects/{project}/planning/approve",
            post(planning_approve),
        )
        .route(
            "/v1/projects/{project}/planning/publish",
            post(planning_publish),
        )
        .route(
            "/v1/projects/{project}/planning/outcome",
            post(planning_outcome),
        )
        .layer(DefaultBodyLimit::max(65536))
        .with_state(state.clone());
    for project in state.projects.values() {
        router = router.merge(mcp::router(state.clone(), project.clone()));
    }
    router = router.merge(web::router(state.clone()));
    Ok(router)
}

pub(crate) fn response(status: StatusCode, value: Value) -> Response {
    (
        status,
        [
            ("content-type", "application/json"),
            ("cache-control", "no-store"),
        ],
        value.to_string(),
    )
        .into_response()
}

pub(crate) fn denied() -> Response {
    response(
        StatusCode::FORBIDDEN,
        json!({"code":"Forbidden","message":"access denied"}),
    )
}

async fn query(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    dispatch(state, key, headers, body, false).await
}

async fn command(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    dispatch(state, key, headers, body, true).await
}

async fn dispatch(
    state: Arc<StateData>,
    key: String,
    headers: HeaderMap,
    body: Bytes,
    write: bool,
) -> Response {
    if !allowed_request(&state, &headers) {
        return denied();
    }
    let Some(token) = bearer(&headers) else {
        return denied();
    };
    dispatch_authorized(state, key, token, body, write).await
}

/// Shared query/command path used by classic bearer transport and the Web entry.
pub(crate) async fn dispatch_authorized(
    state: Arc<StateData>,
    key: String,
    token: &str,
    body: Bytes,
    write: bool,
) -> Response {
    let Some(project) = state.projects.get(&key) else {
        return denied();
    };
    enum Request {
        Query(WorkstreamQuery),
        Command(WorkstreamCommand),
    }
    // Body/selectors never supply identity or grants (TMCP-011).
    if let Ok(raw) = serde_json::from_slice::<Value>(&body) {
        if reject_forged_authority_fields(&raw).is_err() {
            return denied();
        }
    }
    let parsed = if write {
        serde_json::from_slice(&body).map(Request::Command)
    } else {
        serde_json::from_slice(&body).map(Request::Query)
    };
    let request = match parsed {
        Ok(r) => r,
        Err(_) => {
            return response(
                StatusCode::BAD_REQUEST,
                json!({"code":"InvalidInput","message":"invalid workstream request"}),
            );
        }
    };
    let Ok(_permit) = state.permits.try_acquire() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, json!({"code":"Busy"}));
    };
    let (action, work) = match &request {
        Request::Query(q) => (
            if WorkstreamQuery::OPERATIONS.contains(&q.op.as_str()) {
                q.op.clone()
            } else {
                "invalid.query".into()
            },
            q.work_id.clone(),
        ),
        Request::Command(c) => (
            if command_action_name(&c.op).is_some() {
                c.op.clone()
            } else {
                "invalid.command".into()
            },
            Some(c.work_id.clone()),
        ),
    };
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        audited(&state, project, token, &action, work.as_deref(), async {
            match request {
                Request::Query(q) => {
                    state
                        .store
                        .query(&project.tenant_id, &project.project_id, token, q)
                        .await
                }
                Request::Command(c) => {
                    state
                        .commands
                        .execute(&project.tenant_id, &project.project_id, token, c)
                        .await
                }
            }
        }),
    )
    .await;
    match result {
        Ok(Ok(value)) => response(StatusCode::OK, value),
        Ok(Err(error)) => error_response(error),
        Err(_) => unavailable(),
    }
}

/// Metadata is recorded before dispatch; interrupted requests remain unknown.
/// Business receipts remain authoritative when final audit recording fails.
pub(crate) async fn audited(
    state: &StateData,
    project: &ProjectBinding,
    token: &str,
    action: &str,
    work: Option<&str>,
    future: impl std::future::Future<Output = Result<Value, PgError>>,
) -> Result<Value, PgError> {
    let id = state
        .store
        .request_audit_begin(&project.tenant_id, &project.project_id, token, action, work)
        .await?;
    let result = future.await;
    let status = match &result {
        Ok(_) => "succeeded",
        Err(PgError::Forbidden) => "denied",
        Err(
            PgError::Protocol(_)
            | PgError::Unsupported(_)
            | PgError::PreconditionsChanged
            | PgError::IdempotencyConflict,
        ) => "failed",
        Err(_) => "unknown",
    };
    // Do not turn a committed command into a retryable failure. Its access record
    // stays unknown if the final metadata write fails; command.inspect resolves it.
    let _ = state
        .store
        .request_audit_finish(&project.tenant_id, &project.project_id, &id, status)
        .await;
    result
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessInspectBody {
    protocol_version: u32,
    subject_actor_id: Option<String>,
    subject_client_id: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

impl AccessInspectBody {
    async fn inspect(
        self,
        state: &StateData,
        project: &ProjectBinding,
        token: &str,
    ) -> Result<Value, PgError> {
        if self.protocol_version != 1 {
            return Err(PgError::Protocol("invalid access inspect".into()));
        }
        let access = state.store.project_access();
        match (self.subject_actor_id, self.subject_client_id) {
            (Some(actor), Some(client)) if self.cursor.is_none() && self.limit.is_none() => {
                access
                    .inspect(
                        &project.tenant_id,
                        &project.project_id,
                        token,
                        &actor,
                        &client,
                    )
                    .await
            }
            (None, None) => {
                access
                    .members(
                        &project.tenant_id,
                        &project.project_id,
                        token,
                        self.cursor.as_deref(),
                        self.limit.unwrap_or(50),
                    )
                    .await
            }
            _ => Err(PgError::Protocol(
                "supply both subject selectors or neither".into(),
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessPreviewBody {
    protocol_version: u32,
    plan: AdminAccessPlan,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessApplyBody {
    protocol_version: u32,
    request_id: String,
    expected_state: String,
    expected_plan: String,
    plan: AdminAccessPlan,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessOutcomeBody {
    protocol_version: u32,
    request_id: String,
}

async fn access_inspect(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    access_dispatch(state, key, headers, body, AccessOp::Inspect).await
}
async fn access_preview(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    access_dispatch(state, key, headers, body, AccessOp::Preview).await
}
async fn access_apply(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    access_dispatch(state, key, headers, body, AccessOp::Apply).await
}
async fn access_outcome(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    access_dispatch(state, key, headers, body, AccessOp::Outcome).await
}

pub(crate) enum AccessOp {
    Inspect,
    Preview,
    Apply,
    Outcome,
}

pub(crate) async fn access_dispatch(
    state: Arc<StateData>,
    key: String,
    headers: HeaderMap,
    body: Bytes,
    op: AccessOp,
) -> Response {
    if !allowed_request(&state, &headers) {
        return denied();
    }
    let Some(token) = bearer(&headers) else {
        return denied();
    };
    access_dispatch_authorized(state, key, token, body, op).await
}

pub(crate) async fn access_dispatch_authorized(
    state: Arc<StateData>,
    key: String,
    token: &str,
    body: Bytes,
    op: AccessOp,
) -> Response {
    let Some(project) = state.projects.get(&key) else {
        return denied();
    };
    if let Ok(raw) = serde_json::from_slice::<Value>(&body) {
        if reject_access_management_forgeries(&raw).is_err() {
            return denied();
        }
    }
    let Ok(_permit) = state.permits.try_acquire() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, json!({"code":"Busy"}));
    };
    let action = match op {
        AccessOp::Inspect => "access.inspect",
        AccessOp::Preview => "access.preview",
        AccessOp::Apply => "access.apply",
        AccessOp::Outcome => "access.outcome",
    };
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        audited(&state, project, token, action, None, async {
            match op {
                AccessOp::Inspect => {
                    let req: AccessInspectBody = serde_json::from_slice(&body)
                        .map_err(|_| PgError::Protocol("invalid access inspect".into()))?;
                    req.inspect(&state, project, token).await
                }
                AccessOp::Preview => {
                    let req: AccessPreviewBody = serde_json::from_slice(&body)
                        .map_err(|_| PgError::Protocol("invalid access preview".into()))?;
                    if req.protocol_version != 1 {
                        return Err(PgError::Protocol("invalid access preview".into()));
                    }
                    state
                        .store
                        .project_access()
                        .preview(&project.tenant_id, &project.project_id, token, &req.plan)
                        .await
                }
                AccessOp::Apply => {
                    let req: AccessApplyBody = serde_json::from_slice(&body)
                        .map_err(|_| PgError::Protocol("invalid access apply".into()))?;
                    if req.protocol_version != 1 {
                        return Err(PgError::Protocol("invalid access apply".into()));
                    }
                    state
                        .store
                        .project_access()
                        .apply(
                            &project.tenant_id,
                            &project.project_id,
                            token,
                            &req.plan,
                            &req.request_id,
                            &req.expected_state,
                            &req.expected_plan,
                        )
                        .await
                }
                AccessOp::Outcome => {
                    let req: AccessOutcomeBody = serde_json::from_slice(&body)
                        .map_err(|_| PgError::Protocol("invalid access outcome".into()))?;
                    if req.protocol_version != 1 {
                        return Err(PgError::Protocol("invalid access outcome".into()));
                    }
                    state
                        .store
                        .project_access()
                        .outcome(
                            &project.tenant_id,
                            &project.project_id,
                            token,
                            &req.request_id,
                        )
                        .await
                }
            }
        }),
    )
    .await;
    match result {
        Ok(Ok(value)) => response(StatusCode::OK, value),
        Ok(Err(error)) => error_response(error),
        Err(_) => unavailable(),
    }
}

fn allowed_request(state: &StateData, headers: &HeaderMap) -> bool {
    !headers.contains_key("origin")
        && headers.get_all("host").iter().count() == 1
        && headers
            .get("host")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| {
                state
                    .hosts
                    .iter()
                    .any(|allowed| h.eq_ignore_ascii_case(allowed))
            })
}

pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all("authorization").iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value
        .to_str()
        .ok()?
        .split_once(' ')
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, token)| token)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanningPreviewBody {
    protocol_version: u32,
    candidate_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanningOutcomeBody {
    protocol_version: u32,
    request_id: String,
}

async fn planning_suggest(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    planning_dispatch(state, key, headers, body, PlanningOp::Suggest).await
}
async fn planning_draft(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    planning_dispatch(state, key, headers, body, PlanningOp::Draft).await
}
async fn planning_preview(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    planning_dispatch(state, key, headers, body, PlanningOp::Preview).await
}
async fn planning_approve(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    planning_dispatch(state, key, headers, body, PlanningOp::Approve).await
}
async fn planning_publish(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    planning_dispatch(state, key, headers, body, PlanningOp::Publish).await
}
async fn planning_outcome(
    State(state): State<Arc<StateData>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    planning_dispatch(state, key, headers, body, PlanningOp::Outcome).await
}

enum PlanningOp {
    Suggest,
    Draft,
    Preview,
    Approve,
    Publish,
    Outcome,
}

async fn planning_dispatch(
    state: Arc<StateData>,
    key: String,
    headers: HeaderMap,
    body: Bytes,
    op: PlanningOp,
) -> Response {
    if !allowed_request(&state, &headers) {
        return denied();
    }
    let Some(token) = bearer(&headers) else {
        return denied();
    };
    let Some(project) = state.projects.get(&key) else {
        return denied();
    };
    if let Ok(raw) = serde_json::from_slice::<Value>(&body) {
        if reject_forged_authority_fields(&raw).is_err() {
            return denied();
        }
    }
    let Ok(_permit) = state.permits.try_acquire() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, json!({"code":"Busy"}));
    };
    let action = match op {
        PlanningOp::Suggest => "planning.suggest",
        PlanningOp::Draft => "planning.draft",
        PlanningOp::Preview => "planning.preview",
        PlanningOp::Approve => "planning.approve",
        PlanningOp::Publish => "planning.publish",
        PlanningOp::Outcome => "planning.outcome",
    };
    let result = tokio::time::timeout(Duration::from_secs(30), audited(&state, project, token, action, None, async {
        let source = state.store.source();
        match op {
            PlanningOp::Suggest => {
                let req: PlanningSuggestRequest = serde_json::from_slice(&body)
                    .map_err(|_| PgError::Protocol("invalid planning suggest".into()))?;
                // protocol_version is transport-level; strip if present via wrapper
                source
                    .planning_suggest(&project.tenant_id, &project.project_id, token, &req)
                    .await
            }
            PlanningOp::Draft => {
                let req: PlanningDraftRequest = serde_json::from_slice(&body)
                    .map_err(|_| PgError::Protocol("invalid planning draft".into()))?;
                source
                    .planning_draft(&project.tenant_id, &project.project_id, token, &req)
                    .await
            }
            PlanningOp::Preview => {
                let req: PlanningPreviewBody = serde_json::from_slice(&body)
                    .map_err(|_| PgError::Protocol("invalid planning preview".into()))?;
                if req.protocol_version != 1 {
                    return Err(PgError::Protocol("invalid planning preview".into()));
                }
                source
                    .preview_planning_candidate(
                        &project.tenant_id,
                        &project.project_id,
                        token,
                        &req.candidate_id,
                    )
                    .await
            }
            PlanningOp::Approve => {
                let req: PlanningApproveRequest = serde_json::from_slice(&body)
                    .map_err(|_| PgError::Protocol("invalid planning approve".into()))?;
                source
                    .planning_approve(&project.tenant_id, &project.project_id, token, &req)
                    .await
            }
            PlanningOp::Publish => {
                let req: PlanningPublishRequest = serde_json::from_slice(&body)
                    .map_err(|_| PgError::Protocol("invalid planning publish".into()))?;
                source
                    .planning_publish(&project.tenant_id, &project.project_id, token, &req)
                    .await
            }
            PlanningOp::Outcome => {
                let req: PlanningOutcomeBody = serde_json::from_slice(&body)
                    .map_err(|_| PgError::Protocol("invalid planning outcome".into()))?;
                if req.protocol_version != 1 {
                    return Err(PgError::Protocol("invalid planning outcome".into()));
                }
                match source
                    .get_planning_command_receipt(
                        &project.tenant_id,
                        &project.project_id,
                        token,
                        &req.request_id,
                    )
                    .await?
                {
                    Some(v) => Ok(v),
                    None => Ok(json!({
                        "protocol":"awr-team-planning-command-v1",
                        "request_id":req.request_id,
                        "already_recorded":false,
                        "result":null,
                        "next_step":"absent receipt is unknown — wait/retry outcome before submitting a new request_id"
                    })),
                }
            }
        }
    }))
    .await;
    match result {
        Ok(Ok(value)) => response(StatusCode::OK, value),
        Ok(Err(error)) => error_response(error),
        Err(_) => unavailable(),
    }
}

fn unavailable_value() -> Value {
    json!({"code":"Unavailable","message":"request outcome unavailable; inspect a command before retrying with its original identity"})
}

pub(crate) fn unavailable() -> Response {
    response(StatusCode::SERVICE_UNAVAILABLE, unavailable_value())
}

pub(crate) fn error_response(error: PgError) -> Response {
    let (status, value) = public_error(error);
    response(status, value)
}

// Shared by HTTP and MCP; never expose SQL, URLs, credentials or source bodies.
fn public_error(error: PgError) -> (StatusCode, Value) {
    if error.is_invalid_command_fields() {
        return (
            StatusCode::BAD_REQUEST,
            json!({"code":"InvalidInput","message":"command fields or bounds are invalid",
                "next_step":"Check the operation's required args. Version and fence fields are JSON decimal strings (for example \"1\"); use the schema for zero and omitted values."}),
        );
    }
    match error {
        PgError::Forbidden => (
            StatusCode::FORBIDDEN,
            json!({"code":"Forbidden","message":"access denied"}),
        ),
        PgError::Workstream(e) => match e.code() {
            "WorkstreamAccessDenied" | "WorkstreamUnavailable" => public_error(PgError::Forbidden),
            _ => (
                StatusCode::CONFLICT,
                json!({"code":e.code(),"message":e.to_string()}),
            ),
        },
        PgError::Unsupported(msg) => (
            StatusCode::NOT_IMPLEMENTED,
            json!({
                "code":"Unsupported",
                "message": msg,
                "next_step":"use a supported adapter (server_directory sole source) or an implemented planning op; do not hand-edit JSON/SQL"
            }),
        ),
        PgError::ClaimBlocksActivation => (
            StatusCode::CONFLICT,
            json!({
                "code":"ClaimBlocksActivation",
                "message":"publish/activation blocked by in-flight claimed work",
                "next_step":"stop/reconcile/replan affected works, then replay the same request_id with activate=true and stopped_work_ids"
            }),
        ),
        PgError::ActivationImpactUnproven(msg) => (
            StatusCode::CONFLICT,
            json!({
                "code":"ActivationImpactUnproven",
                "message": msg,
                "next_step":"prove impact scope or stop affected in-flight work; cancel/expiry/session-end do not prove process stopped"
            }),
        ),
        PgError::WritebackRefused(msg) => (
            StatusCode::CONFLICT,
            json!({
                "code":"WritebackRefused",
                "message": msg,
                "next_step":"fix the refused source write condition, then inspect planning.outcome before any new request_id"
            }),
        ),
        PgError::CandidateNotApproved => (
            StatusCode::CONFLICT,
            json!({
                "code":"CandidateNotApproved",
                "message":"candidate is not approved for publish",
                "next_step":"approve the current candidate_digest, then publish with the same digest"
            }),
        ),
        PgError::ContextIncomplete => (
            StatusCode::CONFLICT,
            json!({
                "code":"ContextIncomplete",
                "message":"required context exceeds the requested budget or required specs are missing",
                "next_step":"raise max_context_bytes, fetch source.content/artifact.content for missing refs, or restore authorized published specs"
            }),
        ),
        PgError::Protocol(_) => (
            StatusCode::BAD_REQUEST,
            json!({"code":"InvalidInput","message":"query fields or bounds are invalid"}),
        ),
        PgError::CursorExpired => (
            StatusCode::CONFLICT,
            json!({"code":"CursorExpired","message":"refresh the scoped query"}),
        ),
        e @ (PgError::PreconditionsChanged
        | PgError::IdempotencyConflict
        | PgError::EpochChanged
        | PgError::ProjectNotAvailable
        | PgError::RecoveryBlocked
        | PgError::ClaimHeld
        | PgError::LeaseExpired
        | PgError::StaleFence
        | PgError::ScopeExceeded
        | PgError::ResourceConflict
        | PgError::BindingInvalid
        | PgError::WaitOpen) => {
            let code = match e {
                PgError::PreconditionsChanged => "PreconditionsChanged",
                PgError::IdempotencyConflict => "IdempotencyConflict",
                PgError::EpochChanged => "EpochChanged",
                PgError::ProjectNotAvailable => "ProjectNotAvailable",
                PgError::ClaimHeld => "ClaimHeld",
                PgError::LeaseExpired => "LeaseExpired",
                PgError::StaleFence => "StaleFence",
                PgError::ScopeExceeded => "ScopeExceeded",
                PgError::ResourceConflict => "ResourceConflict",
                PgError::BindingInvalid => "BindingInvalid",
                PgError::WaitOpen => "WaitOpen",
                _ => "RecoveryBlocked",
            };
            (
                StatusCode::CONFLICT,
                json!({"code":code,"message":e.to_string()}),
            )
        }
        PgError::ResponseTooLarge => (
            StatusCode::CONFLICT,
            json!({"code":"ResponseTooLarge","message":"response exceeds service limit; use a smaller page or a narrower selector; inspect a command before retrying"}),
        ),
        _ => (StatusCode::SERVICE_UNAVAILABLE, unavailable_value()),
    }
}

pub async fn serve(path: &FilePath) -> Result<(), String> {
    let config = ServiceConfig::read(path)?;
    let url = std::env::var("AWR_TEAM_DATABASE_URL")
        .map_err(|_| "AWR_TEAM_DATABASE_URL is required".to_string())?;
    let store = WorkstreamReadStore::new(url);
    store.check_schema().await.map_err(|error| match error {
        PgError::SchemaIncompatible(_) => {
            "Team schema is incompatible; migrate explicitly as owner".to_string()
        }
        _ => "could not connect to Team database".to_string(),
    })?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|_| "could not bind Team listener".to_string())?;
    let actual = listener
        .local_addr()
        .map_err(|_| "could not inspect listener".to_string())?;
    let router = router(config, actual, store)?;
    println!(
        "{}",
        json!({"service":"awr-team-workstream","listen":actual.to_string(),"protocol_version":1})
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|_| "Team HTTP service stopped with an error".into())
}
