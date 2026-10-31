//! Reconciliation of runtime claims with a source that says their work is finished.
//!
//! A claim is runtime state; the status of a work is a source fact. When the source says a work is
//! completed or cancelled while a session still holds an active claim on it, the two disagree and the
//! source is right: the claim is released and, when nothing else keeps the session going, the session
//! is closed as `interrupted`, never as a normal completion (evidence belongs to the work, not to the
//! reconciliation). The reconciliation only acts on facts it can prove from a fresh projection, and it
//! leaves alone whatever may still be at work: a checkpoint save in flight, a change waiting to be
//! applied, a supervised execution that has not ended, and claims on a branch other than the current
//! one. What it leaves alone is reported, so the doctor can say why it stays.
use crate::{
    Store,
    catalog::id_at,
    db_error,
    execution::executions_of,
    transaction::{optional_id, sqlite_revision},
};
use awr_core::{Error, EventDraft, Freshness, Id, Result, Revision, WorkStatus, now_millis};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The reason carried by the events a reconciliation writes.
pub const SOURCE_TERMINAL: &str = "source_terminal";

/// An object the reconciliation left alone, and why.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReconcileSkip {
    pub object_kind: &'static str,
    pub object_id: Id,
    pub work_item_key: String,
    pub reason: String,
}

/// What a reconciliation did and what it left alone.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct FinishedWorkReconcile {
    pub released_claim_ids: Vec<Id>,
    pub interrupted_session_ids: Vec<Id>,
    pub skipped: Vec<ReconcileSkip>,
}

impl FinishedWorkReconcile {
    /// True when there was nothing to do and nothing to report.
    pub fn is_empty(&self) -> bool {
        self.released_claim_ids.is_empty()
            && self.interrupted_session_ids.is_empty()
            && self.skipped.is_empty()
    }
}

struct OpenClaim {
    id: Id,
    session_id: Id,
    branch_id: Option<Id>,
}

impl Store {
    /// The works that hold an active claim although the projection does not call them finished. Take
    /// it before a source refresh; after the refresh, `reconcile_finished_work` looks only at these,
    /// so a claim that already stood on a finished work before the refresh is never swept up here.
    pub fn works_with_open_claims(&self, project: Id) -> Result<BTreeSet<Id>> {
        self.conn
            .prepare(
                "SELECT DISTINCT c.work_item_id FROM claims c
                 JOIN work_items w ON w.project_id=c.project_id AND w.id=c.work_item_id
                 WHERE c.project_id=?1 AND c.status='active' AND w.active=1
                   AND w.status NOT IN ('completed','cancelled') ORDER BY c.work_item_id",
            )
            .map_err(db_error)?
            .query_map([project.to_string()], |row| id_at(row, 0))
            .map_err(db_error)?
            .collect::<rusqlite::Result<BTreeSet<_>>>()
            .map_err(db_error)
    }

    /// Release the claims on the watched works that the projection now calls completed or cancelled,
    /// and close the sessions they leave without a claim, each under the live project revision.
    /// A failure or a busy database skips that object and says so; it never fails the caller.
    pub fn reconcile_finished_work(
        &mut self,
        project: Id,
        watched: &BTreeSet<Id>,
    ) -> Result<FinishedWorkReconcile> {
        let mut report = FinishedWorkReconcile::default();
        let current_branch = self.project(project)?.current_branch_id;
        for work_id in watched {
            let work = match self.work_item_by_id(project, *work_id) {
                Ok(work) => work,
                Err(Error::NotFound(_)) => continue,
                Err(error) => return Err(error),
            };
            if !matches!(
                work.item.status,
                WorkStatus::Completed | WorkStatus::Cancelled
            ) || work.source.freshness != Freshness::Fresh
            {
                continue;
            }
            let key = work.item.meta.external_key.clone();
            let observed = (
                work.item.meta.source_ref.source_revision,
                work.item.meta.source_ref.source_fingerprint.clone(),
            );
            let mut by_session: BTreeMap<Id, Vec<OpenClaim>> = BTreeMap::new();
            for claim in self.open_claims_on(project, *work_id)? {
                by_session.entry(claim.session_id).or_default().push(claim);
            }
            for (session, claims) in by_session {
                if let Some(claim) = claims
                    .iter()
                    .find(|c| c.branch_id.is_some() && c.branch_id != current_branch)
                {
                    report.skip("claim", claim.id, &key, "claim_on_another_branch");
                    continue;
                }
                if let Some(reason) = self.in_flight(project, session, *work_id)? {
                    report.skip("session", session, &key, reason);
                    continue;
                }
                let ends = self.session_ends_with(project, session, *work_id)?;
                let outcome = if ends {
                    self.with_live_revision(project, |store, revision| {
                        store.interrupt_for_finished_work(
                            project, revision, session, *work_id, &observed, None, false,
                        )
                    })
                    .map(|(claims, session)| (claims, vec![session]))
                } else {
                    let mut released = Vec::new();
                    let mut failure = None;
                    for claim in &claims {
                        match self.with_live_revision(project, |store, revision| {
                            store.release_for_finished_work(
                                project, revision, claim.id, &observed, None,
                            )
                        }) {
                            Ok(id) => released.push(id),
                            Err(error) => {
                                failure = Some(error);
                                break;
                            }
                        }
                    }
                    match failure {
                        None => Ok((released, vec![])),
                        Some(error) => Err(error),
                    }
                };
                match outcome {
                    Ok((claim_ids, sessions)) => {
                        report.released_claim_ids.extend(claim_ids);
                        report.interrupted_session_ids.extend(sessions);
                    }
                    Err(error) => report.skip(
                        "session",
                        session,
                        &key,
                        match error {
                            Error::RevisionConflict { .. } => "runtime_changed_meanwhile",
                            Error::MutationConflict(_) => "claims_changed_meanwhile",
                            _ => "could_not_reconcile",
                        },
                    ),
                }
            }
        }
        Ok(report)
    }

