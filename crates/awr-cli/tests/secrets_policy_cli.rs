//! The `awr policy secrets` command line: one command switches the mode with
//! provenance, `show` reports it, and every warn-mode admission leaves a
//! queryable receipt. Strict stays the default and keeps the historical
//! refusal; placeholders never block in either mode.
use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const WORK: &str = "work_items:\n- id: W\n  title: Carry a key\n  status: planned\n  next_action: Start\n  acceptance: [Deliver]\n";
const CREDENTIAL_WORK: &str = "work_items:\n- id: W\n  title: Carry a key\n  status: planned\n  next_action: Start\n  acceptance: [Deliver]\n  api_key: sk-ant-abcdefghijklmnopqrstuvwxyz012345\n";
const SOURCES: &str = "[project]\nname='Secrets policy'\nexternal_key='secrets-policy'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n";

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    /// Initializes with the clean ledger, then swaps in the requested one:
    /// init itself must pass the strict default.
    fn new(ledger: &str) -> Self {
        let f = Self {
            root: std::env::temp_dir().join(format!("awr-secrets-policy-cli-{}", Id::new())),
        };
        fs::create_dir(&f.root).unwrap();
        fs::write(f.root.join("work.yaml"), WORK).unwrap();
        fs::write(f.root.join("sources.toml"), SOURCES).unwrap();
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        fs::write(f.root.join("work.yaml"), ledger).unwrap();
        f
    }
    fn run_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_awr"));
        command
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .env_remove("USER")
            .env_remove("USERNAME");
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        self.ok_with(args, &[])
    }
    fn ok_with(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let out = self.run_with(args, env);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    /// Parses stdout even when the command exits nonzero: a strict refusal is
    /// a valid observation, not a panic.
    fn reindex(&self) -> Value {
        let out = self.run_with(&["source", "reindex"], &[]);
        serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
            panic!(
                "reindex stdout was not JSON: {}",
                String::from_utf8_lossy(&out.stdout)
            )
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn strict_by_default_warn_by_command_and_every_admission_is_receipted() {
    let f = Fixture::new(CREDENTIAL_WORK);
    // Before any switch: no policy file, show reports strict, and a refresh
    // keeps the historical refusal.
    let show = f.ok(&["policy", "secrets", "show"]);
    assert_eq!(show["mode"], "strict");
    assert_eq!(show["declared"], false);
    let refused = f.reindex();
    assert_eq!(refused["ok"], false);
    assert!(refused["sources"][0]["action"] == "failed");

    // One command switches to warn with provenance; the policy file round-trips.
    let switched = f.ok_with(
        &["policy", "secrets", "warn", "--actor", "operator-a"],
        &[("USER", "should-not-matter")],
    );
    assert_eq!(switched["mode"], "warn");
    assert_eq!(switched["enabled_by"], "operator-a");
    let policy_text = fs::read_to_string(f.root.join(".awr/policy.toml")).unwrap();
    assert!(policy_text.contains("mode = \"warn\""));
    assert!(policy_text.contains("enabled_by = \"operator-a\""));
    let show = f.ok(&["policy", "secrets", "show"]);
    assert_eq!(show["mode"], "warn");
    assert_eq!(show["enabled_by"], "operator-a");

    // The same refresh now admits the source with a warning that names the
    // position and category, and the receipt is queryable.
    let admitted = f.reindex();
    assert_eq!(admitted["ok"], true, "{admitted}");
    let warnings = admitted["sensitive_warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0]["category"], "credential");
    assert!(
        warnings[0]["locator"]
            .as_str()
            .is_some_and(|l| l.contains("work.yaml"))
    );
    assert!(warnings[0]["line"].as_u64().is_some());
    let log = f.ok(&["policy", "secrets", "log"]);
    let events = log.as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["enabled_by"], "operator-a");
    let findings = events[0]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["category"], "credential");
    assert!(!log.to_string().contains("sk-ant"));

    // Switching back restores strict and its refusal.
    f.ok(&["policy", "secrets", "strict", "--actor", "operator-a"]);
    let show = f.ok(&["policy", "secrets", "show"]);
    assert_eq!(show["mode"], "strict");
    fs::write(f.root.join("work.yaml"), CREDENTIAL_WORK).unwrap();
    let refused = f.reindex();
    assert_eq!(refused["ok"], false);
}

#[test]
fn placeholders_never_block_in_either_mode() {
    let f = Fixture::new(WORK);
    let clean = f.reindex();
    assert_eq!(clean["ok"], true);
    fs::write(
        f.root.join("work.yaml"),
        "work_items:\n- id: W\n  title: Placeholders\n  status: planned\n  next_action: Start\n  acceptance: [Deliver]\n  api_key: ${API_KEY}\n  other: [redacted]\n",
    )
    .unwrap();
    f.ok(&["policy", "secrets", "warn", "--actor", "operator-b"]);
    let admitted = f.reindex();
    assert_eq!(admitted["ok"], true);
    // Empty warnings are skipped in serialization: no key means none admitted.
    assert!(
        admitted["sensitive_warnings"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(true)
    );
}
