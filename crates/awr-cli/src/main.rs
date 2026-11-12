mod finished_work;
mod organization;
mod overview;
mod runtime_snapshot;
use awr_core::{Error, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
mod assessment;
mod batch;
mod branch;
mod capabilities;
mod catalog;
mod cli_failure;
mod client;
mod client_dialect;
mod client_install;
mod compaction;
mod context;
mod doctor;
mod document;
mod drill;
mod event_append;
mod execution;
mod help;
mod host_save;
mod intake_plan;
mod management;
mod mutation;
mod nav;
mod onboarding;
mod query;
mod records;
mod recovery;
mod remote;
mod resume;
mod search;
mod session;
mod source;
mod source_changes;
mod source_relocation;
mod specification;
mod takeover;
mod team;
mod verify;
mod work_action;
mod work_activate;
mod work_check;
mod work_create;
mod work_edit;
mod work_prepare;
mod workspace;

#[derive(Debug, Parser)]
#[command(
    name = "awr",
    version = env!("AWR_BUILD_VERSION"),
    about = "Persistent work state and minimal context for long-running agents"
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the local project hub, or inspect its catalog and background service.
    Overview(overview::OverviewArgs),
    /// Inspect, back up and explicitly restore a matching local runtime.
    Runtime {
        #[command(subcommand)]
        command: runtime_snapshot::RuntimeCommand,
    },
    /// Maintain explicitly mapped project phase, scope and focus.
    Organization {
        #[command(subcommand)]
        command: organization::OrganizationCommand,
    },
    /// Preview and apply a bounded batch of source edits.
    Batch {
        #[command(subcommand)]
        command: batch::BatchCommand,
    },
    /// Save explicit host edits with durable request identity and provenance.
    Host {
        #[command(subcommand)]
        command: host_save::HostCommand,
    },
    /// Edit registered Markdown documents or create source-backed drafts.
    Document {
        #[command(subcommand)]
        command: document::DocumentCommand,
    },
    /// Negotiate host capabilities without opening a project or its database.
    Capabilities(capabilities::CapabilitiesArgs),
    /// Preview source authority mapping; --accept initializes using the reviewed mapping.
    Init(onboarding::InitArgs),
    /// Take over an existing project in resumable steps.
    Takeover(takeover::TakeoverArgs),
    /// Diagnose project organization and recheck readiness after source edits.
    Intake {
        #[command(subcommand)]
        command: onboarding::IntakeCommand,
    },
    /// Bind client conversations and persist lifecycle checkpoints.
    Client {
        #[command(subcommand)]
        command: client::ClientCommand,
    },
    /// Register, run and inspect executions that can outlive the calling session.
    Execution {
        #[command(subcommand)]
        command: execution::ExecutionCommand,
    },
    /// Inspect saved work and verify execution evidence before resuming.
    Recovery {
        #[command(subcommand)]
        command: recovery::RecoveryCommand,
    },
    /// List, scan or index authoritative project sources.
    Source {
        #[command(subcommand)]
        command: source::SourceCommand,
    },
    /// Read-only mainline navigation across scope, dependencies, accounting and blockers.
    Nav(nav::NavArgs),
    /// Refresh source projections and summarize current project work.
    Status {
        /// Return one page of current queue items (does not include terminal history).
        #[arg(long, value_parser=["all","current","ready","waiting","blocked"])]
        queue: Option<String>,
        #[arg(long, default_value_t = 0, requires = "queue")]
        offset: usize,
        #[arg(long, default_value_t = 20, value_parser=clap::value_parser!(u16).range(1..=100), requires = "queue")]
        page_size: u16,
        /// Action is the daily queue; full/summary retain the earlier diagnostic views.
        #[arg(long, default_value="action", value_parser=["action","full","summary"])]
        view: String,
        /// Exact work keys in the action/summary scope; repeat to select several.
        #[arg(long)]
        work: Vec<String>,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long)]
        milestone: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        /// Use the last recorded snapshot without refreshing business sources.
        #[arg(long)]
        cached: bool,
        /// Verify completion reports against this explicit full source SHA.
        #[arg(long)]
        source_sha: Option<String>,
    },
    /// List dependency-ready work with explicit reasons for excluded work.
    Ready {
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        branch: Option<String>,
        /// Use the last recorded snapshot without refreshing business sources.
        #[arg(long)]
        cached: bool,
    },
    /// Offline assessment replay, shadow compare, and advice kill-switch.
    Assessment {
        #[command(subcommand)]
        command: assessment::AssessmentCommand,
    },
    /// Read one work item without expanding the full ledger or event history.
    Work {
        #[command(subcommand)]
        command: query::WorkCommand,
    },
    /// Start, inspect, checkpoint or end an explicitly identified agent session.
    Session {
        #[command(subcommand)]
        command: session::SessionCommand,
    },
    /// Add evidence records or inspect a specific record and its report reference.
    Evidence {
        #[command(subcommand)]
        command: records::EvidenceCommand,
    },
    /// Verify completed work for a commit using local observations or fresh checks.
    Verify(verify::VerifyArgs),
    /// Inspect a specific authoritative decision.
    Decision {
        #[command(subcommand)]
        command: records::DecisionCommand,
    },
    /// Inspect explicit specification versions, supersessions and requirement coverage.
    Spec {
        #[command(subcommand)]
        command: specification::SpecificationCommand,
    },
    /// Import, inspect or explicitly read a registered artifact.
    Artifact {
        #[command(subcommand)]
        command: records::ArtifactCommand,
    },
    /// Compile revision-bound context after refreshing project sources.
    Context {
        #[command(subcommand)]
        command: context::ContextCommand,
    },
    /// Read one referenced goal, plan, rule, work item or checkpoint.
    Object {
        #[command(subcommand)]
        command: drill::ObjectCommand,
    },
    /// Append validated events or inspect immutable events and bounded history.
    Event {
        #[command(subcommand)]
        command: drill::EventCommand,
    },
    /// Search bounded summaries, optionally filtering by entity type, status or work item.
    Search(search::SearchArgs),
    /// Create, inspect and review source-bound mutation proposals.
    Proposal {
        #[command(subcommand)]
        command: mutation::ProposalCommand,
    },
    /// Create, inspect or select an Agent Work Branch without changing Git checkout.
    Branch {
        #[command(subcommand)]
        command: branch::BranchCommand,
    },
    /// Diagnose project/database state; apply only explicitly selected runtime repairs.
    Doctor(doctor::DoctorArgs),
    /// Share tracked project files between machines through an object store.
    Workspace(workspace::WorkspaceArgs),
    /// Inspect Team protocol capabilities or submit a remote Team envelope.
    Team {
        #[command(subcommand)]
        command: team::TeamCommand,
    },
    /// Store Team remote endpoint and credential environment references.
    Remote {
        #[command(subcommand)]
        command: remote::RemoteCommand,
    },
    #[command(external_subcommand)]
    Unsupported(Vec<String>),
}

