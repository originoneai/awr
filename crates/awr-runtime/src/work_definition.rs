//! The work definition report as the runtime serves it to its callers.
//! Read-only: no source is touched and no event is written.
use awr_core::*;
use awr_store::Store;

/// Check one work's declared definition against the project as currently indexed.
/// `state` is `not_declared` for work without a definition, which keeps every existing
/// source and output unchanged.
pub fn work_definition_report(store: &Store, project: Id, key: &str) -> Result<DefinitionReport> {
    store.work_definition_report(project, key)
}
