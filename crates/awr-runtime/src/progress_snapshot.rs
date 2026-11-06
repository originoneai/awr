//! Bounded observation of one private SQLite snapshot, independent of source synchronization.
use awr_core::{Error, Result, now_millis};
use awr_store::Store;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// Read progress facts coherently. Source synchronization is an explicit caller action.
pub fn progress_snapshot(root: &Path, refresh: bool) -> Result<Value> {
    let root = root.canonicalize()?;
    let database = root.join(".awr/state.db");
    let store = if refresh {
        let mut origin = Store::open_existing(&database)?;
        let snapshot = awr_source::refresh_snapshot(&mut origin, &root)?;
        if !snapshot.refresh.ok {
            return Err(Error::SourceStale(
                "source synchronization was incomplete; inspect source status".into(),
            ));
        }
        snapshot.store
    } else {
        Store::open_readonly(&database)?.memory_snapshot(256 * 1024 * 1024)?
    };
    let project = store.project_by_root(&root)?;
    let mut graph = crate::work_graph(
        &store,
        &root,
        &crate::WorkGraphRequest {
            limit: 1000,
            roots: vec![],
            branch: None,
        },
    )?;
    let mut nav = crate::mainline_nav(
        &store,
        &project,
        &crate::MainlineNavScope::default(),
        crate::MainlineNavExtras::default(),
    )?;
    let works = store.work_items(project.id)?;
    let by_key: BTreeMap<_, _> = works
        .iter()
        .map(|work| (work.item.meta.external_key.as_str(), &work.item))
        .collect();
    for node in graph["nodes"].as_array_mut().expect("work graph nodes") {
        let work = by_key[node["key"].as_str().expect("work key")];
        node["milestone"] = json!(work.milestone);
        node["person_responsibility"] = json!(work.owner);
        node["blocker"] = json!(work.blocker);
        node["next_action"] = json!(work.next_action);
    }
    let mut milestones: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for work in &works {
        if !work.item.archived {
            if let Some(milestone) = &work.item.milestone {
                milestones
                    .entry(milestone.clone())
                    .or_default()
                    .push(work.item.meta.external_key.clone());
            }
        }
    }
    let goals = store.goals(project.id)?;
    let sessions = store.sessions(project.id, true, 100)?;
    let inspection = store.inspect_runtime(project.id, now_millis()?)?;
    let revision = project.project_revision;
    nav["project_revision"] = json!(revision);
    nav["freshness_basis"] = json!(if refresh {
        "source_refresh"
    } else {
        "last_recorded_source_state"
    });
    Ok(json!({
        "ok": true, "version": 1, "project_revision": revision,
        "basis": {"storage":"private_memory", "coherent":true,
            "freshness_basis":if refresh {"source_refresh"} else {"last_recorded_source_state"},
            "source_refresh_performed":refresh, "source_currentness_verified":refresh,
            "diagnostics_scope":"recorded_runtime", "file_diagnostics_performed":false},
        "work_graph":graph, "nav":nav, "milestones":milestones,
        "goals":{"project_revision":revision,"hits":goals.iter().take(100).map(|goal|
            json!({"external_key":goal.item.meta.external_key,"title":goal.item.title,"status":goal.item.status})
        ).collect::<Vec<_>>(),"truncated":goals.len()>100},
        "sessions":{"project_revision":revision,"sessions":sessions,"may_have_more":sessions.len()==100},
        "doctor":{"project_revision":revision,"scope":"recorded_runtime",
            "ok":inspection.findings.iter().all(|finding| finding.severity=="info"
                || crate::doctor::ADVISORY_FINDINGS.contains(&finding.code.as_str())),
            "findings":inspection.findings},
        "events":{"project_revision":revision,"events":[],"next_cursor":null}
    }))
}
