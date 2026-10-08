//! Mainline navigation explains every required dependency that readiness refuses, including cancelled
//! and archived ones, so a path is never shown as clear while `ready` still rejects it.
use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const LEDGER: &str = concat!(
    "work_items:\n",
    "- id: DONE\n  title: Finished base\n  status: completed\n",
    "- id: GONE\n  title: Dropped base\n  status: cancelled\n",
    "- id: OPEN\n  title: Open base\n  status: planned\n",
    "- id: DONE-ARCH\n  title: Finished and archived base\n  status: completed\n  archived: true\n",
    "- id: GONE-ARCH\n  title: Dropped and archived base\n  status: cancelled\n  archived: true\n",
    "- id: OPEN-ARCH\n  title: Open and archived base\n  status: planned\n  archived: true\n",
    "- id: OK\n  title: Needs completed\n  status: planned\n  depends_on: [DONE]\n",
    "- id: C1\n  title: Needs cancelled\n  status: planned\n  depends_on: [GONE]\n",
    "- id: C2\n  title: Needs the one that needs cancelled\n  status: planned\n  depends_on: [C1]\n",
    "- id: O1\n  title: Needs open\n  status: planned\n  depends_on: [OPEN]\n",
    "- id: A1\n  title: Needs archived completed\n  status: planned\n  depends_on: [DONE-ARCH]\n",
    "- id: A2\n  title: Needs archived cancelled\n  status: planned\n  depends_on: [GONE-ARCH]\n",
    "- id: A3\n  title: Needs archived open\n  status: planned\n  depends_on: [OPEN-ARCH]\n",
);
// Each dependent with its only required dependency.
const DEPENDENTS: [(&str, &str); 7] = [
    ("OK", "DONE"),
    ("C1", "GONE"),
    ("C2", "C1"),
    ("O1", "OPEN"),
    ("A1", "DONE-ARCH"),
    ("A2", "GONE-ARCH"),
    ("A3", "OPEN-ARCH"),
];

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-nav-waits-{}", Id::new()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("work-ledger.yaml"), LEDGER).unwrap();
        let f = Self(root);
        f.ok(&["init", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let mut json_args = vec!["--json"];
        json_args.extend_from_slice(args);
        let r = self.run(&json_args);
        assert!(
            r.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn edit_ledger(&self, from: &str, to: &str) {
        let path = self.0.join("work-ledger.yaml");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from));
        fs::write(path, text.replace(from, to)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn graph_node<'a>(graph: &'a Value, key: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == key)
        .unwrap_or_else(|| panic!("graph has no node {key}"))
}
fn nav_node<'a>(nav: &'a Value, key: &str) -> &'a Value {
    nav["mainline_graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["work_key"] == key)
        .unwrap_or_else(|| panic!("nav has no node {key}"))
}
/// `(kind, summary)` of every wait that nav reports for one work item.
fn waits(nav: &Value, key: &str) -> Vec<(String, String)> {
    nav_node(nav, key)["explainable_waits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["kind"].as_str().unwrap().to_string(),
                w["summary"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}
/// Whether readiness refuses `work` because of its dependency `dependency`.
fn refused_by_readiness(graph: &Value, work: &str, dependency: &str) -> bool {
    graph_node(graph, work)["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "dependency_not_completed" && d["work_item_key"] == dependency)
}

#[test]
fn nav_explains_exactly_the_dependencies_that_readiness_refuses() {
    let f = Fixture::new();
    let graph = f.ok(&["work", "graph"]);
    let nav = f.ok(&["nav", "--cached"]);
    for (work, dependency) in DEPENDENTS {
        let found = waits(&nav, work);
        assert_eq!(
            refused_by_readiness(&graph, work, dependency),
            !found.is_empty(),
            "{work} on {dependency}: readiness and navigation disagree ({found:?})"
        );
    }

    let outcome = |key: &str| {
        (
            "dependency_outcome".to_string(),
            format!("Waiting on outcome of {key}"),
        )
    };
    let cancelled = |key: &str| {
        (
            "dependency_cancelled".to_string(),
            format!("Required dependency {key} was cancelled"),
        )
    };
    // A completed dependency releases its dependent.
    assert!(waits(&nav, "OK").is_empty());
    // A cancelled dependency is a wait that only a change can release; its dependents wait on the live link.
    assert_eq!(waits(&nav, "C1"), [cancelled("GONE")]);
    assert_eq!(waits(&nav, "C2"), [outcome("C1")]);
    assert_eq!(waits(&nav, "O1"), [outcome("OPEN")]);
    // Archived dependencies are refused by readiness whatever their lifecycle status.
    assert_eq!(waits(&nav, "A1"), [outcome("DONE-ARCH")]);
    assert_eq!(waits(&nav, "A2"), [cancelled("GONE-ARCH")]);
    assert_eq!(waits(&nav, "A3"), [outcome("OPEN-ARCH")]);

    let wait = &nav_node(&nav, "C1")["explainable_waits"][0];
    assert!(
        wait["basis"]
            .as_str()
            .unwrap()
            .contains("Required dependency GONE (Dropped base) is cancelled")
    );
    assert_eq!(
        wait["release_condition"],
        "Remove or replace GONE in the dependencies of C1, or reopen GONE and complete it"
    );
}

#[test]
fn the_cancelled_wait_is_additive_and_visible_in_a_narrow_scope() {
    let f = Fixture::new();
    // The producer is outside the selected scope; the consumer still explains why it cannot proceed.
    let nav = f.ok(&["nav", "--cached", "--work", "C1"]);
    assert_eq!(nav["protocol"], "awr-mainline-nav");
    assert_eq!(nav["schema_version"], 1);
    assert_eq!(nav["scope"]["work_count"], 1);
    assert_eq!(waits(&nav, "C1")[0].0, "dependency_cancelled");
    assert_eq!(
        nav["guidance"]["when"],
        "Selected mainline is waiting on an explainable condition"
    );

    let clear = f.ok(&["nav", "--cached", "--work", "OK"]);
    assert!(waits(&clear, "OK").is_empty());
    assert_ne!(
        clear["guidance"]["when"],
        "Selected mainline is waiting on an explainable condition"
    );

    // Terminal and plain-text readers see the new kind through the generic wait output.
    let text = f.run(&["nav", "--cached", "--work", "C1"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("wait[dependency_cancelled]: Required dependency GONE was cancelled"));
}

#[test]
fn either_release_condition_clears_the_wait_and_readiness_agrees() {
    let f = Fixture::new();
    assert_eq!(waits(&f.ok(&["nav"]), "C1")[0].0, "dependency_cancelled");
    assert_eq!(graph_node(&f.ok(&["work", "graph"]), "C1")["ready"], false);

    // Replace the cancelled dependency with a completed one: no wait, ready.
    f.edit_ledger(
        "- id: C1\n  title: Needs cancelled\n  status: planned\n  depends_on: [GONE]",
        "- id: C1\n  title: Needs cancelled\n  status: planned\n  depends_on: [DONE]",
    );
    let replaced = f.ok(&["nav"]);
    assert!(waits(&replaced, "C1").is_empty());
    assert_eq!(graph_node(&f.ok(&["work", "graph"]), "C1")["ready"], true);

    // Reopen the cancelled work instead: the wait becomes an ordinary outcome wait.
    f.edit_ledger(
        "- id: C1\n  title: Needs cancelled\n  status: planned\n  depends_on: [DONE]",
        "- id: C1\n  title: Needs cancelled\n  status: planned\n  depends_on: [GONE]",
    );
    f.edit_ledger(
        "- id: GONE\n  title: Dropped base\n  status: cancelled",
        "- id: GONE\n  title: Dropped base\n  status: planned",
    );
    let reopened = f.ok(&["nav"]);
    assert_eq!(
        waits(&reopened, "C1"),
        [(
            "dependency_outcome".to_string(),
            "Waiting on outcome of GONE".to_string()
        )]
    );
}
