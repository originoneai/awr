//! Read-only mainline navigation snapshot shared by CLI, MCP and Inspector (WS-042).
//!
//! Surfaces select a scope and render authoritative facts already produced by
//! WS-040 accounting, WS-041 usage/time, WS-043 ETA/checkpoints, WS-018
//! review/blockers and WS-030/031/032 dependency graphs. This module does not
//! invent completion rates, mutate team state, or bypass authorization — callers
//! must authenticate the project / workstream before invoking it, and pass the
//! workstreams a limited reader may see as a [`MainlineNavAudience`]. Full Team Web
//! collaboration writes are delivered by the Team Web entry (WS-044).
use awr_core::{
    ACTION_GUIDANCE_MAX_BYTES, ActionGuidance, Edge, EtaComponents, Id, Project, Projected, Result,
    TaskResponsibility, WorkItem, Workstream, WorkstreamAccounting, WorkstreamCatalog,
    WorkstreamError,
};
use awr_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const MAINLINE_NAV_SCHEMA_VERSION: u32 = 1;
pub const MAINLINE_NAV_PROTOCOL: &str = "awr-mainline-nav";

/// Scope selectors for mainline navigation. Empty selectors mean everything the audience
/// may read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainlineNavScope {
    pub workstream: Option<String>,
    #[serde(default)]
    pub work: Vec<String>,
    pub goal: Option<String>,
    pub milestone: Option<String>,
}

/// Who a navigation snapshot is for.
///
/// Navigation joins every work and dependency of a project, so a reader with limited
/// grants is narrowed *before* assembly: a fact that never reaches the assembler cannot
/// be named by a node, wait, blocker, edge, guidance line, selection list or stream id.
/// For [`Workstreams`](Self::Workstreams) the scoped-read rules apply:
///
/// - Only works owned by the listed workstreams exist for the reader, whatever
///   `workstream`, `work`, `goal` or `milestone` selects; selectors only narrow further.
/// - An explicit `work` key that is not readable, because another workstream owns it or
///   because it does not exist, is `WorkstreamAccessDenied`. Both get the same answer and
///   the request is never answered partially.
/// - A dependency edge is listed only when both ends are readable; an edge with an end
///   outside is omitted. When a readable, unfinished work declares a required dependency
///   on unreadable (or dangling) work, it keeps one `dependency_outside_scope` wait per
///   edge that names neither the target nor its status, as `unavailable_dependencies`
///   does in scoped context: a dependency that cannot be verified must not read as "no
///   wait". Who depends on a readable work is never disclosed, not even as a count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainlineNavAudience {
    /// The project owner (local CLI, Inspector): every workstream is readable.
    Owner,
    /// A reader whose current Read grants cover exactly these workstreams. The caller
    /// verifies every grant first; this value carries no authority of its own.
    Workstreams(BTreeSet<Id>),
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
    /// Source-declared work-to-goal associations (`supports` edges, e.g. a ledger `goal:` field) used by the
    /// `goal` scope selector. `mainline_nav` loads them from the store when a goal is selected.
    pub goal_links: Vec<Edge>,
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