fn run(cli: &Cli) -> cli_failure::CliResult<()> {
    let result = match &cli.command {
        Some(Command::Verify(args)) => verify::run(&cli.project, args, cli.json),
        Some(Command::Overview(args)) => overview::run(&cli.project, args, cli.json),
        Some(Command::Host { command }) => host_save::run(&cli.project, command, cli.json),
        Some(Command::Runtime { command }) => {
            runtime_snapshot::run(&cli.project, command, cli.json)
        }
        Some(Command::Organization { command }) => {
            organization::run(&cli.project, command, cli.json)
        }
        Some(Command::Batch { command }) => batch::run(&cli.project, command, cli.json),
        Some(Command::Document { command }) => document::run(&cli.project, command, cli.json),
        Some(Command::Capabilities(args)) => capabilities::run(args, cli.json),
        Some(Command::Init(args)) => onboarding::run(&cli.project, args, cli.json),
        Some(Command::Takeover(args)) => takeover::run(&cli.project, args, cli.json),
        Some(Command::Intake { command }) => onboarding::inspect(&cli.project, command, cli.json),
        Some(Command::Client { command }) => client::run(&cli.project, command, cli.json),
        Some(Command::Execution { command }) => execution::run(&cli.project, command, cli.json),
        Some(Command::Recovery { command }) => recovery::run(&cli.project, command, cli.json),
        Some(Command::Source { command }) => source::run(&cli.project, command, cli.json),
        Some(Command::Nav(args)) => nav::run(&cli.project, &args, cli.json),
        Some(Command::Status {
            queue,
            offset,
            page_size,
            branch,
            source_sha,
            cached,
            view,
            work,
            goal,
            milestone,
        }) => {
            let scope = awr_runtime::StatusScope {
                work: work.clone(),
                goal: goal.clone(),
                milestone: milestone.clone(),
            };
            if queue.is_some() && view != "action" {
                return Err(
                    awr_core::Error::InvalidInput("--queue requires --view action".into()).into(),
                );
            }
            if view == "full" && !scope.is_empty() {
                return Err(awr_core::Error::InvalidInput(
                    "scope selectors require --view action or summary".into(),
                )
                .into());
            }
            query::status_with_scope(
                &cli.project,
                branch.as_deref(),
                source_sha.as_deref(),
                cli.json,
                *cached,
                (view != "full").then_some(&scope),
                view == "action",
                queue
                    .as_deref()
                    .map(|q| (q, *offset, usize::from(*page_size))),
            )
        }
        Some(Command::Ready {
            limit,
            branch,
            cached,
        }) => query::ready(&cli.project, *limit, branch.as_deref(), cli.json, *cached),
        Some(Command::Assessment { command }) => assessment::run(&cli.project, command, cli.json),
        Some(Command::Work { command }) => query::work(&cli.project, command, cli.json),
        Some(Command::Session { command }) => session::run(&cli.project, command, cli.json),
        Some(Command::Evidence { command }) => records::evidence(&cli.project, command, cli.json),
        Some(Command::Decision { command }) => records::decision(&cli.project, command, cli.json),
        Some(Command::Spec { command }) => specification::run(&cli.project, command, cli.json),
        Some(Command::Artifact { command }) => records::artifact(&cli.project, command, cli.json),
        Some(Command::Context { command }) => context::run(&cli.project, command, cli.json),
        Some(Command::Object { command }) => drill::object(&cli.project, command, cli.json),
        Some(Command::Event { command }) => drill::event(&cli.project, command, cli.json),
        Some(Command::Search(args)) => search::run(&cli.project, args, cli.json),
        Some(Command::Proposal { command }) => mutation::run(&cli.project, command, cli.json),
        Some(Command::Branch { command }) => branch::run(&cli.project, command, cli.json),
        Some(Command::Doctor(args)) => doctor::run(&cli.project, args, cli.json),
        Some(Command::Workspace(args)) => workspace::run(&cli.project, args, cli.json),
        Some(Command::Team { command }) => {
            require_team(&cli.project)?;
            return team::run(&cli.project, command, cli.json);
        }
        Some(Command::Remote { command }) => {
            require_team(&cli.project)?;
            remote::run(&cli.project, command, cli.json)
        }
        None => {
            help::command().print_help().map_err(Error::from)?;
            Ok(())
        }
        Some(Command::Unsupported(args)) => Err(Error::Unsupported(args.join(" "))),
    };
    result.map_err(Into::into)
}

