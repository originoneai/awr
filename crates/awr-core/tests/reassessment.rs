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
        subject: ReassessmentSubject::Facts,
        bound: bound.clone(),
        decider_agent: "claude-run".into(),
        root_cause: None,
        escalated_to: None,
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
        budget: None,
        latest_budget: None,
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

fn use_of(rework: (u64, &'static str), effort: (u64, &'static str)) -> BudgetUse {
    BudgetUse::of(&effective_budget(None), rework, effort)
}

#[test]
fn size_gives_the_default_budgets_and_a_declared_limit_replaces_one_of_them() {
    let defaults = |size: Option<WorkSize>, budget: Option<WorkBudget>| {
        let mut definition: WorkDefinition =
            serde_json::from_value(json!({"version": 1, "purpose": "implementation"})).unwrap();
        definition.size = size;
        definition.budget = budget;
        effective_budget(Some(&definition))
    };
    let undeclared = effective_budget(None);
    assert_eq!(
        (
            undeclared.size,
            undeclared.rework_cycles,
            undeclared.active_effort_minutes
        ),
        (WorkSize::Medium, 3, 240)
    );
    assert_eq!(
        (
            undeclared.size_basis,
            undeclared.rework_basis,
            undeclared.effort_basis
        ),
        ("default", "size_default", "size_default")
    );
    let small = defaults(Some(WorkSize::Small), None);
    assert_eq!(
        (
            small.rework_cycles,
            small.active_effort_minutes,
            small.size_basis
        ),
        (2, 60, "declared")
    );
    let large = defaults(Some(WorkSize::Large), None);
    assert_eq!((large.rework_cycles, large.active_effort_minutes), (3, 960));
    let both = defaults(
        Some(WorkSize::Small),
        Some(WorkBudget {
            rework_cycles: Some(4),
            active_effort_minutes: None,
        }),
    );
    assert_eq!((both.rework_cycles, both.rework_basis), (4, "declared"));
    assert_eq!(
        (both.active_effort_minutes, both.effort_basis),
        (60, "size_default")
    );
    let zero = defaults(
        None,
        Some(WorkBudget {
            rework_cycles: Some(0),
            active_effort_minutes: Some(30),
        }),
    );
    assert_eq!((zero.rework_cycles, zero.active_effort_minutes), (0, 30));
    // Bounds are checked when the definition is read.
    let bounded = |budget: Value| -> bool {
        let definition: WorkDefinition = serde_json::from_value(
            json!({"version": 1, "purpose": "implementation", "budget": budget}),
        )
        .unwrap();
        definition.validate().is_ok()
    };
    assert!(bounded(
        json!({"rework_cycles": 20, "active_effort_minutes": 100000})
    ));
    assert!(!bounded(json!({"rework_cycles": 21})));
    assert!(!bounded(json!({"active_effort_minutes": 0})));
    assert!(!bounded(json!({"active_effort_minutes": 100001})));
    assert!(
        serde_json::from_value::<WorkDefinition>(
            json!({"version": 1, "purpose": "implementation", "size": "huge"})
        )
        .is_err()
    );
}

#[test]
fn a_fourth_rework_cycle_under_a_budget_of_three_is_spent_and_the_third_is_not() {
    assert!(use_of((3, "recorded"), (0, "recorded")).trigger().is_none());
    let spent = use_of((4, "recorded"), (0, "recorded"))
        .trigger()
        .expect("the fourth cycle spends it");
    assert_eq!(spent.trigger, "budget_exhausted");
    assert_eq!(spent.references, ["budget:rework:4>3"]);
    assert!(
        spent
            .detail
            .contains("4 rework cycles against a budget of 3")
    );
    // Effort is spent by going over the allowance, and is bound by the allowance, not the minute.
    assert!(
        use_of((0, "recorded"), (240, "recorded"))
            .trigger()
            .is_none()
    );
    let at_300 = use_of((0, "recorded"), (300, "recorded"))
        .trigger()
        .unwrap();
    let at_400 = use_of((0, "recorded"), (400, "recorded"))
        .trigger()
        .unwrap();
    assert_eq!(at_300.references, ["budget:effort:over:240"]);
    assert_eq!(
        at_300.references, at_400.references,
        "the decision does not go stale by the minute"
    );
    let both = use_of((5, "recorded"), (500, "recorded"))
        .trigger()
        .unwrap();
    assert_eq!(
        both.references,
        ["budget:rework:5>3", "budget:effort:over:240"]
    );
}

#[test]
fn own_records_are_counted_before_the_counters_a_host_reports() {
    assert_eq!(count_with_fallback(2, Some(5)), (2, "recorded"));
    assert_eq!(count_with_fallback(0, Some(5)), (5, "host_reported"));
    assert_eq!(count_with_fallback(0, None), (0, "recorded"));
    assert_eq!(count_with_fallback(0, Some(0)), (0, "host_reported"));
    let line = use_of((4, "host_reported"), (0, "recorded")).rework;
    assert_eq!(
        (line.counted_from, line.exhausted()),
        ("host_reported", true)
    );
}

fn budget_decision(
    choice: ReassessmentChoice,
    usage: &BudgetUse,
    escalated_to: Option<&str>,
) -> RecordedReassessment {
    RecordedReassessment {
        event: Id::new(),
        decision: choice,
        subject: ReassessmentSubject::Budget,
        bound: ReassessmentBinding::of(&[usage.trigger().expect("a spent budget")]).unwrap(),
        decider_agent: "claude-run".into(),
        root_cause: None,
        escalated_to: escalated_to.map(String::from),
    }
}

