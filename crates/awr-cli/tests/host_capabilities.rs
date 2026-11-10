use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Host(PathBuf);
impl Host {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr 宿主 contract {}", awr_core::Id::new()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .env_clear()
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let result = self.run(args);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stderr.is_empty());
        serde_json::from_slice(&result.stdout).unwrap()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn check_capabilities_negotiate_together_and_unknown_checks_are_unavailable() {
    let host = Host::new();
    let result = host.ok(&[
        "--json",
        "capabilities",
        "--require",
        "definition.example_checks",
        "--require",
        "gates.checks",
        "--require",
        "mcp.work_check",
    ]);
    assert!(
        result["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "mcp.work_check" && c["available"] == true)
    );
    let refused = host.run(&["--json", "capabilities", "--require", "mcp.unknown_check"]);
    assert_eq!(refused.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert_eq!(error["code"], "CapabilityUnavailable");
    assert_eq!(
        error["details"]["unknown"],
        serde_json::json!(["mcp.unknown_check"])
    );
    assert_eq!(fs::read_dir(&host.0).unwrap().count(), 0);
}

#[test]
fn trust_capability_is_negotiable_without_a_project() {
    let host = Host::new();
    let result = host.ok(&["--json", "capabilities", "--require", "output.trust_field"]);
    assert!(
        result["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "output.trust_field" && c["available"] == true)
    );
    assert_eq!(fs::read_dir(&host.0).unwrap().count(), 0);
}

#[test]
fn negotiation_needs_no_project_tty_model_or_environment() {
    let host = Host::new();
    let result = host.ok(&[
        "capabilities",
        "--json",
        "--project",
        "不存在的 project",
        "--require",
        "context.compile",
        "--require",
        "intake.semantic_preflight",
        "--require",
        "source.relocate",
    ]);
    assert_eq!(result["protocol"]["version"], 1);
    assert_eq!(result["program"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        result["database"]["schema_version"],
        awr_store::SCHEMA_VERSION
    );
    assert_eq!(result["runtime_write_performed"], false);
    assert_eq!(result["source_write_performed"], false);
    assert_eq!(fs::read_dir(&host.0).unwrap().count(), 0);
    // Stable discovery does not acquire a DB, emit timestamps or allocate identities.
    assert_eq!(result, host.ok(&["--json", "capabilities"]));
}

#[test]
fn callers_distinguish_unknown_from_now_available_multi_file_capability() {
    let host = Host::new();
    let result = host.run(&[
        "capabilities",
        "--json",
        "--require",
        "mutation.multi_file",
        "--require",
        "fictional.capability",
        "--require",
        "source.read",
        "--require",
        "fictional.capability",
    ]);
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(error["code"], "CapabilityUnavailable");
    assert_eq!(
        error["details"]["unknown"],
        serde_json::json!(["fictional.capability"])
    );
    assert_eq!(error["details"]["unsupported"], serde_json::json!([]));
    assert_eq!(error["details"]["runtime_write_performed"], false);
    assert_eq!(fs::read_dir(&host.0).unwrap().count(), 0);
}

#[test]
fn unsupported_protocol_and_invalid_usage_keep_distinct_error_codes() {
    let host = Host::new();
    let result = host.run(&["--json", "capabilities", "--protocol-version", "999"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(error["code"], "ProtocolUnsupported");
    assert_eq!(error["details"]["requested"], 999);
    assert_eq!(error["details"]["supported"], serde_json::json!([1]));
    let usage = host.run(&[
        "--json",
        "capabilities",
        "--protocol-version",
        "not-a-number",
    ]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(usage.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&usage.stderr).unwrap()["code"],
        "InvalidInput"
    );
}

#[test]
fn capability_modes_advertise_the_finite_writers() {
    let host = Host::new();
    let result = host.ok(&["--json", "capabilities"]);
    let adapters = result["source_adapters"].as_array().unwrap();
    for adapter in adapters {
        assert_eq!(adapter["read"], true);
        assert_eq!(
            adapter["lossless_field_write"],
            adapter["id"] == "yaml-ledger-v1" || adapter["id"] == "markdown-ledger-v1"
        );
        if adapter["id"] == "yaml-ledger-v1" {
            assert_eq!(adapter["write_mode"], "lossless_supported_fields");
        } else if adapter["id"] == "markdown-ledger-v1" {
            assert_eq!(adapter["write_mode"], "stable_id_table_or_checklist");
        } else {
            assert_eq!(adapter["write_mode"], "document_body_only");
        }
    }
    for id in ["mutation.multi_file", "completion.user_confirmation"] {
        let capability = result["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap();
        assert_eq!(capability["available"], true, "{id}");
    }
}

#[test]
fn discovery_does_not_open_or_migrate_an_existing_database() {
    let host = Host::new();
    fs::create_dir(host.0.join(".awr")).unwrap();
    // Even malformed/future data is outside discovery's scope; opening would fail.
    let db = host.0.join(".awr/state.db");
    let config = host.0.join(".awr/project.toml");
    fs::write(&db, b"opaque future database").unwrap();
    fs::write(&config, b"future manifest").unwrap();
    host.ok(&["--json", "capabilities"]);
    assert_eq!(fs::read(db).unwrap(), b"opaque future database");
    assert_eq!(fs::read(config).unwrap(), b"future manifest");
    assert_eq!(fs::read_dir(host.0.join(".awr")).unwrap().count(), 2);
}

#[test]
fn batch_and_archive_capabilities_are_negotiable() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_awr"))
        .env_clear()
        .args([
            "--json",
            "capabilities",
            "--require",
            "mutation.batch.ledger",
            "--require",
            "mutation.work.archive",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
