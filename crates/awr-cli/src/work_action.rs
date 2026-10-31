use awr_core::*;
use awr_runtime::WorkActionRequest;
use awr_store::Store;
use clap::Args;
use std::path::Path;

#[derive(Debug, Args)]
pub struct ActionArgs {
    work: String,
    #[arg(long)]
    session: Id,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    next_action: Option<String>,
    #[arg(long)]
    summary: Option<String>,
    #[arg(long)]
    blocker: Option<String>,
    #[arg(long)]
    expected_revision: Revision,
    #[arg(long, default_value = "full", value_parser = ["full", "summary"])]
    response_view: String,
}
pub fn run(root: &Path, args: &ActionArgs, action: WorkAction, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let database = crate::source::runtime_dir(&root, false)?.join("state.db");
    let mut store = Store::open_existing(&database)?;
    let result = awr_runtime::perform_work_action(
        &mut store,
        &root,
        &WorkActionRequest {
            target: args.work.clone(),
            session_id: args.session,
            expected_revision: args.expected_revision,
            input: WorkActionInput {
                action,
                reason: args.reason.clone(),
                next_action: args.next_action.clone(),
                summary: args.summary.clone(),
                blocker: args.blocker.clone(),
            },
        },
    )?;
    print_result(result, action, json_output, &args.response_view)
}
fn print_result(
    result: awr_runtime::ProposalReport,
    action: WorkAction,
    json_output: bool,
    response_view: &str,
) -> Result<()> {
    if json_output || response_view == "summary" {
        let mut value = serde_json::to_value(&result)?;
        if response_view == "summary" {
            value = awr_runtime::summarize_work_response(value);
            if value.get("response_view").is_some() {
                value["response_view"]["full_result"] = serde_json::json!({"proposal_argv":["proposal","show",result.proposal.id,"--full"],"event_argv":["event","show",result.event.id,"--full"]});
            }
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "Work action: {action:?}\nProposal: {} ({:?})\nRevision: {}\nSource outcome: {}\nReceipt: {}",
            result.proposal.id,
            result.proposal.status,
            result.project_revision,
            result.write_outcome,
            result.event.id
        );
        if let Some(binding) = result.proposal.bound_patch()?.work_action {
            println!("Bound transition: {:?} → {:?}", binding.from, binding.to);
        }
        if let Some(close) = &result.close_out {
            println!(
                "Next: {} ({}; {})",
                close.next, close.condition, close.basis
            );
        }
        if let Some(end) = &result.session_end {
            println!("Session: {} ({})", end.status, end.session_id);
            if let Some(next) = end.next {
                println!("Next: {next}");
            }
        }
    }
    match result.failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[derive(Debug, Args)]
pub struct CompleteArgs {
    work: String,
    #[arg(long)]
    session: Id,
    #[arg(long)]
    reason: String,
    /// JSON acceptance-to-evidence mapping (version 1, at most 64 KiB).
    #[arg(long)]
    input: std::path::PathBuf,
    #[arg(long)]
    expected_revision: Revision,
    #[arg(long, default_value = "full", value_parser = ["full", "summary"])]
    response_view: String,
    /// End the session in the same request once the work is completed. Refused while the session
    /// still holds claims on other work.
    #[arg(long)]
    end_session: bool,
}
pub fn complete(root: &Path, args: &CompleteArgs, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let database = crate::source::runtime_dir(&root, false)?.join("state.db");
    let mut store = Store::open_existing(&database)?;
    let actual = store.project_by_root(&root)?.project_revision;
    if actual != args.expected_revision {
        return Err(Error::RevisionConflict {
            expected: args.expected_revision,
            actual,
        });
    }
    let bytes = awr_source::read_capped(&args.input, 64 * 1024)?;
    let input: CompletionInput = serde_json::from_slice(&bytes).map_err(|error| {
        Error::InvalidInput(format!(
            "completion input does not match its schema: {}",
            awr_core::redact_schema_error_detail(&error.to_string())
        ))
    })?;
    let result = awr_runtime::complete_work(
        &mut store,
        &root,
        &awr_runtime::CompleteWorkRequest {
            target: args.work.clone(),
            session_id: args.session,
            expected_revision: args.expected_revision,
            reason: args.reason.clone(),
            input,
            end_session: args.end_session,
        },
    )?;
    print_result(
        result,
        WorkAction::Complete,
        json_output,
        &args.response_view,
    )
}
