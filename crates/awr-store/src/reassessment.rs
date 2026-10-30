//! Reassessment decisions as events, and the readiness diagnostics derived from a work's records.
//!
//! The facts come from what AWR itself recorded for the work: its executions, the blocker its
//! source declares, the waits nobody answered and the latest host observation when that is still
//! current. They are never inferred from titles. A decision is a `work.reassessment` event bound
//! to the facts it answered; it needs no table and no migration. The state is re-derived on every
//! read, so a new failure or an outcome nobody knows makes an earlier decision stale instead of
//! silently covering it, and a trigger that no longer holds asks for nothing. Only the work the
//! facts belong to is ever affected.
use crate::{
    Store,
    admission::{PolicyRead, current_branch, digest, read_policy, reject},
    contract::work_contract,
    db_error,
    events::event_row,
    execution::executions_of,
    query::projection,
    session::{require_active, session_at},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

/// Everything the evaluator reads, gathered from the work's own records.
struct Facts {
    deviations: Vec<Deviation>,
    attempts: Vec<MethodAttempt>,
    stale_observations: Vec<String>,
    /// The latest host observation, when it still describes the work.
    host: Option<ManagementObservation>,
}

fn clip(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() > 160 {
        let cut: String = flat.chars().take(160).collect();
        format!("{cut}...")
    } else {
        flat.to_string()
    }
}

/// The same command in the same directory is one method.
fn method_of(execution: &Execution) -> Result<String> {
    digest(&json!([execution.intent.command, execution.intent.cwd]))
}

fn facts_at(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<Facts> {
    let key = &work.item.meta.external_key;
    let mut deviations = Vec::new();
    let mut attempts = Vec::new();
    let mut newest_record = 0i64;
    for execution in executions_of(conn, project, Some(work.item.meta.id))? {
        if execution.branch_id != branch {
            continue;
        }
        newest_record = newest_record
            .max(execution.registered_at)
            .max(execution.finished_at.unwrap_or(0));
        match execution.state {
            ExecutionState::Succeeded => attempts.push(MethodAttempt {
                method: method_of(&execution)?,
                reference: execution.id.to_string(),
                order: execution.revision,
                failed: false,
            }),
            ExecutionState::Failed => {
                let method = method_of(&execution)?;
                let at = execution.finished_at.unwrap_or(execution.registered_at);
                let how = match (execution.exit_code, execution.signal, &execution.error) {
                    (_, Some(signal), _) => format!("ended by signal {signal}"),
                    (Some(code), _, _) => format!("exited with code {code}"),
                    (None, None, Some(error)) => clip(error),
                    (None, None, None) => "failed".to_string(),
                };
                deviations.push(Deviation {
                    kind: DeviationKind::FailedMethod,
                    reference: execution.id.to_string(),
                    at,
                    basis: "recorded",
                    method: Some(method.clone()),
                    detail: format!(
                        "execution {} ({}) {how}",
                        execution.id,
                        clip(&execution.intent.purpose)
                    ),
                });
                attempts.push(MethodAttempt {
                    method,
                    reference: execution.id.to_string(),
                    order: execution.revision,
                    failed: true,
                });
            }
            ExecutionState::Starting | ExecutionState::Running => {
                // It started and nobody is left to say how it ended.
                let watched = session_at(conn, project, execution.session_id)
                    .map(|s| s.status == "active")
                    .unwrap_or(false);
                if !watched {
                    deviations.push(Deviation {
                        kind: DeviationKind::UnknownOutcome,
                        reference: execution.id.to_string(),
                        at: execution.started_at.unwrap_or(execution.registered_at),
                        basis: "recorded",
                        method: None,
                        detail: format!(
                            "execution {} ({}) started and its session is no longer active",
                            execution.id,
                            clip(&execution.intent.purpose)
                        ),
                    });
                }
            }
            ExecutionState::Registered => {}
        }
    }
    if let Some(blocker) = work
        .item
        .blocker
        .as_deref()
        .filter(|b| !b.trim().is_empty())
    {
        deviations.push(Deviation {
            kind: DeviationKind::ObjectiveBlocker,
            reference: key.clone(),
            at: 0,
            basis: "source_declared",
            method: None,
            detail: clip(blocker),
        });
    }
    let mut waits = conn
        .prepare("SELECT json_extract(e.payload_json,'$.wait.id'), json_extract(e.payload_json,'$.wait.created_at'), json_extract(e.payload_json,'$.wait.question') FROM events e WHERE e.project_id=?1 AND e.work_item_id=?2 AND e.branch_id IS ?3 AND e.event_type='mcp.wait_created' AND NOT EXISTS (SELECT 1 FROM events r WHERE r.project_id=e.project_id AND r.event_type='mcp.wait_replied' AND json_extract(r.payload_json,'$.wait.id')=json_extract(e.payload_json,'$.wait.id')) ORDER BY e.project_revision")
        .map_err(db_error)?;
    let rows = waits
        .query_map(
            params![
                project.to_string(),
                work.item.meta.id.to_string(),
                branch.map(|b| b.to_string())
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(db_error)?;
    for row in rows {
        let (id, at, question) = row.map_err(db_error)?;
        deviations.push(Deviation {
            kind: DeviationKind::ExternalWait,
            reference: id,
            at,
            basis: "recorded",
            method: None,
            detail: format!("waiting for an answer to: {}", clip(&question)),
        });
    }
    // The latest host observation counts only while the contract is the one it described and
    // nothing was recorded after it; otherwise it is reported as stale and triggers nothing.
    let mut stale_observations = Vec::new();
    let mut host = None;
    let latest = conn
        .query_row(
            "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type='management.assessed' ORDER BY project_revision DESC,created_at DESC,id DESC LIMIT 1",
            params![
                project.to_string(),
                work.item.meta.id.to_string(),
                branch.map(|b| b.to_string())
            ],
            event_row,
        )
        .optional()
        .map_err(db_error)?;
    if let Some(event) = latest {
        let observation = serde_json::from_value::<ManagementObservation>(
            event.payload["input"]["observation"].clone(),
        )
        .ok();
        if let Some(observation) = observation {
            let contract = work_contract(conn, project, key)?.fingerprint;
            let current = event.payload["assessment"]["contract_fingerprint"] == json!(contract)
                && observation.observed_at >= newest_record;
            if current && observation.outcome_known == Some(false) {
                deviations.push(Deviation {
                    kind: DeviationKind::UnknownOutcome,
                    reference: event.id.to_string(),
                    at: observation.observed_at,
                    basis: "host_reported",
                    method: None,
                    detail: format!(
                        "the host reported that the outcome of its last action is unknown: {}",
                        clip(&observation.note)
                    ),
                });
            } else if !current && observation.outcome_known == Some(false) {
                stale_observations.push(event.id.to_string());
            }
            if current {
                host = Some(observation);
            }
        }
    }
    deviations.sort_by(|a, b| (a.at, &a.reference).cmp(&(b.at, &b.reference)));
    Ok(Facts {
        deviations,
        attempts,
        stale_observations,
        host,
    })
}

/// What the work has used of its budgets. AWR counts from its own records first: the times the
/// work was reopened, and the time claims on it were held. The host's counters stand in only
/// where AWR recorded nothing of its own.
fn budget_use_at(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    host: Option<&ManagementObservation>,
) -> Result<BudgetUse> {
    let reopened: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE project_id=?1 AND work_item_id=?2 AND event_type='work.reopened'",
            params![project.to_string(), work.item.meta.id.to_string()],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    let now = now_millis()?;
    let mut claims = conn
        .prepare("SELECT status, acquired_at, expires_at, released_at FROM claims WHERE project_id=?1 AND work_item_id=?2")
        .map_err(db_error)?;
    let held_ms: i64 = claims
        .query_map(
            params![project.to_string(), work.item.meta.id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?
        .into_iter()
        .map(|(status, acquired, expires, released)| {
            let end = match (released, status.as_str(), expires) {
                (Some(released), ..) => released,
                (None, "active", Some(expires)) => expires.min(now),
                (None, "active", None) => now,
                (None, _, Some(expires)) => expires,
                (None, _, None) => acquired,
            };
            (end - acquired).max(0)
        })
        .sum();
    let own_minutes = u64::try_from(held_ms / 60_000).unwrap_or(0);
    Ok(BudgetUse::of(
        &effective_budget(work.item.definition.as_ref()),
        count_with_fallback(
            u64::try_from(reopened).unwrap_or(0),
            host.and_then(|o| o.completed_rework_cycles).map(u64::from),
        ),
        count_with_fallback(
            own_minutes,
            host.and_then(|o| o.active_elapsed_ms).map(|ms| ms / 60_000),
        ),
    ))
}

fn supervisors(work: &Projected<WorkItem>) -> Vec<String> {
    match read_policy(work) {
        PolicyRead::Valid(policy, _) => policy.supervisors,
        _ => vec![],
    }
}

fn recorded(event: &Event) -> Result<RecordedReassessment> {
    let broken = || Error::Storage("invalid reassessment event".into());
    Ok(RecordedReassessment {
        event: event.id,
        decision: serde_json::from_value(event.payload["decision"].clone())
            .map_err(|_| broken())?,
        subject: if event.payload["input"]["answers"].is_null() {
            ReassessmentSubject::Facts
        } else {
            serde_json::from_value(event.payload["input"]["answers"].clone())
                .map_err(|_| broken())?
        },
        escalated_to: event.payload["input"]["escalated_to"]
            .as_str()
            .map(String::from),
        bound: serde_json::from_value(event.payload["bound"].clone()).map_err(|_| broken())?,
        decider_agent: event.payload["decider_agent"]
            .as_str()
            .ok_or_else(broken)?
            .to_string(),
        root_cause: serde_json::from_value(event.payload["root_cause"].clone()).ok(),
    })
}

/// The latest decision about facts, or about the budget. A decision recorded before budgets
/// existed says nothing about what it answers and answers facts.
fn latest_reassessment(
    conn: &Connection,
    project: Id,
    work: Id,
    branch: Option<Id>,
    subject: ReassessmentSubject,
) -> Result<Option<RecordedReassessment>> {
    let subject = match subject {
        ReassessmentSubject::Facts => "facts",
        ReassessmentSubject::Budget => "budget",
    };
    let event = conn
        .query_row(
            "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type=?4 AND COALESCE(json_extract(payload_json,'$.input.answers'),'facts')=?5 ORDER BY project_revision DESC,created_at DESC,id DESC LIMIT 1",
            params![
                project.to_string(),
                work.to_string(),
                branch.map(|id| id.to_string()),
                REASSESSMENT_EVENT,
                subject
            ],
            event_row,
        )
        .optional()
        .map_err(db_error)?;
    event.map(|event| recorded(&event)).transpose()
}

/// The reassessment state of a work on a branch.
pub(crate) fn status_at(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<ReassessmentStatus> {
    let facts = facts_at(conn, project, work, branch)?;
    let latest = latest_reassessment(
        conn,
        project,
        work.item.meta.id,
        branch,
        ReassessmentSubject::Facts,
    )?;
    let latest_budget = latest_reassessment(
        conn,
        project,
        work.item.meta.id,
        branch,
        ReassessmentSubject::Budget,
    )?;
    let usage = budget_use_at(conn, project, work, facts.host.as_ref())?;
    Ok(evaluate_reassessment(&ReassessmentInputs {
        work: &work.item.meta.external_key,
        deviations: &facts.deviations,
        attempts: &facts.attempts,
        stale_observations: &facts.stale_observations,
        latest: latest.as_ref(),
        budget: Some(&usage),
        latest_budget: latest_budget.as_ref(),
    }))
}

/// Readiness diagnostics of a governed work whose records call for a second look. Only an
/// enforced policy turns them into blockers, and only for the work the facts belong to.
pub(crate) fn diagnostics_for(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<Vec<(String, String)>> {
    Ok(match status_at(conn, project, work, branch) {
        Ok(status) => status
            .diagnostics
            .into_iter()
            .map(|d| (d.code.to_string(), d.detail))
            .collect(),
        Err(error) => vec![(
            "reassessment_unknown".to_string(),
            format!(
                "the records of the work could not be read ({}); refresh the sources and retry",
                error.code()
            ),
        )],
    })
}

impl Store {
    /// The reassessment state of one work as currently recorded, on one branch.
    pub fn reassessment_status(
        &self,
        project: Id,
        work: &str,
        branch: Option<Id>,
    ) -> Result<ReassessmentStatus> {
        self.project(project)?;
        let item: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, work)?;
        status_at(&self.conn, project, &item, branch)
    }

    /// The block `work show` and `work prepare` report: only for a work in a source that declares
    /// an admission policy, and only once there is something to show.
    pub fn reassessment_block(
        &self,
        project: Id,
        work: &str,
        branch: Option<Id>,
    ) -> Result<Option<ReassessmentStatus>> {
        self.project(project)?;
        let item: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, work)?;
        if matches!(read_policy(&item), PolicyRead::Absent) {
            return Ok(None);
        }
        let status = status_at(&self.conn, project, &item, branch)?;
        let silent = status.state == "clear"
            && status.deviations.is_empty()
            && status.stale_observations.is_empty()
            && status.decision_event.is_none()
            && status
                .budget
                .as_ref()
                .is_none_or(|b| b.state == "within" && b.decision_event.is_none());
        Ok((!silent).then_some(status))
    }

    /// A replay of an earlier request: the event that already carries this request key.
    pub fn reassessment_by_key(&self, project: Id, work: Id, key: &str) -> Result<Option<Event>> {
        self.conn
            .query_row(
                "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND event_type=?3 AND json_extract(payload_json,'$.request_key')=?4 ORDER BY project_revision LIMIT 1",
                params![project.to_string(), work.to_string(), REASSESSMENT_EVENT, key],
                event_row,
            )
            .optional()
            .map_err(db_error)
    }

    /// Record one reassessment event. The facts it answers are read inside the transaction, so
    /// facts that changed since the decider read them, or a stale write, are rejected.
    pub fn record_reassessment(
        &mut self,
        project: Id,
        request: &ReassessmentRequest,
    ) -> Result<(Event, ReassessmentStatus)> {
        request.validate()?;
        let event = EventDraft::new(REASSESSMENT_EVENT, "Recorded reassessment decision");
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
                        "refresh the work source before recording a reassessment".into(),
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
                let facts = facts_at(tx, project, &work, branch)?;
                let budget = request.answers == ReassessmentSubject::Budget;
                let current = if budget {
                    // A spent budget is answered by a choice that deals with it, and a decision
                    // that escalated it is answered by someone else.
                    let usage = budget_use_at(tx, project, &work, facts.host.as_ref())?;
                    let Some(trigger) = usage.trigger() else {
                        return Err(Error::InvalidInput(
                            "the budget of this work is not spent, so there is nothing to decide"
                                .into(),
                        ));
                    };
                    let binding = ReassessmentBinding::of(std::slice::from_ref(&trigger))
                        .expect("a spent budget names what is spent");
                    let earlier = latest_reassessment(
                        tx,
                        project,
                        work.item.meta.id,
                        branch,
                        ReassessmentSubject::Budget,
                    )?;
                    if let Some(escalation) = earlier.filter(|e| {
                        e.decision == ReassessmentChoice::Escalate
                            && e.bound.facts_fingerprint == binding.facts_fingerprint
                    }) {
                        if escalation.decider_agent == decider_agent {
                            return Err(reject(
                                "reassessment_budget_escalated",
                                "the budget was escalated to someone else; the one who escalated cannot answer it",
                            ));
                        }
                        let supervisors = supervisors(&work);
                        if !supervisors.is_empty() && !supervisors.contains(&decider_agent) {
                            return Err(reject(
                                "reassessment_not_authorized",
                                format!(
                                    "{decider_agent} is not among the supervisors the policy authorizes to answer an escalated budget"
                                ),
                            ));
                        }
                    }
                    if request.root_cause.is_some() {
                        return Err(Error::InvalidInput(
                            "a root cause groups failed attempts; a budget decision has none".into(),
                        ));
                    }
                    binding
                } else {
                    let found = triggers(&facts.deviations, &facts.attempts);
                    let Some(current) = ReassessmentBinding::of(&found) else {
                        return Err(Error::InvalidInput(
                            "nothing in the records of this work calls for a reassessment".into(),
                        ));
                    };
                    current
                };
                if request.facts_fingerprint != current.facts_fingerprint {
                    return Err(Error::SourceConflict(
                        "the facts changed; read the current facts before deciding".into(),
                    ));
                }
                if let Some(cause) = &request.root_cause {
                    let known: Vec<&String> = facts
                        .deviations
                        .iter()
                        .map(|d| &d.reference)
                        .chain(facts.attempts.iter().map(|a| &a.reference))
                        .collect();
                    if let Some(foreign) = cause.references.iter().find(|r| !known.contains(r)) {
                        return Err(Error::InvalidInput(format!(
                            "root cause names {foreign}, which is no record of this work"
                        )));
                    }
                }
                event.work_item_id = Some(work.item.meta.id);
                event.session_id = decider_session;
                event.branch_id = branch;
                event.payload = json!({
                    "version": 1,
                    "request_key": request.request_key,
                    "decision": request.decision,
                    "input": request.input(),
                    "bound": current,
                    "judgment": request.judgment,
                    "root_cause": request.root_cause,
                    "decider_agent": decider_agent,
                });
                if serde_json::to_vec(&event.payload)?.len() > 64 * 1024 {
                    return Err(Error::InvalidInput(
                        "reassessment receipt exceeds 64 KiB".into(),
                    ));
                }
                ensure_public_value(&event.payload)?;
                Ok(branch)
            },
        )?;
        let status = self.reassessment_status(project, &request.work, branch)?;
        Ok((event, status))
    }
}
