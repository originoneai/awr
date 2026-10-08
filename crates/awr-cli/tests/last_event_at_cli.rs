//! `work graph` and `nav` nodes carry `last_event_at`: the newest `created_at` that `event history` shows for
//! the work over every session and branch, or null when the work has no event.
use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const LEDGER: &str = concat!(
    "work_items:\n",
    "- id: BUSY\n  title: Session and appended events\n  status: ready\n  next_action: Continue\n  acceptance: [Done]\n",
    "- id: SESSION-ONLY\n  title: Only session events\n  status: ready\n  next_action: Continue\n  acceptance: [Done]\n",
    "- id: QUIET\n  title: Never touched\n  status: planned\n",
    "- id: BRANCHED\n  title: Events on a branch only\n  status: ready\n  next_action: Continue\n  acceptance: [Done]\n",
    "- id: RETIRED\n  title: Will be archived\n  status: completed\n",
);
const KEYS: [&str; 5] = ["BUSY", "SESSION-ONLY", "QUIET", "BRANCHED", "RETIRED"];

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-last-event-{}", Id::new()));
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
    fn revision(&self) -> String {
        self.ok(&["session", "list"])["project_revision"].to_string()
    }
    fn start(&self, work: &str) -> String {
        let revision = self.revision();
        self.ok(&[
            "session",
            "start",
            "--work",
            work,
            "--agent",
            "fixture",
            "--provider",
            "local",
            "--model",
            "none",
            "--expected-revision",
            &revision,
        ])["session"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
    fn end(&self, session: &str) {
        let revision = self.revision();
        self.ok(&[
            "session",
            "end",
            "--session",
            session,
            "--expected-revision",
            &revision,
        ]);
    }
    /// Appends a generic event and returns its `created_at`.
    fn observe(&self, work: &str, branch: Option<&str>) -> i64 {
        let revision = self.revision();
        let mut args = vec![
            "event",
            "append",
            "--type",
            "work.observed",
            "--work",
            work,
            "--summary",
            "Observation",
            "--expected-revision",
            &revision,
        ];
        if let Some(branch) = branch {
            args.extend(["--branch", branch]);
        }
        self.ok(&args)["event"]["created_at"].as_i64().unwrap()
    }
    /// The largest `created_at` of `event history --work`, over every branch, or None without events.
    fn newest_in_history(&self, work: &str) -> Option<i64> {
        let page = self.ok(&["event", "history", "--work", work, "--limit", "1000"]);
        assert!(
            page["next_cursor"].is_null(),
            "fixture stays within one page"
        );
        page["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["created_at"].as_i64().unwrap())
            .max()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn graph_value(graph: &Value, key: &str) -> Option<i64> {
    let node = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == key)
        .unwrap_or_else(|| panic!("graph has no node {key}"));
    assert!(node.get("last_event_at").is_some(), "{key} lacks the field");
    node["last_event_at"].as_i64()
}
fn nav_value(nav: &Value, key: &str) -> Option<i64> {
    let node = nav["mainline_graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["work_key"] == key)
        .unwrap_or_else(|| panic!("nav has no node {key}"));
    assert!(node.get("last_event_at").is_some(), "{key} lacks the field");
    node["last_event_at"].as_i64()
}

#[test]
fn nodes_carry_the_newest_event_time_that_event_history_shows_for_each_work() {
    let f = Fixture::new();
    // Nothing happened to any work yet: the field is present and null (source receipts belong to no work).
    let graph = f.ok(&["work", "graph"]);
    for key in KEYS {
        assert_eq!(graph_value(&graph, key), None, "{key}");
    }

    // BUSY: a session plus a later appended event. SESSION-ONLY: session events only.
    let busy = f.start("BUSY");
    let appended = f.observe("BUSY", None);
    let session_only = f.start("SESSION-ONLY");
    f.end(&session_only);
    f.end(&busy);

    let graph = f.ok(&["work", "graph"]);
    let nav = f.ok(&["nav"]);
    for key in ["BUSY", "SESSION-ONLY"] {
        let expected = f.newest_in_history(key);
        assert!(expected.is_some());
        assert_eq!(graph_value(&graph, key), expected, "{key} in work graph");
        assert_eq!(nav_value(&nav, key), expected, "{key} in nav");
    }
    // The session end came after the appended event, so it is the newest thing BUSY did.
    assert!(graph_value(&graph, "BUSY").unwrap() >= appended);
    // A work without events stays null in both views.
    assert_eq!(graph_value(&graph, "QUIET"), None);
    assert_eq!(nav_value(&nav, "QUIET"), None);
    // The recorded navigation view carries the same values.
    let cached_nav = f.ok(&["nav", "--cached"]);
    for key in KEYS {
        assert_eq!(nav_value(&cached_nav, key), nav_value(&nav, key), "{key}");
    }
}

#[test]
fn branch_events_count_whichever_branch_the_graph_is_evaluated_on() {
    let f = Fixture::new();
    let revision = f.revision();
    f.ok(&[
        "branch",
        "create",
        "review",
        "--actor",
        "fixture",
        "--reason",
        "Review the branched work",
        "--expected-revision",
        &revision,
    ]);
    let on_branch = f.observe("BRANCHED", Some("review"));
    // The main line has no event for this work; event history on any branch has the one.
    assert_eq!(f.newest_in_history("BRANCHED"), Some(on_branch));
    let main = f.ok(&["event", "history", "--work", "BRANCHED", "--main"]);
    assert!(main["events"].as_array().unwrap().is_empty());

    for args in [
        &["work", "graph"][..],
        &["work", "graph", "--branch", "review"][..],
    ] {
        assert_eq!(
            graph_value(&f.ok(args), "BRANCHED"),
            Some(on_branch),
            "{args:?}"
        );
    }
    assert_eq!(nav_value(&f.ok(&["nav"]), "BRANCHED"), Some(on_branch));
}

#[test]
fn archived_work_keeps_its_history_and_the_graph_shape_ignores_activity() {
    let f = Fixture::new();
    let session = f.start("RETIRED");
    f.end(&session);
    let before = f.ok(&["work", "graph"]);
    let retired = f.newest_in_history("RETIRED");
    assert_eq!(graph_value(&before, "RETIRED"), retired);

    // Archiving is a source change; the runtime history stays with the work.
    let path = f.0.join("work-ledger.yaml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace(
            "  title: Will be archived\n  status: completed\n",
            "  title: Will be archived\n  status: completed\n  archived: true\n",
        ),
    )
    .unwrap();
    let after = f.ok(&["work", "graph"]);
    let node = after["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == "RETIRED")
        .unwrap();
    assert_eq!(node["archived"], true);
    assert_eq!(graph_value(&after, "RETIRED"), retired);

    // More activity changes the timestamp but not the graph fingerprint or any other node field.
    let fingerprint = after["graph_fingerprint"].clone();
    f.observe("BUSY", None);
    let busy_session = f.start("BUSY");
    f.end(&busy_session);
    let later = f.ok(&["work", "graph"]);
    assert_eq!(later["graph_fingerprint"], fingerprint);
    assert!(graph_value(&later, "BUSY").is_some());
    let strip = |graph: &Value| {
        let mut nodes = graph["nodes"].clone();
        for n in nodes.as_array_mut().unwrap() {
            n.as_object_mut().unwrap().remove("last_event_at");
            n.as_object_mut().unwrap().remove("active_claims");
        }
        nodes
    };
    assert_eq!(strip(&later), strip(&after));
}
