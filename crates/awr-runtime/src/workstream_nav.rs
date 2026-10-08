//! Read-only mainline navigation snapshot shared by CLI, MCP and Inspector (WS-042).
//!
//! Surfaces select a scope and render authoritative facts already produced by
//! WS-040 accounting, WS-041 usage/time, WS-043 ETA/checkpoints, WS-018
//! review/blockers and WS-030/031/032 dependency graphs. This module does not
//! invent completion rates, mutate team state, or bypass authorization — callers
//! must authenticate the project / workstream before invoking it. Full Team Web
//! collaboration writes are delivered by the Team Web entry (WS-044).
use awr_core::{
    ACTION_GUIDANCE_MAX_BYTES, ActionGuidance, Edge, EtaComponents, Project, Projected, Result,
    TaskResponsibility, WorkItem, Workstream, WorkstreamAccounting, WorkstreamCatalog,
};
use awr_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const MAINLINE_NAV_SCHEMA_VERSION: u32 = 1;
pub const MAINLINE_NAV_PROTOCOL: &str = "awr-mainline-nav";

/// Scope selectors for mainline navigation. Empty selectors mean project-wide.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainlineNavScope {
    pub workstream: Option<String>,
    #[serde(default)]
    pub work: Vec<String>,
    pub goal: Option<String>,
    pub milestone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplainableWait {
    pub kind: String,
    pub summary: String,
    pub basis: String,
    pub release_condition: String,
}

