//! The outcome history of a project: what its receipts, executions and usage say, read and counted. Read-only;
//! the instant of the question is the caller's, so the same question asked again gives the same report.
use awr_core::{
    Error, HistoryRequest, Id, OutcomeHistory, OutcomeHistoryError, Result,
    aggregate_outcome_history,
};
use awr_store::Store;

pub fn outcome_history(
    store: &Store,
    project: Id,
    branch: Option<Id>,
    request: &HistoryRequest,
) -> Result<OutcomeHistory> {
    let input = store.outcome_history_input(project, branch)?;
    aggregate_outcome_history(&input, request).map_err(|error| match error {
        OutcomeHistoryError::InvalidRequest | OutcomeHistoryError::TooLarge => {
            Error::InvalidInput(error.to_string())
        }
        // What the store holds does not agree with itself: that is a fault to report, not a count to make.
        other => Error::Storage(format!("outcome history: {other}")),
    })
}
