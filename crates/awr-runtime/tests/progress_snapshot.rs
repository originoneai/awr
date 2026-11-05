use awr_core::{EventDraft, Id};
use awr_runtime::progress_snapshot;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Barrier},
    time::Duration,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let fixture =
            Self(std::env::temp_dir().join(format!("awr-progress-snapshot-{}", Id::new())));
        fs::create_dir_all(fixture.0.join(".awr")).unwrap();
        fs::write(fixture.0.join("work.yaml"),
            "goals:\n- id: G\n  title: Deliver a guide\n  status: active\nwork_items:\n- id: A\n  title: First task\n  status: ready\n  goal: G\n  milestone: M1\n  acceptance: [Useful guide]\n  depends_on: [B]\n  next_action: Implement the guide\n- id: B\n  title: Accepted draft\n  status: completed\n  goal: G\n  acceptance: [Reviewed draft]\n- id: C\n  title: Archived draft\n  status: completed\n  archived: true\n  goal: G\n  acceptance: [Retained draft]\n").unwrap();
        fs::write(fixture.0.join(".awr/project.toml"),
            "[project]\nname='Guide'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        let mut store = fixture.store();
        let indexed = index_project(
            &mut store,
            &fixture.0,
            &Manifest::load(&fixture.0).unwrap(),
            false,
        )
        .unwrap();
        assert!(indexed.ok, "{indexed:?}");
        fixture
    }
    fn store(&self) -> Store {
        Store::open(&self.0.join(".awr/state.db")).unwrap()
    }
    fn revision(&self) -> u64 {
        self.store()
            .project_by_root(&self.0)
            .unwrap()
            .project_revision
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_coherent(snapshot: &Value) {
    for key in ["work_graph", "nav", "goals", "sessions", "doctor", "events"] {
        assert_eq!(
            snapshot[key]["project_revision"], snapshot["project_revision"],
            "{key}"
        );
    }
    assert_eq!(snapshot["basis"]["coherent"], true);
}

#[test]
fn recorded_snapshot_preserves_facts_and_requires_explicit_source_sync() {
    let fixture = Fixture::new();
    let before = fixture.revision();
    let path = fixture.0.join("work.yaml");
    let edited = fs::read_to_string(&path)
        .unwrap()
        .replace("First task", "Updated task");
    fs::write(&path, &edited).unwrap();
    let recorded = progress_snapshot(&fixture.0, false).unwrap();
    assert_coherent(&recorded);
    assert_eq!(recorded["project_revision"], before);
    assert_eq!(recorded["basis"]["source_currentness_verified"], false);
    assert_eq!(recorded["basis"]["file_diagnostics_performed"], false);
    assert_eq!(recorded["work_graph"]["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(recorded["work_graph"]["nodes"][0]["title"], "First task");
    assert_eq!(recorded["work_graph"]["nodes"][0]["ready"], true);
    assert_eq!(recorded["work_graph"]["edges"][0]["to"], "B");
    assert_eq!(recorded["milestones"]["M1"][0], "A");
    assert_eq!(
        fixture.revision(),
        before,
        "Viewing must not update the project"
    );
    let synced = progress_snapshot(&fixture.0, true).unwrap();
    assert_coherent(&synced);
    assert_eq!(synced["work_graph"]["nodes"][0]["title"], "Updated task");
    assert_eq!(synced["basis"]["source_currentness_verified"], true);
    assert!(synced["project_revision"].as_u64().unwrap() > before);
    assert_eq!(fs::read_to_string(path).unwrap(), edited);
}

#[test]
fn concurrent_runtime_writes_never_mix_snapshot_revisions() {
    let fixture = Fixture::new();
    let root = fixture.0.clone();
    let barrier = Arc::new(Barrier::new(2));
    let ready = barrier.clone();
    let writer = std::thread::spawn(move || {
        let mut store = Store::open_existing(&root.join(".awr/state.db")).unwrap();
        ready.wait();
        for _ in 0..20 {
            let project = store.project_by_root(&root).unwrap();
            store
                .append_event(
                    project.id,
                    project.project_revision,
                    EventDraft::new("work.observed", "Retain a synthetic observation"),
                )
                .unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    barrier.wait();
    for _ in 0..5 {
        assert_coherent(&progress_snapshot(&fixture.0, false).unwrap());
    }
    writer.join().unwrap();
}

#[test]
fn unavailable_sources_remain_readable_as_recorded_facts_and_failed_sync_is_explicit() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.0.join("work.yaml")).unwrap();
    let recorded = progress_snapshot(&fixture.0, false).unwrap();
    assert_eq!(recorded["work_graph"]["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(recorded["basis"]["source_refresh_performed"], false);
    assert!(progress_snapshot(&fixture.0, true).is_err());
}
