use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HELP: &[u8] = include_bytes!("fixtures/help/help.txt");
const HELP_FLAG: &[u8] = include_bytes!("fixtures/help/help-flag.txt");
const WORK_HELP: &[u8] = include_bytes!("fixtures/help/work-help.txt");
const CURRENT_WORK_HELP: &[u8] = include_bytes!("fixtures/help/work-help-current.txt");
const HELP_HELP: &[u8] = include_bytes!("fixtures/help/help-help.txt");
const WORK_HELP_HELP: &[u8] = include_bytes!("fixtures/help/work-help-help.txt");
const STATUS_HELP: &[u8] = include_bytes!("fixtures/help/status-help.txt");
const COMPLETE_HELP: &[u8] = include_bytes!("fixtures/help/work-complete-help.txt");

struct BinaryDirectory(PathBuf);

impl Drop for BinaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    run_binary(Path::new(env!("CARGO_BIN_EXE_awr")), args)
}

fn run_binary(binary: &Path, args: &[&str]) -> Output {
    Command::new(binary)
        .env("AWR_LOCAL_WEB", "0")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .args(args)
        .output()
        .unwrap()
}

fn baseline_for_program(baseline: &[u8], program: &str) -> Vec<u8> {
    // Only the expected invocation name varies; compare actual output byte-for-byte.
    String::from_utf8(baseline.to_vec())
        .unwrap()
        .replacen("Usage: awr ", &format!("Usage: {program} "), 1)
        .into_bytes()
}

fn native_baseline(baseline: &[u8]) -> Vec<u8> {
    baseline_for_program(baseline, &format!("awr{}", std::env::consts::EXE_SUFFIX))
}

fn help(args: &[&str]) -> Vec<u8> {
    let output = run(args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    output.stdout
}

#[test]
fn ac1_default_overviews_are_short_and_point_to_all_commands() {
    for (args, limit, hint) in [
        (vec!["--help"], 1536, "awr help all"),
        (vec!["work", "--help"], 1280, "awr work help all"),
    ] {
        let output = help(&args);
        assert!(output.len() <= limit, "{args:?}: {} bytes", output.len());
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Daily work"), "{text}");
        assert!(text.contains(hint), "{text}");
        if args.first() == Some(&"work") {
            assert!(
                text.contains("create, activate, edit, graph, assess"),
                "{text}"
            );
        }
    }
    assert_eq!(help(&[]), help(&["--help"]));
    assert_eq!(help(&["help"]), help(&["--help"]));
    assert_eq!(help(&["-h"]), help(&["--help"]));
    assert_eq!(help(&["work", "help"]), help(&["work", "--help"]));
}

#[test]
fn next_step_ac5_governance_help_exposes_definition_without_growing_overviews() {
    let text = String::from_utf8(help(&["work", "help", "governance"])).unwrap();
    for field in ["definition:", "examples:", "check:"] {
        assert!(text.contains(field), "missing {field}: {text}");
    }
    assert_eq!(help(&["help", "work", "governance"]), text.as_bytes());
    for (args, baseline) in [(vec!["--help"], 951), (vec!["work", "--help"], 945)] {
        let output = help(&args);
        assert!(
            output.len() * 100 <= baseline * 105,
            "{args:?}: {} bytes",
            output.len()
        );
    }
}

#[test]
fn next_step_ac5_definition_example_stays_out_of_first_layer_help() {
    for args in [vec!["--help"], vec!["work", "--help"]] {
        let output = help(&args);
        assert!(output.len() <= 1536);
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains("```yaml"), "{text}");
        assert!(!text.contains("definition:"), "{text}");
    }
}

#[test]
fn gap_hints_ac4_governance_help_is_bounded_and_overviews_keep_their_bytes() {
    let output = help(&["work", "help", "governance"]);
    assert!(output.len() <= 3072, "{} bytes", output.len());
    for (args, baseline) in [(vec!["--help"], 951), (vec!["work", "--help"], 945)] {
        assert_eq!(help(&args).len(), baseline, "{args:?}");
    }
}

#[test]
fn gap_hints_ac4_inlining_the_reference_exceeds_the_receipt_budget() {
    let text = String::from_utf8(help(&["work", "help", "governance"])).unwrap();
    let reference = text
        .split_once("Gap codes | ")
        .expect("gap field reference")
        .1;
    assert!(
        reference.len() > 1536,
        "the full reference belongs in help only"
    );
    for args in [vec!["--help"], vec!["work", "--help"]] {
        let overview = String::from_utf8(help(&args)).unwrap();
        assert!(!overview.contains("intent_missing") && !overview.contains("Gap codes | "));
    }
}

