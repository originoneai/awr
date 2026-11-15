use awr_core::{Error, Result};
use awr_runtime::{AttestedUsageProject, UsageRuntimeError};
use awr_store::Store;
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageListKind {
    Receipts,
    Corrections,
    Allocations,
    Counters,
    Intervals,
    Bindings,
}

#[derive(Debug, Subcommand)]
pub enum UsageCommand {
    /// Record usage observations and adjustments.
    Record {
        /// JSON with expected_revision, request_key and record {kind,data}.
        #[arg(long)]
        input: PathBuf,
    },
    /// Read one bounded page of observations and adjustments.
    List {
        #[arg(long, value_enum, default_value = "receipts")]
        kind: UsageListKind,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
    },
    /// Read observed costs, time and coverage.
    Summary {
        /// Inspect the original cost basis before append-only corrections.
        #[arg(long)]
        uncorrected: bool,
    },
    /// Run a query for raw records or observation coverage.
    Query {
        #[arg(long)]
        input: PathBuf,
    },
}

fn input(root: &Path, path: &Path) -> Result<Value> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let value = serde_json::from_slice(&awr_source::read_capped(&path, 1024 * 1024)?)
        .map_err(|_| Error::InvalidInput("usage input must be valid JSON".into()))?;
    awr_core::ensure_public_value(&value)?;
    Ok(value)
}

fn binding(store: &Store, root: &Path) -> Result<AttestedUsageProject> {
    let project = store.project_by_root(root)?;
    Ok(AttestedUsageProject {
        project_id: project.id.to_string(),
        project: project.id,
        attested: true,
    })
}

fn error(error: UsageRuntimeError) -> Error {
    match error {
        UsageRuntimeError::Store(error) => error,
        error => Error::InvalidInput(error.to_string()),
    }
}

pub fn run(root: &Path, command: &UsageCommand, json_output: bool) -> Result<()> {
    let database = crate::source::runtime_dir(root, false)?.join("state.db");
    let value = match command {
        UsageCommand::Record { input: path } => {
            let request = input(root, path)?;
            let mut store = Store::open_existing(&database)?;
            let project = binding(&store, root)?;
            project.record(&mut store, request).map_err(error)?
        }
        command => {
            let request = match command {
                UsageCommand::List {
                    kind,
                    offset,
                    limit,
                } => json!({"kind":kind,"offset":offset,"limit":limit}),
                UsageCommand::Summary { uncorrected } => {
                    json!({"kind":"summary","apply_corrections":!uncorrected})
                }
                UsageCommand::Query { input: path } => input(root, path)?,
                UsageCommand::Record { .. } => unreachable!(),
            };
            let store = Store::read_snapshot(&database, 256 * 1024 * 1024)?;
            binding(&store, root)?
                .query(&store, request)
                .map_err(error)?
        }
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if let Some(receipt) = value.get("receipt") {
        println!(
            "{}",
            if receipt["replayed"] == true {
                "Usage observation already recorded"
            } else {
                "Usage observation recorded"
            }
        );
    } else if let Some(items) = value["data"]["items"].as_array() {
        println!(
            "Usage ledger — {} of {} records",
            items.len(),
            value["data"]["total"]
        );
        println!("Use --json to read the records and the next page.");
    } else {
        println!("{}", serde_json::to_string_pretty(&value["data"])?);
    }
    Ok(())
}
