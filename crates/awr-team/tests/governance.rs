//! Governance in a Team snapshot: the candidate is canonical and bounded, and the Core's rules
//! judge a recorded decision against the Team's facts.
use awr_core::{
    AdmissionBinding, AdmissionDecision, AdmissionPolicy, DefinitionSummary, Id, RecordedAdmission,
    WorkDefinition,
};
use awr_team::{GOVERNANCE_VERSION, GovernanceCandidate, WorkGovernance, admission_status};
use serde_json::json;

const PIN: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH: &str = "11111111111111111111111111111111111111111111111111111111111111aa";

fn policy(value: serde_json::Value) -> AdmissionPolicy {
    serde_json::from_value(value).unwrap()
}

fn enforce() -> AdmissionPolicy {
    policy(json!({"version": 1, "mode": "governed_enforce", "scope": "all"}))
}

fn definition(deliverable: &str) -> WorkDefinition {
    serde_json::from_value(json!({
        "version": 1, "purpose": "research", "tier": "required",
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": PIN}],
        "contributes": [{"goal": "G", "outcome": "O", "claim": "Maps the failure modes", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One answer", "by": "agent-a"},
        "deliverable": deliverable
    }))
    .unwrap()
}

fn complete() -> DefinitionSummary {
    DefinitionSummary {
        declared: true,
        intent_adopted: true,
        gaps: vec![],
    }
}

fn work(key: &str) -> WorkGovernance {
    WorkGovernance {
        external_key: key.into(),
        tags: vec![],
        definition: Some(definition("A list of failure modes")),
        summary: complete(),
    }
}

fn candidate(works: Vec<WorkGovernance>) -> GovernanceCandidate {
    GovernanceCandidate {
        version: GOVERNANCE_VERSION,
        policy: enforce(),
        works,
    }
}

fn decided(decision: AdmissionDecision, bound: AdmissionBinding) -> RecordedAdmission {
    RecordedAdmission {
        event: Id::new(),
        decision,
        bound,
        reason: None,
        proposer_session: None,
        decider_session: None,
    }
}

fn codes(status: &awr_core::AdmissionStatus) -> Vec<&'static str> {
    status.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn a_candidate_is_bounded_and_has_exactly_one_canonical_form() {
    let good = candidate(vec![work("A"), work("B")]);
    good.validate().unwrap();
    // Works come in the order of their keys, once each.
    assert!(candidate(vec![work("B"), work("A")]).validate().is_err());
    assert!(candidate(vec![work("A"), work("A")]).validate().is_err());
    for key in ["", " A", "A ", &"k".repeat(513)] {
        assert!(candidate(vec![work(key)]).validate().is_err(), "{key:?}");
    }
    let mut bad = work("A");
    bad.tags = vec!["ok".into(), " padded".into()];
    assert!(candidate(vec![bad]).validate().is_err());
    // An unknown version and an invalid policy are refused.
    let mut other = good.clone();
    other.version = 2;
    assert!(other.validate().is_err());
    let mut other = good.clone();
    other.policy = policy(json!({"version": 1, "mode": "governed_enforce"}));
    assert!(
        other.validate().is_err(),
        "an enforced policy needs an explicit scope"
    );
    // A summary cannot describe a definition the work does not declare.
    let mut undeclared = work("A");
    undeclared.definition = None;
    assert!(candidate(vec![undeclared.clone()]).validate().is_err());
    undeclared.summary = DefinitionSummary {
        declared: false,
        intent_adopted: false,
        gaps: vec![],
    };
    candidate(vec![undeclared]).validate().unwrap();
    // The same content has the same hash however it was built, and any change shows in it.
    let reread: GovernanceCandidate =
        serde_json::from_str(&serde_json::to_string_pretty(&good).unwrap()).unwrap();
    assert_eq!(reread, good);
    assert_eq!(reread.hash().unwrap(), good.hash().unwrap());
    let mut edited = good.clone();
    edited.works[1].definition = Some(definition("Something else"));
    assert_ne!(edited.hash().unwrap(), good.hash().unwrap());
    // The wire format is closed.
    let mut wire = serde_json::to_value(&good).unwrap();
    wire["extra"] = json!(1);
    assert!(serde_json::from_value::<GovernanceCandidate>(wire).is_err());
    assert_eq!(good.work("B").unwrap().external_key, "B");
    assert!(good.work("C").is_none());
}

#[test]
fn a_decision_admits_the_work_only_while_the_snapshot_still_matches_it() {
    let policy = enforce();
    let w = work("A");
    // Nothing is decided yet.
    let missing = admission_status(&policy, &w, HASH, false, None).unwrap();
    assert_eq!(codes(&missing), ["admission_missing"]);
    assert!(missing.enforced && !missing.facts.admitted);
    // An approval of exactly this scope admits it.
    let bound = w.binding(&policy, HASH).unwrap();
    let approved = decided(AdmissionDecision::Approved, bound.clone());
    let admitted = admission_status(&policy, &w, HASH, false, Some(&approved)).unwrap();
    assert!(codes(&admitted).is_empty(), "{admitted:?}");
    assert_eq!(admitted.state, "approved");
    assert!(admitted.facts.admitted && admitted.facts.adopted);
    // A pending proposal and a denial admit nothing.
    for (decision, code) in [
        (AdmissionDecision::Proposed, "admission_pending"),
        (AdmissionDecision::Denied, "admission_denied"),
    ] {
        let latest = decided(decision, bound.clone());
        let status = admission_status(&policy, &w, HASH, false, Some(&latest)).unwrap();
        assert_eq!(codes(&status), [code]);
    }
    // Each part of the scope makes the approval stale under its own name.
    let other_contract = "22222222222222222222222222222222222222222222222222222222222222bb";
    let stale = admission_status(&policy, &w, other_contract, false, Some(&approved)).unwrap();
    assert_eq!(codes(&stale), ["admission_stale"]);
    assert!(stale.diagnostics[0].detail.contains("contract"));
    let mut edited = w.clone();
    edited.definition = Some(definition("A different deliverable"));
    let stale = admission_status(&policy, &edited, HASH, false, Some(&approved)).unwrap();
    assert!(stale.diagnostics[0].detail.contains("definition"));
    let relaxed = self::policy(
        json!({"version": 1, "mode": "governed_enforce", "scope": "all", "independent_review_tags": ["security"]}),
    );
    let stale = admission_status(&relaxed, &w, HASH, false, Some(&approved)).unwrap();
    assert!(stale.diagnostics[0].detail.contains("policy"));
}

#[test]
fn an_approval_does_not_cover_a_definition_that_leaves_obligations_unmet() {
    let policy = enforce();
    let mut w = work("A");
    let bound = w.binding(&policy, HASH).unwrap();
    let approved = decided(AdmissionDecision::Approved, bound);
    w.summary = DefinitionSummary {
        declared: true,
        intent_adopted: false,
        gaps: vec!["intent_unpinned".into()],
    };
    let status = admission_status(&policy, &w, HASH, false, Some(&approved)).unwrap();
    assert_eq!(codes(&status), ["admission_obligations_unmet"]);
    assert!(status.diagnostics[0].detail.contains("intent_unpinned"));
    assert!(!status.facts.admitted && !status.facts.adopted);
    // Work that declares no definition owes one before it can be admitted.
    let mut bare = work("B");
    bare.definition = None;
    bare.summary = DefinitionSummary {
        declared: false,
        intent_adopted: false,
        gaps: vec![],
    };
    let status = admission_status(&policy, &bare, HASH, false, None).unwrap();
    assert!(
        status
            .diagnostics
            .iter()
            .any(|d| d.detail.contains("definition_not_declared")),
        "{status:?}"
    );
}

#[test]
fn observing_reports_without_enforcing_and_work_outside_the_scope_is_not_governed() {
    let observe = policy(json!({"version": 1, "mode": "legacy_observe", "scope": "all"}));
    let w = work("A");
    let status = admission_status(&observe, &w, HASH, false, None).unwrap();
    assert!(!status.enforced);
    assert_eq!(
        codes(&status),
        ["admission_missing"],
        "reported, not enforced"
    );
    let tagged =
        policy(json!({"version": 1, "mode": "governed_enforce", "scope": {"tags": ["governed"]}}));
    let status = admission_status(&tagged, &w, HASH, false, None).unwrap();
    assert!(!status.governed && codes(&status).is_empty());
    let mut governed = w.clone();
    governed.tags = vec!["governed".into()];
    let status = admission_status(&tagged, &governed, HASH, false, None).unwrap();
    assert!(status.governed && status.enforced);
}