    fn open_claims_on(&self, project: Id, work: Id) -> Result<Vec<OpenClaim>> {
        self.conn
            .prepare(
                "SELECT id,session_id,branch_id FROM claims
                 WHERE project_id=?1 AND work_item_id=?2 AND status='active' ORDER BY id",
            )
            .map_err(db_error)?
            .query_map(params![project.to_string(), work.to_string()], |row| {
                Ok(OpenClaim {
                    id: id_at(row, 0)?,
                    session_id: id_at(row, 1)?,
                    branch_id: optional_id(row, 2)?,
                })
            })
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)
    }

    /// Something of this session may still be at work: say what, or `None` when nothing is.
    fn in_flight(&self, project: Id, session: Id, work: Id) -> Result<Option<&'static str>> {
        let checkpoint: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM events started
                   WHERE started.project_id=?1 AND started.session_id=?2 AND started.event_type='checkpoint.started'
                     AND NOT EXISTS (SELECT 1 FROM events finished WHERE finished.project_id=started.project_id
                         AND finished.event_type='checkpoint.created' AND json_extract(finished.payload_json,'$.attempt_id')=started.id)
                     AND NOT EXISTS (SELECT 1 FROM events abandoned WHERE abandoned.project_id=started.project_id
                         AND abandoned.event_type='checkpoint.abandoned' AND json_extract(abandoned.payload_json,'$.attempt_id')=started.id))",
                params![project.to_string(), session.to_string()],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if checkpoint {
            return Ok(Some("checkpoint_save_in_flight"));
        }
        let change: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mutation_proposals WHERE project_id=?1
                   AND (work_item_id=?2 OR created_by_session=?3) AND status IN ('draft','ready','approved'))",
                params![project.to_string(), work.to_string(), session.to_string()],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if change {
            return Ok(Some("change_waiting_to_be_applied"));
        }
        if executions_of(&self.conn, project, Some(work))?
            .iter()
            .any(|execution| execution.session_id == session && !execution.state.terminal())
        {
            return Ok(Some("execution_not_ended"));
        }
        Ok(None)
    }

    /// Whether the session is bound to this work and holds no active claim on any other work.
    fn session_ends_with(&self, project: Id, session: Id, work: Id) -> Result<bool> {
        let (bound, others): (Option<String>, i64) = self
            .conn
            .query_row(
                "SELECT s.work_item_id,
                        (SELECT COUNT(*) FROM claims c WHERE c.project_id=s.project_id AND c.session_id=s.id
                           AND c.status='active' AND c.work_item_id!=?3)
                 FROM sessions s WHERE s.project_id=?1 AND s.id=?2 AND s.status='active'",
                params![project.to_string(), session.to_string(), work.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db_error)?
            .unwrap_or((None, 1));
        Ok(bound.as_deref() == Some(work.to_string().as_str()) && others == 0)
    }

    /// Run a step under the live project revision, asking again when another writer was first.
    fn with_live_revision<T>(
        &mut self,
        project: Id,
        mut step: impl FnMut(&mut Store, Revision) -> Result<T>,
    ) -> Result<T> {
        let mut last = None;
        for _ in 0..3 {
            let revision = self.project(project)?.project_revision;
            match step(self, revision) {
                Err(error @ Error::RevisionConflict { .. }) => last = Some(error),
                other => return other,
            }
        }
        Err(last.expect("three attempts were made"))
    }

    fn interrupt_for_finished_work(
        &mut self,
        project: Id,
        expected: Revision,
        session: Id,
        work: Id,
        observed: &(Revision, String),
        note: Option<&str>,
        idle_ok: bool,
    ) -> Result<(Vec<Id>, Id)> {
        let (claims, event) = self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new(
                "session.interrupted",
                "Closed a session whose work the source calls finished",
            ),
            |tx, next, event| {
                let at = now_millis()?;
                let rows = tx
                    .prepare(
                        "SELECT id,expires_at FROM claims WHERE project_id=?1 AND session_id=?2
                           AND work_item_id=?3 AND status='active' ORDER BY id",
                    )
                    .map_err(db_error)?
                    .query_map(
                        params![project.to_string(), session.to_string(), work.to_string()],
                        |row| Ok((id_at(row, 0)?, row.get::<_, Option<i64>>(1)?)),
                    )
                    .map_err(db_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(db_error)?;
                if rows.is_empty() && !idle_ok {
                    return Err(Error::MutationConflict(format!(
                        "the claims of session {session} changed during reconciliation"
                    )));
                }
                let (mut released, mut expired) = (Vec::new(), Vec::new());
                for (id, expires) in &rows {
                    if expires.is_some_and(|expires| expires <= at) {
                        expired.push(*id);
                    } else {
                        released.push(*id);
                    }
                }
                tx.execute(
                    "UPDATE claims SET status=CASE WHEN expires_at IS NOT NULL AND expires_at<=?1 THEN 'expired' ELSE 'released' END,
                       released_at=CASE WHEN expires_at IS NOT NULL AND expires_at<=?1 THEN NULL ELSE ?1 END,revision=revision+1
                     WHERE project_id=?2 AND session_id=?3 AND work_item_id=?4 AND status='active'",
                    params![at, project.to_string(), session.to_string(), work.to_string()],
                )
                .map_err(db_error)?;
                let last_checkpoint: Option<String> = tx
                    .query_row(
                        "SELECT last_checkpoint_id FROM sessions WHERE project_id=?1 AND id=?2",
                        params![project.to_string(), session.to_string()],
                        |row| row.get(0),
                    )
                    .map_err(db_error)?;
                let updated = tx
                    .execute(
                        "UPDATE sessions SET status='interrupted',ended_at=?1,end_project_revision=?2,revision=revision+1
                         WHERE project_id=?3 AND id=?4 AND status='active'",
                        params![at, sqlite_revision(next)?, project.to_string(), session.to_string()],
                    )
                    .map_err(db_error)?;
                if updated != 1 {
                    return Err(Error::MutationConflict(format!(
                        "session {session} changed during reconciliation"
                    )));
                }
                event.work_item_id = Some(work);
                event.session_id = Some(session);
                event.payload = serde_json::json!({
                    "session_id": session,
                    "status": "interrupted",
                    "released_claim_ids": released,
                    "expired_claim_ids": expired,
                    "last_checkpoint_id": last_checkpoint,
                    "reconciled_at": at,
                    "reason": SOURCE_TERMINAL,
                    "source_revision": observed.0,
                    "source_fingerprint": observed.1,
                });
                if let Some(note) = note {
                    event.payload["note"] = serde_json::json!(note);
                }
                Ok(rows.into_iter().map(|(id, _)| id).collect::<Vec<_>>())
            },
        )?;
        let _ = event;
        Ok((claims, session))
    }

    fn release_for_finished_work(
        &mut self,
        project: Id,
        expected: Revision,
        claim: Id,
        observed: &(Revision, String),
        note: Option<&str>,
    ) -> Result<Id> {
        self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new(
                "claim.released",
                "Released a claim on work the source calls finished",
            ),
            |tx, _next, event| {
                let at = now_millis()?;
                let (session, work, agent, expires): (String, String, String, Option<i64>) = tx
                    .query_row(
                        "SELECT session_id,work_item_id,agent_id,expires_at FROM claims
                         WHERE project_id=?1 AND id=?2 AND status='active'",
                        params![project.to_string(), claim.to_string()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()
                    .map_err(db_error)?
                    .ok_or_else(|| {
                        Error::MutationConflict(format!(
                            "claim {claim} changed during reconciliation"
                        ))
                    })?;
                let lapsed = expires.is_some_and(|expires| expires <= at);
                tx.execute(
                    "UPDATE claims SET status=?1,released_at=?2,revision=revision+1
                     WHERE project_id=?3 AND id=?4 AND status='active'",
                    params![
                        if lapsed { "expired" } else { "released" },
                        if lapsed { None } else { Some(at) },
                        project.to_string(),
                        claim.to_string()
                    ],
                )
                .map_err(db_error)?;
                event.event_type = if lapsed {
                    "claim.expired"
                } else {
                    "claim.released"
                }
                .into();
                event.work_item_id = work.parse().ok();
                event.session_id = session.parse().ok();
                event.payload = serde_json::json!({
                    "claim_id": claim,
                    "agent_id": agent,
                    "session_id": session,
                    "reconciled_at": at,
                    "reason": SOURCE_TERMINAL,
                    "source_revision": observed.0,
                    "source_fingerprint": observed.1,
                });
                if let Some(note) = note {
                    event.payload["note"] = serde_json::json!(note);
                }
                Ok(claim)
            },
        )
        .map(|(claim, _)| claim)
    }
}

impl FinishedWorkReconcile {
    fn skip(&mut self, kind: &'static str, id: Id, key: &str, reason: impl Into<String>) {
        self.skipped.push(ReconcileSkip {
            object_kind: kind,
            object_id: id,
            work_item_key: key.to_string(),
            reason: reason.into(),
        });
    }
}

/// A session the explicit clean-up would close, with the claims on the finished work it would release.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlannedSession {
    pub session_id: Id,
    pub agent_id: String,
    pub work_item_id: Id,
    pub work_item_key: String,
    pub work_status: String,
    pub claim_ids: Vec<Id>,
    pub source_revision: Revision,
    pub source_fingerprint: String,
}

