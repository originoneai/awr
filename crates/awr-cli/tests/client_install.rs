use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr adapter {}", awr_core::Id::new())));
        fs::create_dir(&p.0).unwrap();
        p.ok(&[
            "init",
            "--goal",
            "Deliver a reviewed search change",
            "--accept",
        ]);
        p
    }
    fn run(&self, args: &[&str], input: Option<Value>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .env("AWR_LOCAL_WEB", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&input).unwrap())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        child.wait_with_output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        decode(self.run(args, None))
    }
    fn error(&self, args: &[&str], code: &str) {
        let o = self.run(args, None);
        assert!(!o.status.success(), "{args:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&o.stderr).unwrap()["code"],
            code
        );
    }
    fn write(&self, path: &str, value: &Value) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
    fn read(&self, path: &str) -> Value {
        serde_json::from_slice(&fs::read(self.0.join(path)).unwrap()).unwrap()
    }
    fn status(&self, client: &str) -> Value {
        self.ok(&["client", "status", "--client", client])["adapters"][0].clone()
    }
    fn gates(&self, write: &str, completion: bool) {
        fs::write(self.0.join(".awr/gates.toml"), format!("version = 1\n[hooks]\nwrite_gate = \"{write}\"\ncompletion_reminder = {completion}\n")).unwrap();
    }
    fn hook(&self, client: &str, event: &str) -> Value {
        decode(self.run(&["client", "hook", "--client", client, "--adapter-version", "1"],
            Some(json!({"session_id":"simulated-adapter-event","hook_event_name":event,"cwd":self.0,"source":"startup"}))))
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn decode(o: Output) -> Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}
fn commands(config: &Value, event: &str) -> Vec<String> {
    config["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
        .filter_map(|hook| hook["command"].as_str().map(str::to_owned))
        .collect()
}
fn has(report: &Value, code: &str) -> bool {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["code"] == code)
}
fn legacy() -> Value {
    json!({"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"awr --project '/example' client hook --client codex --work 'OLD'"}]}]}})
}

#[test]
fn criterion_1_preview_accept_and_reinstall_are_project_scoped_and_idempotent() {
    let p = Project::new();
    let preview = p.ok(&["client", "install", "--client", "claude"]);
    assert!(!p.0.join(".claude").exists());
    assert_eq!(preview["installed"], false);
    let fingerprint = preview["preview_fingerprint"].as_str().unwrap();
    assert_eq!(fingerprint.len(), 64);
    let installed = p.ok(&[
        "client",
        "install",
        "--client",
        "claude",
        "--accept",
        "--expected-preview",
        fingerprint,
    ]);
    assert_eq!(installed["installed"], true);
    let config = p.read(".claude/settings.local.json");
    for event in ["SessionStart", "PreCompact", "Stop", "SessionEnd"] {
        let commands = commands(&config, event);
        assert_eq!(commands.len(), 1);
        assert!(commands[0].contains("--adapter-version 1"));
        assert!(!commands[0].contains("--work"));
    }
    assert!(config["hooks"].get("PreToolUse").is_none());
    assert!(commands(&config, "Stop")[0].contains(env!("CARGO_BIN_EXE_awr")));
    p.ok(&["client", "install", "--client", "claude", "--accept"]);
    assert_eq!(p.read(".claude/settings.local.json"), config);
    assert!(!p.0.join(".claude/settings.json").exists());
}

#[test]
fn criterion_1_stale_install_preview_is_rejected_without_overwriting_user_edits() {
    let p = Project::new();
    let preview = p.ok(&["client", "install", "--client", "claude"]);
    let changed = json!({"permissions":{"allow":["Read"]}});
    p.write(".claude/settings.local.json", &changed);
    p.error(
        &[
            "client",
            "install",
            "--client",
            "claude",
            "--accept",
            "--expected-preview",
            preview["preview_fingerprint"].as_str().unwrap(),
        ],
        "SourceConflict",
    );
    assert_eq!(p.read(".claude/settings.local.json"), changed);
}

#[test]
fn criterion_1_uninstall_preview_is_checked_and_accept_without_fingerprint_remains_valid() {
    let p = Project::new();
    p.ok(&["client", "install", "--client", "claude", "--accept"]);
    let original = p.read(".claude/settings.local.json");
    let preview = p.ok(&["client", "uninstall", "--client", "claude"]);
    assert_eq!(p.read(".claude/settings.local.json"), original);
    let mut changed = original.clone();
    changed["userSetting"] = json!(true);
    p.write(".claude/settings.local.json", &changed);
    p.error(
        &[
            "client",
            "uninstall",
            "--client",
            "claude",
            "--accept",
            "--expected-preview",
            preview["preview_fingerprint"].as_str().unwrap(),
        ],
        "SourceConflict",
    );
    assert_eq!(p.read(".claude/settings.local.json"), changed);
    let preview = p.ok(&["client", "uninstall", "--client", "claude"]);
    p.ok(&[
        "client",
        "uninstall",
        "--client",
        "claude",
        "--accept",
        "--expected-preview",
        preview["preview_fingerprint"].as_str().unwrap(),
    ]);
    assert_eq!(
        p.read(".claude/settings.local.json"),
        json!({"userSetting":true})
    );
}

#[test]
fn criterion_2_install_and_uninstall_preserve_parsed_user_configuration() {
    let p = Project::new();
    let original = json!({"permissions":{"allow":["Read"]},"preferences":{"theme":"dark"},"hooks":{"Stop":[{"matcher":"custom","hooks":[{"type":"command","command":"user-hook","timeout":7}]}],"CustomEvent":[{"hooks":[{"type":"prompt","prompt":"Review the result"}]}],"EmptyUserEvent":[]}});
    p.write(".claude/settings.local.json", &original);
    p.ok(&["client", "install", "--client", "claude", "--accept"]);
    let installed = p.read(".claude/settings.local.json");
    assert_eq!(installed["permissions"], original["permissions"]);
    assert_eq!(installed["hooks"]["Stop"][0], original["hooks"]["Stop"][0]);
    p.ok(&["client", "uninstall", "--client", "claude", "--accept"]);
    assert_eq!(p.read(".claude/settings.local.json"), original);
}

#[test]
fn criterion_2_uninstall_keeps_user_hooks_inside_mixed_groups_and_command_lookalikes() {
    let p = Project::new();
    p.ok(&["client", "install", "--client", "claude", "--accept"]);
    let mut config = p.read(".claude/settings.local.json");
    let user_hook = json!({"type":"command","command":"echo 'client hook --client claude --adapter-version 1'"});
    config["hooks"]["Stop"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(user_hook.clone());
    let other_host = json!({"hooks":[{"type":"command","command":"awr client hook --client codex --adapter-version 1"}]});
    config["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(other_host.clone());
    p.write(".claude/settings.local.json", &config);
    p.ok(&["client", "uninstall", "--client", "claude", "--accept"]);
    assert_eq!(
        p.read(".claude/settings.local.json"),
        json!({"hooks":{"Stop":[{"hooks":[user_hook]},other_host]}})
    );
}

#[test]
fn criterion_3_status_reports_installation_version_events_and_observed_time() {
    let p = Project::new();
    p.ok(&["client", "install", "--client", "claude", "--accept"]);
    let status = p.status("claude");
    assert_eq!(status["installed"], true);
    assert_eq!(status["adapter_version"], 1);
    assert_eq!(status["program_exists"], true);
    assert_eq!(
        status["events"],
        json!(["PreCompact", "SessionEnd", "SessionStart", "Stop"])
    );
    assert_eq!(status["activation"], "not observed");
    let before = awr_core::now_millis().unwrap();
    assert!(
        p.hook("claude", "SessionStart")["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("client bind")
    );
    let status = p.status("claude");
    assert_eq!(status["activation"], "observed");
    assert!(status["observed_at"].as_i64().unwrap() >= before);
    assert_eq!(status["observed_adapter_version"], 1);
    assert_eq!(p.status("codex")["activation"], "not observed");
}

#[test]
fn criterion_3_absent_or_unrelated_configuration_is_not_installed() {
    let p = Project::new();
    assert_eq!(p.status("claude")["installed"], false);
    p.write(
        ".claude/settings.local.json",
        &json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"user-hook"}]}]}}),
    );
    let status = p.status("claude");
    assert_eq!(status["installed"], false);
    assert_eq!(status["activation"], "not observed");
    assert!(status["adapter_version"].is_null());
}

