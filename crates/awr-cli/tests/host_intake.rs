use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("awr 精确接入 {}", Id::new()));
        fs::create_dir(&path).unwrap();
        let p = Self(path);
        p.write("work-ledger.yaml", "goals:\n- id: G\n  title: Publish a guide\n  status: active\nwork_items:\n- id: W\n  title: Draft a guide\n  goal: G\n  status: ready\n  acceptance: [A useful guide]\n  next_action: Write the first draft\n");
        p.write("mapping.toml", "[project]\nname='Guide'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work-ledger.yaml'\nadapter='yaml-ledger-v1'\n");
        p.write(".gitignore", "# User entries\n*.tmp");
        p
    }
    fn write(&self, path: &str, body: &str) {
        let file = self.0.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, body).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert!(
            r.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn error(&self, args: &[&str], code: &str) {
        let r = self.run(args);
        assert!(!r.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stderr).unwrap()["code"],
            code
        );
    }
    fn init(&self) -> Value {
        let preview = self.ok(&["init", "--manifest", "mapping.toml"]);
        self.ok(&[
            "init",
            "--manifest",
            "mapping.toml",
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
        ])
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn preview_lists_exact_configuration_and_ignore_bytes_without_initializing() {
    let p = Project::new();
    let source = fs::read(p.0.join("work-ledger.yaml")).unwrap();
    let preview = p.ok(&["init", "--manifest", "mapping.toml"]);
    assert!(!p.0.join(".awr").exists());
    assert_eq!(
        fs::read_to_string(p.0.join(".gitignore")).unwrap(),
        "# User entries\n*.tmp"
    );
    let plan = &preview["preview"];
    assert_eq!(plan["source_write_performed"], false);
    assert_eq!(plan["source_snapshots"].as_array().unwrap().len(), 1);
    p.ok(&[
        "init",
        "--manifest",
        "mapping.toml",
        "--accept",
        "--expected-preview",
        plan["fingerprint"].as_str().unwrap(),
    ]);
    for effect in plan["writes"].as_array().unwrap() {
        assert_eq!(
            fs::read_to_string(p.0.join(effect["path"].as_str().unwrap())).unwrap(),
            effect["after_text"].as_str().unwrap()
        );
    }
    assert_eq!(fs::read(p.0.join("work-ledger.yaml")).unwrap(), source);
}

#[test]
fn source_ignore_and_mapping_changes_invalidate_previews_before_any_runtime_write() {
    for changed in ["work-ledger.yaml", ".gitignore", "mapping.toml"] {
        let p = Project::new();
        let preview = p.ok(&["init", "--manifest", "mapping.toml"]);
        let old = fs::read_to_string(p.0.join(changed)).unwrap();
        // Mapping semantics must change; TOML comments are not an authorization change.
        let new = if changed == "mapping.toml" {
            old.replace("name='Guide'", "name='Changed Guide'")
        } else {
            format!("{old}\n# External edit\n")
        };
        p.write(changed, &new);
        p.error(
            &[
                "init",
                "--manifest",
                "mapping.toml",
                "--accept",
                "--expected-preview",
                preview["preview"]["fingerprint"].as_str().unwrap(),
            ],
            "SourceConflict",
        );
        assert!(!p.0.join(".awr").exists());
        assert_eq!(fs::read_to_string(p.0.join(changed)).unwrap(), new);
    }
}

#[test]
fn inferred_intake_covers_generated_files_and_reviewed_draft_acceptance() {
    let p = Project::new();
    fs::remove_file(p.0.join("work-ledger.yaml")).unwrap();
    let draft_path = std::env::temp_dir().join(format!("host-intake-{}.json", Id::new()));
    let preview = p.ok(&[
        "init",
        "--goal",
        "Publish a useful guide",
        "--write-draft",
        draft_path.to_str().unwrap(),
    ]);
    let plan = &preview["preview"];
    assert!(
        plan["writes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == ".awr/intake/inventory.json")
    );
    p.ok(&[
        "init",
        "--from-draft",
        draft_path.to_str().unwrap(),
        "--accept",
        "--expected-preview",
        plan["fingerprint"].as_str().unwrap(),
    ]);
    for effect in plan["writes"].as_array().unwrap() {
        assert_eq!(
            fs::read_to_string(p.0.join(effect["path"].as_str().unwrap())).unwrap(),
            effect["after_text"].as_str().unwrap()
        );
    }
    fs::remove_file(draft_path).unwrap();
}

#[test]
fn repeated_intake_retains_project_work_and_session_identity() {
    let p = Project::new();
    p.init();
    let before = p.ok(&["work", "show", "W"]);
    let session = p.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "host-fixture",
        "--provider",
        "local",
        "--model",
        "none",
        "--claim",
        "--expected-revision",
        &before["project_revision"].to_string(),
    ]);
    let id = session["session"]["id"].as_str().unwrap();
    let context = p.ok(&[
        "context",
        "compile",
        "--work",
        "W",
        "--session",
        id,
        "--goal",
        "G",
    ]);
    let checkpoint = p.ok(&[
        "session",
        "checkpoint",
        "--session",
        id,
        "--context-hash",
        context["work_context"]["context_hash"].as_str().unwrap(),
        "--digest",
        "Started guide outline",
        "--next-action",
        "Write introduction",
        "--expected-revision",
        &context["project_revision"].to_string(),
    ]);
    let preview = p.ok(&["init"]);
    p.ok(&[
        "init",
        "--accept",
        "--expected-preview",
        preview["preview"]["fingerprint"].as_str().unwrap(),
    ]);
    let after = p.ok(&["work", "show", "W"]);
    assert_eq!(before["work"]["id"], after["work"]["id"]);
    let saved = p.ok(&["session", "show", id]);
    assert!(saved.to_string().contains(id));
    assert_eq!(
        saved["session"]["last_checkpoint_id"],
        checkpoint["checkpoint"]["id"]
    );
    assert_eq!(after["work"]["active_claims"].as_array().unwrap().len(), 1);
}

