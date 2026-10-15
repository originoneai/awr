use crate::{
    Store,
    catalog::{id_at, revision_at},
    db_error,
    session::{require_branch, session_at},
    transaction::{optional_id, sqlite_revision},
};
use awr_core::*;
use rusqlite::{OptionalExtension, params};

pub(crate) fn checkpoint_actor(agent: Option<&str>) -> serde_json::Value {
    serde_json::json!({"agent_id":agent,"origin":if agent.is_some(){"caller_supplied_agent"}else{"undeclared"},"identity_verified":false})
}

pub(crate) fn checkpoint_at(
    conn: &rusqlite::Connection,
    project: Id,
    id: Id,
) -> Result<Checkpoint> {
    conn.query_row("SELECT c.id,c.session_id,c.project_revision,c.context_hash,c.digest,c.next_action,c.open_loops_json,c.changed_entities_json,c.revision,c.created_at
            FROM checkpoints c JOIN sessions s ON s.id=c.session_id WHERE s.project_id=?1 AND c.id=?2",params![project.to_string(),id.to_string()],|r| {
                let strings=|column|->rusqlite::Result<Vec<String>> {serde_json::from_str(&r.get::<_,String>(column)?).map_err(|e|rusqlite::Error::FromSqlConversionFailure(column,rusqlite::types::Type::Text,Box::new(e)))};
                Ok(Checkpoint {id:id_at(r,0)?,session_id:id_at(r,1)?,project_revision:revision_at(r,2)?,context_hash:r.get(3)?,digest:r.get(4)?,next_action:r.get(5)?,open_loops:strings(6)?,changed_entities:strings(7)?,revision:revision_at(r,8)?,created_at:r.get(9)?})
            }).optional().map_err(db_error)?.ok_or_else(||Error::NotFound(format!("checkpoint {id}")))
}

pub(crate) fn validate_draft(draft: &CheckpointDraft) -> Result<()> {
    ensure_public_data(draft)?;
    if !is_sha256_hash(&draft.context_hash) {
        return Err(Error::InvalidInput(
            "checkpoint context_hash (--context-hash) must be 64 hexadecimal characters without a sha256: prefix; use work_context.context_hash from the last consumed work prepare or context compile result, not a source fingerprint".into(),
        ));
    }
    for (field, flag, value) in [
        ("digest", "--digest", &draft.digest),
        ("next_action", "--next-action", &draft.next_action),
    ] {
        if value.trim().is_empty() {
            return Err(Error::InvalidInput(format!(
                "checkpoint {field} ({flag}) must not be blank"
            )));
        }
    }
    for (field, flag, entries) in [
        ("open_loops", "--open-loop", &draft.open_loops),
        (
            "changed_entities",
            "--changed-entity",
            &draft.changed_entities,
        ),
    ] {
        if let Some(index) = entries.iter().position(|entry| entry.trim().is_empty()) {
            return Err(Error::InvalidInput(format!(
                "checkpoint {field} ({flag}) entry {} must not be blank",
                index + 1
            )));
        }
    }
    Ok(())
}

