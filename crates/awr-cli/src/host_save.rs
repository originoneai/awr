use awr_core::*;
use awr_runtime::{HostSaveReport, HostSaveRequest};
use awr_store::Store;
use clap::Subcommand;
use std::path::{Path, PathBuf};
#[derive(Debug, Subcommand)]
pub enum HostCommand {
    /// Preview exactly the edit and provenance that the host will apply.
    Preview {
        #[arg(long)]
        input: PathBuf,
    },
    /// One human Save, or one AI application bound to a previously reviewed preview.
    Save {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        expected_revision: Revision,
        #[arg(long)]
        expected_preview: Option<String>,
    },
    /// Read the saved request and its underlying proposal/document outcome.
    Status {
        #[arg(long)]
        key: String,
    },
    /// Explicitly resume a recorded host action without repeating a completed write.
    Recover {
        #[arg(long)]
        key: String,
        #[arg(long)]
        expected_revision: Revision,
    },
}
fn input(root: &Path, path: &Path) -> Result<HostSaveRequest> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    parse_request(
        &awr_source::read_capped(&path, awr_source::MARKDOWN_READ_CAP)?,
        "host",
        serde_json::json!({"version":1,"request_key":"edit-1","actor":{"host":"awr-cli","subject":"caller","origin":"delegated_agent"},"reason":"Describe the intended change","change":{"operation":"activate_draft","work":"WORK","source_fingerprint":"sha256:..."}}),
    )
}
fn output(report: HostSaveReport, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report.value)?)
    } else {
        println!("Host save: {}", report.value["status"])
    }
    match report.failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
pub fn run(root: &Path, command: &HostCommand, json_output: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let db = crate::source::runtime_dir(&root, false)?.join("state.db");
    match command {
        HostCommand::Status { key } => output(
            awr_runtime::host_status(&Store::open_readonly(&db)?, &root, key)?,
            json_output,
        ),
        HostCommand::Preview { input: path } => output(
            awr_runtime::host_preview(&mut Store::open_existing(&db)?, &root, input(&root, path)?)?,
            json_output,
        ),
        HostCommand::Save {
            input: path,
            expected_revision,
            expected_preview,
        } => output(
            awr_runtime::host_save(
                &mut Store::open_existing(&db)?,
                &root,
                input(&root, path)?,
                *expected_revision,
                expected_preview.as_deref(),
            )?,
            json_output,
        ),
        HostCommand::Recover {
            key,
            expected_revision,
        } => output(
            awr_runtime::host_recover(
                &mut Store::open_existing(&db)?,
                &root,
                key,
                *expected_revision,
            )?,
            json_output,
        ),
    }
}

/// Decode the outer object first so shape diagnostics identify absent top-level keys.
pub(crate) fn parse_request<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    surface: &str,
    expected: serde_json::Value,
) -> Result<T> {
    let parsed = serde_json::from_slice::<serde_json::Value>(bytes);
    let missing: Vec<_> = ["version", "request_key", "actor", "reason", "change"]
        .into_iter()
        .filter(|key| {
            parsed
                .as_ref()
                .ok()
                .and_then(|v| v.as_object())
                .is_none_or(|v| !v.contains_key(*key))
        })
        .collect();
    let error = || {
        Error::InvalidInput(format!(
            "{surface} JSON requires version, request_key, actor, reason and one supported change"
        ))
        .with_details(
            serde_json::json!({"missing":missing,"expected":expected.to_string(),
            "next_action":"Edit the input JSON to match expected, then repeat the same command."}),
        )
    };
    let value = parsed.map_err(|_| error())?;
    serde_json::from_value(value).map_err(|_| error())
}
