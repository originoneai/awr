//! Source graph inspection and whole-candidate validation. No executors live here.
//! WS-031: cross-stream task DAGs (A→B→later A) stay acyclic; hard cycles return
//! an explainable path. Necessary dependencies must all be satisfied before ready;
//! shared outcomes are referenced once, not copied per consumer stream.
use awr_core::*;
use awr_source::{
    ParseContext, SourceSnapshot, fingerprint, inspect_registered_source, source_adapter,
};
use awr_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

type Links = BTreeMap<String, BTreeSet<String>>;

fn required_links(works: &BTreeMap<String, WorkItem>, edges: &[Edge]) -> Links {
    let mut links: Links = works.keys().map(|k| (k.clone(), BTreeSet::new())).collect();
    for e in edges.iter().filter(|e| e.required) {
        links
            .entry(e.from_key.clone())
            .or_default()
            .insert(e.to_key.clone());
    }
    links
}

// Iterative traversal avoids a call-stack limit on long project plans.
fn cycle(links: &Links) -> Vec<String> {
    let mut finished = BTreeSet::new();
    for root in links.keys() {
        if finished.contains(root) {
            continue;
        }
        let mut path = vec![root.clone()];
        let mut active = BTreeMap::from([(root.clone(), 0)]);
        let mut stack = vec![links[root].iter()];
        while let Some(iter) = stack.last_mut() {
            if let Some(next) = iter.next() {
                if let Some(index) = active.get(next) {
                    let mut found = path[*index..].to_vec();
                    found.push(next.clone());
                    return found;
                }
                if finished.contains(next) {
                    continue;
                }
                if let Some(children) = links.get(next) {
                    active.insert(next.clone(), path.len());
                    path.push(next.clone());
                    stack.push(children.iter());
                }
            } else {
                stack.pop();
                let done = path.pop().unwrap();
                active.remove(&done);
                finished.insert(done);
            }
        }
    }
    vec![]
}

fn graph_error(edge: &Edge, message: String, rule: &str, repair: &str) -> Error {
    Error::InvalidSource(Box::new(SourceDiagnostic {
        message,
        location: DiagnosticLocation {
            locator: Some(edge.source_ref.locator.clone()),
            pointer: edge.source_ref.pointer.clone(),
            ..Default::default()
        },
        rule: rule.into(),
        repair: repair.into(),
    }))
}