fn budget_state(usage: &BudgetUse, latest: Option<&RecordedReassessment>) -> ReassessmentStatus {
    evaluate_reassessment(&ReassessmentInputs {
        work: "W",
        deviations: &[],
        attempts: &[],
        stale_observations: &[],
        latest: None,
        budget: Some(usage),
        latest_budget: latest,
    })
}

#[test]
fn a_spent_budget_needs_its_own_decision_and_a_further_cycle_makes_it_stale() {
    let within = budget_state(&use_of((3, "recorded"), (10, "recorded")), None);
    assert_eq!(
        (within.state, within.budget.as_ref().unwrap().state),
        ("clear", "within")
    );
    assert!(within.diagnostics.is_empty());
    let spent = use_of((4, "recorded"), (10, "recorded"));
    let required = budget_state(&spent, None);
    assert_eq!(codes(&required), ["reassessment_budget_required"]);
    assert_eq!(required.state, "clear", "the facts are another matter");
    assert!(
        required.diagnostics[0]
            .detail
            .contains("deliver with known issues")
    );
    // Deciding it settles it until the count moves.
    let delivered = budget_decision(ReassessmentChoice::DeliverWithKnownIssues, &spent, None);
    let current = budget_state(&spent, Some(&delivered));
    assert_eq!(current.budget.as_ref().unwrap().state, "current");
    assert!(current.diagnostics.is_empty());
    let fifth = use_of((5, "recorded"), (10, "recorded"));
    let stale = budget_state(&fifth, Some(&delivered));
    assert_eq!(codes(&stale), ["reassessment_budget_stale"]);
    assert!(
        stale.diagnostics[0]
            .detail
            .contains("fact_added:budget:rework:5>3")
    );
    assert!(
        stale.diagnostics[0]
            .detail
            .contains("fact_removed:budget:rework:4>3")
    );
    // A decision for one budget does not settle the other.
    let effort = use_of((3, "recorded"), (500, "recorded"));
    assert_eq!(
        codes(&budget_state(&effort, Some(&delivered))),
        ["reassessment_budget_required"]
    );
    // An escalated budget stays held until someone else decides.
    let escalated = budget_decision(ReassessmentChoice::Escalate, &spent, Some("the owner"));
    let held = budget_state(&spent, Some(&escalated));
    assert_eq!(held.budget.as_ref().unwrap().state, "escalated");
    assert_eq!(codes(&held), ["reassessment_budget_escalated"]);
    assert!(held.diagnostics[0].detail.contains("the owner"));
    // The budget is within again once the work replans it, whatever was decided before.
    let replanned = BudgetUse::of(
        &EffectiveBudget {
            rework_cycles: 6,
            rework_basis: "declared",
            ..effective_budget(None)
        },
        (4, "recorded"),
        (10, "recorded"),
    );
    let after = budget_state(&replanned, Some(&delivered));
    assert_eq!(after.budget.as_ref().unwrap().state, "within");
    assert!(after.diagnostics.is_empty());
}

fn budget_request(edit: impl FnOnce(&mut Value)) -> Result<ReassessmentRequest> {
    request(|v| {
        v["answers"] = json!("budget");
        v["decision"] = json!("replan");
        edit(v)
    })
}

#[test]
fn budget_choices_answer_a_budget_and_carry_what_they_need_and_nothing_else() {
    assert!(budget_request(|_| {}).is_ok());
    assert!(
        budget_request(|v| {
            v["decision"] = json!("deliver_with_known_issues");
            v["known_issues"] = json!(["The retry path is untested", "Docs lag one release"]);
        })
        .is_ok()
    );
    assert!(
        budget_request(|v| v["decision"] = json!("deliver_with_known_issues")).is_err(),
        "issues are named"
    );
    assert!(
        budget_request(|v| {
            v["decision"] = json!("deliver_with_known_issues");
            v["known_issues"] = json!([" "]);
        })
        .is_err()
    );
    assert!(
        budget_request(|v| {
            v["decision"] = json!("deliver_with_known_issues");
            v["known_issues"] = json!((0..17).map(|i| format!("issue {i}")).collect::<Vec<_>>());
        })
        .is_err()
    );
    assert!(
        budget_request(|v| {
            v["decision"] = json!("reduce_scope");
            v["reduced_scope"] = json!("Only the read path ships");
        })
        .is_ok()
    );
    assert!(budget_request(|v| v["decision"] = json!("reduce_scope")).is_err());
    assert!(
        budget_request(|v| {
            v["decision"] = json!("escalate");
            v["escalated_to"] = json!("the owner");
        })
        .is_ok()
    );
    assert!(budget_request(|v| v["decision"] = json!("escalate")).is_err());
    assert!(
        budget_request(|v| {
            v["decision"] = json!("escalate");
            v["escalated_to"] = json!(" the owner ");
        })
        .is_err()
    );
    // Nothing is carried that the choice does not use.
    assert!(budget_request(|v| v["escalated_to"] = json!("the owner")).is_err());
    // Continuing is not an answer to a spent budget, and a budget choice does not answer facts.
    assert!(budget_request(|v| v["decision"] = json!("continue")).is_err());
    assert!(request(|v| v["decision"] = json!("reduce_scope")).is_err());
    assert!(
        request(|v| {
            v["decision"] = json!("escalate");
            v["escalated_to"] = json!("the owner");
        })
        .is_err()
    );
    assert!(
        request(|v| v["decision"] = json!("replan")).is_ok(),
        "replan answers either"
    );
    // A request about facts keeps the content it always had.
    let facts = request(|_| {}).unwrap();
    assert!(facts.input().get("answers").is_none() && facts.input().get("known_issues").is_none());
    let budget = budget_request(|_| {}).unwrap();
    assert_eq!(budget.input()["answers"], "budget");
}
