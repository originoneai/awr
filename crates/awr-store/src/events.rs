use crate::{
    Store,
    catalog::{id_at, revision_at},
    db_error,
    session::session_at,
    transaction::{optional_id, sqlite_revision},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(tag = "scope", content = "id", rename_all = "snake_case")]
pub enum BranchFilter {
    #[default]
    Any,
    Main,
    Branch(Id),
}
impl BranchFilter {
    pub fn exact(branch: Option<Id>) -> Self {
        branch.map(Self::Branch).unwrap_or(Self::Main)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCursor {
    pub project_id: Id,
    pub project_revision: Revision,
    pub created_at: i64,
    pub event_id: Id,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventQuery {
    pub work_item_id: Option<Id>,
    pub session_id: Option<Id>,
    pub branch: BranchFilter,
    pub event_type: Option<String>,
    pub importance: Option<String>,
    pub source_id: Option<Id>,
    pub after_revision: Revision,
    pub through_revision: Option<Revision>,
    pub cursor: Option<EventCursor>,
    pub limit: usize,
}
impl Default for EventQuery {
    fn default() -> Self {
        Self {
            work_item_id: None,
            session_id: None,
            branch: BranchFilter::Any,
            event_type: None,
            importance: None,
            source_id: None,
            after_revision: 0,
            through_revision: None,
            cursor: None,
            limit: 100,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPage {
    pub events: Vec<Event>,
    pub next_cursor: Option<EventCursor>,
    pub project_revision: Revision,
}

pub(crate) fn event_row(row: &Row<'_>) -> rusqlite::Result<Event> {
    let payload = serde_json::from_str(&row.get::<_, String>(8)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Event {
        id: id_at(row, 0)?,
        project_id: id_at(row, 1)?,
        work_item_id: optional_id(row, 2)?,
        session_id: optional_id(row, 3)?,
        branch_id: optional_id(row, 4)?,
        event_type: row.get(5)?,
        importance: row.get(6)?,
        summary: row.get(7)?,
        payload,
        project_revision: revision_at(row, 9)?,
        created_at: row.get(10)?,
    })
}
fn has_id(conn: &Connection, table: &str, project: Id, id: Id) -> Result<()> {
    // All callers pass one of the fixed domain table names below.
    let exists: bool = conn
        .query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE project_id=?1 AND id=?2)"),
            params![project.to_string(), id.to_string()],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if !exists {
        return Err(Error::NotFound(format!("{table} {id}")));
    }
    Ok(())
}

pub(crate) fn bind_generic_payload(
    conn: &Connection,
    project: Id,
    draft: &EventDraft,
) -> Result<()> {
    for (field, table) in [
        ("source_id", "sources"),
        ("artifact_id", "artifacts"),
        ("checkpoint_id", "checkpoints"),
        ("evidence_id", "evidence"),
    ] {
        if let Some(value) = draft.payload.get(field) {
            let id = value
                .as_str()
                .and_then(|v| v.parse::<Id>().ok())
                .ok_or_else(|| {
                    Error::InvalidInput("generic event reference must be an object ID".into())
                })?;
            has_id(conn, table, project, id)?;
        }
    }
    Ok(())
}

pub(crate) fn bind_event(
    conn: &Connection,
    project: Id,
    draft: &mut EventDraft,
    require_active: bool,
) -> Result<()> {
    if let Some(id) = draft.session_id {
        let session = session_at(conn, project, id)?;
        if require_active && session.status != "active" {
            return Err(Error::InvalidTransition(format!(
                "session {id} is {}",
                session.status
            )));
        }
        if let Some(bound) = session.work_item_id {
            if draft.work_item_id.is_some_and(|work| work != bound) {
                return Err(Error::InvalidInput(
                    "event work conflicts with its session binding".into(),
                ));
            }
            draft.work_item_id = Some(bound);
        }
        if draft.branch_id.is_some() && draft.branch_id != session.branch_id {
            return Err(Error::InvalidInput(
                "event branch conflicts with its session binding".into(),
            ));
        }
        draft.branch_id = session.branch_id;
    }
    if let Some(id) = draft.work_item_id {
        has_id(conn, "work_items", project, id)?;
    }
    if let Some(id) = draft.branch_id {
        has_id(conn, "branches", project, id)?;
        if require_active {
            crate::session::require_branch(conn, project, Some(id))?;
        }
    }
    if draft.event_type.trim().is_empty()
        || !draft.payload.is_object()
        || !["low", "normal", "high", "critical"].contains(&draft.importance.as_str())
    {
        return Err(Error::InvalidInput(
            "event requires type, object payload and valid importance".into(),
        ));
    }
    Ok(())
}

impl Store {
    pub fn event_payload_bytes(&self, project: Id, id: Id) -> Result<u64> {
        self.conn.query_row("SELECT length(CAST(payload_json AS BLOB)) FROM events WHERE project_id=?1 AND id=?2",params![project.to_string(),id.to_string()],|r|r.get::<_,i64>(0)).optional().map_err(db_error)?
            .map(|n|n as u64).ok_or_else(|| Error::NotFound(format!("event {id}")))
    }
    /// No arbitrary event payload/body is loaded by a metadata read.
    pub fn event_metadata(&self, project: Id, id: Id) -> Result<serde_json::Value> {
        self.conn.query_row("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,substr(summary,1,240),project_revision,created_at,
            CASE WHEN json_type(payload_json,'$.source_id')='text' THEN json_extract(payload_json,'$.source_id') END,
            CASE WHEN json_type(payload_json,'$.checkpoint_id')='text' THEN json_extract(payload_json,'$.checkpoint_id') END,
            CASE WHEN json_type(payload_json,'$.artifact_id')='text' THEN json_extract(payload_json,'$.artifact_id') END,length(summary)>240
            FROM events WHERE project_id=?1 AND id=?2",params![project.to_string(),id.to_string()],|r| {
                let mut summary=r.get::<_,String>(7)?;if r.get::<_,bool>(13)?{summary.push('…');}
                Ok(serde_json::json!({"id":id_at(r,0)?,"project_id":id_at(r,1)?,"work_item_id":optional_id(r,2)?,"session_id":optional_id(r,3)?,"branch_id":optional_id(r,4)?,
                    "type":r.get::<_,String>(5)?,"importance":r.get::<_,String>(6)?,"summary":summary,"project_revision":revision_at(r,8)?,"created_at":r.get::<_,i64>(9)?,
                    "source_id":r.get::<_,Option<String>>(10)?,"checkpoint_id":r.get::<_,Option<String>>(11)?,"artifact_id":r.get::<_,Option<String>>(12)?}))
            }).optional().map_err(db_error)?.ok_or_else(|| Error::NotFound(format!("event {id}")))
    }
    /// Newest event time of every work item that has events, over all branches and sessions: the value
    /// `event history` shows as the largest `created_at` of that work. Events without a work belong to no
    /// item, and a work without events is absent from the map.
    pub fn last_event_times(&self, project: Id) -> Result<std::collections::BTreeMap<Id, i64>> {
        self.conn
            .prepare("SELECT work_item_id,MAX(created_at) FROM events WHERE project_id=?1 AND work_item_id IS NOT NULL GROUP BY work_item_id")
            .map_err(db_error)?
            .query_map([project.to_string()], |r| Ok((id_at(r, 0)?, r.get::<_, i64>(1)?)))
            .map_err(db_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db_error)
    }
    pub fn event(&self, project: Id, id: Id) -> Result<Event> {
        self.conn.query_row("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND id=?2",params![project.to_string(),id.to_string()],event_row).optional().map_err(db_error)?.ok_or_else(||Error::NotFound(format!("event {id}")))
    }
    pub fn query_events(&self, project: Id, query: &EventQuery) -> Result<EventPage> {
        self.query_events_filtered(project, query, false, false)
    }
    pub(crate) fn query_events_scoped(&self, project: Id, query: &EventQuery) -> Result<EventPage> {
        self.query_events_filtered(project, query, false, true)
    }
    /// Reuse event ordering, cursor validation and immutable upper bounds for source consumers.
    pub fn query_source_events(&self, project: Id, query: &EventQuery) -> Result<EventPage> {
        self.query_events_filtered(project, query, true, false)
    }
    fn query_events_filtered(
        &self,
        project: Id,
        query: &EventQuery,
        sources_only: bool,
        scoped: bool,
    ) -> Result<EventPage> {
        if query.limit == 0 || query.limit > 1000 {
            return Err(Error::InvalidInput("event limit must be 1..1000".into()));
        }
        if query.cursor.is_some() && query.after_revision != 0 {
            return Err(Error::InvalidInput(
                "use either an event cursor or after_revision".into(),
            ));
        }
        if query
            .event_type
            .as_ref()
            .is_some_and(|s| s.trim().is_empty())
            || query
                .importance
                .as_ref()
                .is_some_and(|s| !["low", "normal", "high", "critical"].contains(&s.as_str()))
        {
            return Err(Error::InvalidInput(
                "invalid event type or importance filter".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let revision = tx
            .query_row(
                "SELECT project_revision FROM projects WHERE id=?1",
                [project.to_string()],
                |r| revision_at(r, 0),
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| Error::NotFound(format!("project {project}")))?;
        if query.through_revision.is_some_and(|through| {
            through > revision
                || through < query.after_revision
                || query
                    .cursor
                    .as_ref()
                    .is_some_and(|c| c.project_revision > through)
        }) {
            return Err(Error::InvalidInput(
                "event revision range is outside the current project/cursor bounds".into(),
            ));
        }
        if let Some(id) = query.source_id {
            has_id(&tx, "sources", project, id)?;
        }
        if let Some(id) = query.work_item_id {
            has_id(&tx, "work_items", project, id)?;
        }
        if let Some(id) = query.session_id {
            has_id(&tx, "sessions", project, id)?;
        }
        let (all_branches, branch) = match query.branch {
            BranchFilter::Any => (true, None),
            BranchFilter::Main => (false, None),
            BranchFilter::Branch(id) => {
                has_id(&tx, "branches", project, id)?;
                (false, Some(id))
            }
        };
        if let Some(cursor) = &query.cursor {
            if cursor.project_id != project {
                return Err(Error::InvalidInput(
                    "event cursor belongs to another project".into(),
                ));
            }
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND id=?2 AND project_revision=?3 AND created_at=?4)",params![project.to_string(),cursor.event_id.to_string(),sqlite_revision(cursor.project_revision)?,cursor.created_at],|r|r.get(0)).map_err(db_error)?;
            if !exists {
                return Err(Error::InvalidInput(
                    "event cursor does not identify an existing event".into(),
                ));
            }
        }
        let visibility = if scoped {
            crate::scoped_read::visible("events", "events.id")
        } else {
            "1".into()
        };
        let mut events=tx.prepare(&format!("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at
            FROM events WHERE project_id=?1 AND {visibility} AND (?2 IS NULL OR work_item_id=?2) AND (?3 IS NULL OR session_id=?3)
              AND (?4 OR branch_id IS ?5) AND (?6 IS NULL OR event_type=?6) AND (?7 IS NULL OR importance=?7)
              AND project_revision>?8 AND (?9 IS NULL OR (project_revision,created_at,id)>(?9,?10,?11))
              AND (?13 IS NULL OR (event_type LIKE 'source.%' AND json_extract(payload_json,'$.source_id')=?13))
              AND (?14 IS NULL OR project_revision<=?14)
              AND (NOT ?15 OR event_type LIKE 'source.%')
            ORDER BY project_revision,created_at,id LIMIT ?12")).map_err(db_error)?
            .query_map(params![project.to_string(),query.work_item_id.map(|id|id.to_string()),query.session_id.map(|id|id.to_string()),all_branches,branch.map(|id|id.to_string()),query.event_type,query.importance,sqlite_revision(query.after_revision)?,query.cursor.as_ref().map(|c|sqlite_revision(c.project_revision)).transpose()?,query.cursor.as_ref().map(|c|c.created_at),query.cursor.as_ref().map(|c|c.event_id.to_string()),(query.limit+1) as i64,query.source_id.map(|id|id.to_string()),query.through_revision.map(sqlite_revision).transpose()?,sources_only],event_row).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        let more = events.len() > query.limit;
        events.truncate(query.limit);
        let next_cursor = if more {
            events.last().map(|event| EventCursor {
                project_id: project,
                project_revision: event.project_revision,
                created_at: event.created_at,
                event_id: event.id,
            })
        } else {
            None
        };
        tx.commit().map_err(db_error)?;
        Ok(EventPage {
            events,
            next_cursor,
            project_revision: revision,
        })
    }
}
