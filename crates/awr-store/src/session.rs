use crate::{
    Store,
    catalog::{id_at, revision_at},
    db_error,
    query::projection,
    transaction::{optional_id, sqlite_revision},
    work::readiness,
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, Row, params};

pub(crate) const SESSION_COLUMNS: &str = "id,project_id,work_item_id,branch_id,agent_id,provider,model,status,started_at,ended_at,start_project_revision,end_project_revision,last_checkpoint_id,revision";
pub(crate) const CLAIM_COLUMNS: &str = "id,project_id,work_item_id,session_id,agent_id,branch_id,status,acquired_at,expires_at,released_at,revision";

pub(crate) fn session_row(row: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: id_at(row, 0)?,
        project_id: id_at(row, 1)?,
        work_item_id: optional_id(row, 2)?,
        branch_id: optional_id(row, 3)?,
        agent_id: row.get(4)?,
        provider: row.get(5)?,
        model: row.get(6)?,
        status: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
        start_project_revision: revision_at(row, 10)?,
        end_project_revision: row
            .get::<_, Option<i64>>(11)?
            .map(|_| revision_at(row, 11))
            .transpose()?,
        last_checkpoint_id: optional_id(row, 12)?,
        revision: revision_at(row, 13)?,
    })
}
pub(crate) fn claim_row(row: &Row<'_>) -> rusqlite::Result<Claim> {
    Ok(Claim {
        id: id_at(row, 0)?,
        project_id: id_at(row, 1)?,
        work_item_id: id_at(row, 2)?,
        session_id: id_at(row, 3)?,
        agent_id: row.get(4)?,
        branch_id: optional_id(row, 5)?,
        status: row.get(6)?,
        acquired_at: row.get(7)?,
        expires_at: row.get(8)?,
        released_at: row.get(9)?,
        revision: revision_at(row, 10)?,
    })
}
pub(crate) fn session_at(conn: &Connection, project: Id, id: Id) -> Result<Session> {
    conn.query_row(
        &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE project_id=?1 AND id=?2"),
        params![project.to_string(), id.to_string()],
        session_row,
    )
    .optional()
    .map_err(db_error)?
    .ok_or_else(|| Error::NotFound(format!("session {id}")))
}
fn claim_at(conn: &Connection, project: Id, id: Id) -> Result<Claim> {
    conn.query_row(
        &format!("SELECT {CLAIM_COLUMNS} FROM claims WHERE project_id=?1 AND id=?2"),
        params![project.to_string(), id.to_string()],
        claim_row,
    )
    .optional()
    .map_err(db_error)?
    .ok_or_else(|| Error::NotFound(format!("claim {id}")))
}
pub(crate) fn require_active(session: &Session) -> Result<()> {
    if session.status != "active" {
        return Err(Error::InvalidTransition(format!(
            "session {} is {}",
            session.id, session.status
        )));
    }
    Ok(())
}
pub(crate) fn require_branch(conn: &Connection, project: Id, branch: Option<Id>) -> Result<()> {
    if let Some(id) = branch {
        let active:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM branches WHERE project_id=?1 AND id=?2 AND status='active')",params![project.to_string(),id.to_string()],|r|r.get(0)).map_err(db_error)?;
        if !active {
            return Err(Error::NotFound(format!("active branch {id}")));
        }
    }
    Ok(())
}
pub(crate) fn expires_at(at: i64, ttl: Option<u64>) -> Result<Option<i64>> {
    ttl.map(|ttl| {
        if ttl == 0 {
            return Err(Error::InvalidInput("claim TTL must be positive".into()));
        }
        let ttl =
            i64::try_from(ttl).map_err(|_| Error::InvalidInput("claim TTL overflow".into()))?;
        at.checked_add(ttl)
            .ok_or_else(|| Error::InvalidInput("claim expiration overflow".into()))
    })
    .transpose()
}

