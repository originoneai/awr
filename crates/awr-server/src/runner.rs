use awr_team_pg::{
    PgError, ReferenceReportRequest, ReferenceRunRequest, ReferenceWritePlan,
    ScopedReferenceRunner, WorkstreamCommandStore,
};
use clap::Subcommand;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io::Read;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum RunnerCommand {
    /// Validate and hash a bounded file-write plan, without database access.
    Digest {
        #[arg(long)]
        input: PathBuf,
    },
    /// Consume one new scoped admission, write files, then attest the result.
    Run {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        credential_file: PathBuf,
        #[arg(long)]
        root: PathBuf,
    },
    /// Retry only a saved result report; never execute the file-write plan.
    Report {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        credential_file: PathBuf,
        #[arg(long)]
        root: PathBuf,
    },
}
type Error = (&'static str, &'static str);
fn read<T: DeserializeOwned>(path: &PathBuf) -> Result<T, Error> {
    let bytes = bounded(path, 2 * 1024 * 1024)?;
    serde_json::from_slice(&bytes).map_err(|_| ("InvalidInput", "invalid runner JSON"))
}
fn bounded(path: &PathBuf, limit: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![];
    std::fs::File::open(path)
        .map_err(|_| ("InvalidInput", "cannot open runner input"))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ("InvalidInput", "cannot read runner input"))?;
    if bytes.len() > limit {
        return Err(("InvalidInput", "runner input exceeds its bound"));
    }
    Ok(bytes)
}
fn pg_error(error: PgError) -> Error {
    match error {
        PgError::Forbidden => ("Forbidden", "runner identity or scope denied"),
        PgError::Protocol(_) => (
            "InvalidInput",
            "invalid runner request, plan or saved journal",
        ),
        PgError::PreconditionsChanged => (
            "PreconditionsChanged",
            "runner preconditions changed; inspect before retrying",
        ),
        PgError::IdempotencyConflict => (
            "IdempotencyConflict",
            "request ID is bound to a different intent",
        ),
        PgError::RecoveryBlocked => ("RecoveryBlocked", "unresolved effects require recovery"),
        PgError::Unsupported(_) => ("Unsupported", "runner mode is unavailable"),
        _ => (
            "Unavailable",
            "admission or report not confirmed; inspect the original request before retrying; never assume no effects",
        ),
    }
}
pub async fn run(command: RunnerCommand) -> Result<Value, Error> {
    if let RunnerCommand::Digest { input } = &command {
        let plan: ReferenceWritePlan = read(input)?;
        return Ok(
            json!({"input_digest":plan.digest().map_err(pg_error)?,"execution_mode":"reference_write_v1","writes":plan.writes.len(),"executed":false}),
        );
    }
    let (input, credential, root) = match &command {
        RunnerCommand::Run {
            input,
            credential_file,
            root,
        }
        | RunnerCommand::Report {
            input,
            credential_file,
            root,
        } => (input, credential_file, root),
        _ => unreachable!(),
    };
    let bytes = bounded(credential, 512)?;
    let bearer = std::str::from_utf8(&bytes)
        .map_err(|_| ("InvalidInput", "invalid credential file"))?
        .trim();
    awr_team_pg::workstream_credential_hash(bearer).map_err(pg_error)?;
    let url = std::env::var("AWR_TEAM_DATABASE_URL").map_err(|_| {
        (
            "InvalidInput",
            "AWR_TEAM_DATABASE_URL is required; use the service application role",
        )
    })?;
    let runner = ScopedReferenceRunner::new(WorkstreamCommandStore::new(url), root);
    match &command {
        RunnerCommand::Run { .. } => {
            runner
                .run(bearer, read::<ReferenceRunRequest>(input)?)
                .await
        }
        RunnerCommand::Report { .. } => {
            runner
                .report(bearer, read::<ReferenceReportRequest>(input)?)
                .await
        }
        _ => unreachable!(),
    }
    .map_err(pg_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_command_is_invalid_input_without_exposing_protocol_details() {
        let command = pg_error(PgError::invalid_command_fields());
        assert_eq!(command.0, "InvalidInput");
        let internal = pg_error(PgError::Protocol("private-runner-sentinel".into()));
        assert_eq!(command, internal);
        assert!(!internal.1.contains("private-runner-sentinel"));
    }
}
