//! The earliest material gap of governed work becomes one bounded instruction, and unknown,
//! denied and stale judgments are told apart.
use awr_core::*;

fn view(codes: &[(&str, &str)], blocking: &[&str]) -> Option<ActionGuidance> {
    let diagnostics: Vec<(String, String)> = codes
        .iter()
        .map(|(c, d)| (c.to_string(), d.to_string()))
        .collect();
    let blocking: Vec<String> = blocking.iter().map(|k| k.to_string()).collect();
    governance_guidance(&GovernanceView {
        diagnostics: &diagnostics,
        open_blocking_findings: &blocking,
    })
}

const CODES: &[&str] = &[
    "admission_policy_invalid",
    "admission_unknown",
    "reassessment_unknown",
    "admission_obligations_unmet",
    "admission_missing",
    "admission_pending",
    "admission_denied",
    "admission_stale",
    "admission_suspended",
    "containment_unchecked",
    "containment_missing",
    "containment_pending",
    "containment_denied",
    "containment_stale",
    "containment_not_authorized",
    "containment_suspended",
    "reassessment_required",
    "reassessment_stale",
    "reassessment_budget_required",
    "reassessment_budget_stale",
    "reassessment_budget_escalated",
    "reassessment_not_authorized",
];

#[test]
fn every_gap_gets_one_instruction_with_a_condition_a_basis_an_action_and_a_recheck() {
    for code in CODES {
        let guidance = view(&[(code, "the detail the program recorded")], &[])
            .unwrap_or_else(|| panic!("{code} has no guidance"));
        for (field, text) in [
            ("when", &guidance.when),
            ("basis", &guidance.basis),
            ("next_action", &guidance.next_action),
            ("recheck", &guidance.recheck),
        ] {
            assert!(!text.trim().is_empty(), "{code}: {field}");
        }
        assert!(
            guidance.basis.starts_with(code),
            "{code}: {}",
            guidance.basis
        );
    }
    // No two different gaps share their condition, except the pairs that are one thing in two places.
    let mut conditions: Vec<(String, &str)> = CODES
        .iter()
        .map(|c| (view(&[(c, "d")], &[]).unwrap().when, *c))
        .collect();
    conditions.sort();
    let shared: Vec<_> = conditions
        .windows(2)
        .filter(|w| w[0].0 == w[1].0)
        .map(|w| (w[0].1, w[1].1))
        .collect();
    assert_eq!(
        shared,
        [
            ("admission_suspended", "containment_suspended"),
            ("admission_unknown", "reassessment_unknown")
        ]
    );
}

#[test]
fn the_instruction_is_bounded_however_long_the_detail_is() {
    let long = "x".repeat(5000);
    for code in CODES {
        let guidance = view(&[(code, &long)], &[]).unwrap();
        let size = serde_json::to_vec(&guidance).unwrap().len();
        assert!(size <= ACTION_GUIDANCE_MAX_BYTES, "{code}: {size}");
    }
    let many: Vec<String> = (0..200).map(|n| format!("finding-{n}")).collect();
    let keys: Vec<&str> = many.iter().map(String::as_str).collect();
    let guidance = view(&[], &keys).unwrap();
    assert!(serde_json::to_vec(&guidance).unwrap().len() <= ACTION_GUIDANCE_MAX_BYTES);
    // Control characters in a detail never reach the instruction.
    let guidance = view(&[("admission_denied", "line one\nline\ttwo\u{7}")], &[]).unwrap();
    assert!(
        !guidance.basis.chars().any(char::is_control),
        "{}",
        guidance.basis
    );
}