/// Compact work facts used by the pure assembler (CLI/MCP/Inspector share this).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainlineNavWorkFact {
    pub key: String,
    pub id: String,
    pub title: String,
    pub status: String,
    pub owner: Option<String>,
    pub blocker: Option<String>,
    pub next_action: String,
    pub milestone: Option<String>,
    pub tags: Vec<String>,
    pub acceptance: Vec<String>,
    pub archived: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MainlineNavExtras {
    pub accounting: Option<WorkstreamAccounting>,
    pub eta_by_work: BTreeMap<String, EtaComponents>,
    pub responsibility_by_work: BTreeMap<String, TaskResponsibility>,
    pub waits_by_work: BTreeMap<String, Vec<ExplainableWait>>,
    /// Newest event time (milliseconds) per work key; `mainline_nav` loads it from the store.
    pub last_event_at_by_work: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainlineNavNode {
    pub work_key: String,
    pub title: String,
    pub status: String,
    pub workstream_id: Option<String>,
    pub person_responsibility: Option<String>,
    pub agent_execution: Option<Value>,
    pub stage_acceptance_window: Option<Value>,
    pub explainable_waits: Vec<ExplainableWait>,
    pub blocker: Option<String>,
    pub next_action: String,
    pub outcomes: Vec<String>,
    /// Newest event of this work in milliseconds, over all branches and sessions; null when it has none.
    #[serde(default)]
    pub last_event_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainlineNavEdge {
    pub from: String,
    pub to: String,
    pub required: bool,
    /// Concrete task outcome the consumer waits on (not a vague milestone blob).
    pub outcome: String,
    pub cross_workstream: bool,
    pub from_workstream_id: Option<String>,
    pub to_workstream_id: Option<String>,
}

fn short(text: &str) -> String {
    awr_core::public_summary(text, 240)
        .unwrap_or_else(|_| awr_core::SENSITIVE_CONTENT_WITHHELD.into())
}

fn work_fact(work: &Projected<WorkItem>) -> MainlineNavWorkFact {
    MainlineNavWorkFact {
        key: work.item.meta.external_key.clone(),
        id: work.item.meta.id.to_string(),
        title: work.item.title.clone(),
        status: serde_json::to_value(work.item.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{:?}", work.item.status).to_ascii_lowercase()),
        owner: work.item.owner.clone(),
        blocker: work.item.blocker.clone(),
        next_action: work.item.next_action.clone(),
        milestone: work.item.milestone.clone(),
        tags: work.item.tags.clone(),
        acceptance: work.item.acceptance.clone(),
        archived: work.item.archived,
    }
}

fn concrete_outcomes(work: &MainlineNavWorkFact) -> Vec<String> {
    let mut out: Vec<String> = work
        .acceptance
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(short)
        .collect();
    if out.is_empty() {
        out.push(format!(
            "accepted completion of {} — {}",
            work.key,
            short(&work.title)
        ));
    }
    out
}

fn execution_value(execution: &awr_core::ExecutionInstance) -> Value {
    match execution {
        awr_core::ExecutionInstance::Person { person_id } => json!({
            "kind": "person",
            "person_id": person_id.as_str(),
        }),
        awr_core::ExecutionInstance::AgentRun {
            person_id,
            agent_id,
            binding_id,
        } => json!({
            "kind": "agent_run",
            "person_id": person_id.as_str(),
            "agent_id": agent_id,
            "binding_id": binding_id,
        }),
    }
}

fn id_matches(stored: &str, selector: &str) -> bool {
    stored == selector || stored.ends_with(selector) || selector.ends_with(stored)
}

fn workstream_value(stream: &Workstream) -> Value {
    json!({
        "id": stream.id,
        "external_key": stream.external_key,
        "title": stream.title,
        "state": stream.state,
        "authority_version": stream.authority_version,
        "goal_keys": stream.goal_keys,
        "resolved": true,
    })
}

fn resolve_selected_workstream(
    scope: &MainlineNavScope,
    catalog: Option<&WorkstreamCatalog>,
    ownership: &BTreeMap<String, String>,
    selected_keys: &BTreeSet<&str>,
) -> Value {
    if let Some(sel) = &scope.workstream {
        if let Some(catalog) = catalog {
            if let Some(stream) = catalog.workstreams.iter().find(|s| {
                s.id.to_string() == *sel
                    || s.external_key == *sel
                    || id_matches(&s.id.to_string(), sel)
            }) {
                return workstream_value(stream);
            }
        }
        return json!({"selector": sel, "resolved": false});
    }
    let mut ids: BTreeSet<_> = selected_keys
        .iter()
        .filter_map(|k| ownership.get(*k).cloned())
        .collect();
    if ids.len() == 1 {
        let id = ids.pop_first().unwrap();
        if let Some(catalog) = catalog {
            if let Some(stream) = catalog.workstreams.iter().find(|s| s.id.to_string() == id) {
                return workstream_value(stream);
            }
        }
        return json!({"id": id, "resolved": true});
    }
    json!({"resolved": false, "distinct_workstreams": ids.len()})
}

fn select_nav_works<'a>(
    scope: &MainlineNavScope,
    works: &'a [MainlineNavWorkFact],
    ownership: &BTreeMap<String, String>,
    edges: &[Edge],
) -> Vec<&'a MainlineNavWorkFact> {
    let work_filter: BTreeSet<_> = scope.work.iter().cloned().collect();
    let stream_id = scope.workstream.clone();
    works
        .iter()
        .filter(|w| {
            if w.archived {
                return false;
            }
            if !work_filter.is_empty() && !work_filter.contains(&w.key) {
                return false;
            }
            if let Some(stream) = &stream_id {
                match ownership.get(&w.key) {
                    Some(id) if id == stream || id_matches(id, stream) => {}
                    _ => return false,
                }
            }
            if let Some(milestone) = &scope.milestone {
                if w.milestone.as_deref() != Some(milestone.as_str()) {
                    return false;
                }
            }
            if let Some(goal) = &scope.goal {
                let tagged = w.tags.iter().any(|t| t == goal);
                let linked = edges.iter().any(|e| {
                    (e.relation == "supports" || e.relation == "depends_on")
                        && e.from_key == w.key
                        && e.to_key == *goal
                });
                if !tagged && !linked {
                    return false;
                }
            }
            true
        })
        .collect()
}

fn nav_guidance(
    nodes: &[MainlineNavNode],
    blockers: &[Value],
    cross_dependencies: &[Value],
    accounting_available: bool,
) -> Result<ActionGuidance> {
    let guidance = if !blockers.is_empty() {
        ActionGuidance::new(
            "Selected mainline has recorded blockers",
            "Work blocker fields and blocked status in the selected scope",
            "Inspect the cited blocker and clear it after rechecking required dependency outcomes",
            "Blocker cleared or dependency outcome accepted",
        )
    } else if nodes.iter().any(|n| !n.explainable_waits.is_empty()) {
        ActionGuidance::new(
            "Selected mainline is waiting on an explainable condition",
            "Dependency outcomes, user waits or stage acceptance windows",
            "Resolve the indicated wait; do not invent progress while the release condition is unmet",
            "Wait release condition is satisfied and rechecked",
        )
    } else if !cross_dependencies.is_empty() {
        ActionGuidance::new(
            "Cross-workstream dependency outcomes are in scope",
            "Required edges whose endpoints belong to different workstreams",
            "Navigate to the producer outcome before starting the dependent consumer work",
            "Producer outcome accepted or dependency edge removed by an authorized change",
        )
    } else if let Some(focus) = nodes.iter().find(|n| {
        matches!(
            n.status.as_str(),
            "ready" | "claimed" | "in_progress" | "inprogress"
        )
    }) {
        ActionGuidance::new(
            "Selected mainline has actionable work",
            &format!("Work {} ({})", focus.work_key, focus.title),
            "Prepare required context and continue only with an owned session; mutations re-check authorization",
            "Claim conflict, source change, wait or execution outcome change",
        )
    } else if accounting_available {
        ActionGuidance::new(
            "Scope accounting is available for the selected mainline",
            "WS-040 authoritative stage counts on the approved contract",
            "Use stage counts for navigation only; do not treat them as a write grant or blended percent",
            "Approved contract revision or verified observations change",
        )
    } else {
        ActionGuidance::new(
            "No actionable work in this mainline selection",
            "Current status, dependency outcomes and blocker fields",
            "Widen or correct the scope selectors, then re-read navigation",
            "Source correction or newly ready work appears in scope",
        )
    };
    if serde_json::to_vec(&guidance)?.len() > ACTION_GUIDANCE_MAX_BYTES {
        return Err(awr_core::Error::InvalidInput(
            "mainline navigation guidance exceeds its fixed byte budget".into(),
        ));
    }
    Ok(guidance)
}

/// Assemble a navigation snapshot from already-loaded facts (unit-test friendly).
pub fn assemble_mainline_nav(
    project: &Project,
    scope: &MainlineNavScope,
    catalog: Option<&WorkstreamCatalog>,
    works: &[MainlineNavWorkFact],
    dependency_edges: &[Edge],
    ownership: &BTreeMap<String, String>,
    extras: &MainlineNavExtras,
) -> Result<Value> {
    let selected = select_nav_works(scope, works, ownership, dependency_edges);
    let selected_keys: BTreeSet<&str> = selected.iter().map(|w| w.key.as_str()).collect();
    let by_key: BTreeMap<&str, &MainlineNavWorkFact> =
        works.iter().map(|w| (w.key.as_str(), w)).collect();

    let mut nodes = Vec::with_capacity(selected.len());
    let mut blockers = Vec::new();
    for work in &selected {
        let stream = ownership.get(&work.key).cloned();
        let responsibility = extras.responsibility_by_work.get(&work.key);
        let person = responsibility
            .and_then(|r| r.owner.as_ref())
            .map(|p| p.as_str().to_string())
            .or_else(|| work.owner.clone());
        let agent_execution = responsibility
            .and_then(|r| r.current_executor.as_ref())
            .map(execution_value);
        let stage_acceptance_window = extras.eta_by_work.get(&work.key).map(|c| {
            json!({
                "calendar_acceptance_window_ms": c.calendar_acceptance_window_ms,
                "effective_execution_ms": c.effective_execution_ms,
                "dependency_or_human_wait_ms": c.dependency_or_human_wait_ms,
            })
        });
        let mut waits = extras
            .waits_by_work
            .get(&work.key)
            .cloned()
            .unwrap_or_default();
        // depends_on: from_key depends on to_key (to_key is the producer).
        for edge in dependency_edges
            .iter()
            .filter(|e| e.required && e.from_key == work.key)
        {
            if let Some(dep) = by_key.get(edge.to_key.as_str()) {
                if dep.status != "completed" && dep.status != "cancelled" {
                    waits.push(ExplainableWait {
                        kind: "dependency_outcome".into(),
                        summary: format!("Waiting on outcome of {}", dep.key),
                        basis: format!("Required dependency {} ({})", dep.key, dep.title),
                        release_condition: format!(
                            "Complete and accept {} so its concrete outcome is available",
                            dep.key
                        ),
                    });
                }
            }
        }
        if let Some(blocker) = work.blocker.as_ref().filter(|b| !b.trim().is_empty()) {
            waits.push(ExplainableWait {
                kind: "explicit_blocker".into(),
                summary: short(blocker),
                basis: "Recorded work blocker".into(),
                release_condition: "Clear the blocker after rechecking required dependencies"
                    .into(),
            });
            blockers.push(json!({
                "work": work.key,
                "blocker": short(blocker),
                "status": work.status,
            }));
        } else if work.status == "blocked" {
            blockers.push(json!({
                "work": work.key,
                "blocker": "status_blocked",
                "status": "blocked",
            }));
        }
        nodes.push(MainlineNavNode {
            work_key: work.key.clone(),
            title: work.title.clone(),
            status: work.status.clone(),
            workstream_id: stream,
            person_responsibility: person,
            agent_execution,
            stage_acceptance_window,
            explainable_waits: waits,
            blocker: work.blocker.clone(),
            next_action: short(&work.next_action),
            outcomes: concrete_outcomes(work),
            last_event_at: extras.last_event_at_by_work.get(&work.key).copied(),
        });
    }

    let mut edges = Vec::new();
    let mut cross_dependencies = Vec::new();
    for edge in dependency_edges {
        // Producer = to_key, consumer = from_key for depends_on.
        let producer_key = &edge.to_key;
        let consumer_key = &edge.from_key;
        if !(selected_keys.contains(producer_key.as_str())
            || selected_keys.contains(consumer_key.as_str()))
        {
            continue;
        }
        let from_stream = ownership.get(producer_key).cloned();
        let to_stream = ownership.get(consumer_key).cloned();
        let cross = match (&from_stream, &to_stream) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        };
        let outcome = by_key
            .get(producer_key.as_str())
            .map(|w| {
                concrete_outcomes(w)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| format!("completion of {} ({})", w.key, w.title))
            })
            .unwrap_or_else(|| format!("completion of {producer_key}"));
        let nav_edge = MainlineNavEdge {
            from: producer_key.clone(),
            to: consumer_key.clone(),
            required: edge.required,
            outcome: outcome.clone(),
            cross_workstream: cross,
            from_workstream_id: from_stream.clone(),
            to_workstream_id: to_stream.clone(),
        };
        if cross {
            cross_dependencies.push(json!({
                "from": nav_edge.from,
                "to": nav_edge.to,
                "outcome": outcome,
                "from_workstream_id": from_stream,
                "to_workstream_id": to_stream,
                "required": edge.required,
            }));
        }
        if selected_keys.contains(producer_key.as_str())
            && selected_keys.contains(consumer_key.as_str())
        {
            edges.push(nav_edge);
        }
    }

    let accounting = match &extras.accounting {
        Some(report) => json!({
            "available": true,
            "authoritative": true,
            "contract": report.contract,
            "required_count": report.required_count,
            "source_declared_done": report.source_declared_done,
            "stages_are_independent": true,
            "shared_outcomes_not_owned_achievements": true,
            "is_not_goal_query_completion_rate": true,
            "planned": report.planned,
            "implemented": report.implemented,
            "verified": report.verified,
            "merged": report.merged,
            "released": report.released,
            "shared_reference_count": report.shared_reference_count,
        }),
        None => json!({
            "available": false,
            "authoritative": false,
            "reason": "approved_contract_and_verified_observations_required",
            "stages_are_independent": true,
            "is_not_goal_query_completion_rate": true,
            "detail": "Attach WS-040 account_approved_scope output; never invent a blended completion percent",
        }),
    };

    let selected_stream = resolve_selected_workstream(scope, catalog, ownership, &selected_keys);
    let guidance = nav_guidance(
        &nodes,
        &blockers,
        &cross_dependencies,
        extras.accounting.is_some(),
    )?;

    Ok(json!({
        "ok": true,
        "protocol": MAINLINE_NAV_PROTOCOL,
        "schema_version": MAINLINE_NAV_SCHEMA_VERSION,
        "read_only": true,
        "writes": [],
        "ws044_writes_deferred": false,
        "ws044_team_web": true,
        "authorization_note": "Navigation and reads only; mutations must re-check authorization on their own paths",
        "project": {
            "id": project.id,
            "name": project.name,
            "external_key": project.external_key,
            "revision": project.project_revision,
        },
        "scope": {
            "requested": scope,
            "workstream": selected_stream,
            "work_count": nodes.len(),
            "selection": selected_keys.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
        },
        "cross_dependencies": cross_dependencies,
        "accounting": accounting,
        "blockers": blockers,
        "mainline_graph": {
            "edge_basis": "concrete_task_outcomes",
            "nodes": nodes,
            "edges": edges,
        },
        "guidance": guidance,
        "compat": {
            "action_guidance_fields": ["when", "basis", "next_action", "recheck"],
            "legacy_status_action_view": "unchanged",
            "unrelated_payload_omitted": true,
        },
    }))
}

