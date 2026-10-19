//! MCP tools over project-isolated AWR domain services. No shell commands are executed.
pub use awr_core::{Error, Result};
mod arguments;
mod changes;
mod compaction;
pub mod domains;
pub mod hub;
mod input_diagnostics;
mod lifecycle;
mod operations;
mod project;
mod requests;
mod schema;
mod team_handoff;
mod team_review;
mod waiting;
mod workstreams;

use hub::{Hub, Principal, ProjectService};
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
pub use schema::{TOOL_NAMES, tools};
use std::{path::Path, sync::Arc};

#[derive(Clone)]
pub struct AwrServer {
    project: Option<Arc<ProjectService>>,
    hub: Option<Arc<Hub>>,
    capacity: Arc<tokio::sync::Semaphore>,
    /// Runtime Team switch, resolved once when the server binds its projects.
    team: bool,
}
/// MCP tools offered only while the Team switch is on.
const TEAM_TOOLS: &[&str] = &["awr_team_handoff", "awr_team_review"];
fn without_team_tools(mut catalog: Vec<Tool>, team: bool) -> Vec<Tool> {
    if !team {
        catalog.retain(|tool| !TEAM_TOOLS.contains(&tool.name.as_ref()));
    }
    catalog
}
impl AwrServer {
    /// Bind one project directory. Each operation validates its database/source state;
    /// status can explain how to initialize or repair a project before a database exists.
    pub fn open(root: &Path) -> Result<Self> {
        Ok(Self {
            project: Some(Arc::new(ProjectService::open(root, None)?)),
            hub: None,
            capacity: Arc::new(tokio::sync::Semaphore::new(64)),
            // An explicit override must parse; a broken manifest must not stop
            // the server, because status is how users learn to repair it.
            team: match awr_source::team_enabled_override()? {
                Some(enabled) => enabled,
                None => awr_source::team_enabled(root).unwrap_or(false),
            },
        })
    }
    pub(crate) fn shared(hub: Arc<Hub>) -> Self {
        Self {
            project: None,
            capacity: hub.capacity.clone(),
            team: hub.team,
            hub: Some(hub),
        }
    }
    fn catalog(&self) -> Vec<Tool> {
        without_team_tools(self.full_catalog(), self.team)
    }
    fn full_catalog(&self) -> Vec<Tool> {
        let shared = self.hub.is_some();
        if domains::ExposureMode::from_env() == domains::ExposureMode::Hierarchical {
            let mut catalog = domains::domain_tools(shared);
            // Project discovery stays visible on shared endpoints; it is the
            // only entry point that resolves the required `project` key.
            if shared {
                if let Some(tool) = schema::shared_tools()
                    .into_iter()
                    .find(|tool| tool.name == "awr_projects_list")
                {
                    catalog.insert(0, tool);
                }
                catalog.push(crate::workstreams::tool());
                catalog.push(crate::team_handoff::tool());
                catalog.push(crate::team_review::tool());
            }
            catalog
        } else if shared {
            schema::shared_tools()
        } else {
            tools()
        }
    }
    /// Flat names remain callable in hierarchical mode: the advertised catalog
    /// shrinks, while already-integrated hosts keep working.
    fn callable(&self, name: &str) -> bool {
        self.catalog().iter().any(|t| t.name == name)
            || (domains::ExposureMode::from_env() == domains::ExposureMode::Hierarchical
                && (crate::schema::TOOL_NAMES.contains(&name) || name == "awr_projects_list"))
    }
}