#[test]
fn unknown_denied_and_stale_are_explained_as_the_different_things_they_are() {
    let unknown = view(&[("admission_unknown", "SourceStale")], &[]).unwrap();
    assert!(unknown.when.contains("could not be established"));
    assert!(
        unknown
            .next_action
            .contains("neither an approval nor a denial")
            && unknown.next_action.contains("Refresh"),
        "{}",
        unknown.next_action
    );
    let denied = view(
        &[(
            "admission_denied",
            "the second example is a restatement of the first",
        )],
        &[],
    )
    .unwrap();
    assert!(denied.when.contains("denied"));
    assert!(
        denied.basis.contains("restatement of the first"),
        "the reason is the basis"
    );
    assert!(denied.next_action.contains("propose again"));
    let stale = view(&[("admission_stale", "scope changed: examples")], &[]).unwrap();
    assert!(stale.when.contains("no longer matches"));
    assert!(stale.basis.contains("examples"));
    assert!(stale.next_action.contains("Decide again"));
    // Three states, three instructions.
    assert_ne!(unknown.next_action, denied.next_action);
    assert_ne!(denied.next_action, stale.next_action);
    assert_ne!(unknown.when, stale.when);
}

#[test]
fn a_supervisor_replanning_within_the_granted_scope_needs_no_new_permission_from_the_user() {
    for code in ["admission_stale", "containment_stale"] {
        let guidance = view(&[(code, "changed")], &[]).unwrap();
        assert!(
            guidance.next_action.contains("supervisor")
                && guidance
                    .next_action
                    .contains("needs no new permission from the user"),
            "{code}: {}",
            guidance.next_action
        );
        // Widening is another matter.
        assert!(
            guidance.next_action.contains("widening") || guidance.next_action.contains("paths")
        );
    }
}

#[test]
fn the_earliest_material_gap_wins_and_nothing_means_no_instruction() {
    assert!(view(&[], &[]).is_none());
    assert!(
        view(&[("active_claim", "held by another session")], &[]).is_none(),
        "other diagnostics are not governance"
    );
    let first = |codes: &[&str]| -> String {
        let pairs: Vec<(&str, &str)> = codes.iter().map(|c| (*c, "d")).collect();
        view(&pairs, &["f1"]).unwrap().basis
    };
    // What makes the state unreadable comes first, whatever else is wrong.
    assert!(
        first(&["admission_stale", "admission_missing", "admission_unknown"])
            .starts_with("admission_unknown")
    );
    // The definition before the decision, the decision before the parent's set, the set before the facts.
    assert!(
        first(&["admission_missing", "admission_obligations_unmet"])
            .starts_with("admission_obligations_unmet")
    );
    assert!(first(&["containment_missing", "admission_missing"]).starts_with("admission_missing"));
    assert!(
        first(&["reassessment_required", "containment_missing"]).starts_with("containment_missing")
    );
    assert!(
        first(&["reassessment_budget_required", "reassessment_required"])
            .starts_with("reassessment_required")
    );
    // Blocking findings hold completion only, so they come after every readiness gap.
    assert!(first(&["reassessment_budget_required"]).starts_with("reassessment_budget_required"));
    let findings = view(&[], &["bug", "leak"]).unwrap();
    assert!(findings.when.contains("Blocking findings"));
    assert!(findings.basis.contains("bug, leak"));
    assert!(
        findings
            .next_action
            .contains("deferred records need no action")
    );
}

#[test]
fn forming_a_definition_and_repairing_one_are_told_apart_and_neither_needs_ceremony() {
    let forming = view(
        &[(
            "admission_obligations_unmet",
            "definition gaps: definition_not_declared",
        )],
        &[],
    )
    .unwrap();
    let repairing = view(
        &[(
            "admission_obligations_unmet",
            "definition gaps: examples_missing, discrimination_missing",
        )],
        &[],
    )
    .unwrap();
    assert!(
        forming.when.contains("no definition yet"),
        "{}",
        forming.when
    );
    assert!(
        repairing.when.contains("leaves obligations unmet"),
        "{}",
        repairing.when
    );
    assert!(
        repairing.basis.contains("examples_missing"),
        "the detail names what is missing"
    );
    for guidance in [&forming, &repairing] {
        // One source edit and one admit call; no stage document, confirmation round or step-level task.
        assert!(
            guidance.next_action.contains("one source edit"),
            "{}",
            guidance.next_action
        );
        assert!(guidance.next_action.contains("one work admit call"));
        assert!(guidance.next_action.contains("no stage document"));
    }
    assert_ne!(forming.when, repairing.when);
}