/// Store-backed read façade. Does not mutate state and does not grant write rights.
pub fn mainline_nav(
    store: &Store,
    project: &Project,
    scope: &MainlineNavScope,
    extras: MainlineNavExtras,
) -> Result<Value> {
    let projected = store.work_items(project.id)?;
    let works: Vec<MainlineNavWorkFact> = projected.iter().map(work_fact).collect();
    let edges = store.work_dependency_links(project.id)?;
    let catalog = store.workstream_catalog(project.id).ok();
    let mut ownership = BTreeMap::new();
    for work in &projected {
        if let Ok(binding) = store.workstream_binding(project.id, work.item.meta.id) {
            ownership.insert(
                work.item.meta.external_key.clone(),
                binding.workstream_id.to_string(),
            );
        }
    }
    let mut extras = extras;
    if extras.last_event_at_by_work.is_empty() {
        let last_events = store.last_event_times(project.id)?;
        extras.last_event_at_by_work = projected
            .iter()
            .filter_map(|w| {
                last_events
                    .get(&w.item.meta.id)
                    .map(|at| (w.item.meta.external_key.clone(), *at))
            })
            .collect();
    }
    for work in &projected {
        let key = work.item.meta.external_key.as_str();
        if !extras.responsibility_by_work.contains_key(key) {
            if let Ok(task) = store.task_responsibility(project.id, &work.item.meta.id.to_string())
            {
                extras.responsibility_by_work.insert(key.to_string(), task);
            }
        }
        if !extras.eta_by_work.contains_key(key) {
            if let Ok(forecasts) = store.list_eta_forecasts_for_target(project.id, key) {
                if let Some(latest) = forecasts.last() {
                    extras
                        .eta_by_work
                        .insert(key.to_string(), latest.components.clone());
                }
            }
        }
        if !extras.waits_by_work.contains_key(key) {
            if let Ok(waits) =
                store.pending_work_waits(project.id, work.item.meta.id, project.current_branch_id)
            {
                if !waits.is_empty() {
                    extras.waits_by_work.insert(
                        key.to_string(),
                        waits
                            .into_iter()
                            .map(|w| ExplainableWait {
                                kind: "user_wait".into(),
                                summary: short(&w.question),
                                basis: format!("MCP wait {}", w.id),
                                release_condition: "Record the user's actual reply on this wait"
                                    .into(),
                            })
                            .collect(),
                    );
                }
            }
        }
    }
    assemble_mainline_nav(
        project,
        scope,
        catalog.as_ref(),
        &works,
        &edges,
        &ownership,
        &extras,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::{
        AccountingContractIdentity, AccountingStageCount, AuthorityMode, EntityKind, Id, SourceRef,
    };
    use std::path::PathBuf;

    fn project() -> Project {
        Project {
            id: Id::new(),
            external_key: "demo".into(),
            name: "Demo".into(),
            root: PathBuf::from("/tmp/demo"),
            authority_mode: AuthorityMode::SourceFirst,
            current_branch_id: None,
            project_revision: 1,
        }
    }

    fn fact(key: &str, title: &str, status: &str, acceptance: &[&str]) -> MainlineNavWorkFact {
        MainlineNavWorkFact {
            key: key.into(),
            id: Id::new().to_string(),
            title: title.into(),
            status: status.into(),
            owner: Some("alice".into()),
            blocker: None,
            next_action: "Continue".into(),
            milestone: Some("WS4".into()),
            tags: vec![],
            acceptance: acceptance.iter().map(|s| (*s).to_string()).collect(),
            archived: false,
        }
    }

    fn edge(project_id: Id, from: &str, to: &str) -> Edge {
        Edge {
            id: Id::new(),
            project_id,
            from_kind: EntityKind::WorkItem,
            from_key: from.into(),
            relation: "depends_on".into(),
            to_kind: EntityKind::WorkItem,
            to_key: to.into(),
            required: true,
            revision: 1,
            source_ref: SourceRef {
                source_id: Id::new(),
                locator: "ledger.yaml".into(),
                source_revision: 1,
                source_fingerprint: "fp".into(),
                pointer: None,
                start_line: None,
                end_line: None,
                section_fingerprint: None,
            },
        }
    }

    #[test]
    fn nav_edges_use_concrete_outcomes_and_guidance_stays_compatible() {
        let mut consumer = fact(
            "AWR-WS-042",
            "Mainline navigation",
            "in_progress",
            &["CLI/MCP/Inspector nav snapshot"],
        );
        consumer.blocker = Some("Waiting on inspector wiring".into());
        let producer = fact(
            "AWR-WS-040",
            "Scope accounting",
            "completed",
            &["Frozen contract denominator with independent stages"],
        );
        let project = project();
        // consumer depends_on producer
        let edge = edge(project.id, "AWR-WS-042", "AWR-WS-040");
        let accounting = WorkstreamAccounting {
            contract: AccountingContractIdentity {
                project_id: "demo".into(),
                workstream_id: Id::new(),
                contract_id: "c1".into(),
                revision: 3,
                digest: "digest".into(),
            },
            required_count: 2,
            source_declared_done: 1,
            planned: AccountingStageCount {
                recorded: 2,
                not_met: 0,
                unknown: 0,
            },
            implemented: AccountingStageCount {
                recorded: 1,
                not_met: 1,
                unknown: 0,
            },
            verified: AccountingStageCount {
                recorded: 0,
                not_met: 0,
                unknown: 2,
            },
            merged: AccountingStageCount {
                recorded: 0,
                not_met: 0,
                unknown: 2,
            },
            released: AccountingStageCount {
                recorded: 0,
                not_met: 0,
                unknown: 2,
            },
            shared_reference_count: 0,
        };
        let mut ownership = BTreeMap::new();
        ownership.insert("AWR-WS-040".into(), "ws-a".into());
        ownership.insert("AWR-WS-042".into(), "ws-b".into());
        let extras = MainlineNavExtras {
            accounting: Some(accounting),
            ..Default::default()
        };
        let snap = assemble_mainline_nav(
            &project,
            &MainlineNavScope::default(),
            None,
            &[producer, consumer],
            &[edge],
            &ownership,
            &extras,
        )
        .unwrap();
        assert_eq!(snap["protocol"], MAINLINE_NAV_PROTOCOL);
        assert_eq!(snap["read_only"], true);
        assert_eq!(snap["ws044_writes_deferred"], false);
        assert_eq!(snap["ws044_team_web"], true);
        assert_eq!(snap["accounting"]["available"], true);
        assert_eq!(
            snap["accounting"]["is_not_goal_query_completion_rate"],
            true
        );
        assert_eq!(snap["accounting"]["implemented"]["recorded"], 1);
        assert_eq!(snap["cross_dependencies"].as_array().unwrap().len(), 1);
        assert_eq!(
            snap["cross_dependencies"][0]["outcome"],
            "Frozen contract denominator with independent stages"
        );
        assert_eq!(
            snap["mainline_graph"]["edges"][0]["outcome"],
            "Frozen contract denominator with independent stages"
        );
        assert_eq!(snap["mainline_graph"]["edges"][0]["from"], "AWR-WS-040");
        assert_eq!(snap["mainline_graph"]["edges"][0]["to"], "AWR-WS-042");
        assert_eq!(snap["blockers"].as_array().unwrap().len(), 1);
        let g = &snap["guidance"];
        assert!(g.get("when").is_some());
        assert!(g.get("basis").is_some());
        assert!(g.get("next_action").is_some());
        assert!(g.get("recheck").is_some());
        assert!(g.get("condition").is_none());
        assert_eq!(
            snap["compat"]["action_guidance_fields"],
            json!(["when", "basis", "next_action", "recheck"])
        );
        let encoded = serde_json::to_vec(g).unwrap();
        assert!(encoded.len() <= ACTION_GUIDANCE_MAX_BYTES);
    }

    #[test]
    fn scope_selector_filters_workstream_and_omits_unrelated_payload() {
        let a = fact("A", "Alpha", "ready", &["Alpha done"]);
        let b = fact("B", "Beta", "ready", &["Beta done"]);
        let mut ownership = BTreeMap::new();
        ownership.insert("A".into(), "stream-1".into());
        ownership.insert("B".into(), "stream-2".into());
        let snap = assemble_mainline_nav(
            &project(),
            &MainlineNavScope {
                workstream: Some("stream-1".into()),
                ..Default::default()
            },
            None,
            &[a, b],
            &[],
            &ownership,
            &MainlineNavExtras::default(),
        )
        .unwrap();
        let keys: Vec<_> = snap["mainline_graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["work_key"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(keys, vec!["A".to_string()]);
        assert!(snap.get("team_write_ops").is_none());
        assert!(snap.get("accept_responsibility").is_none());
        assert_eq!(snap["accounting"]["available"], false);
    }
    #[test]
    fn nodes_carry_the_newest_event_time_or_null_without_changing_other_fields() {
        let busy = fact("BUSY", "Has events", "in_progress", &["Done"]);
        let quiet = fact("QUIET", "Has none", "ready", &["Done"]);
        let snap = |extras: &MainlineNavExtras| {
            assemble_mainline_nav(
                &project(),
                &MainlineNavScope::default(),
                None,
                &[busy.clone(), quiet.clone()],
                &[],
                &BTreeMap::new(),
                extras,
            )
            .unwrap()
        };
        let plain = snap(&MainlineNavExtras::default());
        let dated = snap(&MainlineNavExtras {
            last_event_at_by_work: BTreeMap::from([("BUSY".to_string(), 1_790_000_000_123)]),
            ..Default::default()
        });
        let node = |snap: &Value, key: &str| {
            snap["mainline_graph"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["work_key"] == key)
                .unwrap()
                .clone()
        };
        assert_eq!(node(&dated, "BUSY")["last_event_at"], 1_790_000_000_123_i64);
        assert!(node(&dated, "QUIET")["last_event_at"].is_null());
        assert!(node(&plain, "BUSY")["last_event_at"].is_null());
        // Only the new field differs: protocol, schema and every other node field are untouched.
        let mut without = node(&dated, "BUSY");
        without["last_event_at"] = Value::Null;
        assert_eq!(without, node(&plain, "BUSY"));
        assert_eq!(dated["protocol"], MAINLINE_NAV_PROTOCOL);
        assert_eq!(dated["schema_version"], MAINLINE_NAV_SCHEMA_VERSION);
        // Older payloads without the field still decode.
        let mut old = node(&plain, "QUIET");
        old.as_object_mut().unwrap().remove("last_event_at");
        let decoded: MainlineNavNode = serde_json::from_value(old).unwrap();
        assert_eq!(decoded.last_event_at, None);
    }
}
