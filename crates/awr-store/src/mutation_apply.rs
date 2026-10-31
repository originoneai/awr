use crate::{
    Store,
    catalog::{id_at, revision_at},
    db_error,
    mutation::{bound_mutation_target_at, proposal_at, proposal_branch, validate_binding},
    transaction::{optional_id, sqlite_revision},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

const RESOLVED: &str = "SELECT r.id FROM events r WHERE r.project_id=s.project_id AND r.event_type IN ('proposal.applied','proposal.apply_failed','proposal.apply_conflict','work.progressed','work.blocked','work.unblocked','work.cancelled','work.reopened','work.completed','work.draft_activated','work.ordinary_confirmed') AND json_extract(r.payload_json,'$.attempt_event_id')=s.id";
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MutationApplyAttempt> {
    let payload: serde_json::Value =
        serde_json::from_str(&row.get::<_, String>(1)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let parse = |key: &str| {
        serde_json::from_value::<Id>(payload[key].clone()).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })
    };
    Ok(MutationApplyAttempt {
        event_id: id_at(row, 0)?,
        proposal_id: parse("proposal_id")?,
        source_id: parse("source_id")?,
        project_revision: revision_at(row, 2)?,
        plan: serde_json::from_value(payload["write_plan"].clone()).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?,
        resolved_event_id: optional_id(row, 3)?,
    })
}
pub(crate) fn pending_for_source(
    conn: &Connection,
    project: Id,
    source: Id,
) -> Result<Option<MutationApplyAttempt>> {
    conn.query_row(&format!("SELECT s.id,s.payload_json,s.project_revision,NULL FROM events s WHERE s.project_id=?1 AND s.event_type='proposal.apply_started' AND json_extract(s.payload_json,'$.source_id')=?2 AND NOT EXISTS({RESOLVED}) ORDER BY s.project_revision DESC LIMIT 1"),params![project.to_string(),source.to_string()],row).optional().map_err(db_error)
}
pub(crate) fn latest(
    conn: &Connection,
    project: Id,
    proposal: Id,
) -> Result<Option<MutationApplyAttempt>> {
    conn.query_row(&format!("SELECT s.id,s.payload_json,s.project_revision,({RESOLVED} LIMIT 1) FROM events s WHERE s.project_id=?1 AND s.event_type='proposal.apply_started' AND json_extract(s.payload_json,'$.proposal_id')=?2 ORDER BY s.project_revision DESC LIMIT 1"),params![project.to_string(),proposal.to_string()],row).optional().map_err(db_error)
}
fn actor_reason(actor: &str, reason: &str) -> Result<()> {
    if actor.trim().is_empty()
        || actor.len() > 256
        || reason.trim().is_empty()
        || reason.len() > 16384
    {
        return Err(Error::InvalidInput(
            "application requires a bounded actor and reason".into(),
        ));
    }
    Ok(())
}
fn bind(
    draft: &mut EventDraft,
    conn: &Connection,
    project: Id,
    proposal: &MutationProposal,
) -> Result<()> {
    draft.work_item_id = proposal.work_item_id;
    draft.branch_id = proposal_branch(conn, project, proposal)?;
    Ok(())
}
fn open_attempt(
    conn: &Connection,
    project: Id,
    proposal: Id,
    event: Id,
) -> Result<(MutationProposal, MutationApplyAttempt)> {
    let proposal = proposal_at(conn, project, proposal)?;
    if proposal.status != ProposalStatus::Approved {
        return Err(Error::InvalidTransition(
            "only an approved pending application can be resolved".into(),
        ));
    }
    let attempt = latest(conn, project, proposal.id)?
        .filter(|a| a.event_id == event && a.resolved_event_id.is_none())
        .ok_or_else(|| {
            Error::InvalidTransition("application attempt is missing or already resolved".into())
        })?;
    Ok((proposal, attempt))
}
impl Store {
    pub fn source_apply_pending(
        &self,
        project: Id,
        source: Id,
    ) -> Result<Option<MutationApplyAttempt>> {
        self.source(project, source)?;
        pending_for_source(&self.conn, project, source)
    }
    pub fn proposal_apply_attempt(
        &self,
        project: Id,
        proposal: Id,
    ) -> Result<Option<MutationApplyAttempt>> {
        proposal_at(&self.conn, project, proposal)?;
        latest(&self.conn, project, proposal)
    }
    /// Reserve one source for an already staged write. This transaction performs no file I/O.
    pub fn begin_proposal_apply(
        &mut self,
        project: Id,
        expected: Revision,
        id: Id,
        plan: MutationWritePlan,
        actor: &str,
        reason: &str,
    ) -> Result<(MutationApplyAttempt, Event)> {
        plan.validate()?;
        actor_reason(actor, reason)?;
        let (_,event)=self.runtime_transaction_with_event(project,expected,EventDraft::new("proposal.apply_started","Started source mutation application"),|tx,_,event|{
            let proposal=proposal_at(tx,project,id)?;
            if proposal.status!=ProposalStatus::Approved {return Err(Error::InvalidTransition("source application requires an approved proposal".into()));}
            if pending_for_source(tx,project,proposal.source_id)?.is_some() {return Err(Error::MutationConflict("source has an unfinished application; inspect and recover it first".into()));}
            let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND event_type='proposal.apply_started' AND json_extract(payload_json,'$.write_plan.id')=?2)",params![project.to_string(),plan.id.to_string()],|r|r.get(0)).map_err(db_error)?;
            if duplicate {return Err(Error::InvalidInput("write plan ID is already recorded".into()));}
            if plan.before_fingerprint!=proposal.base_fingerprint {return Err(Error::SourceConflict("write plan disagrees with proposal baseline".into()));}
            let patch=proposal.bound_patch()?;
            let target=validate_binding(tx,project,proposal.source_id,&proposal.base_fingerprint,&proposal.mutation_type,&patch)?;
            crate::work_action::validate_action(tx,project,expected,&patch,&target.item,proposal.created_by_session)?;
            tx.execute("UPDATE mutation_proposals SET revision=revision+1 WHERE project_id=?1 AND id=?2",params![project.to_string(),id.to_string()]).map_err(db_error)?;
            bind(event,tx,project,&proposal)?;
            event.payload=json!({"proposal_id":id,"source_id":proposal.source_id,"write_plan":plan,"actor":actor,"reason":reason,"work_action":patch.work_action,"source_write_confirmed":false});
            Ok(())
        })?;
        let attempt = latest(&self.conn, project, id)?
            .ok_or_else(|| Error::Storage("application receipt disappeared".into()))?;
        Ok((attempt, event))
    }
    /// Only a fresh projection of the recorded after fingerprint and exact target facts can
    /// finalize application. The runtime separately verifies actual file bytes before this call.
    pub fn finish_proposal_apply(
        &mut self,
        project: Id,
        expected: Revision,
        id: Id,
        attempt_id: Id,
        actor: &str,
        reason: &str,
    ) -> Result<(MutationProposal, Event)> {
        self.finish_proposal_apply_renewing(project, expected, id, attempt_id, actor, reason, None)
    }
    /// As `finish_proposal_apply`; when a policy asks for it, a progress, block or unblock action extends the live
    /// claims of the session that made it in the same transaction, and the receipt says so.
    pub fn finish_proposal_apply_renewing(
        &mut self,
        project: Id,
        expected: Revision,
        id: Id,
        attempt_id: Id,
        actor: &str,
        reason: &str,
        renewal: Option<ClaimRenewal>,
    ) -> Result<(MutationProposal, Event)> {
        actor_reason(actor, reason)?;
        self.runtime_transaction_with_event(project,expected,EventDraft::new("proposal.applied","Verified source mutation and projection"),|tx,_,event|{
            let (mut proposal,attempt)=open_attempt(tx,project,id,attempt_id)?;
            let patch=proposal.bound_patch()?;
            let target=bound_mutation_target_at(tx,project,&patch,proposal.source_id)?;
            if target.source.freshness!=Freshness::Fresh || target.source.fingerprint!=attempt.plan.after_fingerprint
                || target.source.config!=patch.source_config || mutation_projection_hash(&target.item)? != attempt.plan.target_after_hash {
                return Err(Error::SourceConflict("post-write projection does not match the planned source and target facts".into()));
            }
            if let Some(binding)=patch.work_action.as_ref().and_then(|a|a.completion.as_ref()) {
                let work:WorkItem=serde_json::from_value(target.item.clone())?;
                if work.status!=WorkStatus::Completed {return Err(Error::SourceConflict("completion requires a completed source projection".into()));}
                let branch=proposal_branch(tx,project,&proposal)?;
                crate::work_action::check_dependencies(tx,project,expected,&work,branch,false)?;
                crate::work_action::validate_completion_binding(tx,project,&work,branch,binding)?;
            }
            let meta:ProjectionMeta=serde_json::from_value(target.item)?;
            if meta.source_ref.source_id!=proposal.source_id || meta.source_ref.source_fingerprint!=attempt.plan.after_fingerprint || meta.source_ref.source_revision!=target.source.revision || meta.source_ref.pointer!=patch.target.meta.source_ref.pointer {
                return Err(Error::SourceConflict("post-write target provenance does not match the exact source pointer".into()));
            }
            tx.execute("UPDATE mutation_proposals SET status='applied',revision=revision+1,applied_at=?1 WHERE project_id=?2 AND id=?3",params![now_millis()?,project.to_string(),id.to_string()]).map_err(db_error)?;
            proposal.status=ProposalStatus::Applied; proposal.revision+=1;
            bind(event,tx,project,&proposal)?;
            event.payload=json!({"proposal_id":id,"source_id":proposal.source_id,"attempt_event_id":attempt_id,"write_plan_id":attempt.plan.id,"before_fingerprint":attempt.plan.before_fingerprint,"after_fingerprint":attempt.plan.after_fingerprint,"target_after_hash":attempt.plan.target_after_hash,"source_revision":target.source.revision,"target_revision":meta.revision,"actor":actor,"reason":reason});
            if let Some(host)=&patch.host_edit {
                event.payload["host_edit"]=json!(host);
                if host.action==HostEditAction::ActivateDraft {
                    event.event_type="work.draft_activated".into();
                    event.summary="Explicitly activated source-declared draft".into();
                }
                if host.action==HostEditAction::ConfirmOrdinary {
                    event.event_type="work.ordinary_confirmed".into();
                    event.summary="Recorded ordinary completion; no engineering verification asserted".into();
                }
            }
            if let Some(binding)=patch.work_action {
                event.event_type=binding.action.event_type().into();
                event.summary=format!("Verified source work action {:?}",binding.action);
                event.payload["work_action"]=json!(binding);
                event.payload["action_reason"]=json!(patch.intent);
                event.payload["creating_session_id"]=json!(proposal.created_by_session);
                event.payload["released_claim_ids"]=json!(crate::work_action::release_terminal_claims(tx,project,&proposal)?);
                if let (Some(renewal),Some(session))=(renewal,proposal.created_by_session) {
                    if binding.action.is_activity() {
                        crate::claim_renewal::renew_session_claims(tx,project,session,renewal,&mut event.payload)?;
                    }
                }
            }
            Ok(proposal)
        })
    }
    pub fn fail_proposal_apply(
        &mut self,
        project: Id,
        expected: Revision,
        id: Id,
        attempt_id: Id,
        conflict: bool,
        actor: &str,
        reason: &str,
    ) -> Result<(MutationProposal, Event)> {
        actor_reason(actor, reason)?;
        self.runtime_transaction_with_event(project,expected,EventDraft::new(if conflict {"proposal.apply_conflict"} else {"proposal.apply_failed"},"Stopped source mutation application"),|tx,_,event|{
            let (mut proposal,attempt)=open_attempt(tx,project,id,attempt_id)?;
            proposal.status=if conflict {ProposalStatus::Conflict} else {ProposalStatus::Failed};
            proposal.revision+=1;
            tx.execute("UPDATE mutation_proposals SET status=?1,revision=?2 WHERE project_id=?3 AND id=?4",params![proposal.status.as_str(),sqlite_revision(proposal.revision)?,project.to_string(),id.to_string()]).map_err(db_error)?;
            bind(event,tx,project,&proposal)?;
            event.payload=json!({"proposal_id":id,"source_id":proposal.source_id,"attempt_event_id":attempt_id,"write_plan_id":attempt.plan.id,"actor":actor,"reason":reason});
            Ok(proposal)
        })
    }
}
