//! Admission decisions as events, and the readiness diagnostics derived from them.
//!
//! A decision is a `work.admission` event, so it needs no table and no migration. Whether a work
//! is admitted is never stored: it is re-derived from the latest event, the current contract, the
//! declared definition, the reviewed examples and the effective policy each time something asks.
//! The same derivation serves readiness (claims, `ready`), execution registration and start, and
//! the status the CLI and MCP report.
use crate::{
    Store,
    contract::work_contract,
    db_error,
    definition::work_definition_report,
    events::event_row,
    query::projection,
    session::{require_active, session_at},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub(crate) fn digest(value: &Value) -> Result<String> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(value)?)
    ))
}

pub(crate) fn reject(code: &'static str, detail: impl Into<String>) -> Error {
    let detail = detail.into();
    Error::Admission {
        code,
        diagnostics: json!([{"code": code, "detail": detail}]),
        message: detail,
    }
}

pub(crate) enum PolicyRead {
    Absent,
    Valid(AdmissionPolicy, Value),
    Invalid,
}

pub(crate) fn read_policy(work: &Projected<WorkItem>) -> PolicyRead {
    match work
        .source
        .config
        .get("adapter_options")
        .and_then(|options| options.get("admission_policy"))
    {
        None | Some(Value::Null) => PolicyRead::Absent,
        Some(value) => match serde_json::from_value::<AdmissionPolicy>(value.clone()) {
            Ok(policy) if policy.validate().is_ok() => PolicyRead::Valid(policy, value.clone()),
            _ => PolicyRead::Invalid,
        },
    }
}

fn mode_name(policy: &AdmissionPolicy) -> &'static str {
    match policy.mode {
        AdmissionMode::LegacyObserve => "legacy_observe",
        AdmissionMode::GovernedEnforce => "governed_enforce",
    }
}

