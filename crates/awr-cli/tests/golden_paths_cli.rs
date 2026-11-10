//! Call and UTF-8 text budgets for the native CLI. Setup and final assertions are
//! outside each measured path; every invocation inside it contributes both streams.
#[path = "support/checked_work.rs"]
mod checked_work;

use checked_work::{Call, Flow, Project, checked_flow};
use serde_json::{Value, json};

// Budget changes require an explanation in the PR. These are limits, not observations.
const P1_CALLS: usize = 4;
const P1_TEXT_BYTES: usize = 6 * 1024;
const P2_AUTHOR_CALLS: usize = 6;
const P2_AUTHOR_TEXT_BYTES: usize = 8 * 1024;
const P2_REVIEWER_CALLS: usize = 2;
const P3_MODEL_CALLS: usize = 0;
const P4_RECOVERY_CALLS: usize = 1;

fn idea_path(start_progress: bool) -> (Project, Flow) {
    let p = Project::new(false);
    let input = json!({"version":1,"request_key":"new-draft","title":"Retain another draft",
        "fields":{"goal":["G"],"acceptance":["The second draft is retained"],"next_action":"Implement retention"}});
    p.write_json(".awr/create.json", &input);
    let mut flow = Flow::default();
    let preview = p.call(
        &mut flow,
        "work create text preview",
        &[
            "work",
            "create",
            "--input",
            ".awr/create.json",
            "--activate",
        ],
        false,
    );
    let key = preview
        .lines()
        .find_map(|line| line.strip_prefix("Work key: "))
        .unwrap();
    let next = preview
        .lines()
        .find_map(|line| line.strip_prefix("Next: "))
        .unwrap();
    // The relative literal input path needs no shell quoting on any platform.
    let accept: Vec<_> = next.split_whitespace().collect();
    assert_eq!(accept[0], "awr");
    assert!(accept.contains(&"--accept"));
    assert!(accept.contains(&"--activate"));
    p.call(
        &mut flow,
        "work create accept and activate",
        &accept[1..],
        false,
    );
    let started = p.call(
        &mut flow,
        "session start and claim",
        &[
            "session",
            "start",
            "--work",
            key,
            "--agent",
            "author",
            "--provider",
            "fixture",
            "--model",
            "local",
            "--claim",
            "--ttl-ms",
            "60000",
            "--expected-revision",
            &p.revision(),
        ],
        false,
    );
    let session = started
        .lines()
        .find_map(|line| line.strip_prefix("Session: "))
        .unwrap();
    if start_progress {
        p.call(
            &mut flow,
            "work progress",
            &[
                "work",
                "progress",
                key,
                "--session",
                session,
                "--reason",
                "Start draft retention",
                "--next-action",
                "Implement draft retention",
                "--expected-revision",
                &p.revision(),
            ],
            false,
        );
        assert_eq!(
            p.json(&["work", "show", key])["work"]["status"],
            "in_progress"
        );
    } else {
        let prepared = p.call(
            &mut flow,
            "work prepare",
            &[
                "work",
                "prepare",
                key,
                "--session",
                session,
                "--response-view",
                "action",
            ],
            false,
        );
        assert!(
            prepared.contains("The second draft is retained"),
            "{prepared}"
        );
        assert_ne!(p.json(&["work", "show", key])["work"]["status"], "draft");
    }
    assert!(
        p.json(&["session", "show", session])["claims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|claim| claim["status"] == "active")
    );
    (p, flow)
}

#[test]
fn p1_new_idea_starts_work_in_four_calls_within_six_kib_of_text() {
    let (_p, flow) = idea_path(true);
    assert_eq!(flow.calls[0].name, "work create text preview");
    assert_eq!(flow.calls[3].name, "work progress");
    assert_eq!(flow.calls.len(), P1_CALLS);
    flow.budget("P1", P1_CALLS, P1_TEXT_BYTES).unwrap();
}

#[test]
fn p1_text_preview_prepares_work_in_four_calls_within_six_kib_of_text() {
    let (_p, flow) = idea_path(false);
    assert_eq!(flow.calls.len(), P1_CALLS);
    flow.budget("P1 preparation", P1_CALLS, P1_TEXT_BYTES)
        .unwrap();
}

#[test]
fn p1_an_extra_real_cli_call_fails_the_budget() {
    let (p, mut flow) = idea_path(true);
    p.call(&mut flow, "unnecessary status", &["status"], false);
    let error = flow.budget("P1", P1_CALLS, P1_TEXT_BYTES).unwrap_err();
    assert!(error.contains("5 calls (limit 4)"), "{error}");
    // Isolate the call guard from the output-byte guard.
    assert!(flow.budget("P1 calls", P1_CALLS, usize::MAX).is_err());
}

#[test]
fn p1_fixed_negative_witnesses_reject_relaxed_call_and_byte_limits() {
    let mut at_limit = Flow {
        calls: (0..4)
            .map(|step| Call {
                name: format!("step {step}"),
                bytes: 0,
            })
            .collect(),
    };
    at_limit.calls[0].bytes = 6144;
    at_limit
        .budget("P1 boundary", P1_CALLS, P1_TEXT_BYTES)
        .unwrap();
    let mut extra_call = at_limit.clone();
    extra_call.calls.push(Call {
        name: "extra call".into(),
        bytes: 0,
    });
    assert!(
        extra_call
            .budget("P1 call witness", P1_CALLS, usize::MAX)
            .is_err()
    );
    at_limit.calls[0].bytes += 1;
    assert!(
        at_limit
            .budget("P1 byte witness", P1_CALLS, P1_TEXT_BYTES)
            .is_err()
    );
}

#[test]
fn p2_checked_work_completes_in_six_author_calls_and_two_reviewer_calls() {
    let (_p, author, reviewer, _) = checked_flow(false, true, false);
    assert_eq!(author.calls.len(), P2_AUTHOR_CALLS);
    assert_eq!(reviewer.calls.len(), P2_REVIEWER_CALLS);
    author
        .budget("P2 author", P2_AUTHOR_CALLS, P2_AUTHOR_TEXT_BYTES)
        .unwrap();
    reviewer
        .budget("P2 reviewer", P2_REVIEWER_CALLS, usize::MAX)
        .unwrap();
    assert_eq!(
        &author.calls[4..]
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        &["work check acceptance", "work complete and session end"]
    );
}

#[test]
fn p2_an_extra_real_cli_call_fails_and_reports_every_steps_bytes() {
    let (p, mut author, _, _) = checked_flow(false, true, false);
    p.call(
        &mut author,
        "unnecessary check status",
        &["work", "check", "W", "--status"],
        false,
    );
    let error = author
        .budget("P2 author", P2_AUTHOR_CALLS, P2_AUTHOR_TEXT_BYTES)
        .unwrap_err();
    assert!(error.contains("7 calls (limit 6)"));
    for call in &author.calls {
        assert!(
            error.contains(&format!("{}: {} bytes", call.name, call.bytes)),
            "{error}"
        );
    }
}

#[test]
fn text_byte_limits_reject_oversized_output_at_the_allowed_call_count() {
    for (name, calls, bytes) in [
        ("P1", P1_CALLS, P1_TEXT_BYTES),
        ("P2", P2_AUTHOR_CALLS, P2_AUTHOR_TEXT_BYTES),
    ] {
        let mut flow = Flow {
            calls: (0..calls)
                .map(|i| Call {
                    name: format!("step {i}"),
                    bytes: 0,
                })
                .collect(),
        };
        flow.calls[0].bytes = bytes;
        flow.budget(name, calls, bytes).unwrap();
        flow.calls[0].bytes += 1;
        assert!(flow.budget(name, calls, bytes).is_err());
    }
}

fn compacted_hook() -> (Project, Value, usize) {
    let p = Project::new(false);
    let session = p.start(&mut Flow::default(), "author", true, true);
    p.json(&[
        "client",
        "bind",
        "--client",
        "claude",
        "--external-session",
        "golden-conversation",
        "--work",
        "W",
        "--session",
        &session,
    ]);
    p.json(&[
        "client",
        "progress",
        "--client",
        "claude",
        "--external-session",
        "golden-conversation",
        "--next-action",
        "Inspect the retained draft before editing",
    ]);
    let event = json!({"session_id":"golden-conversation","hook_event_name":"SessionStart","source":"compact","cwd":p.root});
    let output = p.raw(
        &["client", "hook", "--client", "claude"],
        false,
        Some(&event),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = serde_json::from_slice(&output.stdout).unwrap();
    let bytes = output.stdout.len() + output.stderr.len();
    (p, value, bytes)
}

#[test]
fn p3_compacted_session_start_injects_the_next_action_without_model_calls() {
    let (_p, value, bytes) = compacted_hook();
    let context = value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("Inspect the retained draft before editing"),
        "{context}"
    );
    // A successful native hook has already enforced its existing context limit.
    // The protocol response is measured as bytes; provider token usage is absent.
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    Flow::default()
        .budget("P3 model", P3_MODEL_CALLS, 0)
        .unwrap();
    println!("P3: 1 host hook call, {bytes} output bytes, 0 model CLI calls");
}

#[test]
fn p3_a_model_lookup_after_the_hook_fails_the_zero_call_budget() {
    let (p, _, _) = compacted_hook();
    let mut model = Flow::default();
    p.call(&mut model, "unnecessary ready lookup", &["ready"], false);
    assert!(
        model
            .budget("P3 model", P3_MODEL_CALLS, usize::MAX)
            .is_err()
    );
}

// The recovery fixtures use a relative --input path and simple work keys, so their
// suggestions contain no shell quoting. Execute argv directly on every platform.
fn follow(p: &Project, error: &Value, flow: &mut Flow) -> String {
    let action = error["details"]["next_action"].as_str().unwrap();
    let args: Vec<_> = action.split_whitespace().collect();
    assert_eq!(args[0], "awr");
    p.call(flow, "next_action", &args[1..], false)
}

fn recoverable_error(kind: &str) -> (Project, Value) {
    let p = Project::new(false);
    let output = if kind == "budget" {
        p.raw(
            &[
                "context",
                "compile",
                "--work",
                "W",
                "--detached",
                "--budget",
                "1",
            ],
            true,
            None,
        )
    } else {
        p.write_json(".awr/new.json", &json!({"version":1,"request_key":"recover-note","title":"Review a note","fields":{"goal":["G"],"acceptance":["The note is reviewed"]}}));
        let preview = p.json(&["work", "create", "--input", ".awr/new.json"]);
        let revision = preview["project_revision"].to_string();
        p.start(&mut Flow::default(), "observer", true, false);
        p.raw(
            &[
                "work",
                "create",
                "--input",
                ".awr/new.json",
                "--accept",
                "--expected-preview",
                preview["preview"]["fingerprint"].as_str().unwrap(),
                "--expected-revision",
                &revision,
            ],
            true,
            None,
        )
    };
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        error["code"],
        if kind == "budget" {
            "BudgetExceeded"
        } else {
            "RevisionConflict"
        }
    );
    (p, error)
}