#[test]
fn explicit_configuration_change_preserves_runtime_and_rejects_stale_or_identity_changes() {
    let p = Project::new();
    p.init();
    let original =
        fs::read_to_string(p.0.join(".awr/project.toml")).unwrap() + "\n# Preserve this comment\n";
    p.write(".awr/project.toml", &original);
    let unchanged = p.ok(&["source", "configure", "--manifest", "mapping.toml"]);
    assert_eq!(
        p.ok(&[
            "source",
            "configure",
            "--manifest",
            "mapping.toml",
            "--accept",
            "--expected-preview",
            unchanged["preview"]["fingerprint"].as_str().unwrap()
        ])["write_outcome"],
        "no_change"
    );
    assert_eq!(
        fs::read_to_string(p.0.join(".awr/project.toml")).unwrap(),
        original
    );
    let before = p.ok(&["work", "show", "W"]);
    p.write(
        "decisions/choice.md",
        "# Use a short guide\n\nKeep the first edition concise.\n",
    );
    let mapping = fs::read_to_string(p.0.join("mapping.toml")).unwrap();
    p.write("replacement.toml", &format!("{mapping}\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"));
    p.error(
        &[
            "source",
            "configure",
            "--manifest",
            "replacement.toml",
            "--accept",
        ],
        "InvalidInput",
    );
    let preview = p.ok(&["source", "configure", "--manifest", "replacement.toml"]);
    p.write(
        "decisions/choice.md",
        "# Changed choice\n\nUse a detailed guide.\n",
    );
    p.error(
        &[
            "source",
            "configure",
            "--manifest",
            "replacement.toml",
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
        ],
        "SourceConflict",
    );
    let preview = p.ok(&["source", "configure", "--manifest", "replacement.toml"]);
    let result = p.ok(&[
        "source",
        "configure",
        "--manifest",
        "replacement.toml",
        "--accept",
        "--expected-preview",
        preview["preview"]["fingerprint"].as_str().unwrap(),
    ]);
    assert_eq!(result["configuration_write_performed"], true);
    assert_eq!(result["write_outcome"], "applied");
    let status = p.ok(&[
        "source",
        "configure-status",
        preview["preview"]["fingerprint"].as_str().unwrap(),
    ]);
    assert_eq!(status["observed_configuration"], "after");
    assert_eq!(status["receipt"]["id"], result["id"]);
    assert_eq!(status["runtime_write_performed"], false);
    assert_eq!(
        p.ok(&["work", "show", "W"])["work"]["id"],
        before["work"]["id"]
    );
    let receipt =
        p.0.join(result["recovery_directory"].as_str().unwrap())
            .join("receipt.json");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(receipt).unwrap()).unwrap()["write_outcome"],
        "applied"
    );
    let preview = p.ok(&["source", "configure", "--manifest", "replacement.toml"]);
    assert_eq!(
        p.ok(&[
            "source",
            "configure",
            "--manifest",
            "replacement.toml",
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap()
        ])["write_outcome"],
        "no_change"
    );
    p.write(
        "replacement.toml",
        &mapping.replace("name='Guide'", "name='Another project'"),
    );
    p.error(
        &["source", "configure", "--manifest", "replacement.toml"],
        "SourceConflict",
    );
}