pub(crate) fn write_checkpoint(
    tx: &rusqlite::Connection,
    project: Id,
    session: &Session,
    base_revision: Revision,
    draft: CheckpointDraft,
    event: &mut EventDraft,
) -> Result<Checkpoint> {
    let workstream_binding = crate::workstream_runtime::recorded(tx, project, session.id)?;
    if workstream_binding.work_item_id != session.work_item_id.map(|id| id.to_string()) {
        return Err(WorkstreamError::BindingMismatch.into());
    }
    let checkpoint = Checkpoint {
        id: Id::new(),
        session_id: session.id,
        project_revision: base_revision,
        context_hash: draft.context_hash.to_ascii_lowercase(),
        digest: draft.digest,
        next_action: draft.next_action,
        open_loops: draft.open_loops,
        changed_entities: draft.changed_entities,
        revision: 1,
        created_at: now_millis()?,
    };
    tx.execute("INSERT INTO checkpoints(id,session_id,project_revision,context_hash,digest,next_action,open_loops_json,changed_entities_json,revision,created_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1,?9)",params![checkpoint.id.to_string(),session.id.to_string(),sqlite_revision(base_revision)?,checkpoint.context_hash,checkpoint.digest,checkpoint.next_action,serde_json::to_string(&checkpoint.open_loops)?,serde_json::to_string(&checkpoint.changed_entities)?,checkpoint.created_at]).map_err(db_error)?;
    tx.execute("UPDATE sessions SET last_checkpoint_id=?1,revision=revision+1 WHERE project_id=?2 AND id=?3",params![checkpoint.id.to_string(),project.to_string(),session.id.to_string()]).map_err(db_error)?;
    event.session_id = Some(session.id);
    event.work_item_id = session.work_item_id;
    event.branch_id = session.branch_id;
    event.payload = serde_json::json!({"workstream_binding":workstream_binding,"checkpoint_id":checkpoint.id,"context_hash":checkpoint.context_hash,"checkpoint_project_revision":base_revision,"changed_entities":checkpoint.changed_entities,"next_action":checkpoint.next_action,"actor":checkpoint_actor(None)});
    Ok(checkpoint)
}

