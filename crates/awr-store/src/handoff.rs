use crate::{
    Store,
    checkpoint::checkpoint_at,
    db_error,
    session::{expires_at, require_active, require_branch, session_at},
    transaction::{insert_event, sqlite_revision},
};
use awr_core::*;
use rusqlite::{OptionalExtension, params};

impl Store {
    /// Closing the sender, releasing/transferring its claim, and both history receipts are atomic.
    /// Handoffs preserve checkpoint ownership; an inherited checkpoint is not fresh recipient context.
    pub fn handoff(
        &mut self,
        project: Id,
        expected: Revision,
        from: Id,
        to: Option<Id>,
        ttl_ms: Option<u64>,
    ) -> Result<(Handoff, Event)> {
        if to.is_none() && ttl_ms.is_some() {
            return Err(Error::InvalidInput(
                "claim TTL requires a receiving session".into(),
            ));
        }
        self.runtime_transaction_with_event(project, expected, EventDraft::new("work.handoff", "Handed off unfinished work"), |tx, next, event| {
            let sender = session_at(tx, project, from)?;
            require_active(&sender)?;
            let work = sender.work_item_id.ok_or_else(|| Error::InvalidInput("handoff requires a work-bound session".into()))?;
            let checkpoint_id = sender.last_checkpoint_id.ok_or_else(|| Error::ContextIncomplete("save a checkpoint with next action and open loops before handoff".into()))?;
            let checkpoint = checkpoint_at(tx, project, checkpoint_id)?;
            if checkpoint.session_id != from {
                return Err(Error::Storage("handoff checkpoint belongs to another session".into()));
            }
            let receiver = to.map(|id| session_at(tx, project, id)).transpose()?;
            if let Some(receiver) = &receiver {
                require_active(receiver)?;
                require_branch(tx, project, receiver.branch_id)?;
                if receiver.id == from || receiver.work_item_id != Some(work) || receiver.branch_id != sender.branch_id {
                    return Err(Error::InvalidInput("receiving session must be different and bound to the same work and branch".into()));
                }
                crate::workstream_runtime::require_current(tx,&sender)?;
                crate::workstream_runtime::require_current(tx,receiver)?;
                crate::workstream_runtime::require_same(tx,project,from,receiver.id)?;
            }
            let at = now_millis()?;
            let override_expiration = expires_at(at, ttl_ms)?;
            // Capture the effective claim before closing the sender. An expired claim confers no ownership.
            let live_expiration: Option<Option<i64>> = tx.query_row("SELECT expires_at FROM claims WHERE project_id=?1 AND session_id=?2 AND status='active' AND released_at IS NULL AND (expires_at IS NULL OR expires_at>?3)",params![project.to_string(),from.to_string(),at],|r|r.get(0)).optional().map_err(db_error)?;
            let closed_claim_ids = tx.prepare("SELECT id FROM claims WHERE project_id=?1 AND session_id=?2 AND status='active' ORDER BY id").map_err(db_error)?.query_map(params![project.to_string(),from.to_string()],|r|crate::catalog::id_at(r,0)).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
            tx.execute("UPDATE claims SET status=CASE WHEN expires_at<=?1 THEN 'expired' ELSE 'released' END,released_at=CASE WHEN expires_at<=?1 THEN NULL ELSE ?1 END,revision=revision+1 WHERE project_id=?2 AND session_id=?3 AND status='active'",params![at,project.to_string(),from.to_string()]).map_err(db_error)?;
            tx.execute("UPDATE sessions SET status='incomplete',ended_at=?1,end_project_revision=?2,revision=revision+1 WHERE project_id=?3 AND id=?4",params![at,sqlite_revision(next)?,project.to_string(),from.to_string()]).map_err(db_error)?;
            let transferred_claim = if let (Some(receiver), Some(previous_expiration)) = (&receiver, live_expiration) {
                // A transferred claim is a new claim: it needs the work's admission to be current.
                crate::admission::require(tx, project, work, receiver.branch_id)?;
                let claim = Claim { id:Id::new(), project_id:project, work_item_id:work, session_id:receiver.id, agent_id:receiver.agent_id.clone(), branch_id:receiver.branch_id, status:"active".into(), acquired_at:at, expires_at:override_expiration.or(previous_expiration), released_at:None, revision:1 };
                tx.execute("INSERT INTO claims(id,project_id,work_item_id,session_id,agent_id,branch_id,status,acquired_at,expires_at,revision) VALUES(?1,?2,?3,?4,?5,?6,'active',?7,?8,1)",params![claim.id.to_string(),project.to_string(),work.to_string(),receiver.id.to_string(),receiver.agent_id,receiver.branch_id.map(|id|id.to_string()),at,claim.expires_at]).map_err(db_error)?;
                Some(claim)
            } else { None };
            event.work_item_id=Some(work); event.session_id=Some(from); event.branch_id=sender.branch_id;
            event.importance="high".into();
            event.payload=serde_json::json!({"from_session_id":from,"to_session_id":to,"checkpoint_id":checkpoint.id,"closed_claim_ids":closed_claim_ids,"transferred_claim_id":transferred_claim.as_ref().map(|c|c.id),"next_action":checkpoint.next_action,"open_loops":checkpoint.open_loops,"context_requires_refresh":true});
            if let Some(receiver) = &receiver {
                // One receipt per session, sharing the same transaction revision.
                insert_event(tx,&Event {id:Id::new(),project_id:project,work_item_id:event.work_item_id,session_id:Some(from),branch_id:event.branch_id,event_type:event.event_type.clone(),importance:event.importance.clone(),summary:event.summary.clone(),payload:event.payload.clone(),project_revision:next,created_at:at})?;
                event.session_id=Some(receiver.id);
                event.event_type="session.handoff_received".into();
                event.summary="Received work handoff; refresh context before continuing".into();
            }
            Ok(Handoff {from_session:session_at(tx,project,from)?,to_session:receiver,checkpoint,closed_claim_ids,transferred_claim})
        })
    }

    pub fn incoming_handoff(&self, project: Id, session: Id) -> Result<Option<Checkpoint>> {
        let receiver = self.session(project, session)?;
        let payload:Option<String> = self.conn.query_row("SELECT payload_json FROM events WHERE project_id=?1 AND session_id=?2 AND event_type='session.handoff_received' ORDER BY project_revision DESC,created_at DESC,id DESC LIMIT 1",params![project.to_string(),session.to_string()],|r|r.get(0)).optional().map_err(db_error)?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        let payload: serde_json::Value = serde_json::from_str(&payload)?;
        let id: Id = serde_json::from_value(payload["checkpoint_id"].clone())?;
        let checkpoint = self.checkpoint(project, id)?;
        let sender = self.session(project, checkpoint.session_id)?;
        crate::workstream_runtime::require_same(&self.conn, project, sender.id, receiver.id)?;
        if sender.id == receiver.id
            || sender.work_item_id != receiver.work_item_id
            || sender.branch_id != receiver.branch_id
        {
            return Err(Error::Storage(
                "handoff checkpoint does not match receiver work and branch".into(),
            ));
        }
        Ok(Some(checkpoint))
    }
}
