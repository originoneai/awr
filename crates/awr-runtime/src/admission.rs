//! Recording admission decisions. The store derives and persists; this adds the idempotent replay
//! and the same source refresh the other recording operations perform, so CLI and MCP share it.
use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::path::Path;

pub fn admit_work(store: &mut Store, root: &Path, request: &AdmissionRequest) -> Result<Value> {
    request.validate()?;
    let project = store.project_by_root(root)?;
    let work = store.work_item(project.id, &request.work)?;
    if let Some(event) =
        store.admission_by_key(project.id, work.item.meta.id, &request.request_key)?
    {
        if event.payload["input"] != request.input() {
            return Err(Error::SourceConflict(
                "admission request_key already binds a different request".into(),
            ));
        }
        let admission = store.admission_status(project.id, &request.work, event.branch_id)?;
        return Ok(
            json!({"ok":true,"already_recorded":true,"event":event,"admission":admission,
            "project_revision":project.project_revision}),
        );
    }
    let manifest = awr_source::Manifest::load(root)?;
    if !awr_source::index_project(store, root, &manifest, false)?.ok {
        return Err(Error::SourceStale(
            "refresh sources before recording admission".into(),
        ));
    }
    let actual = store.project(project.id)?.project_revision;
    if actual != request.expected_revision {
        return Err(Error::RevisionConflict {
            expected: request.expected_revision,
            actual,
        });
    }
    let (event, admission) = store.record_admission(project.id, request)?;
    Ok(
        json!({"ok":true,"already_recorded":false,"event":event,"admission":admission,
        "project_revision":event.project_revision,"source_write_performed":false,
        "execution_started":false,"completion_claimed":false}),
    )
}
