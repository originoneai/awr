use crate::{
    EventReference, ImportantEvent, ProjectionChange, SourceState, Store,
    catalog::{id_at, revision_at},
    checkpoint::{checkpoint_actor, checkpoint_at, validate_draft, write_checkpoint},
    db_error,
    events::event_row,
    session::{require_branch, session_at},
    transaction::{optional_id, sqlite_revision},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceObservation {
    pub event: EventReference,
    pub source_id: Id,
    pub operation: String,
    pub historical_details_known: bool,
    pub before: Option<SourceState>,
    pub after: Option<SourceState>,
    pub changes: Vec<ProjectionChange>,
}

/// Immutable references and structural changes, without arbitrary event or source bodies.
/// Source observations are project-wide; process events belong to this session exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDeltaSnapshot {
    pub schema: u32,
    pub project_id: Id,
    pub session_id: Id,
    pub work_item_id: Option<Id>,
    pub branch_id: Option<Id>,
    pub baseline_checkpoint_id: Option<Id>,
    pub after_revision: Revision,
    pub through_revision: Revision,
    pub session_events: Vec<ImportantEvent>,
    pub source_observations: Vec<SourceObservation>,
    pub observed_changed_entities: Vec<String>,
    pub reported_changed_entities: Vec<String>,
    pub context_hash_origin: String,
    pub digest_origin: String,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointAttempt {
    /// ID of the immutable checkpoint.started event, readable via event show --full.
    pub id: Id,
    pub base_project_revision: Revision,
    pub started_project_revision: Revision,
    pub created_at: i64,
    pub checkpoint_id: Option<Id>,
    pub status: &'static str,
}
#[derive(Debug, Clone, Serialize)]
pub struct CheckpointAttempts {
    pub attempts: Vec<CheckpointAttempt>,
    pub incomplete_count: usize,
    pub limit: usize,
    pub may_have_more: bool,
}

fn snapshot(
    conn: &Connection,
    project: Id,
    session: &Session,
    through: Revision,
    reported: Vec<String>,
) -> Result<SessionDeltaSnapshot> {
    let baseline = session
        .last_checkpoint_id
        .map(|id| checkpoint_at(conn, project, id))
        .transpose()?;
    if baseline
        .as_ref()
        .is_some_and(|c| c.session_id != session.id)
    {
        return Err(Error::Storage(
            "checkpoint baseline belongs to another session".into(),
        ));
    }
    let after = baseline
        .as_ref()
        .map(|c| c.project_revision)
        .unwrap_or(session.start_project_revision);
    if after > through {
        return Err(Error::InvalidInput(
            "checkpoint baseline is newer than its snapshot".into(),
        ));
    }
    let session_events=conn.prepare("SELECT id,project_revision,created_at,event_type,importance,substr(summary,1,240),work_item_id,session_id,branch_id,length(summary)>240
        FROM events WHERE project_id=?1 AND session_id=?2 AND project_revision>?3 AND project_revision<=?4
        AND event_type NOT LIKE 'source.%' AND event_type NOT LIKE 'checkpoint.%' ORDER BY project_revision,created_at,id").map_err(db_error)?
        .query_map(params![project.to_string(),session.id.to_string(),sqlite_revision(after)?,sqlite_revision(through)?],|r| {
            let mut summary=r.get::<_,String>(5)?;if r.get::<_,bool>(9)? {summary.push('…');}
            Ok(ImportantEvent {event:EventReference{id:id_at(r,0)?,project_revision:revision_at(r,1)?,created_at:r.get(2)?},event_type:r.get(3)?,importance:r.get(4)?,summary,work_item_id:optional_id(r,6)?,session_id:optional_id(r,7)?,branch_id:optional_id(r,8)?})
        }).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
    let mut source_observations = Vec::new();
    let mut observed = BTreeSet::new();
    let mut statement=conn.prepare("SELECT id,project_revision,created_at,event_type,json_extract(payload_json,'$.source_id'),
        json_extract(payload_json,'$.change_schema'),json_extract(payload_json,'$.before'),json_extract(payload_json,'$.after'),json_extract(payload_json,'$.changes')
        FROM events WHERE project_id=?1 AND project_revision>?2 AND project_revision<=?3 AND event_type LIKE 'source.%' ORDER BY project_revision,created_at,id").map_err(db_error)?;
    let rows = statement
        .query_map(
            params![
                project.to_string(),
                sqlite_revision(after)?,
                sqlite_revision(through)?
            ],
            |r| {
                Ok((
                    EventReference {
                        id: id_at(r, 0)?,
                        project_revision: revision_at(r, 1)?,
                        created_at: r.get(2)?,
                    },
                    r.get::<_, String>(3)?,
                    id_at(r, 4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .map_err(db_error)?;
    for row in rows {
        let (event, operation, source_id, schema, before, after, changes) =
            row.map_err(db_error)?;
        let known = schema == Some(1);
        let (before, after, changes) = if known {
            (
                before
                    .map(|v| serde_json::from_str::<SourceState>(&v))
                    .transpose()?,
                Some(serde_json::from_str::<SourceState>(&after.ok_or_else(
                    || Error::Storage("source receipt has no after state".into()),
                )?)?),
                serde_json::from_str::<Vec<ProjectionChange>>(
                    &changes
                        .ok_or_else(|| Error::Storage("source receipt has no changes".into()))?,
                )?,
            )
        } else {
            (None, None, Vec::new())
        };
        for change in &changes {
            observed.insert(format!("{}:{}", change.kind, change.id));
        }
        source_observations.push(SourceObservation {
            event,
            source_id,
            operation,
            historical_details_known: known,
            before,
            after,
            changes,
        });
    }
    // Only domain-created receipts can assert runtime entity changes; arbitrary payload labels cannot.
    let runtime_changes=conn.prepare("SELECT event_type,CASE event_type WHEN 'artifact.recorded' THEN json_extract(payload_json,'$.artifact_id')
        WHEN 'evidence.recorded' THEN json_extract(payload_json,'$.evidence_id') END FROM events
        WHERE project_id=?1 AND session_id=?2 AND project_revision>?3 AND project_revision<=?4 AND event_type IN ('artifact.recorded','evidence.recorded')").map_err(db_error)?
        .query_map(params![project.to_string(),session.id.to_string(),sqlite_revision(after)?,sqlite_revision(through)?],|r|Ok((r.get::<_,String>(0)?,id_at(r,1)?))).map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
    for (kind, id) in runtime_changes {
        observed.insert(format!("{}:{id}", kind.split('.').next().unwrap()));
    }
    Ok(SessionDeltaSnapshot {schema:1,project_id:project,session_id:session.id,work_item_id:session.work_item_id,branch_id:session.branch_id,
        baseline_checkpoint_id:baseline.map(|c|c.id),after_revision:after,through_revision:through,session_events,source_observations,
        observed_changed_entities:observed.into_iter().collect(),reported_changed_entities:reported,
        context_hash_origin:"caller_supplied_last_used_context; hash syntax checked, content not independently verified".into(),
        digest_origin:"caller_supplied_progress_summary".into(),
        scope:"Exact session process summaries after its previous successful checkpoint or session start; checkpoint bookkeeping excluded. All project source observations in the same revision window are observations, not claims that this agent authored them. Full event/source/artifact bodies are excluded; legacy source receipts retain unknown historical details.".into()})
}

impl Store {
    /// Count and provenance fields only; saved histories are an explicit detail read.
    pub fn checkpoint_save_metadata(
        &self,
        project: Id,
        checkpoint: Id,
    ) -> Result<serde_json::Value> {
        checkpoint_at(&self.conn, project, checkpoint)?;
        self.conn.query_row("SELECT id,json_extract(payload_json,'$.attempt_id'),
            json_extract(payload_json,'$.session_delta.schema'),json_extract(payload_json,'$.session_delta.after_revision'),
            json_extract(payload_json,'$.session_delta.through_revision'),json_array_length(payload_json,'$.session_delta.session_events'),
            json_array_length(payload_json,'$.session_delta.source_observations'),json_array_length(payload_json,'$.session_delta.observed_changed_entities'),
            json_array_length(payload_json,'$.session_delta.reported_changed_entities'),
            length(CAST(json_extract(payload_json,'$.session_delta') AS BLOB)),json_extract(payload_json,'$.actor') FROM events
            WHERE project_id=?1 AND event_type='checkpoint.created' AND json_extract(payload_json,'$.checkpoint_id')=?2",
            params![project.to_string(),checkpoint.to_string()],|r|Ok(serde_json::json!({"event_id":id_at(r,0)?,"attempt_id":optional_id(r,1)?,
                "delta_recorded":r.get::<_,Option<i64>>(2)?==Some(1),"after_revision":r.get::<_,Option<i64>>(3)?,"through_revision":r.get::<_,Option<i64>>(4)?,
                "session_event_count":r.get::<_,Option<i64>>(5)?,"source_observation_count":r.get::<_,Option<i64>>(6)?,
                "observed_changed_entity_count":r.get::<_,Option<i64>>(7)?,"reported_changed_entity_count":r.get::<_,Option<i64>>(8)?,"delta_bytes":r.get::<_,Option<i64>>(9)?,
                "actor":r.get::<_,Option<String>>(10)?.map(|s|serde_json::from_str::<serde_json::Value>(&s)).transpose().map_err(|e|rusqlite::Error::FromSqlConversionFailure(10,rusqlite::types::Type::Text,Box::new(e)))?.unwrap_or_else(||serde_json::json!({"agent_id":null,"origin":"not_recorded","identity_verified":false})),
                "context_hash_verified":false,"source_work_updated":false,"warning":CHECKPOINT_LIMITATION})))
            .optional().map_err(db_error)?.ok_or_else(||Error::Storage("checkpoint has no creation receipt".into()))
    }

    /// The durable start receipt does not create or activate a checkpoint.
    pub fn begin_checkpoint_save(
        &mut self,
        project: Id,
        expected: Revision,
        session: Id,
        draft: CheckpointDraft,
    ) -> Result<Event> {
        self.begin_checkpoint_save_as(project, expected, session, draft, None)
    }

    /// Caller identity is a declaration, never inferred from the session label.
    /// An explicitly different agent must resume into its own session.
    pub fn begin_checkpoint_save_as(
        &mut self,
        project: Id,
        expected: Revision,
        session: Id,
        draft: CheckpointDraft,
        agent: Option<&str>,
    ) -> Result<Event> {
        validate_draft(&draft)?;
        ensure_public_data(&agent)?;
        if agent.is_some_and(|s| {
            s.trim().is_empty() || s.len() > 256 || s.chars().any(char::is_control)
        }) {
            return Err(Error::InvalidInput("checkpoint agent must be nonblank and at most 256 bytes without control characters".into()));
        }
        self.runtime_transaction_with_event(project,expected,EventDraft::new("checkpoint.started","Started checkpoint save; completion receipt required"),|tx,_,event| {
            let current=session_at(tx,project,session)?;
            if current.status!="active" {return Err(Error::InvalidTransition(format!("session {session} is {}",current.status)));}
            if agent.is_some_and(|agent|agent!=current.agent_id) {
                return Err(Error::RuleViolation("checkpoint caller differs from the session agent; use session resume to continue in a new session with your own agent/provider/model, then save there".into()));
            }
            require_branch(tx,project,current.branch_id)?;
            event.session_id=Some(session);event.work_item_id=current.work_item_id;event.branch_id=current.branch_id;
            event.payload=serde_json::json!({"save_schema":1,"base_project_revision":expected,"draft":draft,"actor":checkpoint_actor(agent)});
            Ok(())
        }).map(|(_,event)|event)
    }

    /// Snapshot, checkpoint row, latest pointer and completion receipt commit together.
    /// An intervening mutation invalidates this attempt; a fresh save is required.
    pub fn finish_checkpoint_save(
        &mut self,
        project: Id,
        expected: Revision,
        attempt: Id,
    ) -> Result<(Checkpoint, Event)> {
        self.finish_checkpoint_save_renewing(project, expected, attempt, None)
    }

    /// As `finish_checkpoint_save`; when a policy asks for it, the live claims of the session are extended in the
    /// same transaction and the receipt says so.
    pub fn finish_checkpoint_save_renewing(
        &mut self,
        project: Id,
        expected: Revision,
        attempt: Id,
        renewal: Option<ClaimRenewal>,
    ) -> Result<(Checkpoint, Event)> {
        self.runtime_transaction_with_event(project,expected,EventDraft::new("checkpoint.created","Saved session checkpoint and observed delta"),|tx,_,event| {
            let started=tx.query_row("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at
                FROM events WHERE project_id=?1 AND id=?2 AND event_type='checkpoint.started'",params![project.to_string(),attempt.to_string()],event_row).optional().map_err(db_error)?
                .ok_or_else(||Error::NotFound(format!("checkpoint save attempt {attempt}")))?;
            let abandoned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND event_type='checkpoint.abandoned' AND json_extract(payload_json,'$.attempt_id')=?2)",params![project.to_string(),attempt.to_string()],|r|r.get(0)).map_err(db_error)?;
            if abandoned {return Err(Error::InvalidTransition("checkpoint attempt was abandoned".into()));}
            if started.project_revision!=expected {return Err(Error::RevisionConflict{expected:started.project_revision,actual:expected});}
            let completed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND event_type='checkpoint.created' AND json_extract(payload_json,'$.attempt_id')=?2)",params![project.to_string(),attempt.to_string()],|r|r.get(0)).map_err(db_error)?;
            if completed {return Err(Error::InvalidTransition("checkpoint attempt already completed".into()));}
            let session=started.session_id.ok_or_else(||Error::Storage("checkpoint attempt has no session".into()))?;
            let current=session_at(tx,project,session)?;
            if current.status!="active" {return Err(Error::InvalidTransition(format!("session {session} is {}",current.status)));}
            require_branch(tx,project,current.branch_id)?;
            let base=started.payload["base_project_revision"].as_u64().ok_or_else(||Error::Storage("checkpoint attempt has no base revision".into()))?;
            if started.payload["save_schema"]!=1 || base.checked_add(1)!=Some(expected) {return Err(Error::Storage("invalid checkpoint attempt version or base revision".into()));}
            let mut draft:CheckpointDraft=serde_json::from_value(started.payload["draft"].clone())?;
            validate_draft(&draft)?;
            let delta=snapshot(tx,project,&current,base,draft.changed_entities.clone())?;
            draft.changed_entities=delta.observed_changed_entities.clone();
            let checkpoint=write_checkpoint(tx,project,&current,base,draft,event)?;
            event.payload["actor"]=started.payload.get("actor").cloned().unwrap_or_else(||serde_json::json!({"agent_id":null,"origin":"not_recorded","identity_verified":false}));
            event.payload["attempt_id"]=serde_json::json!(attempt);
            event.payload["session_delta"]=serde_json::to_value(delta)?;
            if let Some(renewal)=renewal {
                crate::claim_renewal::renew_session_claims(tx,project,session,renewal,&mut event.payload)?;
            }
            Ok(checkpoint)
        })
    }

    pub fn checkpoint_saved_delta(
        &self,
        project: Id,
        checkpoint: Id,
    ) -> Result<Option<SessionDeltaSnapshot>> {
        checkpoint_at(&self.conn, project, checkpoint)?;
        let value=self.conn.query_row("SELECT json_extract(payload_json,'$.session_delta') FROM events
            WHERE project_id=?1 AND event_type='checkpoint.created' AND json_extract(payload_json,'$.checkpoint_id')=?2",
            params![project.to_string(),checkpoint.to_string()],|r|r.get::<_,Option<String>>(0)).optional().map_err(db_error)?.flatten();
        value
            .map(|v| serde_json::from_str(&v))
            .transpose()
            .map_err(Error::from)
    }

    pub fn checkpoint_attempts(
        &self,
        project: Id,
        session: Id,
        limit: usize,
    ) -> Result<CheckpointAttempts> {
        self.session(project, session)?;
        if limit == 0 || limit > 100 {
            return Err(Error::InvalidInput(
                "checkpoint attempt limit must be 1..100".into(),
            ));
        }
        let mut attempts=self.conn.prepare("SELECT e.id,e.project_revision,e.created_at,
            (SELECT json_extract(c.payload_json,'$.checkpoint_id') FROM events c WHERE c.project_id=e.project_id AND c.event_type='checkpoint.created' AND json_extract(c.payload_json,'$.attempt_id')=e.id LIMIT 1),
            EXISTS(SELECT 1 FROM events a WHERE a.project_id=e.project_id AND a.event_type='checkpoint.abandoned' AND json_extract(a.payload_json,'$.attempt_id')=e.id)
            FROM events e WHERE e.project_id=?1 AND e.session_id=?2 AND e.event_type='checkpoint.started' ORDER BY e.project_revision DESC,e.id DESC LIMIT ?3").map_err(db_error)?
            .query_map(params![project.to_string(),session.to_string(),(limit+1) as i64],|r| {
                let revision=revision_at(r,1)?;let checkpoint_id=optional_id(r,3)?;let abandoned:bool=r.get(4)?;
                Ok(CheckpointAttempt {id:id_at(r,0)?,base_project_revision:revision.saturating_sub(1),started_project_revision:revision,created_at:r.get(2)?,checkpoint_id,status:if checkpoint_id.is_some(){"completed"}else if abandoned{"abandoned"}else{"pending_or_interrupted"}})
            }).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        let incomplete_count=self.conn.query_row("SELECT count(*) FROM events e WHERE e.project_id=?1 AND e.session_id=?2 AND e.event_type='checkpoint.started'
            AND NOT EXISTS(SELECT 1 FROM events c WHERE c.project_id=e.project_id AND c.event_type='checkpoint.created' AND json_extract(c.payload_json,'$.attempt_id')=e.id)
            AND NOT EXISTS(SELECT 1 FROM events a WHERE a.project_id=e.project_id AND a.event_type='checkpoint.abandoned' AND json_extract(a.payload_json,'$.attempt_id')=e.id)",params![project.to_string(),session.to_string()],|r|r.get::<_,i64>(0)).map_err(db_error)? as usize;
        let may_have_more = attempts.len() > limit;
        attempts.truncate(limit);
        Ok(CheckpointAttempts {
            attempts,
            incomplete_count,
            limit,
            may_have_more,
        })
    }
}
