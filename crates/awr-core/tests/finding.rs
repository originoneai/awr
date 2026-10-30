//! Positive and negative examples for finding requests, the rule that derives severity, the fold
//! of recorded findings and the count of rework cycles.
use awr_core::*;
use serde_json::{Value, json};

fn request(edit: impl FnOnce(&mut Value)) -> Result<FindingRequest> {
    let mut value = json!({"work": "W", "agent": "claude-run", "expected_revision": 3,
        "request_key": "k1", "finding_key": "f1", "action": "record", "round": 1,
        "summary": "Saving offline loses the draft"});
    edit(&mut value);
    let parsed: FindingRequest = serde_json::from_value(value)?;
    parsed.validate()?;
    Ok(parsed)
}

fn remove(value: &mut Value, field: &str) {
    value.as_object_mut().unwrap().remove(field);
}

fn example() -> Value {
    json!({"given": "the storage is read-only", "when": "the user saves", "then": "the draft is gone"})
}

#[test]
fn recording_a_finding_needs_a_round_and_a_summary_and_may_name_what_it_is_measured_against() {
    assert!(request(|_| {}).is_ok());
    assert!(request(|v| remove(v, "round")).is_err());
    assert!(
        request(|v| v["round"] = json!(0)).is_err(),
        "rounds count from 1"
    );
    assert!(request(|v| remove(v, "summary")).is_err());
    assert!(request(|v| v["summary"] = json!(" ")).is_err());
    assert!(
        request(|v| {
            v["criterion"] = json!("Saving failure keeps the user's input");
            v["rule"] = json!("data-integrity");
            v["counterexample"] = example();
            v["risk"] =
                json!({"kind": "data_loss", "reasoning": "A failed save deletes the only copy"});
        })
        .is_ok()
    );
    // A counterexample says what to do; a risk says why it matters.
    assert!(request(|v| v["counterexample"] = json!({"given": "x", "when": "y"})).is_err());
    assert!(
        request(|v| v["counterexample"] = json!({"given": "x", "when": "y", "then": " "})).is_err()
    );
    assert!(request(|v| v["risk"] = json!({"kind": "style", "reasoning": "x"})).is_err());
    assert!(request(|v| v["risk"] = json!({"kind": "security", "reasoning": " "})).is_err());
    assert!(request(|v| v["rule"] = json!(" data-integrity")).is_err());
    assert!(request(|v| v["resolution"] = json!("done")).is_err());
    assert!(request(|v| v["summary"] = json!("password: hunter2hunter2")).is_err());
}

#[test]
fn resolving_a_finding_states_only_how_it_was_dealt_with() {
    let resolve = |v: &mut Value| {
        for field in ["round", "summary"] {
            remove(v, field);
        }
        v["action"] = json!("resolve");
        v["resolution"] = json!("The fallback keeps the draft");
    };
    assert!(request(resolve).is_ok());
    assert!(
        request(|v| {
            resolve(v);
            remove(v, "resolution");
        })
        .is_err()
    );
    assert!(
        request(|v| {
            resolve(v);
            v["summary"] = json!("again");
        })
        .is_err()
    );
    assert!(
        request(|v| {
            resolve(v);
            v["counterexample"] = example();
        })
        .is_err()
    );
}

#[test]
fn a_request_names_one_decider_and_binds_everything_but_its_key_and_revision() {
    assert!(
        request(|v| {
            remove(v, "agent");
            v["session"] = json!(Id::new());
        })
        .is_ok()
    );
    assert!(request(|v| v["session"] = json!(Id::new())).is_err());
    assert!(request(|v| remove(v, "agent")).is_err());
    assert!(request(|v| v["finding_key"] = json!("")).is_err());
    assert!(request(|v| v["request_key"] = json!(" ")).is_err());
    assert!(serde_json::from_value::<FindingRequest>(json!({"work": "W", "extra": 1})).is_err());
    let first = request(|_| {}).unwrap();
    let mut second = first.clone();
    second.request_key = "k2".into();
    second.expected_revision = 99;
    assert_eq!(first.input(), second.input());
    second.round = Some(2);
    assert_ne!(first.input(), second.input());
}

