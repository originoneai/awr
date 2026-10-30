//! Gap events, and the judgments an open gap puts in question.
//!
//! A gap is a `work.gap` event on the work it belongs to; opening, resolving and withdrawing are
//! events, so nothing is ever rewritten and every failure stays on record. The state of a gap is
//! folded from them on every read, and the judgments an open gap puts in question are derived
//! from its route: only the work's own admission or the containment of its children, never
//! another work and never a task. A gap that needs a run is closed only by the run, which the
//! store establishes from the execution journal, not from any repair of the plan.
use crate::{
    Store,
    admission::{PolicyRead, current_branch, read_policy, reject},
    db_error,
    events::event_row,
    execution::key_at,
    query::projection,
    session::{require_active, session_at},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

fn recorded_gaps(
    conn: &Connection,
    project: Id,
    work: Id,
    branch: Option<Id>,
) -> Result<Vec<RecordedGap>> {
    let mut query = conn
        .prepare("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type=?4 ORDER BY project_revision")
        .map_err(db_error)?;
    let events = query
        .query_map(
            params![
                project.to_string(),
                work.to_string(),
                branch.map(|id| id.to_string()),
                GAP_EVENT
            ],
            event_row,
        )
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let broken = || Error::Storage("invalid gap event".into());
    events
        .into_iter()
        .map(|event| {
            Ok(RecordedGap {
                event: event.id,
                decider_agent: event.payload["decider_agent"]
                    .as_str()
                    .ok_or_else(broken)?
                    .to_string(),
                input: serde_json::from_value(event.payload["input"].clone())
                    .map_err(|_| broken())?,
            })
        })
        .collect()
}

/// The gaps of a work on a branch, folded from their events.
pub(crate) fn gaps_at(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<Vec<Gap>> {
    Ok(fold_gaps(&recorded_gaps(
        conn,
        project,
        work.item.meta.id,
        branch,
    )?))
}

fn detail(judgment: &str, suspension: &Suspension) -> String {
    format!(
        "{judgment} is put in question by the open gap(s) {}; resolve or withdraw them, and decide again if the work changed",
        suspension.gaps.join(", ")
    )
}

/// The judgment of the work itself that its own open gaps put in question.
pub(crate) fn admission_suspension(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<Option<(String, String)>> {
    let gaps = gaps_at(conn, project, work, branch)?;
    Ok(suspensions(&gaps)
        .iter()
        .find(|s| s.judgment == GapJudgment::Admission)
        .map(|s| {
            (
                "admission_suspended".to_string(),
                detail("the admission of this work", s),
            )
        }))
}

/// The containment of a parent's children, when the parent's own open gaps put it in question.
pub(crate) fn containment_suspension(
    conn: &Connection,
    project: Id,
    parent: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<Option<(String, String)>> {
    let gaps = gaps_at(conn, project, parent, branch)?;
    Ok(suspensions(&gaps)
        .iter()
        .find(|s| s.judgment == GapJudgment::Containment)
        .map(|s| {
            (
                "containment_suspended".to_string(),
                detail(
                    &format!(
                        "the containment of the children of {}",
                        parent.item.meta.external_key
                    ),
                    s,
                ),
            )
        }))
}

/// Whether `operation_key` is the experiment of an open gap of the work: the one execution that
/// an open gap must never hold back, since it is how the gap gets closed.
pub(crate) fn is_open_experiment(
    conn: &Connection,
    project: Id,
    work: Id,
    branch: Option<Id>,
    operation_key: &str,
) -> Result<bool> {
    Ok(fold_gaps(&recorded_gaps(conn, project, work, branch)?)
        .iter()
        .any(|g| g.state == "open" && g.experiment.as_deref() == Some(operation_key)))
}

/// Did an execution with the experiment's key end after the gap was opened? A run counts
/// whether it succeeded or not.
fn experiment_ran(conn: &Connection, project: Id, work: Id, gap: &Gap) -> Result<bool> {
    let Some(key) = gap.experiment.as_deref() else {
        return Ok(false);
    };
    let Some(execution) = key_at(conn, project, key)? else {
        return Ok(false);
    };
    if execution.work_item_id != work || !execution.state.terminal() {
        return Ok(false);
    }
    let revision_of = |sql: &str, value: String| -> Result<Option<i64>> {
        conn.query_row(sql, params![project.to_string(), value], |r| r.get(0))
            .optional()
            .map_err(db_error)
    };
    let opened = revision_of(
        "SELECT project_revision FROM events WHERE project_id=?1 AND id=?2",
        gap.opened_event.to_string(),
    )?;
    let registered = revision_of(
        "SELECT project_revision FROM events WHERE project_id=?1 AND event_type='execution.registered' AND json_extract(payload_json,'$.execution.id')=?2",
        execution.id.to_string(),
    )?;
    Ok(matches!((opened, registered), (Some(opened), Some(registered)) if registered > opened))
}

fn supervisors(work: &Projected<WorkItem>) -> Vec<String> {
    match read_policy(work) {
        PolicyRead::Valid(policy, _) => policy.supervisors,
        _ => vec![],
    }
}

impl Store {
    /// The gaps of one work as currently recorded, on one branch.
    pub fn gap_status(&self, project: Id, work: &str, branch: Option<Id>) -> Result<GapStatus> {
        self.project(project)?;
        let item: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, work)?;
        Ok(GapStatus::of(
            work,
            gaps_at(&self.conn, project, &item, branch)?,
        ))
    }

    /// The block `work show` and `work prepare` report: only for a work in a source that
    /// declares an admission policy, and only once a gap was ever recorded.
    pub fn gap_block(
        &self,
        project: Id,
        work: &str,
        branch: Option<Id>,
    ) -> Result<Option<GapStatus>> {
        self.project(project)?;
        let item: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, work)?;
        if matches!(read_policy(&item), PolicyRead::Absent) {
            return Ok(None);
        }
        let status = GapStatus::of(work, gaps_at(&self.conn, project, &item, branch)?);
        Ok((!status.gaps.is_empty()).then_some(status))
    }

    /// A replay of an earlier request: the event that already carries this request key.
    pub fn gap_by_key(&self, project: Id, work: Id, key: &str) -> Result<Option<Event>> {
        self.conn
            .query_row(
                "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND event_type=?3 AND json_extract(payload_json,'$.request_key')=?4 ORDER BY project_revision LIMIT 1",
                params![project.to_string(), work.to_string(), GAP_EVENT, key],
                event_row,
            )
            .optional()
            .map_err(db_error)
    }

    /// Record one gap event. What it is checked against is read inside the transaction, so a
    /// stale write, a gap that is no longer open or an experiment that did not run is refused.
    pub fn record_gap(&mut self, project: Id, request: &GapRequest) -> Result<(Event, GapStatus)> {
        request.validate()?;
        let event = EventDraft::new(GAP_EVENT, "Recorded work gap");
        let (branch, event) = self.runtime_transaction_with_event(
            project,
            request.expected_revision,
            event,
            |tx, _next, event| {
                let session = request
                    .session
                    .map(|id| session_at(tx, project, id))
                    .transpose()?;
                if let Some(session) = &session {
                    require_active(session)?;
                }
                let decider_session = session.as_ref().map(|s| s.id);
                let decider_agent = session
                    .as_ref()
                    .map(|s| s.agent_id.clone())
                    .or_else(|| request.agent.clone())
                    .unwrap_or_default();
                let work: Projected<WorkItem> =
                    projection(tx, project, EntityKind::WorkItem, &request.work)?;
                if work.source.freshness != Freshness::Fresh {
                    return Err(Error::SourceStale(
                        "refresh the work source before recording a gap".into(),
                    ));
                }
                match read_policy(&work) {
                    PolicyRead::Valid(policy, _) if policy.governs(&work.item.tags) => {}
                    PolicyRead::Invalid => {
                        return Err(reject(
                            "admission_policy_invalid",
                            "the admission_policy option of the owning source cannot be used",
                        ));
                    }
                    _ => {
                        return Err(reject(
                            "admission_not_governed",
                            "the work is not governed by an admission policy",
                        ));
                    }
                }
                let branch = match &session {
                    Some(session) => session.branch_id,
                    None => current_branch(tx, project)?,
                };
                let gaps = gaps_at(tx, project, &work, branch)?;
                let existing = gaps.iter().find(|g| g.key == request.gap_key);
                match request.action {
                    GapAction::Open => {
                        if existing.is_some() {
                            return Err(Error::SourceConflict(
                                "a gap with this key was already recorded; name a new gap"
                                    .into(),
                            ));
                        }
                        if request.resolution == Some(GapResolution::AuthorizedChoice)
                            && request.authority.is_empty()
                            && supervisors(&work).is_empty()
                        {
                            return Err(reject(
                                "gap_authority_unknown",
                                "an authorized choice names its authority, or the policy lists supervisors",
                            ));
                        }
                    }
                    GapAction::Resolve | GapAction::Withdraw => {
                        let gap = existing.filter(|g| g.state == "open").ok_or_else(|| {
                            Error::InvalidInput(
                                "no open gap has this key, so there is nothing to close".into(),
                            )
                        })?;
                        let supervisors = supervisors(&work);
                        let facts = ClosingFacts {
                            experiment_ran: experiment_ran(
                                tx,
                                project,
                                work.item.meta.id,
                                gap,
                            )?,
                            decider: &decider_agent,
                            supervisors: &supervisors,
                        };
                        if let Err((code, why)) = check_closing(gap, request, &facts) {
                            return Err(reject(code, why));
                        }
                    }
                }
                event.work_item_id = Some(work.item.meta.id);
                event.session_id = decider_session;
                event.branch_id = branch;
                event.payload = json!({
                    "version": 1,
                    "request_key": request.request_key,
                    "action": request.action,
                    "input": request.input(),
                    "decider_agent": decider_agent,
                });
                ensure_public_value(&event.payload)?;
                Ok(branch)
            },
        )?;
        let status = self.gap_status(project, &request.work, branch)?;
        Ok((event, status))
    }
}
