//! After a refresh of the sources: release the claims on work the source now calls finished.
//!
//! Take a `Watch` before the sources are refreshed and `reconcile` it afterwards, in a runtime
//! transaction of its own; the store acts only on the works that were open when the watch was taken
//! (see `awr_store::Store::reconcile_finished_work`). It never fails the command that asked for the
//! refresh: what it cannot do it reports, and nothing is said when there was nothing to do.
use awr_core::Id;
use awr_store::{SOURCE_TERMINAL, Store};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(crate) struct Watch {
    project: Id,
    works: BTreeSet<Id>,
}

impl Watch {
    pub(crate) fn none() -> Self {
        Self {
            project: Id::new(),
            works: BTreeSet::new(),
        }
    }

    pub(crate) fn take(store: &Store, project: Id) -> Self {
        Self {
            project,
            works: store.works_with_open_claims(project).unwrap_or_default(),
        }
    }

    pub(crate) fn reconcile(&self, store: &mut Store) -> Option<Value> {
        if self.works.is_empty() {
            return None;
        }
        match store.reconcile_finished_work(self.project, &self.works) {
            Ok(report) if report.is_empty() => None,
            Ok(report) => {
                let mut note = serde_json::to_value(report).ok()?;
                note["reason"] = json!(SOURCE_TERMINAL);
                Some(note)
            }
            Err(error) => Some(json!({
                "reason": SOURCE_TERMINAL,
                "skipped": [{"reason": "could_not_reconcile", "code": error.code()}],
            })),
        }
    }
}
