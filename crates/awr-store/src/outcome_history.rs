//! The receipts, executions, sessions and usage of one project, read as the input of the outcome history.
use crate::{Store, db_error};
use awr_core::*;
use rusqlite::params;
use serde_json::Value;

fn id_of(value: &Value) -> Option<Id> {
    value.as_str().and_then(|text| text.parse().ok())
}

/// What the event says about the verification and the acceptance a completion bound; nothing when the payload
/// does not have the shape of a completion, so an old or foreign receipt is counted without those facts.
fn completion_facts(payload: &Value) -> (Option<EvidenceLevel>, Option<String>) {
    let Some(binding) = serde_json::from_value::<WorkActionBinding>(payload["work_action"].clone())
        .ok()
        .and_then(|binding| binding.completion)
    else {
        return (None, None);
    };
    let level = binding
        .evidence
        .iter()
        .filter_map(|e| verification_rank(e.level).map(|rank| (rank, e.level)))
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, level)| level);
    let criteria: Vec<String> = binding
        .acceptance
        .iter()
        .map(|a| a.criterion.clone())
        .collect();
    (
        level,
        (!criteria.is_empty()).then(|| acceptance_digest(&criteria)),
    )
}

impl Store {
    /// Everything the outcome history needs from one project on one branch (none: the project's main line).
    /// Receipts and executions are those of the branch; sessions and usage have no branch of their own here.
    /// Larger families than `MAX_HISTORY_RECORDS` are refused rather than read from a prefix.
    pub fn outcome_history_input(&self, project: Id, branch: Option<Id>) -> Result<HistoryInput> {
        self.project(project)?;
        let works = self
            .work_items(project)?
            .into_iter()
            .map(|work| HistoryWork {
                work_item_id: work.item.meta.id,
                work_key: work.item.meta.external_key.clone(),
                acceptance_digest: (!work.item.acceptance.is_empty())
                    .then(|| acceptance_digest(&work.item.acceptance)),
                confirmed_acceptance_digest: work
                    .item
                    .ordinary_completion
                    .as_ref()
                    .map(|confirmed| acceptance_digest(&confirmed.acceptance)),
            })
            .collect();
        let limit = (MAX_HISTORY_RECORDS + 1) as i64;
        let rows = self
            .conn
            .prepare(
                "SELECT id,work_item_id,session_id,event_type,payload_json,project_revision,created_at
                 FROM events
                 WHERE project_id=?1 AND branch_id IS ?2 AND work_item_id IS NOT NULL
                   AND event_type IN ('work.completed','work.ordinary_confirmed','work.cancelled','work.reopened')
                 ORDER BY project_revision,id LIMIT ?3",
            )
            .map_err(db_error)?
            .query_map(
                params![project.to_string(), branch.map(|id| id.to_string()), limit],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        if rows.len() > MAX_HISTORY_RECORDS {
            return Err(Error::InvalidInput(
                "too many outcome receipts for one history".into(),
            ));
        }
        let mut receipts = Vec::with_capacity(rows.len());
        for (event, work, session, kind, payload, revision, at) in rows {
            let payload: Value = serde_json::from_str(&payload)
                .map_err(|e| Error::Storage(format!("corrupt event payload: {e}")))?;
            let (kind, level, digest, origin) = match kind.as_str() {
                "work.completed" => {
                    let (level, digest) = completion_facts(&payload);
                    (ReceiptKind::Completed, level, digest, None)
                }
                "work.ordinary_confirmed" => {
                    let origin =
                        serde_json::from_value::<HostEditBinding>(payload["host_edit"].clone())
                            .ok()
                            .map(|host| host.actor.origin);
                    (ReceiptKind::OrdinaryConfirmed, None, None, origin)
                }
                "work.cancelled" => (ReceiptKind::Cancelled, None, None, None),
                _ => (ReceiptKind::Reopened, None, None, None),
            };
            let parse = |text: &str| {
                text.parse::<Id>()
                    .map_err(|_| Error::Storage("corrupt event identity".into()))
            };
            receipts.push(HistoryReceipt {
                event_id: parse(&event)?,
                project_id: project,
                work_item_id: parse(&work)?,
                kind,
                at_ms: at,
                project_revision: u64::try_from(revision)
                    .map_err(|_| Error::Storage("corrupt event revision".into()))?,
                work_revision: payload["target_revision"].as_u64(),
                session_id: id_of(&payload["creating_session_id"])
                    .or(session.as_deref().and_then(|text| text.parse().ok())),
                evidence_level: level,
                acceptance_digest: digest,
                actor_origin: origin,
            });
        }
        let executions: Vec<HistoryExecution> = self
            .executions(project, None)?
            .into_iter()
            .filter(|run| run.branch_id == branch)
            .map(|run| HistoryExecution {
                execution_id: run.id,
                project_id: project,
                work_item_id: run.work_item_id,
                session_id: run.session_id,
                state: run.state,
                registered_at_ms: run.registered_at,
                started_at_ms: run.started_at,
                finished_at_ms: run.finished_at,
            })
            .collect();
        let sessions = self
            .conn
            .prepare(
                "SELECT id,provider,model,agent_id FROM sessions WHERE project_id=?1 ORDER BY id",
            )
            .map_err(db_error)?
            .query_map([project.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?
            .into_iter()
            .map(|(id, provider, model, agent)| {
                Ok(HistorySession {
                    session_id: id
                        .parse()
                        .map_err(|_| Error::Storage("corrupt session identity".into()))?,
                    project_id: project,
                    provider,
                    model,
                    agent,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(HistoryInput {
            project_id: project,
            works,
            receipts,
            executions,
            sessions,
            usage: self.usage_receipts_corrected(project)?,
        })
    }
}
