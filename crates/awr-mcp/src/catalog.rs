//! Local catalog mode: one stdio server that resolves projects from the per-user catalog.
//!
//! The catalog is read on every call, so a project registered after the server started is
//! usable at once. Each project is still served with owner semantics, exactly as
//! `awr-mcp --project` serves one; this module only chooses which project a call is for.
use crate::{Error, Result, hub::ProjectService};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Entries returned by `awr_projects_list` when the caller names no `limit`.
const DEFAULT_LIMIT: usize = 25;
const MAX_LIMIT: usize = 200;

pub(crate) struct Catalog {
    dir: PathBuf,
    allow_roots: Vec<PathBuf>,
    /// The operator's `--project`, or the starting directory when it is an initialized project.
    default: Option<Arc<ProjectService>>,
    /// One service per project, so writes to a project stay serialized across calls.
    services: Mutex<BTreeMap<PathBuf, Arc<ProjectService>>>,
}

impl Catalog {
    /// `allow_roots` must be absolute; they are canonicalized here.
    pub(crate) fn open(
        dir: PathBuf,
        allow_roots: &[PathBuf],
        default_root: Option<&Path>,
    ) -> Result<Self> {
        let allow_roots = allow_roots
            .iter()
            .map(|root| {
                if !root.is_absolute() {
                    return Err(Error::InvalidInput(
                        "--allow-root must be an absolute directory".into(),
                    ));
                }
                root.canonicalize().map_err(|_| {
                    Error::InvalidInput("--allow-root must be an existing directory".into())
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let default = match default_root {
            Some(root) => Some(Arc::new(ProjectService::open(root, None)?)),
            None => None,
        };
        Ok(Self {
            dir,
            allow_roots,
            default,
            services: Mutex::new(BTreeMap::new()),
        })
    }

    pub(crate) fn default_root(&self) -> Option<&Path> {
        self.default.as_ref().map(|service| service.root.as_path())
    }

    /// The project a call is for: the one it names, else the default.
    pub(crate) fn select(&self, selector: Option<&str>) -> Result<Arc<ProjectService>> {
        let Some(selector) = selector else {
            return self.default.clone().ok_or_else(|| {
                Error::InvalidInput(
                    "this server has no default project: pass `project` (a key, a unique name or a root path); awr_projects_list shows the registered projects".into(),
                )
            });
        };
        // The default project is always selectable by its own root, registered or not.
        if let Some(default) = &self.default
            && Path::new(selector.trim()).is_absolute()
            && Path::new(selector.trim())
                .canonicalize()
                .is_ok_and(|root| root == default.root)
        {
            return Ok(default.clone());
        }
        let entry = awr_local::resolve(&self.dir, selector, &self.allow_roots)?;
        let id = entry
            .project_id
            .as_deref()
            .and_then(|id| id.parse().ok())
            .ok_or_else(|| Error::NotFound("registered local project".into()))?;
        let mut services = self.services.lock().map_err(|_| {
            Error::Storage("MCP project registry lock is poisoned; restart the server".into())
        })?;
        if let Some(service) = services.get(&entry.root)
            && service.id == Some(id)
        {
            return Ok(service.clone());
        }
        let service = Arc::new(ProjectService::open(&entry.root, Some(id))?);
        services.insert(entry.root.clone(), service.clone());
        Ok(service)
    }

    /// `awr_projects_list`: the registered projects this server can use.
    pub(crate) fn list(&self, args: &serde_json::Map<String, Value>) -> Result<Value> {
        if let Some(unknown) = args
            .keys()
            .find(|key| !matches!(key.as_str(), "query" | "limit"))
        {
            return Err(Error::InvalidInput(format!(
                "unknown argument `{unknown}`; accepted: query, limit"
            )));
        }
        let query = match args.get("query") {
            None => None,
            Some(Value::String(text)) => Some(text.to_lowercase()),
            Some(_) => return Err(Error::InvalidInput("query must be a string".into())),
        };
        let limit = match args.get("limit") {
            None => DEFAULT_LIMIT,
            Some(value) => value
                .as_u64()
                .filter(|limit| (1..=MAX_LIMIT as u64).contains(limit))
                .ok_or_else(|| {
                    Error::InvalidInput(format!("limit must be an integer from 1 to {MAX_LIMIT}"))
                })? as usize,
        };
        let matching: Vec<_> = awr_local::served(&self.dir, &self.allow_roots)?
            .into_iter()
            .filter(|entry| {
                query.as_ref().is_none_or(|query| {
                    entry.name.to_lowercase().contains(query)
                        || entry.root.to_string_lossy().to_lowercase().contains(query)
                })
            })
            .collect();
        let projects: Vec<Value> = matching
            .iter()
            .take(limit)
            .map(|entry| {
                json!({
                    "key": entry.key,
                    "name": entry.name,
                    "root": entry.root,
                    "project_id": entry.project_id,
                    "last_used_at": entry.last_used_at,
                })
            })
            .collect();
        Ok(json!({
            "ok": true,
            "read_only": true,
            "total": matching.len(),
            "returned": projects.len(),
            "truncated": matching.len() > projects.len(),
            "projects": projects,
            "default_project": self.default_root(),
            "selection": "pass `project` (a key, a unique name or a root path) on a call; omit it to use the default project. Register more with `awr overview register`.",
        }))
    }
}