#[test]
fn criterion_4_legacy_codex_is_outdated_and_replaced_by_project_wide_commands() {
    let p = Project::new();
    p.write(".codex/hooks.json", &legacy());
    assert_eq!(p.status("codex")["outdated"], true);
    let result = p.ok(&[
        "client",
        "install",
        "--client",
        "codex",
        "--work",
        "IGNORED-KEY",
        "--accept",
    ]);
    assert_eq!(result["work_ignored"], true);
    let status = p.status("codex");
    assert_eq!(status["outdated"], false);
    assert_eq!(status["adapter_version"], 1);
    assert!(!p.read(".codex/hooks.json").to_string().contains("--work"));
}

#[test]
fn criterion_4_old_and_new_codex_entries_do_not_coexist_after_reinstall() {
    let p = Project::new();
    p.ok(&["client", "install", "--client", "codex", "--accept"]);
    let mut config = p.read(".codex/hooks.json");
    config["hooks"]["SessionStart"]
        .as_array_mut()
        .unwrap()
        .push(legacy()["hooks"]["SessionStart"][0].clone());
    p.write(".codex/hooks.json", &config);
    p.ok(&["client", "install", "--client", "codex", "--accept"]);
    let commands = commands(&p.read(".codex/hooks.json"), "SessionStart");
    assert_eq!(commands.len(), 1);
    assert!(!commands[0].contains("--work"));
}