fn finding(edit: impl FnOnce(&mut Value)) -> FindingInput {
    request(edit).unwrap().input()
}

fn judged(
    input: &FindingInput,
    tier: Option<DeliveryTier>,
    hard: Option<bool>,
) -> FindingAssessment {
    assess_finding(
        input,
        &FindingFacts {
            criterion_tier: tier,
            rule_is_hard: hard,
        },
    )
}

#[test]
fn only_a_required_criterion_or_a_hard_rule_with_a_counterexample_blocks_and_a_stated_risk_blocks_alone()
 {
    let with_example = finding(|v| v["counterexample"] = example());
    // A required criterion or a hard rule, shown with a counterexample: blocking.
    let by_criterion = judged(&with_example, Some(DeliveryTier::Required), None);
    assert_eq!(by_criterion.severity, "blocking");
    assert!(
        by_criterion.basis.contains("a required criterion")
            && by_criterion.basis.contains("counterexample")
    );
    let by_rule = judged(&with_example, None, Some(true));
    assert_eq!(by_rule.severity, "blocking");
    assert!(by_rule.basis.contains("a hard rule"));
    let by_both = judged(&with_example, Some(DeliveryTier::Required), Some(true));
    assert!(
        by_both
            .basis
            .contains("a required criterion and a hard rule")
    );
    // Without a counterexample it does not block, however firmly it is put.
    let no_example = finding(|_| {});
    let unproven = judged(&no_example, Some(DeliveryTier::Required), Some(true));
    assert_eq!(unproven.severity, "non_blocking");
    assert!(unproven.basis.contains("no reproducible counterexample"));
    // A later-tier criterion, a soft rule or nothing adopted does not block, counterexample or not.
    let later = judged(&with_example, Some(DeliveryTier::Later), None);
    assert_eq!(later.severity, "non_blocking");
    assert!(later.basis.contains("later-tier"));
    let soft = judged(&with_example, None, Some(false));
    assert_eq!(soft.severity, "non_blocking");
    let style = judged(&with_example, None, None);
    assert_eq!(style.severity, "non_blocking");
    assert!(style.basis.contains("taste or style"));
    // A stated security or data-loss risk with its reasoning blocks without any of that.
    for kind in ["security", "data_loss"] {
        let risky = finding(|v| {
            v["risk"] = json!({"kind": kind, "reasoning": "A failed save deletes the only copy"})
        });
        let assessed = judged(&risky, Some(DeliveryTier::Later), Some(false));
        assert_eq!(assessed.severity, "blocking", "{kind}");
        assert!(assessed.basis.contains("risk with its reasoning"));
    }
    // A second-round style finding is deferred.
    let second_round = finding(|v| {
        v["round"] = json!(2);
        v["summary"] = json!("Prefer shorter names for the helpers");
    });
    assert_eq!(judged(&second_round, None, None).severity, "non_blocking");
}

fn recorded(
    action: FindingAction,
    key: &str,
    edit: impl FnOnce(&mut FindingInput),
    assessment: Option<FindingAssessment>,
) -> RecordedFinding {
    let mut input = FindingInput {
        work: "W".into(),
        finding_key: key.into(),
        action: Some(action),
        ..Default::default()
    };
    if action == FindingAction::Record {
        input.round = Some(1);
        input.summary = Some("Saving offline loses the draft".into());
    }
    edit(&mut input);
    RecordedFinding {
        event: Id::new(),
        decider_agent: "reviewer".into(),
        input,
        assessment,
    }
}

fn verdict(severity: &str) -> Option<FindingAssessment> {
    Some(FindingAssessment {
        severity: severity.into(),
        basis: "derived".into(),
    })
}

