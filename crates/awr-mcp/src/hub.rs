//! Operator-owned project registry and per-request access for the shared service.
use crate::{AwrServer, Error, Result, operations, project::database};
use awr_core::{Id, WorkstreamAccess, WorkstreamGrant};
use awr_store::Store;
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u32,
    projects: Vec<ProjectConfig>,
    clients: Vec<ClientConfig>,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_origins: Vec<String>,
    /// Offer Team MCP tools on this endpoint. `AWR_TEAM_ENABLED` overrides it.
    #[serde(default)]
    team_enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectConfig {
    key: String,
    root: PathBuf,
    project_id: Id,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientConfig {
    id: String,
    token_env: String,
    #[serde(default)]
    read: Vec<String>,
    #[serde(default)]
    write: Vec<String>,
    #[serde(default)]
    workstreams: Vec<WorkstreamConfig>,
}

/// Operator policy, never a tool argument. Version 2 initially exposes reads.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkstreamConfig {
    project: String,
    workstream_id: Id,
    authority_version: u64,
}

pub(crate) struct ProjectService {
    pub root: PathBuf,
    pub id: Option<Id>,
    pub operation: RwLock<()>,
}
impl ProjectService {
    pub fn open(root: &Path, id: Option<Id>) -> Result<Self> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(Error::InvalidInput(
                "MCP project root must be a directory".into(),
            ));
        }
        let service = Self {
            root,
            id,
            operation: RwLock::new(()),
        };
        service.validate()?;
        Ok(service)
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(id) = self.id {
            // Do not initialize, migrate, or silently accept a replacement project.
            if self.root.canonicalize()? != self.root {
                return Err(Error::RuleViolation(
                    "registered project root changed".into(),
                ));
            }
            let store = Store::open_readonly(&database(&self.root)?)?;
            if store.project_by_root(&self.root)?.id != id {
                return Err(Error::RuleViolation(
                    "registered project identity changed".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn call(
        &self,
        name: &str,
        args: rmcp::model::JsonObject,
        principal: Option<&Principal>,
    ) -> Result<rmcp::model::CallToolResult> {
        if operations::is_read_only(name) {
            let _guard = self.operation.read().map_err(|_| {
                Error::Storage("MCP project operation lock is poisoned; restart the server".into())
            })?;
            self.validate()?;
            self.call_locked(name, args, principal)
        } else {
            let _guard = self.operation.write().map_err(|_| {
                Error::Storage("MCP project operation lock is poisoned; restart the server".into())
            })?;
            self.validate()?;
            self.call_locked(name, args, principal)
        }
    }

    fn call_locked(
        &self,
        name: &str,
        args: rmcp::model::JsonObject,
        principal: Option<&Principal>,
    ) -> Result<rmcp::model::CallToolResult> {
        if let Some(principal) = principal {
            let id = self.id.expect("shared projects have a verified identity");
            let access = principal.workstreams.get(&id);
            if name == "awr_team_handoff" {
                return crate::team_handoff::handle(Value::Object(args));
            }
            if name == "awr_team_review" {
                return crate::team_review::handle(Value::Object(args));
            }
            if name == "awr_workstream" {
                return crate::workstreams::call(&self.root, id, access, args);
            }
            // A project-wide grant must never become an implicit grant to every
            // workstream. This also covers flat aliases in hierarchical mode.
            if access.is_some() || crate::workstreams::enabled(&self.root, id)? {
                return Err(Error::Unsupported(
                    "shared workstream operations require awr_workstream; inspect its capabilities; this legacy operation was not executed".into(),
                ));
            }
        }
        operations::call_as(&self.root, name, args, principal.map(|p| p.id.as_str()))
    }
}

#[derive(Clone)]
pub(crate) struct Principal {
    pub id: String,
    read: BTreeSet<String>,
    write: BTreeSet<String>,
    workstreams: BTreeMap<Id, WorkstreamAccess>,
}
struct Credential {
    hash: [u8; 32],
    principal: Principal,
}
pub struct Hub {
    pub(crate) capacity: Arc<tokio::sync::Semaphore>,
    projects: BTreeMap<String, Arc<ProjectService>>,
    credentials: Vec<Credential>,
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
    pub(crate) team: bool,
}
fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
impl Hub {
    pub fn load(path: &Path) -> Result<Arc<Self>> {
        if std::fs::metadata(path)?.len() > 1024 * 1024 {
            return Err(Error::InvalidInput("MCP registry exceeds 1 MiB".into()));
        }
        let config: Config = toml::from_str(&std::fs::read_to_string(path)?)
            .map_err(|_| Error::InvalidInput("invalid MCP registry configuration".into()))?;
        if !matches!(config.version, 1 | 2)
            || config.projects.is_empty()
            || config.clients.is_empty()
            || config.projects.len() > 1000
            || config.clients.len() > 1000
        {
            return Err(Error::InvalidInput(
                "registry version must be 1 or 2 with 1..1000 projects and clients".into(),
            ));
        }
        let mut projects = BTreeMap::new();
        let mut roots = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for entry in config.projects {
            if !key(&entry.key) || !entry.root.is_absolute() {
                return Err(Error::InvalidInput(
                    "project keys must be identifiers and roots absolute".into(),
                ));
            }
            let service = ProjectService::open(&entry.root, Some(entry.project_id))?;
            if !roots.insert(service.root.clone())
                || !ids.insert(entry.project_id)
                || projects.insert(entry.key, Arc::new(service)).is_some()
            {
                return Err(Error::InvalidInput(
                    "duplicate project key, root or identity".into(),
                ));
            }
        }
        let mut credentials = Vec::new();
        let mut client_ids = BTreeSet::new();
        let mut hashes = BTreeSet::new();
        for client in config.clients {
            if !key(&client.id) || !client_ids.insert(client.id.clone()) || !key(&client.token_env)
            {
                return Err(Error::InvalidInput(
                    "invalid or duplicate client identity".into(),
                ));
            }
            let token = std::env::var(&client.token_env).map_err(|_| {
                Error::InvalidInput(
                    "a configured client token environment variable is missing".into(),
                )
            })?;
            if token.len() < 32
                || token.len() > 4096
                || !token.bytes().all(|b| b.is_ascii_graphic())
            {
                return Err(Error::InvalidInput(
                    "client tokens must contain 32..4096 visible ASCII bytes".into(),
                ));
            }
            let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            if !hashes.insert(hash) {
                return Err(Error::InvalidInput("client tokens must be distinct".into()));
            }
            let write: BTreeSet<_> = client.write.into_iter().collect();
            let mut read: BTreeSet<_> = client.read.into_iter().collect();
            read.extend(write.iter().cloned());
            if read.is_empty() || read.iter().any(|k| !projects.contains_key(k)) {
                return Err(Error::InvalidInput(
                    "client grants must name registered project keys".into(),
                ));
            }
            if config.version == 1 && !client.workstreams.is_empty() {
                return Err(Error::InvalidInput(
                    "workstream grants require registry version 2".into(),
                ));
            }
            let mut workstreams = BTreeMap::<Id, WorkstreamAccess>::new();
            for grant in client.workstreams {
                if !read.contains(&grant.project) {
                    return Err(Error::InvalidInput(
                        "workstream grants require access to the registered project".into(),
                    ));
                }
                let id = projects[&grant.project].id.expect("registered project id");
                workstreams
                    .entry(id)
                    .or_insert_with(|| WorkstreamAccess {
                        project_id: id.to_string(),
                        subject: client.id.clone(),
                        grants: Vec::new(),
                    })
                    .grants
                    .push(WorkstreamGrant {
                        workstream_id: grant.workstream_id,
                        authority_version: grant.authority_version,
                        read: true,
                        write: false,
                        manage: false,
                    });
            }
            for access in workstreams.values() {
                access.validate()?;
            }
            credentials.push(Credential {
                hash,
                principal: Principal {
                    id: client.id,
                    read,
                    write,
                    workstreams,
                },
            });
        }
        Ok(Arc::new(Self {
            capacity: Arc::new(tokio::sync::Semaphore::new(64)),
            projects,
            credentials,
            allowed_hosts: config.allowed_hosts,
            allowed_origins: config.allowed_origins,
            team: awr_source::team_enabled_override()?.unwrap_or(config.team_enabled),
        }))
    }
    fn authenticate(&self, headers: &HeaderMap) -> Option<Principal> {
        let header = headers
            .get(axum::http::header::AUTHORIZATION)?
            .to_str()
            .ok()?;
        let (scheme, token) = header.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("Bearer") {
            return None;
        }
        let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.credentials
            .iter()
            .find(|entry| {
                candidate
                    .iter()
                    .zip(entry.hash)
                    .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
                    == 0
            })
            .map(|entry| entry.principal.clone())
    }
    pub(crate) fn select(
        &self,
        principal: &Principal,
        project: &str,
        write: bool,
    ) -> Result<Arc<ProjectService>> {
        if !principal.read.contains(project) || (write && !principal.write.contains(project)) {
            return Err(Error::RuleViolation("project access denied".into()));
        }
        self.projects
            .get(project)
            .cloned()
            .ok_or_else(|| Error::RuleViolation("project access denied".into()))
    }
    pub(crate) fn catalog(&self, principal: &Principal) -> Value {
        json!({"ok":true,"read_only":true,"client":principal.id,"projects":principal.read.iter().map(|key| {
            json!({"key":key,"project_id":self.projects[key].id,"access":if principal.write.contains(key){"write"}else{"read"}})
        }).collect::<Vec<_>>(),"selection":"pass project on every project-scoped call; no shared current project"})
    }
    pub fn router(self: &Arc<Self>) -> Router {
        let mut config = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true);
        if !self.allowed_hosts.is_empty() {
            config.allowed_hosts = self.allowed_hosts.clone();
        }
        config.max_request_body_bytes = 2 * 1024 * 1024;
        let hub = self.clone();
        let service: StreamableHttpService<AwrServer, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(AwrServer::shared(hub.clone())),
                Default::default(),
                config,
            );
        Router::new()
            .nest_service("/mcp", service)
            .layer(middleware::from_fn_with_state(self.clone(), authorize))
    }
}

async fn authorize(State(hub): State<Arc<Hub>>, mut request: Request, next: Next) -> Response {
    // An empty allow-list denies all browser origins, while native clients omit Origin.
    if let Some(origin) = request.headers().get(axum::http::header::ORIGIN) {
        if !origin
            .to_str()
            .is_ok_and(|origin| hub.allowed_origins.iter().any(|allowed| allowed == origin))
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let Some(principal) = hub.authenticate(request.headers()) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response();
    };
    request.extensions_mut().insert(principal);
    next.run(request).await
}
