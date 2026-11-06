//! MCP identities are indexed from atomic session events, not a second database.
use crate::{Store, catalog::id_at, db_error, session::session_at};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

thread_local! { static OPERATION: std::cell::Cell<Option<(Id,Id)>> = const { std::cell::Cell::new(None) }; }
pub(crate) fn tag_operation(project: Id, value: &mut Value) {
    OPERATION.with(|scope| {
        if let Some((owner, id)) = scope.get() {
            if owner == project {
                value["mcp_operation_id"] = json!(id);
            }
        }
    });
}
/// Synchronous request scope; cleared even when a domain operation panics. Never crosses an await.
pub fn with_mcp_operation<T>(project: Id, operation: Id, apply: impl FnOnce() -> T) -> T {
    struct Restore(Option<(Id, Id)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OPERATION.with(|scope| scope.set(self.0));
        }
    }
    let _restore = Restore(OPERATION.with(|scope| scope.replace(Some((project, operation)))));
    apply()
}

fn operation_at(
    conn: &Connection,
    project: Id,
    client: &str,
    request: &str,
) -> Result<Option<McpOperation>> {
    let value: Option<String>=conn.query_row("SELECT json_extract(payload_json,'$.operation') FROM events WHERE project_id=?1 AND event_type IN ('mcp.operation_started','mcp.operation_finished','mcp.operation_recovered') AND json_extract(payload_json,'$.operation.client')=?2 AND json_extract(payload_json,'$.operation.request_id')=?3 ORDER BY project_revision DESC,id DESC LIMIT 1",params![project.to_string(),client,request],|row|row.get(0)).optional().map_err(db_error)?;
    value
        .map(|v| {
            serde_json::from_str(&v)
                .map_err(|_| Error::Storage("invalid MCP operation receipt".into()))
        })
        .transpose()
}
fn wait_at(conn: &Connection, project: Id, id: Id) -> Result<McpWait> {
    let value: String=conn.query_row("SELECT json_extract(payload_json,'$.wait') FROM events WHERE project_id=?1 AND event_type IN ('mcp.wait_created','mcp.wait_replied') AND json_extract(payload_json,'$.wait.id')=?2 ORDER BY project_revision DESC,id DESC LIMIT 1",params![project.to_string(),id.to_string()],|row|row.get(0)).optional().map_err(db_error)?.ok_or_else(||Error::NotFound("MCP wait".into()))?;
    serde_json::from_str(&value).map_err(|_| Error::Storage("invalid MCP wait receipt".into()))
}

pub(crate) fn binding_session(
    conn: &Connection,
    project: Id,
    binding: &McpSessionBinding,
) -> Result<Option<Session>> {
    let id = conn.query_row("SELECT session_id FROM events WHERE project_id=?1 AND event_type IN ('session.started','session.resumed') AND json_extract(payload_json,'$.mcp_binding.client')=?2 AND json_extract(payload_json,'$.mcp_binding.conversation')=?3 ORDER BY project_revision DESC,id DESC LIMIT 1",
        params![project.to_string(),binding.client,binding.conversation], |row|id_at(row,0)).optional().map_err(db_error)?;
    id.map(|id| session_at(conn, project, id)).transpose()
}
pub(crate) fn session_binding(
    conn: &Connection,
    project: Id,
    session: Id,
) -> Result<Option<McpSessionBinding>> {
    let value: Option<String> = conn.query_row("SELECT json_extract(payload_json,'$.mcp_binding') FROM events WHERE project_id=?1 AND session_id=?2 AND event_type IN ('session.started','session.resumed') ORDER BY project_revision DESC,id DESC LIMIT 1",
        params![project.to_string(),session.to_string()], |row|row.get(0)).optional().map_err(db_error)?.flatten();
    value
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|_| Error::Storage("invalid stored MCP binding".into()))
        })
        .transpose()
}
fn pending_waits(
    conn: &Connection,
    project: Id,
    branch: Option<Id>,
    work: Option<Id>,
) -> Result<Vec<(Id, McpWait)>> {
    let mut query = conn
        .prepare(
            "WITH replied(wait_id) AS MATERIALIZED (
            SELECT DISTINCT json_extract(payload_json,'$.wait.id') FROM events
            WHERE project_id=?1 AND event_type='mcp.wait_replied'
        )
        SELECT s.work_item_id,json_extract(e.payload_json,'$.wait') FROM events e
        JOIN sessions s ON s.id=e.session_id AND s.project_id=e.project_id
        LEFT JOIN replied ON replied.wait_id=json_extract(e.payload_json,'$.wait.id')
        WHERE e.project_id=?1 AND e.event_type='mcp.wait_created' AND s.branch_id IS ?2
          AND s.work_item_id IS NOT NULL AND (?3 IS NULL OR s.work_item_id=?3)
          AND replied.wait_id IS NULL ORDER BY e.project_revision",
        )
        .map_err(db_error)?;
    let rows = query
        .query_map(
            params![
                project.to_string(),
                branch.map(|id| id.to_string()),
                work.map(|id| id.to_string())
            ],
            |row| Ok((id_at(row, 0)?, row.get::<_, String>(1)?)),
        )
        .map_err(db_error)?;
    rows.map(|row| {
        let (id, value) = row.map_err(db_error)?;
        let wait = serde_json::from_str(&value)
            .map_err(|_| Error::Storage("invalid MCP wait receipt".into()))?;
        Ok((id, wait))
    })
    .collect()
}

