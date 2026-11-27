use awr_core::{EntityKind, Id, Source};
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-github-pr-{}", Id::new()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("pulls.json"),
            include_str!("../../../tests/fixtures/github-pr/pulls.json"),
        )
        .unwrap();
        Self(root)
    }
    fn write(&self, content: &str) {
        fs::write(self.0.join("pulls.json"), content).unwrap();
    }
    fn store(&self) -> Store {
        Store::open(&self.0.join("state.db")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn manifest() -> Manifest {
    Manifest::parse(
        "[project]\nname='PR fixture'\n\
[[sources]]\ndomain='ledger'\nrole='primary'\npath='pulls.json'\nadapter='github-pr-snapshot-v1'\n",
    )
    .unwrap()
}
fn source(store: &Store, project: Id) -> Source {
    store
        .sources(project)
        .unwrap()
        .into_iter()
        .find(|source| source.domain == "ledger")
        .unwrap()
}
fn work(store: &Store, source: &Source) -> BTreeMap<String, Value> {
    store
        .source_projection_payloads(source, EntityKind::WorkItem)
        .unwrap()
        .into_iter()
        .map(|item| (item["external_key"].as_str().unwrap().into(), item))
        .collect()
}

#[test]
fn snapshot_projects_mapped_work() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let report = index_project(&mut store, &fixture.0, &manifest(), false).unwrap();
    assert!(report.ok, "{:?}", report.issues);
    assert_eq!(report.indexed, 1);
    let source = source(&store, report.project_id);
    let work = work(&store, &source);
    assert_eq!(work.len(), 8);

    let review = &work["example-org/widgets#517"];
    assert_eq!(review["status"], "blocked");
    assert_eq!(review["raw_status"], "OPEN");
    assert_eq!(review["blocker"], "Waiting for review.");
    assert_eq!(review["next_action"], "Wait for review.");
    assert_eq!(review["kind"], "github-pr");
    assert_eq!(review["owner"], "octocat");
    assert_eq!(review["milestone"], "v0.7");
    assert_eq!(review["tags"], serde_json::json!(["enhancement", "source"]));
    assert_eq!(
        review["paths"],
        serde_json::json!(["crates/awr-source/src/locator.rs"])
    );
    assert_eq!(review["source_ref"]["pointer"], "/0");
    assert!(review["source_ref"]["section_fingerprint"].is_string());

    let draft = &work["example-org/widgets#408"];
    assert_eq!(draft["status"], "draft");
    assert_eq!(
        draft["next_action"],
        "Mark the pull request ready for review."
    );

    let changes = &work["example-org/widgets#502"];
    assert_eq!(changes["status"], "in_progress");
    assert_eq!(changes["next_action"], "Address the review feedback.");
    assert!(changes["blocker"].is_null());

    let approved = &work["example-org/widgets#490"];
    assert_eq!(approved["status"], "blocked");
    assert_eq!(
        approved["blocker"],
        "Approved; waiting for a maintainer to merge."
    );

    assert_eq!(work["example-org/widgets#355"]["status"], "completed");
    assert_eq!(work["example-org/widgets#355"]["raw_status"], "MERGED");
    assert_eq!(work["example-org/widgets#340"]["status"], "cancelled");

    // Unknown review decisions never guess a transition.
    assert_eq!(work["example-org/widgets#610"]["status"], "unknown");
    let warnings = &report.sources[0].warnings;
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("reviewDecision") && w.contains("COMMENTED")),
        "{warnings:?}"
    );

    // Same number in another repository keeps a distinct identity, and an
    // empty reviewDecision reads as awaiting review.
    let mirror = &work["other-org/gadgets#502"];
    assert_eq!(mirror["status"], "blocked");
    assert_eq!(mirror["blocker"], "Waiting for review.");
}

