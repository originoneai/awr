//! Positive and negative examples for containment requests and the pure evaluator.
use awr_core::*;
use serde_json::{Value, json};

const A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn judgment() -> Value {
    json!({"contained_because": "Both children serve the saving outcome and add no new surface",
           "reuse_checked": "The existing draft buffer is reused instead of a new store",
           "out_of_scope_excluded": "Syncing is left to a later change"})
}

fn request(decision: &str, edit: impl FnOnce(&mut Value)) -> Result<ContainmentRequest> {
    let mut value = json!({"parent": "P", "session": Id::new(), "expected_revision": 3,
        "request_key": "k1", "decision": decision, "change_set_fingerprint": A});
    edit(&mut value);
    let parsed: ContainmentRequest = serde_json::from_value(value)?;
    parsed.validate()?;
    Ok(parsed)
}

#[test]
fn a_containment_request_names_one_decider_a_reviewed_set_and_a_judgment_that_fits_the_decision() {
    assert!(request("proposed", |v| v["judgment"] = judgment()).is_ok());
    assert!(request("proposed", |_| {}).is_err());
    assert!(request("approved", |v| v["judgment"] = judgment()).is_ok());
    assert!(request("approved", |v| v["proposal_event"] = json!(Id::new())).is_ok());
    assert!(request("approved", |_| {}).is_err());
    assert!(
        request("denied", |v| v["reason"] =
            json!("The second child widens the parent"))
        .is_ok()
    );
    assert!(request("denied", |_| {}).is_err());
    // Exactly one of a session or a declaring agent decides.
    let as_agent = |v: &mut Value| {
        v.as_object_mut().unwrap().remove("session");
        v["agent"] = json!("claude-run");
        v["judgment"] = judgment();
    };
    assert!(request("approved", as_agent).is_ok());
    assert!(
        request("approved", |v| {
            v["judgment"] = judgment();
            v["agent"] = json!("claude-run");
        })
        .is_err()
    );
    assert!(
        request("approved", |v| {
            v.as_object_mut().unwrap().remove("session");
            v["judgment"] = judgment();
        })
        .is_err()
    );
    // The reviewed set is a fingerprint, the judgment is bounded and public.
    assert!(
        request("approved", |v| {
            v["judgment"] = judgment();
            v["change_set_fingerprint"] = json!("not-a-fingerprint");
        })
        .is_err()
    );
    assert!(
        request("approved", |v| {
            v["judgment"] = judgment();
            v["judgment"]["reuse_checked"] = json!("");
        })
        .is_err()
    );
    assert!(
        request("approved", |v| {
            v["judgment"] = judgment();
            v["judgment"]["contained_because"] = json!("password: hunter2hunter2");
        })
        .is_err()
    );
    assert!(
        serde_json::from_value::<ContainmentRequest>(json!({"parent": "P", "extra": 1})).is_err()
    );
}

fn binding(change_set: &str, children: &[&str]) -> ContainmentBinding {
    ContainmentBinding {
        parent_contract: A.into(),
        change_set: change_set.into(),
        dependencies: A.into(),
        children: children.iter().map(|c| c.to_string()).collect(),
    }
}

fn recorded(
    decision: AdmissionDecision,
    bound: ContainmentBinding,
    agent: &str,
) -> RecordedContainment {
    RecordedContainment {
        event: Id::new(),
        decision,
        bound,
        reason: Some("The second child widens the parent".into()),
        decider_agent: agent.into(),
    }
}

fn status(
    latest: Option<&RecordedContainment>,
    current: &ContainmentBinding,
    failures: &[(String, String)],
    supervisors: &[String],
) -> ContainmentStatus {
    evaluate_containment(&ContainmentInputs {
        parent: "P",
        supervisors,
        current,
        failures,
        latest,
    })
}

