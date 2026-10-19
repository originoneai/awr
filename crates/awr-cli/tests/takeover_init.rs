use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("awr 接入 project {}", awr_core::Id::new()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, body: &str) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn empty_and_code_only_projects_get_a_real_work_entry_without_invented_history() {
    for code in [false, true] {
        let p = Project::new();
        if code {
            p.write("src/main.rs", "fn main() {}\n");
        }
        let preview = p.ok(&["init", "--goal", "Deliver a useful document portal"]);
        assert_eq!(preview["status"], "preview");
        assert!(!p.0.join(".awr").exists());
        assert_eq!(
            preview["draft"]["generated_files"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert!(
            preview["draft"]["generated_files"]
                .get(".awr/intake/work-ledger.yaml")
                .is_some()
        );
        assert!(
            preview["draft"]["generated_files"]
                .get(".awr/intake/GOALS.md")
                .is_none()
        );
        let installed = p.ok(&[
            "init",
            "--accept",
            "--goal",
            "Deliver a useful document portal",
        ]);
        assert_eq!(installed["index"]["indexed"], 1);
        let work = p.ok(&["work", "show", "INTAKE-001"]);
        assert!(work.to_string().contains("Verify project goals"));
        let before = fs::read(p.0.join(".awr/intake/work-ledger.yaml")).unwrap();
        p.ok(&["init", "--accept"]);
        assert_eq!(
            before,
            fs::read(p.0.join(".awr/intake/work-ledger.yaml")).unwrap()
        );
        if code {
            assert_eq!(
                fs::read_to_string(p.0.join("src/main.rs")).unwrap(),
                "fn main() {}\n"
            );
        }
    }
}
#[test]
fn a_plan_yields_reviewable_work_and_does_not_claim_existing_implementation() {
    let p = Project::new();
    let plan = "# Publish documents {#publish status=active}\n\nDeliver a search page.\n\n## Search UI {#search status=planned}\n\nReaders can find documents.\n";
    p.write("PLAN.md", plan);
    p.ok(&["init", "--accept"]);
    let ledger = fs::read_to_string(p.0.join(".awr/intake/work-ledger.yaml")).unwrap();
    assert!(ledger.contains("Publish documents"));
    assert!(ledger.contains("Search UI"));
    assert!(!ledger.contains("status: completed"));
    assert_eq!(fs::read_to_string(p.0.join("PLAN.md")).unwrap(), plan);
}
#[test]
fn existing_markdown_table_and_checkbox_states_are_preserved() {
    let p = Project::new();
    let body = "# Existing tasks\n\n| 编号 | 任务 | 状态 | 下一步 |\n| --- | --- | --- | --- |\n| DONE-1 | Old delivery | 已完成 | Review receipt |\n| OPEN-1 | Current delivery | 进行中 | Finish search |\n\n- [ ] Confirm remaining scope\n";
    p.write("台账.md", body);
    p.ok(&["init", "--accept"]);
    assert!(!p.0.join(".awr/intake/work-ledger.yaml").exists());
    assert!(p.0.join(".awr/intake/GOALS.md").is_file());
    assert!(
        p.ok(&["work", "show", "DONE-1"])
            .to_string()
            .contains("completed")
    );
    assert!(
        p.ok(&["work", "show", "OPEN-1"])
            .to_string()
            .contains("in_progress")
    );
    assert_eq!(fs::read_to_string(p.0.join("台账.md")).unwrap(), body);
}
#[test]
fn reviewed_draft_rejects_changed_inventory_and_cannot_write_arbitrary_paths() {
    let p = Project::new();
    p.write("README.md", "# Document portal\n");
    let draft = std::env::temp_dir().join(format!("awr-draft-{}.json", awr_core::Id::new()));
    p.ok(&["init", "--write-draft", draft.to_str().unwrap()]);
    p.write("README.md", "# Changed portal\n");
    let stale = p.run(&["init", "--from-draft", draft.to_str().unwrap(), "--accept"]);
    assert!(!stale.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&stale.stderr).unwrap()["code"],
        "SourceConflict"
    );
    assert!(!p.0.join(".awr").exists());
    fs::remove_file(&draft).unwrap();
    p.ok(&["init", "--write-draft", draft.to_str().unwrap()]);
    let mut value: Value = serde_json::from_slice(&fs::read(&draft).unwrap()).unwrap();
    value["generated_files"]["README.md"] = Value::String("overwrite".into());
    fs::write(&draft, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        !p.run(&["init", "--from-draft", draft.to_str().unwrap(), "--accept"])
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(p.0.join("README.md")).unwrap(),
        "# Changed portal\n"
    );
    fs::remove_file(draft).unwrap();
}
#[test]
fn reviewed_draft_can_supply_concrete_new_work() {
    let p = Project::new();
    let draft = std::env::temp_dir().join(format!("awr-draft-{}.json", awr_core::Id::new()));
    p.ok(&["init", "--write-draft", draft.to_str().unwrap()]);
    let mut value: Value = serde_json::from_slice(&fs::read(&draft).unwrap()).unwrap();
    value["generated_files"][".awr/intake/work-ledger.yaml"]=Value::String("work_items:\n- id: SEARCH-001\n  title: Implement approved document search\n  status: ready\n  acceptance: [Return the approved documents for a query]\n  next_action: Implement the search handler\n".into());
    fs::write(&draft, serde_json::to_vec(&value).unwrap()).unwrap();
    p.ok(&["init", "--from-draft", draft.to_str().unwrap(), "--accept"]);
    assert!(
        p.ok(&["work", "show", "SEARCH-001"])
            .to_string()
            .contains("Implement approved")
    );
    fs::remove_file(draft).unwrap();
}
#[cfg(unix)]
#[test]
fn inventory_does_not_follow_symlinks() {
    let p = Project::new();
    std::os::unix::fs::symlink(Path::new("/"), p.0.join("outside")).unwrap();
    let preview = p.ok(&["init"]);
    assert!(preview["draft"]["inventory"].as_array().unwrap().is_empty());
}

#[test]
fn deep_directories_are_skipped_with_an_observation_instead_of_failing() {
    let p = Project::new();
    p.write("README.md", "# Document portal\n");
    // A framework build output (`_next`, like chatui bundles) and an ordinary
    // tree, both reaching depth 13 where the intake walk used to abort.
    let tail = "a/b/c/d/e/f/g/h/i/j/k/l";
    p.write(
        &format!("web/_next/static/{tail}/bundle.js"),
        "console.log(1);\n",
    );
    p.write(&format!("deep/{tail}/notes.md"), "# Deep notes\n");
    let preview = p.ok(&["init"]);
    assert_eq!(preview["status"], "preview");
    let paths: Vec<&str> = preview["draft"]["inventory"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["README.md"]);
    assert!(
        preview["draft"]["observations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| {
                o.as_str()
                    .unwrap()
                    .contains("1 directory subtree deeper than 12 levels was skipped")
            })
    );
    p.ok(&[
        "init",
        "--accept",
        "--goal",
        "Deliver a useful document portal",
    ]);
    assert!(
        p.ok(&["work", "show", "INTAKE-001"])
            .to_string()
            .contains("Verify project goals")
    );
}

