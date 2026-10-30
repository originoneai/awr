//! The runtime fills the checker from the real projections: the workstream each work is bound to,
//! the declared goals and success criteria, and the work's own dependency links. Parent relations
//! across workstreams are a gap; a unit test with invented facts could not show that the bindings
//! are really read.
use awr_core::*;
use awr_runtime::work_definition_report;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const OUTCOME: &str = "Users keep their input when saving fails";

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn definition(parent: Option<&str>) -> Value {
    let mut definition = json!({
        "version": 1, "purpose": "implementation",
        "contributes": [{"goal": "api-delivery", "outcome": OUTCOME,
                         "claim": "The failure path keeps the draft", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge",
                        "by": "agent-a"}
    });
    if let Some(parent) = parent {
        definition["parent"] = json!(parent);
    }
    definition
}

fn project() -> (Project, Store, Id) {
    let root = std::env::temp_dir().join(format!("awr-work-definition-runtime-{}", Id::new()));
    fs::create_dir_all(root.join(".awr")).unwrap();
    let ledger = json!({
        "workstreams": {"version": 1, "definitions": [
            {"id": "01K00000000000000000000001", "external_key": "api", "title": "API",
             "state": "active", "authority_version": 1, "goal_keys": ["api-delivery"],
             "acceptance_contracts": []},
            {"id": "01K00000000000000000000002", "external_key": "client", "title": "Client",
             "state": "active", "authority_version": 1, "goal_keys": ["client-delivery"],
             "acceptance_contracts": []}
        ]},
        "goals": [
            {"id": "api-delivery", "title": "Deliver the API", "status": "active",
             "success_criteria": [OUTCOME]},
            {"id": "client-delivery", "title": "Deliver the client", "status": "active"}
        ],
        "work_items": [
            {"id": "API-0", "title": "The whole API", "status": "in_progress", "workstream": "api",
             "next_action": "Split it", "acceptance": ["The API works"],
             "definition": definition(None)},
            {"id": "API-1", "title": "First step", "status": "in_progress", "workstream": "api",
             "next_action": "Go", "acceptance": ["Step one works"], "depends_on": ["API-0"],
             "definition": definition(Some("API-0"))},
            {"id": "API-2", "title": "Second step", "status": "in_progress", "workstream": "api",
             "next_action": "Go", "acceptance": ["Step two works"],
             "definition": definition(Some("CLIENT-1"))},
            {"id": "API-3", "title": "Third step", "status": "in_progress", "workstream": "api",
             "next_action": "Go", "acceptance": ["Step three works"],
             "definition": definition(Some("API-404"))},
            {"id": "CLIENT-1", "title": "Client work", "status": "in_progress",
             "workstream": "client", "next_action": "Go", "acceptance": ["The client works"]}
        ]
    });
    fs::write(
        root.join("work.yaml"),
        serde_json::to_vec_pretty(&ledger).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join(".awr/project.toml"),
        "[project]\nname = \"Definition runtime fixture\"\nexternal_key = \"definition-runtime-fixture\"\n\n[[sources]]\ndomain = \"ledger\"\nrole = \"primary\"\npath = \"work.yaml\"\nadapter = \"yaml-workstream-ledger-v1\"\n",
    )
    .unwrap();
    let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
    let indexed = index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
    assert!(indexed.ok, "{indexed:?}");
    (Project(root), store, indexed.project_id)
}

fn codes(report: &DefinitionReport) -> Vec<&'static str> {
    report.gaps.iter().map(|gap| gap.code).collect()
}

fn state<'a>(report: &'a DefinitionReport, obligation: &str) -> &'a str {
    report
        .obligations
        .iter()
        .find(|o| o.obligation == obligation)
        .unwrap_or_else(|| panic!("no obligation {obligation}"))
        .state
}

#[test]
fn a_parent_is_compared_with_the_workstream_the_projection_binds_each_work_to() {
    let (_keep, store, project) = project();
    let same = work_definition_report(&store, project, "API-1").unwrap();
    assert!(
        !codes(&same).contains(&"parent_cross_workstream"),
        "{same:?}"
    );
    assert!(!codes(&same).contains(&"parent_not_found"), "{same:?}");
    assert_eq!(state(&same, "parent"), "satisfied");

    let across = work_definition_report(&store, project, "API-2").unwrap();
    assert!(
        codes(&across).contains(&"parent_cross_workstream"),
        "{across:?}"
    );
    assert_eq!(state(&across, "parent"), "unknown");

    let unknown = work_definition_report(&store, project, "API-3").unwrap();
    assert!(codes(&unknown).contains(&"parent_not_found"), "{unknown:?}");
    assert!(!codes(&unknown).contains(&"parent_cross_workstream"));
}

#[test]
fn goals_and_prerequisites_are_read_from_the_indexed_project() {
    let (_keep, store, project) = project();
    // The goal key and the exact success criterion resolve against the projected goal.
    let report = work_definition_report(&store, project, "API-1").unwrap();
    for code in [
        "contribution_goal_not_found",
        "contribution_outcome_unknown",
    ] {
        assert!(!codes(&report).contains(&code), "{code}: {report:?}");
    }
    assert_eq!(state(&report, "contribution"), "satisfied");
    // Only the work that declares a dependency has a prerequisite.
    assert_eq!(state(&report, "prerequisites"), "satisfied");
    let root = work_definition_report(&store, project, "API-0").unwrap();
    assert_eq!(state(&root, "prerequisites"), "not_applicable");
}

#[test]
fn work_without_a_definition_reports_nothing_and_an_unknown_key_is_not_found() {
    let (_keep, store, project) = project();
    let plain = work_definition_report(&store, project, "CLIENT-1").unwrap();
    assert_eq!(plain.state, "not_declared");
    assert!(plain.obligations.is_empty() && plain.gaps.is_empty());
    assert!(matches!(
        work_definition_report(&store, project, "NOPE-1"),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn a_stale_workstream_catalog_is_reported_and_never_skipped() {
    let (keep, mut store, project) = project();
    // A ledger that no longer parses leaves the earlier projection in place, marked stale.
    fs::write(keep.0.join("work.yaml"), "workstreams: [").unwrap();
    let again = index_project(
        &mut store,
        &keep.0,
        &Manifest::load(&keep.0).unwrap(),
        false,
    )
    .unwrap();
    assert!(!again.ok);
    assert!(matches!(
        work_definition_report(&store, project, "API-2"),
        Err(Error::SourceStale(_))
    ));
    // Work whose definition names no parent never needs the catalog.
    assert!(work_definition_report(&store, project, "API-0").is_ok());
}
