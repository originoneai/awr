//! Runtime Team switch: Team code is always built, but `awr team` and
//! `awr remote` only run when the switch is on.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn dir() -> Dir {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "awr-team-switch-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    Dir(path)
}
fn awr(project: &Dir, switch: Option<&str>, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_awr"));
    command.env_remove("AWR_TEAM_ENABLED");
    if let Some(value) = switch {
        command.env("AWR_TEAM_ENABLED", value);
    }
    command
        .arg("--json")
        .arg("--project")
        .arg(&project.0)
        .args(args)
        .output()
        .unwrap()
}
fn write_manifest(project: &Dir, team: Option<bool>) {
    fs::create_dir_all(project.0.join(".awr")).unwrap();
    fs::write(project.0.join("ledger.yaml"), "items: []\n").unwrap();
    let mut text = String::from(
        "[project]\nname = 'Switch fixture'\n\n[[sources]]\ndomain = 'ledger'\nrole = 'primary'\npath = 'ledger.yaml'\nadapter = 'yaml-ledger-v1'\n",
    );
    if let Some(enabled) = team {
        text.push_str(&format!("\n[team]\nenabled = {enabled}\n"));
    }
    fs::write(project.0.join(".awr/project.toml"), text).unwrap();
}
fn disabled(out: std::process::Output) {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "Unsupported");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("team features are disabled"), "{message}");
    assert!(message.contains("AWR_TEAM_ENABLED=1"), "{message}");
}

#[test]
fn team_and_remote_are_off_by_default_and_create_no_state() {
    let project = dir();
    disabled(awr(&project, None, &["team", "capabilities"]));
    disabled(awr(&project, None, &["remote", "inspect", "origin"]));
    assert!(!project.0.join(".awr").exists());
}

#[test]
fn environment_override_turns_team_on_and_off() {
    let project = dir();
    for on in ["1", "true", "ON"] {
        let out = awr(&project, Some(on), &["team", "capabilities"]);
        assert!(out.status.success(), "{out:?}");
    }
    write_manifest(&project, Some(true));
    for off in ["0", "false", "off"] {
        disabled(awr(&project, Some(off), &["team", "capabilities"]));
    }
}

#[test]
fn project_config_key_enables_team() {
    let project = dir();
    write_manifest(&project, None);
    disabled(awr(&project, None, &["team", "capabilities"]));
    write_manifest(&project, Some(false));
    disabled(awr(&project, None, &["team", "capabilities"]));
    write_manifest(&project, Some(true));
    let out = awr(&project, None, &["team", "capabilities"]);
    assert!(out.status.success(), "{out:?}");
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["local"], true);
}

#[test]
fn invalid_override_is_rejected_without_echo() {
    let project = dir();
    let out = awr(
        &project,
        Some("enable-secret-xyz"),
        &["team", "capabilities"],
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("AWR_TEAM_ENABLED must be one of"),
        "{stderr}"
    );
    assert!(!stderr.contains("enable-secret-xyz"));
}
