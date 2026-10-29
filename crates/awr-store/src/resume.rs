use crate::{
    Store,
    catalog::id_at,
    checkpoint::checkpoint_at,
    db_error,
    query::projection,
    session::{SESSION_COLUMNS, acquire, expires_at, require_branch, session_at, session_row},
    transaction::{insert_event, optional_id, sqlite_revision},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};

fn recovery_checkpoint(
    conn: &Connection,
    project: Id,
    from: &Session,
) -> Result<Option<Checkpoint>> {
    let own = from
        .last_checkpoint_id
        .map(|id| checkpoint_at(conn, project, id))
        .transpose()?;
    if own.as_ref().is_some_and(|cp| cp.session_id != from.id) {
        return Err(Error::Storage("checkpoint ownership mismatch".into()));
    }
    let inherited:Option<String>=conn.query_row("SELECT json_extract(payload_json,'$.checkpoint_id') FROM events WHERE project_id=?1 AND session_id=?2 AND event_type='session.handoff_received' ORDER BY project_revision DESC,created_at DESC,id DESC LIMIT 1",params![project.to_string(),from.id.to_string()],|r|r.get(0)).optional().map_err(db_error)?.flatten();
    let inherited = inherited
        .map(|id| {
            id.parse::<Id>()
                .map_err(|e| Error::Storage(e.to_string()))
                .and_then(|id| checkpoint_at(conn, project, id))
        })
        .transpose()?;
    if let Some(cp) = &inherited {
        let owner = session_at(conn, project, cp.session_id)?;
        crate::workstream_runtime::require_same(conn, project, from.id, owner.id)?;
        if owner.id == from.id
            || owner.work_item_id != from.work_item_id
            || owner.branch_id != from.branch_id
        {
            return Err(Error::Storage(
                "inherited checkpoint work/branch mismatch".into(),
            ));
        }
    }
    if let Some(cp) = own
        .into_iter()
        .chain(inherited)
        .max_by_key(|cp| (cp.project_revision, cp.created_at, cp.id))
    {
        return Ok(Some(cp));
    }
    let Some(work) = from.work_item_id else {
        return Ok(None);
    };
    let id=conn.query_row("SELECT c.id FROM checkpoints c JOIN sessions s ON s.id=c.session_id JOIN session_workstreams b ON b.project_id=s.project_id AND b.session_id=s.id JOIN session_workstreams origin ON origin.project_id=s.project_id AND origin.session_id=?4 AND origin.workstream_id=b.workstream_id AND origin.ownership_revision=b.ownership_revision WHERE s.project_id=?1 AND s.work_item_id=?2 AND s.branch_id IS ?3 AND s.status!='active' ORDER BY c.project_revision DESC,c.created_at DESC,c.id DESC LIMIT 1",params![project.to_string(),work.to_string(),from.branch_id.map(|id|id.to_string()),from.id.to_string()],|r|id_at(r,0)).optional().map_err(db_error)?;
    id.map(|id| checkpoint_at(conn, project, id)).transpose()
}

fn successor_id(conn: &Connection, project: Id, from: Id) -> Result<Option<Id>> {
    conn.query_row("SELECT session_id FROM events WHERE project_id=?1 AND event_type='session.resumed' AND json_extract(payload_json,'$.from_session_id')=?2 ORDER BY project_revision DESC,id DESC LIMIT 1",params![project.to_string(),from.to_string()],|r|id_at(r,0)).optional().map_err(db_error)
}

fn recovery_revision(conn: &Connection, project: Id, session: &Session) -> Result<Revision> {
    let recorded: Option<i64> = conn.query_row(
        "SELECT json_extract(payload_json,'$.recovery_after_revision') FROM events WHERE project_id=?1 AND session_id=?2 AND event_type='session.resumed' ORDER BY project_revision DESC,id DESC LIMIT 1",
        params![project.to_string(),session.id.to_string()], |r| r.get(0),
    ).optional().map_err(db_error)?.flatten();
    match recorded {
        Some(revision) if revision >= 0 && revision as u64 <= session.start_project_revision => {
            Ok(revision as u64)
        }
        Some(_) => Err(Error::Storage("invalid session recovery revision".into())),
        None => Ok(session.start_project_revision),
    }
}

