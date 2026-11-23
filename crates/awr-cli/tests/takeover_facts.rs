use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Project {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    home: PathBuf,
}

impl Project {
    fn new(label: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("awr-rcn-facts-{label}-{}", awr_core::Id::new()));
        let root = base.join("project");
        let state = base.join("state");
        let home = base.join("local-home");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            base,
            root,
            state,
            home,
        }
    }

    fn init(&self) {
        self.git(&["init", "-q"]);
        self.git(&["config", "user.name", "Fixture Author"]);
        self.git(&["config", "user.email", "fixture@example.invalid"]);
        self.git(&["config", "commit.gpgsign", "false"]);
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}\n{}\n{}",
            text(&output.stdout),
            text(&output.stderr)
        );
        text(&output.stdout).trim().to_string()
    }

    fn write(&self, relative: &str, body: &str) {
        self.write_bytes(relative, body.as_bytes());
    }

    fn write_bytes(&self, relative: &str, body: &[u8]) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn commit_all(&self, title: &str) -> String {
        self.commit_all_with_body(title, "Synthetic commit body")
    }

    fn commit_all_with_body(&self, title: &str, body: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", title, "-m", body]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn run_takeover_at(&self, state: &Path, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .arg("takeover")
            .arg("--state-dir")
            .arg(state)
            .args(extra)
            .env("AWR_LOCAL_HOME", &self.home)
            .env("AWR_LOCAL_WEB", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .unwrap()
    }

    fn preview_at(&self, state: &Path, extra: &[&str]) -> Value {
        let mut args = vec!["--await-review"];
        args.extend_from_slice(extra);
        let output = self.run_takeover_at(state, &args);
        assert!(
            output.status.success(),
            "{}\n{}",
            text(&output.stdout),
            text(&output.stderr)
        );
        let outcome = outcome(&output);
        assert_eq!(outcome["outcome"], "awaiting_review", "{outcome}");
        outcome
    }

    fn preview(&self, extra: &[&str]) -> Value {
        self.preview_at(&self.state, extra)
    }

    fn facts(outcome: &Value) -> Value {
        serde_json::from_slice(&Self::raw_facts(outcome)).unwrap()
    }

    fn raw_facts(outcome: &Value) -> Vec<u8> {
        let path = outcome["record"]["steps"]["preview"]["result"]["facts_path"]
            .as_str()
            .unwrap();
        fs::read(path).unwrap()
    }

    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(base: &Path, directory: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                let relative = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let kind = fs::symlink_metadata(&path).unwrap().file_type();
                if kind.is_symlink() {
                    out.insert(
                        relative,
                        fs::read_link(path)
                            .unwrap()
                            .to_string_lossy()
                            .as_bytes()
                            .to_vec(),
                    );
                } else if kind.is_dir() {
                    walk(base, &path, out);
                } else {
                    out.insert(relative, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &self.root, &mut out);
        out
    }

    fn state_text(&self) -> String {
        fn walk(directory: &Path, out: &mut String) {
            let Ok(entries) = fs::read_dir(directory) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push_str(&String::from_utf8_lossy(&fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = String::new();
        walk(&self.state, &mut out);
        out
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn outcome(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout was not JSON: {}\n{}",
            text(&output.stdout),
            text(&output.stderr)
        )
    })
}

fn notes(facts: &Value) -> BTreeSet<&str> {
    facts["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note["code"].as_str().unwrap())
        .collect()
}

fn commit<'a>(facts: &'a Value, title: &str) -> &'a Value {
    facts["commits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|commit| commit["title"] == title)
        .unwrap()
}

#[test]
fn preview_records_history_refs_rules_coverage_and_is_byte_for_byte_read_only() {
    let p = Project::new("inventory");
    p.init();
    for (path, body) in [
        ("README.md", "# Synthetic project\n"),
        ("CONTRIBUTING.md", "Use the synthetic contribution path.\n"),
        ("CODEOWNERS", "* @fixture-owner\n"),
        ("AGENTS.md", "Follow the synthetic agent agreement.\n"),
        (
            ".github/workflows/ci.yml",
            "name: fixture\non: push\njobs: {}\n",
        ),
        (
            ".github/ISSUE_TEMPLATE/report.md",
            "---\nname: Fixture report\n---\nDescribe the fixture.\n",
        ),
        (".gitlab-ci.yml", "fixture:\n  script: echo ok\n"),
        (
            ".private/CONTRIBUTING.md",
            "This hidden directory is outside the whitelist.\n",
        ),
        ("src/main.txt", "one\n"),
    ] {
        p.write(path, body);
    }
    for index in 0..25 {
        p.write(&format!("data/file-{index:02}.txt"), "fixture\n");
    }
    let first_oid = p.commit_all("Initial synthetic import");
    p.write("README.md", "# Synthetic project\n\nUpdated.\n");
    p.write("docs/PLAN.md", "# Fixture plan\n");
    let second_oid = p.commit_all("Advance synthetic plan");
    p.git(&["tag", "v1-fixture"]);
    p.git(&[
        "tag",
        "-a",
        "v0-fixture",
        &first_oid,
        "-m",
        "Synthetic annotated tag",
    ]);
    p.git(&["checkout", "-q", "-b", "fixture-feature"]);
    p.write("feature.txt", "feature\n");
    p.commit_all("Add synthetic feature");
    p.git(&["checkout", "-q", "-"]);
    p.git(&[
        "commit",
        "--allow-empty",
        "-q",
        "-m",
        "Synthetic empty commit",
    ]);
    let before = p.snapshot();

    let first = p.preview(&[]);
    let facts = Project::facts(&first);
    assert_eq!(facts["schema_version"], 1);
    assert_eq!(facts["document"], "awr_takeover_fact_baseline");
    assert_eq!(facts["verification"], "recorded_unverified");
    assert_eq!(facts["repository"]["state"], "ready");
    assert_eq!(facts["repository"]["head_state"], "branch");
    assert_eq!(facts["repository"]["shallow"], false);
    assert_eq!(facts["repository"]["history_complete"], true);
    assert_eq!(facts["commits"].as_array().unwrap().len(), 4);
    assert_eq!(commit(&facts, "Initial synthetic import")["oid"], first_oid);
    assert_eq!(
        commit(&facts, "Initial synthetic import")["id"],
        format!("F-C-{first_oid}")
    );
    let initial = commit(&facts, "Initial synthetic import");
    assert_eq!(initial["paths"]["added"], 34);
    assert_eq!(initial["paths"]["changed"], 34);
    assert_eq!(initial["paths"]["sample"].as_array().unwrap().len(), 20);
    assert_eq!(initial["paths"]["sample_truncated"], true);
    assert_eq!(initial["bulk_import"], true);
    let second = commit(&facts, "Advance synthetic plan");
    assert_eq!(second["paths"]["added"], 1);
    assert_eq!(second["paths"]["modified"], 1);
    assert_eq!(second["paths"]["changed"], 2);
    assert_eq!(second["bulk_import"], false);
    let feature = commit(&facts, "Add synthetic feature");
    assert_eq!(feature["author"]["name"], "Fixture Author");
    assert_eq!(feature["author"]["email"], "fixture@example.invalid");
    assert_eq!(feature["authored_at"], "2023-11-14T22:13:20Z");
    let empty = commit(&facts, "Synthetic empty commit");
    assert_eq!(empty["paths"]["changed"], 0);
    assert!(empty["paths"]["sample"].as_array().unwrap().is_empty());
    let branches = facts["branches"].as_array().unwrap();
    assert_eq!(branches.len(), 2);
    assert!(branches.iter().any(|branch| {
        branch["kind"] == "local" && branch["current"] == true && branch["merged_into_head"] == true
    }));
    assert!(branches.iter().any(|branch| {
        branch["kind"] == "local"
            && branch["current"] == false
            && branch["merged_into_head"] == false
    }));
    let tags = facts["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 2);
    let annotated = tags
        .iter()
        .find(|tag| tag["name"] == "refs/tags/v0-fixture")
        .unwrap();
    assert_eq!(annotated["object_type"], "tag");
    assert_eq!(annotated["target"], first_oid);
    let lightweight = tags
        .iter()
        .find(|tag| tag["name"] == "refs/tags/v1-fixture")
        .unwrap();
    assert_eq!(lightweight["object_type"], "commit");
    assert_eq!(lightweight["target"], second_oid);
    let kinds = facts["rule_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["kind"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    for expected in [
        "contribution_guide",
        "review_owners",
        "agent_instructions",
        "pipeline",
        "issue_template",
    ] {
        assert!(kinds.contains(expected), "{kinds:?}");
    }
    assert_eq!(
        facts["rule_files"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|rule| rule["kind"] == "pipeline")
            .count(),
        2,
        "both the .github whitelist and the root .gitlab-ci.yml file are read"
    );
    assert!(facts["rule_files"].as_array().unwrap().iter().all(|rule| {
        rule["path"]
            .as_str()
            .is_none_or(|path| !path.starts_with(".private/"))
    }));
    assert!(
        facts["rule_files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["path"] == "AGENTS.md")
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("synthetic agent agreement")
    );

    let coverage = &facts["coverage"];
    assert_eq!(coverage["commits_read"], 4);
    assert_eq!(coverage["commits_available"], 4);
    assert_eq!(coverage["commits_truncated"], false);
    assert_eq!(coverage["current_tracked_files"], 35);
    assert_eq!(coverage["bulk_import_file_count"], 34);
    assert_eq!(coverage["bulk_import_file_share"]["numerator"], 34);
    assert_eq!(coverage["bulk_import_file_share"]["denominator"], 35);
    assert_eq!(coverage["bulk_import_file_share"]["percent"], 97.14);
    assert_eq!(coverage["unmerged_branch_count"], 1);
    assert_eq!(coverage["workstream_assignment_ratio"], "not_computed");
    assert_eq!(
        first["record"]["steps"]["preview"]["result"]["fact_coverage"],
        *coverage
    );

    let mut ids = Vec::new();
    ids.push(facts["repository"]["id"].as_str().unwrap().to_string());
    ids.push(coverage["id"].as_str().unwrap().to_string());
    for section in ["commits", "branches", "tags", "rule_files", "notes"] {
        for fact in facts[section].as_array().unwrap() {
            ids.push(fact["id"].as_str().unwrap().to_string());
        }
    }
    let unique = ids.iter().collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), ids.len(), "every fact ID is unique: {ids:?}");

    let second_state = p.base.join("second-state");
    let second = p.preview_at(&second_state, &[]);
    assert_eq!(
        Project::raw_facts(&first),
        Project::raw_facts(&second),
        "the same repository produces byte-identical structured facts"
    );
    assert_eq!(p.snapshot(), before, "project files and .git are unchanged");
    assert!(!p.root.join(".awr").exists());
}

#[test]
fn completed_receipt_repeats_the_exact_fact_coverage() {
    let p = Project::new("receipt");
    p.init();
    p.write("README.md", "# Synthetic receipt\n");
    p.commit_all("Create synthetic receipt fixture");
    let output = p.run_takeover_at(&p.state, &[]);
    assert!(
        output.status.success(),
        "{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let done = outcome(&output);
    assert_eq!(done["outcome"], "completed");
    let receipt: Value =
        serde_json::from_slice(&fs::read(p.root.join(".awr/takeover/receipt.json")).unwrap())
            .unwrap();
    let facts = Project::facts(&done);
    assert_eq!(receipt["facts"]["coverage"], facts["coverage"]);
    assert_eq!(receipt["facts"]["coverage"]["commits_read"], 1);
    assert_eq!(receipt["facts"]["coverage"]["bulk_import_file_count"], 1);
    assert_eq!(
        receipt["facts"]["coverage"]["bulk_import_file_share"]["percent"],
        100.0
    );
    assert_eq!(receipt["facts"]["coverage"]["unmerged_branch_count"], 0);
    assert_eq!(
        receipt["facts"]["coverage"]["workstream_assignment_ratio"],
        "not_computed"
    );
}

#[test]
fn commit_limit_reports_actual_coverage_and_a_changed_limit_refreshes_preview() {
    let p = Project::new("limit");
    p.init();
    p.write("README.md", "# Synthetic limit\n");
    for index in 0..4 {
        p.write("README.md", &format!("# Synthetic limit {index}\n"));
        p.commit_all(&format!("Synthetic step {index}"));
    }
    let before = p.snapshot();
    let limited = p.preview(&["--fact-commit-limit", "2"]);
    let facts = Project::facts(&limited);
    assert_eq!(facts["commits"].as_array().unwrap().len(), 2);
    assert_eq!(facts["coverage"]["commits_read"], 2);
    assert_eq!(facts["coverage"]["commits_available"], 4);
    assert_eq!(facts["coverage"]["commit_limit"], 2);
    assert_eq!(facts["coverage"]["commits_truncated"], true);
    assert_eq!(facts["coverage"]["history_complete"], false);
    assert!(notes(&facts).contains("commit_limit"));

    let refreshed = p.preview(&["--fact-commit-limit", "3"]);
    let refreshed = Project::facts(&refreshed);
    assert_eq!(refreshed["coverage"]["commits_read"], 3);
    assert_eq!(refreshed["coverage"]["commits_available"], 4);
    assert_eq!(refreshed["coverage"]["commit_limit"], 3);
    assert_eq!(refreshed["coverage"]["commits_truncated"], true);
    let refused = p.run_takeover_at(
        &p.base.join("too-large"),
        &["--fact-commit-limit", "100001"],
    );
    assert!(!refused.status.success());
    assert_eq!(p.snapshot(), before);
}

#[test]
fn rule_file_limits_report_recorded_read_and_oversized_counts() {
    let p = Project::new("rule-limits");
    p.init();
    p.write("CONTRIBUTING.md", &"x".repeat(256 * 1024 + 1));
    for index in 0..260 {
        p.write(
            &format!("docs/group-{index:03}/CONTRIBUTING.md"),
            "Synthetic contribution rule.\n",
        );
    }
    p.commit_all("Synthetic rule limit fixture");
    let before = p.snapshot();
    let facts = Project::facts(&p.preview(&[]));
    assert_eq!(facts["rule_files"].as_array().unwrap().len(), 256);
    let coverage = &facts["coverage"]["rule_files"];
    assert_eq!(coverage["discovered"], 261);
    assert_eq!(coverage["recorded"], 256);
    assert_eq!(coverage["read"], 255);
    assert_eq!(coverage["over_read_limit"], 1);
    assert_eq!(coverage["truncated"], true);
    assert_eq!(coverage["file_limit"], 256);
    assert_eq!(coverage["per_file_byte_limit"], 256 * 1024);
    assert_eq!(facts["rule_files"][0]["path"], "CONTRIBUTING.md");
    assert_eq!(facts["rule_files"][0]["read_status"], "over_read_limit");
    assert_eq!(facts["rule_files"][0]["text"], Value::Null);
    assert!(notes(&facts).contains("rule_file_limit"));
    assert_eq!(p.snapshot(), before);
}

#[test]
fn no_git_empty_repository_and_detached_head_have_explicit_facts() {
    let no_git = Project::new("no-git");
    no_git.write("AGENTS.md", "Synthetic non-Git rule.\n");
    let before = no_git.snapshot();
    let facts = Project::facts(&no_git.preview(&[]));
    assert_eq!(facts["repository"]["state"], "not_a_git_repository");
    assert_eq!(facts["repository"]["head_state"], "no_repository");
    assert_eq!(facts["coverage"]["commits_read"], 0);
    assert_eq!(facts["coverage"]["history_complete"], false);
    assert_eq!(facts["rule_files"].as_array().unwrap().len(), 1);
    assert!(notes(&facts).contains("no_git_history"));
    assert_eq!(no_git.snapshot(), before);

    let empty = Project::new("empty");
    empty.init();
    let before = empty.snapshot();
    let facts = Project::facts(&empty.preview(&[]));
    assert_eq!(facts["repository"]["state"], "empty_repository");
    assert_eq!(facts["repository"]["head_state"], "unborn");
    assert_eq!(facts["coverage"]["commits_available"], 0);
    assert_eq!(facts["coverage"]["history_complete"], true);
    assert_eq!(facts["coverage"]["unmerged_branch_basis"], "unborn_HEAD");
    assert!(notes(&facts).contains("empty_repository"));
    assert_eq!(empty.snapshot(), before);

    let detached = Project::new("detached");
    detached.init();
    detached.write("README.md", "# Synthetic detached fixture\n");
    let head = detached.commit_all("Synthetic detached commit");
    detached.git(&["checkout", "-q", "--detach", "HEAD"]);
    let before = detached.snapshot();
    let facts = Project::facts(&detached.preview(&[]));
    assert_eq!(facts["repository"]["head"], head);
    assert_eq!(facts["repository"]["head_state"], "detached");
    assert_eq!(facts["repository"]["current_branch"], Value::Null);
    assert!(notes(&facts).contains("detached_head"));
    assert_eq!(detached.snapshot(), before);
}

#[test]
fn shallow_clone_reports_partial_history_even_when_available_commits_fit_the_limit() {
    let source = Project::new("shallow-source");
    source.init();
    source.write("README.md", "# Synthetic shallow fixture\n");
    for index in 0..3 {
        source.write("README.md", &format!("# Synthetic shallow {index}\n"));
        source.commit_all(&format!("Synthetic shallow step {index}"));
    }
    let clone = Project::new("shallow-clone");
    let source_url = url::Url::from_file_path(&source.root).unwrap().to_string();
    let output = Command::new("git")
        .args(["clone", "--depth", "1", &source_url])
        .arg(&clone.root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let before = clone.snapshot();
    let facts = Project::facts(&clone.preview(&[]));
    assert_eq!(facts["repository"]["shallow"], true);
    assert_eq!(facts["repository"]["history_complete"], false);
    assert_eq!(facts["coverage"]["commits_read"], 1);
    assert_eq!(facts["coverage"]["commits_available"], 1);
    assert_eq!(facts["coverage"]["commits_truncated"], false);
    assert_eq!(facts["coverage"]["history_complete"], false);
    assert!(notes(&facts).contains("shallow_history"));
    assert_eq!(clone.snapshot(), before);
}

#[test]
fn submodule_gitlink_is_recorded_without_folding_in_submodule_history() {
    let child = Project::new("submodule-child");
    child.init();
    child.write("README.md", "# Synthetic child\n");
    child.commit_all("Synthetic child history");
    let parent = Project::new("submodule-parent");
    parent.init();
    parent.write("README.md", "# Synthetic parent\n");
    parent.commit_all("Synthetic parent history");
    let child_path = child.root.to_string_lossy().into_owned();
    parent.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        &child_path,
        "modules/child",
    ]);
    parent.commit_all("Record synthetic child gitlink");
    let before = parent.snapshot();
    let facts = Project::facts(&parent.preview(&[]));
    assert_eq!(facts["coverage"]["commits_read"], 2);
    assert_eq!(facts["coverage"]["submodule_count"], 1);
    assert_eq!(
        facts["repository"]["submodules"].as_array().unwrap().len(),
        1
    );
    let submodule = &facts["repository"]["submodules"][0];
    assert_eq!(submodule["path"], "modules/child");
    assert_eq!(submodule["status"], "initialized");
    assert!(submodule["id"].as_str().unwrap().starts_with("F-S-"));
    assert!(notes(&facts).contains("submodule_history"));
    assert_eq!(parent.snapshot(), before);
}

#[test]
fn commit_bodies_diffs_and_suspicious_text_never_reach_facts_or_state() {
    let p = Project::new("withheld");
    p.init();
    p.write("README.md", "# Synthetic safe start\n");
    p.commit_all("Synthetic safe start");
    p.write_bytes("src/blob.bin", b"blob-private-marker-hunter2");
    p.write(
        ".github/workflows/secret.yml",
        "fixture:\n  script: echo ok\npassword: hunter2hunter2hunter2\n",
    );
    let head = p.commit_all_with_body(
        "password: hunter2hunter2hunter2",
        "body-private-marker-hunter2\n\nThis body must stay outside the baseline.",
    );
    let output = p.run_takeover_at(&p.state, &["--await-review"]);
    assert!(
        output.status.success(),
        "{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let done = outcome(&output);
    let facts = Project::facts(&done);
    let withheld = facts["commits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|commit| commit["oid"] == head)
        .unwrap();
    assert_eq!(withheld["title"], Value::Null);
    assert!(
        withheld["withheld_fields"]
            .as_array()
            .unwrap()
            .contains(&json!("title"))
    );
    let rule = facts["rule_files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["path"] == ".github/workflows/secret.yml")
        .unwrap();
    assert_eq!(rule["read_status"], "withheld_sensitive");
    assert_eq!(rule["text"], Value::Null);
    assert_eq!(rule["sha256"], Value::Null);

    let all_output = format!(
        "{}\n{}\n{}\n{}",
        text(&output.stdout),
        text(&output.stderr),
        p.state_text(),
        text(&Project::raw_facts(&done))
    );
    for marker in [
        "hunter2",
        "blob-private-marker",
        "body-private-marker",
        "This body must stay outside",
    ] {
        assert!(
            !all_output.contains(marker),
            "marker reached output: {marker}"
        );
    }
}

#[test]
fn three_synthetic_repositories_produce_pairwise_distinct_fact_documents() {
    let mut documents = Vec::new();
    for (label, commits) in [("small", 1), ("medium", 2), ("large", 3)] {
        let p = Project::new(label);
        p.init();
        p.write("README.md", &format!("# Synthetic {label}\n"));
        for index in 0..commits {
            p.write("README.md", &format!("# Synthetic {label} step {index}\n"));
            p.commit_all(&format!("Synthetic {label} step {index}"));
        }
        documents.push(Project::raw_facts(&p.preview(&[])));
    }
    for left in 0..documents.len() {
        for right in left + 1..documents.len() {
            assert_ne!(
                documents[left], documents[right],
                "documents {left} and {right} must differ"
            );
        }
    }
}
