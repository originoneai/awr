use awr_core::Id;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Project {
    fn new() -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-move-{}", Id::new())));
        fs::create_dir(&p.0).unwrap();
        p.write("work.yaml","goals:\n- id: G\n  title: Publish a guide\n  status: active\n  summary: Explain the workflow\nwork_items:\n- id: W\n  title: Draft the guide\n  status: ready\n  goal: G\n  acceptance: [Useful guide]\n  next_action: Draft the introduction\n- id: R\n  title: Review the guide\n  status: planned\n  depends_on: [W]\n");
        p.write("mapping.toml","[project]\nname='Guide'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n");
        p.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        p
    }
    fn write(&self, path: &str, text: &str) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
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
            "{}\n{}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn fail(&self, args: &[&str], code: &str) {
        let r = self.run(args);
        assert!(!r.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stderr).unwrap()["code"],
            code,
            "args={args:?}, stderr={}",
            String::from_utf8_lossy(&r.stderr)
        );
    }
    fn source(&self) -> String {
        self.ok(&["source", "list"])["sources"][0]["id"]
            .as_str()
            .unwrap()
            .into()
    }
    fn copy(&self) {
        let text = fs::read_to_string(self.0.join("work.yaml")).unwrap();
        self.write("plans/ledger.yaml", &text);
    }
    fn preview(&self, id: &str) -> Value {
        self.ok(&[
            "source",
            "relocate",
            "--source",
            id,
            "--to",
            "plans/ledger.yaml",
        ])["preview"]
            .clone()
    }
    fn apply(&self, id: &str, key: &str) -> Value {
        self.ok(&[
            "source",
            "relocate",
            "--source",
            id,
            "--to",
            "plans/ledger.yaml",
            "--accept",
            "--expected-preview",
            key,
        ])
    }
}
#[test]
fn relocation_preserves_graph_sessions_checkpoints_evidence_and_old_location_history() {
    let p = Project::new();
    let work = p.ok(&["work", "show", "W"]);
    let review = p.ok(&["work", "show", "R"]);
    let s = p.ok(&[
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
        "--claim",
        "--expected-revision",
        &work["project_revision"].to_string(),
    ]);
    let sid = s["session"]["id"].as_str().unwrap();
    let context = p.ok(&["context", "compile", "--work", "W", "--session", sid]);
    assert_eq!(context["completeness"]["complete"], true);
    let cp = p.ok(&[
        "session",
        "checkpoint",
        "--session",
        sid,
        "--context-hash",
        context["work_context"]["context_hash"].as_str().unwrap(),
        "--digest",
        "Guide outline drafted",
        "--next-action",
        "Write introduction",
        "--expected-revision",
        &context["project_revision"].to_string(),
    ]);
    p.write("evidence.json",&json!({"external_key":"E","work_item_key":"W","evidence_type":"observation","level":"designed","summary":"Guide outline","locator":"guide.txt","sha256":null,"source_sha":null,"command":null,"scope":["W"],"branch_id":null,"verified_at":null}).to_string());
    p.write("guide.txt", "A guide outline");
    let evidence = p.ok(&[
        "evidence",
        "add",
        "--input",
        "evidence.json",
        "--expected-revision",
        &cp["project_revision"].to_string(),
    ]);
    let evidence_before = p.ok(&[
        "evidence",
        "show",
        evidence["evidence"]["id"].as_str().unwrap(),
    ]);
    let source = p.source();
    p.copy();
    // A file already moved by the user is supported against the indexed baseline.
    fs::remove_file(p.0.join("work.yaml")).unwrap();
    let manifest = fs::read(p.0.join(".awr/project.toml")).unwrap();
    let before = fs::read(p.0.join(".awr/state.db")).unwrap();
    let preview = p.preview(&source);
    assert_eq!(preview["old_file_present"], false);
    assert_eq!(preview["fingerprint"], p.preview(&source)["fingerprint"]);
    assert_eq!(fs::read(p.0.join(".awr/project.toml")).unwrap(), manifest);
    assert!(fs::read(p.0.join(".awr/state.db")).unwrap() == before);
    let key = preview["fingerprint"].as_str().unwrap();
    let result = p.apply(&source, key);
    assert_eq!(result["write_outcome"], "completed");
    assert_eq!(p.source(), source);
    assert_eq!(
        p.ok(&["work", "show", "W"])["work"]["id"],
        work["work"]["id"]
    );
    let after = p.ok(&["work", "show", "R"]);
    assert_eq!(after["work"]["id"], review["work"]["id"]);
    assert_eq!(review["required_dependencies"][0]["external_key"], "W");
    assert_eq!(after["required_dependencies"][0]["external_key"], "W");
    assert_eq!(after["required_dependencies"].as_array().unwrap().len(), 1);
    assert_eq!(
        p.ok(&["session", "show", sid])["session"]["last_checkpoint_id"],
        cp["checkpoint"]["id"]
    );
    assert!(!evidence_before["evidence"].is_null());
    let evidence_after = p.ok(&[
        "evidence",
        "show",
        evidence["evidence"]["id"].as_str().unwrap(),
    ]);
    assert_eq!(evidence_after["evidence"], evidence_before["evidence"]);
    let history = p.ok(&["source", "history", &source]);
    let relocation = history["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "source.relocated")
        .unwrap();
    let event = p.ok(&[
        "event",
        "show",
        relocation["id"].as_str().unwrap(),
        "--full",
    ]);
    let text = event.to_string();
    assert!(text.contains("work.yaml") && text.contains("plans/ledger.yaml"));
    let changes = p.ok(&["source", "changes"]);
    assert!(
        changes["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["event_type"] == "source.relocated" && e["configuration_changed"] == true)
    );
    let revision = p.ok(&["work", "show", "W"])["project_revision"].clone();
    p.apply(&source, key);
    assert_eq!(p.ok(&["work", "show", "W"])["project_revision"], revision);
}
#[test]
fn changed_previews_and_colliding_destinations_are_rejected_before_binding_changes() {
    let p = Project::new();
    let id = p.source();
    p.copy();
    let plan = p.preview(&id);
    let before = fs::read(p.0.join(".awr/project.toml")).unwrap();
    p.write("plans/ledger.yaml", "work_items: []\n");
    p.fail(
        &[
            "source",
            "relocate",
            "--source",
            &id,
            "--to",
            "plans/ledger.yaml",
            "--accept",
            "--expected-preview",
            plan["fingerprint"].as_str().unwrap(),
        ],
        "SourceConflict",
    );
    assert_eq!(fs::read(p.0.join(".awr/project.toml")).unwrap(), before);
    p.copy();
    p.write(
        ".awr/project.toml",
        &(String::from_utf8(before).unwrap() + "\n# User note\n"),
    );
    p.fail(
        &[
            "source",
            "relocate",
            "--source",
            &id,
            "--to",
            "plans/ledger.yaml",
            "--accept",
            "--expected-preview",
            plan["fingerprint"].as_str().unwrap(),
        ],
        "SourceConflict",
    );
    // A retained source owns the destination even if it has no colliding work keys.
    p.write("empty.yaml", "work_items: []\n");
    let base = fs::read_to_string(p.0.join(".awr/project.toml")).unwrap();
    let config = fs::read_to_string(p.0.join(".awr/project.toml")).unwrap()
        + "\n[[sources]]\ndomain='ledger'\nrole='supporting'\npath='empty.yaml'\nadapter='yaml-ledger-v1'\n";
    p.write(".awr/project.toml", &config);
    p.ok(&["source", "reindex"]);
    p.write(".awr/project.toml", &base);
    p.ok(&["source", "reindex"]);
    p.write(
        "empty.yaml",
        &fs::read_to_string(p.0.join("work.yaml")).unwrap(),
    );
    p.fail(
        &["source", "relocate", "--source", &id, "--to", "empty.yaml"],
        "SourceConflict",
    );
    assert_eq!(p.source(), id);
}
#[test]
fn interrupted_binding_can_be_inspected_recovered_and_rejects_external_edits() {
    let p = Project::new();
    let id = p.source();
    p.copy();
    let plan = p.preview(&id);
    let key = plan["fingerprint"].as_str().unwrap();
    let before = fs::read_to_string(p.0.join(".awr/project.toml")).unwrap();
    let mut receipt = p.apply(&id, key);
    // Inject the durable state after binding commit but before manifest replacement.
    let dir = format!(
        ".awr/mutations/source-relocate-{}",
        key.strip_prefix("sha256:").unwrap()
    );
    receipt["write_outcome"] = json!("binding_relocated");
    receipt["ok"] = json!(false);
    p.write(&format!("{dir}/receipt.json"), &receipt.to_string());
    p.write(".awr/project.toml", &before);
    p.write(".awr/mutations/source-relocation.pending", key);
    p.fail(&["source", "reindex"], "SourceConflict");
    assert_eq!(
        p.ok(&["source", "relocate-status", key])["observed"]["pending_marker"],
        key
    );
    p.write("plans/ledger.yaml", "work_items: []\n");
    p.fail(&["source", "relocate-recover", key], "SourceConflict");
    assert_eq!(
        fs::read_to_string(p.0.join(".awr/project.toml")).unwrap(),
        before
    );
    p.copy();
    let recovered = p.ok(&["source", "relocate-recover", key]);
    assert_eq!(recovered["write_outcome"], "completed");
    assert!(
        !p.0.join(".awr/mutations/source-relocation.pending")
            .exists()
    );
    assert_eq!(p.source(), id);
    p.ok(&["source", "relocate-recover", key]);
    p.ok(&["work", "show", "W"]);
}
#[test]
fn relocation_receipt_without_from_reports_invalid_input() {
    let p = Project::new();
    let id = p.source();
    p.copy();
    let plan = p.preview(&id);
    let key = plan["fingerprint"].as_str().unwrap();
    let mut receipt = p.apply(&id, key);
    let dir = format!(
        ".awr/mutations/source-relocate-{}",
        key.strip_prefix("sha256:").unwrap()
    );
    // A receipt that still matches this project and preview, but lost the original path.
    receipt["write_outcome"] = json!("binding_relocated");
    receipt["ok"] = json!(false);
    receipt["plan"].as_object_mut().unwrap().remove("from");
    p.write(&format!("{dir}/receipt.json"), &receipt.to_string());
    p.write(".awr/mutations/source-relocation.pending", key);
    p.fail(&["source", "relocate-recover", key], "InvalidInput");
}
