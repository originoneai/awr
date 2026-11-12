//! `awr init --manifest` together with `--write-draft` (issue #622).
//!
//! A project with a credential finding in one file could not use the draft and review flow: the flag pair was
//! refused, and even a draft of the other files read every document of the project when it listed them, so the
//! credential stopped it again. With a manifest the draft maps exactly the sources the owner listed, reads only
//! those, and still lists the other files by path and size so a change to any of them asks for a new review.

use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Project(PathBuf);

impl Project {
    fn new(name: &str) -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-manifest-draft-{name}-{}", Id::new())));
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
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}\n{}\n{}",
            text(&out.stdout),
            text(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    /// The error report a failed command prints on stderr.
    fn fails(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            !out.status.success(),
            "{args:?} should fail:\n{}",
            text(&out.stdout)
        );
        let stderr = text(&out.stderr);
        serde_json::from_str(
            stderr
                .lines()
                .rev()
                .find(|l| l.trim_start().starts_with('{'))
                .unwrap_or("{}"),
        )
        .unwrap_or_else(|_| serde_json::json!({ "message": stderr }))
    }
    fn manifest(&self, sources: &[(&str, &str, &str)]) -> PathBuf {
        let mut toml = String::from("[project]\nname = \"p\"\nauthority_mode = \"source_first\"\n");
        for (domain, path, adapter) in sources {
            toml.push_str(&format!("\n[[sources]]\ndomain = \"{domain}\"\nrole = \"primary\"\npath = \"{path}\"\nadapter = \"{adapter}\"\n"));
        }
        let path = std::env::temp_dir().join(format!("awr-manifest-{}.toml", Id::new()));
        fs::write(&path, toml).unwrap();
        path
    }
    /// Every file of the project with its bytes, for "nothing was written" checks.
    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else {
                    out.push((
                        path.strip_prefix(base)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                        fs::read(&path).unwrap(),
                    ));
                }
            }
        }
        let mut out = vec![];
        walk(&self.0, &self.0, &mut out);
        out.sort();
        out
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn draft_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("awr-manifest-draft-{name}-{}.json", Id::new()))
}
fn read_draft(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn source_paths(draft: &Value) -> Vec<String> {
    draft["authority_mapping"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["path"].as_str().unwrap().to_string())
        .collect()
}

const PLAN: &str = "# Plan\n\n## Goals\n\n- Ship it\n";
const CREDENTIAL_README: &str = "# P\n\nsetup\npassword: hunter2hunter2hunter2\n";

#[test]
fn a_manifest_lets_a_project_with_a_credential_file_use_the_draft_flow() {
    let p = Project::new("credential");
    p.write("README.md", CREDENTIAL_README);
    p.write("docs/PLAN.md", PLAN);
    let manifest = p.manifest(&[("plan", "docs/PLAN.md", "markdown-heading-v1")]);
    let before = p.snapshot();

    // Without the manifest the credential stops the draft, as before.
    let stopped = p.fails(&[
        "init",
        "--write-draft",
        &draft_file("plain").to_string_lossy(),
    ]);
    assert!(
        stopped["message"]
            .as_str()
            .unwrap()
            .contains("sensitive content"),
        "{stopped}"
    );

    let draft_path = draft_file("manifest");
    p.ok(&[
        "init",
        "--manifest",
        &manifest.to_string_lossy(),
        "--write-draft",
        &draft_path.to_string_lossy(),
    ]);
    assert_eq!(
        p.snapshot(),
        before,
        "writing a draft changes nothing in the project"
    );
    let draft = read_draft(&draft_path);
    assert_eq!(
        source_paths(&draft),
        ["docs/PLAN.md", ".awr/intake/work-ledger.yaml"]
    );
    assert_eq!(
        draft["inventory_scope"],
        serde_json::json!(["docs/PLAN.md"])
    );
    let readme = draft["inventory"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "README.md")
        .unwrap();
    assert_eq!(
        readme["sha256"],
        Value::Null,
        "a file left out of the manifest is listed, never read"
    );
    assert!(!fs::read_to_string(&draft_path).unwrap().contains("hunter2"));

    // The reviewed draft is applied as any other, and the project's own files stay as they were.
    p.ok(&[
        "init",
        "--from-draft",
        &draft_path.to_string_lossy(),
        "--accept",
    ]);
    assert!(p.ok(&["status"])["project_revision"].is_number());
    let after = p.snapshot();
    for (path, bytes) in &before {
        assert_eq!(
            after.iter().find(|(p, _)| p == path).map(|(_, b)| b),
            Some(bytes),
            "{path} changed"
        );
    }
}

#[test]
fn the_draft_maps_exactly_the_listed_sources() {
    let p = Project::new("exact");
    p.write("README.md", "# P\n");
    p.write("AGENTS.md", "# Agents\n");
    p.write(
        "tasks.md",
        "# Tasks\n\n| 工作项 | 状态 |\n| --- | --- |\n| T-1 | planned |\n",
    );
    p.write("docs/PLAN.md", PLAN);

    // Control: without a manifest the conventional files are mapped on their own.
    let plain = draft_file("plain");
    p.ok(&["init", "--write-draft", &plain.to_string_lossy()]);
    let plain_sources = source_paths(&read_draft(&plain));
    for path in ["README.md", "AGENTS.md", "tasks.md"] {
        assert!(
            plain_sources.iter().any(|s| s == path),
            "{path} in {plain_sources:?}"
        );
    }

    let manifest = p.manifest(&[("plan", "docs/PLAN.md", "markdown-heading-v1")]);
    let owned = draft_file("owned");
    p.ok(&[
        "init",
        "--manifest",
        &manifest.to_string_lossy(),
        "--write-draft",
        &owned.to_string_lossy(),
    ]);
    let draft = read_draft(&owned);
    assert_eq!(
        source_paths(&draft),
        ["docs/PLAN.md", ".awr/intake/work-ledger.yaml"]
    );
    assert!(
        draft["observations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o.as_str().unwrap().contains("supplied manifest")),
        "{}",
        draft["observations"]
    );
}

#[test]
fn a_change_to_any_file_after_the_draft_asks_for_a_new_review() {
    let p = Project::new("changed");
    p.write("README.md", "# P\n");
    p.write("docs/PLAN.md", PLAN);
    let manifest = p.manifest(&[("plan", "docs/PLAN.md", "markdown-heading-v1")]);

    for (changed, text) in [
        ("docs/PLAN.md", "# Plan\n\n## Goals\n\n- Ship it twice\n"),
        ("README.md", "# P\n\nmore\n"),
    ] {
        let draft_path = draft_file("changed");
        p.ok(&[
            "init",
            "--manifest",
            &manifest.to_string_lossy(),
            "--write-draft",
            &draft_path.to_string_lossy(),
        ]);
        let original = fs::read_to_string(p.0.join(changed)).unwrap();
        p.write(changed, text);
        let report = p.fails(&[
            "init",
            "--from-draft",
            &draft_path.to_string_lossy(),
            "--accept",
        ]);
        assert!(
            report["message"]
                .as_str()
                .unwrap()
                .contains("inventory changed"),
            "{changed}: {report}"
        );
        assert!(
            !p.0.join(".awr").exists(),
            "{changed}: nothing was initialized"
        );
        p.write(changed, &original);
    }
}

#[test]
fn a_credential_in_a_listed_source_still_stops_the_draft() {
    let p = Project::new("listed");
    p.write("README.md", CREDENTIAL_README);
    let manifest = p.manifest(&[("plan", "README.md", "markdown-heading-v1")]);
    let draft_path = draft_file("listed");
    let report = p.fails(&[
        "init",
        "--manifest",
        &manifest.to_string_lossy(),
        "--write-draft",
        &draft_path.to_string_lossy(),
    ]);
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("sensitive content"),
        "{report}"
    );
    assert!(!p.0.join(".awr").exists());
    // A blocked draft leaves at most a diagnostic without source text, never an acceptable draft.
    if draft_path.exists() {
        assert!(!fs::read_to_string(&draft_path).unwrap().contains("hunter2"));
        assert!(read_draft(&draft_path).get("authority_mapping").is_none());
    }
}

#[test]
fn a_manifest_never_replaces_an_existing_mapping() {
    let p = Project::new("existing");
    p.write("README.md", "# P\n");
    p.write("docs/PLAN.md", PLAN);
    p.ok(&["init", "--accept"]);
    let manifest = p.manifest(&[("plan", "docs/PLAN.md", "markdown-heading-v1")]);
    let draft_path = draft_file("existing");
    let report = p.fails(&[
        "init",
        "--manifest",
        &manifest.to_string_lossy(),
        "--write-draft",
        &draft_path.to_string_lossy(),
    ]);
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("never overwrites"),
        "{report}"
    );
    assert!(!draft_path.exists(), "no draft is written");
}

