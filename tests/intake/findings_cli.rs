//! A scan that finds credentials in several files names all of them in one stop (issue #627), and stops only for
//! the documents the intake maps (issue #661).
//!
//! The intake scan used to end at the first file the secret boundary refused, so a project with findings in several
//! documents was fixed one `init` at a time. It now goes on through the other documents and reports every file with
//! its category and location, never its content. One finding is reported exactly as before.
//!
//! A finding stops the intake only in a document the mapping being applied lists: a source found by its
//! conventional name, README.md and AGENTS.md, a file under a mapped directory, or a file a reviewed draft lists.
//! Any other document is listed by path and size without a fingerprint; one observation of the draft and the
//! `refused_documents` field of the JSON preview name it with the line and category of its finding. Its content
//! reaches no output and nothing under `.awr/`.

use awr_core::Id;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Project(PathBuf);

impl Project {
    fn new(name: &str) -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-findings-{name}-{}", Id::new())));
        fs::create_dir_all(p.0.join("docs")).unwrap();
        p
    }
    fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.0)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    /// The result of a successful command, from stdout, and everything it printed.
    fn ok(&self, args: &[&str]) -> (Value, String) {
        let out = self.run(args);
        let all = output(&out);
        assert!(out.status.success(), "{args:?} should succeed:\n{all}");
        (serde_json::from_slice(&out.stdout).unwrap(), all)
    }
    /// The error report of a failed command, from stderr.
    fn fails(&self, args: &[&str]) -> (Value, String) {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} should fail");
        let all = output(&out);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let report = serde_json::from_str(
            stderr
                .lines()
                .rev()
                .find(|l| l.trim_start().starts_with('{'))
                .unwrap_or("{}"),
        )
        .unwrap();
        (report, all)
    }
    /// Every file under `.awr/`, including the state database, by relative path.
    fn runtime_files(&self) -> Vec<(String, Vec<u8>)> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else {
                    let relative = path.strip_prefix(base).unwrap().to_string_lossy();
                    out.push((relative.into_owned(), fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = vec![];
        walk(&self.0.join(".awr"), &self.0, &mut out);
        out
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn secret(label: &str) -> String {
    format!("# Notes\n\nsetup\npassword: hunter2-{label}-hunter2hunter2\n")
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn holds(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
}

fn file_name(finding: &Value) -> String {
    finding["location"]["locator"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string()
}

fn draft_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("awr-findings-{name}-{}.json", Id::new()))
}

/// The inventory entry of `path` in a draft.
fn listed<'a>(draft: &'a Value, path: &str) -> &'a Value {
    draft["inventory"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == path)
        .unwrap_or_else(|| panic!("{path} is not listed"))
}

/// The `refused_documents` entry the JSON preview gives for one refused document.
fn refused_place(path: &str, line: u64, category: &str) -> Value {
    json!({ "path": path, "line": line, "category": category })
}

/// The observation of a draft about refused documents the intake does not map, if any.
fn refused_observation(draft: &Value) -> Option<String> {
    draft["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find(|o| o.contains("secret boundary"))
        .map(str::to_owned)
}

fn mapped_paths(draft: &Value) -> Vec<String> {
    draft["authority_mapping"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["path"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn every_mapped_file_with_a_finding_is_named_in_one_stop() {
    let p = Project::new("several");
    p.write("README.md", &secret("a"));
    p.write("AGENTS.md", &secret("b"));
    // A conventional plan name is mapped, so its finding joins the same stop instead of failing a later read.
    p.write("docs/design.md", &format!("{}\nmore\n", secret("c")));
    // Not mapped: listed in the inventory, never part of the stop.
    p.write("docs/notes.md", &secret("d"));
    p.write("docs/clean.md", "# Clean\n\nnothing here\n");

    let (report, all) = p.fails(&["init"]);
    assert_eq!(report["code"], "RuleViolation");
    let details = &report["details"];
    assert_eq!(details["finding_total"], 3, "{report}");
    let findings = details["findings"].as_array().unwrap();
    let mut names: Vec<String> = findings.iter().map(file_name).collect();
    names.sort();
    assert_eq!(names, ["AGENTS.md", "README.md", "design.md"]);
    for finding in findings {
        assert!(
            finding["category"].is_string() && finding["location"]["line"].as_u64().is_some(),
            "{finding}"
        );
    }
    // The first finding keeps the fields a caller already reads, and is the first of the list.
    assert_eq!(details["category"], findings[0]["category"]);
    assert_eq!(details["location"], findings[0]["location"]);
    assert!(!all.contains("hunter2"), "no content in any output");
    assert!(!p.0.join(".awr").exists(), "nothing was written");
}

#[test]
fn one_finding_is_reported_exactly_as_before() {
    let p = Project::new("single");
    p.write("README.md", &secret("a"));
    p.write("docs/clean.md", "# Clean\n");
    let (report, all) = p.fails(&["init"]);
    assert_eq!(report["code"], "RuleViolation");
    assert!(
        report["details"].get("findings").is_none()
            && report["details"].get("finding_total").is_none(),
        "{report}"
    );
    assert_eq!(report["details"]["location"]["line"], 4);
    assert!(
        report["details"]["location"]["locator"]
            .as_str()
            .unwrap()
            .ends_with("README.md")
    );
    assert!(!all.contains("hunter2"));
}

#[test]
fn the_list_is_bounded_and_the_total_is_exact() {
    let p = Project::new("bounded");
    // A directory source maps every file under it.
    for n in 0..55 {
        p.write(
            &format!("docs/decisions/note-{n:02}.md"),
            &secret(&n.to_string()),
        );
    }
    let (report, all) = p.fails(&["init"]);
    assert_eq!(report["details"]["finding_total"], 55);
    assert_eq!(report["details"]["findings"].as_array().unwrap().len(), 50);
    assert!(!all.contains("hunter2"));
}

#[test]
fn a_blocked_draft_leaves_a_diagnostic_that_lists_every_mapped_file_and_no_content() {
    let p = Project::new("draft");
    p.write("README.md", &secret("a"));
    p.write("AGENTS.md", &secret("b"));
    p.write("docs/guide.md", &secret("c"));
    let draft = draft_file("draft");
    let (report, _) = p.fails(&["init", "--write-draft", &draft.to_string_lossy()]);
    assert_eq!(report["details"]["finding_total"], 2);
    let diagnostic: Value = serde_json::from_slice(&fs::read(&draft).unwrap()).unwrap();
    assert_eq!(diagnostic["status"], "content_review_required");
    assert_eq!(diagnostic["diagnostic"]["details"]["finding_total"], 2);
    assert!(!fs::read_to_string(&draft).unwrap().contains("hunter2"));
    let _ = fs::remove_file(draft);
}

#[test]
fn a_finding_in_a_document_the_intake_does_not_map_is_listed_and_the_intake_goes_on() {
    let p = Project::new("unmapped");
    p.write("README.md", "# P\n\nA public project.\n");
    let refused = secret("a");
    p.write("docs/guide.md", &refused);
    let digest = sha256(&refused);

    let (preview, all) = p.ok(&["init"]);
    assert_eq!(preview["status"], "preview");
    assert_eq!(preview["preview"]["can_apply"], true, "{preview}");
    let draft = &preview["draft"];
    assert!(!mapped_paths(draft).contains(&"docs/guide.md".to_owned()));
    let guide = listed(draft, "docs/guide.md");
    assert_eq!(
        guide["sha256"],
        Value::Null,
        "a refused document is not fingerprinted"
    );
    assert_eq!(guide["bytes"], refused.len());
    let observation = refused_observation(draft).expect("an observation names the document");
    assert!(
        observation.starts_with("1 document holds content the secret boundary refuses")
            && observation.contains("docs/guide.md:4 (labelled_value)"),
        "{observation}"
    );
    let refused_documents = &preview["refused_documents"];
    assert_eq!(refused_documents["total"], 1, "{preview}");
    assert_eq!(
        refused_documents["documents"],
        json!([refused_place("docs/guide.md", 4, "labelled_value")])
    );
    assert!(
        refused_documents["meaning"]
            .as_str()
            .unwrap()
            .contains("not fingerprinted"),
        "{refused_documents}"
    );
    assert!(!all.contains("hunter2") && !all.contains(&digest), "{all}");

    let (_, all) = p.ok(&["init", "--accept"]);
    assert!(!all.contains("hunter2") && !all.contains(&digest), "{all}");
    let recorded: Value =
        serde_json::from_slice(&fs::read(p.0.join(".awr/intake/inventory.json")).unwrap()).unwrap();
    assert_eq!(listed(&recorded, "docs/guide.md")["sha256"], Value::Null);
    assert_eq!(refused_observation(&recorded), Some(observation));
    let runtime = p.runtime_files();
    assert!(runtime.iter().any(|(path, _)| path.ends_with("state.db")));
    for (path, bytes) in &runtime {
        assert!(
            !holds(bytes, "hunter2") && !holds(bytes, &digest),
            "{path} holds the refused document"
        );
    }
    assert_eq!(
        fs::read_to_string(p.0.join("docs/guide.md")).unwrap(),
        refused
    );
    p.ok(&["status"]);
}

#[test]
fn a_reviewed_draft_that_maps_a_refused_document_stops_at_that_file() {
    let p = Project::new("reviewed");
    p.write("README.md", "# P\n");
    p.write("docs/guide.md", &secret("a"));
    let draft = draft_file("reviewed");
    let draft_arg = draft.to_string_lossy().into_owned();
    p.ok(&["init", "--write-draft", &draft_arg]);
    // Unchanged, the draft goes on and its preview names the document it leaves out.
    let (preview, _) = p.ok(&["init", "--from-draft", &draft_arg]);
    assert_eq!(
        preview["refused_documents"]["documents"],
        json!([refused_place("docs/guide.md", 4, "labelled_value")]),
        "{preview}"
    );

    // The reviewer maps the listed document as one more supporting source.
    let mut value: Value = serde_json::from_slice(&fs::read(&draft).unwrap()).unwrap();
    let sources = value["authority_mapping"]["sources"]
        .as_array_mut()
        .unwrap();
    let mut guide = sources
        .iter()
        .find(|s| s["path"] == "README.md")
        .unwrap()
        .clone();
    guide["path"] = Value::String("docs/guide.md".into());
    sources.push(guide);
    fs::write(&draft, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    for args in [
        vec!["init", "--from-draft", draft_arg.as_str()],
        vec!["init", "--from-draft", draft_arg.as_str(), "--accept"],
    ] {
        let (report, all) = p.fails(&args);
        assert_eq!(report["code"], "RuleViolation", "{args:?}: {report}");
        assert_eq!(report["details"]["location"]["line"], 4);
        assert!(
            report["details"]["location"]["locator"]
                .as_str()
                .unwrap()
                .ends_with("docs/guide.md"),
            "{report}"
        );
        assert!(!all.contains("hunter2"));
        assert!(!p.0.join(".awr").exists(), "nothing was written");
    }
    let _ = fs::remove_file(draft);
}

#[test]
fn redacting_a_listed_document_after_the_draft_asks_for_a_new_review() {
    let p = Project::new("redacted");
    p.write("README.md", "# P\n");
    p.write("docs/guide.md", &secret("a"));
    let draft = draft_file("redacted");
    let draft_arg = draft.to_string_lossy().into_owned();
    p.ok(&["init", "--write-draft", &draft_arg]);

    p.write("docs/guide.md", "# Notes\n\nsetup\npassword: <redacted>\n");
    let (report, _) = p.fails(&["init", "--from-draft", &draft_arg, "--accept"]);
    assert_eq!(report["code"], "SourceConflict", "{report}");
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("inventory changed"),
        "{report}"
    );
    assert!(!p.0.join(".awr").exists(), "nothing was initialized");

    // A new draft reads the redacted document and fingerprints it.
    let (preview, _) = p.ok(&["init"]);
    assert!(listed(&preview["draft"], "docs/guide.md")["sha256"].is_string());
    assert_eq!(refused_observation(&preview["draft"]), None);
    assert!(preview.get("refused_documents").is_none(), "{preview}");
    let _ = fs::remove_file(draft);
}

#[test]
fn a_project_without_findings_gets_no_observation_about_them() {
    let p = Project::new("clean");
    p.write("README.md", "# P\n");
    p.write("docs/guide.md", "# Guide\n\nnothing here\n");
    let (preview, _) = p.ok(&["init"]);
    assert_eq!(refused_observation(&preview["draft"]), None);
    assert!(preview.get("refused_documents").is_none(), "{preview}");
    for file in preview["draft"]["inventory"].as_array().unwrap() {
        assert!(file["sha256"].is_string(), "{file}");
    }
}

#[test]
fn the_preview_names_ten_refused_documents_and_counts_all_of_them() {
    let p = Project::new("observed");
    p.write("README.md", "# P\n");
    for n in 0..12 {
        p.write(&format!("docs/note-{n:02}.md"), &secret(&n.to_string()));
    }
    let (preview, all) = p.ok(&["init"]);
    let refused = &preview["refused_documents"];
    assert_eq!(refused["total"], 12, "{refused}");
    let documents = refused["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 10, "{refused}");
    assert_eq!(
        documents[9],
        refused_place("docs/note-09.md", 4, "labelled_value")
    );
    let observation = refused_observation(&preview["draft"]).unwrap();
    assert!(
        observation.starts_with("12 documents hold content the secret boundary refuses")
            && observation.contains("and 2 more."),
        "{observation}"
    );
    assert!(!all.contains("hunter2"));
}

#[test]
fn a_document_left_out_of_a_manifest_and_mapped_in_review_is_refused_as_a_source() {
    // With a manifest the intake never reads the documents the manifest leaves out, so none of them is refused or
    // named. A reviewer who maps one anyway gets no secret boundary stop from the inventory; the preview reads the
    // new source and reports it as a source issue, and accepting the draft is refused.
    let p = Project::new("manifest");
    p.write("PLAN.md", "# Plan\n\n## Goals\n\n- Ship it\n");
    let refused = secret("a");
    p.write("docs/guide.md", &refused);
    let digest = sha256(&refused);
    let manifest = std::env::temp_dir().join(format!("awr-findings-manifest-{}.toml", Id::new()));
    fs::write(
        &manifest,
        "[project]\nname = \"p\"\nauthority_mode = \"source_first\"\n\n[[sources]]\ndomain = \"plan\"\nrole = \"primary\"\npath = \"PLAN.md\"\nadapter = \"markdown-heading-v1\"\n",
    )
    .unwrap();
    let draft = draft_file("manifest");
    let draft_arg = draft.to_string_lossy().into_owned();
    let (preview, mut all) = p.ok(&[
        "init",
        "--manifest",
        &manifest.to_string_lossy(),
        "--write-draft",
        &draft_arg,
    ]);
    assert!(preview.get("refused_documents").is_none(), "{preview}");
    assert_eq!(refused_observation(&preview["draft"]), None);
    assert_eq!(
        listed(&preview["draft"], "docs/guide.md")["sha256"],
        Value::Null
    );

    let mut value: Value = serde_json::from_slice(&fs::read(&draft).unwrap()).unwrap();
    let sources = value["authority_mapping"]["sources"]
        .as_array_mut()
        .unwrap();
    let mut guide = sources[0].clone();
    guide["path"] = Value::String("docs/guide.md".into());
    guide["role"] = Value::String("supporting".into());
    sources.push(guide);
    fs::write(&draft, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let (preview, shown) = p.ok(&["init", "--from-draft", &draft_arg]);
    all.push_str(&shown);
    assert_eq!(preview["preview"]["can_apply"], false, "{preview}");
    let issues = preview["preview"]["source_issues"].as_array().unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| issue["source"]["path"] == "docs/guide.md"),
        "{issues:?}"
    );
    let (report, shown) = p.fails(&["init", "--from-draft", &draft_arg, "--accept"]);
    all.push_str(&shown);
    assert_eq!(report["code"], "SourceStale", "{report}");
    assert!(!p.0.join(".awr").exists(), "nothing was initialized");
    assert!(!all.contains("hunter2") && !all.contains(&digest), "{all}");
    let _ = fs::remove_file(draft);
    let _ = fs::remove_file(manifest);
}

#[test]
fn mapped_readme_with_empty_default_env_and_auth_placeholders_previews_cleanly() {
    let p = Project::new("env-placeholder");
    let readme = "\
# Example Project

Edit `config/domains.yaml`:

```yaml
cubejs:
  url: \"http://172.16.70.88:4000\"
  token: \"${CUBEJS_API_TOKEN:-}\"  # optional
```

```json
{
  \"headers\": {
    \"Authorization\": \"Bearer ${env:AWR_ENGINEERING_TOKEN}\"
  }
}
```
";
    p.write("README.md", readme);

    let (preview, _) = p.ok(&["init"]);
    assert_eq!(preview["status"], "preview", "{preview}");
    assert!(
        mapped_paths(&preview["draft"])
            .iter()
            .any(|path| path == "README.md"),
        "{preview}"
    );
    assert!(listed(&preview["draft"], "README.md")["sha256"].is_string());
    assert!(preview.get("refused_documents").is_none(), "{preview}");

    let leaked = readme.replace(
        "${CUBEJS_API_TOKEN:-}",
        "${CUBEJS_API_TOKEN:-synthetic-private-value}",
    );
    p.write("README.md", &leaked);
    let (report, all) = p.fails(&["init"]);
    assert_eq!(report["code"], "RuleViolation", "{report}");
    assert_eq!(report["details"]["category"], "labelled_value", "{report}");
    assert!(
        report["details"]["location"]["locator"]
            .as_str()
            .unwrap()
            .ends_with("README.md"),
        "{report}"
    );
    assert!(!all.contains("synthetic-private-value"), "{all}");
}
