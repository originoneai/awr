use serde_json::Value;
use std::process::Command;

#[test]
fn version_identity_ac1_binary_reports_the_build_commit() {
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .env("AWR_LOCAL_WEB", "0")
        .args(["--json", "--version"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let line = String::from_utf8(output.stdout).unwrap();
    let injected = option_env!("AWR_BUILD_SHA")
        .filter(|s| s.len() >= 8 && s.bytes().all(|b| b.is_ascii_hexdigit()));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if injected.is_none() && !root.join(".git").exists() {
        assert_eq!(line, format!("awr {}\n", env!("CARGO_PKG_VERSION")));
        return;
    }
    let prefix = format!("awr {} (", env!("CARGO_PKG_VERSION"));
    let identity = line
        .strip_prefix(&prefix)
        .and_then(|s| s.strip_suffix(")\n"))
        .expect("a Git build must report its source commit after the package version");
    let sha = identity.strip_suffix("-dirty").unwrap_or(identity);
    assert_eq!(sha.len(), 8, "{line}");
    assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()), "{line}");
    if let Some(sha_override) = injected {
        assert_eq!(sha, sha_override[..8].to_ascii_lowercase());
        return;
    }
    let head = Command::new("git")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(head.status.success(), "{head:?}");
    assert_eq!(sha, &String::from_utf8(head.stdout).unwrap()[..8]);
}

#[test]
fn version_identity_ac3_program_and_version_keep_their_positions() {
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .env("AWR_LOCAL_WEB", "0")
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let line = String::from_utf8(output.stdout).unwrap();
    let words: Vec<_> = line.split_whitespace().collect();
    assert_eq!(words[0], "awr");
    assert_eq!(words[1], env!("CARGO_PKG_VERSION"));
    assert_eq!(line.lines().count(), 1);
}

#[test]
fn json_usage_errors_work_before_and_after_subcommands() {
    for args in [
        vec!["--json", "work", "ready"],
        vec!["ready", "--limit", "invalid", "--json"],
        vec!["--json", "work", "show"],
        vec!["context", "compile", "--json", "--checkpoint", "invalid"],
        vec!["--json", "ready", "--unknown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "InvalidInput");
        assert!(!error["message"].as_str().unwrap().is_empty());
    }
}

#[test]
fn literal_json_text_does_not_change_error_format() {
    for args in [
        vec!["work", "show", "--", "--json", "extra"],
        vec!["--project=--json", "--unknown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(serde_json::from_slice::<Value>(&output.stderr).is_err());
    }
}

#[test]
fn help_version_and_unsupported_have_explicit_exit_contracts() {
    for flag in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--json", flag])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!output.stdout.is_empty());
        if flag == "--help" {
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(text.contains("Daily work"), "{text}");
            assert!(text.contains("awr help all"), "{text}");
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["--json", "unsupported-command"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "Unsupported"
    );
}

#[test]
fn session_start_help_documents_json_output_shape() {
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["session", "start", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("session.id"),
        "session start --help must surface the session.id JSON path"
    );
}

#[test]
fn evidence_add_help_documents_required_draft_fields() {
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["evidence", "add", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8_lossy(&output.stdout);
    for needle in ["external_key", "work_item_key", "locator"] {
        assert!(
            text.contains(needle),
            "evidence add --help must mention {needle}"
        );
    }
}
