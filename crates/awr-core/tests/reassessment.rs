//! Positive and negative examples for reassessment requests, the triggers the records show and
//! the pure evaluator.
use awr_core::*;
use serde_json::{Value, json};

const A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn judgment() -> Value {
    json!({"reason": "The three failures came from the test I was fixing",
           "next_step": "Run the same check after the fix",
           "evidence_considered": "The output of the three failed runs"})
}

fn request(edit: impl FnOnce(&mut Value)) -> Result<ReassessmentRequest> {
    let mut value = json!({"work": "W", "session": Id::new(), "expected_revision": 3,
        "request_key": "k1", "decision": "continue", "facts_fingerprint": A, "judgment": judgment()});
    edit(&mut value);
    let parsed: ReassessmentRequest = serde_json::from_value(value)?;
    parsed.validate()?;
    Ok(parsed)
}

#[test]
fn a_request_names_one_decider_the_reviewed_facts_and_a_bounded_judgment() {
    assert!(request(|_| {}).is_ok());
    for decision in [
        "continue",
        "wait",
        "reuse",
        "alter_method",
        "split",
        "replan",
    ] {
        assert!(
            request(|v| v["decision"] = json!(decision)).is_ok(),
            "{decision}"
        );
    }
    assert!(request(|v| v["decision"] = json!("ignore")).is_err());
    // Exactly one of a session or a declaring agent decides.
    assert!(
        request(|v| {
            v.as_object_mut().unwrap().remove("session");
            v["agent"] = json!("claude-run");
        })
        .is_ok()
    );
    assert!(request(|v| v["agent"] = json!("claude-run")).is_err());
    assert!(
        request(|v| {
            v.as_object_mut().unwrap().remove("session");
        })
        .is_err()
    );
    assert!(
        request(|v| {
            v.as_object_mut().unwrap().remove("session");
            v["agent"] = json!(" padded ");
        })
        .is_err()
    );
    // The reviewed facts are a fingerprint; the judgment is bounded and public.
    assert!(request(|v| v["facts_fingerprint"] = json!("not-a-fingerprint")).is_err());
    for field in ["reason", "next_step", "evidence_considered"] {
        assert!(
            request(|v| v["judgment"][field] = json!("")).is_err(),
            "{field}"
        );
        assert!(
            request(|v| v["judgment"][field] = json!("x".repeat(2049))).is_err(),
            "{field}"
        );
    }
    assert!(request(|v| v["judgment"]["reason"] = json!("password: hunter2hunter2")).is_err());
    assert!(request(|v| v["request_key"] = json!(" ")).is_err());
    assert!(request(|v| v["work"] = json!("")).is_err());
    assert!(
        serde_json::from_value::<ReassessmentRequest>(json!({"work": "W", "extra": 1})).is_err()
    );
}

#[test]
fn a_root_cause_is_an_attributed_group_of_distinct_references() {
    let cause = |label: Value, references: Value| {
        request(|v| v["root_cause"] = json!({"label": label, "references": references}))
    };
    assert!(cause(json!("The fixture is read-only"), json!(["e1", "e2"])).is_ok());
    assert!(cause(json!(""), json!(["e1"])).is_err());
    assert!(cause(json!("x".repeat(257)), json!(["e1"])).is_err());
    assert!(cause(json!("Cause"), json!([])).is_err());
    assert!(cause(json!("Cause"), json!(["e1", "e1"])).is_err());
    assert!(cause(json!("Cause"), json!([" "])).is_err());
    assert!(
        cause(
            json!("Cause"),
            json!((0..33).map(|i| format!("e{i}")).collect::<Vec<_>>())
        )
        .is_err()
    );
    // The key binds the request without its revision guard and its own spelling.
    let first = request(|_| {}).unwrap();
    let mut second = first.clone();
    second.request_key = "k2".into();
    second.expected_revision = 99;
    assert_eq!(first.input(), second.input());
    second.root_cause = Some(RootCause {
        label: "Cause".into(),
        references: vec!["e1".into()],
    });
    assert_ne!(first.input(), second.input());
}

fn attempt(method: &str, reference: &str, order: i64, failed: bool) -> MethodAttempt {
    MethodAttempt {
        method: method.into(),
        reference: reference.into(),
        order: order as u64,
        failed,
    }
}