/// Navigation must not call a path clear while readiness still refuses it. Readiness releases a
/// required dependency only when it is completed and not archived (`dependency_not_completed`);
/// every other state is a wait here, and a cancelled one gets its own kind because it can never
/// finish by itself: the dependency has to change or the cancelled work has to be reopened.
fn dependency_wait(
    work: &MainlineNavWorkFact,
    dependency: &MainlineNavWorkFact,
) -> Option<ExplainableWait> {
    if dependency.status == "cancelled" {
        let reopen = if dependency.archived {
            format!("restore {} from the archive and reopen it", dependency.key)
        } else {
            format!("reopen {} and complete it", dependency.key)
        };
        return Some(ExplainableWait {
            kind: "dependency_cancelled".into(),
            summary: format!("Required dependency {} was cancelled", dependency.key),
            basis: format!(
                "Required dependency {} ({}) is cancelled; readiness does not count it as completed",
                dependency.key,
                short(&dependency.title)
            ),
            release_condition: format!(
                "Remove or replace {} in the dependencies of {}, or {reopen}",
                dependency.key, work.key
            ),
        });
    }
    if dependency.status == "completed" && !dependency.archived {
        return None;
    }
    let (basis, release_condition) = if dependency.archived {
        (
            format!(
                "Required dependency {} ({}) is archived; readiness does not count an archived dependency as completed",
                dependency.key, dependency.title
            ),
            format!(
                "Restore {} from the archive and complete it so its concrete outcome is available",
                dependency.key
            ),
        )
    } else {
        (
            format!(
                "Required dependency {} ({})",
                dependency.key, dependency.title
            ),
            format!(
                "Complete and accept {} so its concrete outcome is available",
                dependency.key
            ),
        )
    };
    Some(ExplainableWait {
        kind: "dependency_outcome".into(),
        summary: format!("Waiting on outcome of {}", dependency.key),
        basis,
        release_condition,
    })
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
    goal_links: &[Edge],
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
                let linked = edges.iter().chain(goal_links).any(|e| {
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
    let selected = select_nav_works(
        scope,
        works,
        ownership,
        dependency_edges,
        &extras.goal_links,
    );
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
                waits.extend(dependency_wait(work, dep));
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

/// The loaded facts a snapshot is assembled from, before and after narrowing.
struct NavFacts {
    works: Vec<MainlineNavWorkFact>,
    edges: Vec<Edge>,
    /// Work key to owning workstream id; a work without an owner is not listed.
    ownership: BTreeMap<String, String>,
    catalog: Option<WorkstreamCatalog>,
    /// Waits standing in for required dependencies on work the reader cannot read.
    boundary_waits: BTreeMap<String, Vec<ExplainableWait>>,
}

/// Reports that a required dependency exists (the declaring work's own fact) without the
/// target's key, title, owner, outcome, workstream or status.
fn boundary_wait(edge: &Edge) -> ExplainableWait {
    ExplainableWait {
        kind: "dependency_outside_scope".into(),
        summary: "A required dependency is outside the readable scope".into(),
        basis: format!(
            "Required dependency edge {} points to work this reader cannot read; its identity and status are withheld",
            edge.id
        ),
        release_condition:
            "Have a reader of the owning workstream confirm that the dependency outcome is accepted"
                .into(),
    }
}

impl NavFacts {
    /// Keep only what a reader of `streams` may see; see [`MainlineNavAudience`].
    fn narrowed(self, streams: &BTreeSet<Id>, scope: &MainlineNavScope) -> Result<Self> {
        let streams: BTreeSet<String> = streams.iter().map(Id::to_string).collect();
        // A work without a resolvable owner is never readable: unknown fails closed.
        let readable: BTreeSet<String> = self
            .works
            .iter()
            .filter(|work| {
                self.ownership
                    .get(&work.key)
                    .is_some_and(|stream| streams.contains(stream))
            })
            .map(|work| work.key.clone())
            .collect();
        if scope.work.iter().any(|key| !readable.contains(key)) {
            return Err(WorkstreamError::AccessDenied.into());
        }
        let unfinished: BTreeSet<&str> = self
            .works
            .iter()
            .filter(|work| !matches!(work.status.as_str(), "completed" | "cancelled"))
            .map(|work| work.key.as_str())
            .collect();
        let mut boundary_waits: BTreeMap<String, Vec<ExplainableWait>> = BTreeMap::new();
        for edge in &self.edges {
            if edge.required
                && unfinished.contains(edge.from_key.as_str())
                && readable.contains(&edge.from_key)
                && !readable.contains(&edge.to_key)
            {
                boundary_waits
                    .entry(edge.from_key.clone())
                    .or_default()
                    .push(boundary_wait(edge));
            }
        }
        Ok(Self {
            works: self
                .works
                .into_iter()
                .filter(|work| readable.contains(&work.key))
                .collect(),
            edges: self
                .edges
                .into_iter()
                .filter(|edge| readable.contains(&edge.from_key) && readable.contains(&edge.to_key))
                .collect(),
            ownership: self
                .ownership
                .into_iter()
                .filter(|(key, _)| readable.contains(key))
                .collect(),
            catalog: self.catalog.map(|mut catalog| {
                catalog
                    .workstreams
                    .retain(|stream| streams.contains(&stream.id.to_string()));
                catalog.legacy_default = catalog
                    .legacy_default
                    .filter(|id| streams.contains(&id.to_string()));
                catalog
            }),
            boundary_waits,
        })
    }
}

/// Store-backed read façade for the project owner (CLI, Inspector). Does not mutate
/// state and does not grant write rights.
pub fn mainline_nav(
    store: &Store,
    project: &Project,
    scope: &MainlineNavScope,
    extras: MainlineNavExtras,
) -> Result<Value> {
    mainline_nav_for(store, project, scope, extras, &MainlineNavAudience::Owner)
}

/// Store-backed read façade for an explicit audience. Does not mutate state and does not
/// grant write rights. The facts are narrowed before the per-work lookups and the
/// assembly, so neither loads or can name work the reader may not see.
pub fn mainline_nav_for(
    store: &Store,
    project: &Project,
    scope: &MainlineNavScope,
    extras: MainlineNavExtras,
    audience: &MainlineNavAudience,
) -> Result<Value> {
    let mut projected = store.work_items(project.id)?;
    let works: Vec<MainlineNavWorkFact> = projected.iter().map(work_fact).collect();
    let edges = store.work_dependency_links(project.id)?;
    let mut extras = extras;
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
    let mut facts = NavFacts {
        works,
        edges,
        ownership,
        catalog,
        boundary_waits: BTreeMap::new(),
    };
    if let MainlineNavAudience::Workstreams(streams) = audience {
        facts = facts.narrowed(streams, scope)?;
        let readable: BTreeSet<&str> = facts.works.iter().map(|w| w.key.as_str()).collect();
        projected.retain(|work| readable.contains(work.item.meta.external_key.as_str()));
    }
    if let Some(goal) = &scope.goal {
        if extras.goal_links.is_empty() {
            extras.goal_links = store.work_goal_links(project.id)?;
        }
        // A goal that nothing in the readable scope names would otherwise look like a goal
        // without work. A reader never learns whether such a key exists elsewhere: an unreadable
        // goal answers exactly like an unknown one.
        let known = match audience {
            MainlineNavAudience::Owner => {
                store
                    .goals(project.id)?
                    .iter()
                    .any(|g| g.item.meta.external_key == *goal)
                    || extras.goal_links.iter().any(|e| e.to_key == *goal)
                    || facts.works.iter().any(|w| w.tags.iter().any(|t| t == goal))
                    || facts.edges.iter().any(|e| e.to_key == *goal)
            }
            MainlineNavAudience::Workstreams(_) => {
                let readable: BTreeSet<&str> = facts.works.iter().map(|w| w.key.as_str()).collect();
                facts.catalog.as_ref().is_some_and(|catalog| {
                    catalog
                        .workstreams
                        .iter()
                        .any(|stream| stream.goal_keys.iter().any(|key| key == goal))
                }) || extras
                    .goal_links
                    .iter()
                    .any(|e| e.to_key == *goal && readable.contains(e.from_key.as_str()))
                    || facts.works.iter().any(|w| w.tags.iter().any(|t| t == goal))
                    || facts.edges.iter().any(|e| e.to_key == *goal)
                    // A readable work's own key may serve as a dependency-link selector.
                    || readable.contains(goal.as_str())
            }
        };
        if !known {
            return Err(awr_core::Error::NotFound(format!("goal {goal}")));
        }
    }
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
    assemble_facts(project, scope, facts, extras)
}

/// Assemble from loaded facts. Boundary waits join a work's other waits, so this runs
/// after the per-work lookups, which only fill keys that have no waits yet.
fn assemble_facts(
    project: &Project,
    scope: &MainlineNavScope,
    mut facts: NavFacts,
    mut extras: MainlineNavExtras,
) -> Result<Value> {
    for (key, waits) in std::mem::take(&mut facts.boundary_waits) {
        extras.waits_by_work.entry(key).or_default().extend(waits);
    }
    assemble_mainline_nav(
        project,
        scope,
        facts.catalog.as_ref(),
        &facts.works,
        &facts.edges,
        &facts.ownership,
        &extras,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use awr_core::{
        AccountingContractIdentity, AccountingStageCount, AuthorityMode, EntityKind, SourceRef,
        WORKSTREAM_CATALOG_VERSION, WorkstreamState,
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

    fn supports(project_id: Id, work: &str, goal: &str) -> Edge {
        let mut link = edge(project_id, work, goal);
        link.relation = "supports".into();
        link.to_kind = EntityKind::Goal;
        link
    }

    fn selected(
        scope_goal: &str,
        works: &[MainlineNavWorkFact],
        dependencies: &[Edge],
        goal_links: Vec<Edge>,
    ) -> Vec<String> {
        let snap = assemble_mainline_nav(
            &project(),
            &MainlineNavScope {
                goal: Some(scope_goal.into()),
                ..Default::default()
            },
            None,
            works,
            dependencies,
            &BTreeMap::new(),
            &MainlineNavExtras {
                goal_links,
                ..Default::default()
            },
        )
        .unwrap();
        snap["scope"]["selection"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn goal_scope_matches_source_declared_links_and_stays_compatible_with_tags() {
        let project_id = project().id;
        let declared = fact("DECLARED", "Declares the goal", "ready", &["Done"]);
        let both = fact("BOTH", "Declares two goals", "in_progress", &["Done"]);
        let cancelled = fact(
            "CANCELLED",
            "Cancelled but declared",
            "cancelled",
            &["Done"],
        );
        let mut archived = fact("ARCHIVED", "Archived and declared", "ready", &["Done"]);
        archived.archived = true;
        let mut tagged = fact("TAGGED", "Carries the tag", "ready", &["Done"]);
        tagged.tags = vec!["G".into()];
        let unrelated = fact("UNRELATED", "Names no goal", "ready", &["Done"]);
        let works = [declared, both, cancelled, archived, tagged, unrelated];
        let links = || {
            vec![
                supports(project_id, "DECLARED", "G"),
                supports(project_id, "BOTH", "G"),
                supports(project_id, "BOTH", "OTHER"),
                supports(project_id, "CANCELLED", "G"),
                supports(project_id, "ARCHIVED", "G"),
            ]
        };

        // Declared work, cancelled work and tagged work are in; archived work stays out as before.
        assert_eq!(
            selected("G", &works, &[], links()),
            ["BOTH", "CANCELLED", "DECLARED", "TAGGED"]
        );
        // One work can serve several goals.
        assert_eq!(selected("OTHER", &works, &[], links()), ["BOTH"]);
        // Without the source links only the tag matches: the previous behavior.
        assert_eq!(selected("G", &works, &[], vec![]), ["TAGGED"]);
        // A goal nobody declares selects nothing in the pure assembler; `mainline_nav` rejects unknown goals.
        assert!(selected("ABSENT", &works, &[], links()).is_empty());
    }

    fn consumer_waits(
        dependency_status: &str,
        archived: bool,
        required: bool,
    ) -> Vec<ExplainableWait> {
        let project = project();
        let mut dependency = fact(
            "PRODUCER",
            "Producer",
            dependency_status,
            &["Producer done"],
        );
        dependency.archived = archived;
        let consumer = fact("CONSUMER", "Consumer", "planned", &["Consumer done"]);
        let mut link = edge(project.id, "CONSUMER", "PRODUCER");
        link.required = required;
        let snap = assemble_mainline_nav(
            &project,
            &MainlineNavScope {
                work: vec!["CONSUMER".into()],
                ..Default::default()
            },
            None,
            &[dependency, consumer],
            &[link],
            &BTreeMap::new(),
            &MainlineNavExtras::default(),
        )
        .unwrap();
        let nodes = snap["mainline_graph"]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        serde_json::from_value(nodes[0]["explainable_waits"].clone()).unwrap()
    }

    #[test]
    fn a_required_dependency_releases_only_when_completed_and_unarchived_like_readiness() {
        for status in ["planned", "ready", "in_progress", "blocked", "draft"] {
            let waits = consumer_waits(status, false, true);
            assert_eq!(waits.len(), 1, "{status}");
            assert_eq!(waits[0].kind, "dependency_outcome");
            assert_eq!(waits[0].summary, "Waiting on outcome of PRODUCER");
            assert!(
                waits[0]
                    .release_condition
                    .starts_with("Complete and accept PRODUCER")
            );
        }
        // Completed work releases its dependents; nothing to wait for.
        assert!(consumer_waits("completed", false, true).is_empty());

        // Cancelled work never finishes by itself: its own kind, with the two real ways out.
        let cancelled = consumer_waits("cancelled", false, true);
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].kind, "dependency_cancelled");
        assert_eq!(
            cancelled[0].summary,
            "Required dependency PRODUCER was cancelled"
        );
        assert!(
            cancelled[0]
                .basis
                .contains("readiness does not count it as completed")
        );
        assert_eq!(
            cancelled[0].release_condition,
            "Remove or replace PRODUCER in the dependencies of CONSUMER, or reopen PRODUCER and complete it"
        );

        // Readiness also refuses archived dependencies, whatever their lifecycle status.
        let archived_done = consumer_waits("completed", true, true);
        assert_eq!(archived_done.len(), 1);
        assert_eq!(archived_done[0].kind, "dependency_outcome");
        assert!(archived_done[0].basis.contains("is archived"));
        assert!(
            archived_done[0]
                .release_condition
                .starts_with("Restore PRODUCER from the archive")
        );
        let archived_open = consumer_waits("planned", true, true);
        assert_eq!(archived_open[0].kind, "dependency_outcome");
        assert!(archived_open[0].basis.contains("is archived"));
        let archived_cancelled = consumer_waits("cancelled", true, true);
        assert_eq!(archived_cancelled[0].kind, "dependency_cancelled");
        assert!(
            archived_cancelled[0]
                .release_condition
                .ends_with("or restore PRODUCER from the archive and reopen it")
        );

        // Optional edges never block.
        for status in ["planned", "cancelled", "completed"] {
            assert!(consumer_waits(status, false, false).is_empty(), "{status}");
        }
    }

    #[test]
    fn a_cancelled_cross_workstream_dependency_is_explained_even_outside_the_selected_scope() {
        let producer = fact(
            "EVO-11",
            "Retired design",
            "cancelled",
            &["Design accepted"],
        );
        let consumer = fact(
            "EVO-12",
            "Build on the design",
            "planned",
            &["Build accepted"],
        );
        let project = project();
        let link = edge(project.id, "EVO-12", "EVO-11");
        let mut ownership = BTreeMap::new();
        ownership.insert("EVO-11".into(), "stream-design".into());
        ownership.insert("EVO-12".into(), "stream-build".into());
        let snap = assemble_mainline_nav(
            &project,
            &MainlineNavScope {
                workstream: Some("stream-build".into()),
                ..Default::default()
            },
            None,
            &[producer, consumer],
            &[link],
            &ownership,
            &MainlineNavExtras::default(),
        )
        .unwrap();
        // Only the consumer's stream is selected, yet the wait names the cancelled producer.
        let nodes = snap["mainline_graph"]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["work_key"], "EVO-12");
        assert_eq!(
            nodes[0]["explainable_waits"][0]["kind"],
            "dependency_cancelled"
        );
        assert_eq!(snap["cross_dependencies"].as_array().unwrap().len(), 1);
        assert_eq!(snap["cross_dependencies"][0]["from"], "EVO-11");
        assert_eq!(snap["cross_dependencies"][0]["to"], "EVO-12");
        // The additive kind changes neither the protocol nor the guidance contract.
        assert_eq!(snap["protocol"], MAINLINE_NAV_PROTOCOL);
        assert_eq!(snap["schema_version"], MAINLINE_NAV_SCHEMA_VERSION);
        assert_eq!(
            snap["guidance"]["when"],
            "Selected mainline is waiting on an explainable condition"
        );
    }

    /// Three workstreams: api (API-1, API-2), client (CLIENT-1) and ops (OPS-1). CLIENT-1
    /// and API-2 depend on API-1, and API-2 also depends on OPS-1.
    fn world() -> (Project, [Id; 3], NavFacts) {
        let project = project();
        let streams = [Id::new(), Id::new(), Id::new()];
        let mut client = fact(
            "CLIENT-1",
            "PRIVATE_CLIENT_TASK",
            "in_progress",
            &["PRIVATE_CLIENT_OUTCOME"],
        );
        client.blocker = Some("PRIVATE_CLIENT_BLOCKER".into());
        let ops = fact(
            "OPS-1",
            "PRIVATE_OPS_TASK",
            "ready",
            &["PRIVATE_OPS_OUTCOME"],
        );
        let works = vec![
            fact(
                "API-1",
                "Define the interface",
                "in_progress",
                &["Interface contract"],
            ),
            fact("API-2", "Review the interface", "ready", &["Reviewed"]),
            client,
            ops,
        ];
        let ownership = [("API-1", 0), ("API-2", 0), ("CLIENT-1", 1), ("OPS-1", 2)]
            .into_iter()
            .map(|(key, stream)| (key.to_owned(), streams[stream].to_string()))
            .collect();
        let catalog = WorkstreamCatalog {
            version: WORKSTREAM_CATALOG_VERSION,
            project_id: "demo".into(),
            legacy_default: None,
            workstreams: streams
                .iter()
                .zip(["api", "client", "ops"])
                .map(|(id, key)| Workstream {
                    id: *id,
                    project_id: "demo".into(),
                    external_key: key.into(),
                    title: format!("PRIVATE_STREAM_{key}"),
                    state: WorkstreamState::Active,
                    authority_version: 1,
                    goal_keys: vec![],
                    acceptance_contracts: vec![],
                })
                .collect(),
        };
        let facts = NavFacts {
            edges: vec![
                edge(project.id, "CLIENT-1", "API-1"),
                edge(project.id, "API-2", "API-1"),
                edge(project.id, "API-2", "OPS-1"),
            ],
            works,
            ownership,
            catalog: Some(catalog),
            boundary_waits: BTreeMap::new(),
        };
        (project, streams, facts)
    }

    fn reader_nav(
        streams: &[Id],
        scope: &MainlineNavScope,
        project: &Project,
        facts: NavFacts,
    ) -> Result<Value> {
        let facts = facts.narrowed(&streams.iter().copied().collect(), scope)?;
        assemble_facts(project, scope, facts, MainlineNavExtras::default())
    }

    fn node<'a>(nav: &'a Value, key: &str) -> &'a Value {
        nav["mainline_graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["work_key"] == key)
            .unwrap_or_else(|| panic!("{key} is not a node of {nav}"))
    }

    fn wait_kinds(node: &Value) -> Vec<&str> {
        node["explainable_waits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|wait| wait["kind"].as_str().unwrap())
            .collect()
    }

    fn selection(nav: &Value) -> Vec<&str> {
        nav["scope"]["selection"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect()
    }

    #[test]
    fn reader_view_never_names_unreadable_work_streams_or_outcomes() {
        let (project, streams, facts) = world();
        let nav = reader_nav(&streams[..1], &MainlineNavScope::default(), &project, facts).unwrap();
        assert_eq!(selection(&nav), ["API-1", "API-2"]);
        assert_eq!(nav["scope"]["work_count"], 2);
        // One readable stream among the selected work: resolved, not "N distinct streams".
        assert_eq!(nav["scope"]["workstream"]["id"], streams[0].to_string());
        // CLIENT-1 -> API-1 (hidden consumer) and API-2 -> OPS-1 (hidden producer) each have
        // an end outside the readable set, so only the edge inside it remains.
        assert_eq!(nav["cross_dependencies"], json!([]));
        assert_eq!(nav["blockers"], json!([]));
        let edges = nav["mainline_graph"]["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(
            (&edges[0]["from"], &edges[0]["to"]),
            (&json!("API-1"), &json!("API-2"))
        );
        // The one hidden required dependency of API-2 is an opaque wait beside the named one.
        assert_eq!(
            wait_kinds(node(&nav, "API-2")),
            ["dependency_outside_scope", "dependency_outcome"]
        );
        assert_eq!(wait_kinds(node(&nav, "API-1")), Vec::<&str>::new());
        let text = nav.to_string();
        for hidden in [
            "CLIENT-1",
            "OPS-1",
            "PRIVATE_CLIENT",
            "PRIVATE_OPS",
            "PRIVATE_STREAM_client",
            "PRIVATE_STREAM_ops",
            &streams[1].to_string(),
            &streams[2].to_string(),
        ] {
            assert!(!text.contains(hidden), "{hidden} leaked: {text}");
        }
        // The foreign blocker must not steer the guidance either.
        assert_ne!(
            nav["guidance"]["when"],
            "Selected mainline has recorded blockers"
        );
    }

    #[test]
    fn reader_of_two_streams_keeps_the_edges_between_them_but_not_a_third() {
        let (project, streams, facts) = world();
        let nav = reader_nav(&streams[..2], &MainlineNavScope::default(), &project, facts).unwrap();
        assert_eq!(selection(&nav), ["API-1", "API-2", "CLIENT-1"]);
        let cross = nav["cross_dependencies"].as_array().unwrap();
        assert_eq!(cross.len(), 1);
        assert_eq!(cross[0]["from"], "API-1");
        assert_eq!(cross[0]["to"], "CLIENT-1");
        assert_eq!(cross[0]["from_workstream_id"], streams[0].to_string());
        assert_eq!(cross[0]["to_workstream_id"], streams[1].to_string());
        assert_eq!(nav["blockers"][0]["work"], "CLIENT-1");
        // The ops stream is still out of reach, and its edge is still opaque.
        assert!(wait_kinds(node(&nav, "API-2")).contains(&"dependency_outside_scope"));
        assert_eq!(nav["scope"]["workstream"]["resolved"], false);
        assert_eq!(nav["scope"]["workstream"]["distinct_workstreams"], 2);
        let text = nav.to_string();
        assert!(!text.contains("OPS-1") && !text.contains(&streams[2].to_string()));
        assert!(!text.contains("PRIVATE_OPS") && !text.contains("PRIVATE_STREAM_ops"));
    }

    #[test]
    fn explicit_work_outside_the_streams_is_denied_whether_foreign_or_absent() {
        for keys in [
            vec!["CLIENT-1"],
            vec!["API-1", "CLIENT-1"],
            vec!["no-such-work"],
        ] {
            let (project, streams, facts) = world();
            let scope = MainlineNavScope {
                work: keys.iter().map(|key| (*key).to_owned()).collect(),
                ..Default::default()
            };
            let denied = reader_nav(&streams[..1], &scope, &project, facts);
            assert!(
                matches!(
                    denied,
                    Err(awr_core::Error::Workstream(WorkstreamError::AccessDenied))
                ),
                "{keys:?}: {denied:?}"
            );
        }
        let (project, streams, facts) = world();
        let scope = MainlineNavScope {
            work: vec!["API-2".into()],
            ..Default::default()
        };
        let nav = reader_nav(&streams[..1], &scope, &project, facts).unwrap();
        assert_eq!(selection(&nav), ["API-2"]);
    }

    /// Every work carries the same tag and milestone, so each selector matches all of them
    /// in the owner's view; the reader is the api stream only.
    fn tagged_world() -> (Project, [Id; 3], NavFacts) {
        let (project, streams, mut facts) = world();
        for work in &mut facts.works {
            work.tags = vec!["shared-tag".into()];
        }
        (project, streams, facts)
    }

    /// The scope is built from the world's own stream ids, which differ per world.
    fn api_reader_nav(scope: impl FnOnce(&[Id; 3]) -> MainlineNavScope) -> Value {
        let (project, streams, facts) = tagged_world();
        reader_nav(&streams[..1], &scope(&streams), &project, facts).unwrap()
    }

    #[test]
    fn selectors_only_narrow_what_a_reader_may_see() {
        let milestone = |_: &[Id; 3]| MainlineNavScope {
            milestone: Some("WS4".into()),
            ..Default::default()
        };
        let tag = |_: &[Id; 3]| MainlineNavScope {
            goal: Some("shared-tag".into()),
            ..Default::default()
        };
        assert_eq!(selection(&api_reader_nav(milestone)), ["API-1", "API-2"]);
        assert_eq!(selection(&api_reader_nav(tag)), ["API-1", "API-2"]);
        // An owner's goal can match through a dependency link. The link to OPS-1 is hidden
        // from the reader, so it can neither select API-2 nor confirm that OPS-1 exists.
        let linked = api_reader_nav(|_| MainlineNavScope {
            goal: Some("OPS-1".into()),
            ..Default::default()
        });
        assert_eq!(selection(&linked), Vec::<&str>::new());
        // A stream selector resolves against the narrowed catalog.
        let own = api_reader_nav(|streams| MainlineNavScope {
            workstream: Some(streams[0].to_string()),
            ..Default::default()
        });
        assert_eq!(own["scope"]["workstream"]["external_key"], "api");
        assert_eq!(selection(&own), ["API-1", "API-2"]);
        // Another stream, by id or by key, selects nothing and is not even resolved.
        for foreign in [
            api_reader_nav(|streams| MainlineNavScope {
                workstream: Some(streams[1].to_string()),
                ..Default::default()
            }),
            api_reader_nav(|_| MainlineNavScope {
                workstream: Some("client".into()),
                ..Default::default()
            }),
        ] {
            assert_eq!(selection(&foreign), Vec::<&str>::new());
            assert_eq!(foreign["scope"]["workstream"]["resolved"], false);
            assert!(!foreign.to_string().contains("PRIVATE_STREAM"));
        }
        // The owner's full view still matches through that link.
        let (project, _, facts) = tagged_world();
        let owner = assemble_mainline_nav(
            &project,
            &MainlineNavScope {
                goal: Some("OPS-1".into()),
                ..Default::default()
            },
            facts.catalog.as_ref(),
            &facts.works,
            &facts.edges,
            &facts.ownership,
            &MainlineNavExtras::default(),
        )
        .unwrap();
        assert_eq!(selection(&owner), ["API-2"]);
    }

    #[test]
    fn boundary_waits_follow_unfinished_work_and_required_declared_dependencies_only() {
        let hidden = || {
            let (project, streams, mut facts) = world();
            facts.edges = vec![];
            (project, streams, facts)
        };
        let waits = |facts: NavFacts, project: &Project, streams: &[Id]| {
            let nav = reader_nav(streams, &MainlineNavScope::default(), project, facts).unwrap();
            node(&nav, "API-2")["explainable_waits"].clone()
        };
        // A finished consumer has nothing left to wait for.
        for status in ["completed", "cancelled"] {
            let (project, streams, mut facts) = hidden();
            facts.edges = vec![edge(project.id, "API-2", "OPS-1")];
            facts.works[1].status = status.into();
            assert_eq!(waits(facts, &project, &streams[..1]), json!([]), "{status}");
        }
        // An optional dependency is not a wait.
        let (project, streams, mut facts) = hidden();
        let mut optional = edge(project.id, "API-2", "OPS-1");
        optional.required = false;
        facts.edges = vec![optional];
        assert_eq!(waits(facts, &project, &streams[..1]), json!([]));
        // A dangling target cannot be told apart from an unreadable one: same wait.
        let (project, streams, mut facts) = hidden();
        facts.edges = vec![edge(project.id, "API-2", "GHOST-9")];
        let dangling = waits(facts, &project, &streams[..1]);
        let (project, streams, mut facts) = hidden();
        facts.edges = vec![edge(project.id, "API-2", "OPS-1")];
        let foreign = waits(facts, &project, &streams[..1]);
        assert_eq!(dangling[0]["kind"], "dependency_outside_scope");
        assert_eq!(dangling[0]["summary"], foreign[0]["summary"]);
        assert_eq!(
            dangling[0]["release_condition"],
            foreign[0]["release_condition"]
        );
        // One wait per required edge, each distinguishable by its edge, none naming a target.
        let (project, streams, mut facts) = hidden();
        facts.edges = vec![
            edge(project.id, "API-2", "OPS-1"),
            edge(project.id, "API-2", "CLIENT-1"),
        ];
        let both = waits(facts, &project, &streams[..1]);
        assert_eq!(both.as_array().unwrap().len(), 2);
        assert_ne!(both[0]["basis"], both[1]["basis"]);
        let text = both.to_string();
        for hidden in ["OPS-1", "CLIENT-1", "PRIVATE", "in_progress", "ready"] {
            assert!(!text.contains(hidden), "{hidden} leaked: {text}");
        }
        // Who depends on a readable work is never disclosed, not even as a count.
        let (project, streams, mut facts) = hidden();
        facts.edges = vec![
            edge(project.id, "CLIENT-1", "API-1"),
            edge(project.id, "OPS-1", "API-1"),
        ];
        let nav = reader_nav(&streams[..1], &MainlineNavScope::default(), &project, facts).unwrap();
        assert_eq!(wait_kinds(node(&nav, "API-1")), Vec::<&str>::new());
        assert_eq!(nav["mainline_graph"]["edges"], json!([]));
        assert_eq!(nav["cross_dependencies"], json!([]));
    }

    #[test]
    fn boundary_waits_keep_the_work_s_own_waits() {
        let (project, streams, facts) = world();
        let facts = facts
            .narrowed(
                &streams[..1].iter().copied().collect(),
                &MainlineNavScope::default(),
            )
            .unwrap();
        let mut extras = MainlineNavExtras::default();
        extras.waits_by_work.insert(
            "API-2".into(),
            vec![ExplainableWait {
                kind: "user_wait".into(),
                summary: "Question".into(),
                basis: "MCP wait".into(),
                release_condition: "Reply".into(),
            }],
        );
        let nav = assemble_facts(&project, &MainlineNavScope::default(), facts, extras).unwrap();
        assert_eq!(
            wait_kinds(node(&nav, "API-2")),
            [
                "user_wait",
                "dependency_outside_scope",
                "dependency_outcome"
            ]
        );
    }

    #[test]
    fn work_without_a_resolvable_owner_is_never_readable() {
        let (project, streams, mut facts) = world();
        facts.ownership.remove("API-2");
        let nav = reader_nav(&streams[..1], &MainlineNavScope::default(), &project, facts).unwrap();
        assert_eq!(selection(&nav), ["API-1"]);
        let (_, streams, mut facts) = world();
        facts.ownership.remove("API-2");
        let scope = MainlineNavScope {
            work: vec!["API-2".into()],
            ..Default::default()
        };
        assert!(
            facts
                .narrowed(&streams[..1].iter().copied().collect(), &scope)
                .is_err()
        );
    }

    #[test]
    fn owner_view_is_not_narrowed() {
        let (project, _, facts) = world();
        let nav = assemble_facts(
            &project,
            &MainlineNavScope::default(),
            facts,
            MainlineNavExtras::default(),
        )
        .unwrap();
        assert_eq!(selection(&nav), ["API-1", "API-2", "CLIENT-1", "OPS-1"]);
        assert_eq!(nav["cross_dependencies"].as_array().unwrap().len(), 2);
        assert!(nav.to_string().contains("PRIVATE_CLIENT_TASK"));
        assert!(!nav.to_string().contains("dependency_outside_scope"));
    }

    #[test]
    fn a_readers_catalog_keeps_only_readable_streams_and_a_readable_default() {
        let readable = |facts: NavFacts, stream: Id| {
            facts
                .narrowed(&BTreeSet::from([stream]), &MainlineNavScope::default())
                .unwrap()
                .catalog
                .unwrap()
        };
        // The legacy default may be a stream the reader cannot read: it is dropped with it.
        let (_, streams, mut facts) = world();
        facts.catalog.as_mut().unwrap().legacy_default = Some(streams[1]);
        let catalog = readable(facts, streams[0]);
        assert_eq!(
            catalog.workstreams.iter().map(|s| s.id).collect::<Vec<_>>(),
            [streams[0]]
        );
        assert_eq!(catalog.legacy_default, None);
        // A readable default stays.
        let (_, streams, mut facts) = world();
        facts.catalog.as_mut().unwrap().legacy_default = Some(streams[0]);
        assert_eq!(readable(facts, streams[0]).legacy_default, Some(streams[0]));
    }
}