#[test]
fn authored_documents_under_ambiguous_directory_names_are_still_indexed() {
    let p = Project::new();
    p.write("README.md", "# Document portal\n");
    // Ambiguous names (vendor/coverage/Pods) match authored content at any
    // depth, so SKIP must not swallow documents under them; only unambiguous
    // build output names are skipped.
    p.write("docs/vendor/notes.md", "# Vendored notes\n");
    p.write("policy/coverage/report.md", "# Coverage policy\n");
    p.write("guides/Pods/setup.md", "# Pods guide\n");
    let preview = p.ok(&["init"]);
    assert_eq!(preview["status"], "preview");
    let paths: Vec<&str> = preview["draft"]["inventory"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        vec![
            "README.md",
            "docs/vendor/notes.md",
            "guides/Pods/setup.md",
            "policy/coverage/report.md",
        ]
    );
}

const MEASUREMENT: &str = "## Payload measurement\n\nThe measurement serializes tool objects as compact, sorted JSON.\n\nEnvironment:\n\n- Server commit: `0123456789abcdef0123456789abcdef01234567`\n- Python: `3.12.11`\n- Node: `22.14.0`\n- Operating system: Ubuntu 24.04\n- catalog: 8 domains and 55 children\n- measurement unit: UTF-8 serialized bytes, not model tokens\n";

