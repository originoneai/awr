//! Progress observations are displayed beside source authority, never promoted over it.
use crate::status_summary::short;
use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};

pub fn work_progress(
    store: &Store,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<Value> {
    let source = &work.item.meta.source_ref;
    let checkpoint = store.latest_work_progress_checkpoint(project, work.item.meta.id, branch)?;
    let latest = checkpoint.as_ref().map(|c| -> Result<Value> {
        let session = store.session(project, c.session_id)?;
        let metadata = store.checkpoint_save_metadata(project, c.id)?;
        Ok(json!({"text":short(&c.next_action),"truncated":c.next_action.chars().count()>240,
            "checkpoint_id":c.id,"session_id":c.session_id,"recorded_at":c.created_at,
            "project_revision":c.project_revision,"session_status":session.status,
            "actor":metadata["actor"],
            "session_label":{"agent_id":short(&session.agent_id),"provider":short(&session.provider),"model":short(&session.model),"is_caller_proof":false},
            "context_hash_verified":false,"trust":Trust::Reported,
            "details":{"cli":["session","show",c.session_id.to_string()],"mcp":"awr_session_get"}}))
    }).transpose()?;
    Ok(json!({
        "source_next_action":{"text":short(&work.item.next_action),"truncated":work.item.next_action.chars().count()>240,"trust":Trust::Source,
            "source_id":source.source_id,"locator":source.locator,"pointer":source.pointer,"source_revision":source.source_revision,
            "projected_at":store.source_projected_at(project,source.source_id,source.source_revision)?,
            "timestamp_basis":"source projection receipt; source edit time is not recorded","freshness":work.source.freshness},
        "latest_checkpoint_next_action":latest,
        "differs_from_source":checkpoint.as_ref().map(|c|c.next_action!=work.item.next_action),
        "boundary":"Checkpoint progress does not modify the authoritative source or establish completion.",
        "scope":"exact work, branch and current workstream ownership; includes active and closed sessions"
    }))
}
