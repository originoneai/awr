use awr_core::{Error, Id, Result};
use awr_team::{AuthContext, authorize, execute, parse_envelope};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;

use crate::cli_failure::CliResult;
use crate::remote::{load_profile, remote_name, team_err};

mod client;
mod output;

#[derive(Debug, Subcommand)]
pub enum TeamCommand {
    /// Show local capabilities, or authenticated service capabilities with --remote.
    Capabilities {
        #[arg(long, value_parser = remote_name)]
        remote: Option<String>,
    },
    /// Discover workstreams visible to the authenticated caller.
    Workstream {
        #[command(subcommand)]
        command: WorkstreamCommand,
    },
    /// Navigate and read authoritative Team work. Queries do not grant execution authority.
    Work {
        #[command(subcommand)]
        command: WorkCommand,
    },
    /// Inspect a session and its historical checkpoint.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Inspect a coordination claim without renewing it.
    Claim {
        #[command(subcommand)]
        command: ClaimCommand,
    },
    /// Inspect a recorded execution without starting or retrying it.
    Execution {
        #[command(subcommand)]
        command: ExecutionCommand,
    },
    /// Inspect an original request, or validate a legacy envelope locally.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Command {
        #[command(subcommand)]
        read: Option<CommandRead>,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long, required = true)]
        body: Option<String>,
        #[arg(long)]
        offline: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkstreamCommand {
    List(NavigationArgs),
}