impl Store {
    pub fn mcp_operation(
        &self,
        project: Id,
        client: &str,
        request: &str,
    ) -> Result<Option<McpOperation>> {
        operation_at(&self.conn, project, client, request)
    }
    pub fn begin_mcp_operation(
        &mut self,
        project: Id,
        expected: Revision,
        client: &str,
        request: &str,
        tool: &str,
        fingerprint: &str,
    ) -> Result<McpOperation> {
        if client.trim().is_empty()
            || client.len() > 128
            || request.trim().is_empty()
            || request.len() > 256
            || tool.len() > 128
            || !is_sha256_hash(fingerprint)
        {
            return Err(Error::InvalidInput("invalid MCP request identity".into()));
        }
        self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new(
                "mcp.operation_started",
                "Started an identified MCP operation; inspect its final receipt",
            ),
            |tx, next, event| {
                if operation_at(tx, project, client, request)?.is_some() {
                    return Err(Error::SourceConflict(
                        "MCP request already exists; query its receipt".into(),
                    ));
                }
                let operation = McpOperation {
                    id: Id::new(),
                    client: client.into(),
                    request_id: request.into(),
                    tool: tool.into(),
                    fingerprint: fingerprint.into(),
                    expected_revision: expected,
                    started_revision: next,
                    status: "started".into(),
                    result: None,
                    is_error: None,
                };
                event.payload = json!({"operation":operation});
                Ok(operation)
            },
        )
        .map(|(operation, _)| operation)
    }
    pub fn finish_mcp_operation(
        &mut self,
        project: Id,
        expected: Revision,
        operation: &McpOperation,
        mut result: Value,
        is_error: bool,
        recovered: bool,
    ) -> Result<(McpOperation, Event)> {
        if serde_json::to_vec(&result)?.len() > 8 * 1024 * 1024 {
            return Err(Error::InvalidInput(
                "MCP result receipt exceeds 8 MiB".into(),
            ));
        }
        self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new(
                if recovered {
                    "mcp.operation_recovered"
                } else {
                    "mcp.operation_finished"
                },
                "Recorded an MCP operation outcome",
            ),
            |tx, next, event| {
                let mut current =
                    operation_at(tx, project, &operation.client, &operation.request_id)?
                        .ok_or_else(|| Error::NotFound("MCP request".into()))?;
                if current.id != operation.id || current.status != "started" {
                    return Err(Error::SourceConflict(
                        "MCP request already has an outcome; query its receipt".into(),
                    ));
                }
                // The original domain revision remains visible; the envelope is usable for the next call.
                if let Some(revision) = result.get("project_revision").cloned() {
                    result["domain_project_revision"] = revision;
                }
                result["project_revision"] = json!(next);
                current.status = if recovered { "recovered" } else { "finished" }.into();
                current.result = Some(result);
                current.is_error = Some(is_error);
                event.payload = json!({"operation":current});
                Ok(current)
            },
        )
    }
    pub fn mcp_operation_events(
        &self,
        project: Id,
        operation: Id,
        limit: usize,
    ) -> Result<Vec<Event>> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::InvalidInput(
                "operation event limit must be 1..1000".into(),
            ));
        }
        let ids=self.conn.prepare("SELECT id FROM events WHERE project_id=?1 AND json_extract(payload_json,'$.mcp_operation_id')=?2 ORDER BY project_revision,id LIMIT ?3").map_err(db_error)?
            .query_map(params![project.to_string(),operation.to_string(),limit as i64],|row|id_at(row,0)).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        ids.into_iter().map(|id| self.event(project, id)).collect()
    }
    pub fn mcp_wait(&self, project: Id, id: Id) -> Result<McpWait> {
        wait_at(&self.conn, project, id)
    }
    pub fn mcp_waits(&self, project: Id, session: Id) -> Result<Vec<McpWait>> {
        let values=self.conn.prepare("WITH RECURSIVE lineage(session,depth) AS (SELECT ?2,0 UNION ALL SELECT json_extract(r.payload_json,'$.from_session_id'),l.depth+1 FROM events r JOIN lineage l ON r.session_id=l.session WHERE r.project_id=?1 AND r.event_type='session.resumed' AND l.depth<32) SELECT json_extract(e.payload_json,'$.wait') FROM events e WHERE e.project_id=?1 AND e.session_id IN (SELECT session FROM lineage) AND e.event_type IN ('mcp.wait_created','mcp.wait_replied') AND NOT EXISTS(SELECT 1 FROM events newer WHERE newer.project_id=e.project_id AND newer.event_type='mcp.wait_replied' AND json_extract(newer.payload_json,'$.wait.id')=json_extract(e.payload_json,'$.wait.id') AND newer.project_revision>e.project_revision) ORDER BY e.project_revision DESC LIMIT 100").map_err(db_error)?
            .query_map(params![project.to_string(),session.to_string()],|row|row.get::<_,String>(0)).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        values
            .into_iter()
            .map(|v| {
                serde_json::from_str(&v)
                    .map_err(|_| Error::Storage("invalid MCP wait receipt".into()))
            })
            .collect()
    }
    /// Unresolved waits survive an ended session and are scoped to the work branch.
    /// No UI sample limit may turn an old unanswered question into available work.
    pub fn pending_work_waits(
        &self,
        project: Id,
        work: Id,
        branch: Option<Id>,
    ) -> Result<Vec<McpWait>> {
        Ok(pending_waits(&self.conn, project, branch, Some(work))?
            .into_iter()
            .map(|(_, wait)| wait)
            .collect())
    }
    /// Project-owner observation query. Adapters authorize before exposing these facts.
    pub fn pending_project_waits(
        &self,
        project: Id,
        branch: Option<Id>,
    ) -> Result<std::collections::BTreeMap<Id, Vec<McpWait>>> {
        let mut grouped: std::collections::BTreeMap<Id, Vec<McpWait>> = Default::default();
        for (id, wait) in pending_waits(&self.conn, project, branch, None)? {
            grouped.entry(id).or_default().push(wait);
        }
        Ok(grouped)
    }
    pub fn create_mcp_wait(
        &mut self,
        project: Id,
        expected: Revision,
        client: &str,
        session: Id,
        checkpoint: Id,
        question: String,
    ) -> Result<(McpWait, Event)> {
        if question.trim().is_empty() || question.len() > 8192 {
            return Err(Error::InvalidInput(
                "wait question must contain 1..8192 bytes".into(),
            ));
        }
        self.runtime_transaction_with_event(project,expected,EventDraft::new("mcp.wait_created","Waiting for user input; host must collect the reply"),|tx,next,event| {
            let current=session_at(tx,project,session)?; crate::session::require_active(&current)?;
            if session_binding(tx,project,session)?.is_none_or(|binding|binding.client!=client) { return Err(Error::RuleViolation("wait requires this client's bound session".into())); }
            let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM events e WHERE e.project_id=?1 AND e.session_id=?2 AND e.event_type='mcp.wait_created' AND NOT EXISTS(SELECT 1 FROM events r WHERE r.project_id=e.project_id AND r.event_type='mcp.wait_replied' AND json_extract(r.payload_json,'$.wait.id')=json_extract(e.payload_json,'$.wait.id')))",params![project.to_string(),session.to_string()],|row|row.get(0)).map_err(db_error)?;
            if pending { return Err(Error::InvalidTransition("session already has a pending wait".into())); }
            if crate::checkpoint::checkpoint_at(tx,project,checkpoint)?.session_id!=session { return Err(Error::RuleViolation("wait checkpoint belongs to another session".into())); }
            let wait=McpWait{id:Id::new(),client:client.into(),session_id:session,checkpoint_id:checkpoint,question,status:"waiting_user".into(),reply:None,created_at:now_millis()?,revision:next};
            event.session_id=Some(session);event.work_item_id=current.work_item_id;event.branch_id=current.branch_id;event.importance="high".into();event.payload=json!({"wait":wait});Ok(wait)
        })
    }
    pub fn reply_mcp_wait(
        &mut self,
        project: Id,
        expected: Revision,
        client: &str,
        id: Id,
        reply: String,
        cancel: bool,
    ) -> Result<(McpWait, Event)> {
        if reply.trim().is_empty() || reply.len() > 8192 {
            return Err(Error::InvalidInput(
                "reply or cancellation reason must contain 1..8192 bytes".into(),
            ));
        }
        self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new(
                "mcp.wait_replied",
                "Recorded a user reply; host may explicitly continue work",
            ),
            |tx, next, event| {
                let mut wait = wait_at(tx, project, id)?;
                if wait.client != client {
                    return Err(Error::RuleViolation(
                        "wait belongs to another client".into(),
                    ));
                }
                if wait.status != "waiting_user" {
                    return Err(Error::InvalidTransition(
                        "wait already resolved; inspect the saved reply".into(),
                    ));
                }
                let session = session_at(tx, project, wait.session_id)?;
                wait.status = if cancel { "cancelled" } else { "answered" }.into();
                wait.reply = Some(reply);
                wait.revision = next;
                event.session_id = Some(session.id);
                event.work_item_id = session.work_item_id;
                event.branch_id = session.branch_id;
                event.importance = "high".into();
                event.payload = json!({"wait":wait});
                Ok(wait)
            },
        )
    }
    pub fn mcp_bound_session(
        &self,
        project: Id,
        binding: &McpSessionBinding,
    ) -> Result<Option<Session>> {
        binding.validate()?;
        binding_session(&self.conn, project, binding)
    }
    pub fn mcp_session_binding(
        &self,
        project: Id,
        session: Id,
    ) -> Result<Option<McpSessionBinding>> {
        self.session(project, session)?;
        session_binding(&self.conn, project, session)
    }
    pub fn mcp_sessions(
        &self,
        project: Id,
        client: &str,
        limit: usize,
        before: Option<Revision>,
    ) -> Result<Vec<Session>> {
        if !(1..=100).contains(&limit) {
            return Err(Error::InvalidInput("session limit must be 1..100".into()));
        }
        let ids = self.conn.prepare("SELECT session_id FROM events WHERE project_id=?1 AND event_type IN ('session.started','session.resumed') AND json_extract(payload_json,'$.mcp_binding.client')=?2 AND (?3 IS NULL OR project_revision<?3) ORDER BY project_revision DESC,id DESC LIMIT ?4")
            .map_err(db_error)?.query_map(params![project.to_string(),client,before.map(crate::transaction::sqlite_revision).transpose()?,limit as i64],|row|id_at(row,0)).map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        ids.into_iter()
            .map(|id| self.session(project, id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_correlation_is_project_scoped_and_restored_after_panics() {
        let project = Id::new();
        let other = Id::new();
        let outer = Id::new();
        let inner = Id::new();
        with_mcp_operation(project, outer, || {
            let mut unrelated = json!({});
            tag_operation(other, &mut unrelated);
            assert!(unrelated.get("mcp_operation_id").is_none());
            let panic = std::panic::catch_unwind(|| {
                with_mcp_operation(project, inner, || {
                    let mut event = json!({});
                    tag_operation(project, &mut event);
                    assert_eq!(event["mcp_operation_id"], json!(inner));
                    panic!("interrupted synchronous request");
                });
            });
            assert!(panic.is_err());
            let mut event = json!({});
            tag_operation(project, &mut event);
            assert_eq!(event["mcp_operation_id"], json!(outer));
        });
        let mut next_request = json!({});
        tag_operation(project, &mut next_request);
        assert!(next_request.get("mcp_operation_id").is_none());
    }
}