impl ServerHandler for AwrServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("awr-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions("AWR is source-first. On shared HTTP, discover project keys with awr_projects_list and pass project on every project call. For isolated workstreams, use awr_workstream capabilities/list and bind work/session; unsupported operations are rejected. For legacy projects, start with awr_project_status and organization.actions; empty ledgers/intake drafts do not grant readiness. Use awr_work_prepare with response_view=action; consume required context and follow the condition, basis, next action and recheck trigger. Reads persist nothing; on SourceStale explicitly reindex and inspect again. Bind a stable conversation/session and claim before source transitions; HTTP connections do not select or end work sessions. Mutations need reviewed expected_revision; completion also needs bound evidence. Command/report metadata is never executed. Inspect interrupted-write receipts before retrying. Report compaction from host telemetry, keep it enabled, and obtain user approval before changing native windows.")
    }
    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        if request.is_some_and(|r| r.cursor.is_some()) {
            return Err(ErrorData::invalid_params(
                "this tool catalog has no next cursor",
                None,
            ));
        }
        let mut result = ListToolsResult::default();
        result.tools = self.catalog();
        Ok(result)
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.catalog().into_iter().find(|t| t.name == name)
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let mut name: String = request.name.to_string();
        if !self.team && TEAM_TOOLS.contains(&name.as_str()) {
            return Err(ErrorData::invalid_request(
                awr_source::TEAM_DISABLED_MESSAGE,
                None,
            ));
        }
        if !self.callable(&name) && !domains::is_domain(&name) {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                "unknown AWR tool",
                None,
            ));
        }
        let mut args = request.arguments.unwrap_or_default();
        if domains::is_domain(&name) {
            let shared = self.hub.is_some();
            let project_key = if shared {
                args.remove("project")
                    .and_then(|v| v.as_str().map(str::to_owned))
            } else {
                None
            };
            let child_tool = args
                .remove("child_tool")
                .and_then(|v| v.as_str().map(str::to_owned));
            let child_args = args
                .remove("arguments")
                .filter(|v| v.is_object())
                .and_then(|v| v.as_object().cloned());
            match (child_tool, child_args) {
                (None, _) if args.is_empty() => {
                    let flat = without_team_tools(
                        if shared {
                            schema::shared_tools()
                        } else {
                            tools()
                        },
                        self.team,
                    );
                    if let (Some(hub), Some(key)) = (&self.hub, &project_key) {
                        let principal = context
                            .extensions
                            .get::<axum::http::request::Parts>()
                            .and_then(|parts| parts.extensions.get::<Principal>())
                            .cloned();
                        let principal = principal.ok_or_else(|| {
                            ErrorData::invalid_params("authenticated client required", None)
                        })?;
                        hub.select(&principal, key, false).map_err(|error| {
                            ErrorData::internal_error(
                                serde_json::to_string(&error.report()).unwrap(),
                                None,
                            )
                        })?;
                    }
                    let value = domains::manifest(&name, &flat, shared);
                    return Ok(CallToolResult::structured(value).into());
                }
                (Some(child), Some(map)) => {
                    if !domains::DOMAINS
                        .iter()
                        .any(|d| d.name == name && d.children.contains(&child.as_str()))
                    {
                        return Err(ErrorData::invalid_params(
                            "child_tool is not a member of this domain",
                            None,
                        ));
                    }
                    name = child;
                    args = serde_json::Map::from_iter(map);
                    if let Some(key) = project_key {
                        args.insert("project".into(), serde_json::json!(key));
                    }
                }
                _ => {
                    return Err(ErrorData::invalid_params(
                        "provide both child_tool and arguments, or neither for discovery",
                        None,
                    ));
                }
            }
        }
        let principal = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Principal>())
            .cloned();
        let selected = if let Some(hub) = &self.hub {
            let principal = principal
                .as_ref()
                .ok_or_else(|| ErrorData::invalid_params("authenticated client required", None))?;
            if name == "awr_projects_list" {
                if !args.is_empty() {
                    return Err(ErrorData::invalid_params(
                        "project catalog takes no arguments",
                        None,
                    ));
                }
                return Ok(CallToolResult::structured(hub.catalog(&principal)).into());
            }
            let selected = args
                .remove("project")
                .and_then(|v| v.as_str().map(str::to_owned));
            match selected {
                Some(key) => hub.select(&principal, &key, !operations::is_read_only(&name)),
                None => Err(Error::InvalidInput(
                    "shared MCP calls require an explicit project key".into(),
                )),
            }
        } else {
            Ok(self.project.as_ref().expect("bound project").clone())
        };
        let project = match selected {
            Ok(project) => project,
            Err(error) => {
                return Ok(CallToolResult::structured_error(
                    serde_json::to_value(error.report()).unwrap(),
                )
                .into());
            }
        };
        let permit = self.capacity.clone().try_acquire_owned().map_err(|_| {
            ErrorData::internal_error(
                "MCP service busy; retry after inspecting any prior write receipt",
                None,
            )
        })?;
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            project.call(&name, args, principal.as_ref())
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("MCP domain worker failed: {e}"), None))?;
        Ok(match result {
            Ok(result) => result,
            Err(error) => CallToolResult::structured_error(
                serde_json::to_value(error.report()).expect("error report serializes"),
            ),
        }
        .into())
    }
}
