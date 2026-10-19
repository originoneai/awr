//! `source configure` must not reset the context profile by omission: a
//! candidate manifest without `context_profile` keeps the project's current
//! profile, while an explicitly written profile still wins.
use awr_core::Id;
use awr_source::ContextProfile;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

const PLAIN: &str = "[project]\nname='Guide'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work-ledger.yaml'\nadapter='yaml-ledger-v1'\n";
const MINIMAL: &str = "[project]\nname='Guide'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work-ledger.yaml'\nadapter='yaml-ledger-v1'\n";
const DECISIONS: &str = "\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n";

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("awr context profile {}", Id::new()));
        fs::create_dir(&path).unwrap();
        let p = Self(path);
        p.write(
            "work-ledger.yaml",
            "goals:\n- id: G\n  title: Publish a guide\n  status: active\nwork_items:\n- id: W\n  title: Draft a guide\n  goal: G\n  status: ready\n  acceptance: [A useful guide]\n  next_action: Write the first draft\n",
        );
        p.write("decisions/choice.md", "# Use a short guide\n");
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
    fn init(&self, manifest: &str) {
        let preview = self.ok(&["init", "--manifest", manifest]);
        self.ok(&[
            "init",
            "--manifest",
            manifest,
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
        ]);
    }
    fn configure(&self, manifest: &str) -> Value {
        let preview = self.ok(&["source", "configure", "--manifest", manifest]);
        self.ok(&[
            "source",
            "configure",
            "--manifest",
            manifest,
            "--accept",
            "--expected-preview",
            preview["preview"]["fingerprint"].as_str().unwrap(),
        ])
    }
    fn written_profile(&self) -> ContextProfile {
        awr_source::Manifest::load(&self.0)
            .unwrap()
            .project
            .context_profile
    }
    fn written_text(&self) -> String {
        fs::read_to_string(self.0.join(".awr/project.toml")).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn configure_preserves_minimal_context_profile_when_candidate_omits_it() {
    let p = Project::new();
    p.write("mapping.toml", MINIMAL);
    p.init("mapping.toml");
    assert_eq!(p.written_profile(), ContextProfile::Minimal);
    p.write("candidate.toml", &format!("{PLAIN}{DECISIONS}"));
    let result = p.configure("candidate.toml");
    assert_eq!(result["write_outcome"], "applied");
    assert_eq!(p.written_profile(), ContextProfile::Minimal);
    assert!(
        p.written_text().contains("context_profile = \"minimal\""),
        "{}",
        p.written_text()
    );
}

#[test]
fn configure_leaves_standard_profile_alone_when_candidate_omits_it() {
    let p = Project::new();
    p.write("mapping.toml", PLAIN);
    p.init("mapping.toml");
    assert_eq!(p.written_profile(), ContextProfile::Standard);
    p.write("candidate.toml", &format!("{PLAIN}{DECISIONS}"));
    let result = p.configure("candidate.toml");
    assert_eq!(result["write_outcome"], "applied");
    assert_eq!(p.written_profile(), ContextProfile::Standard);
    assert!(
        !p.written_text().contains("context_profile"),
        "{}",
        p.written_text()
    );
}

#[test]
fn configure_honours_explicit_standard_profile() {
    let p = Project::new();
    p.write("mapping.toml", MINIMAL);
    p.init("mapping.toml");
    assert_eq!(p.written_profile(), ContextProfile::Minimal);
    p.write(
        "candidate.toml",
        &MINIMAL.replace("'minimal'", "'standard'"),
    );
    let result = p.configure("candidate.toml");
    assert_eq!(result["write_outcome"], "applied");
    assert_eq!(p.written_profile(), ContextProfile::Standard);
    assert!(
        !p.written_text().contains("context_profile"),
        "{}",
        p.written_text()
    );
}