fn unknown(reference: &str) -> Deviation {
    Deviation {
        kind: DeviationKind::UnknownOutcome,
        reference: reference.into(),
        at: 10,
        basis: "recorded",
        method: None,
        detail: "started and nobody is left to say how it ended".into(),
    }
}

fn names(found: &[ReassessmentTrigger]) -> Vec<&'static str> {
    found.iter().map(|t| t.trigger).collect()
}

#[test]
fn repeated_failures_of_one_method_trigger_only_as_an_unbroken_streak() {
    let failed = |method: &str, n: i64| attempt(method, &format!("{method}{n}"), n, true);
    // Two failures are ordinary work.
    assert!(triggers(&[], &[failed("a", 1), failed("a", 2)]).is_empty());
    // Three in a row are not.
    let found = triggers(&[], &[failed("a", 1), failed("a", 2), failed("a", 3)]);
    assert_eq!(names(&found), ["repeated_failed_method"]);
    assert_eq!(found[0].references, ["a1", "a2", "a3"]);
    // A success of the same method ends the streak, whatever came before.
    let attempts = [
        failed("a", 1),
        failed("a", 2),
        failed("a", 3),
        attempt("a", "a4", 4, false),
    ];
    assert!(triggers(&[], &attempts).is_empty());
    // ... and only failures after it count.
    let attempts = [
        failed("a", 1),
        failed("a", 2),
        attempt("a", "a3", 3, false),
        failed("a", 4),
        failed("a", 5),
    ];
    assert!(triggers(&[], &attempts).is_empty());
    // Failures of different methods never add up, and another method's success ends nothing.
    let attempts = [
        failed("a", 1),
        failed("b", 2),
        failed("a", 3),
        attempt("b", "b4", 4, false),
        failed("a", 5),
    ];
    let found = triggers(&[], &attempts);
    assert_eq!(names(&found), ["repeated_failed_method"]);
    assert_eq!(found[0].references, ["a1", "a3", "a5"]);
    // Order is the order of the records, not of the list: a success that came first ends nothing,
    // and one that came last ends everything, wherever it is listed.
    let early = [
        failed("a", 1),
        failed("a", 2),
        failed("a", 3),
        attempt("a", "a0", 0, false),
    ];
    assert_eq!(names(&triggers(&[], &early)), ["repeated_failed_method"]);
    let late = [
        attempt("a", "a4", 4, false),
        failed("a", 1),
        failed("a", 2),
        failed("a", 3),
    ];
    assert!(triggers(&[], &late).is_empty());
    // Two methods can each be stuck.
    let attempts = [
        failed("a", 1),
        failed("a", 2),
        failed("a", 3),
        failed("b", 4),
        failed("b", 5),
        failed("b", 6),
    ];
    assert_eq!(triggers(&[], &attempts).len(), 2);
}

#[test]
fn an_outcome_nobody_knows_triggers_and_waits_and_blockers_do_not() {
    let found = triggers(&[unknown("x2"), unknown("x1")], &[]);
    assert_eq!(names(&found), ["unknown_outcome_unresolved"]);
    assert_eq!(found[0].references, ["x1", "x2"]);
    let quiet = [
        Deviation {
            kind: DeviationKind::ExternalWait,
            reference: "w1".into(),
            at: 1,
            basis: "recorded",
            method: None,
            detail: "waiting for an answer".into(),
        },
        Deviation {
            kind: DeviationKind::ObjectiveBlocker,
            reference: "W".into(),
            at: 0,
            basis: "source_declared",
            method: None,
            detail: "the vendor key is missing".into(),
        },
        Deviation {
            kind: DeviationKind::FailedMethod,
            reference: "e1".into(),
            at: 2,
            basis: "recorded",
            method: Some("a".into()),
            detail: "failed once".into(),
        },
    ];
    assert!(triggers(&quiet, &[attempt("a", "e1", 2, true)]).is_empty());
}

fn recorded(bound: &ReassessmentBinding) -> RecordedReassessment {
    RecordedReassessment {
        event: Id::new(),
        decision: ReassessmentChoice::Continue,
        bound: bound.clone(),
        decider_agent: "claude-run".into(),
        root_cause: None,
    }
}