fn current_binding(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    policy: &Value,
    report: &DefinitionReport,
) -> Result<(AdmissionBinding, Vec<ExampleOracle>)> {
    let contract = work_contract(conn, project, &work.item.meta.external_key)?;
    let mut binding =
        admission_binding(&contract.fingerprint, policy, work.item.definition.as_ref())?;
    let mut oracles = Vec::new();
    if let Some(definition) = &work.item.definition {
        let root: String = conn
            .query_row(
                "SELECT root FROM projects WHERE id=?1",
                [project.to_string()],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        for example in &definition.examples {
            let required = report.criterion_tiers.iter().any(|(criterion, tier)| {
                criterion == &example.criterion && *tier == DeliveryTier::Required
            });
            if let (true, Some(id), Some(check)) = (required, &example.id, &example.check) {
                let files = current_oracle_files(Path::new(&root), check);
                let oracle = files
                    .map(|files| example_oracle(id, &check.run, &files))
                    .transpose()?;
                oracles.push(ExampleOracle {
                    example: id.clone(),
                    oracle,
                });
            }
        }
    }
    if !oracles.is_empty() {
        let known: Option<Vec<_>> = oracles
            .iter()
            .map(|o| {
                o.oracle
                    .as_ref()
                    .map(|hash| (o.example.clone(), hash.clone()))
            })
            .collect();
        binding.oracle_fingerprint = known.map(|known| oracle_fingerprint(&known)).transpose()?;
    }
    Ok((binding, oracles))
}

struct ExampleOracle {
    example: String,
    oracle: Option<String>,
}

/// Admission runs inside the store, including direct claim and execution entry points.
/// Read only regular files within the registered project, with a bounded read per file.
fn current_oracle_files(root: &Path, check: &ExampleCheck) -> Option<Vec<OracleFile>> {
    let root = root.canonicalize().ok()?;
    check
        .files
        .iter()
        .map(|path| {
            let resolved = root.join(path.replace('\\', "/")).canonicalize().ok()?;
            if !resolved.starts_with(&root) || !resolved.is_file() {
                return None;
            }
            let file = File::open(&resolved).ok()?;
            let meta = file.metadata().ok()?;
            if !meta.is_file() || meta.len() > CHECK_FILE_CAP {
                return None;
            }
            let mut bytes = Vec::new();
            file.take(CHECK_FILE_CAP + 1).read_to_end(&mut bytes).ok()?;
            if bytes.len() as u64 > CHECK_FILE_CAP {
                return None;
            }
            Some(OracleFile {
                path: path.clone(),
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                size: bytes.len() as u64,
            })
        })
        .collect()
}

fn needs_baseline(conn: &Connection, project: Id, work: &WorkItem) -> Result<bool> {
    let Some(definition) = &work.definition else {
        return Ok(false);
    };
    let inherited: Option<Projected<WorkItem>> = if definition.discrimination.is_none() {
        definition
            .inherits
            .as_ref()
            .map(
                |key| match projection(conn, project, EntityKind::WorkItem, key) {
                    Ok(parent) => Ok(Some(parent)),
                    Err(Error::NotFound(_)) => Ok(None),
                    Err(error) => Err(error),
                },
            )
            .transpose()?
            .flatten()
    } else {
        None
    };
    let discrimination = definition.discrimination.as_ref().or_else(|| {
        inherited
            .as_ref()?
            .item
            .definition
            .as_ref()?
            .discrimination
            .as_ref()
    });
    Ok(discrimination.is_some_and(|d| d.method == DiscriminationMethod::FailBeforePassAfter))
}

fn baselines_at(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
    oracles: &[ExampleOracle],
) -> Result<Vec<AdmissionBaseline>> {
    if oracles.is_empty() || !needs_baseline(conn, project, &work.item)? {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare("SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type=?4 AND json_extract(payload_json,'$.phase')='baseline' ORDER BY project_revision DESC,id DESC").map_err(db_error)?;
    let events = stmt
        .query_map(
            params![
                project.to_string(),
                work.item.meta.id.to_string(),
                branch.map(|id| id.to_string()),
                CHECK_EVENT
            ],
            event_row,
        )
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let revision: u64 = conn
        .query_row(
            "SELECT project_revision FROM projects WHERE id=?1",
            [project.to_string()],
            |r| crate::catalog::revision_at(r, 0),
        )
        .map_err(db_error)?;
    let mut baselines = Vec::new();
    for oracle in oracles {
        let matches = |event: &&Event| {
            event.payload["example"] == oracle.example
                && oracle
                    .oracle
                    .as_ref()
                    .is_some_and(|hash| event.payload["example_oracle"] == *hash)
        };
        // One failed observation of this exact example oracle is sufficient. A later run does
        // not erase that fact, and another example never contributes a baseline here.
        let observed = events
            .iter()
            .filter(matches)
            .find(|event| event.payload["outcome"] == "failed")
            .or_else(|| events.iter().find(matches));
        let state = if oracle.oracle.is_none() {
            "baseline_inconclusive"
        } else {
            match observed.and_then(|event| event.payload["outcome"].as_str()) {
                Some("failed") => "baseline_recorded",
                Some("passed") => "baseline_passed",
                Some(_) => "baseline_inconclusive",
                None => "baseline_missing",
            }
        };
        let log_dir = observed
            .and_then(|event| event.payload["log_dir"].as_str())
            .map(String::from);
        let detail = match state {
            "baseline_recorded" => "A failed baseline is recorded for the current example oracle.".into(),
            "baseline_passed" => "This check passed before the change, so it cannot demonstrate the change's effect. Rewrite the example or check.".into(),
            "baseline_inconclusive" => format!(
                "Read the check logs at {} and verify that every check file is readable and at most 4 MiB.",
                log_dir.as_deref().unwrap_or(".awr/artifacts/checks/")
            ),
            _ => format!(
                "Run awr work check {} --example {} --phase baseline --session <session-id> --expected-revision {revision}",
                work.item.meta.external_key, oracle.example
            ),
        };
        baselines.push(AdmissionBaseline {
            example: oracle.example.clone(),
            state,
            example_oracle: oracle.oracle.clone(),
            event: observed.map(|event| event.id),
            log_dir,
            detail,
        });
    }
    Ok(baselines)
}

/// The branch an agent-declared decision applies to: the project's current one, like a read.
pub(crate) fn current_branch(conn: &Connection, project: Id) -> Result<Option<Id>> {
    conn.query_row(
        "SELECT current_branch_id FROM projects WHERE id=?1",
        [project.to_string()],
        |r| r.get::<_, Option<String>>(0),
    )
    .map_err(db_error)?
    .map(|id| {
        id.parse()
            .map_err(|_| Error::Storage("invalid current branch identity".into()))
    })
    .transpose()
}

pub(crate) fn event_at(conn: &Connection, project: Id, id: Id) -> Result<Option<Event>> {
    conn.query_row(
        "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND id=?2",
        params![project.to_string(), id.to_string()],
        event_row,
    )
    .optional()
    .map_err(db_error)
}

fn recorded(event: &Event) -> Result<RecordedAdmission> {
    let broken = || Error::Storage("invalid admission event".into());
    Ok(RecordedAdmission {
        event: event.id,
        decision: serde_json::from_value(event.payload["decision"].clone())
            .map_err(|_| broken())?,
        bound: serde_json::from_value(event.payload["bound"].clone()).map_err(|_| broken())?,
        reason: event.payload["reason"].as_str().map(String::from),
        proposer_session: event.payload["proposer_session"]
            .as_str()
            .and_then(|id| id.parse().ok()),
        decider_session: event.session_id,
    })
}

/// The newest admission event of a work on a branch.
fn latest_admission(
    conn: &Connection,
    project: Id,
    work: Id,
    branch: Option<Id>,
) -> Result<Option<(Event, RecordedAdmission)>> {
    let event = conn
        .query_row(
            "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type=?4 ORDER BY project_revision DESC,created_at DESC,id DESC LIMIT 1",
            params![
                project.to_string(),
                work.to_string(),
                branch.map(|id| id.to_string()),
                ADMISSION_EVENT
            ],
            event_row,
        )
        .optional()
        .map_err(db_error)?;
    event
        .map(|event| recorded(&event).map(|record| (event, record)))
        .transpose()
}

/// Derive the admission state of a work. Work no policy governs costs one config lookup.
pub(crate) fn status_at(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<AdmissionStatus> {
    let key = &work.item.meta.external_key;
    let (policy, value) = match read_policy(work) {
        PolicyRead::Absent => return Ok(AdmissionStatus::ungoverned(key, "not_governed")),
        PolicyRead::Invalid => return Ok(AdmissionStatus::policy_invalid(key)),
        PolicyRead::Valid(policy, value) => (policy, value),
    };
    if !policy.governs(&work.item.tags) {
        return Ok(AdmissionStatus::ungoverned(key, mode_name(&policy)));
    }
    let definition = work_definition_report(conn, project, key)?;
    let (current, oracles) = current_binding(conn, project, work, &value, &definition)?;
    let baselines = baselines_at(conn, project, work, branch, &oracles)?;
    let latest = latest_admission(conn, project, work.item.meta.id, branch)?;
    Ok(evaluate_admission(&AdmissionInputs {
        work: key,
        tags: &work.item.tags,
        completed: work.item.status == WorkStatus::Completed,
        policy: Some(&policy),
        current: &current,
        latest: latest.as_ref().map(|(_, record)| record),
        definition: &DefinitionSummary::from(&definition),
        baselines: &baselines,
    }))
}

/// Readiness diagnostics: only an enforced policy turns the derived reasons into blockers.
/// Work that cannot be claimed anyway owes none, and a state that cannot be established is a
/// reason of its own (`admission_unknown`) instead of an error that would break every listing.
pub(crate) fn diagnostics(
    conn: &Connection,
    project: Id,
    work: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<Vec<(String, String)>> {
    match read_policy(work) {
        PolicyRead::Absent => return Ok(vec![]),
        PolicyRead::Invalid => {
            return Ok(
                AdmissionStatus::policy_invalid(&work.item.meta.external_key)
                    .diagnostics
                    .into_iter()
                    .map(|d| (d.code.to_string(), d.detail))
                    .collect(),
            );
        }
        PolicyRead::Valid(policy, _) if !policy.enforced() || !policy.governs(&work.item.tags) => {
            return Ok(vec![]);
        }
        PolicyRead::Valid(..) => {}
    }
    if matches!(
        work.item.status,
        WorkStatus::Completed | WorkStatus::Cancelled
    ) {
        return Ok(vec![]);
    }
    let decisions = || -> Result<Vec<(String, String)>> {
        let status = status_at(conn, project, work, branch)?;
        let baseline_gap = baseline_diagnostic(&status.baselines);
        // Stale admission requires a new decision first. Its baseline details remain visible
        // in status, while readiness keeps the stale reason when it sorts diagnostic codes.
        let stale = status.state == "stale";
        let mut found: Vec<(String, String)> = status
            .diagnostics
            .into_iter()
            .filter(|diagnostic| !(stale && baseline_gap.as_ref() == Some(diagnostic)))
            .map(|d| (d.code.to_string(), d.detail))
            .collect();
        found.extend(crate::containment::diagnostics_for_child(
            conn, project, work, branch,
        )?);
        // The gaps of the work itself put its own admission in question.
        found.extend(crate::gap::admission_suspension(
            conn, project, work, branch,
        )?);
        Ok(found)
    };
    let mut found = match decisions() {
        Ok(found) => found,
        Err(error) => vec![(
            "admission_unknown".into(),
            format!(
                "the admission state could not be established ({}); refresh the sources and retry",
                error.code()
            ),
        )],
    };
    // A work whose records call for a second look owes a current decision of its own.
    found.extend(crate::reassessment::diagnostics_for(
        conn, project, work, branch,
    )?);
    Ok(found)
}

/// The error a blocked claim, registration or start returns; none when the work is free to run.
pub(crate) fn require(
    conn: &Connection,
    project: Id,
    work_id: Id,
    branch: Option<Id>,
) -> Result<()> {
    require_with(conn, project, work_id, branch, None)
}

/// Like [`require`] for an execution under `operation_key`: the experiment of an open gap is how
/// that gap gets closed, so the gap never holds it back.
pub(crate) fn require_with(
    conn: &Connection,
    project: Id,
    work_id: Id,
    branch: Option<Id>,
    operation_key: Option<&str>,
) -> Result<()> {
    let key: Option<String> = conn
        .query_row(
            "SELECT external_key FROM work_items WHERE project_id=?1 AND id=?2 AND active=1",
            params![project.to_string(), work_id.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let Some(key) = key else {
        return Ok(());
    };
    let work: Projected<WorkItem> = projection(conn, project, EntityKind::WorkItem, &key)?;
    let mut diagnostics = diagnostics(conn, project, &work, branch)?;
    if let Some(key) = operation_key
        && crate::gap::is_open_experiment(conn, project, work_id, branch, key)?
    {
        diagnostics.retain(|(code, _)| code != "admission_suspended");
    }
    if let Some((code, detail)) = diagnostics.first() {
        return Err(Error::Admission {
            code: static_code(code),
            message: detail.clone(),
            diagnostics: json!(
                diagnostics
                    .iter()
                    .map(|(code, detail)| json!({"code": code, "detail": detail}))
                    .collect::<Vec<_>>()
            ),
        });
    }
    Ok(())
}

/// Diagnostic codes are a closed set; map a derived code back to its static spelling.
pub(crate) fn static_code(code: &str) -> &'static str {
    [
        "admission_policy_invalid",
        "admission_missing",
        "admission_pending",
        "admission_denied",
        "admission_stale",
        "admission_obligations_unmet",
        "admission_unknown",
        "containment_missing",
        "containment_pending",
        "containment_denied",
        "containment_stale",
        "containment_unchecked",
        "containment_not_authorized",
        "reassessment_required",
        "reassessment_stale",
        "reassessment_unknown",
        "reassessment_budget_required",
        "reassessment_budget_stale",
        "reassessment_budget_escalated",
        "reassessment_not_authorized",
        "admission_suspended",
        "containment_suspended",
    ]
    .into_iter()
    .find(|known| *known == code)
    .unwrap_or("admission_missing")
}

impl Store {
    /// The admission status of one work as currently indexed, on one branch.
    pub fn admission_status(
        &self,
        project: Id,
        key: &str,
        branch: Option<Id>,
    ) -> Result<AdmissionStatus> {
        self.project(project)?;
        let work: Projected<WorkItem> = projection(&self.conn, project, EntityKind::WorkItem, key)?;
        status_at(&self.conn, project, &work, branch)
    }

    /// A replay of an earlier request: the event that already carries this request key.
    pub fn admission_by_key(&self, project: Id, work: Id, key: &str) -> Result<Option<Event>> {
        self.conn
            .query_row(
                "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND event_type=?3 AND json_extract(payload_json,'$.request_key')=?4 ORDER BY project_revision LIMIT 1",
                params![project.to_string(), work.to_string(), ADMISSION_EVENT, key],
                event_row,
            )
            .optional()
            .map_err(db_error)
    }

    /// Record one admission event. Everything the decision binds is read inside the transaction,
    /// so a scope that moved since the decider read it, or a stale write, is rejected.
    pub fn record_admission(
        &mut self,
        project: Id,
        request: &AdmissionRequest,
    ) -> Result<(Event, AdmissionStatus)> {
        request.validate()?;
        let event = EventDraft::new(ADMISSION_EVENT, "Recorded work admission decision");
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
                        "refresh the work source before recording admission".into(),
                    ));
                }
                let (policy, value) = match read_policy(&work) {
                    PolicyRead::Valid(policy, value) => (policy, value),
                    PolicyRead::Invalid => {
                        return Err(reject(
                            "admission_policy_invalid",
                            "the admission_policy option of the owning source cannot be used",
                        ));
                    }
                    PolicyRead::Absent => {
                        return Err(reject(
                            "admission_not_governed",
                            "the owning source declares no admission_policy",
                        ));
                    }
                };
                if !policy.governs(&work.item.tags) {
                    return Err(reject(
                        "admission_not_governed",
                        "the work is outside the scope of the admission policy",
                    ));
                }
                let branch = match &session {
                    Some(session) => session.branch_id,
                    None => current_branch(tx, project)?,
                };
                let definition = work_definition_report(tx, project, &request.work)?;
                let (current, oracles) = current_binding(tx, project, &work, &value, &definition)?;
                if request.contract_fingerprint != current.contract_fingerprint {
                    return Err(Error::SourceConflict(
                        "work contract changed; read the current assessment before deciding".into(),
                    ));
                }
                let latest = latest_admission(tx, project, work.item.meta.id, branch)?;
                let review_required = policy.requires_independent_review(&work.item.tags);
                if review_required
                    && request.decision == AdmissionDecision::Proposed
                    && decider_session.is_none()
                {
                    return Err(reject(
                        "admission_review_not_independent",
                        "proposing work that needs independent review requires a session, so that the reviewer can be shown to differ",
                    ));
                }
                // What the decision answers: an earlier proposal, or nothing for an inline decision.
                let (judgment, proposal, proposer) = match request.proposal_event {
                    None => (request.judgment.clone(), None, None),
                    Some(id) => {
                        let proposal = event_at(tx, project, id)?
                            .filter(|e| {
                                e.event_type == ADMISSION_EVENT
                                    && e.work_item_id == Some(work.item.meta.id)
                                    && e.branch_id == branch
                                    && e.payload["decision"] == json!(AdmissionDecision::Proposed)
                            })
                            .ok_or_else(|| {
                                Error::InvalidInput(
                                    "proposal_event is not an admission proposal of this work on this branch".into(),
                                )
                            })?;
                        if latest.as_ref().map(|(event, _)| event.id) != Some(proposal.id) {
                            return Err(Error::SourceConflict(
                                "a newer admission event exists; answer the latest proposal".into(),
                            ));
                        }
                        let changed = recorded(&proposal)?.bound.changed_from(&current);
                        if !changed.is_empty() {
                            return Err(reject(
                                "admission_stale",
                                format!("the proposal describes an earlier {}; propose again", changed.join(", ")),
                            ));
                        }
                        let judgment: Option<AdmissionJudgment> =
                            serde_json::from_value(proposal.payload["judgment"].clone()).ok();
                        (judgment, Some(proposal.id), proposal.session_id)
                    }
                };
                if request.decision == AdmissionDecision::Approved {
                    if review_required
                        && (decider_session.is_none()
                            || proposer.is_none_or(|p| Some(p) == decider_session))
                    {
                        return Err(reject(
                            "admission_review_not_independent",
                            "this work needs a review by a session other than the one that proposed it",
                        ));
                    }
                    let gaps: Vec<&str> = if definition.state == "not_declared" {
                        vec!["definition_not_declared"]
                    } else {
                        definition.gaps.iter().map(|gap| gap.code).collect()
                    };
                    if !gaps.is_empty() {
                        return Err(reject(
                            "admission_obligations_unmet",
                            format!("definition gaps: {}", gaps.join(", ")),
                        ));
                    }
                    if policy.enforced() {
                        let baselines = baselines_at(tx, project, &work, branch, &oracles)?;
                        if let Some(diagnostic) = baseline_diagnostic(&baselines) {
                            return Err(reject(diagnostic.code, diagnostic.detail));
                        }
                        if !oracles.is_empty() && current.oracle_fingerprint.is_none() {
                            return Err(reject("admission_stale", "the current oracle is unknown; restore readable check files before approving"));
                        }
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
                    "judgment": judgment,
                    "proposal_event": proposal,
                    "proposer_session": proposer,
                    "decider_agent": decider_agent,
                    "independent_review_required": review_required,
                    "reason": request.reason,
                });
                if serde_json::to_vec(&event.payload)?.len() > 64 * 1024 {
                    return Err(Error::InvalidInput(
                        "admission receipt exceeds 64 KiB".into(),
                    ));
                }
                ensure_public_value(&event.payload)?;
                Ok(branch)
            },
        )?;
        let status = self.admission_status(project, &request.work, branch)?;
        Ok((event, status))
    }
}