#[test]
fn criterion_5_doctor_reports_missing_outdated_and_inactive_adapters_as_advisory() {
    let p = Project::new();
    p.gates("ask", false);
    let missing = p.ok(&["doctor"]);
    assert!(has(&missing, "host_adapter_missing"));
    p.write(".codex/hooks.json", &legacy());
    let outdated = p.ok(&["doctor"]);
    assert!(has(&outdated, "host_adapter_outdated"));
    assert!(has(&outdated, "host_adapter_inactive"));
    assert!(!has(&outdated, "host_adapter_missing"));
    p.ok(&["client", "install", "--client", "codex", "--accept"]);
    p.hook("codex", "SessionStart");
    let healthy = p.ok(&["doctor"]);
    assert!(!has(&healthy, "host_adapter_inactive"));
    assert!(!has(&healthy, "host_adapter_outdated"));
}

#[test]
fn criterion_5_doctor_does_not_report_missing_when_hook_rules_are_off() {
    let p = Project::new();
    p.gates("off", false);
    assert!(!has(&p.ok(&["doctor"]), "host_adapter_missing"));
    p.gates("off", true);
    assert!(has(&p.ok(&["doctor"]), "host_adapter_missing"));
}

#[test]
fn shared_claude_settings_use_portable_project_and_program_and_track_write_gate_changes() {
    let p = Project::new();
    p.gates("ask", false);
    p.ok(&[
        "client", "install", "--client", "claude", "--shared", "--accept",
    ]);
    assert!(!p.0.join(".claude/settings.local.json").exists());
    let config = p.read(".claude/settings.json");
    let command = &commands(&config, "SessionStart")[0];
    assert!(command.contains("\"$CLAUDE_PROJECT_DIR\""));
    assert!(!command.contains(p.0.to_str().unwrap()));
    assert!(!command.contains(env!("CARGO_BIN_EXE_awr")));
    assert_eq!(
        config["hooks"]["PreToolUse"][0]["matcher"],
        "Edit|Write|MultiEdit|NotebookEdit"
    );
    assert_eq!(p.status("claude")["gates_consistent"], true);
    p.gates("off", false);
    assert_eq!(p.status("claude")["gates_consistent"], false);
    p.ok(&[
        "client", "install", "--client", "claude", "--shared", "--accept",
    ]);
    assert!(
        p.read(".claude/settings.json")["hooks"]
            .get("PreToolUse")
            .is_none()
    );
    p.ok(&["client", "uninstall", "--client", "claude", "--accept"]);
    assert_eq!(p.read(".claude/settings.json"), json!({}));
}

#[test]
fn invalid_configuration_and_unsupported_clients_are_preserved_and_rejected() {
    let p = Project::new();
    for client in ["generic", "kimi", "unknown"] {
        p.error(
            &["client", "install", "--client", client, "--accept"],
            "Unsupported",
        );
    }
    p.write(
        ".claude/settings.local.json",
        &json!({"hooks":{"Stop":"invalid"}}),
    );
    let original = fs::read(p.0.join(".claude/settings.local.json")).unwrap();
    p.error(
        &["client", "install", "--client", "claude", "--accept"],
        "InvalidInput",
    );
    assert_eq!(
        fs::read(p.0.join(".claude/settings.local.json")).unwrap(),
        original
    );
}