fn codes(status: &ContainmentStatus) -> Vec<&'static str> {
    status.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn missing_pending_denied_stale_and_approved_each_answer_with_their_own_code() {
    let now = binding(A, &["C1", "C2"]);
    assert_eq!(
        codes(&status(None, &now, &[], &[])),
        ["containment_missing"]
    );
    let proposed = recorded(AdmissionDecision::Proposed, now.clone(), "agent-a");
    let pending = status(Some(&proposed), &now, &[], &[]);
    assert_eq!(
        (pending.state, codes(&pending)),
        ("pending", vec!["containment_pending"])
    );
    let denied = recorded(AdmissionDecision::Denied, now.clone(), "agent-a");
    let refused = status(Some(&denied), &now, &[], &[]);
    assert_eq!(
        (refused.state, codes(&refused)),
        ("denied", vec!["containment_denied"])
    );
    assert!(refused.diagnostics[0].detail.contains("widens the parent"));
    let approved = recorded(AdmissionDecision::Approved, now.clone(), "agent-a");
    let ok = status(Some(&approved), &now, &[], &[]);
    assert_eq!(ok.state, "approved");
    assert!(ok.diagnostics.is_empty(), "{ok:?}");
    assert_eq!(ok.children, ["C1", "C2"]);
}

#[test]
fn a_stale_approval_names_what_changed_and_each_child_added_or_removed() {
    let approved = recorded(
        AdmissionDecision::Approved,
        binding(A, &["C1", "C2"]),
        "agent-a",
    );
    // A child was added and one removed: the removal is named, so a denominator cannot shrink unseen.
    let drifted = binding(B, &["C1", "C3"]);
    let stale = status(Some(&approved), &drifted, &[], &[]);
    assert_eq!(
        (stale.state, codes(&stale)),
        ("stale", vec!["containment_stale"])
    );
    let detail = &stale.diagnostics[0].detail;
    for part in ["change_set", "child_added:C3", "child_removed:C2"] {
        assert!(detail.contains(part), "{part} in {detail}");
    }
    assert!(
        !detail.contains("parent_contract") && !detail.contains("dependencies"),
        "{detail}"
    );
    for (label, edit) in [
        (
            "parent_contract",
            (|b: &mut ContainmentBinding| b.parent_contract = B.into())
                as fn(&mut ContainmentBinding),
        ),
        ("dependencies", |b| b.dependencies = B.into()),
    ] {
        let mut now = binding(A, &["C1", "C2"]);
        edit(&mut now);
        let status = status(Some(&approved), &now, &[], &[]);
        assert_eq!(status.state, "stale", "{label}");
        assert!(status.diagnostics[0].detail.contains(label));
    }
}

#[test]
fn deterministic_failures_of_the_children_are_reported_even_under_an_approval() {
    let now = binding(A, &["C1"]);
    let approved = recorded(AdmissionDecision::Approved, now.clone(), "agent-a");
    let failures = vec![(
        "child_outside_parent_scope".to_string(),
        "C1 touches crates/other".to_string(),
    )];
    let found = status(Some(&approved), &now, &failures, &[]);
    assert_eq!(codes(&found), ["containment_unchecked"]);
    assert!(
        found.diagnostics[0]
            .detail
            .contains("child_outside_parent_scope: C1 touches crates/other")
    );
}

#[test]
fn supervisors_gate_the_approval_only_when_the_policy_names_some() {
    let now = binding(A, &["C1"]);
    let approved = recorded(AdmissionDecision::Approved, now.clone(), "agent-a");
    assert!(
        status(Some(&approved), &now, &[], &[])
            .diagnostics
            .is_empty(),
        "no names, any agent may"
    );
    let named = vec!["lead".to_string()];
    let refused = status(Some(&approved), &now, &[], &named);
    assert_eq!(codes(&refused), ["containment_not_authorized"]);
    assert!(refused.diagnostics[0].detail.contains("agent-a"));
    let by_lead = recorded(AdmissionDecision::Approved, now.clone(), "lead");
    assert!(
        status(Some(&by_lead), &now, &[], &named)
            .diagnostics
            .is_empty()
    );
}

#[test]
fn a_parent_without_children_has_nothing_to_contain() {
    let status = ContainmentStatus::no_children("P");
    assert_eq!(status.state, "no_children");
    assert!(status.diagnostics.is_empty() && status.current.is_none());
}

#[test]
fn supervisors_are_a_bounded_list_of_agent_names_in_the_policy() {
    let policy = |supervisors: Value| -> AdmissionPolicy {
        serde_json::from_value(json!({"version": 1, "mode": "governed_enforce", "scope": "all", "supervisors": supervisors})).unwrap()
    };
    assert!(policy(json!(["lead", "claude-run"])).validate().is_ok());
    assert!(policy(json!([" "])).validate().is_err());
    assert!(
        policy(json!((0..33).map(|i| format!("a{i}")).collect::<Vec<_>>()))
            .validate()
            .is_err()
    );
}