#[test]
fn reindex_is_a_noop_and_tracks_state_changes() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let manifest = manifest();
    let first = index_project(&mut store, &fixture.0, &manifest, false).unwrap();
    assert!(first.ok);
    let source = source(&store, first.project_id);
    let before = work(&store, &source);
    let repeat = index_project(&mut store, &fixture.0, &manifest, false).unwrap();
    assert!(repeat.ok);
    assert_eq!((repeat.indexed, repeat.unchanged), (0, 1));
    assert_eq!(work(&store, &source), before);

    let path = fixture.0.join("pulls.json");
    let mut document: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    document[2]["state"] = "MERGED".into();
    document[2]["mergedAt"] = "2026-11-10T08:00:00Z".into();
    fixture.write(&serde_json::to_string_pretty(&document).unwrap());

    let changed = index_project(&mut store, &fixture.0, &manifest, false).unwrap();
    assert!(changed.ok, "{:?}", changed.issues);
    assert_eq!((changed.indexed, changed.unchanged), (1, 0));
    let after = work(&store, &source);
    let key = "example-org/widgets#502";
    assert_eq!(after[key]["status"], "completed");
    assert_eq!(after[key]["raw_status"], "MERGED");
    assert_eq!(after[key]["id"], before[key]["id"]);
    assert!(after[key]["revision"].as_i64() > before[key]["revision"].as_i64());
    // Untouched entries keep identity, revision and section binding; only the
    // source-level stamps move with the new snapshot.
    let untouched_before = &before["example-org/widgets#517"];
    let untouched_after = &after["example-org/widgets#517"];
    assert_eq!(untouched_after["id"], untouched_before["id"]);
    assert_eq!(untouched_after["revision"], untouched_before["revision"]);
    assert_eq!(
        untouched_after["source_ref"]["section_fingerprint"],
        untouched_before["source_ref"]["section_fingerprint"]
    );
    assert_eq!(untouched_after["status"], untouched_before["status"]);
}

#[test]
fn empty_snapshot_projects_no_work() {
    let fixture = Fixture::new();
    fixture.write("[]\n");
    let mut store = fixture.store();
    let report = index_project(&mut store, &fixture.0, &manifest(), false).unwrap();
    assert!(report.ok, "{:?}", report.issues);
    let source = source(&store, report.project_id);
    assert!(work(&store, &source).is_empty());
}

fn rejected(content: &str) -> (String, String) {
    let fixture = Fixture::new();
    fixture.write(content);
    let mut store = fixture.store();
    let report = index_project(&mut store, &fixture.0, &manifest(), false).unwrap();
    assert!(!report.ok);
    assert_eq!(report.issues.len(), 1);
    (
        report.issues[0].code.clone(),
        report.issues[0].message.clone(),
    )
}

#[test]
fn invalid_json_reports_syntax_rule() {
    let (code, message) = rejected("{ not an array ");
    assert_eq!(code, "InvalidInput");
    assert!(message.contains("invalid JSON"), "{message}");
}

#[test]
fn non_array_root_is_rejected() {
    let (code, message) = rejected("{\"items\": []}");
    assert_eq!(code, "InvalidInput");
    assert!(message.contains("JSON array"), "{message}");
}

#[test]
fn mismatched_url_is_rejected() {
    let (code, message) = rejected(
        r#"[{"number": 517, "title": "t", "state": "OPEN",
"url": "https://github.com/example-org/widgets/pull/518"}]"#,
    );
    assert_eq!(code, "InvalidInput");
    assert!(message.contains("html URL"), "{message}");
}

#[test]
fn duplicate_keys_are_rejected() {
    let (code, message) = rejected(
        r#"[{"number": 1, "title": "a", "state": "OPEN", "url": "https://github.com/o/r/pull/1"},
{"number": 1, "title": "b", "state": "MERGED", "url": "https://github.com/o/r/pull/1"}]"#,
    );
    assert_eq!(code, "SourceConflict");
    assert!(message.contains("duplicate pull request key"), "{message}");
}

#[test]
fn missing_state_is_rejected() {
    let (code, message) =
        rejected(r#"[{"number": 1, "title": "t", "url": "https://github.com/o/r/pull/1"}]"#);
    assert_eq!(code, "InvalidInput");
    assert!(message.contains("state"), "{message}");
}

#[test]
fn options_are_rejected_at_manifest() {
    let error = Manifest::parse(
        "[project]\nname='PR fixture'\n\
[[sources]]\ndomain='ledger'\nrole='primary'\npath='pulls.json'\nadapter='github-pr-snapshot-v1'\n\
[sources.options]\nfield_map = {}\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("no adapter options"), "{error}");
}

#[test]
fn non_ledger_domain_is_rejected() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let manifest = Manifest::parse(
        "[project]\nname='PR fixture'\n\
[[sources]]\ndomain='goal'\nrole='primary'\npath='pulls.json'\nadapter='github-pr-snapshot-v1'\n",
    )
    .unwrap();
    let report = index_project(&mut store, &fixture.0, &manifest, false).unwrap();
    assert!(!report.ok);
    assert!(
        report.issues[0].message.contains("ledger domain"),
        "{:?}",
        report.issues[0].message
    );
}