#[test]
fn p4_each_automatic_refusal_recovers_by_executing_next_action_once() {
    // Other refusal classes require changed input, reviewed source changes, or a
    // holder's handoff. Revision-only retries and explicit budget retries are automatic.
    for kind in ["revision", "budget"] {
        let (p, error) = recoverable_error(kind);
        let mut recovery = Flow::default();
        let recovered = follow(&p, &error, &mut recovery);
        assert_eq!(recovery.calls.len(), P4_RECOVERY_CALLS);
        recovery
            .budget(&format!("P4 {kind}"), P4_RECOVERY_CALLS, usize::MAX)
            .unwrap();
        if kind == "revision" {
            assert!(
                std::fs::read_to_string(p.root.join("work.yaml"))
                    .unwrap()
                    .contains("Review a note")
            );
        } else {
            assert!(recovered.contains(checked_work::CRITERION), "{recovered}");
        }
    }
}

#[test]
fn p4_a_second_recovery_call_fails_the_budget_for_each_automatic_refusal() {
    for kind in ["revision", "budget"] {
        let (p, error) = recoverable_error(kind);
        let mut recovery = Flow::default();
        follow(&p, &error, &mut recovery);
        p.call(
            &mut recovery,
            "unnecessary second lookup",
            &["status"],
            false,
        );
        let error = recovery
            .budget(&format!("P4 {kind}"), P4_RECOVERY_CALLS, usize::MAX)
            .unwrap_err();
        assert!(error.contains("2 calls (limit 1)"));
    }
}
