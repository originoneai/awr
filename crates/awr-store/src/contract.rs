//! The scope contract that a management assessment or an admission decision binds. It is computed
//! on a connection, so the same function serves the read queries and readiness inside a
//! transaction. Progress, next action, evidence additions and unrelated source edits never enter it.
use crate::{
    Store,
    query::{projection, projections},
    work::{dependency_links, goal_links},
    workstream::{binding_at, enabled_at, goal_keys_at},
};
use awr_core::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct WorkContract {
    pub fingerprint: String,
    /// The fingerprint before goal contents were bound; a receipt carrying it is reassessed once.
    pub legacy_fingerprint: String,
    pub goal_diagnostics: Vec<&'static str>,
    pub inherited_goals: bool,
    pub goal_bindings: usize,
}

fn fingerprint(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn terminal_goal(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "completed" | "done" | "cancelled" | "canceled" | "archived" | "retired"
    )
}

fn goal_binding(goal: &Projected<Goal>, required: Option<bool>) -> Value {
    json!({
        "id": goal.item.meta.id,
        "key": goal.item.meta.external_key,
        "title": goal.item.title,
        "status": goal.item.status,
        "summary": goal.item.summary,
        "success_criteria": goal.item.success_criteria,
        "required": required,
    })
}

pub(crate) fn work_contract(
    conn: &rusqlite::Connection,
    project: Id,
    key: &str,
) -> Result<WorkContract> {
    let projected: Projected<WorkItem> = projection(conn, project, EntityKind::WorkItem, key)?;
    let work = &projected.item;
    let mut dependency_links = dependency_links(conn, project)?
        .into_iter()
        .filter(|e| e.from_key == key)
        .collect::<Vec<_>>();
    dependency_links
        .sort_by(|a, b| (&a.to_key, a.required, a.id).cmp(&(&b.to_key, b.required, b.id)));
    let dependencies = dependency_links
        .iter()
        .map(|e| json!({"key":e.to_key,"required":e.required}))
        .collect::<Vec<_>>();
    let mut goal_links = goal_links(conn, project)?
        .into_iter()
        .filter(|e| e.from_key == key)
        .collect::<Vec<_>>();
    goal_links.sort_by(|a, b| (&a.to_key, a.required, a.id).cmp(&(&b.to_key, b.required, b.id)));
    let scoped_goal_keys = if enabled_at(conn, project)? {
        let stream = binding_at(conn, project, work.meta.id)?;
        Some(
            goal_keys_at(conn, project, stream)?
                .into_iter()
                .collect::<BTreeSet<_>>(),
        )
    } else {
        None
    };
    let mut goal_diagnostics = Vec::new();
    let inherited_goals = goal_links.is_empty();
    let (goal_selection, mut goals) = if inherited_goals {
        let mut goals = Vec::new();
        if let Some(keys) = &scoped_goal_keys {
            for key in keys {
                match projection::<Goal>(conn, project, EntityKind::Goal, key) {
                    Ok(goal) if !terminal_goal(&goal.item.status) => {
                        if goal.source.freshness != Freshness::Fresh {
                            goal_diagnostics.push("goal_context_not_fresh");
                        }
                        goals.push(goal_binding(&goal, None));
                    }
                    Ok(_) => {}
                    Err(Error::NotFound(_)) => {
                        goal_diagnostics.push("goal_context_unavailable");
                    }
                    Err(error) => return Err(error),
                }
            }
            ("workstream_goal_references", goals)
        } else {
            for goal in projections::<Goal>(conn, project, EntityKind::Goal, None)?
                .into_iter()
                .filter(|goal| {
                    goal.source.role == "primary"
                        && matches!(goal.source.domain.as_str(), "goal" | "ledger")
                        && !terminal_goal(&goal.item.status)
                })
            {
                if goal.source.freshness != Freshness::Fresh {
                    goal_diagnostics.push("goal_context_not_fresh");
                }
                goals.push(goal_binding(&goal, None));
            }
            ("project_primary_goal_sources", goals)
        }
    } else {
        let mut goals = Vec::new();
        for link in &goal_links {
            let scoped_unavailable = scoped_goal_keys
                .as_ref()
                .is_some_and(|keys| !keys.contains(&link.to_key));
            if scoped_unavailable {
                goal_diagnostics.push("linked_goal_unavailable");
                goals.push(json!({"id":null,"key":link.to_key,"required":link.required,
                    "title":null,"status":null,"summary":null,"success_criteria":null}));
                continue;
            }
            match projection::<Goal>(conn, project, EntityKind::Goal, &link.to_key) {
                Ok(goal) => {
                    if goal.source.freshness != Freshness::Fresh {
                        goal_diagnostics.push("linked_goal_source_not_fresh");
                    }
                    goals.push(goal_binding(&goal, Some(link.required)));
                }
                Err(Error::NotFound(_)) => {
                    goal_diagnostics.push(if scoped_goal_keys.is_some() {
                        "linked_goal_unavailable"
                    } else {
                        "linked_goal_not_found"
                    });
                    goals.push(json!({"id":null,"key":link.to_key,"required":link.required,
                        "title":null,"status":null,"summary":null,"success_criteria":null}));
                }
                Err(error) => return Err(error),
            }
        }
        ("work_goal_links", goals)
    };
    goals.sort_by(|a, b| {
        (a["key"].as_str(), a["required"].as_bool())
            .cmp(&(b["key"].as_str(), b["required"].as_bool()))
    });
    goal_diagnostics.sort_unstable();
    goal_diagnostics.dedup();
    let legacy_goals = goal_links
        .iter()
        .map(|edge| json!({"key":edge.to_key,"required":edge.required}))
        .collect::<Vec<_>>();
    let ordinary_policy = &projected.source.config["adapter_options"]["ordinary_work_policy"];
    let mut legacy_contract = json!({"id":work.meta.id,
        "title":work.title,"summary":work.summary,"kind":work.kind,"acceptance":work.acceptance,
        "paths":work.paths,"tags":work.tags,"goals":legacy_goals,"dependencies":dependencies,
        "ordinary_work_policy":ordinary_policy});
    if !work.spec_refs.is_empty() {
        legacy_contract["spec_refs"] = json!(work.spec_refs);
    }
    let legacy_fingerprint = fingerprint(&serde_json::to_vec(&legacy_contract)?);
    let mut contract = json!({"id":work.meta.id,
        "title":work.title,"summary":work.summary,"kind":work.kind,"acceptance":work.acceptance,
        "paths":work.paths,"tags":work.tags,
        "goals":{"selection":goal_selection,"bindings":goals},"dependencies":dependencies,
        "ordinary_work_policy":ordinary_policy});
    if !work.spec_refs.is_empty() {
        contract["spec_refs"] = json!(work.spec_refs);
    }
    if let Some(definition) = &work.definition {
        contract["definition"] = json!(definition);
    }
    Ok(WorkContract {
        fingerprint: fingerprint(&serde_json::to_vec(&contract)?),
        legacy_fingerprint,
        goal_diagnostics,
        inherited_goals,
        goal_bindings: goals.len(),
    })
}

impl Store {
    /// The scope contract of one work as currently indexed; see [`WorkContract`].
    pub fn work_contract(&self, project: Id, key: &str) -> Result<WorkContract> {
        self.project(project)?;
        work_contract(&self.conn, project, key)
    }
}
