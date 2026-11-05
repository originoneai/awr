use serde_json::Value;
use std::process::{Command, Output};

fn refusal(args: &[&str], index: usize, category: &str, private_fragments: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(args)
        .output()
        .unwrap();
    refusal_output(output, index, category, private_fragments)
}

fn refusal_output(
    output: Output,
    index: usize,
    category: &str,
    private_fragments: &[&str],
) -> Value {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    for fragment in private_fragments {
        assert!(
            !stderr.contains(fragment),
            "argument contents must not be echoed"
        );
    }
    let report: Value =
        serde_json::from_str(&stderr).expect("preflight refusal is one JSON report");
    assert_eq!(report["code"], "RuleViolation");
    assert_eq!(
        report["message"],
        "rule violation: sensitive command arguments are not accepted"
    );
    let details = &report["details"];
    assert_eq!(details["category"], category);
    assert_eq!(details["policy_version"], awr_core::SECRET_POLICY_VERSION);
    assert_eq!(details["argument_index"], index);
    assert_eq!(details["argument_index_basis"], "zero_based_process_argv");
    assert!(!details["next_action"].as_str().unwrap().is_empty());
    let mut keys = details
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "argument_index",
            "argument_index_basis",
            "category",
            "next_action",
            "policy_version"
        ]
    );
    report
}

#[test]
fn standalone_environment_argument_after_separator_has_safe_actionable_diagnostics() {
    for prefix in [vec![], vec!["--json"]] {
        let mut args = prefix;
        args.extend([
            "execution",
            "run",
            "--",
            "env",
            "PATH=/synthetic/toolchain/bin",
            "public-build",
        ]);
        let index = args
            .iter()
            .position(|arg| arg.starts_with("PATH="))
            .unwrap()
            + 1;
        let report = refusal(
            &args,
            index,
            "environment_dump",
            &["PATH", "/synthetic/toolchain/bin"],
        );
        let guidance = report["details"]["next_action"].as_str().unwrap();
        assert!(guidance.contains("before invoking awr"));
        assert!(guidance.contains("supervisor and managed child inherit"));
        assert!(guidance.contains("outside AWR-managed data"));
    }
}

#[test]
fn malformed_arguments_cannot_bypass_preflight_or_echo_sensitive_content() {
    for (input, category, fragments) in [
        ("Basic YTpi", "credential", vec!["YTpi"]),
        (
            "SYNTHETIC_PASSWORD=fixture-labelled-value",
            "labelled_value",
            vec!["SYNTHETIC_PASSWORD", "fixture-labelled-value"],
        ),
        (
            "# Private prompt\nfixture-private-instructions",
            "private_prompt",
            vec!["# Private prompt", "fixture-private-instructions"],
        ),
        (
            "SYNTHETIC_HOME=/synthetic/home\nSYNTHETIC_BIN=/synthetic/bin",
            "environment_dump",
            vec![
                "SYNTHETIC_HOME",
                "SYNTHETIC_BIN",
                "/synthetic/home",
                "/synthetic/bin",
            ],
        ),
    ] {
        // Unknown options normally trigger parser diagnostics, including argument text.
        refusal(&["--unknown-option", input], 2, category, &fragments);
        refusal(
            &["--json", "--unknown-option", input],
            3,
            category,
            &fragments,
        );
    }
}

#[test]
fn issue_515_reporter_command_controls_keep_their_admission_behavior() {
    enum Expected {
        Refusal(usize),
        Usage,
        Help,
        InheritedHelp,
    }
    let cases: &[(&[&str], Expected)] = &[
        (
            &[
                "execution",
                "run",
                "--",
                "env",
                "PATH=/synthetic/bin",
                "true",
            ],
            Expected::Refusal(5),
        ),
        (&["execution", "run", "--", "true"], Expected::Usage),
        (
            &["--help", "--", "PATH=/synthetic/bin"],
            Expected::Refusal(3),
        ),
        (
            &[
                "--help",
                "--",
                "env",
                "PATH=/synthetic/bin",
                "pnpm",
                "--config.ignore-scripts=true",
                "pack",
            ],
            Expected::Refusal(4),
        ),
        (
            &["--help", "--", "PATH=/synthetic/bin pnpm pack"],
            Expected::Help,
        ),
        (
            &["--help", "--", "--config.ignore-scripts=true"],
            Expected::Help,
        ),
        (&["--help"], Expected::InheritedHelp),
    ];
    for (args, expected) in cases {
        let mut command = Command::new(env!("CARGO_BIN_EXE_awr"));
        command.args(*args);
        if matches!(expected, Expected::InheritedHelp) {
            let original = std::env::var_os("PATH").unwrap_or_default();
            let path = std::env::join_paths(
                std::iter::once(std::path::PathBuf::from("/synthetic/bin"))
                    .chain(std::env::split_paths(&original)),
            )
            .unwrap();
            command.env("PATH", path);
        }
        let output = command.output().unwrap();
        match expected {
            Expected::Refusal(index) => {
                refusal_output(
                    output,
                    *index,
                    "environment_dump",
                    &["PATH", "/synthetic/bin"],
                );
            }
            Expected::Usage => {
                assert_eq!(output.status.code(), Some(2), "{args:?}");
                assert!(output.stdout.is_empty(), "{args:?}");
                let stderr = String::from_utf8(output.stderr).unwrap();
                for option in ["--session", "--key", "--purpose"] {
                    assert!(stderr.contains(option), "{args:?}: {stderr}");
                }
                assert!(!stderr.contains("RuleViolation"), "{args:?}: {stderr}");
            }
            Expected::Help | Expected::InheritedHelp => {
                assert_eq!(output.status.code(), Some(0), "{args:?}");
                assert!(output.stderr.is_empty(), "{args:?}");
                let stdout = String::from_utf8(output.stdout).unwrap();
                assert!(stdout.contains("Usage:"), "{args:?}: {stdout}");
            }
        }
    }
}

#[test]
fn first_sensitive_argument_is_reported_in_process_argv_order() {
    refusal(
        &["--help", "PATH=/synthetic/first", "Basic YTpi"],
        2,
        "environment_dump",
        &["PATH", "/synthetic/first", "YTpi"],
    );
}

#[cfg(unix)]
#[test]
fn executable_argument_remains_checked_at_index_zero() {
    use std::os::unix::process::CommandExt;
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .arg0("PATH=/synthetic/executable")
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("PATH"));
    assert!(!stderr.contains("/synthetic/executable"));
    let report: Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(report["details"]["argument_index"], 0);
    assert_eq!(
        report["details"]["argument_index_basis"],
        "zero_based_process_argv"
    );
    assert_eq!(report["details"]["category"], "environment_dump");
}

#[test]
fn public_arguments_keep_existing_admission_and_execution_help_describes_environment() {
    for input in [
        "--config.ignore-scripts=true",
        "PROJECT_ROOT=/public/project cargo test --offline",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--help", input])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
    }
    let help = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args(["execution", "run", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("before invoking awr"));
    assert!(help.contains("inherit it"));
}