#[test]
fn work_check_help_joins_completion_in_daily_and_preserves_detailed_help() {
    let daily = String::from_utf8(help(&["work", "help", "daily"])).unwrap();
    let members = daily
        .lines()
        .filter(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .collect::<Vec<_>>();
    let check = members.iter().position(|name| *name == "check").unwrap();
    assert_eq!(members[check + 1], "prepare-completion");
    assert_eq!(members[check + 2], "complete");
    let description = daily
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("check "))
        .unwrap()
        .trim();
    assert!(description.chars().count() <= 60);
    for group in ["planning", "governance", "lifecycle", "recovery"] {
        let text = String::from_utf8(help(&["work", "help", group])).unwrap();
        assert!(
            !text
                .lines()
                .any(|line| line.split_whitespace().next() == Some("check")),
            "check also appears in {group}: {text}"
        );
    }
    let all = String::from_utf8(help(&["work", "help", "all"])).unwrap();
    assert!(visible_command_names(&all).contains(&"check"));
    assert_eq!(
        help(&["work", "help", "check"]),
        help(&["work", "check", "--help"])
    );
}

#[test]
fn ac2_full_lists_match_the_baseline_line_for_line() {
    assert_eq!(HELP, HELP_FLAG);
    assert_eq!(help(&["help", "all"]), native_baseline(HELP));
    assert_eq!(
        help(&["work", "help", "all"]),
        native_baseline(CURRENT_WORK_HELP)
    );
}

#[test]
fn ac2_full_list_comparison_detects_an_omitted_command() {
    let output = help(&["help", "all"]);
    let baseline = native_baseline(HELP);
    assert_eq!(output, baseline);
    let text = String::from_utf8(output).unwrap();
    let omitted = text
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("  status "))
        .collect::<String>();
    assert_ne!(omitted.as_bytes(), baseline);
    assert_eq!(text.lines().count(), omitted.lines().count() + 1);
}

#[test]
fn ac3_leaf_help_matches_the_baseline_byte_for_byte() {
    for (args, baseline) in [
        (vec!["help", "work"], CURRENT_WORK_HELP),
        (vec!["status", "--help"], STATUS_HELP),
        (vec!["work", "complete", "--help"], COMPLETE_HELP),
        (vec!["help", "status"], STATUS_HELP),
        (vec!["help", "work", "complete"], COMPLETE_HELP),
        (vec!["work", "help", "complete"], COMPLETE_HELP),
    ] {
        assert_eq!(help(&args), native_baseline(baseline), "{args:?}");
    }
}