pub(crate) fn acquire(
    conn: &Connection,
    session: &Session,
    ttl: Option<u64>,
    revision: Revision,
    event: &mut EventDraft,
) -> Result<Claim> {
    require_active(session)?;
    crate::workstream_runtime::require_current(conn, session)?;
    require_branch(conn, session.project_id, session.branch_id)?;
    let work_id = session
        .work_item_id
        .ok_or_else(|| Error::InvalidInput("claim requires a work-bound session".into()))?;
    let key: String = conn
        .query_row(
            "SELECT external_key FROM work_items WHERE project_id=?1 AND id=?2 AND active=1",
            params![session.project_id.to_string(), work_id.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| Error::NotFound(format!("active work item {work_id}")))?;
    let at = now_millis()?;
    let expires_at = expires_at(at, ttl)?;
    let ready = readiness(
        conn,
        session.project_id,
        &key,
        session.branch_id,
        at,
        revision,
    )?;
    let diagnostics = ready
        .diagnostics
        .iter()
        .filter(|d| {
            !(d.code == "status_not_selectable"
                && matches!(
                    ready.work.item.status,
                    WorkStatus::Claimed | WorkStatus::InProgress
                ))
        })
        .collect::<Vec<_>>();
    if diagnostics.iter().any(|d| d.code == "active_claim") {
        return Err(Error::ClaimConflict(serde_json::to_string(
            &ready.active_claims,
        )?));
    }
    if diagnostics.iter().any(|d| d.code == "source_not_fresh") {
        return Err(Error::SourceStale(format!(
            "work {key} or its dependencies are not fresh"
        )));
    }
    if !diagnostics.is_empty() && diagnostics.iter().all(|d| d.code.starts_with("admission_")) {
        return Err(Error::Admission {
            code: crate::admission::static_code(&diagnostics[0].code),
            message: diagnostics[0].detail.clone(),
            diagnostics: serde_json::json!(
                diagnostics
                    .iter()
                    .map(|d| serde_json::json!({"code": d.code, "detail": d.detail}))
                    .collect::<Vec<_>>()
            ),
        });
    }
    if !diagnostics.is_empty() {
        return Err(Error::DependencyBlocked(serde_json::to_string(
            &diagnostics,
        )?));
    }
    let expired=conn.prepare("SELECT id FROM claims WHERE project_id=?1 AND work_item_id=?2 AND status='active' AND expires_at<=?3").map_err(db_error)?
        .query_map(params![session.project_id.to_string(),work_id.to_string(),at],|r|id_at(r,0)).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
    conn.execute("UPDATE claims SET status='expired',revision=revision+1 WHERE project_id=?1 AND work_item_id=?2 AND status='active' AND expires_at<=?3",params![session.project_id.to_string(),work_id.to_string(),at]).map_err(db_error)?;
    let claim = Claim {
        id: Id::new(),
        project_id: session.project_id,
        work_item_id: work_id,
        session_id: session.id,
        agent_id: session.agent_id.clone(),
        branch_id: session.branch_id,
        status: "active".into(),
        acquired_at: at,
        expires_at,
        released_at: None,
        revision: 1,
    };
    conn.execute("INSERT INTO claims(id,project_id,work_item_id,session_id,agent_id,branch_id,status,acquired_at,expires_at,revision)
        VALUES(?1,?2,?3,?4,?5,?6,'active',?7,?8,1)",params![claim.id.to_string(),session.project_id.to_string(),work_id.to_string(),session.id.to_string(),session.agent_id,session.branch_id.map(|id|id.to_string()),at,expires_at]).map_err(db_error)?;
    event.work_item_id = Some(work_id);
    event.session_id = Some(session.id);
    event.branch_id = session.branch_id;
    event.payload["claim_id"] = serde_json::json!(claim.id);
    event.payload["expired_claim_ids"] = serde_json::json!(expired);
    event.payload["agent_id"] = serde_json::json!(session.agent_id);
    event.payload["expires_at"] = serde_json::json!(expires_at);
    Ok(claim)
}

impl Store {
    /// Filter before limiting, so a large session history cannot hide ambiguity.
    pub fn select_active_session(
        &self,
        project: Id,
        explicit: Option<Id>,
        work: Option<Id>,
        agent: Option<&str>,
        branch: Option<Id>,
    ) -> Result<Session> {
        self.select_active_session_scoped(project, explicit, work, agent, branch, false)
    }
    pub(crate) fn select_active_session_scoped(
        &self,
        project: Id,
        explicit: Option<Id>,
        work: Option<Id>,
        agent: Option<&str>,
        branch: Option<Id>,
        scoped: bool,
    ) -> Result<Session> {
        self.project(project)?;
        let visibility = if scoped {
            format!(
                "{} AND (work_item_id IS NULL OR EXISTS(SELECT 1 FROM session_workstreams b JOIN workstream_ownership o ON o.project_id=b.project_id AND o.work_item_id=b.work_item_id AND o.workstream_id=b.workstream_id AND o.revision=b.ownership_revision WHERE b.session_id=sessions.id))",
                crate::scoped_read::visible("sessions", "sessions.id")
            )
        } else {
            "1".into()
        };
        let sessions = self.conn.prepare(&format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE ({visibility}) AND project_id=?1 AND status='active' AND (?2 IS NULL OR id=?2) AND (?3 IS NULL OR work_item_id=?3) AND (?4 IS NULL OR agent_id=?4) AND (?2 IS NOT NULL OR branch_id IS ?5) ORDER BY id LIMIT 2")).map_err(db_error)?
            .query_map(params![project.to_string(),explicit.map(|id|id.to_string()),work.map(|id|id.to_string()),agent,branch.map(|id|id.to_string())],session_row).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        match sessions.len() {
            0 => Err(Error::NotFound(
                "active session matching the supplied identity".into(),
            )),
            1 => Ok(sessions.into_iter().next().unwrap()),
            _ => Err(Error::InvalidInput(format!(
                "multiple active sessions match; specify --session: {}",
                sessions
                    .iter()
                    .map(|s| s.id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// Resolve retained identity even when a work item was removed from its source.
    pub fn work_identity(&self, project: Id, key: &str) -> Result<Id> {
        self.conn
            .query_row(
                "SELECT id FROM work_items WHERE project_id=?1 AND external_key=?2",
                params![project.to_string(), key],
                |r| id_at(r, 0),
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| Error::NotFound(format!("work item {key}")))
    }
    pub fn work_item_by_id(&self, project: Id, id: Id) -> Result<Projected<WorkItem>> {
        let key: String = self
            .conn
            .query_row(
                "SELECT external_key FROM work_items WHERE project_id=?1 AND id=?2 AND active=1",
                params![project.to_string(), id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| Error::NotFound(format!("active work item {id}")))?;
        self.work_item(project, &key)
    }
    pub fn session(&self, project: Id, id: Id) -> Result<Session> {
        session_at(&self.conn, project, id)
    }
    pub fn claim(&self, project: Id, id: Id) -> Result<Claim> {
        claim_at(&self.conn, project, id)
    }
    pub fn sessions(&self, project: Id, active_only: bool, limit: usize) -> Result<Vec<Session>> {
        if limit == 0 || limit > 1000 {
            return Err(Error::InvalidInput("session limit must be 1..1000".into()));
        }
        self.project(project)?;
        self.conn.prepare(&format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE project_id=?1 AND (?2=0 OR status='active') ORDER BY started_at DESC,id DESC LIMIT ?3")).map_err(db_error)?
            .query_map(params![project.to_string(),active_only,limit as i64],session_row).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }
    pub fn session_claims(&self, project: Id, session: Id) -> Result<Vec<Claim>> {
        self.session(project, session)?;
        self.conn.prepare(&format!("SELECT {CLAIM_COLUMNS} FROM claims WHERE project_id=?1 AND session_id=?2 ORDER BY acquired_at,id")).map_err(db_error)?
            .query_map(params![project.to_string(),session.to_string()],claim_row).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }
    pub fn start_session(
        &mut self,
        project: Id,
        expected: Revision,
        draft: SessionDraft,
    ) -> Result<(SessionStarted, Event)> {
        self.start_session_inner(project, expected, draft, None, None)
    }
    pub fn start_bound_session(
        &mut self,
        project: Id,
        expected: Revision,
        draft: SessionDraft,
        binding: McpSessionBinding,
    ) -> Result<(SessionStarted, Event)> {
        binding.validate()?;
        self.start_session_inner(project, expected, draft, Some(binding), None)
    }
    pub fn start_session_in_workstream(
        &mut self,
        project: Id,
        expected: Revision,
        draft: SessionDraft,
        binding: Option<McpSessionBinding>,
        workstream: Id,
    ) -> Result<(SessionStarted, Event)> {
        if let Some(binding) = &binding {
            binding.validate()?;
        }
        self.start_session_inner(project, expected, draft, binding, Some(workstream))
    }
    fn start_session_inner(
        &mut self,
        project: Id,
        expected: Revision,
        draft: SessionDraft,
        binding: Option<McpSessionBinding>,
        workstream: Option<Id>,
    ) -> Result<(SessionStarted, Event)> {
        if [&draft.agent_id, &draft.provider, &draft.model]
            .iter()
            .any(|s| s.trim().is_empty())
        {
            return Err(Error::InvalidInput(
                "session requires agent, provider and model".into(),
            ));
        }
        if !draft.claim && draft.claim_ttl_ms.is_some() {
            return Err(Error::InvalidInput(
                "claim TTL requires claim acquisition".into(),
            ));
        }
        if draft.claim && draft.work_item_key.is_none() {
            return Err(Error::InvalidInput("claim requires a work item".into()));
        }
        let id = Id::new();
        let mut event = EventDraft::new("session.started", "Started agent work session");
        event.session_id = Some(id);
        event.branch_id = draft.branch_id;
        self.runtime_transaction_with_event(project,expected,event,|tx,_next,event| {
            if let Some(binding)=&binding {
                if crate::mcp::binding_session(tx,project,binding)?.is_some() {
                    return Err(Error::SourceConflict("MCP conversation already has a session; inspect or resume it".into()));
                }
            }
            require_branch(tx,project,draft.branch_id)?;
            let work=draft.work_item_key.as_deref().map(|key|projection::<WorkItem>(tx,project,EntityKind::WorkItem,key)).transpose()?;
            if work.as_ref().is_some_and(|w|w.source.freshness!=Freshness::Fresh) {return Err(Error::SourceStale("session work source is not fresh".into()));}
            let session=Session {id,project_id:project,work_item_id:work.as_ref().map(|w|w.item.meta.id),branch_id:draft.branch_id,agent_id:draft.agent_id,provider:draft.provider,model:draft.model,status:"active".into(),started_at:now_millis()?,ended_at:None,start_project_revision:expected,end_project_revision:None,last_checkpoint_id:None,revision:1};
            tx.execute("INSERT INTO sessions(id,project_id,work_item_id,branch_id,agent_id,provider,model,status,started_at,start_project_revision,revision)
                VALUES(?1,?2,?3,?4,?5,?6,?7,'active',?8,?9,1)",params![id.to_string(),project.to_string(),session.work_item_id.map(|id|id.to_string()),session.branch_id.map(|id|id.to_string()),session.agent_id,session.provider,session.model,session.started_at,sqlite_revision(expected)?]).map_err(db_error)?;
            let scope=crate::workstream_runtime::bind(tx,&session,workstream,binding.as_ref())?;
            event.work_item_id=session.work_item_id;event.payload=serde_json::json!({"agent_id":session.agent_id,"provider":session.provider,"model":session.model,"start_project_revision":expected,"workstream_binding":scope});
            if let Some(binding)=binding { event.payload["mcp_binding"]=serde_json::to_value(binding)?; }
            let claim=if draft.claim {Some(acquire(tx,&session,draft.claim_ttl_ms,expected,event)?)} else {None};
            Ok(SessionStarted {session,claim})
        })
    }
    pub fn acquire_claim(
        &mut self,
        project: Id,
        expected: Revision,
        session: Id,
        ttl_ms: Option<u64>,
    ) -> Result<(Claim, Event)> {
        self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new("work.claimed", "Acquired runtime work claim"),
            |tx, _next, event| {
                let session = session_at(tx, project, session)?;
                acquire(tx, &session, ttl_ms, expected, event)
            },
        )
    }
    pub fn release_claim(
        &mut self,
        project: Id,
        expected: Revision,
        session: Id,
        claim: Id,
    ) -> Result<(Claim, Event)> {
        self.runtime_transaction_with_event(project,expected,EventDraft::new("claim.released","Released runtime work claim"),|tx,_next,event| {
            let owner=session_at(tx,project,session)?;let current=claim_at(tx,project,claim)?;
            if current.session_id!=owner.id || current.agent_id!=owner.agent_id {return Err(Error::ClaimConflict("only the holding session can release a claim".into()));}
            if current.status!="active" {return Err(Error::InvalidTransition(format!("claim {claim} is {}",current.status)));}
            let at=now_millis()?;let status=if current.active_at(at) {"released"} else {"expired"};
            tx.execute("UPDATE claims SET status=?1,released_at=?2,revision=revision+1 WHERE project_id=?3 AND id=?4",params![status,if status=="released" {Some(at)} else {None},project.to_string(),claim.to_string()]).map_err(db_error)?;
            event.event_type=format!("claim.{status}");event.work_item_id=Some(current.work_item_id);event.session_id=Some(session);event.branch_id=current.branch_id;event.payload=serde_json::json!({"claim_id":claim,"agent_id":owner.agent_id});
            claim_at(tx,project,claim)
        })
    }
    pub fn end_session(
        &mut self,
        project: Id,
        expected: Revision,
        session: Id,
        outcome: SessionOutcome,
    ) -> Result<(Session, Event)> {
        self.runtime_transaction_with_event(project,expected,EventDraft::new("session.ended","Ended agent work session"),|tx,next,event| {
            let current=session_at(tx,project,session)?;require_active(&current)?;let at=now_millis()?;
            let claims=tx.prepare("SELECT id FROM claims WHERE project_id=?1 AND session_id=?2 AND status='active' ORDER BY id").map_err(db_error)?.query_map(params![project.to_string(),session.to_string()],|r|id_at(r,0)).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
            tx.execute("UPDATE claims SET status=CASE WHEN expires_at<=?1 THEN 'expired' ELSE 'released' END,released_at=CASE WHEN expires_at<=?1 THEN NULL ELSE ?1 END,revision=revision+1 WHERE project_id=?2 AND session_id=?3 AND status='active'",params![at,project.to_string(),session.to_string()]).map_err(db_error)?;
            tx.execute("UPDATE sessions SET status=?1,ended_at=?2,end_project_revision=?3,revision=revision+1 WHERE project_id=?4 AND id=?5",params![outcome.as_str(),at,sqlite_revision(next)?,project.to_string(),session.to_string()]).map_err(db_error)?;
            event.work_item_id=current.work_item_id;event.session_id=Some(session);event.branch_id=current.branch_id;event.payload=serde_json::json!({"status":outcome,"closed_claim_ids":claims,"last_checkpoint_id":current.last_checkpoint_id});
            session_at(tx,project,session)
        })
    }
}