fn state(
    deviations: &[Deviation],
    attempts: &[MethodAttempt],
    latest: Option<&RecordedReassessment>,
) -> ReassessmentStatus {
    evaluate_reassessment(&ReassessmentInputs {
        work: "W",
        deviations,
        attempts,
        stale_observations: &[],
        latest,
    })
}

fn codes(status: &ReassessmentStatus) -> Vec<&'static str> {
    status.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn clear_required_current_and_stale_each_answer_with_their_own_state() {
    let three = |from: i64| -> Vec<MethodAttempt> {
        (from..from + 3)
            .map(|n| attempt("a", &format!("a{n}"), n, true))
            .collect()
    };
    let clear = state(&[], &[attempt("a", "a1", 1, true)], None);
    assert_eq!((clear.state, codes(&clear)), ("clear", vec![]));
    assert!(clear.current.is_none());

    let attempts = three(1);
    let required = state(&[], &attempts, None);
    assert_eq!(
        (required.state, codes(&required)),
        ("required", vec!["reassessment_required"])
    );
    let bound = required.current.clone().expect("the facts are bound");
    assert!(bound.facts_fingerprint.starts_with("sha256:"));
    assert_eq!(bound.references, ["a1", "a2", "a3"]);

    // A decision on exactly these facts is current, and nothing is asked for.
    let decided = recorded(&bound);
    let current = state(&[], &attempts, Some(&decided));
    assert_eq!((current.state, codes(&current)), ("current", vec![]));
    assert_eq!(current.decision_event, Some(decided.event));

    // A further failure makes it stale and names what appeared.
    let more = three(1)
        .into_iter()
        .chain([attempt("a", "a4", 4, true)])
        .collect::<Vec<_>>();
    let stale = state(&[], &more, Some(&decided));
    assert_eq!(
        (stale.state, codes(&stale)),
        ("stale", vec!["reassessment_stale"])
    );
    assert!(stale.diagnostics[0].detail.contains("fact_added:a4"));

    // The streak ending asks for nothing, and the decision stays on record.
    let ended = state(
        &[],
        &[three(1), vec![attempt("a", "a4", 4, false)]].concat(),
        Some(&decided),
    );
    assert_eq!((ended.state, codes(&ended)), ("clear", vec![]));
    assert_eq!(ended.decision_event, Some(decided.event));

    // A decision about other facts is no decision about these: required, not stale.
    let other = state(&[unknown("x1")], &[], Some(&decided));
    assert_eq!(
        (other.state, codes(&other)),
        ("required", vec!["reassessment_required"])
    );
    assert!(other.diagnostics[0].detail.contains("other facts"));
}

#[test]
fn the_binding_names_added_and_removed_facts_and_ignores_their_order() {
    let make = |refs: &[&str]| ReassessmentBinding {
        facts_fingerprint: A.into(),
        references: refs.iter().map(|r| r.to_string()).collect(),
    };
    let old = make(&["a", "b"]);
    assert!(old.changed_from(&make(&["a", "b"])).is_empty());
    assert_eq!(
        old.changed_from(&make(&["b", "c"])),
        ["fact_added:c", "fact_removed:a"]
    );
    let trigger = |references: &[&str]| ReassessmentTrigger {
        trigger: "unknown_outcome_unresolved",
        references: references.iter().map(|r| r.to_string()).collect(),
        detail: String::new(),
    };
    let one = ReassessmentBinding::of(&[trigger(&["b", "a"])]).unwrap();
    let two = ReassessmentBinding::of(&[trigger(&["a"]), trigger(&["a", "b"])]).unwrap();
    assert_eq!(
        one, two,
        "order and repetition do not change what was decided"
    );
    assert_eq!(one.references, ["a", "b"]);
    assert!(ReassessmentBinding::of(&[]).is_none());
    assert_ne!(
        one.facts_fingerprint,
        ReassessmentBinding::of(&[trigger(&["a", "c"])])
            .unwrap()
            .facts_fingerprint
    );
}

#[test]
fn reassessment_codes_belong_to_the_admission_family_that_gates_claims() {
    for code in [
        "reassessment_required",
        "reassessment_stale",
        "reassessment_unknown",
    ] {
        assert!(is_admission_code(code), "{code}");
    }
    assert!(!is_admission_code("active_blocker"));
}
