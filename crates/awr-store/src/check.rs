//! Direct-check observations share the event journal, with no execution registry or schema.
use crate::{
    Store, admission, db_error,
    events::event_row,
    query::projection,
    session::{require_active, require_branch, session_at},
};
use awr_core::*;
use rusqlite::{Connection, params};

fn check_session(
    conn: &Connection,
    project: Id,
    key: &str,
    session: Id,
) -> Result<(Projected<WorkItem>, Session)> {
    let work: Projected<WorkItem> = projection(conn, project, EntityKind::WorkItem, key)?;
    let session = session_at(conn, project, session)?;
    require_active(&session)?;
    require_branch(conn, project, session.branch_id)?;
    if session.work_item_id != Some(work.item.meta.id) {
        return Err(Error::InvalidInput(
            "work check requires an active session bound to this work".into(),
        ));
    }
    if work.source.freshness != Freshness::Fresh {
        return Err(Error::SourceStale(
            "refresh the work source before running checks".into(),
        ));
    }
    Ok((work, session))
}

impl Store {
    /// Check the revision and authority before starting any child; baseline remains available.
    pub fn require_check(
        &self,
        project: Id,
        expected: Revision,
        key: &str,
        session: Id,
        phase: CheckPhase,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let actual = tx
            .query_row(
                "SELECT project_revision FROM projects WHERE id=?1",
                [project.to_string()],
                |r| crate::catalog::revision_at(r, 0),
            )
            .map_err(db_error)?;
        if actual != expected {
            return Err(Error::RevisionConflict { expected, actual });
        }
        let (work, session) = check_session(&tx, project, key, session)?;
        if phase != CheckPhase::Baseline {
            admission::require_with(&tx, project, work.item.meta.id, session.branch_id, None)?;
            if matches!(admission::read_policy(&work), admission::PolicyRead::Valid(ref p, _) if p.enforced() && p.governs(&work.item.tags))
            {
                let claims = crate::work::active_claims(
                    &tx,
                    project,
                    work.item.meta.id,
                    session.branch_id,
                    now_millis()?,
                )?;
                if !claims.iter().any(|c| {
                    c.session_id == session.id
                        && c.agent_id == session.agent_id
                        && c.branch_id == session.branch_id
                }) {
                    return Err(Error::Admission { code:"admission_claim_required", diagnostics:serde_json::json!([]), message:"acceptance and mutation checks require this session's active work claim".into() });
                }
            }
        }
        tx.commit().map_err(db_error)
    }

    pub fn record_check(&mut self, project: Id, request: &RecordCheckRequest) -> Result<Event> {
        request.payload.validate()?;
        let (_, event) = self.runtime_transaction_with_event(
            project,
            request.expected_revision,
            EventDraft::new(CHECK_EVENT, "Recorded direct work check"),
            |tx, _, event| {
                let (work, session) = check_session(tx, project, &request.work, request.session)?;
                event.work_item_id = Some(work.item.meta.id);
                event.session_id = Some(session.id);
                event.branch_id = session.branch_id;
                event.payload = serde_json::to_value(&request.payload)?;
                Ok(())
            },
        )?;
        Ok(event)
    }

    /// Exact work and branch history; callers decide which observations are still current.
    pub fn check_events(&self, project: Id, key: &str, branch: Option<Id>) -> Result<Vec<Event>> {
        let work = self.work_identity(project, key)?;
        self.conn.prepare("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type=?4 ORDER BY project_revision,id")
            .map_err(db_error)?.query_map(params![project.to_string(),work.to_string(),branch.map(|id|id.to_string()),CHECK_EVENT],event_row)
            .map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    /// Managed log directories are recognized only through a typed producer event.
    pub fn check_log_observations(&self, project: Id) -> Result<Vec<CheckPayload>> {
        let mut stmt = self.conn.prepare("SELECT payload_json FROM events WHERE project_id=?1 AND event_type=?2 ORDER BY project_revision").map_err(db_error)?;
        let values = stmt
            .query_map(params![project.to_string(), CHECK_EVENT], |r| {
                r.get::<_, String>(0)
            })
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        values
            .into_iter()
            .map(|v| {
                let mut v: serde_json::Value = serde_json::from_str(&v)?;
                v.as_object_mut()
                    .ok_or_else(|| Error::Storage("invalid check observation".into()))?
                    .remove("mcp_operation_id");
                let p: CheckPayload = serde_json::from_value(v)?;
                p.validate()?;
                Ok(p)
            })
            .collect()
    }

    /// Only this producer path can add the event provenance to evidence registration.
    pub fn record_check_evidence(
        &mut self,
        project: Id,
        expected: Revision,
        draft: EvidenceDraft,
        events: &[Id],
    ) -> Result<(Evidence, Event)> {
        if events.is_empty() || events.len() > 64 {
            return Err(Error::InvalidInput(
                "check evidence requires 1..64 producer events".into(),
            ));
        }
        let key = draft
            .work_item_key
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("check evidence requires work".into()))?;
        let work = self.work_item(project, key)?.item;
        for id in events {
            let event = self.event(project, *id)?;
            if event.event_type != CHECK_EVENT
                || event.work_item_id != Some(work.meta.id)
                || event.branch_id != draft.branch_id
                || event.payload["phase"] != "acceptance"
                || event.payload["outcome"] != "passed"
                || event.payload["source_sha"].as_str() != draft.source_sha.as_deref()
            {
                return Err(Error::InvalidInput("check evidence requires passing acceptance events with the same source, work and branch".into()));
            }
        }
        self.record_evidence_with_checks(project, expected, draft, Some(events))
    }

    pub fn evidence_trust(&self, project: Id, evidence: Id) -> Result<EvidenceTrust> {
        self.evidence_by_id(project, evidence)?;
        let produced: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND event_type='evidence.recorded' AND json_extract(payload_json,'$.evidence_id')=?2 AND json_array_length(json_extract(payload_json,'$.check_events'))>0)",params![project.to_string(),evidence.to_string()], |r|r.get(0)).map_err(db_error)?;
        Ok(if produced {
            EvidenceTrust::Recorded
        } else {
            EvidenceTrust::Reported
        })
    }
}
