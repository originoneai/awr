use awr_core::{EntityKind, Id};
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::Value;
use std::{fs, path::PathBuf};

const LEDGER: &str = r#"goals:
  G1:
    title: Keep planning quality visible
    status: active
    success_criteria: [Mechanical checks only]
work_items:
  - id: W1
    title: Normal Chinese acceptance entries
    status: ready
    goal: G1
    acceptance:
      - "单元测试覆盖全部分支，行覆盖率不低于 90%"
      - "回归测试锁定三个场景：无覆盖、孤儿、正常"
  - id: W2
    title: Normal English acceptance entries
    status: ready
    goal: G1
    acceptance:
      - "All unit tests pass with 90% line coverage"
      - "The projection payload includes the source fingerprint"
  - id: W3
    title: Empty acceptance entries
    status: ready
    goal: G1
    acceptance:
      - ""
      - "   "
  - id: W4
    title: Placeholder acceptance entries
    status: ready
    goal: G1
    acceptance:
      - "优化相关流程，等等"
      - "Works properly and delivers good results"
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-acceptance-lint-{}", Id::new()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ledger.yaml"), LEDGER).unwrap();
        Self(root)
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
        "[project]\nname='Acceptance lint fixture'\nexternal_key='acceptance-lint'\n\
[[sources]]\ndomain='ledger'\nrole='primary'\npath='ledger.yaml'\nadapter='yaml-ledger-v1'\n",
    )
    .unwrap()
}
fn lint_issues(report: &awr_source::IndexReport) -> Vec<Value> {
    report
        .issues
        .iter()
        .filter(|issue| issue.code.starts_with("acceptance_lint."))
        .map(|issue| serde_json::to_value(issue).unwrap())
        .collect()
}

#[test]
fn normal_entries_produce_no_lint_advisories() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let report = index_project(&mut store, &fixture.0, &manifest(), false).unwrap();
    assert!(report.ok, "advisories must never flip report.ok");
    // W1 and W2 carry only normal entries; their acceptance indexes 0 and 1
    // must stay silent even though W3/W4 in the same ledger carry findings.
    let issues = lint_issues(&report);
    let normal_hits: Vec<_> = issues
        .iter()
        .filter(|issue| {
            let locator = issue["locator"].as_str().unwrap_or_default();
            locator.contains("#/work_items/0/") || locator.contains("#/work_items/1/")
        })
        .collect();
    assert!(
        normal_hits.is_empty(),
        "normal entries must stay silent, got: {normal_hits:?}"
    );
}

#[test]
fn empty_entries_are_reported_with_field_locator() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let report = index_project(&mut store, &fixture.0, &manifest(), false).unwrap();
    assert!(report.ok, "advisories must never flip report.ok");
    let issues = lint_issues(&report);
    let empty: Vec<_> = issues
        .iter()
        .filter(|issue| issue["code"] == "acceptance_lint.empty")
        .collect();
    assert_eq!(
        empty.len(),
        2,
        "both blank entries are reported: {issues:?}"
    );
    let locators: Vec<_> = empty
        .iter()
        .map(|issue| issue["locator"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        locators
            .iter()
            .all(|l| l.contains("/work_items/") && l.contains("/acceptance/")),
        "locator must be field level: {locators:?}"
    );
    assert!(
        locators.iter().any(|l| l.ends_with("/acceptance/0"))
            && locators.iter().any(|l| l.ends_with("/acceptance/1")),
        "locator must carry the entry index: {locators:?}"
    );
}

#[test]
fn placeholder_entries_report_matched_words() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let report = index_project(&mut store, &fixture.0, &manifest(), false).unwrap();
    assert!(report.ok, "advisories must never flip report.ok");
    let issues = lint_issues(&report);
    let placeholders: Vec<_> = issues
        .iter()
        .filter(|issue| issue["code"] == "acceptance_lint.placeholder")
        .collect();
    assert_eq!(
        placeholders.len(),
        2,
        "both placeholder entries are reported: {issues:?}"
    );
    let matched: Vec<String> = placeholders
        .iter()
        .flat_map(|issue| {
            issue["details"]["matched"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap().to_owned())
        })
        .collect();
    assert!(
        matched.contains(&"优化".to_string()) && matched.contains(&"等等".to_string()),
        "Chinese hits are listed: {matched:?}"
    );
    assert!(
        matched.iter().any(|w| w == "properly") && matched.iter().any(|w| w == "good"),
        "English hits are listed: {matched:?}"
    );
    // W1/W2 stay silent even though W3/W4 carry findings.
    let locators: Vec<_> = placeholders
        .iter()
        .map(|issue| issue["locator"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        locators
            .iter()
            .all(|l| !l.contains("/work_items/0") && !l.contains("/work_items/1")),
        "normal entries stay silent: {locators:?}"
    );
}

#[test]
fn work_item_payload_still_projects_with_findings() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let report = index_project(&mut store, &fixture.0, &manifest(), false).unwrap();
    assert!(report.ok);
    assert_eq!(report.indexed, 1, "the ledger still indexes normally");
    let source = store
        .sources(report.project_id)
        .unwrap()
        .into_iter()
        .find(|source| source.domain == "ledger")
        .unwrap();
    let works = store
        .source_projection_payloads(&source, EntityKind::WorkItem)
        .unwrap();
    assert_eq!(works.len(), 4, "no work item is dropped or blocked");
    assert!(works.iter().all(|w| w["external_key"].is_string()));
}