impl Store {
    /// The two newest eligible sessions suffice to detect competing active candidates.
    /// Ended sessions are eligible only when a work key is explicitly selected.
    pub fn resume_candidates(
        &self,
        project: Id,
        work: Option<&str>,
        branch: Option<Id>,
    ) -> Result<Vec<Session>> {
        self.resume_candidates_scoped(project, work, branch, false)
    }
    pub(crate) fn resume_candidates_scoped(
        &self,
        project: Id,
        work: Option<&str>,
        branch: Option<Id>,
        scoped: bool,
    ) -> Result<Vec<Session>> {
        let columns = SESSION_COLUMNS
            .split(',')
            .map(|c| format!("s.{c}"))
            .collect::<Vec<_>>()
            .join(",");
        let visibility = if scoped {
            crate::scoped_read::visible("sessions", "s.id")
        } else {
            "1".into()
        };
        self.conn.prepare(&format!("SELECT {columns} FROM sessions s JOIN work_items w ON s.work_item_id=w.id AND s.project_id=w.project_id
            JOIN session_workstreams b ON b.project_id=s.project_id AND b.session_id=s.id
            JOIN workstream_ownership o ON o.project_id=b.project_id AND o.work_item_id=b.work_item_id AND o.workstream_id=b.workstream_id AND o.revision=b.ownership_revision
            JOIN sources src ON w.source_id=src.id AND w.project_id=src.project_id WHERE s.project_id=?1 AND s.branch_id IS ?2
            AND {visibility} AND w.active=1 AND src.active=1 AND w.status NOT IN ('completed','cancelled','unknown')
            AND (s.status IN ('active','incomplete','interrupted') OR (?3 IS NOT NULL AND s.status='ended'))
            AND (?3 IS NULL OR w.external_key=?3)
            AND NOT EXISTS(SELECT 1 FROM events e WHERE e.project_id=s.project_id AND e.event_type='session.resumed' AND json_extract(e.payload_json,'$.from_session_id')=s.id)
            ORDER BY (s.status='active') DESC,coalesce(s.end_project_revision,s.start_project_revision) DESC,s.started_at DESC,s.id DESC LIMIT 2")).map_err(db_error)?
            .query_map(params![project.to_string(),branch.map(|id|id.to_string()),work],session_row).map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }
    pub fn recovery_checkpoint(&self, project: Id, from: Id) -> Result<Option<Checkpoint>> {
        recovery_checkpoint(&self.conn, project, &self.session(project, from)?)
    }
    /// A checkpointless successor retains the recovery window without changing its real start.
    pub fn session_recovery_revision(&self, project: Id, session: Id) -> Result<Revision> {
        recovery_revision(&self.conn, project, &self.session(project, session)?)
    }
    pub fn resumed_successor(&self, project: Id, from: Id) -> Result<Option<Session>> {
        self.session(project, from)?;
        successor_id(&self.conn, project, from)?
            .map(|id| self.session(project, id))
            .transpose()
    }
    /// After source/context preflight, atomically create a successor, inherit recovery references,
    /// close an active predecessor and transfer/reacquire its requested claim.
    pub fn resume_session(
        &mut self,
        project: Id,
        expected: Revision,
        draft: SessionResumeDraft,
    ) -> Result<(SessionResumed, Event)> {
        self.resume_session_inner(project, expected, draft, None)
    }
    pub fn resume_bound_session(
        &mut self,
        project: Id,
        expected: Revision,
        draft: SessionResumeDraft,
        binding: McpSessionBinding,
    ) -> Result<(SessionResumed, Event)> {
        binding.validate()?;
        self.resume_session_inner(project, expected, draft, Some(binding))
    }
    fn resume_session_inner(
        &mut self,
        project: Id,
        expected: Revision,
        draft: SessionResumeDraft,
        binding: Option<McpSessionBinding>,
    ) -> Result<(SessionResumed, Event)> {
        if [&draft.agent_id, &draft.provider, &draft.model]
            .iter()
            .any(|s| s.trim().is_empty())
            || !is_sha256_hash(&draft.prepared_context_hash)
        {
            return Err(Error::InvalidInput(
                "resume needs agent, provider, model and prepared context hash".into(),
            ));
        }
        if draft.claim != ResumeClaim::Acquire && draft.claim_ttl_ms.is_some() {
            return Err(Error::InvalidInput(
                "claim TTL requires explicit acquisition".into(),
            ));
        }
        expires_at(now_millis()?, draft.claim_ttl_ms)?;
        self.runtime_transaction_with_event(project,expected,EventDraft::new("session.resumed","Resumed work in a new session; compile current execution context"),|tx,next,event| {
            let from=session_at(tx,project,draft.from_session_id)?;
            let from_scope=crate::workstream_runtime::require_current(tx,&from)?;
            let inherited_binding=crate::mcp::session_binding(tx,project,from.id)?;
            let binding=binding.or(inherited_binding.clone());
            if let Some(binding)=&binding {
                if inherited_binding.as_ref().is_some_and(|old|old.client!=binding.client) {
                    return Err(Error::RuleViolation("MCP session belongs to another client".into()));
                }
                if crate::mcp::binding_session(tx,project,binding)?.is_some_and(|session|session.id!=from.id) {
                    return Err(Error::SourceConflict("target conversation belongs to another session".into()));
                }
            }
            if !["active","incomplete","interrupted","ended"].contains(&from.status.as_str()) {return Err(Error::InvalidTransition(format!("session {} is {}",from.id,from.status)));}
            if let Some(id)=successor_id(tx,project,from.id)? {return Err(Error::InvalidTransition(format!("session {} already resumed as {id}; inspect that successor",from.id)));}
            let branch=tx.query_row("SELECT current_branch_id FROM projects WHERE id=?1",[project.to_string()],|r|optional_id(r,0)).map_err(db_error)?;
            if branch!=from.branch_id {return Err(Error::InvalidInput("resume requires the current project branch".into()));}
            require_branch(tx,project,branch)?;
            let work_id=from.work_item_id.ok_or_else(||Error::InvalidInput("resume requires a work-bound session".into()))?;
            let key=tx.query_row("SELECT external_key FROM work_items WHERE project_id=?1 AND id=?2 AND active=1",params![project.to_string(),work_id.to_string()],|r|r.get::<_,String>(0)).optional().map_err(db_error)?.ok_or_else(||Error::NotFound(format!("active work item {work_id}")))?;
            let work=projection::<WorkItem>(tx,project,EntityKind::WorkItem,&key)?;
            if work.item.meta.id!=work_id {return Err(Error::SourceConflict("work identity changed".into()));}
            if work.source.freshness!=Freshness::Fresh {return Err(Error::SourceStale("resume work is not fresh".into()));}
            if matches!(work.item.status,WorkStatus::Completed|WorkStatus::Cancelled|WorkStatus::Unknown) {return Err(Error::InvalidTransition("resume requires known nonterminal work".into()));}
            let checkpoint=recovery_checkpoint(tx,project,&from)?;
            if checkpoint.as_ref().map(|cp|cp.id)!=draft.checkpoint_id {return Err(Error::MutationConflict("recovery checkpoint differs from preflight".into()));}
            let recovery_after_revision=match &checkpoint {Some(cp)=>cp.project_revision,None=>recovery_revision(tx,project,&from)?};
            let at=now_millis()?;
            let live_expiration:Option<Option<i64>>=tx.query_row("SELECT expires_at FROM claims WHERE project_id=?1 AND session_id=?2 AND status='active' AND released_at IS NULL AND (expires_at IS NULL OR expires_at>?3)",params![project.to_string(),from.id.to_string(),at],|r|r.get(0)).optional().map_err(db_error)?;
            let closed_claim_ids=tx.prepare("SELECT id FROM claims WHERE project_id=?1 AND session_id=?2 AND status='active' ORDER BY id").map_err(db_error)?.query_map(params![project.to_string(),from.id.to_string()],|r|id_at(r,0)).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
            tx.execute("UPDATE claims SET status=CASE WHEN expires_at<=?1 THEN 'expired' ELSE 'released' END,released_at=CASE WHEN expires_at<=?1 THEN NULL ELSE ?1 END,revision=revision+1 WHERE project_id=?2 AND session_id=?3 AND status='active'",params![at,project.to_string(),from.id.to_string()]).map_err(db_error)?;
            if from.status=="active" {tx.execute("UPDATE sessions SET status='interrupted',ended_at=?1,end_project_revision=?2,revision=revision+1 WHERE project_id=?3 AND id=?4",params![at,sqlite_revision(next)?,project.to_string(),from.id.to_string()]).map_err(db_error)?;}
            let session=Session {id:Id::new(),project_id:project,work_item_id:Some(work_id),branch_id:branch,agent_id:draft.agent_id,provider:draft.provider,model:draft.model,status:"active".into(),started_at:at,ended_at:None,start_project_revision:expected,end_project_revision:None,last_checkpoint_id:None,revision:1};
            tx.execute("INSERT INTO sessions(id,project_id,work_item_id,branch_id,agent_id,provider,model,status,started_at,start_project_revision,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,'active',?8,?9,1)",params![session.id.to_string(),project.to_string(),work_id.to_string(),branch.map(|id|id.to_string()),session.agent_id,session.provider,session.model,at,sqlite_revision(expected)?]).map_err(db_error)?;
            let workstream_binding=crate::workstream_runtime::bind(tx,&session,from_scope.workstream_id,None)?;
            crate::workstream_runtime::require_same(tx,project,from.id,session.id)?;
            let claim=match (draft.claim,live_expiration) {
                (ResumeClaim::Inherit,Some(expiration))=>{
                    // An inherited claim is a new claim: it needs the work's admission to be current.
                    crate::admission::require(tx,project,work_id,branch)?;
                    let claim=Claim{id:Id::new(),project_id:project,work_item_id:work_id,session_id:session.id,agent_id:session.agent_id.clone(),branch_id:branch,status:"active".into(),acquired_at:at,expires_at:expiration,released_at:None,revision:1};
                    tx.execute("INSERT INTO claims(id,project_id,work_item_id,session_id,agent_id,branch_id,status,acquired_at,expires_at,revision) VALUES(?1,?2,?3,?4,?5,?6,'active',?7,?8,1)",params![claim.id.to_string(),project.to_string(),work_id.to_string(),session.id.to_string(),session.agent_id,branch.map(|id|id.to_string()),at,expiration]).map_err(db_error)?;
                    Some(claim)
                },
                (ResumeClaim::Acquire,_)=>Some(acquire(tx,&session,draft.claim_ttl_ms,expected,event)?),
                _=>None,
            };
            event.session_id=Some(session.id);event.work_item_id=Some(work_id);event.branch_id=branch;event.importance="high".into();
            let expired_claim_ids=event.payload.get("expired_claim_ids").cloned().unwrap_or_else(||serde_json::json!([]));
            event.payload=serde_json::json!({"workstream_binding":workstream_binding,"from_session_id":from.id,"to_session_id":session.id,"checkpoint_id":checkpoint.as_ref().map(|c|c.id),"recovery_after_revision":recovery_after_revision,"prepared_context_hash":draft.prepared_context_hash,"prepared_project_revision":expected,"claim_mode":draft.claim,"claim_id":claim.as_ref().map(|c|c.id),"closed_claim_ids":closed_claim_ids,"expired_claim_ids":expired_claim_ids,"context_requires_refresh":true});
            if let Some(binding)=binding { event.payload["mcp_binding"]=serde_json::to_value(binding)?; }
            insert_event(tx,&Event{id:Id::new(),project_id:project,session_id:Some(from.id),work_item_id:Some(work_id),branch_id:branch,event_type:"session.resumed_from".into(),importance:"high".into(),summary:"Work continued in a successor session".into(),payload:event.payload.clone(),project_revision:next,created_at:at})?;
            if checkpoint.is_some() {insert_event(tx,&Event{id:Id::new(),project_id:project,session_id:Some(session.id),work_item_id:Some(work_id),branch_id:branch,event_type:"session.handoff_received".into(),importance:"high".into(),summary:"Inherited checkpoint during session resume".into(),payload:event.payload.clone(),project_revision:next,created_at:at})?;}
            Ok(SessionResumed {from_session:session_at(tx,project,from.id)?,session,checkpoint,claim,closed_claim_ids})
        })
    }
}