#[test]
fn blocking_findings_return_the_work_and_the_rest_are_linked_deferred_records() {
    let blocking = recorded(
        FindingAction::Record,
        "f1",
        |i| i.criterion = Some("Saving failure keeps the user's input".into()),
        verdict("blocking"),
    );
    let style = recorded(
        FindingAction::Record,
        "f2",
        |i| {
            i.round = Some(2);
            i.rule = Some("naming".into());
        },
        verdict("non_blocking"),
    );
    let findings = fold_findings(&[blocking.clone(), style.clone()]);
    assert_eq!(findings.len(), 2);
    assert_eq!(
        (findings[0].disposition, findings[0].state),
        ("returns_work", "open")
    );
    assert_eq!(
        (findings[1].disposition, findings[1].state),
        ("deferred", "open")
    );
    // The deferred record stays linked to what it was measured against.
    assert_eq!(findings[1].rule.as_deref(), Some("naming"));
    assert_eq!(
        findings[0].criterion.as_deref(),
        Some("Saving failure keeps the user's input")
    );
    let status = FindingStatus::of("W", findings.clone());
    assert_eq!(status.open_blocking, ["f1"]);
    assert_eq!(status.open_deferred, ["f2"]);
    // Resolving the blocking one releases the work; the history stays.
    let resolve = recorded(
        FindingAction::Resolve,
        "f1",
        |i| i.resolution = Some("The fallback keeps the draft".into()),
        None,
    );
    let after = fold_findings(&[blocking.clone(), style.clone(), resolve.clone()]);
    assert_eq!(after[0].state, "resolved");
    assert_eq!(after[0].resolved_event, Some(resolve.event));
    assert_eq!(after[0].resolved_by.as_deref(), Some("reviewer"));
    assert!(open_blocking(&after).is_empty());
    assert_eq!(FindingStatus::of("W", after).open_deferred, ["f2"]);
    // A key names one finding for good, and only an open finding can be closed.
    let again = recorded(
        FindingAction::Record,
        "f1",
        |i| i.summary = Some("Another".into()),
        verdict("blocking"),
    );
    let late = recorded(
        FindingAction::Resolve,
        "f1",
        |i| i.resolution = Some("late".into()),
        None,
    );
    let folded = fold_findings(&[blocking, resolve, again, late]);
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].summary, "Saving offline loses the draft");
    assert_eq!(
        folded[0].resolution.as_deref(),
        Some("The fallback keeps the draft")
    );
    assert!(
        fold_findings(&[recorded(
            FindingAction::Resolve,
            "ghost",
            |i| i.resolution = Some("x".into()),
            None
        )])
        .is_empty()
    );
}

#[test]
fn a_work_is_sent_back_when_it_is_reopened_or_a_review_round_finds_blocking_issues_in_unfinished_work()
 {
    use ReworkRecord::*;
    assert_eq!(rework_cycles(&[]), 0);
    assert_eq!(rework_cycles(&[Reopened, Reopened, Reopened, Reopened]), 4);
    // Several blocking findings of one round are one return; a new round is another.
    assert_eq!(
        rework_cycles(&[
            BlockingFinding { round: 1 },
            BlockingFinding { round: 1 },
            BlockingFinding { round: 2 },
        ]),
        2
    );
    // A blocking finding against completed work is answered by the reopening, which is what counts.
    assert_eq!(
        rework_cycles(&[Completed, BlockingFinding { round: 1 }, Reopened]),
        1
    );
    assert_eq!(
        rework_cycles(&[BlockingFinding { round: 1 }, Completed, Reopened]),
        2
    );
    // After a reopening the work is unfinished again, so the next round counts.
    assert_eq!(
        rework_cycles(&[
            Completed,
            Reopened,
            BlockingFinding { round: 2 },
            Completed,
            BlockingFinding { round: 3 },
        ]),
        2
    );
    // The same round recorded before and after a completion is still one round.
    assert_eq!(
        rework_cycles(&[
            BlockingFinding { round: 1 },
            Completed,
            Reopened,
            BlockingFinding { round: 1 }
        ]),
        2
    );
}