#[cfg(unix)]
#[test]
fn project_setting_directory_symlinks_cannot_redirect_writes_outside_the_project() {
    let p = Project::new();
    let other = Project::new();
    std::os::unix::fs::symlink(&other.0, p.0.join(".claude")).unwrap();
    let output = p.run(
        &["client", "install", "--client", "claude", "--accept"],
        None,
    );
    assert!(!output.status.success());
    assert!(!other.0.join("settings.local.json").exists());
}

#[test]
fn missing_program_is_visible_and_uninstalling_an_absent_adapter_creates_no_settings() {
    let p = Project::new();
    p.ok(&["client", "uninstall", "--client", "claude", "--accept"]);
    assert!(!p.0.join(".claude").exists());
    let missing = p.0.join("missing/awr");
    p.write(".claude/settings.local.json", &json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":format!("'{}' client hook --client claude --adapter-version 1",missing.display())}]}]}}));
    assert_eq!(p.status("claude")["installed"], true);
    assert_eq!(p.status("claude")["program_exists"], false);
    assert_eq!(p.status("claude")["activation"], "not observed");
}

#[test]
fn criterion_6_documentation_keeps_native_verification_pending() {
    let page = include_str!("../../../docs/dev/integrations/claude-code.md");
    assert!(page.contains("simulated events verified"));
    assert!(page.contains("unexecuted, awaiting approval"));
    assert!(!page.contains("native verification passed"));
}

#[cfg(unix)]
#[test]
fn generated_local_and_shared_claude_commands_handle_simulated_lifecycle_and_write_events() {
    for shared in [false, true] {
        let p = Project::new();
        p.gates("ask", false);
        let mut args = vec!["client", "install", "--client", "claude", "--accept"];
        if shared {
            args.push("--shared");
        }
        p.ok(&args);
        let config = p.read(if shared {
            ".claude/settings.json"
        } else {
            ".claude/settings.local.json"
        });
        for (event, source) in [
            ("SessionStart", "startup"),
            ("SessionStart", "compact"),
            ("PreCompact", "startup"),
            ("Stop", "startup"),
            ("SessionEnd", "startup"),
            ("PreToolUse", "startup"),
        ] {
            let command = &commands(&config, event)[0];
            let mut paths = vec![
                std::path::Path::new(env!("CARGO_BIN_EXE_awr"))
                    .parent()
                    .unwrap()
                    .to_owned(),
            ];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            let mut child = Command::new("sh")
                .args(["-c", command])
                .env("AWR_LOCAL_WEB", "0")
                .env("CLAUDE_PROJECT_DIR", &p.0)
                .env("PATH", std::env::join_paths(paths).unwrap())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(&serde_json::to_vec(&json!({"session_id":"simulated-generated-command","cwd":p.0,"hook_event_name":event,"source":source,"tool_name":"Write","tool_input":{"file_path":p.0.join("new.rs")}})).unwrap()).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{event}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if event == "SessionStart" {
                let value: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert!(
                    value["hookSpecificOutput"]["additionalContext"]
                        .as_str()
                        .unwrap()
                        .contains("client bind")
                );
                assert!(value.get("awr").is_none());
            } else if event == "PreToolUse" {
                let value: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(value["hookSpecificOutput"]["permissionDecision"], "ask");
                assert!(value.get("awr").is_none());
            } else {
                assert!(output.stdout.is_empty());
            }
        }
        assert_eq!(p.status("claude")["activation"], "observed");
    }
}

#[test]
fn criterion_1_preview_conflicts_when_external_edit_is_invalid_json() {
    let p = Project::new();
    let preview = p.ok(&["client", "install", "--client", "claude"]);
    fs::create_dir(p.0.join(".claude")).unwrap();
    fs::write(p.0.join(".claude/settings.local.json"), b"{broken").unwrap();
    p.error(
        &[
            "client",
            "install",
            "--client",
            "claude",
            "--accept",
            "--expected-preview",
            preview["preview_fingerprint"].as_str().unwrap(),
        ],
        "SourceConflict",
    );
    assert_eq!(
        fs::read(p.0.join(".claude/settings.local.json")).unwrap(),
        b"{broken"
    );
}

#[test]
fn criterion_1_supplied_fingerprint_is_checked_even_on_a_new_preview() {
    let p = Project::new();
    let preview = p.ok(&["client", "install", "--client", "claude"]);
    p.write(
        ".claude/settings.local.json",
        &json!({"preferences":{"theme":"dark"}}),
    );
    p.error(
        &[
            "client",
            "install",
            "--client",
            "claude",
            "--expected-preview",
            preview["preview_fingerprint"].as_str().unwrap(),
        ],
        "SourceConflict",
    );
}