#[test]
fn public_measurement_notes_support_preview_draft_accept_and_retry() {
    let p = Project::new();
    p.write("docs/measurement.md", MEASUREMENT);
    p.write(
        "package-lock.json",
        r#"{"packages":{"node_modules/std-env":{"version":"3.10.0","dev":true}}}"#,
    );
    let preview = p.ok(&["init", "--goal", "Reproduce public measurements"]);
    assert_eq!(preview["status"], "preview");
    assert!(!p.0.join(".awr").exists());
    // Keep the draft outside the scanned inventory.
    let draft = p.0.join(".local/draft.json");
    fs::create_dir(p.0.join(".local")).unwrap();
    p.ok(&[
        "init",
        "--write-draft",
        draft.to_str().unwrap(),
        "--goal",
        "Reproduce public measurements",
    ]);
    assert!(draft.is_file());
    assert!(!p.0.join(".awr").exists());
    p.ok(&["init", "--from-draft", draft.to_str().unwrap(), "--accept"]);
    assert!(p.0.join(".awr/state.db").is_file());
    p.ok(&["doctor"]);
    p.ok(&["init", "--accept"]);
    assert_eq!(
        fs::read_to_string(p.0.join("docs/measurement.md")).unwrap(),
        MEASUREMENT
    );
}

#[test]
fn boolean_gloss_notes_support_preview_draft_accept_and_retry_without_rewriting_source() {
    for notes in [
        r#"[{"comment":"FALSE=disabled; TRUE=enabled"},{"comment":"状态说明；NULL=未设置"}]"#,
        r#"[{"comment":"；NULL=未设置；TRUE=enabled；FALSE=disabled"}]"#,
        r#"[{"comment":"TRUE=enabled; FALSE=disabled; NULL=unset"}]"#,
    ] {
        let p = Project::new();
        p.write("notes.json", notes);
        let before = fs::read(p.0.join("notes.json")).unwrap();
        let goal = "Reproduce public boolean gloss notes";

        let preview = p.ok(&["init", "--goal", goal]);
        assert_eq!(preview["status"], "preview");
        assert_eq!(preview["preview"]["source_issues"], serde_json::json!([]));
        assert_eq!(preview["preview"]["can_apply"], true);
        assert!(!p.0.join(".awr").exists());
        assert_eq!(fs::read(p.0.join("notes.json")).unwrap(), before);

        let draft = p.0.join(".local/draft.json");
        fs::create_dir(p.0.join(".local")).unwrap();
        p.ok(&[
            "init",
            "--write-draft",
            draft.to_str().unwrap(),
            "--goal",
            goal,
        ]);
        assert!(draft.is_file());
        assert!(!p.0.join(".awr").exists());
        assert_eq!(fs::read(p.0.join("notes.json")).unwrap(), before);

        p.ok(&["init", "--from-draft", draft.to_str().unwrap(), "--accept"]);
        assert!(p.0.join(".awr/state.db").is_file());
        assert_eq!(fs::read(p.0.join("notes.json")).unwrap(), before);
        p.ok(&["doctor"]);

        // Repeating accept is the normal retry/idempotent path after a reviewed
        // draft has been applied; it must preserve the authored notes exactly.
        p.ok(&["init", "--accept"]);
        assert_eq!(fs::read(p.0.join("notes.json")).unwrap(), before);
    }
}

#[test]
fn boolean_gloss_prefixes_do_not_allow_environment_or_secret_content() {
    for (notes, category) in [
        (
            r#"[{"comment":"export HOME=/synthetic-private-value"}]"#,
            "environment_dump",
        ),
        (
            r#"[{"environment":{"HOME":"/synthetic-private-value"}}]"#,
            "environment_dump",
        ),
        (
            r#"[{"comment":"FALSE=disabled; HOME=/synthetic-private-value"}]"#,
            "environment_dump",
        ),
        (
            "FALSE=disabled\nHOME=/synthetic-private-value\nPATH=/synthetic-bin\n",
            "environment_dump",
        ),
        (
            r#"[{"comment":"FALSE=disabled; password: synthetic-private-value"}]"#,
            "labelled_value",
        ),
        (
            r#"[{"comment":"TRUE=enabled; sk-synthetic-private-value"}]"#,
            "credential",
        ),
    ] {
        let p = Project::new();
        p.write("notes.json", notes);
        let before = fs::read(p.0.join("notes.json")).unwrap();
        let draft = p.0.join(".local/draft.json");
        fs::create_dir(p.0.join(".local")).unwrap();
        let goal = "Review public boolean gloss notes";
        let mut reports = Vec::new();

        for args in [
            vec!["init", "--goal", goal],
            vec![
                "init",
                "--write-draft",
                draft.to_str().unwrap(),
                "--goal",
                goal,
            ],
            vec!["init", "--accept", "--goal", goal],
        ] {
            let out = p.run(&args);
            assert!(!out.status.success());
            assert!(out.stdout.is_empty());
            let report: Value = serde_json::from_slice(&out.stderr).unwrap();
            assert_eq!(report["code"], "RuleViolation");
            assert_eq!(report["details"]["category"], category);
            assert_eq!(report["details"]["rule"], "source.public_content");
            assert!(!String::from_utf8_lossy(&out.stderr).contains("synthetic-private-value"));
            assert!(!String::from_utf8_lossy(&out.stdout).contains("synthetic-private-value"));
            assert!(!p.0.join(".awr").exists());
            if draft.exists() {
                let diagnostic = fs::read_to_string(&draft).unwrap();
                assert!(!diagnostic.contains("synthetic-private-value"));
                assert_eq!(
                    serde_json::from_str::<Value>(&diagnostic).unwrap()["status"],
                    "content_review_required"
                );
            }
            assert_eq!(fs::read(p.0.join("notes.json")).unwrap(), before);
            reports.push(report);
        }
        assert_eq!(reports[0], reports[1]);
        assert_eq!(reports[0], reports[2]);
    }
}

