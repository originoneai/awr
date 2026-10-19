//! `source configure` must not turn the Team switch off by omission: a
//! candidate manifest without a `[team]` table keeps the project's current
//! value, while an explicit `enabled = false` still wins.
use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

const MAPPING: &str = "[project]\nname='Guide'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work-ledger.yaml'\nadapter='yaml-ledger-v1'\n";

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("awr team configure {}", Id::new()));
        fs::create_dir(&path).unwrap();
        let p = Self(path);
        p.write(
            "work-ledger.yaml",
            "goals:\n- id: G\n  title: Publish a guide\n  status: active\nwork_items:\n- id: W\n  title: Draft a guide\n  goal: G\n  status: ready\n  acceptance: [A useful guide]\n  next_action: Write the first draft\n",
        );
        p.write("mapping.toml", MAPPING);
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
    fn init(&self) {
        let preview = self.ok(&["init", "--manifest", "mapping.toml"]);
        self.ok(&[
            "init",
            "--manifest",
            "mapping.toml",
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
    fn append_team(&self, enabled: bool) {
        let current = fs::read_to_string(self.0.join(".awr/project.toml")).unwrap();
        self.write(
            ".awr/project.toml",
            &format!("{current}\n[team]\nenabled = {enabled}\n"),
        );
    }
    fn written_team_enabled(&self) -> bool {
        awr_source::Manifest::load(&self.0).unwrap().team.enabled
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn configure_preserves_team_when_candidate_omits_it() {
    let p = Project::new();
    p.init();
    p.append_team(true);
    assert!(p.written_team_enabled());
    let result = p.configure("mapping.toml");
    assert_eq!(result["write_outcome"], "no_change");
    assert!(p.written_team_enabled());
}

#[test]
fn configure_leaves_default_team_alone_when_candidate_omits_it() {
    let p = Project::new();
    p.init();
    assert!(!p.written_team_enabled());
    let result = p.configure("mapping.toml");
    assert_eq!(result["write_outcome"], "no_change");
    assert!(!p.written_team_enabled());
}

#[test]
fn configure_honours_explicit_team_disabled() {
    let p = Project::new();
    p.init();
    p.append_team(true);
    p.write(
        "team-off.toml",
        &format!("{MAPPING}\n[team]\nenabled = false\n"),
    );
    let result = p.configure("team-off.toml");
    assert_eq!(result["write_outcome"], "applied");
    assert!(!p.written_team_enabled());
}