/// Validate the final candidate, so a batch may add dependencies and their targets together.
pub(crate) fn validate_graph_projection(
    store: &Store,
    project: Id,
    source: Id,
    projection: &ProjectionBatch,
) -> Result<()> {
    let current = store.work_items(project)?;
    let mut works: BTreeMap<_, _> = current
        .iter()
        .filter(|w| w.item.meta.source_ref.source_id != source)
        .map(|w| (w.item.meta.external_key.clone(), w.item.clone()))
        .collect();
    for work in &projection.work_items {
        if works
            .insert(work.meta.external_key.clone(), work.clone())
            .is_some()
        {
            return Err(Error::SourceConflict(format!(
                "work key {} exists in another source",
                work.meta.external_key
            )));
        }
    }
    let mut edges = store.work_dependency_links(project)?;
    edges.retain(|e| e.source_ref.source_id != source);
    edges.extend(
        projection
            .edges
            .iter()
            .filter(|e| {
                e.from_kind == EntityKind::WorkItem
                    && e.to_kind == EntityKind::WorkItem
                    && e.relation == "depends_on"
            })
            .cloned(),
    );
    for e in edges.iter().filter(|e| e.required) {
        if !works.contains_key(&e.from_key) || !works.contains_key(&e.to_key) {
            return Err(graph_error(
                e,
                format!(
                    "required dependency {} -> {} has a missing work reference",
                    e.from_key, e.to_key
                ),
                "work_graph.required_reference",
                "Create the referenced work or remove the incorrect required dependency in the same batch.",
            ));
        }
    }
    let links = required_links(&works, &edges);
    let found = cycle(&links);
    if !found.is_empty() {
        let edge = edges
            .iter()
            .find(|e| e.required && e.from_key == found[0] && e.to_key == found[1])
            .unwrap();
        return Err(graph_error(
            edge,
            format!("required dependency cycle: {}", found.join(" -> ")),
            "work_graph.acyclic",
            "Remove or redirect a dependency in this cycle; parallel execution cannot satisfy a cycle.",
        ));
    }
    // A planner cannot replace an active executor's contract underneath its claim.
    let old_edges = store.work_dependency_links(project)?;
    let old_goals = store.work_goal_links(project)?;
    for old in current
        .iter()
        .filter(|w| w.item.meta.source_ref.source_id == source)
    {
        let key = &old.item.meta.external_key;
        let Some(new) = works.get(key) else {
            store.ensure_work_unoccupied(project, old.item.meta.id)?;
            continue;
        };
        let contract = |w: &WorkItem| {
            json!([
                w.title,
                w.summary,
                w.kind,
                w.acceptance,
                w.paths,
                w.tags,
                w.milestone
            ])
        };
        let dependencies = |all: &[Edge]| -> BTreeSet<_> {
            all.iter()
                .filter(|e| e.from_key == *key)
                .map(|e| (e.to_key.clone(), e.required))
                .collect()
        };
        let goals: Vec<_> = projection
            .edges
            .iter()
            .filter(|e| e.to_kind == EntityKind::Goal && e.relation == "supports")
            .cloned()
            .collect();
        if contract(&old.item) != contract(new)
            || dependencies(&old_edges) != dependencies(&edges)
            || dependencies(&old_goals) != dependencies(&goals)
        {
            store.ensure_work_unoccupied(project, old.item.meta.id)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_graph_snapshot(
    store: &Store,
    root: &Path,
    source: &Source,
    after: &SourceSnapshot,
) -> Result<()> {
    let (_, spec, _) = inspect_registered_source(root, source)?;
    let projection = source_adapter(&source.adapter)?.parse(
        after,
        &ParseContext {
            source,
            existing_ids: store.projection_ids(source)?,
        },
        &spec,
    )?;
    validate_graph_projection(store, source.project_id, source.id, &projection)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkGraphRequest {
    /// Empty selects all work. Otherwise include dependents and their required ancestors.
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    100
}

pub fn work_graph(store: &Store, root: &Path, request: &WorkGraphRequest) -> Result<Value> {
    if !(1..=1000).contains(&request.limit) || request.roots.len() > 100 {
        return Err(Error::InvalidInput(
            "graph limit must be 1..1000 and roots at most 100".into(),
        ));
    }
    let project = store.project_by_root(root)?;
    let branch = match &request.branch {
        Some(b) => store.resolve_branch(project.id, b)?,
        None => project.current_branch_id,
    };
    let projected = store.work_items(project.id)?;
    let works: BTreeMap<_, _> = projected
        .iter()
        .map(|w| (w.item.meta.external_key.clone(), w.item.clone()))
        .collect();
    let edges = store.work_dependency_links(project.id)?;
    let links = required_links(&works, &edges);
    let mut selected = BTreeSet::new();
    for key in &request.roots {
        if !works.contains_key(key) {
            return Err(Error::NotFound(format!("work {key}")));
        }
        selected.insert(key.clone());
    }
    if request.roots.is_empty() {
        selected.extend(works.keys().cloned());
    }
    loop {
        let before = selected.len();
        for e in edges.iter().filter(|e| e.required) {
            if selected.contains(&e.to_key) {
                selected.insert(e.from_key.clone());
            }
        }
        if selected.len() == before {
            break;
        }
    }
    let affected = selected.clone();
    loop {
        let before = selected.len();
        for e in edges.iter().filter(|e| e.required) {
            if selected.contains(&e.from_key) {
                selected.insert(e.to_key.clone());
            }
        }
        if selected.len() == before {
            break;
        }
    }
    if selected.len() > request.limit {
        return Err(Error::BudgetExceeded {
            required: selected.len(),
            budget: request.limit,
        });
    }
    let at = now_millis()?;
    let last_events = store.last_event_times(project.id)?;
    let mut nodes = vec![];
    for key in &selected {
        if !works.contains_key(key) {
            continue;
        }
        let r = store.work_readiness(project.id, key, branch, at)?;
        let last_event_at = last_events.get(&r.work.item.meta.id).copied();
        nodes.push(json!({"key":key,"id":r.work.item.meta.id,"title":r.work.item.title,"status":r.work.item.status,"archived":r.work.item.archived,"ready":r.ready,"paths":r.work.item.paths,"diagnostics":r.diagnostics,"active_claims":r.active_claims,"last_event_at":last_event_at,"source_ref":r.work.item.meta.source_ref}));
    }
    let selected_edges: Vec<_> = edges.iter().filter(|e| selected.contains(&e.from_key)).map(|e| json!({"from":e.from_key,"to":e.to_key,"required":e.required,"source_ref":e.source_ref})).collect();
    let missing: BTreeSet<_> = edges
        .iter()
        .filter(|e| e.required && selected.contains(&e.from_key) && !works.contains_key(&e.to_key))
        .map(|e| e.to_key.clone())
        .collect();
    let scoped: Links = links
        .into_iter()
        .filter(|(k, _)| selected.contains(k))
        .collect();
    let found = cycle(&scoped);
    let sources: BTreeMap<_, _> = projected
        .iter()
        .filter(|w| selected.contains(&w.item.meta.external_key))
        .map(|w| (w.source.id.to_string(), w.source.fingerprint.clone()))
        .collect();
    let graph_fingerprint = fingerprint(&serde_json::to_vec(
        &json!({"nodes":nodes.iter().map(|n|json!([n["key"],n["status"],n["archived"],n["paths"]])).collect::<Vec<_>>(),"edges":selected_edges,"sources":sources}),
    )?);
    Ok(
        json!({"ok":true,"project_revision":project.project_revision,"branch":branch,"evaluated_at":at,"graph_fingerprint":graph_fingerprint,"sources":sources,"roots":request.roots,"affected":affected,"nodes":nodes,"edges":selected_edges,"missing_required":missing,"cycle":found,"graph_valid":missing.is_empty() && found.is_empty(),"complete":true,"execution_admitted":false,"next_action":"Use readiness, then consume context and acquire a claim before dispatch. Recheck after any source or revision change; the host owns concurrency and resource conflicts."}),
    )
}

/// Shared delivery outcome identity. Consumers hold the same ref; payload bytes
/// are not copied into each stream (pair with WS-030 adoption credentials).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SharedOutcomeRef {
    pub provider_work_key: String,
    pub artifact_sha256: String,
    pub contract_sha256: String,
}

pub fn reference_shared_outcome(
    provider_work_key: impl Into<String>,
    artifact_sha256: impl Into<String>,
    contract_sha256: impl Into<String>,
) -> SharedOutcomeRef {
    SharedOutcomeRef {
        provider_work_key: provider_work_key.into(),
        artifact_sha256: artifact_sha256.into(),
        contract_sha256: contract_sha256.into(),
    }
}

/// Deduplicate shared work keys so a graph projection never materializes the
/// same provider outcome twice for multiple consumer streams.
pub fn unique_shared_work_keys(keys: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
    keys.into_iter()
        .map(|k| k.into())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Ready only when every necessary dependency is in `satisfied` (completed local
/// work and/or WS-030 adoption-unlocked providers).
pub fn necessary_dependencies_ready(
    required: impl IntoIterator<Item = impl AsRef<str>>,
    satisfied: &BTreeSet<String>,
) -> std::result::Result<(), Vec<String>> {
    let missing: Vec<String> = required
        .into_iter()
        .map(|d| d.as_ref().to_string())
        .filter(|d| !satisfied.contains(d))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Validate a complete same-project workstream graph. Cross-stream chains such as
/// A1→B1→A2 are accepted; hard cycles return `DependencyBlocked` with the path.
pub fn validate_cross_stream_work_graph(
    catalog: &WorkstreamCatalog,
    work_ids: &[String],
    ownership: &[WorkstreamWorkBinding],
    edges: &[WorkstreamDependencyEdge],
) -> Result<()> {
    validate_workstream_graph(catalog, work_ids, ownership, edges).map_err(|error| match error {
        WorkstreamGraphError::Dependency(DependencyDagError::Cycle(path)) => {
            Error::DependencyBlocked(format!("required dependency cycle: {}", path.join(" -> ")))
        }
        WorkstreamGraphError::Dependency(DependencyDagError::MissingEndpoint)
        | WorkstreamGraphError::InvalidRequiredEndpoint => {
            Error::DependencyBlocked("required dependency endpoint is missing".into())
        }
        WorkstreamGraphError::InvalidOwnership => {
            Error::InvalidInput("invalid workstream ownership for graph validation".into())
        }
        WorkstreamGraphError::BudgetExceeded => Error::BudgetExceeded {
            required: work_ids.len().max(edges.len()),
            budget: MAX_WORKSTREAM_GRAPH_NODES,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cross_stream_fixture() -> (WorkstreamCatalog, Vec<String>, Vec<WorkstreamWorkBinding>) {
        let catalog = WorkstreamCatalog {
            version: WORKSTREAM_CATALOG_VERSION,
            project_id: "project".into(),
            legacy_default: None,
            workstreams: [1, 2]
                .into_iter()
                .map(|n| Workstream {
                    id: Id::from(n),
                    project_id: "project".into(),
                    external_key: format!("s{n}"),
                    title: format!("stream {n}"),
                    state: WorkstreamState::Active,
                    authority_version: 1,
                    goal_keys: vec![],
                    acceptance_contracts: vec![],
                })
                .collect(),
        };
        let ownership: Vec<_> = [("A1", 1), ("B1", 2), ("A2", 1)]
            .into_iter()
            .map(|(id, stream)| WorkstreamWorkBinding {
                project_id: "project".into(),
                workstream_id: Id::from(stream),
                work_item_id: id.into(),
            })
            .collect();
        let ids = ownership.iter().map(|b| b.work_item_id.clone()).collect();
        (catalog, ids, ownership)
    }

    #[test]
    fn cycle_reports_only_a_real_cycle_and_handles_a_long_chain() {
        let mut links: Links = (0..5000)
            .map(|i| {
                (
                    i.to_string(),
                    if i == 4999 {
                        BTreeSet::new()
                    } else {
                        BTreeSet::from([(i + 1).to_string()])
                    },
                )
            })
            .collect();
        assert!(cycle(&links).is_empty());
        links.get_mut("4999").unwrap().insert("4998".into());
        let found = cycle(&links);
        assert_eq!(found.len(), 3);
        assert_eq!(found.first(), found.last());
    }

    #[test]
    fn cross_stream_a_to_b_to_later_a_is_acyclic_and_hard_cycle_explains_path() {
        let (catalog, ids, own) = cross_stream_fixture();
        let edge = |from: usize, to: usize, required| WorkstreamDependencyEdge {
            from: own[from].clone(),
            to: own[to].clone(),
            required,
        };
        assert!(
            validate_cross_stream_work_graph(
                &catalog,
                &ids,
                &own,
                &[edge(0, 1, true), edge(1, 2, true)]
            )
            .is_ok()
        );
        let err = validate_cross_stream_work_graph(
            &catalog,
            &ids,
            &own,
            &[edge(0, 1, true), edge(1, 2, true), edge(2, 0, true)],
        )
        .unwrap_err();
        match err {
            Error::DependencyBlocked(message) => {
                assert!(
                    message.contains("A1 -> B1 -> A2 -> A1"),
                    "explainable path missing: {message}"
                );
            }
            other => panic!("unexpected {other}"),
        }
    }

    #[test]
    fn readiness_needs_all_necessary_deps_and_shared_outcomes_are_not_copied() {
        let satisfied = BTreeSet::from(["B1".into()]);
        assert_eq!(
            necessary_dependencies_ready(["B1", "C1"], &satisfied),
            Err(vec!["C1".into()])
        );
        let all = BTreeSet::from(["B1".into(), "C1".into()]);
        assert_eq!(necessary_dependencies_ready(["B1", "C1"], &all), Ok(()));
        let keys = unique_shared_work_keys(["A1", "B1", "A1", "A2"]);
        assert_eq!(
            keys,
            vec!["A1".to_string(), "A2".to_string(), "B1".to_string()]
        );
        let left = reference_shared_outcome("A1", "art", "contract");
        let right = reference_shared_outcome("A1", "art", "contract");
        assert_eq!(left, right);
    }
}