#[test]
fn known_source_failure_is_rejected_before_any_initialization() {
    let p = Project::new();
    p.write("missing.toml", &(fs::read_to_string(p.0.join("mapping.toml")).unwrap() + "\n[[sources]]\ndomain='plan'\nrole='supporting'\npath='not-present.md'\nadapter='markdown-heading-v1'\n"));
    let preview = p.ok(&["init", "--manifest", "missing.toml"]);
    assert_eq!(
        preview["preview"]["source_issues"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let result = p.run(&[
        "init",
        "--manifest",
        "missing.toml",
        "--accept",
        "--expected-preview",
        preview["preview"]["fingerprint"].as_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stderr).unwrap()["code"],
        "SourceStale"
    );
    assert!(result.stdout.is_empty());
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(error["details"]["configuration_write_performed"], false);
    assert_eq!(error["details"]["runtime_write_performed"], false);
    assert!(!p.0.join(".awr").exists());
}

#[cfg(unix)]
#[test]
fn a_linked_ignore_target_is_rejected_and_readonly_preview_does_not_initialize() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let p = Project::new();
    fs::remove_file(p.0.join(".gitignore")).unwrap();
    p.write("ignore-target", "keep me\n");
    symlink(p.0.join("ignore-target"), p.0.join(".gitignore")).unwrap();
    assert!(
        !p.run(&["init", "--manifest", "mapping.toml"])
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(p.0.join("ignore-target")).unwrap(),
        "keep me\n"
    );
    fs::remove_file(p.0.join(".gitignore")).unwrap();
    p.write(".gitignore", "# Read only\n");
    fs::set_permissions(&p.0, fs::Permissions::from_mode(0o555)).unwrap();
    let preview = p.run(&["init", "--manifest", "mapping.toml"]);
    let accept = p.run(&["init", "--manifest", "mapping.toml", "--accept"]);
    fs::set_permissions(&p.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert!(!p.0.join(".awr").exists());
    assert!(!accept.status.success());
}

fn tree_bytes(root: &std::path::Path) -> std::collections::BTreeMap<PathBuf, String> {
    fn walk(
        root: &std::path::Path,
        path: &std::path::Path,
        out: &mut std::collections::BTreeMap<PathBuf, String>,
    ) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    awr_source::fingerprint(&fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut result = Default::default();
    walk(root, root, &mut result);
    result
}

#[test]
fn semantic_preview_rejects_malformed_source_without_creating_runtime() {
    let p = Project::new();
    p.write("work-ledger.yaml", "work_items: [\n");
    let before = tree_bytes(&p.0);
    let plan = p.ok(&["init", "--manifest", "mapping.toml"]);
    assert_eq!(plan["preview"]["can_apply"], false);
    assert_eq!(plan["preview"]["semantic"]["can_execute"], false);
    assert!(
        !plan["preview"]["source_issues"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    p.error(
        &["init", "--manifest", "mapping.toml", "--accept"],
        "SourceStale",
    );
    assert_eq!(tree_bytes(&p.0), before);
}

#[test]
fn unresolved_business_references_are_visible_but_do_not_prevent_intake() {
    let p = Project::new();
    p.write("work-ledger.yaml", "work_items:\n- id: W\n  title: Write the guide\n  goal: absent\n  status: ready\n  acceptance: [Useful guide]\n  next_action: Draft it\n");
    let plan = p.ok(&["init", "--manifest", "mapping.toml"]);
    let semantic = &plan["preview"]["semantic"];
    assert_eq!(plan["preview"]["can_apply"], true);
    assert_eq!(semantic["can_execute"], false);
    assert!(
        semantic["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["code"] == "work_goal_unresolved")
    );
    assert_eq!(
        plan["preview"]["fingerprint"],
        p.ok(&["init", "--manifest", "mapping.toml"])["preview"]["fingerprint"]
    );
    p.init();
}

#[test]
fn lint_advisories_stay_visible_without_blocking_init() {
    // Review requirement: an advisory in `source_issues` used to flip
    // `can_apply` to false and stop `init --accept`, `source configure` and
    // `awr takeover`. Advisories now travel in `source_advisories`; the
    // preview stays applicable and the advisory is still visible.
    let p = Project::new();
    p.write(
        "work-ledger.yaml",
        "goals:\n- id: G\n  title: Publish a guide\n  status: active\nwork_items:\n- id: W\n  title: Draft a guide\n  goal: G\n  status: ready\n  acceptance: [Results TBD]\n  next_action: Write the first draft\n",
    );
    let plan = p.ok(&["init", "--manifest", "mapping.toml"]);
    let preview = &plan["preview"];
    assert_eq!(preview["can_apply"], true, "advisories must not block init");
    assert!(
        preview["source_issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["error"]["code"] != "acceptance_lint.placeholder"),
        "source_issues stays free of lint advisories: {:?}",
        preview["source_issues"]
    );
    let advisories = preview["source_advisories"].as_array().unwrap();
    assert!(
        advisories
            .iter()
            .any(|i| i["error"]["code"] == "acceptance_lint.placeholder"
                && i["locator"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("/acceptance/")),
        "the advisory is still visible with its locator: {advisories:?}"
    );
    p.init();
}

#[test]
fn replacement_identity_conflicts_are_preflighted_against_a_readonly_history_snapshot() {
    let p = Project::new();
    p.init();
    let work = p.ok(&["work", "show", "W"]);
    let session = p.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "fixture",
        "--provider",
        "local",
        "--model",
        "none",
        "--expected-revision",
        &work["project_revision"].to_string(),
    ]);
    let ledger = fs::read_to_string(p.0.join("work-ledger.yaml")).unwrap();
    p.write("relocated.yaml", &ledger);
    let mapping = fs::read_to_string(p.0.join("mapping.toml"))
        .unwrap()
        .replace("work-ledger.yaml", "relocated.yaml");
    p.write("replacement.toml", &mapping);
    let before = tree_bytes(&p.0);
    let plan = p.ok(&["source", "configure", "--manifest", "replacement.toml"]);
    assert_eq!(plan["preview"]["can_apply"], false);
    assert!(
        !plan["preview"]["source_issues"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        plan["preview"]["fingerprint"],
        p.ok(&["source", "configure", "--manifest", "replacement.toml"])["preview"]["fingerprint"]
    );
    p.error(
        &[
            "source",
            "configure",
            "--manifest",
            "replacement.toml",
            "--accept",
            "--expected-preview",
            plan["preview"]["fingerprint"].as_str().unwrap(),
        ],
        "SourceStale",
    );
    assert_eq!(
        tree_bytes(&p.0),
        before,
        "preview and rejected acceptance must not alter persisted files"
    );
    let saved = p.ok(&[
        "session",
        "show",
        session["session"]["id"].as_str().unwrap(),
    ]);
    assert_eq!(saved["session"]["id"], session["session"]["id"]);
    assert_eq!(
        p.ok(&["work", "show", "W"])["work"]["id"],
        work["work"]["id"]
    );
}