#[test]
fn ac3_all_visible_command_help_paths_match_detailed_help() {
    let mut differences = Vec::new();
    for scope in [vec![], vec!["work"]] {
        let mut full_args = scope.clone();
        full_args.extend(["help", "all"]);
        let full = String::from_utf8(help(&full_args)).unwrap();
        let names = visible_command_names(&full);
        assert!(!names.is_empty(), "{full}");
        let baseline =
            std::str::from_utf8(if scope.is_empty() { HELP } else { WORK_HELP }).unwrap();
        for name in visible_command_names(baseline) {
            assert!(
                names.contains(&name),
                "missing baseline command {name}: {full}"
            );
        }
        for name in names {
            let mut help_args = scope.clone();
            help_args.extend(["help", name]);
            let expected = if scope.is_empty() && name == "work" {
                // Work topics retain the full format while allowing newly added commands.
                help(&["work", "help", "all"])
            } else if name == "help" {
                // The original built-in help topic has no equivalent --help form.
                native_baseline(if scope.is_empty() {
                    HELP_HELP
                } else {
                    WORK_HELP_HELP
                })
            } else {
                let mut flag_args = scope.clone();
                flag_args.extend([name, "--help"]);
                help(&flag_args)
            };
            for args in std::iter::once(help_args)
                .chain((!scope.is_empty()).then(|| vec!["help", "work", name]))
            {
                let output = run(&args);
                if !output.status.success()
                    || !output.stderr.is_empty()
                    || output.stdout != expected
                {
                    differences.push(format!(
                        "awr {} changed detailed help (exit {:?})",
                        args.join(" "),
                        output.status.code()
                    ));
                }
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

fn visible_command_names(full: &str) -> Vec<&str> {
    full.split_once("Commands:\n")
        .unwrap()
        .1
        .split_once("\nOptions:")
        .unwrap()
        .0
        .lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .filter_map(|line| line.split_whitespace().next())
        .collect()
}

#[test]
fn ac3_leaf_help_comparison_detects_a_single_byte_change() {
    for (args, baseline) in [
        (vec!["status", "--help"], STATUS_HELP),
        (vec!["work", "complete", "--help"], COMPLETE_HELP),
    ] {
        let mut output = help(&args);
        let baseline = native_baseline(baseline);
        assert_eq!(output, baseline, "{args:?}");
        output[0] ^= 1;
        assert_ne!(output, baseline, "{args:?}");
    }
}

#[test]
fn help_fixtures_preserve_the_invoked_executable_name() {
    let directory = BinaryDirectory(
        std::env::temp_dir().join(format!("awr-help-executable-{}", awr_core::Id::new())),
    );
    std::fs::create_dir(&directory.0).unwrap();
    let binary = directory.0.join("awr.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_awr"), &binary).unwrap();
    for (args, baseline) in [
        (vec!["help", "all"], HELP),
        (vec!["work", "help", "all"], CURRENT_WORK_HELP),
        (vec!["help", "help"], HELP_HELP),
        (vec!["work", "help", "help"], WORK_HELP_HELP),
        (vec!["help", "work"], CURRENT_WORK_HELP),
        (vec!["status", "--help"], STATUS_HELP),
        (vec!["work", "complete", "--help"], COMPLETE_HELP),
        (vec!["help", "status"], STATUS_HELP),
        (vec!["help", "work", "complete"], COMPLETE_HELP),
        (vec!["work", "help", "complete"], COMPLETE_HELP),
    ] {
        let output = run_binary(&binary, &args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        assert_eq!(
            output.stdout,
            baseline_for_program(baseline, "awr.exe"),
            "{args:?}"
        );
    }
}

#[test]
fn ac4_planning_groups_list_members_and_short_descriptions() {
    for (args, members) in [
        (
            vec!["help", "planning"],
            vec![
                "init",
                "intake",
                "source",
                "batch",
                "host",
                "document",
                "proposal",
                "organization",
            ],
        ),
        (
            vec!["work", "help", "planning"],
            vec!["create", "activate", "edit", "graph", "assess"],
        ),
    ] {
        let text = String::from_utf8(help(&args)).unwrap();
        assert!(text.contains("Planning"), "{text}");
        for member in members {
            let line = text
                .lines()
                .find(|line| {
                    line.starts_with("  ") && line.split_whitespace().next() == Some(member)
                })
                .unwrap_or_else(|| panic!("missing {member}: {text}"));
            let description = line.trim_start().strip_prefix(member).unwrap().trim();
            assert!(!description.is_empty(), "{line}");
            assert!(description.chars().count() <= 60, "{line}");
        }
        assert!(
            !text
                .lines()
                .any(|line| line.split_whitespace().next() == Some("status")),
            "{text}"
        );
    }
}

#[test]
fn ac4_unknown_groups_exit_two_and_list_available_groups() {
    for (args, groups) in [
        (
            vec!["help", "nosuch"],
            vec![
                "daily",
                "planning",
                "records",
                "hosts",
                "environments",
                "collaboration",
                "all",
            ],
        ),
        (
            vec!["work", "help", "nosuch"],
            vec![
                "daily",
                "planning",
                "governance",
                "lifecycle",
                "recovery",
                "all",
            ],
        ),
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("Available groups:"), "{error}");
        for group in groups {
            assert!(error.contains(group), "{error}");
        }
    }
}

#[test]
fn every_advertised_group_is_available_with_global_options() {
    for (prefix, groups) in [
        (
            vec![],
            vec![
                "daily",
                "planning",
                "records",
                "hosts",
                "environments",
                "collaboration",
            ],
        ),
        (
            vec!["work"],
            vec!["daily", "planning", "governance", "lifecycle", "recovery"],
        ),
    ] {
        for group in groups {
            let mut args = prefix.clone();
            args.extend(["help", group]);
            let expected = help(&args);
            assert!(!expected.is_empty());
            args.extend(["--json", "--project", "a-project-that-does-not-exist"]);
            assert_eq!(help(&args), expected, "{args:?}");
        }
    }
}

#[test]
fn ac5_usage_errors_remain_invalid_input_json() {
    for args in [
        vec!["--json", "work", "show"],
        vec!["ready", "--limit", "invalid", "--json"],
        vec!["--json", "work", "ready"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "InvalidInput");
        assert!(!error["message"].as_str().unwrap().is_empty());
    }
}

#[test]
fn ac5_unknown_group_json_errors_never_fall_back_to_plain_text() {
    for args in [
        vec!["--json", "help", "nosuch"],
        vec!["help", "nosuch", "--json"],
        vec!["work", "help", "nosuch", "--json"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr)
            .unwrap_or_else(|_| panic!("JSON usage error became plain text: {output:?}"));
        assert_eq!(error["code"], "InvalidInput");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("Available groups:")
        );
    }
}
