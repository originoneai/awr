use serde_json::Value;
use std::process::Command;

/// These tests exercise Team surfaces, so they turn the runtime switch on.
/// Switch-off behaviour is covered by `team_switch.rs`.
fn awr() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_awr"));
    command.env("AWR_TEAM_ENABLED", "1");
    command
}

#[test]
fn team_claim_without_remote_does_not_succeed_locally() {
    let output = awr()
        .args([
            "--json",
            "team",
            "command",
            "--body",
            r#"{"protocol_version":1,"request_id":"r1","op":"work.claim","args":{"work_id":"work-a","scope_id":"main","session_id":"s1","expected_work_version":"1","expected_contract_hash":"h"}}"#,
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "Unsupported");
    assert!(error["message"].as_str().unwrap().contains("team remote"));
}

#[test]
fn remote_add_rejects_database_urls() {
    let dir = tempfile_dir();
    let output = awr()
        .args([
            "--json",
            "--project",
            &dir,
            "remote",
            "add",
            "prod",
            "--endpoint",
            "postgres://postgres:awr-test@127.0.0.1/awr",
            "--project-key",
            "alpha",
            "--credential-env",
            "AWR_TEAM_TOKEN",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "RuleViolation");
}

fn tempfile_dir() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "awr-team-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().into_owned()
}

fn claim() -> Value {
    serde_json::json!({"protocol_version":1,"request_id":"r","op":"work.claim","args":{"work_id":"w","scope_id":"main","session_id":"s","expected_work_version":"1","expected_contract_hash":"h"}})
}
fn add(dir: &str, endpoint: &str, key: &str) -> std::process::Output {
    awr()
        .args([
            "--json",
            "--project",
            dir,
            "remote",
            "add",
            "test",
            "--endpoint",
            endpoint,
            "--project-key",
            key,
            "--credential-env",
            "AWR_TEST_TEAM_TOKEN",
        ])
        .output()
        .unwrap()
}
fn command(dir: &str, body: &Value, offline: bool, token: bool) -> std::process::Output {
    let mut c = awr();
    c.args([
        "--json",
        "--project",
        dir,
        "team",
        "command",
        "--remote",
        "test",
        "--body",
        &body.to_string(),
    ]);
    if offline {
        c.arg("--offline");
    }
    c.env_remove("AWR_TEST_TEAM_TOKEN");
    if token {
        c.env("AWR_TEST_TEAM_TOKEN", "synthetic-test-token");
    }
    c.output().unwrap()
}
fn error(out: std::process::Output) -> Value {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty());
    serde_json::from_slice(&out.stderr).expect("normal structured error, not panic")
}
#[test]
fn configured_online_command_is_explicitly_unsupported_and_sends_nothing() {
    let dir = tempfile_dir();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    assert!(
        add(
            &dir,
            &format!("http://{}/team/v1", listener.local_addr().unwrap()),
            "alpha"
        )
        .status
        .success()
    );
    for token in [false, true] {
        for _ in 0..2 {
            let e = error(command(&dir, &claim(), false, token));
            assert_eq!(e["code"], "Unsupported");
            assert!(
                e["message"]
                    .as_str()
                    .unwrap()
                    .contains("nothing was submitted")
            );
        }
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let entries: Vec<_> = std::fs::read_dir(std::path::Path::new(&dir).join(".awr"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("team-remotes")]);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn cli_rejects_unknown_offline_operations_and_invalid_claim_parameters() {
    let dir = tempfile_dir();
    assert!(
        add(&dir, "https://example.invalid/team/v1", "alpha")
            .status
            .success()
    );
    for op in [
        "definitely.not.an.operation",
        "execution.cancel",
        "source.activate",
    ] {
        let mut b = claim();
        b["op"] = serde_json::json!(op);
        for offline in [false, true] {
            assert_eq!(
                error(command(&dir, &b, offline, false))["code"],
                "Unsupported"
            );
        }
    }
    for args in [
        serde_json::json!({}),
        serde_json::json!([]),
        serde_json::json!("not-an-object"),
        serde_json::json!({"work_id":false,"scope_id":null,"session_id":[],"expected_work_version":{},"expected_contract_hash":42}),
    ] {
        let mut b = claim();
        b["args"] = args;
        assert_eq!(
            error(command(&dir, &b, false, false))["code"],
            "InvalidInput"
        );
    }
    for body in [
        serde_json::json!([]),
        serde_json::json!(true),
        serde_json::json!(7),
        serde_json::json!("text"),
    ] {
        assert_eq!(
            error(command(&dir, &body, false, false))["code"],
            "InvalidInput"
        );
    }
    for version in [
        serde_json::json!(4294967297u64),
        serde_json::json!("4294967297"),
        serde_json::json!(-1),
    ] {
        let mut b = claim();
        b["protocol_version"] = version;
        assert!(!command(&dir, &b, false, false).status.success());
    }
    for key in ["protocol_version", "request_id"] {
        let mut b = claim();
        b.as_object_mut().unwrap().remove(key);
        error(command(&dir, &b, false, false));
    }
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn remote_unicode_roundtrip_and_safe_url_controls() {
    let dir = tempfile_dir();
    let key = "alpha\u{a0}beta\"\\中文";
    for url in [
        "https://example.invalid/team/v1",
        "http://localhost:8080/team/v1",
        "http://[::1]:8080/team/v1",
        "https://[2001:db8::1]/team/v1",
    ] {
        let out = add(&dir, url, key);
        assert!(out.status.success(), "{out:?}");
        let out = awr()
            .args(["--json", "--project", &dir, "remote", "inspect", "test"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["project_key"], key);
        assert_eq!(v["endpoint"], url);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn add_and_existing_profile_load_reject_secret_urls_without_echo() {
    let dir = tempfile_dir();
    for url in [
        "https://:review-only-dummy@example.invalid/team/v1",
        "https://user:review-only-dummy@example.invalid/team/v1",
        "postgresql://127.0.0.1/db",
        "https://example.invalid/?password=review-only-dummy",
    ] {
        let out = add(&dir, url, "alpha");
        assert!(!out.status.success());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("review-only-dummy"));
        assert!(!String::from_utf8_lossy(&out.stdout).contains("review-only-dummy"));
        assert!(
            !std::path::Path::new(&dir)
                .join(".awr/team-remotes/test.toml")
                .exists()
        );
    }
    assert!(
        add(&dir, "https://example.invalid/team/v1", "alpha")
            .status
            .success()
    );
    let path = std::path::Path::new(&dir).join(".awr/team-remotes/test.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    for text in [
        original.replace(
            "https://example.invalid/team/v1",
            "https://:review-only-dummy@example.invalid/team/v1",
        ),
        "endpoint = \"https://:review-only-dummy@example.invalid/\\u{a0}\"".into(),
    ] {
        std::fs::write(&path, text).unwrap();
        let out = awr()
            .args(["--json", "--project", &dir, "remote", "inspect", "test"])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("review-only-dummy"));
        assert!(out.stdout.is_empty());
    }
    for text in [
        original.replace("protocol_version = 1", "protocol_version = 4294967297"),
        original.replace("protocol_version = 1", "protocol_version = -1"),
        original.replace("protocol_version = 1", "protocol_version = \"1\""),
        original.replace("protocol_version = 1", ""),
    ] {
        std::fs::write(&path, text).unwrap();
        let out = awr()
            .args(["--json", "--project", &dir, "remote", "inspect", "test"])
            .output()
            .unwrap();
        assert!(!out.status.success());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn capabilities_are_explicitly_local_and_create_no_runtime_state() {
    let dir = tempfile_dir();
    let out = awr()
        .args(["--json", "--project", &dir, "team", "capabilities"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["local"], true);
    assert_eq!(v["submitted"], false);
    assert_eq!(v["command_transport"], false);
    assert!(!std::path::Path::new(&dir).join(".awr").exists());
    std::fs::remove_dir_all(dir).unwrap();
}
