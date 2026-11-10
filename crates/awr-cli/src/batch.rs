use awr_core::*;
use awr_runtime::{BatchReport, BatchRequest};
use awr_store::Store;
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Debug, Args)]
pub struct ChangeArgs {
    /// Versioned batch edit or new draft, including a stable request key.
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    accept: bool,
    #[arg(long)]
    expected_preview: Option<String>,
    #[arg(long)]
    expected_revision: Option<Revision>,
}
#[derive(Debug, Subcommand)]
pub enum BatchCommand {
    /// Preview a source-bound change; accept exactly the returned preview to write it.
    Change(ChangeArgs),
    /// Read a retained request outcome without indexing or writing.
    Status {
        #[arg(long, visible_alias = "request-key")]
        key: String,
    },
    /// Explicitly recover one interrupted batch change at the current revision.
    Recover {
        #[arg(long, visible_alias = "request-key")]
        key: String,
        #[arg(long)]
        expected_revision: Revision,
    },
}
fn output(report: BatchReport, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report.value)?)
    } else {
        println!("Batch change: {}", report.value["status"])
    }
    match report.failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
pub fn run(root: &Path, command: &BatchCommand, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let db = crate::source::runtime_dir(&root, false)?.join("state.db");
    match command {
        BatchCommand::Status { key } => output(
            awr_runtime::batch_status(&Store::open_readonly(&db)?, &root, key)?,
            json_output,
        ),
        BatchCommand::Recover {
            key,
            expected_revision,
        } => output(
            awr_runtime::recover_batch(
                &mut Store::open_existing(&db)?,
                &root,
                key,
                *expected_revision,
            )?,
            json_output,
        ),
        BatchCommand::Change(args) => {
            let path = if args.input.is_absolute() {
                args.input.clone()
            } else {
                root.join(&args.input)
            };
            let input: BatchRequest = crate::host_save::parse_request(
                &awr_source::read_capped(&path, 64 * 1024 * 1024)?,
                "batch",
                serde_json::json!({"version":1,"request_key":"change-1","actor":{"host":"awr-cli","subject":"caller","origin":"delegated_agent"},"reason":"Describe the intended change","change":{"kind":"ledger","source_id":"00000000000000000000000000","source_fingerprint":"sha256:...","operations":[]}}),
            )?;
            output(
                awr_runtime::change_batch(
                    &mut Store::open_existing(&db)?,
                    &root,
                    input,
                    args.accept,
                    args.expected_preview.as_deref(),
                    args.expected_revision,
                )?,
                json_output,
            )
        }
    }
}