/// A claim on finished work that the explicit clean-up would release while its session goes on.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlannedClaim {
    pub claim_id: Id,
    pub session_id: Id,
    pub agent_id: String,
    pub work_item_id: Id,
    pub work_item_key: String,
    pub source_revision: Revision,
    pub source_fingerprint: String,
}

/// What the explicit clean-up of the backlog would do: the sessions still active on finished work and the
/// claims still held on it, each with the source revision and fingerprint it rests on, and what it would
/// leave alone. Reading it changes nothing; the fingerprint names it, so applying it needs the same plan.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct FinishedWorkPlan {
    pub sessions: Vec<PlannedSession>,
    pub claims: Vec<PlannedClaim>,
    pub skipped: Vec<ReconcileSkip>,
}

impl FinishedWorkPlan {
    fn skip(&mut self, kind: &'static str, id: Id, key: &str, reason: impl Into<String>) {
        self.skipped.push(ReconcileSkip {
            object_kind: kind,
            object_id: id,
            work_item_key: key.to_string(),
            reason: reason.into(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.claims.is_empty()
    }

    /// A name for exactly this plan.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        format!("sha256:{:x}", Sha256::digest(bytes))
    }
}

impl Store {
    /// The backlog: every active session bound to a completed or cancelled work (with or without a claim) and
    /// every active claim on one, judged on the recorded projection when its source is fresh. It does not
    /// refresh the sources and it writes nothing.
    pub fn plan_finished_work(&self, project: Id) -> Result<FinishedWorkPlan> {
        let current_branch = self.project(project)?.current_branch_id;
        let finished = self
            .conn
            .prepare(
                "SELECT id FROM work_items WHERE project_id=?1 AND active=1
                   AND status IN ('completed','cancelled') ORDER BY id",
            )
            .map_err(db_error)?
            .query_map([project.to_string()], |row| id_at(row, 0))
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        let mut plan = FinishedWorkPlan::default();
        for work_id in finished {
            let work = self.work_item_by_id(project, work_id)?;
            let key = work.item.meta.external_key.clone();
            if work.source.freshness != Freshness::Fresh {
                plan.skipped.push(ReconcileSkip {
                    object_kind: "work",
                    object_id: work_id,
                    work_item_key: key,
                    reason: "source_not_fresh".into(),
                });
                continue;
            }
            let status = serde_json::to_value(work.item.status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            let observed = (
                work.item.meta.source_ref.source_revision,
                work.item.meta.source_ref.source_fingerprint.clone(),
            );
            let mut claims: BTreeMap<Id, Vec<OpenClaim>> = BTreeMap::new();
            for claim in self.open_claims_on(project, work_id)? {
                claims.entry(claim.session_id).or_default().push(claim);
            }
            let sessions = self
                .conn
                .prepare(
                    "SELECT s.id,s.agent_id,s.branch_id FROM sessions s
                     WHERE s.project_id=?1 AND s.status='active' AND (s.work_item_id=?2 OR EXISTS(
                       SELECT 1 FROM claims c WHERE c.project_id=s.project_id AND c.session_id=s.id
                         AND c.work_item_id=?2 AND c.status='active')) ORDER BY s.id",
                )
                .map_err(db_error)?
                .query_map(params![project.to_string(), work_id.to_string()], |row| {
                    Ok((
                        id_at(row, 0)?,
                        row.get::<_, String>(1)?,
                        optional_id(row, 2)?,
                    ))
                })
                .map_err(db_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db_error)?;
            for (session, agent, branch) in sessions {
                let held = claims.remove(&session).unwrap_or_default();
                if branch.is_some() && branch != current_branch {
                    plan.skip("session", session, &key, "session_on_another_branch");
                    continue;
                }
                if let Some(claim) = held
                    .iter()
                    .find(|c| c.branch_id.is_some() && c.branch_id != current_branch)
                {
                    plan.skip("claim", claim.id, &key, "claim_on_another_branch");
                    continue;
                }
                if let Some(reason) = self.in_flight(project, session, work_id)? {
                    plan.skip("session", session, &key, reason);
                    continue;
                }
                if self.session_ends_with(project, session, work_id)? {
                    plan.sessions.push(PlannedSession {
                        session_id: session,
                        agent_id: agent,
                        work_item_id: work_id,
                        work_item_key: key.clone(),
                        work_status: status.clone(),
                        claim_ids: held.iter().map(|c| c.id).collect(),
                        source_revision: observed.0,
                        source_fingerprint: observed.1.clone(),
                    });
                } else {
                    for claim in held {
                        plan.claims.push(PlannedClaim {
                            claim_id: claim.id,
                            session_id: session,
                            agent_id: agent.clone(),
                            work_item_id: work_id,
                            work_item_key: key.clone(),
                            source_revision: observed.0,
                            source_fingerprint: observed.1.clone(),
                        });
                    }
                }
            }
            // Claims whose session is not active any more belong to nobody who could release them.
            for (session, held) in claims {
                for claim in held {
                    plan.claims.push(PlannedClaim {
                        claim_id: claim.id,
                        session_id: session,
                        agent_id: String::new(),
                        work_item_id: work_id,
                        work_item_key: key.clone(),
                        source_revision: observed.0,
                        source_fingerprint: observed.1.clone(),
                    });
                }
            }
        }
        Ok(plan)
    }

    /// Carry out a plan, each object in a runtime transaction of its own under the live revision, with the
    /// operator's note on every receipt. What changed since the plan was made is skipped and reported.
    pub fn apply_finished_work(
        &mut self,
        project: Id,
        plan: &FinishedWorkPlan,
        note: &str,
    ) -> Result<FinishedWorkReconcile> {
        let mut report = FinishedWorkReconcile::default();
        for planned in &plan.sessions {
            let observed = (planned.source_revision, planned.source_fingerprint.clone());
            if let Some(reason) =
                self.in_flight(project, planned.session_id, planned.work_item_id)?
            {
                report.skip(
                    "session",
                    planned.session_id,
                    &planned.work_item_key,
                    reason,
                );
                continue;
            }
            if !self.session_ends_with(project, planned.session_id, planned.work_item_id)? {
                report.skip(
                    "session",
                    planned.session_id,
                    &planned.work_item_key,
                    "session_changed_since_the_preview",
                );
                continue;
            }
            match self.with_live_revision(project, |store, revision| {
                store.interrupt_for_finished_work(
                    project,
                    revision,
                    planned.session_id,
                    planned.work_item_id,
                    &observed,
                    Some(note),
                    true,
                )
            }) {
                Ok((claims, session)) => {
                    report.released_claim_ids.extend(claims);
                    report.interrupted_session_ids.push(session);
                }
                Err(_) => report.skip(
                    "session",
                    planned.session_id,
                    &planned.work_item_key,
                    "could_not_reconcile",
                ),
            }
        }
        for planned in &plan.claims {
            let observed = (planned.source_revision, planned.source_fingerprint.clone());
            match self.with_live_revision(project, |store, revision| {
                store.release_for_finished_work(
                    project,
                    revision,
                    planned.claim_id,
                    &observed,
                    Some(note),
                )
            }) {
                Ok(id) => report.released_claim_ids.push(id),
                Err(_) => report.skip(
                    "claim",
                    planned.claim_id,
                    &planned.work_item_key,
                    "could_not_reconcile",
                ),
            }
        }
        Ok(report)
    }
}
