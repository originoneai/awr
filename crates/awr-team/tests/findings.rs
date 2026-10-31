//! Findings of a Team review: severity is a consequence of the record, and a re-review answers for
//! the open blocking findings and the delta only.
use awr_core::{Counterexample, DeliveryTier, Risk, RiskKind};
use awr_team::{ReviewFinding, ReviewScope, judge_finding};
use std::collections::BTreeSet;

const CRITERION: &str = "Saving failure keeps the user's input";
const RULE: &str = "Never lose a draft";

fn acceptance() -> Vec<String> {
    vec![CRITERION.into(), "The note shows its date".into()]
}

fn rules() -> Vec<String> {
    vec![RULE.into()]
}

fn finding(key: &str) -> ReviewFinding {
    ReviewFinding {
        finding_key: key.into(),
        summary: "Saving offline loses the draft".into(),
        criterion: None,
        rule: None,
        counterexample: None,
        risk: None,
        scope: None,
    }
}

fn example() -> Counterexample {
    Counterexample {
        given: "the storage is read-only".into(),
        when: "the user saves".into(),
        then: "the draft is gone".into(),
    }
}

fn judge(
    f: &ReviewFinding,
    round: u32,
    tier: DeliveryTier,
    unresolved: &[&str],
) -> awr_team::JudgedFinding {
    let open: BTreeSet<String> = unresolved.iter().map(|k| k.to_string()).collect();
    judge_finding(f, round, &acceptance(), &rules(), tier, &open).unwrap()
}

#[test]
fn a_required_criterion_with_a_counterexample_blocks_and_returns_the_work() {
    let mut f = finding("F1");
    f.criterion = Some(CRITERION.into());
    f.counterexample = Some(example());
    let judged = judge(&f, 1, DeliveryTier::Required, &[]);
    assert_eq!(judged.severity, "blocking");
    assert_eq!(judged.disposition, "returns_work");
    assert!(
        judged.basis.contains("required criterion"),
        "{}",
        judged.basis
    );
}

#[test]
fn without_a_counterexample_style_or_a_later_tier_a_finding_is_deferred() {
    // Names a required criterion but shows nothing reproducible.
    let mut f = finding("F2");
    f.criterion = Some(CRITERION.into());
    let judged = judge(&f, 1, DeliveryTier::Required, &[]);
    assert_eq!(
        (judged.severity.as_str(), judged.disposition),
        ("non_blocking", "deferred")
    );
    // Shows a counterexample against a later-tier criterion.
    f.counterexample = Some(example());
    let judged = judge(&f, 1, DeliveryTier::Later, &[]);
    assert_eq!(
        (judged.severity.as_str(), judged.disposition),
        ("non_blocking", "deferred")
    );
    assert!(judged.basis.contains("later-tier"), "{}", judged.basis);
    // Names nothing the project adopted: a matter of style.
    let style = judge(&finding("F3"), 1, DeliveryTier::Required, &[]);
    assert_eq!(style.disposition, "deferred");
    assert!(style.basis.contains("taste or style"), "{}", style.basis);
}

#[test]
fn a_hard_rule_with_a_counterexample_blocks_and_a_rule_outside_the_contract_is_refused() {
    let mut f = finding("F4");
    f.rule = Some(RULE.into());
    f.counterexample = Some(example());
    assert_eq!(judge(&f, 1, DeliveryTier::Later, &[]).severity, "blocking");
    f.rule = Some("Prefer short names".into());
    let refused = judge_finding(
        &f,
        1,
        &acceptance(),
        &rules(),
        DeliveryTier::Required,
        &BTreeSet::new(),
    );
    assert!(refused.is_err());
}

#[test]
fn a_stated_risk_blocks_whatever_the_tier() {
    let mut f = finding("F5");
    f.risk = Some(Risk {
        kind: RiskKind::DataLoss,
        reasoning: "A failed save overwrites the previous draft".into(),
    });
    for tier in [DeliveryTier::Required, DeliveryTier::Later] {
        assert_eq!(judge(&f, 1, tier, &[]).severity, "blocking");
    }
    // A risk without its reasoning is not a stated risk.
    f.risk = Some(Risk {
        kind: RiskKind::Security,
        reasoning: "  ".into(),
    });
    assert!(f.validate().is_err());
}