impl Store {
    /// Latest recorded progress, including active sessions, in the exact branch
    /// and current ownership scope. This is not the closed-session resume selector.
    pub fn latest_work_progress_checkpoint(
        &self,
        project: Id,
        work: Id,
        branch: Option<Id>,
    ) -> Result<Option<Checkpoint>> {
        let id = self.conn.query_row("SELECT c.id FROM checkpoints c JOIN sessions s ON s.id=c.session_id
            JOIN session_workstreams b ON b.project_id=s.project_id AND b.session_id=s.id
            JOIN workstream_ownership o ON o.project_id=b.project_id AND o.work_item_id=b.work_item_id
                AND o.workstream_id=b.workstream_id AND o.revision=b.ownership_revision
            WHERE s.project_id=?1 AND s.work_item_id=?2 AND s.branch_id IS ?3
            ORDER BY c.project_revision DESC,c.created_at DESC,c.id DESC LIMIT 1",
            params![project.to_string(),work.to_string(),branch.map(|id|id.to_string())],|r|id_at(r,0))
            .optional().map_err(db_error)?;
        id.map(|id| self.checkpoint(project, id)).transpose()
    }
    /// Latest checkpoint from a closed session for this work and exact branch.
    pub fn latest_work_checkpoint(
        &self,
        project: Id,
        work: Id,
        branch: Option<Id>,
    ) -> Result<Option<Checkpoint>> {
        let id=self.conn.query_row("SELECT c.id FROM checkpoints c JOIN sessions s ON s.id=c.session_id JOIN session_workstreams b ON b.project_id=s.project_id AND b.session_id=s.id JOIN workstream_ownership o ON o.project_id=b.project_id AND o.work_item_id=b.work_item_id AND o.workstream_id=b.workstream_id AND o.revision=b.ownership_revision WHERE s.project_id=?1 AND s.work_item_id=?2 AND s.branch_id IS ?3 AND s.status!='active' ORDER BY c.project_revision DESC,c.created_at DESC,c.id DESC LIMIT 1",params![project.to_string(),work.to_string(),branch.map(|id|id.to_string())],|r|id_at(r,0)).optional().map_err(db_error)?;
        id.map(|id| self.checkpoint(project, id)).transpose()
    }
    pub fn create_checkpoint(
        &mut self,
        project: Id,
        expected: Revision,
        session: Id,
        draft: CheckpointDraft,
    ) -> Result<(Checkpoint, Event)> {
        validate_draft(&draft)?;
        self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new("checkpoint.created", "Saved session checkpoint"),
            |tx, _, event| {
                let current = session_at(tx, project, session)?;
                if current.status != "active" {
                    return Err(Error::InvalidTransition(format!(
                        "session {session} is {}",
                        current.status
                    )));
                }
                require_branch(tx, project, current.branch_id)?;
                write_checkpoint(tx, project, &current, expected, draft, event)
            },
        )
    }
    pub fn checkpoint(&self, project: Id, id: Id) -> Result<Checkpoint> {
        checkpoint_at(&self.conn, project, id)
    }
    pub fn latest_checkpoint(&self, project: Id, session: Id) -> Result<Option<Checkpoint>> {
        let current = self.session(project, session)?;
        let checkpoint = current
            .last_checkpoint_id
            .map(|id| self.checkpoint(project, id))
            .transpose()?;
        if checkpoint.as_ref().is_some_and(|c| c.session_id != session) {
            return Err(Error::Storage(
                "latest checkpoint belongs to another session".into(),
            ));
        }
        Ok(checkpoint)
    }
    /// Record metadata for an external artifact. Byte copying and hash verification live in Runtime.
    pub fn record_artifact(
        &mut self,
        project: Id,
        expected: Revision,
        draft: ArtifactDraft,
    ) -> Result<(Artifact, Event)> {
        ensure_public_data(&draft)?;
        if [&draft.artifact_type, &draft.locator, &draft.mime]
            .iter()
            .any(|s| s.trim().is_empty())
            || !is_sha256_hash(&draft.sha256)
            || draft.size > i64::MAX as u64
        {
            return Err(Error::InvalidInput(
                "artifact requires type, locator, SHA256, size and MIME".into(),
            ));
        }
        self.runtime_transaction_with_event(project,expected,EventDraft::new("artifact.recorded","Registered external artifact"),|tx,_,event| {
            let source=tx.query_row("SELECT work_item_id,session_id,branch_id FROM events WHERE project_id=?1 AND id=?2",params![project.to_string(),draft.source_event_id.to_string()],|r|Ok((optional_id(r,0)?,optional_id(r,1)?,optional_id(r,2)?))).optional().map_err(db_error)?.ok_or_else(||Error::NotFound(format!("source event {}",draft.source_event_id)))?;
            let artifact=Artifact {id:Id::new(),project_id:project,artifact_type:draft.artifact_type,locator:draft.locator,sha256:draft.sha256.to_ascii_lowercase(),size:draft.size,mime:draft.mime,source_event_id:Some(draft.source_event_id),revision:1};
            tx.execute("INSERT INTO artifacts(id,project_id,artifact_type,locator,sha256,size,mime,source_event_id,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1)",params![artifact.id.to_string(),project.to_string(),artifact.artifact_type,artifact.locator,artifact.sha256,artifact.size as i64,artifact.mime,draft.source_event_id.to_string()]).map_err(db_error)?;
            event.work_item_id=source.0;event.session_id=source.1;event.branch_id=source.2;event.payload=serde_json::json!({"artifact_id":artifact.id,"source_event_id":draft.source_event_id,"sha256":artifact.sha256,"size":artifact.size,"locator":artifact.locator});
            Ok(artifact)
        })
    }
    pub fn artifact(&self, project: Id, id: Id) -> Result<Artifact> {
        self.conn.query_row("SELECT id,project_id,artifact_type,locator,sha256,size,mime,source_event_id,revision FROM artifacts WHERE project_id=?1 AND id=?2",params![project.to_string(),id.to_string()],|r|Ok(Artifact {id:id_at(r,0)?,project_id:id_at(r,1)?,artifact_type:r.get(2)?,locator:r.get(3)?,sha256:r.get(4)?,size:revision_at(r,5)?,mime:r.get(6)?,source_event_id:optional_id(r,7)?,revision:revision_at(r,8)?})).optional().map_err(db_error)?.ok_or_else(||Error::NotFound(format!("artifact {id}")))
    }
}
