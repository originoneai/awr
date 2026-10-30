//! Containment decisions as events, and the readiness diagnostics derived from them.
//!
//! Splitting a parent, widening its scope or changing its dependencies is a change of the
//! parent's set of children. A decision is a `work.containment` event bound to the parent's
//! contract, the exact children (their definitions, paths, acceptance and dependencies) and the
//! dependencies between them; it needs no table and no migration. The state is re-derived on
//! every read, so a child added, removed or edited, or a parent whose contract moved, makes an
//! earlier approval stale instead of silently covering the new set. Governed children are not
//! admitted until the decision on their parent's set is current.
use crate::{
    Store,
    admission::{PolicyRead, current_branch, digest, event_at, read_policy, reject},
    contract::work_contract,
    db_error,
    events::event_row,
    query::{projection, projections},
    session::{require_active, session_at},
    work::dependency_links,
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

/// The children of a parent: active work whose definition names it as parent, ordered by key.
fn children_of(conn: &Connection, project: Id, parent: &str) -> Result<Vec<Projected<WorkItem>>> {
    let mut children: Vec<Projected<WorkItem>> =
        projections::<WorkItem>(conn, project, EntityKind::WorkItem, None)?
            .into_iter()
            .filter(|w| {
                !w.item.archived
                    && w.item.definition.as_ref().and_then(|d| d.parent.as_deref()) == Some(parent)
            })
            .collect();
    children.sort_by(|a, b| a.item.meta.external_key.cmp(&b.item.meta.external_key));
    Ok(children)
}

/// Is `path` the same as `scope` or inside it?
fn within(path: &str, scope: &str) -> bool {
    let (path, scope) = (path.trim_end_matches('/'), scope.trim_end_matches('/'));
    path == scope || path.starts_with(&format!("{scope}/"))
}

fn binding_at(
    conn: &Connection,
    project: Id,
    parent: &Projected<WorkItem>,
    children: &[Projected<WorkItem>],
) -> Result<(ContainmentBinding, Vec<(String, String)>)> {
    let keys: Vec<String> = children
        .iter()
        .map(|c| c.item.meta.external_key.clone())
        .collect();
    let facts: Vec<_> = children
        .iter()
        .map(|c| {
            json!({"key": c.item.meta.external_key, "title": c.item.title,
                   "definition": c.item.definition, "paths": c.item.paths,
                   "acceptance": c.item.acceptance, "tags": c.item.tags,
                   "spec_refs": c.item.spec_refs})
        })
        .collect();
    let mut edges: Vec<(String, String, bool)> = dependency_links(conn, project)?
        .into_iter()
        .filter(|e| keys.contains(&e.from_key))
        .map(|e| (e.from_key, e.to_key, e.required))
        .collect();
    edges.sort();
    let failures = children
        .iter()
        .flat_map(|child| {
            let scope = &parent.item.paths;
            child
                .item
                .paths
                .iter()
                .filter(move |path| {
                    !scope.is_empty() && !scope.iter().any(|allowed| within(path, allowed))
                })
                .map(|path| {
                    (
                        "child_outside_parent_scope".to_string(),
                        format!(
                            "{} touches {path}, which is outside the paths of {}",
                            child.item.meta.external_key, parent.item.meta.external_key
                        ),
                    )
                })
        })
        .collect();
    Ok((
        ContainmentBinding {
            parent_contract: work_contract(conn, project, &parent.item.meta.external_key)?
                .fingerprint,
            change_set: digest(&json!(facts))?,
            dependencies: digest(&json!(edges))?,
            children: keys,
        },
        failures,
    ))
}

fn supervisors(parent: &Projected<WorkItem>) -> Vec<String> {
    match read_policy(parent) {
        PolicyRead::Valid(policy, _) => policy.supervisors,
        _ => vec![],
    }
}

fn recorded(event: &Event) -> Result<RecordedContainment> {
    let broken = || Error::Storage("invalid containment event".into());
    Ok(RecordedContainment {
        event: event.id,
        decision: serde_json::from_value(event.payload["decision"].clone())
            .map_err(|_| broken())?,
        bound: serde_json::from_value(event.payload["bound"].clone()).map_err(|_| broken())?,
        reason: event.payload["reason"].as_str().map(String::from),
        decider_agent: event.payload["decider_agent"]
            .as_str()
            .ok_or_else(broken)?
            .to_string(),
    })
}

fn latest_containment(
    conn: &Connection,
    project: Id,
    parent: Id,
    branch: Option<Id>,
) -> Result<Option<(Event, RecordedContainment)>> {
    let event = conn
        .query_row(
            "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND branch_id IS ?3 AND event_type=?4 ORDER BY project_revision DESC,created_at DESC,id DESC LIMIT 1",
            params![
                project.to_string(),
                parent.to_string(),
                branch.map(|id| id.to_string()),
                CONTAINMENT_EVENT
            ],
            event_row,
        )
        .optional()
        .map_err(db_error)?;
    event
        .map(|event| recorded(&event).map(|record| (event, record)))
        .transpose()
}

/// The containment state of a parent's children on a branch.
pub(crate) fn status_at(
    conn: &Connection,
    project: Id,
    parent: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<ContainmentStatus> {
    let key = &parent.item.meta.external_key;
    let children = children_of(conn, project, key)?;
    if children.is_empty() {
        return Ok(ContainmentStatus::no_children(key));
    }
    let (current, failures) = binding_at(conn, project, parent, &children)?;
    let latest = latest_containment(conn, project, parent.item.meta.id, branch)?;
    let supervisors = supervisors(parent);
    Ok(evaluate_containment(&ContainmentInputs {
        parent: key,
        supervisors: &supervisors,
        current: &current,
        failures: &failures,
        latest: latest.as_ref().map(|(_, record)| record),
    }))
}

/// Readiness diagnostics of a governed child: its parent's set of children needs a current
/// decision before the child can be admitted. Work without a declared parent owes none.
pub(crate) fn diagnostics_for_child(
    conn: &Connection,
    project: Id,
    child: &Projected<WorkItem>,
    branch: Option<Id>,
) -> Result<Vec<(String, String)>> {
    let Some(parent_key) = child
        .item
        .definition
        .as_ref()
        .and_then(|d| d.parent.as_deref())
    else {
        return Ok(vec![]);
    };
    let parent: Projected<WorkItem> =
        match projection(conn, project, EntityKind::WorkItem, parent_key) {
            Ok(parent) => parent,
            // A parent that does not exist is the child's own definition gap.
            Err(Error::NotFound(_)) => return Ok(vec![]),
            Err(error) => return Err(error),
        };
    Ok(status_at(conn, project, &parent, branch)?
        .diagnostics
        .into_iter()
        .map(|d| (d.code.to_string(), d.detail))
        .collect())
}

impl Store {
    /// The containment status of one parent as currently indexed, on one branch.
    pub fn containment_status(
        &self,
        project: Id,
        parent: &str,
        branch: Option<Id>,
    ) -> Result<ContainmentStatus> {
        self.project(project)?;
        let work: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, parent)?;
        status_at(&self.conn, project, &work, branch)
    }

    /// The change-set fingerprint a decider reviews, with the children it covers.
    pub fn containment_change_set(&self, project: Id, parent: &str) -> Result<ContainmentBinding> {
        self.project(project)?;
        let work: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, parent)?;
        let children = children_of(&self.conn, project, parent)?;
        Ok(binding_at(&self.conn, project, &work, &children)?.0)
    }

    /// The containment block `work show` and `work prepare` report: only for a parent that has
    /// children in a source that declares an admission policy.
    pub fn containment_block(
        &self,
        project: Id,
        parent: &str,
        branch: Option<Id>,
    ) -> Result<Option<ContainmentStatus>> {
        self.project(project)?;
        let work: Projected<WorkItem> =
            projection(&self.conn, project, EntityKind::WorkItem, parent)?;
        if matches!(read_policy(&work), PolicyRead::Absent) {
            return Ok(None);
        }
        let status = status_at(&self.conn, project, &work, branch)?;
        Ok((status.state != "no_children").then_some(status))
    }

    /// A replay of an earlier request: the event that already carries this request key.
    pub fn containment_by_key(&self, project: Id, parent: Id, key: &str) -> Result<Option<Event>> {
        self.conn
            .query_row(
                "SELECT id,project_id,work_item_id,session_id,branch_id,event_type,importance,summary,payload_json,project_revision,created_at FROM events WHERE project_id=?1 AND work_item_id=?2 AND event_type=?3 AND json_extract(payload_json,'$.request_key')=?4 ORDER BY project_revision LIMIT 1",
                params![project.to_string(), parent.to_string(), CONTAINMENT_EVENT, key],
                event_row,
            )
            .optional()
            .map_err(db_error)
    }

    /// Record one containment event. The set it binds is read inside the transaction, so a
    /// change of the children since the decider read them, or a stale write, is rejected.
    pub fn record_containment(
        &mut self,
        project: Id,
        request: &ContainmentRequest,
    ) -> Result<(Event, ContainmentStatus)> {
        request.validate()?;
        let event = EventDraft::new(CONTAINMENT_EVENT, "Recorded containment decision");
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
                let parent: Projected<WorkItem> =
                    projection(tx, project, EntityKind::WorkItem, &request.parent)?;
                if parent.source.freshness != Freshness::Fresh {
                    return Err(Error::SourceStale(
                        "refresh the parent's source before recording containment".into(),
                    ));
                }
                if matches!(read_policy(&parent), PolicyRead::Absent) {
                    return Err(reject(
                        "admission_not_governed",
                        "the owning source declares no admission_policy",
                    ));
                }
                let branch = match &session {
                    Some(session) => session.branch_id,
                    None => current_branch(tx, project)?,
                };
                let children = children_of(tx, project, &request.parent)?;
                if children.is_empty() {
                    return Err(Error::InvalidInput(
                        "no work names this work as its parent, so there is nothing to contain".into(),
                    ));
                }
                let (current, failures) = binding_at(tx, project, &parent, &children)?;
                if request.change_set_fingerprint != current.change_set {
                    return Err(Error::SourceConflict(
                        "the children changed; read the current change set before deciding".into(),
                    ));
                }
                let latest = latest_containment(tx, project, parent.item.meta.id, branch)?;
                let (judgment, proposal) = match request.proposal_event {
                    None => (request.judgment.clone(), None),
                    Some(id) => {
                        let proposal = event_at(tx, project, id)?
                            .filter(|e| {
                                e.event_type == CONTAINMENT_EVENT
                                    && e.work_item_id == Some(parent.item.meta.id)
                                    && e.branch_id == branch
                                    && e.payload["decision"] == json!(AdmissionDecision::Proposed)
                            })
                            .ok_or_else(|| {
                                Error::InvalidInput(
                                    "proposal_event is not a containment proposal of this work on this branch".into(),
                                )
                            })?;
                        if latest.as_ref().map(|(event, _)| event.id) != Some(proposal.id) {
                            return Err(Error::SourceConflict(
                                "a newer containment event exists; answer the latest proposal".into(),
                            ));
                        }
                        if recorded(&proposal)?.bound != current {
                            return Err(reject(
                                "containment_stale",
                                "the proposal describes an earlier set of children; propose again",
                            ));
                        }
                        let judgment: Option<ContainmentJudgment> =
                            serde_json::from_value(proposal.payload["judgment"].clone()).ok();
                        (judgment, Some(proposal.id))
                    }
                };
                if request.decision == AdmissionDecision::Approved {
                    if !failures.is_empty() {
                        return Err(reject(
                            "containment_unchecked",
                            failures
                                .iter()
                                .map(|(code, detail)| format!("{code}: {detail}"))
                                .collect::<Vec<_>>()
                                .join("; "),
                        ));
                    }
                    let allowed = supervisors(&parent);
                    if !allowed.is_empty() && !allowed.contains(&decider_agent) {
                        return Err(reject(
                            "containment_not_authorized",
                            format!(
                                "{decider_agent} is not among the supervisors the policy authorizes to approve a containment"
                            ),
                        ));
                    }
                }
                event.work_item_id = Some(parent.item.meta.id);
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
                    "decider_agent": decider_agent,
                    "reason": request.reason,
                });
                if serde_json::to_vec(&event.payload)?.len() > 64 * 1024 {
                    return Err(Error::InvalidInput(
                        "containment receipt exceeds 64 KiB".into(),
                    ));
                }
                ensure_public_value(&event.payload)?;
                Ok(branch)
            },
        )?;
        let status = self.containment_status(project, &request.parent, branch)?;
        Ok((event, status))
    }
}
