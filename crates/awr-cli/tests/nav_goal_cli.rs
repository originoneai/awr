//! `nav --goal` selects the work whose source declares the goal, keeps matching tags, and reports an
//! unknown goal instead of returning an empty selection.
use awr_core::Id;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const LEDGER: &str = concat!(
    "goals:\n",
    "- id: G1\n  title: First goal\n  status: active\n",
    "- id: G2\n  title: Second goal\n  status: active\n",
    "- id: G3\n  title: A goal that has no work yet\n  status: active\n",
    "work_items:\n",
    "- id: W1\n  title: Declares the first goal\n  status: ready\n  goal: [G1]\n  milestone: M1\n",
    "- id: W2\n  title: Serves two goals\n  status: in_progress\n  goal: [G1, G2]\n  milestone: M2\n",
    "- id: W3\n  title: Cancelled but still declared\n  status: cancelled\n  goal: G2\n",
    "- id: W4\n  title: Archived\n  status: ready\n  goal: [G1]\n  archived: true\n",
    "- id: W5\n  title: Carries a free-form tag\n  status: ready\n  tags: [LEGACY-GOAL]\n",
    "- id: W6\n  title: Names no goal\n  status: ready\n",
);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-nav-goal-{}", Id::new()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("work-ledger.yaml"), LEDGER).unwrap();
        let f = Self(root);
        f.ok(&["init", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert!(
            r.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn error(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert!(
            !r.status.success(),
            "{args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&r.stdout)
        );
        serde_json::from_slice(&r.stderr).unwrap()
    }
    fn selection(&self, args: &[&str]) -> Value {
        self.ok(args)["scope"]["selection"].clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_goal_selects_the_work_that_declares_it_in_the_source() {
    let f = Fixture::new();
    // The archived W4 is out of every navigation, the cancelled W3 is still shown with its status.
    assert_eq!(
        f.selection(&["nav", "--cached", "--goal", "G1"]),
        json!(["W1", "W2"])
    );
    assert_eq!(
        f.selection(&["nav", "--cached", "--goal", "G2"]),
        json!(["W2", "W3"])
    );
    let nav = f.ok(&["nav", "--cached", "--goal", "G2"]);
    assert_eq!(nav["scope"]["work_count"], 2);
    assert_eq!(nav["scope"]["requested"]["goal"], "G2");
    let statuses: Vec<_> = nav["mainline_graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["work_key"].as_str().unwrap(),
                n["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(statuses, [("W2", "in_progress"), ("W3", "cancelled")]);
    // Protocol and fields are untouched.
    assert_eq!(nav["protocol"], "awr-mainline-nav");
    assert_eq!(nav["schema_version"], 1);
}

#[test]
fn tags_keep_working_and_other_selectors_still_narrow_the_goal() {
    let f = Fixture::new();
    // A free-form tag that no goal object defines still selects the tagged work, as before.
    assert_eq!(
        f.selection(&["nav", "--cached", "--goal", "LEGACY-GOAL"]),
        json!(["W5"])
    );
    assert_eq!(
        f.selection(&["nav", "--cached", "--goal", "G1", "--milestone", "M2"]),
        json!(["W2"])
    );
    assert_eq!(
        f.selection(&["nav", "--cached", "--goal", "G1", "--work", "W1"]),
        json!(["W1"])
    );
    // No goal selector: the whole project, unchanged.
    assert_eq!(f.ok(&["nav", "--cached"])["scope"]["work_count"], json!(5));
}

#[test]
fn an_unknown_goal_is_reported_but_a_goal_without_work_is_a_real_empty_selection() {
    let f = Fixture::new();
    let unknown = f.error(&["nav", "--cached", "--goal", "NOPE"]);
    assert_eq!(unknown["code"], "NotFound");
    assert!(unknown["message"].as_str().unwrap().contains("goal NOPE"));

    // G3 exists and nothing supports it yet: an empty selection, said so by the guidance.
    let empty = f.ok(&["nav", "--cached", "--goal", "G3"]);
    assert_eq!(empty["scope"]["work_count"], 0);
    assert_eq!(
        empty["guidance"]["when"],
        "No actionable work in this mainline selection"
    );
}

#[test]
fn the_goal_selection_follows_source_edits_after_a_refresh() {
    let f = Fixture::new();
    assert_eq!(f.selection(&["nav", "--goal", "G3"]), json!([]));
    let path = f.0.join("work-ledger.yaml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace(
            "- id: W6\n  title: Names no goal\n  status: ready\n",
            "- id: W6\n  title: Names no goal\n  status: ready\n  goal: G3\n",
        ),
    )
    .unwrap();
    assert_eq!(f.selection(&["nav", "--goal", "G3"]), json!(["W6"]));
    // The recorded view serves what the last refresh recorded.
    assert_eq!(
        f.selection(&["nav", "--cached", "--goal", "G3"]),
        json!(["W6"])
    );
}
