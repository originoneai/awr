//! `work graph --cached` reads the last recorded projection: the content of a refreshing read at the
//! same revision, no writes, no business-file access, and an explicit error when nothing is recorded.
use awr_core::{EventDraft, Id};
use awr_store::Store;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const LEDGER: &str = concat!(
    "work_items:\n",
    "- id: DONE\n  title: Finished base\n  status: completed\n",
    "- id: GONE\n  title: Dropped base\n  status: cancelled\n",
    "- id: B\n  title: Build feature\n  status: ready\n  next_action: Implement query\n  depends_on: [DONE]\n  acceptance: [Preserve provenance]\n",
    "- id: C\n  title: Follow up\n  status: planned\n  depends_on: [B]\n",
    "- id: STUCK\n  title: Depends on dropped work\n  status: planned\n  depends_on: [GONE]\n",
    "- id: LOST\n  title: Depends on unknown work\n  status: planned\n  depends_on: [MISSING]\n",
);
const WORK_COUNT: usize = 6;

struct Fixture(PathBuf);
impl Fixture {
    fn uninitialized() -> Self {
        let root = std::env::temp_dir().join(format!("awr-work-graph-{}", Id::new()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("work-ledger.yaml"), LEDGER).unwrap();
        Self(root)
    }
    fn new() -> Self {
        let f = Self::uninitialized();
        f.ok(&["init", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        run(&self.0, args)
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
    /// A failed command reports one JSON error object on stderr.
    fn error(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert!(
            !r.status.success(),
            "{args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&r.stdout)
        );
        error_of(&r)
    }
    fn db(&self) -> PathBuf {
        self.0.join(".awr/state.db")
    }
    fn revision(&self) -> String {
        self.ok(&["session", "list"])["project_revision"].to_string()
    }
    fn runtime_files(&self) -> BTreeSet<OsString> {
        fs::read_dir(self.0.join(".awr"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
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

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["--project", root.to_str().unwrap(), "--json"])
        .args(args)
        .output()
        .unwrap()
}
fn error_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap_or_else(|e| {
        panic!(
            "stderr is not a JSON error ({e}): {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
/// Everything a graph consumer reads, minus the evaluation clock and how the projection was obtained.
fn content(graph: &Value) -> Value {
    let mut graph = graph.clone();
    let fields = graph.as_object_mut().unwrap();
    for key in [
        "evaluated_at",
        "freshness_basis",
        "source_refresh_performed",
        "read_only",
        "snapshot",
    ] {
        fields.remove(key);
    }
    graph
}
fn node<'a>(graph: &'a Value, key: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == key)
        .unwrap_or_else(|| panic!("graph has no node {key}"))
}
fn keys(graph: &Value) -> Vec<&str> {
    let mut keys: Vec<_> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["key"].as_str().unwrap())
        .collect();
    keys.sort();
    keys
}

#[test]
fn cached_graph_has_the_content_of_the_refreshing_graph_at_the_same_revision() {
    let f = Fixture::new();
    // A live claim is runtime state: the recorded view must carry it exactly as a refreshing view does.
    let revision = f.revision();
    f.ok(&[
        "session",
        "start",
        "--work",
        "B",
        "--claim",
        "--ttl-ms",
        "3600000",
        "--agent",
        "fixture",
        "--provider",
        "local",
        "--model",
        "none",
        "--expected-revision",
        &revision,
    ]);
    let refreshed = f.ok(&["work", "graph"]);
    // The default behavior is unchanged: it refreshes sources and says so.
    assert_eq!(refreshed["freshness_basis"], "source_refresh");
    assert_eq!(refreshed["source_refresh_performed"], true);
    assert_eq!(refreshed["read_only"], false);
    assert_eq!(refreshed["snapshot"]["source_currentness_verified"], true);

    let database = fs::read(f.db()).unwrap();
    let files = f.runtime_files();
    let cached = f.ok(&["work", "graph", "--cached"]);
    assert_eq!(
        fs::read(f.db()).unwrap(),
        database,
        "a cached read must not write the runtime database"
    );
    assert_eq!(f.runtime_files(), files);

    assert_eq!(cached["freshness_basis"], "last_recorded_source_state");
    assert_eq!(cached["source_refresh_performed"], false);
    assert_eq!(cached["read_only"], true);
    assert_eq!(cached["snapshot"]["coherent"], true);
    assert_eq!(cached["snapshot"]["source_currentness_verified"], false);
    assert!(cached["snapshot"]["source_refresh_revision"].is_null());
    assert_eq!(cached["project_revision"], refreshed["project_revision"]);
    assert_eq!(
        cached["snapshot"]["project_revision"],
        refreshed["snapshot"]["project_revision"]
    );
    assert_eq!(
        cached["snapshot"]["source_state_fingerprint"],
        refreshed["snapshot"]["source_state_fingerprint"]
    );
    assert_eq!(content(&cached), content(&refreshed));

    // The comparison is not vacuous: the closure, every prerequisite state and the claim are present.
    assert_eq!(cached["nodes"].as_array().unwrap().len(), WORK_COUNT);
    let claimed = node(&cached, "B");
    assert_eq!(claimed["ready"], false);
    assert_eq!(claimed["diagnostics"][0]["code"], "active_claim");
    assert_eq!(claimed["active_claims"].as_array().unwrap().len(), 1);
    assert_eq!(claimed["active_claims"][0]["agent_id"], "fixture");
    let waiting = node(&cached, "C")["diagnostics"][0].clone();
    assert_eq!(waiting["code"], "dependency_not_completed");
    assert_eq!(waiting["work_item_key"], "B");
    let cancelled = node(&cached, "STUCK")["diagnostics"][0].clone();
    assert_eq!(cancelled["code"], "dependency_not_completed");
    assert_eq!(cancelled["work_item_key"], "GONE");
    assert!(cancelled["detail"].as_str().unwrap().contains("cancelled"));
    assert_eq!(
        node(&cached, "LOST")["diagnostics"][0]["code"],
        "missing_dependency"
    );
    assert_eq!(cached["missing_required"][0], "MISSING");
    assert_eq!(cached["graph_valid"], false);
    assert_eq!(cached["edges"].as_array().unwrap().len(), 4);
}

#[test]
fn cached_graph_keeps_the_recorded_state_until_a_refresh_records_the_source_change() {
    let f = Fixture::new();
    let recorded = f.ok(&["work", "graph"]);
    assert_eq!(node(&recorded, "B")["status"], "ready");
    f.edit_ledger(
        "status: ready\n  next_action: Implement query",
        "status: blocked\n  blocker: Needs input",
    );

    // The recorded view does not open business files, so it cannot see the edit.
    let stale = f.ok(&["work", "graph", "--cached"]);
    assert_eq!(node(&stale, "B")["status"], "ready");
    assert_eq!(stale["project_revision"], recorded["project_revision"]);
    assert_eq!(stale["snapshot"]["source_currentness_verified"], false);
    assert_eq!(content(&stale), content(&recorded));

    // The default read still notices it and records it.
    let refreshed = f.ok(&["work", "graph"]);
    assert_eq!(node(&refreshed, "B")["status"], "blocked");
    assert!(
        refreshed["project_revision"].as_u64().unwrap()
            > recorded["project_revision"].as_u64().unwrap()
    );
    let after = f.ok(&["work", "graph", "--cached"]);
    assert_eq!(node(&after, "B")["status"], "blocked");
    assert_eq!(after["project_revision"], refreshed["project_revision"]);
    assert_eq!(content(&after), content(&refreshed));
}

#[test]
fn default_graph_reports_source_failures_while_cached_graph_serves_the_record() {
    let f = Fixture::new();
    let recorded = f.ok(&["work", "graph"]);
    fs::remove_file(f.0.join("work-ledger.yaml")).unwrap();

    let cached = f.ok(&["work", "graph", "--cached"]);
    assert_eq!(content(&cached), content(&recorded));
    assert_eq!(cached["snapshot"]["source_currentness_verified"], false);

    let failure = f.run(&["work", "graph"]);
    assert!(!failure.status.success());
    assert_eq!(error_of(&failure)["code"], "SourceStale");
}

#[test]
fn cached_graph_without_recorded_state_is_an_explicit_error_that_creates_nothing() {
    let f = Fixture::uninitialized();
    let missing = f.error(&["work", "graph", "--cached"]);
    assert_eq!(missing["code"], "SourceUnavailable");
    assert!(missing["message"].as_str().unwrap().contains(".awr"));
    assert!(!f.0.join(".awr").exists());

    // A runtime directory without a database has no snapshot to read either.
    fs::create_dir(f.0.join(".awr")).unwrap();
    let no_database = f.error(&["work", "graph", "--cached"]);
    assert_eq!(no_database["code"], "NotFound");
    assert!(
        no_database["message"]
            .as_str()
            .unwrap()
            .contains("initialize")
    );
    assert_eq!(f.runtime_files(), BTreeSet::new());

    // A database recorded for another project root holds no snapshot of this one: never an empty graph.
    let other = Fixture::new();
    fs::copy(other.db(), f.0.join(".awr/state.db")).unwrap();
    let foreign = f.error(&["work", "graph", "--cached"]);
    assert_eq!(foreign["code"], "NotFound");
    assert!(
        foreign["message"]
            .as_str()
            .unwrap()
            .contains("project root")
    );
}

#[test]
fn cached_graph_selects_roots_and_enforces_limits_like_the_default() {
    let f = Fixture::new();
    let rooted = f.ok(&["work", "graph", "--cached", "--root", "B"]);
    // B, its dependent C and the required ancestors of that affected set.
    assert_eq!(keys(&rooted), ["B", "C", "DONE"]);
    assert_eq!(rooted["affected"], serde_json::json!(["B", "C"]));
    assert_eq!(
        content(&rooted),
        content(&f.ok(&["work", "graph", "--root", "B"]))
    );

    let budget = f.error(&["work", "graph", "--cached", "--root", "B", "--limit", "2"]);
    assert_eq!(budget["code"], "BudgetExceeded");
    assert_eq!(
        budget,
        f.error(&["work", "graph", "--root", "B", "--limit", "2"])
    );
    assert_eq!(
        f.error(&["work", "graph", "--cached", "--limit", "0"])["code"],
        "InvalidInput"
    );
    assert_eq!(
        f.error(&["work", "graph", "--cached", "--root", "ABSENT"])["code"],
        "NotFound"
    );
}

#[test]
fn cached_graph_stays_coherent_while_runtime_writes_land() {
    const EVENTS: u64 = 40;
    let f = Fixture::new();
    let recorded = f.ok(&["work", "graph", "--cached"]);
    let first = recorded["project_revision"].as_u64().unwrap();
    let shape = recorded["graph_fingerprint"].clone();

    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (root, done) = (f.0.clone(), done.clone());
        thread::spawn(move || {
            let mut store = Store::open_existing(&root.join(".awr/state.db")).unwrap();
            for i in 0..EVENTS {
                let project = store.project_by_root(&root).unwrap();
                store
                    .append_event(
                        project.id,
                        project.project_revision,
                        EventDraft::new("work.progress", format!("Review pass {i}")),
                    )
                    .unwrap();
                thread::sleep(Duration::from_millis(5));
            }
            // Closing the connection may checkpoint the WAL; readers started afterwards see a settled database.
            drop(store);
            done.store(true, Ordering::SeqCst);
        })
    };
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let (root, done) = (f.0.clone(), done.clone());
            thread::spawn(move || {
                let mut seen = Vec::new();
                loop {
                    let finished = done.load(Ordering::SeqCst);
                    let output = run(&root, &["work", "graph", "--cached"]);
                    if output.status.success() {
                        let graph: Value = serde_json::from_slice(&output.stdout).unwrap();
                        // One capture: the revision, the flags and the graph all come from the same snapshot.
                        assert_eq!(graph["snapshot"]["coherent"], true);
                        assert_eq!(
                            graph["project_revision"],
                            graph["snapshot"]["project_revision"]
                        );
                        assert_eq!(graph["read_only"], true);
                        assert_eq!(graph["source_refresh_performed"], false);
                        assert_eq!(graph["nodes"].as_array().unwrap().len(), WORK_COUNT);
                        seen.push((
                            graph["project_revision"].as_u64().unwrap(),
                            graph["graph_fingerprint"].clone(),
                        ));
                    } else {
                        // A capture that overlaps a write is rejected whole, with the documented retryable code.
                        assert_eq!(error_of(&output)["code"], "SourceConflict");
                    }
                    if finished {
                        break;
                    }
                }
                seen
            })
        })
        .collect();
    writer.join().unwrap();
    for reader in readers {
        let seen = reader.join().unwrap();
        // The last attempt began after the writer finished, so it could not overlap a write.
        let (last, _) = *seen.last().expect("a read after the writes settled");
        assert_eq!(last, first + EVENTS);
        assert!(seen.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert!(seen.iter().all(|(revision, _)| *revision >= first));
        // Runtime events advance the revision but never change the graph itself.
        assert!(seen.iter().all(|(_, fingerprint)| *fingerprint == shape));
    }
    let settled = f.ok(&["work", "graph", "--cached"]);
    assert_eq!(settled["project_revision"], first + EVENTS);
    assert_eq!(content(&settled)["graph_fingerprint"], shape);
}

#[test]
fn hosts_can_discover_the_recorded_graph_read_under_the_snapshot_capability() {
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["capabilities", "--json", "--project", "no such project"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let capability = report["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "query.coherent_snapshot")
        .unwrap();
    assert!(
        capability["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "work graph")
    );
    assert!(
        capability["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "cached_mode_does_not_verify_current_sources")
    );
}