#[derive(Debug, Subcommand)]
pub enum WorkCommand {
    /// Suggest work and current-client sessions. Does not claim or reserve work.
    Next(NavigationArgs),
    /// Read one page of coordination items; an empty page may still have a cursor.
    Inbox {
        #[command(flatten)]
        navigation: NavigationArgs,
        #[command(flatten)]
        budget: BudgetArgs,
    },
    /// List contracts in one workstream; runtime state is not included.
    List(ListArgs),
    /// Case-insensitive literal substring search of titles in one workstream.
    Search {
        #[arg(value_name = "TEXT", value_parser = search_text)]
        text: String,
        #[command(flatten)]
        list: ListArgs,
    },
    /// Read required context. Exit 0 means read success; check context_complete separately.
    Prepare(DetailArgs),
    /// Read context and observations in one transaction; does not grant execution authority.
    Snapshot(DetailArgs),
    /// Read recorded progress; a running record does not prove a live process.
    Observe(DetailArgs),
    /// Read up to two current-ownership sessions/checkpoints; does not resume work.
    Recovery(RecoveryArgs),
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    Inspect {
        #[arg(value_name = "SESSION_ID", value_parser = selector)]
        session: String,
        #[command(flatten)]
        remote: RemoteArgs,
        /// Optional confirming task selector.
        #[arg(long, value_name = "WORK", value_parser = selector)]
        work: Option<String>,
        #[arg(long, value_name = "ID")]
        workstream: Option<Id>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ClaimCommand {
    Inspect {
        #[arg(value_name = "CLAIM_ID", value_parser = selector)]
        claim: String,
        #[command(flatten)]
        scope: RecordScopeArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum ExecutionCommand {
    Inspect {
        #[arg(value_name = "EXECUTION_ID", value_parser = selector)]
        execution: String,
        #[command(flatten)]
        scope: RecordScopeArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum CommandRead {
    /// Inspect the original request ID; an unknown result does not prove failure.
    Inspect {
        #[arg(value_name = "REQUEST_ID", value_parser = selector)]
        request: String,
        #[command(flatten)]
        scope: RecordScopeArgs,
    },
}

#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[arg(long, value_name = "NAME", value_parser = remote_name)]
    remote: String,
}

#[derive(Debug, Default, Args)]
pub struct PageArgs {
    /// Page size, 1..100. Omit to use the service default.
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=100))]
    limit: Option<u16>,
    /// Opaque service cursor; retain the same operation and filters when continuing.
    #[arg(long, value_parser = cursor_text)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
pub struct NavigationArgs {
    #[command(flatten)]
    remote: RemoteArgs,
    #[command(flatten)]
    page: PageArgs,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    #[command(flatten)]
    navigation: NavigationArgs,
    /// Workstream ULID, not its title or external key.
    #[arg(long, value_name = "ID")]
    workstream: Option<Id>,
}

#[derive(Debug, Args)]
#[group(id = "work_selection", required = true, multiple = true)]
pub struct WorkSelection {
    #[arg(value_name = "WORK", value_parser = selector)]
    work: Option<String>,
    /// Select by session; may accompany WORK if both identify the same work.
    #[arg(long, value_name = "ID", value_parser = selector)]
    session: Option<String>,
}

#[derive(Debug, Args)]
#[group(id = "record_work_selection", required = true, multiple = true)]
pub struct RecordWorkSelection {
    #[arg(long, value_name = "WORK", value_parser = selector)]
    work: Option<String>,
    /// Select by session; may accompany --work if both identify the same work.
    #[arg(long, value_name = "ID", value_parser = selector)]
    session: Option<String>,
}

#[derive(Debug, Args)]
pub struct RecordScopeArgs {
    #[command(flatten)]
    remote: RemoteArgs,
    #[command(flatten)]
    selection: RecordWorkSelection,
    /// Optional confirming workstream ULID; alone it does not select a task.
    #[arg(long, value_name = "ID")]
    workstream: Option<Id>,
}

#[derive(Debug, Args)]
pub struct RecoveryArgs {
    #[command(flatten)]
    remote: RemoteArgs,
    #[command(flatten)]
    selection: WorkSelection,
    #[arg(long, value_name = "ID")]
    workstream: Option<Id>,
}

#[derive(Debug, Default, Args)]
pub struct BudgetArgs {
    /// Context byte budget, 1..262144; not a token count. Service default: 65536.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=262144))]
    max_context_bytes: Option<u32>,
}

#[derive(Debug, Args)]
pub struct DetailArgs {
    #[command(flatten)]
    remote: RemoteArgs,
    #[command(flatten)]
    selection: WorkSelection,
    /// Optional confirming workstream ULID; alone it does not select a task.
    #[arg(long, value_name = "ID")]
    workstream: Option<Id>,
    #[command(flatten)]
    budget: BudgetArgs,
}

// Only the selected read operations need a CLI adapter. The service owns validation
// of identity, selection consistency, permission and current business facts.
#[derive(Serialize)]
struct QueryRequest {
    protocol_version: u32,
    op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    workstream_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    search: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_context_bytes: Option<u32>,
}

impl QueryRequest {
    fn new(op: &'static str) -> Self {
        Self {
            protocol_version: 1,
            op,
            workstream_id: None,
            work_id: None,
            session_id: None,
            claim_id: None,
            execution_id: None,
            request_id: None,
            search: None,
            limit: None,
            cursor: None,
            max_context_bytes: None,
        }
    }

    fn page(mut self, args: &PageArgs) -> Self {
        self.limit = args.limit;
        self.cursor = args.cursor.clone();
        self
    }

    fn list(self, args: &ListArgs) -> Self {
        let mut query = self.page(&args.navigation.page);
        query.workstream_id = args.workstream;
        query
    }

    fn scope(mut self, work: Option<&str>, session: Option<&str>, stream: Option<Id>) -> Self {
        self.work_id = work.map(str::to_owned);
        self.session_id = session.map(str::to_owned);
        self.workstream_id = stream;
        self
    }

    fn detail(self, args: &DetailArgs) -> Self {
        let mut query = self.scope(
            args.selection.work.as_deref(),
            args.selection.session.as_deref(),
            args.workstream,
        );
        query.max_context_bytes = args.budget.max_context_bytes;
        query
    }

    fn record(self, args: &RecordScopeArgs) -> Self {
        self.scope(
            args.selection.work.as_deref(),
            args.selection.session.as_deref(),
            args.workstream,
        )
    }
}

pub fn run(project: &Path, command: &TeamCommand, json: bool) -> CliResult<()> {
    let (remote, query) = match command {
        TeamCommand::Capabilities { remote: None } => {
            let auth = auth_from_profile(None)?;
            let value = execute(
                "cli",
                parse_envelope(&json!({
                    "protocol_version":1,"request_id":"capabilities","op":"capabilities"
                }))
                .map_err(team_err)?,
                &auth,
                None,
                true,
            )
            .map_err(team_err)?;
            print_value(&value, json);
            return Ok(());
        }
        TeamCommand::Capabilities { remote: Some(name) } => {
            (name.as_str(), QueryRequest::new("capabilities"))
        }
        TeamCommand::Workstream {
            command: WorkstreamCommand::List(args),
        } => (
            args.remote.remote.as_str(),
            QueryRequest::new("workstreams.list").page(&args.page),
        ),
        TeamCommand::Work { command } => match command {
            WorkCommand::Next(args) => (
                args.remote.remote.as_str(),
                QueryRequest::new("work.next").page(&args.page),
            ),
            WorkCommand::Inbox { navigation, budget } => {
                let mut query = QueryRequest::new("work.inbox").page(&navigation.page);
                query.max_context_bytes = budget.max_context_bytes;
                (navigation.remote.remote.as_str(), query)
            }
            WorkCommand::List(args) => (
                args.navigation.remote.remote.as_str(),
                QueryRequest::new("work.list").list(args),
            ),
            WorkCommand::Search { text, list } => {
                let mut query = QueryRequest::new("work.search").list(list);
                query.search = Some(text.clone());
                (list.navigation.remote.remote.as_str(), query)
            }
            WorkCommand::Prepare(args) => (
                args.remote.remote.as_str(),
                QueryRequest::new("work.prepare").detail(args),
            ),
            WorkCommand::Snapshot(args) => (
                args.remote.remote.as_str(),
                QueryRequest::new("work.snapshot").detail(args),
            ),
            WorkCommand::Observe(args) => (
                args.remote.remote.as_str(),
                QueryRequest::new("work.observe").detail(args),
            ),
            WorkCommand::Recovery(args) => (
                args.remote.remote.as_str(),
                QueryRequest::new("work.recovery").scope(
                    args.selection.work.as_deref(),
                    args.selection.session.as_deref(),
                    args.workstream,
                ),
            ),
        },
        TeamCommand::Session {
            command:
                SessionCommand::Inspect {
                    session,
                    remote,
                    work,
                    workstream,
                },
        } => (
            remote.remote.as_str(),
            QueryRequest::new("session.inspect").scope(work.as_deref(), Some(session), *workstream),
        ),
        TeamCommand::Claim {
            command: ClaimCommand::Inspect { claim, scope },
        } => {
            let mut query = QueryRequest::new("claim.inspect").record(scope);
            query.claim_id = Some(claim.clone());
            (scope.remote.remote.as_str(), query)
        }
        TeamCommand::Execution {
            command: ExecutionCommand::Inspect { execution, scope },
        } => {
            let mut query = QueryRequest::new("execution.inspect").record(scope);
            query.execution_id = Some(execution.clone());
            (scope.remote.remote.as_str(), query)
        }
        TeamCommand::Command {
            read: Some(CommandRead::Inspect { request, scope }),
            ..
        } => {
            let mut query = QueryRequest::new("command.inspect").record(scope);
            query.request_id = Some(request.clone());
            (scope.remote.remote.as_str(), query)
        }
        TeamCommand::Command {
            read: None,
            remote,
            body,
            offline,
        } => {
            let parsed: Value =
                serde_json::from_str(body.as_deref().expect("clap requires legacy body"))
                    .map_err(|e| Error::InvalidInput(e.to_string()))?;
            let profile = remote
                .as_deref()
                .map(|name| load_profile(project, name))
                .transpose()?;
            let auth = auth_from_profile(profile.as_ref())?;
            if profile.is_some() {
                authorize(&auth, &parsed).map_err(team_err)?;
            }
            let envelope = parse_envelope(&parsed).map_err(team_err)?;
            let value =
                execute("cli", envelope, &auth, profile.as_ref(), !offline).map_err(team_err)?;
            print_value(&value, json);
            return Ok(());
        }
    };
    let profile = load_profile(project, remote)?;
    let value = client::TeamClient::new(&profile)?.query(&query)?;
    output::validate(query.op, &value)?;
    output::print(query.op, remote, &value, json);
    Ok(())
}

fn bounded_text(value: &str, max: usize, label: &str) -> std::result::Result<String, String> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(format!(
            "{label} must be nonempty, at most {max} UTF-8 bytes, and contain no control characters"
        ))
    } else {
        Ok(value.to_owned())
    }
}
fn selector(value: &str) -> std::result::Result<String, String> {
    bounded_text(value, 128, "selector")
}
fn search_text(value: &str) -> std::result::Result<String, String> {
    bounded_text(value, 512, "search")
}
fn cursor_text(value: &str) -> std::result::Result<String, String> {
    if value.len() > 4096 {
        Err("cursor must be at most 4096 UTF-8 bytes".into())
    } else {
        Ok(value.to_owned())
    }
}
fn auth_from_profile(profile: Option<&awr_team::RemoteProfile>) -> Result<AuthContext> {
    Ok(AuthContext {
        tenant_id: "local".into(),
        project_id: profile
            .map(|p| p.project_key.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_default(),
        actor_id: "cli".into(),
        client_id: "awr-cli".into(),
    })
}

fn print_value(value: &Value, json: bool) {
    if json {
        println!("{value}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        );
    }
}
