//! Domain contracts shared by storage, source adapters, runtime and interfaces.
mod workstream_graph;
pub use workstream_graph::*;
mod workstream_readset;
pub use workstream_readset::*;
mod branch;
mod branch_close;
mod completion;
mod error;
mod event_payload;
mod execution;
mod execution_adapter;
pub use execution_adapter::*;
mod management;
mod model;
pub use management::*;
mod content_review;
mod mutation;
mod query;
mod runtime;
mod secrets;
pub use content_review::*;
mod work_action;
mod workstream;
mod workstream_accounting;

pub use branch::*;
pub use branch_close::*;
pub use completion::*;
pub use error::{
    DiagnosticLocation, Error, ErrorReport, Result, SourceDiagnostic, render_diagnostic_details,
};
pub use event_payload::*;
pub use execution::*;
pub use model::*;
pub use mutation::*;
pub use query::*;
pub use runtime::*;
pub use secrets::*;
pub use work_action::*;
pub use workstream::*;
pub use workstream_accounting::*;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn now_millis() -> Result<i64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    i64::try_from(elapsed.as_millis()).map_err(|_| Error::InvalidInput("timestamp overflow".into()))
}

mod client;
pub use client::ClientBinding;
mod compaction;
pub use compaction::*;
mod guidance;
pub use guidance::*;
mod fact_snapshot;
pub use fact_snapshot::*;
mod assessment_envelope;
pub use assessment_envelope::*;
mod ordinary;
pub use ordinary::*;
mod responsibility;
pub use responsibility::*;
mod agent_authorization;
pub use agent_authorization::*;
mod team_handoff;
pub use team_handoff::*;

pub mod workstream_usage;

pub mod workstream_eta;
pub use workstream_eta::*;

pub mod workstream_adoption;
pub use workstream_adoption::*;

pub mod migration_takeover;
pub use migration_takeover::*;

pub use secrets::sensitive_field_category;

pub use secrets::content_text_ranges;