#[test]
fn a_missing_source_is_reported_as_for_any_draft_and_a_broken_manifest_writes_nothing() {
    let p = Project::new("missing");
    p.write("README.md", "# P\n");

    // As for a draft without a manifest: the preview says it cannot be applied and names the source, and
    // accepting the draft is refused.
    let draft_path = draft_file("missing");
    let missing = p.manifest(&[("plan", "docs/MISSING.md", "markdown-heading-v1")]);
    let shown = p.ok(&[
        "init",
        "--manifest",
        &missing.to_string_lossy(),
        "--write-draft",
        &draft_path.to_string_lossy(),
    ]);
    assert_eq!(shown["preview"]["can_apply"], false, "{shown}");
    assert_eq!(
        shown["preview"]["source_issues"][0]["error"]["code"],
        "SourceUnavailable"
    );
    p.fails(&[
        "init",
        "--from-draft",
        &draft_path.to_string_lossy(),
        "--accept",
    ]);
    assert!(!p.0.join(".awr").exists(), "nothing was initialized");

    // A manifest that does not parse is refused before any draft is written.
    let broken = std::env::temp_dir().join(format!("awr-manifest-broken-{}.toml", Id::new()));
    fs::write(&broken, "[project\nname =").unwrap();
    let never = draft_file("broken");
    p.fails(&[
        "init",
        "--manifest",
        &broken.to_string_lossy(),
        "--write-draft",
        &never.to_string_lossy(),
    ]);
    assert!(!never.exists());
}

#[test]
fn the_manifest_still_conflicts_with_the_flags_that_cannot_share_its_meaning() {
    let p = Project::new("conflicts");
    p.write("docs/PLAN.md", PLAN);
    let manifest = p.manifest(&[("plan", "docs/PLAN.md", "markdown-heading-v1")]);
    let m = manifest.to_string_lossy().into_owned();
    let d = draft_file("conflicts").to_string_lossy().into_owned();
    for args in [
        vec!["init", "--manifest", &m, "--write-draft", &d, "--accept"],
        vec!["init", "--manifest", &m, "--from-draft", &d],
        vec!["init", "--manifest", &m, "--write-draft", &d, "--goal", "x"],
        vec![
            "init",
            "--manifest",
            &m,
            "--write-draft",
            &d,
            "--status-map",
            "a=b",
        ],
    ] {
        let report = p.fails(&args);
        assert!(
            report["message"]
                .as_str()
                .unwrap()
                .contains("cannot be used with"),
            "{args:?}: {report}"
        );
    }
    assert!(!p.0.join(".awr").exists());
}