fn main() -> std::process::ExitCode {
    let args = std::env::args_os().collect::<Vec<_>>();
    for (index, arg) in args.iter().enumerate() {
        if let Err(error) = awr_core::ensure_public_text(&arg.to_string_lossy()) {
            let mut report =
                Error::RuleViolation("sensitive command arguments are not accepted".into())
                    .report();
            let mut details = error
                .report()
                .details
                .unwrap_or_else(|| serde_json::json!({}));
            details["argument_index"] = serde_json::json!(index);
            details["argument_index_basis"] = serde_json::json!("zero_based_process_argv");
            if details["category"] == "environment_dump" {
                details["next_action"] = serde_json::json!(
                    "Set required variables in the process environment before invoking awr; its supervisor and managed child inherit that environment. Standalone environment assignments are not accepted as recorded command arguments. Keep environment dumps outside AWR-managed data."
                );
            }
            report.details = Some(details);
            eprintln!(
                "{}",
                serde_json::to_string(&report).expect("error report serializes")
            );
            return std::process::ExitCode::from(1);
        }
    }
    let cli = match help::parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => {
            if !error.use_stderr() {
                awr_local::maintain(None);
            }
            // A literal after `--` is data, and `--field=--json` is not the JSON flag.
            let json = args
                .iter()
                .skip(1)
                .take_while(|arg| *arg != "--")
                .any(|arg| arg == "--json");
            if error.use_stderr() && json {
                let report = Error::InvalidInput(error.to_string().trim().into()).report();
                eprintln!(
                    "{}",
                    serde_json::to_string(&report).expect("error report serializes")
                );
            } else {
                let _ = error.print();
            }
            return std::process::ExitCode::from(error.exit_code() as u8);
        }
    };
    let stateless_verify = matches!(&cli.command, Some(Command::Verify(args)) if args.stateless());
    if !matches!(cli.command, Some(Command::Overview(_))) && !stateless_verify {
        let project = if matches!(cli.command, Some(Command::Capabilities(_))) {
            None
        } else {
            Some(cli.project.as_path())
        };
        awr_local::maintain(project);
    }
    let result = run(&cli);
    if result.is_ok() && matches!(cli.command, Some(Command::Init(_))) {
        awr_local::maintain(Some(&cli.project));
    }
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            error.print(cli.json);
            std::process::ExitCode::from(1)
        }
    }
}

/// Team code is always built; the runtime switch decides whether it may run.
fn require_team(project: &std::path::Path) -> Result<()> {
    if awr_source::team_enabled(project)? {
        Ok(())
    } else {
        Err(Error::Unsupported(awr_source::TEAM_DISABLED_MESSAGE.into()))
    }
}