#[test]
fn rejected_measurement_sources_have_consistent_locations_and_leave_no_partial_init() {
    for (tail, category) in [
        ("password: synthetic-private-value\n", "labelled_value"),
        (
            "HOME=/synthetic-private-value\nPATH=/synthetic-bin\n",
            "environment_dump",
        ),
        (
            "environment:\n  HOME: /synthetic-private-value\n",
            "environment_dump",
        ),
    ] {
        let p = Project::new();
        let body = format!("{MEASUREMENT}\n{tail}");
        p.write("docs/measurement.md", &body);
        let draft = p.0.join(".local/draft.json");
        fs::create_dir(p.0.join(".local")).unwrap();
        let mut reports = Vec::new();
        for args in [
            vec!["init"],
            vec!["init", "--write-draft", draft.to_str().unwrap()],
            vec!["init", "--accept"],
        ] {
            let out = p.run(&args);
            assert!(!out.status.success());
            let report: Value = serde_json::from_slice(&out.stderr).unwrap();
            assert_eq!(report["code"], "RuleViolation");
            assert_eq!(report["details"]["category"], category);
            let entry_line = body
                .lines()
                .position(|line| line.contains("synthetic-private-value"))
                .unwrap()
                + 1;
            assert_eq!(report["details"]["location"]["line"], entry_line);
            assert!(
                report["details"]["location"]["locator"]
                    .as_str()
                    .unwrap()
                    .ends_with("docs/measurement.md")
            );
            assert!(!String::from_utf8_lossy(&out.stderr).contains("synthetic-private-value"));
            assert!(!String::from_utf8_lossy(&out.stdout).contains("synthetic-private-value"));
            assert!(!p.0.join(".awr").exists());
            if draft.exists() {
                let diagnostic = fs::read_to_string(&draft).unwrap();
                assert!(!diagnostic.contains("synthetic-private-value"));
                assert_eq!(
                    serde_json::from_str::<Value>(&diagnostic).unwrap()["status"],
                    "content_review_required"
                );
            }
            assert_eq!(
                fs::read_to_string(p.0.join("docs/measurement.md")).unwrap(),
                body
            );
            reports.push(report);
        }
        assert_eq!(reports[0], reports[1]);
        assert_eq!(reports[0], reports[2]);
        p.write("docs/measurement.md", MEASUREMENT);
        p.ok(&[
            "init",
            "--accept",
            "--goal",
            "Reproduce public measurements",
        ]);
        p.ok(&["doctor"]);
    }
}

#[test]
fn blank_project_embeds_the_placeholder_goal_in_the_ledger() {
    let p = Project::new();
    let installed = p.ok(&["init", "--accept"]);
    assert_eq!(installed["index"]["indexed"], 1);
    assert!(!p.0.join(".awr/intake/GOALS.md").exists());
    let ledger = fs::read_to_string(p.0.join(".awr/intake/work-ledger.yaml")).unwrap();
    assert!(ledger.contains("id: intake-goal"));
    assert!(ledger.contains("status: draft"));
    assert!(!ledger.contains("file://"));
    let report = p.ok(&["intake", "inspect"]);
    let goal = &report["organization"]["goals"][0];
    assert_eq!(goal["key"], "intake-goal");
    assert_eq!(goal["status"], "draft");
    let named = Project::new();
    named.ok(&["init", "--accept", "--goal", "Deliver a report exporter"]);
    let named_ledger = fs::read_to_string(named.0.join(".awr/intake/work-ledger.yaml")).unwrap();
    assert!(named_ledger.contains("id: intake-goal"));
    assert!(named_ledger.contains("status: active"));
    assert!(named_ledger.contains("Deliver a report exporter"));
    let named_report = named.ok(&["intake", "inspect"]);
    assert_eq!(
        named_report["organization"]["goals"][0]["key"],
        "intake-goal"
    );
}