#[test]
fn what_a_finding_names_must_exist_in_the_contract() {
    let mut f = finding("F6");
    f.criterion = Some("A criterion nobody wrote".into());
    assert!(
        judge_finding(
            &f,
            1,
            &acceptance(),
            &rules(),
            DeliveryTier::Required,
            &BTreeSet::new()
        )
        .is_err()
    );
}

#[test]
fn a_second_round_answers_for_open_findings_and_the_delta_and_defers_the_rest() {
    let mut f = finding("F7");
    f.criterion = Some(CRITERION.into());
    f.counterexample = Some(example());
    // The first round counts everything that shows a counterexample.
    assert_eq!(
        judge(&f, 1, DeliveryTier::Required, &[]).severity,
        "blocking"
    );
    // From the second round on a blocking finding must belong to the re-review scope.
    let loose = judge(&f, 2, DeliveryTier::Required, &["F1"]);
    assert_eq!(
        (loose.severity.as_str(), loose.disposition),
        ("non_blocking", "deferred")
    );
    assert!(loose.basis.contains("re-review"), "{}", loose.basis);
    f.scope = Some(ReviewScope::Delta);
    assert_eq!(
        judge(&f, 2, DeliveryTier::Required, &["F1"]).severity,
        "blocking"
    );
    f.scope = Some(ReviewScope::Unresolved {
        finding_key: "F1".into(),
    });
    assert_eq!(
        judge(&f, 2, DeliveryTier::Required, &["F1"]).severity,
        "blocking"
    );
    f.scope = Some(ReviewScope::Unresolved {
        finding_key: "F9".into(),
    });
    assert_eq!(
        judge(&f, 2, DeliveryTier::Required, &["F1"]).disposition,
        "deferred"
    );
    // A stated risk is never out of scope, and a new non-blocking finding is deferred anyway.
    let mut risk = finding("F8");
    risk.risk = Some(Risk {
        kind: RiskKind::Security,
        reasoning: "The token is written to the log".into(),
    });
    assert_eq!(
        judge(&risk, 3, DeliveryTier::Required, &[]).severity,
        "blocking"
    );
    let new_style = judge(&finding("F10"), 2, DeliveryTier::Required, &["F1"]);
    assert_eq!(new_style.disposition, "deferred");
}

#[test]
fn findings_are_bounded_and_the_wire_format_is_closed() {
    let mut f = finding("F11");
    f.finding_key = " padded".into();
    assert!(f.validate().is_err());
    let mut f = finding("F11");
    f.summary = "x".repeat(2049);
    assert!(f.validate().is_err());
    let mut f = finding("F11");
    f.counterexample = Some(Counterexample {
        given: "g".into(),
        when: "".into(),
        then: "t".into(),
    });
    assert!(f.validate().is_err());
    // A key is a name: no control character in it, and a scope names a finding that is a name too.
    let mut f = finding("F11");
    f.finding_key = "bell\u{7}".into();
    assert!(f.validate().is_err());
    let mut f = finding("F11");
    f.scope = Some(ReviewScope::Unresolved {
        finding_key: " ".into(),
    });
    assert!(f.validate().is_err());
    let mut f = finding("F11");
    f.scope = Some(ReviewScope::Unresolved {
        finding_key: "F10".into(),
    });
    assert!(f.validate().is_ok());
    let wire = serde_json::json!({"finding_key": "F1", "summary": "s", "severity": "blocking"});
    assert!(
        serde_json::from_value::<ReviewFinding>(wire).is_err(),
        "severity is derived, never stated"
    );
    let scoped = serde_json::json!({"finding_key": "F1", "summary": "s",
        "scope": {"kind": "unresolved", "finding_key": "F0"}});
    assert!(serde_json::from_value::<ReviewFinding>(scoped).is_ok());
}
