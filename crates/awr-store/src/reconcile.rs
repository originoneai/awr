use crate::{
    Store,
    catalog::{id_at, revision_at},
    db_error,
    transaction::{optional_id, sqlite_revision},
};
use awr_core::{Artifact, Error, Event, EventDraft, Id, Result, Revision, now_millis};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// How long a claim may be held without an expiry before `doctor` says so.
pub const CLAIM_WITHOUT_EXPIRY_WARN_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeFinding {
    pub code: String,
    pub severity: String,
    pub object_kind: String,
    pub object_id: String,
    pub message: String,
    pub repair: Option<ReconcileAction>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeInspection {
    pub project_id: Id,
    pub project_revision: Revision,
    pub checked_at: i64,
    pub findings: Vec<RuntimeFinding>,
    pub active_session_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReconcileAction {
    ExpireClaim { claim_id: Id },
    InterruptSession { session_id: Id },
    AbandonCheckpoint { attempt_id: Id },
    ClearInvalidBranch { branch_id: Id },
}

#[derive(Debug, Serialize)]
pub struct ReconcileReceipt {
    pub action: ReconcileAction,
    pub event: Event,
    pub project_revision: Revision,
}

fn finding(
    code: &str,
    severity: &str,
    object_kind: &str,
    object_id: impl ToString,
    message: impl Into<String>,
    repair: Option<ReconcileAction>,
) -> RuntimeFinding {
    RuntimeFinding {
        code: code.into(),
        severity: severity.into(),
        object_kind: object_kind.into(),
        object_id: object_id.to_string(),
        message: message.into(),
        repair,
    }
}

fn inspect_sources(
    conn: &Connection,
    project: Id,
    findings: &mut Vec<RuntimeFinding>,
) -> Result<()> {
    let rows = conn
        .prepare(
            "SELECT id,freshness FROM sources
             WHERE project_id=?1 AND active=1 AND freshness!='fresh' ORDER BY id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((id_at(row, 0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for (id, freshness) in rows {
        let (code, severity) = if freshness == "unavailable" {
            ("source_unavailable", "error")
        } else {
            ("source_stale", "warning")
        };
        findings.push(finding(
            code,
            severity,
            "source",
            id,
            format!(
                "Active source metadata is {freshness}; Store did not access the source locator or change its state"
            ),
            None,
        ));
    }
    Ok(())
}

fn inspect_sessions(
    conn: &Connection,
    project: Id,
    project_revision: Revision,
    findings: &mut Vec<RuntimeFinding>,
) -> Result<usize> {
    let rows = conn
        .prepare(
            "SELECT s.id,s.work_item_id,s.branch_id,
                    w.id,w.active,w.status,
                    b.id,b.status,b.fork_project_revision,b.parent_branch_id,parent.id
             FROM sessions s
             LEFT JOIN work_items w ON w.project_id=s.project_id AND w.id=s.work_item_id
             LEFT JOIN branches b ON b.project_id=s.project_id AND b.id=s.branch_id
             LEFT JOIN branches parent ON parent.project_id=b.project_id AND parent.id=b.parent_branch_id
             WHERE s.project_id=?1 AND s.status='active' ORDER BY s.id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((
                id_at(row, 0)?,
                optional_id(row, 1)?,
                optional_id(row, 2)?,
                optional_id(row, 3)?,
                row.get::<_, Option<bool>>(4)?,
                row.get::<_, Option<String>>(5)?,
                optional_id(row, 6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                optional_id(row, 9)?,
                optional_id(row, 10)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let active_session_count = rows.len();
    for (
        session_id,
        work_id,
        branch_id,
        joined_work_id,
        work_active,
        work_status,
        joined_branch_id,
        branch_status,
        fork_revision,
        parent_id,
        joined_parent_id,
    ) in rows
    {
        let mut problems = Vec::new();
        if work_id.is_some() && joined_work_id.is_none() {
            problems.push("bound work item is missing".to_string());
        }
        if work_active == Some(false) {
            problems.push("bound work item is retired".to_string());
        }
        if work_status
            .as_deref()
            .is_some_and(|status| matches!(status, "completed" | "cancelled" | "unknown"))
        {
            problems.push(format!(
                "bound work item is terminal or unusable ({})",
                work_status.as_deref().unwrap_or_default()
            ));
        }
        if branch_id.is_some() && joined_branch_id.is_none() {
            problems.push("bound branch is missing".to_string());
        }
        if branch_status
            .as_deref()
            .is_some_and(|status| status != "active")
        {
            problems.push(format!(
                "bound branch is {}",
                branch_status.as_deref().unwrap_or_default()
            ));
        }
        if fork_revision.is_some_and(|revision| revision < 0 || revision as u64 > project_revision)
        {
            problems.push("bound branch forks from a future project revision".to_string());
        }
        if (parent_id.is_some() && parent_id == branch_id)
            || (parent_id.is_some() && joined_parent_id.is_none())
        {
            problems.push("bound branch has an invalid parent reference".to_string());
        }
        let repair = Some(ReconcileAction::InterruptSession { session_id });
        if problems.is_empty() {
            findings.push(finding(
                "active_session",
                "info",
                "session",
                session_id,
                "Session is active in the database; process liveness was not inspected, so death cannot be inferred",
                repair,
            ));
        } else {
            findings.push(finding(
                "orphan_session",
                "error",
                "session",
                session_id,
                format!(
                    "Active session has inconsistent bindings: {}",
                    problems.join("; ")
                ),
                repair,
            ));
        }
    }
    Ok(active_session_count)
}

fn inspect_claims(
    conn: &Connection,
    project: Id,
    checked_at: i64,
    findings: &mut Vec<RuntimeFinding>,
) -> Result<()> {
    let rows = conn
        .prepare(
            "SELECT c.id,c.work_item_id,c.session_id,c.agent_id,c.branch_id,c.status,c.expires_at,c.released_at,
                    s.id,s.work_item_id,s.agent_id,s.branch_id,s.status,
                    w.id,w.active,w.status,b.id,b.status,c.acquired_at
             FROM claims c
             LEFT JOIN sessions s ON s.project_id=c.project_id AND s.id=c.session_id
             LEFT JOIN work_items w ON w.project_id=c.project_id AND w.id=c.work_item_id
             LEFT JOIN branches b ON b.project_id=c.project_id AND b.id=c.branch_id
             WHERE c.project_id=?1 ORDER BY c.id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((
                id_at(row, 0)?,
                id_at(row, 1)?,
                id_at(row, 2)?,
                row.get::<_, String>(3)?,
                optional_id(row, 4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                optional_id(row, 8)?,
                optional_id(row, 9)?,
                row.get::<_, Option<String>>(10)?,
                optional_id(row, 11)?,
                row.get::<_, Option<String>>(12)?,
                optional_id(row, 13)?,
                row.get::<_, Option<bool>>(14)?,
                row.get::<_, Option<String>>(15)?,
                optional_id(row, 16)?,
                row.get::<_, Option<String>>(17)?,
                row.get::<_, i64>(18)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for (
        claim_id,
        work_id,
        session_id,
        agent_id,
        branch_id,
        status,
        expires_at,
        released_at,
        joined_session_id,
        session_work_id,
        session_agent_id,
        session_branch_id,
        session_status,
        joined_work_id,
        work_active,
        work_status,
        joined_branch_id,
        branch_status,
        acquired_at,
    ) in rows
    {
        let truly_expired = status == "active"
            && released_at.is_none()
            && expires_at.is_some_and(|expires| expires <= checked_at);
        if truly_expired {
            findings.push(finding(
                "expired_claim",
                "warning",
                "claim",
                claim_id,
                format!(
                    "Active claim expired at {}; no automatic repair was applied",
                    expires_at.unwrap_or_default()
                ),
                Some(ReconcileAction::ExpireClaim { claim_id }),
            ));
        }

        let mut problems = Vec::new();
        if joined_session_id.is_none() {
            problems.push(format!("owning session {session_id} is missing"));
        } else {
            if session_work_id != Some(work_id) {
                problems.push("work item differs from the owning session".to_string());
            }
            if session_agent_id.as_deref() != Some(agent_id.as_str()) {
                problems.push("agent differs from the owning session".to_string());
            }
            if session_branch_id != branch_id {
                problems.push("branch differs from the owning session".to_string());
            }
            if status == "active" && session_status.as_deref() != Some("active") {
                problems.push("active claim belongs to a non-active session".to_string());
            }
        }
        if joined_work_id.is_none() {
            problems.push("work item is missing".to_string());
        }
        if status == "active" && work_active == Some(false) {
            problems.push("work item is retired".to_string());
        }
        if status == "active"
            && work_status
                .as_deref()
                .is_some_and(|value| matches!(value, "completed" | "cancelled" | "unknown"))
        {
            problems.push("work item is terminal or unusable".to_string());
        }
        if branch_id.is_some() && joined_branch_id.is_none() {
            problems.push("branch is missing".to_string());
        }
        if status == "active"
            && branch_status
                .as_deref()
                .is_some_and(|value| value != "active")
        {
            problems.push("branch is not active".to_string());
        }
        if status == "active" && released_at.is_some() {
            problems.push("active claim has a release timestamp".to_string());
        }
        if problems.is_empty()
            && status == "active"
            && released_at.is_none()
            && expires_at.is_none()
            && acquired_at.saturating_add(CLAIM_WITHOUT_EXPIRY_WARN_AFTER_MS) <= checked_at
        {
            findings.push(finding(
                "claim_without_expiry",
                "warning",
                "claim",
                claim_id,
                format!(
                    "Active claim has had no expiry since {acquired_at}; a session that stopped without ending leaves it held for good. \
                     Give claims a lifetime with --ttl-ms, or declare a default in .awr/policy.toml (claims.default_ttl) or AWR_DEFAULT_CLAIM_TTL"
                ),
                None,
            ));
        }
        if !problems.is_empty() {
            findings.push(finding(
                "invalid_claim",
                "error",
                "claim",
                claim_id,
                format!(
                    "Claim has inconsistent bindings or state: {}",
                    problems.join("; ")
                ),
                None,
            ));
        }
    }
    Ok(())
}

fn inspect_checkpoint_attempts(
    conn: &Connection,
    project: Id,
    findings: &mut Vec<RuntimeFinding>,
) -> Result<()> {
    let rows = conn
        .prepare(
            "SELECT started.id,started.project_revision,started.created_at
             FROM events started
             WHERE started.project_id=?1 AND started.event_type='checkpoint.started'
               AND NOT EXISTS (
                 SELECT 1 FROM events finished
                 WHERE finished.project_id=started.project_id AND finished.event_type='checkpoint.created'
                   AND json_extract(finished.payload_json,'$.attempt_id')=started.id)
               AND NOT EXISTS (
                 SELECT 1 FROM events abandoned
                 WHERE abandoned.project_id=started.project_id AND abandoned.event_type='checkpoint.abandoned'
                   AND json_extract(abandoned.payload_json,'$.attempt_id')=started.id)
             ORDER BY started.project_revision,started.id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((
                id_at(row, 0)?,
                revision_at(row, 1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for (attempt_id, revision, created_at) in rows {
        findings.push(finding(
            "incomplete_checkpoint",
            "warning",
            "checkpoint_attempt",
            attempt_id,
            format!(
                "Checkpoint save started at revision {revision} and {created_at} has no completion or abandonment receipt; it may still be in flight"
            ),
            Some(ReconcileAction::AbandonCheckpoint { attempt_id }),
        ));
    }
    Ok(())
}

fn inspect_checkpoint_pointers(
    conn: &Connection,
    project: Id,
    findings: &mut Vec<RuntimeFinding>,
) -> Result<()> {
    let rows = conn
        .prepare(
            "SELECT s.id,s.last_checkpoint_id,c.id,c.session_id,
                    CASE WHEN c.id IS NULL THEN 0 ELSE EXISTS(
                      SELECT 1 FROM checkpoints newer WHERE newer.session_id=s.id AND
                        (newer.project_revision>c.project_revision OR
                         (newer.project_revision=c.project_revision AND newer.created_at>c.created_at) OR
                         (newer.project_revision=c.project_revision AND newer.created_at=c.created_at AND newer.id>c.id))) END
             FROM sessions s LEFT JOIN checkpoints c ON c.id=s.last_checkpoint_id
             WHERE s.project_id=?1 AND s.last_checkpoint_id IS NOT NULL ORDER BY s.id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((
                id_at(row, 0)?,
                optional_id(row, 1)?,
                optional_id(row, 2)?,
                optional_id(row, 3)?,
                row.get::<_, bool>(4)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for (session_id, pointer, checkpoint_id, owner, stale) in rows {
        let pointer = pointer.expect("query filters null checkpoint pointers");
        let message = if checkpoint_id.is_none() {
            Some("Session last_checkpoint_id points to a missing checkpoint".to_string())
        } else if owner != Some(session_id) {
            Some("Session last_checkpoint_id points to another session's checkpoint".to_string())
        } else if stale {
            Some("Session last_checkpoint_id does not point to its newest checkpoint".to_string())
        } else {
            None
        };
        if let Some(message) = message {
            findings.push(finding(
                "invalid_checkpoint_pointer",
                "error",
                "session",
                session_id,
                format!("{message}: {pointer}"),
                None,
            ));
        }
    }
    Ok(())
}

fn inspect_artifacts(
    conn: &Connection,
    project: Id,
    findings: &mut Vec<RuntimeFinding>,
) -> Result<()> {
    let rows = conn
        .prepare(
            "SELECT artifact.id,artifact.source_event_id,event.id
             FROM artifacts artifact
             LEFT JOIN events event ON event.project_id=artifact.project_id AND event.id=artifact.source_event_id
             WHERE artifact.project_id=?1 AND (artifact.source_event_id IS NULL OR event.id IS NULL)
             ORDER BY artifact.id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((id_at(row, 0)?, optional_id(row, 1)?))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for (artifact_id, source_event_id) in rows {
        findings.push(finding(
            "orphan_artifact",
            "error",
            "artifact",
            artifact_id,
            source_event_id.map_or_else(
                || "Artifact metadata has no source event; no artifact file was accessed".into(),
                |event| format!(
                    "Artifact metadata references missing source event {event}; no artifact file was accessed"
                ),
            ),
            None,
        ));
    }
    Ok(())
}

fn inspect_branches(
    conn: &Connection,
    project: Id,
    project_revision: Revision,
    current_branch: Option<Id>,
    findings: &mut Vec<RuntimeFinding>,
) -> Result<()> {
    let rows = conn
        .prepare(
            "SELECT branch.id,branch.status,branch.fork_project_revision,branch.parent_branch_id,parent.id
             FROM branches branch
             LEFT JOIN branches parent ON parent.project_id=branch.project_id AND parent.id=branch.parent_branch_id
             WHERE branch.project_id=?1 ORDER BY branch.id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((
                id_at(row, 0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                optional_id(row, 3)?,
                optional_id(row, 4)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let mut current_seen = false;
    for (branch_id, status, fork_revision, parent_id, joined_parent_id) in rows {
        let is_current = current_branch == Some(branch_id);
        current_seen |= is_current;
        let mut problems = Vec::new();
        if is_current && status != "active" {
            problems.push(format!("current branch is {status}"));
        }
        if status == "active" {
            if fork_revision < 0 || fork_revision as u64 > project_revision {
                problems.push("active branch forks from a future project revision".to_string());
            }
            if parent_id == Some(branch_id) || (parent_id.is_some() && joined_parent_id.is_none()) {
                problems.push("active branch has an invalid parent reference".to_string());
            }
        }
        if !problems.is_empty() {
            findings.push(finding(
                "invalid_branch",
                "error",
                "branch",
                branch_id,
                problems.join("; "),
                is_current.then_some(ReconcileAction::ClearInvalidBranch { branch_id }),
            ));
        }
    }
    if let Some(branch_id) = current_branch.filter(|_| !current_seen) {
        findings.push(finding(
            "invalid_branch",
            "error",
            "branch",
            branch_id,
            "Project current_branch_id points to a missing branch",
            Some(ReconcileAction::ClearInvalidBranch { branch_id }),
        ));
    }
    Ok(())
}

fn inspect_mutations(
    conn: &Connection,
    project: Id,
    findings: &mut Vec<RuntimeFinding>,
) -> Result<()> {
    let rows = conn
        .prepare(
            "SELECT id,status FROM mutation_proposals
             WHERE project_id=?1 AND status IN ('draft','ready','approved','failed') ORDER BY id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((id_at(row, 0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for (id, status) in rows {
        if let Some(attempt) = crate::mutation_apply::latest(conn, project, id)?
            .filter(|a| a.resolved_event_id.is_none())
        {
            findings.push(finding("incomplete_mutation","error","mutation_proposal",id,
                format!("Application {} is unfinished; source bytes may already be written. Inspect proposal show {id} and retained snapshots at {}; use proposal recover with the current revision to resolve it",attempt.event_id,attempt.plan.recovery_directory()),None));
            continue;
        }
        let failed = status == "failed";
        findings.push(finding(
            if failed {
                "failed_mutation"
            } else {
                "pending_mutation"
            },
            if failed { "error" } else { "warning" },
            "mutation_proposal",
            id,
            if failed {
                "Mutation proposal failed; reconciliation reported it without retrying or applying it automatically".into()
            } else {
                format!(
                    "Mutation proposal is {status}; it remains pending and was not applied automatically"
                )
            },
            None,
        ));
    }
    Ok(())
}

fn inspect_edges(conn: &Connection, project: Id, findings: &mut Vec<RuntimeFinding>) -> Result<()> {
    let rows = conn
        .prepare(
            "WITH active_entities(kind,key) AS (
               SELECT 'goal',external_key FROM goals WHERE project_id=?1 AND active=1
               UNION ALL SELECT 'plan',external_key FROM plans WHERE project_id=?1 AND active=1
               UNION ALL SELECT 'rule',external_key FROM rules WHERE project_id=?1 AND active=1
               UNION ALL SELECT 'work_item',external_key FROM work_items WHERE project_id=?1 AND active=1
               UNION ALL SELECT 'decision',external_key FROM decisions WHERE project_id=?1 AND active=1
               UNION ALL SELECT 'evidence',external_key FROM evidence WHERE project_id=?1 AND active=1
             )
             SELECT edge.id,edge.from_kind,edge.from_key,edge.relation,edge.to_kind,edge.to_key,
                    source.id,source.active,from_entity.key,to_entity.key
             FROM edges edge
             LEFT JOIN sources source ON source.project_id=edge.project_id AND source.id=edge.source_id
             LEFT JOIN active_entities from_entity ON from_entity.kind=edge.from_kind AND from_entity.key=edge.from_key
             LEFT JOIN active_entities to_entity ON to_entity.kind=edge.to_kind AND to_entity.key=edge.to_key
             WHERE edge.project_id=?1 AND edge.active=1 AND edge.required=1
               AND (source.id IS NULL OR source.active=0 OR from_entity.key IS NULL OR to_entity.key IS NULL)
             ORDER BY edge.id",
        )
        .map_err(db_error)?
        .query_map([project.to_string()], |row| {
            Ok((
                id_at(row, 0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                optional_id(row, 6)?,
                row.get::<_, Option<bool>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for (
        edge_id,
        from_kind,
        from_key,
        relation,
        to_kind,
        to_key,
        source_id,
        source_active,
        from_entity,
        to_entity,
    ) in rows
    {
        let mut problems = Vec::new();
        if source_id.is_none() || source_active == Some(false) {
            problems.push("edge source is missing or inactive".to_string());
        }
        if from_entity.is_none() {
            problems.push(format!(
                "origin {from_kind}:{from_key} is missing or retired"
            ));
        }
        if to_entity.is_none() {
            problems.push(format!("target {to_kind}:{to_key} is missing or retired"));
        }
        let missing_dependency = relation == "depends_on"
            && from_kind == "work_item"
            && to_kind == "work_item"
            && to_entity.is_none();
        findings.push(finding(
            if missing_dependency {
                "missing_dependency"
            } else {
                "dangling_edge"
            },
            "error",
            "edge",
            edge_id,
            problems.join("; "),
            None,
        ));
    }
    Ok(())
}

fn branch_is_invalid(
    conn: &Connection,
    project: Id,
    branch: Id,
    revision: Revision,
) -> Result<bool> {
    let state = conn
        .query_row(
            "SELECT b.status,b.fork_project_revision,b.parent_branch_id,parent.id
             FROM branches b
             LEFT JOIN branches parent ON parent.project_id=b.project_id AND parent.id=b.parent_branch_id
             WHERE b.project_id=?1 AND b.id=?2",
            params![project.to_string(), branch.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    optional_id(row, 2)?,
                    optional_id(row, 3)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some((status, fork_revision, parent_id, joined_parent_id)) = state else {
        return Ok(true);
    };
    Ok(status != "active"
        || fork_revision < 0
        || fork_revision as u64 > revision
        || parent_id == Some(branch)
        || (parent_id.is_some() && joined_parent_id.is_none()))
}

impl Store {
    /// Inspect runtime metadata in one SQLite read snapshot. No source or artifact locator is opened.
    pub fn inspect_runtime(&self, project: Id, checked_at: i64) -> Result<RuntimeInspection> {
        if checked_at < 0 {
            return Err(Error::InvalidInput(
                "runtime inspection time cannot be negative".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let (project_revision, current_branch) = tx
            .query_row(
                "SELECT project_revision,current_branch_id FROM projects WHERE id=?1",
                [project.to_string()],
                |row| Ok((revision_at(row, 0)?, optional_id(row, 1)?)),
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| Error::NotFound(format!("project {project}")))?;
        let mut findings = Vec::new();
        inspect_sources(&tx, project, &mut findings)?;
        let active_session_count = inspect_sessions(&tx, project, project_revision, &mut findings)?;
        inspect_claims(&tx, project, checked_at, &mut findings)?;
        inspect_checkpoint_attempts(&tx, project, &mut findings)?;
        inspect_checkpoint_pointers(&tx, project, &mut findings)?;
        inspect_artifacts(&tx, project, &mut findings)?;
        inspect_branches(
            &tx,
            project,
            project_revision,
            current_branch,
            &mut findings,
        )?;
        inspect_mutations(&tx, project, &mut findings)?;
        inspect_edges(&tx, project, &mut findings)?;
        findings.sort_by(|left, right| {
            fn severity_rank(value: &str) -> u8 {
                match value {
                    "error" => 0,
                    "warning" => 1,
                    _ => 2,
                }
            }
            severity_rank(&left.severity)
                .cmp(&severity_rank(&right.severity))
                .then_with(|| left.code.cmp(&right.code))
                .then_with(|| left.object_kind.cmp(&right.object_kind))
                .then_with(|| left.object_id.cmp(&right.object_id))
        });
        tx.commit().map_err(db_error)?;
        Ok(RuntimeInspection {
            project_id: project,
            project_revision,
            checked_at,
            findings,
            active_session_count,
        })
    }

    /// Reconcile exactly one selected runtime object under a project revision CAS.
    pub fn reconcile(
        &mut self,
        project: Id,
        expected: Revision,
        action: ReconcileAction,
        reason: &str,
    ) -> Result<ReconcileReceipt> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(Error::InvalidInput(
                "reconciliation reason must not be empty".into(),
            ));
        }
        let event_type = match action {
            ReconcileAction::ExpireClaim { .. } => "claim.expired",
            ReconcileAction::InterruptSession { .. } => "session.interrupted",
            ReconcileAction::AbandonCheckpoint { .. } => "checkpoint.abandoned",
            ReconcileAction::ClearInvalidBranch { .. } => "branch.current_cleared",
        };
        let action_for_apply = action.clone();
        let (_, event) = self.runtime_transaction_with_event(
            project,
            expected,
            EventDraft::new(event_type, "Applied explicit runtime reconciliation"),
            |tx, next, event| {
                match action_for_apply {
                    ReconcileAction::ExpireClaim { claim_id } => {
                        let (
                            status,
                            expires_at,
                            released_at,
                            session_id,
                            work_id,
                            branch_id,
                            joined_session_id,
                            session_work_id,
                            session_branch_id,
                            joined_work_id,
                            joined_branch_id,
                        ) = tx
                            .query_row(
                                "SELECT claim.status,claim.expires_at,claim.released_at,claim.session_id,claim.work_item_id,claim.branch_id,
                                        session.id,session.work_item_id,session.branch_id,work.id,branch.id
                                 FROM claims claim
                                 LEFT JOIN sessions session ON session.project_id=claim.project_id AND session.id=claim.session_id
                                 LEFT JOIN work_items work ON work.project_id=claim.project_id AND work.id=claim.work_item_id
                                 LEFT JOIN branches branch ON branch.project_id=claim.project_id AND branch.id=claim.branch_id
                                 WHERE claim.project_id=?1 AND claim.id=?2",
                                params![project.to_string(), claim_id.to_string()],
                                |row| {
                                    Ok((
                                        row.get::<_, String>(0)?,
                                        row.get::<_, Option<i64>>(1)?,
                                        row.get::<_, Option<i64>>(2)?,
                                        id_at(row, 3)?,
                                        id_at(row, 4)?,
                                        optional_id(row, 5)?,
                                        optional_id(row, 6)?,
                                        optional_id(row, 7)?,
                                        optional_id(row, 8)?,
                                        optional_id(row, 9)?,
                                        optional_id(row, 10)?,
                                    ))
                                },
                            )
                            .optional()
                            .map_err(db_error)?
                            .ok_or_else(|| Error::NotFound(format!("claim {claim_id}")))?;
                        let at = now_millis()?;
                        if status != "active"
                            || released_at.is_some()
                            || expires_at.is_none_or(|expires| expires > at)
                        {
                            return Err(Error::InvalidTransition(format!(
                                "claim {claim_id} is not an expired active claim"
                            )));
                        }
                        let updated = tx
                            .execute(
                                "UPDATE claims SET status='expired',released_at=NULL,revision=revision+1
                                 WHERE project_id=?1 AND id=?2 AND status='active' AND released_at IS NULL AND expires_at<=?3",
                                params![project.to_string(), claim_id.to_string(), at],
                            )
                            .map_err(db_error)?;
                        if updated != 1 {
                            return Err(Error::MutationConflict(format!(
                                "claim {claim_id} changed during reconciliation"
                            )));
                        }
                        let session_scope_valid = joined_session_id == Some(session_id)
                            && session_work_id == Some(work_id)
                            && session_branch_id == branch_id
                            && joined_work_id == Some(work_id)
                            && (branch_id.is_none() || joined_branch_id == branch_id);
                        if session_scope_valid {
                            event.session_id = Some(session_id);
                        } else {
                            event.work_item_id = (joined_work_id == Some(work_id)).then_some(work_id);
                            event.branch_id = branch_id
                                .filter(|branch| joined_branch_id == Some(*branch));
                        }
                        event.payload = serde_json::json!({
                            "claim_id": claim_id,
                            "session_id": session_id,
                            "expired_at": expires_at,
                            "reconciled_at": at,
                            "reason": reason,
                        });
                    }
                    ReconcileAction::InterruptSession { session_id } => {
                        let (
                            status,
                            last_checkpoint_id,
                            work_id,
                            branch_id,
                            joined_work_id,
                            joined_branch_id,
                        ) = tx
                            .query_row(
                                "SELECT session.status,session.last_checkpoint_id,session.work_item_id,session.branch_id,work.id,branch.id
                                 FROM sessions session
                                 LEFT JOIN work_items work ON work.project_id=session.project_id AND work.id=session.work_item_id
                                 LEFT JOIN branches branch ON branch.project_id=session.project_id AND branch.id=session.branch_id
                                 WHERE session.project_id=?1 AND session.id=?2",
                                params![project.to_string(), session_id.to_string()],
                                |row| {
                                    Ok((
                                        row.get::<_, String>(0)?,
                                        optional_id(row, 1)?,
                                        optional_id(row, 2)?,
                                        optional_id(row, 3)?,
                                        optional_id(row, 4)?,
                                        optional_id(row, 5)?,
                                    ))
                                },
                            )
                            .optional()
                            .map_err(db_error)?
                            .ok_or_else(|| Error::NotFound(format!("session {session_id}")))?;
                        if status != "active" {
                            return Err(Error::InvalidTransition(format!(
                                "session {session_id} is {status}"
                            )));
                        }
                        let at = now_millis()?;
                        let claim_rows = tx
                            .prepare(
                                "SELECT id,expires_at FROM claims
                                 WHERE project_id=?1 AND session_id=?2 AND status='active' ORDER BY id",
                            )
                            .map_err(db_error)?
                            .query_map(params![project.to_string(), session_id.to_string()], |row| {
                                Ok((id_at(row, 0)?, row.get::<_, Option<i64>>(1)?))
                            })
                            .map_err(db_error)?
                            .collect::<rusqlite::Result<Vec<_>>>()
                            .map_err(db_error)?;
                        let mut expired_claim_ids = Vec::new();
                        let mut released_claim_ids = Vec::new();
                        for (claim_id, expiration) in claim_rows {
                            if expiration.is_some_and(|expires| expires <= at) {
                                expired_claim_ids.push(claim_id);
                            } else {
                                released_claim_ids.push(claim_id);
                            }
                        }
                        tx.execute(
                            "UPDATE claims SET status=CASE WHEN expires_at IS NOT NULL AND expires_at<=?1 THEN 'expired' ELSE 'released' END,
                               released_at=CASE WHEN expires_at IS NOT NULL AND expires_at<=?1 THEN NULL ELSE ?1 END,revision=revision+1
                             WHERE project_id=?2 AND session_id=?3 AND status='active'",
                            params![at, project.to_string(), session_id.to_string()],
                        )
                        .map_err(db_error)?;
                        let updated = tx
                            .execute(
                                "UPDATE sessions SET status='interrupted',ended_at=?1,end_project_revision=?2,revision=revision+1
                                 WHERE project_id=?3 AND id=?4 AND status='active'",
                                params![
                                    at,
                                    sqlite_revision(next)?,
                                    project.to_string(),
                                    session_id.to_string()
                                ],
                            )
                            .map_err(db_error)?;
                        if updated != 1 {
                            return Err(Error::MutationConflict(format!(
                                "session {session_id} changed during reconciliation"
                            )));
                        }
                        if work_id.is_none_or(|work| joined_work_id == Some(work))
                            && branch_id.is_none_or(|branch| joined_branch_id == Some(branch))
                        {
                            event.session_id = Some(session_id);
                        } else {
                            event.work_item_id = work_id
                                .filter(|work| joined_work_id == Some(*work));
                            event.branch_id = branch_id
                                .filter(|branch| joined_branch_id == Some(*branch));
                        }
                        event.payload = serde_json::json!({
                            "session_id": session_id,
                            "status": "interrupted",
                            "expired_claim_ids": expired_claim_ids,
                            "released_claim_ids": released_claim_ids,
                            "last_checkpoint_id": last_checkpoint_id,
                            "reconciled_at": at,
                            "reason": reason,
                        });
                    }
                    ReconcileAction::AbandonCheckpoint { attempt_id } => {
                        let started = tx
                            .query_row(
                                "SELECT started.session_id,started.work_item_id,started.branch_id,
                                        session.id,session.work_item_id,session.branch_id,work.id,branch.id
                                 FROM events started
                                 LEFT JOIN sessions session ON session.project_id=started.project_id AND session.id=started.session_id
                                 LEFT JOIN work_items work ON work.project_id=started.project_id AND work.id=started.work_item_id
                                 LEFT JOIN branches branch ON branch.project_id=started.project_id AND branch.id=started.branch_id
                                 WHERE started.project_id=?1 AND started.id=?2 AND started.event_type='checkpoint.started'",
                                params![project.to_string(), attempt_id.to_string()],
                                |row| {
                                    Ok((
                                        optional_id(row, 0)?,
                                        optional_id(row, 1)?,
                                        optional_id(row, 2)?,
                                        optional_id(row, 3)?,
                                        optional_id(row, 4)?,
                                        optional_id(row, 5)?,
                                        optional_id(row, 6)?,
                                        optional_id(row, 7)?,
                                    ))
                                },
                            )
                            .optional()
                            .map_err(db_error)?;
                        let Some((
                            session_id,
                            work_id,
                            branch_id,
                            joined_session_id,
                            session_work_id,
                            session_branch_id,
                            joined_work_id,
                            joined_branch_id,
                        )) = started else {
                            return Err(Error::NotFound(format!("checkpoint save attempt {attempt_id}")));
                        };
                        let (completed, abandoned): (bool, bool) = tx
                            .query_row(
                                "SELECT
                                   EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND event_type='checkpoint.created' AND json_extract(payload_json,'$.attempt_id')=?2),
                                   EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND event_type='checkpoint.abandoned' AND json_extract(payload_json,'$.attempt_id')=?2)",
                                params![project.to_string(), attempt_id.to_string()],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .map_err(db_error)?;
                        if completed || abandoned {
                            return Err(Error::InvalidTransition(format!(
                                "checkpoint attempt {attempt_id} is already {}",
                                if completed { "completed" } else { "abandoned" }
                            )));
                        }
                        let session_scope_valid = session_id.is_some()
                            && joined_session_id == session_id
                            && session_work_id == work_id
                            && session_branch_id == branch_id
                            && work_id.is_none_or(|work| joined_work_id == Some(work))
                            && branch_id.is_none_or(|branch| joined_branch_id == Some(branch));
                        if session_scope_valid {
                            event.session_id = session_id;
                        } else {
                            event.work_item_id = work_id
                                .filter(|work| joined_work_id == Some(*work));
                            event.branch_id = branch_id
                                .filter(|branch| joined_branch_id == Some(*branch));
                        }
                        event.payload = serde_json::json!({
                            "attempt_id": attempt_id,
                            "reason": reason,
                        });
                    }
                    ReconcileAction::ClearInvalidBranch { branch_id } => {
                        let current = tx
                            .query_row(
                                "SELECT current_branch_id FROM projects WHERE id=?1",
                                [project.to_string()],
                                |row| optional_id(row, 0),
                            )
                            .map_err(db_error)?;
                        if current != Some(branch_id) {
                            return Err(Error::InvalidTransition(format!(
                                "branch {branch_id} is not the project's current branch"
                            )));
                        }
                        if !branch_is_invalid(tx, project, branch_id, expected)? {
                            return Err(Error::InvalidTransition(format!(
                                "current branch {branch_id} is still valid"
                            )));
                        }
                        let updated = tx
                            .execute(
                                "UPDATE projects SET current_branch_id=NULL WHERE id=?1 AND current_branch_id=?2",
                                params![project.to_string(), branch_id.to_string()],
                            )
                            .map_err(db_error)?;
                        if updated != 1 {
                            return Err(Error::MutationConflict(format!(
                                "current branch {branch_id} changed during reconciliation"
                            )));
                        }
                        let branch_exists: bool = tx
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM branches WHERE project_id=?1 AND id=?2)",
                                params![project.to_string(), branch_id.to_string()],
                                |row| row.get(0),
                            )
                            .map_err(db_error)?;
                        event.branch_id = branch_exists.then_some(branch_id);
                        event.payload = serde_json::json!({
                            "branch_id": branch_id,
                            "current_branch_id": null,
                            "reason": reason,
                        });
                    }
                }
                Ok(())
            },
        )?;
        Ok(ReconcileReceipt {
            action,
            project_revision: event.project_revision,
            event,
        })
    }

    /// Artifact metadata only. Runtime is responsible for reading and validating artifact files.
    pub fn artifacts(&self, project: Id) -> Result<Vec<Artifact>> {
        self.project(project)?;
        self.conn
            .prepare(
                "SELECT id,project_id,artifact_type,locator,sha256,size,mime,source_event_id,revision
                 FROM artifacts WHERE project_id=?1 ORDER BY id",
            )
            .map_err(db_error)?
            .query_map([project.to_string()], |row| {
                let size: i64 = row.get(5)?;
                Ok(Artifact {
                    id: id_at(row, 0)?,
                    project_id: id_at(row, 1)?,
                    artifact_type: row.get(2)?,
                    locator: row.get(3)?,
                    sha256: row.get(4)?,
                    size: size.try_into().map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    mime: row.get(6)?,
                    source_event_id: optional_id(row, 7)?,
                    revision: revision_at(row, 8)?,
                })
            })
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)
    }
}
