//! Finding events, and what the blocking ones hold.
//!
//! A finding is a `work.finding` event on the work it is about, so recording and resolving are
//! events and nothing is rewritten. Its severity is derived when it is recorded from the tier
//! adopted for the criterion it names and the hard rules of the project, and kept with the event,
//! so a later change of tier does not rewrite what a review found. Only an open blocking finding
//! holds anything: the completion of that work. Deferred records are listed and block nothing.
use crate::{
    Store,
    admission::{PolicyRead, current_branch, read_policy, reject},
    db_error,
    definition::work_definition_report,
    events::event_row,
    query::projection,
    session::{require_active, session_at},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

fn recorded_findings(
    conn: &Connection,
    project: Id,
    work: Id,
    branch: Option<Id>,
) -> Result<Vec<RecordedFinding>> {
    let mut query = conn
        .prepare("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type=?4 ORDER BY project_revision")
        .map_err(db_error)?;
    let events = query
        .query_map(
            params![
                project.to_string(),
                work.to_string(),
                branch.map(|id| id.to_string()),
                FINDING_EVENT
            ],
            event_row,
        )
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let broken = || Error::Storage("invalid finding event".into());
    events
        .into_iter()
        .map(|event| {
            Ok(RecordedFinding {
                event: event.id,
                decider_agent: event.payload["decider_agent"]
                    .as_str()
                    .ok_or_else(broken)?
                    .to_string(),
                input: serde_json::from_value(event.payload["input"].clone())
                    .map_err(|_| broken())?,
                assessment: if event.payload["assessment"].is_null() {
                    None
                } else {
                    Some(
                        serde_json::from_value(event.payload["assessment"].clone())
                            .map_err(|_| broken())?,
                    )
                },
            })
        })
        .collect()
}

/// The findings of a work on a branch, folded from their events.
pub(crate) fn findings_at(
    conn: &Connection,
    project: Id,
    work: Id,
    branch: Option<Id>,
) -> Result<Vec<Finding>> {
    Ok(fold_findings(&recorded_findings(
        conn, project, work, branch,
    )?))
}

/// What AWR itself recorded that bears on how often the work was sent back, in journal order:
/// completions, reopenings and the blocking findings of review rounds. Findings and reopenings
/// are counted across branches, since the work they are about is one.
pub(crate) fn rework_records(
    conn: &Connection,
    project: Id,
    work: Id,
) -> Result<Vec<ReworkRecord>> {
    let mut query = conn
        .prepare("SELECT event_type, payload_json FROM events WHERE project_id=?1 AND work_item_id=?2 AND event_type IN ('work.completed','work.reopened','work.finding') ORDER BY project_revision")
        .map_err(db_error)?;
    let rows = query
        .query_map(params![project.to_string(), work.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let mut records = Vec::new();
    for (kind, payload) in rows {
        match kind.as_str() {
            "work.completed" => records.push(ReworkRecord::Completed),
            "work.reopened" => records.push(ReworkRecord::Reopened),
            _ => {
                let payload: serde_json::Value = serde_json::from_str(&payload)
                    .map_err(|_| Error::Storage("invalid finding event".into()))?;
                if payload["input"]["action"] == "record"
                    && payload["assessment"]["severity"] == "blocking"
                    && let Some(round) = payload["input"]["round"].as_u64()
                {
                    records.push(ReworkRecord::BlockingFinding {
                        round: u32::try_from(round).unwrap_or(u32::MAX),
                    });
                }
            }
        }
    }
    Ok(records)
}

impl Store {
    /// The findings of one work as currently recorded, on one branch.
    pub fn finding_status(
        &self,
        project: Id,
        work: &str,
        branch: Option<Id>,
    ) -> Result<FindingStatus> {
        self.project(project)?;
        let item: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, work)?;
        Ok(FindingStatus::of(
            work,
            findings_at(&self.conn, project, item.item.meta.id, branch)?,
        ))
    }

    /// The block `work show` and `work prepare` report: only for a work in a source that declares
    /// an admission policy, and only once a finding was recorded.
    pub fn finding_block(
        &self,
        project: Id,
        work: &str,
        branch: Option<Id>,
    ) -> Result<Option<FindingStatus>> {
        self.project(project)?;
        let item: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, work)?;
        if matches!(read_policy(&item), PolicyRead::Absent) {
            return Ok(None);
        }
        let status = FindingStatus::of(
            work,
            findings_at(&self.conn, project, item.item.meta.id, branch)?,
        );
        Ok((!status.findings.is_empty()).then_some(status))
    }

    /// Completion is refused while a blocking finding of the work is open. Deferred records
    /// never hold it.
    pub fn check_completion_findings(
        &self,
        project: Id,
        work: &str,
        branch: Option<Id>,
    ) -> Result<()> {
        let status = self.finding_status(project, work, branch)?;
        if status.open_blocking.is_empty() {
            return Ok(());
        }
        Err(Error::DependencyBlocked(format!(
            "work has open blocking findings ({}); resolve them before completion",
            status.open_blocking.join(", ")
        )))
    }

    /// A replay of an earlier request: the event that already carries this request key.
    pub fn finding_by_key(&self, project: Id, work: Id, key: &str) -> Result<Option<Event>> {
        self.conn
            .query_row(
                "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND event_type=?3 AND json_extract(payload_json,'$.request_key')=?4 ORDER BY project_revision LIMIT 1",
                params![project.to_string(), work.to_string(), FINDING_EVENT, key],
                event_row,
            )
            .optional()
            .map_err(db_error)
    }

    /// Record a finding, or close one. What the finding is measured against is read inside the
    /// transaction, so a stale write or a criterion or rule that does not exist is refused.
    pub fn record_finding(
        &mut self,
        project: Id,
        request: &FindingRequest,
    ) -> Result<(Event, FindingStatus)> {
        request.validate()?;
        let event = EventDraft::new(FINDING_EVENT, "Recorded work finding");
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
                        "refresh the work source before recording a finding".into(),
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
                let findings = findings_at(tx, project, work.item.meta.id, branch)?;
                let existing = findings.iter().find(|f| f.key == request.finding_key);
                let mut assessment = None;
                match request.action {
                    FindingAction::Record => {
                        if existing.is_some() {
                            return Err(Error::SourceConflict(
                                "a finding with this key was already recorded; name a new finding"
                                    .into(),
                            ));
                        }
                        let tier = match &request.criterion {
                            None => None,
                            Some(criterion) => {
                                if !work.item.acceptance.contains(criterion) {
                                    return Err(Error::InvalidInput(
                                        "the criterion is not one of the acceptance criteria of this work, word for word"
                                            .into(),
                                    ));
                                }
                                let report = work_definition_report(
                                    tx,
                                    project,
                                    &work.item.meta.external_key,
                                )?;
                                // Work without a declared tier owes every criterion.
                                Some(
                                    report
                                        .criterion_tiers
                                        .iter()
                                        .find(|(c, _)| c == criterion)
                                        .map_or(DeliveryTier::Required, |(_, tier)| *tier),
                                )
                            }
                        };
                        let rule_is_hard = match &request.rule {
                            None => None,
                            Some(rule) => {
                                let found: Projected<Rule> =
                                    projection(tx, project, EntityKind::Rule, rule).map_err(
                                        |error| match error {
                                            Error::NotFound(_) => Error::InvalidInput(
                                                "no rule of the project has this key".into(),
                                            ),
                                            other => other,
                                        },
                                    )?;
                                Some(found.item.severity == Some(Severity::Hard))
                            }
                        };
                        assessment = Some(assess_finding(
                            &request.input(),
                            &FindingFacts {
                                criterion_tier: tier,
                                rule_is_hard,
                            },
                        ));
                    }
                    FindingAction::Resolve => {
                        if existing.is_none_or(|f| f.state != "open") {
                            return Err(Error::InvalidInput(
                                "no open finding has this key, so there is nothing to resolve"
                                    .into(),
                            ));
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
                    "assessment": assessment,
                    "decider_agent": decider_agent,
                });
                ensure_public_value(&event.payload)?;
                Ok(branch)
            },
        )?;
        let status = self.finding_status(project, &request.work, branch)?;
        Ok((event, status))
    }
}
