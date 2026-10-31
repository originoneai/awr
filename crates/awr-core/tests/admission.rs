//! Positive and negative examples for the admission policy, the request shapes and the pure
//! evaluator. Each test name states the assertion; the examples come from the contract.
use awr_core::*;
use serde_json::{Value, json};

const A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn policy(value: Value) -> AdmissionPolicy {
    serde_json::from_value(value).expect("a policy that matches the schema")
}

fn governed() -> AdmissionPolicy {
    policy(
        json!({"version": 1, "mode": "governed_enforce", "scope": "all",
                  "independent_review_tags": ["security"]}),
    )
}

fn tags(list: &[&str]) -> Vec<String> {
    list.iter().map(|t| t.to_string()).collect()
}

// ---------------------------------------------------------------- policy

#[test]
fn a_governed_policy_needs_a_known_version_and_an_explicit_scope() {
    assert!(governed().validate().is_ok());
    let observe = policy(json!({"version": 1, "mode": "legacy_observe"}));
    assert!(observe.validate().is_ok(), "observe needs no scope");
    assert!(!observe.enforced() && governed().enforced());

    let no_scope = policy(json!({"version": 1, "mode": "governed_enforce"}));
    assert!(
        no_scope.validate().is_err(),
        "governing needs an explicit scope"
    );
    let future = policy(json!({"version": 2, "mode": "legacy_observe"}));
    assert!(
        future.validate().is_err(),
        "an unknown version is unusable, not downgraded"
    );
    let empty_tags =
        policy(json!({"version": 1, "mode": "governed_enforce", "scope": {"tags": []}}));
    assert!(empty_tags.validate().is_err());
    let blank = policy(json!({"version": 1, "mode": "governed_enforce", "scope": {"tags": [" "]}}));
    assert!(blank.validate().is_err());
    let too_many = policy(json!({"version": 1, "mode": "legacy_observe",
        "independent_review_tags": (0..33).map(|i| format!("t{i}")).collect::<Vec<_>>()}));
    assert!(too_many.validate().is_err());
    // Unknown fields and unknown modes are refused at parse time.
    assert!(
        serde_json::from_value::<AdmissionPolicy>(
            json!({"version": 1, "mode": "legacy_observe", "extra": 1})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<AdmissionPolicy>(json!({"version": 1, "mode": "strict"})).is_err()
    );
}

#[test]
fn scope_and_review_tags_select_the_governed_work() {
    let all = governed();
    assert!(all.governs(&tags(&[])) && all.governs(&tags(&["x"])));
    let by_tag =
        policy(json!({"version": 1, "mode": "governed_enforce", "scope": {"tags": ["core"]}}));
    assert!(by_tag.governs(&tags(&["core", "x"])));
    assert!(!by_tag.governs(&tags(&["x"])) && !by_tag.governs(&tags(&[])));
    let none = policy(json!({"version": 1, "mode": "legacy_observe"}));
    assert!(
        !none.governs(&tags(&["core"])),
        "a missing scope governs nothing"
    );
    assert!(all.requires_independent_review(&tags(&["security"])));
    assert!(!all.requires_independent_review(&tags(&["docs"])));
}

// ---------------------------------------------------------------- request

fn judgment() -> Value {
    json!({"contribution": "Advances the saving outcome", "necessity": "The draft is lost today",
           "alternatives": "Reuse of the existing buffer was checked", "deliverable": "A fallback path",
           "exit": "Stop when the fallback is verified", "responsibility": "storage-module"})
}

fn request(decision: &str, edit: impl FnOnce(&mut Value)) -> Result<AdmissionRequest> {
    let mut value = json!({"work": "W", "session": Id::new(), "expected_revision": 3,
        "request_key": "k1", "decision": decision, "contract_fingerprint": A});
    edit(&mut value);
    let parsed: AdmissionRequest = serde_json::from_value(value)?;
    parsed.validate()?;
    Ok(parsed)
}

#[test]
fn each_decision_has_its_own_request_shape() {
    // A proposal needs the judgment and answers nothing.
    assert!(request("proposed", |v| v["judgment"] = judgment()).is_ok());
    assert!(request("proposed", |_| {}).is_err());
    assert!(
        request("proposed", |v| {
            v["judgment"] = judgment();
            v["proposal_event"] = json!(Id::new());
        })
        .is_err()
    );
    // An approval is either inline with its judgment or answers a proposal, never both.
    assert!(request("approved", |v| v["judgment"] = judgment()).is_ok());
    assert!(request("approved", |v| v["proposal_event"] = json!(Id::new())).is_ok());
    assert!(request("approved", |_| {}).is_err());
    assert!(
        request("approved", |v| {
            v["judgment"] = judgment();
            v["proposal_event"] = json!(Id::new());
        })
        .is_err()
    );
    // A denial needs a reason and carries no judgment.
    assert!(
        request("denied", |v| v["reason"] =
            json!("Not needed for this increment"))
        .is_ok()
    );
    assert!(request("denied", |_| {}).is_err());
    assert!(
        request("denied", |v| {
            v["reason"] = json!("No");
            v["judgment"] = judgment();
        })
        .is_err()
    );
}

#[test]
fn a_request_names_exactly_one_of_a_session_or_a_declaring_agent() {
    let with = |edit: fn(&mut Value)| {
        request("proposed", |v| {
            v["judgment"] = judgment();
            edit(v);
        })
    };
    assert!(with(|_| {}).is_ok(), "a session names who decides");
    assert!(
        with(|v| {
            v.as_object_mut().unwrap().remove("session");
            v["agent"] = json!("claude-run");
        })
        .is_ok()
    );
    assert!(
        with(|v| {
            v.as_object_mut().unwrap().remove("session");
        })
        .is_err()
    );
    assert!(with(|v| v["agent"] = json!("claude-run")).is_err());
    assert!(
        with(|v| {
            v.as_object_mut().unwrap().remove("session");
            v["agent"] = json!(" ");
        })
        .is_err()
    );
    assert!(
        with(|v| {
            v.as_object_mut().unwrap().remove("session");
            v["agent"] = json!("a".repeat(257));
        })
        .is_err()
    );
}

#[test]
fn request_content_is_bounded_attributed_and_public() {
    let ok = |edit: fn(&mut Value)| {
        request("proposed", |v| {
            v["judgment"] = judgment();
            edit(v);
        })
    };
    assert!(ok(|_| {}).is_ok());
    assert!(ok(|v| v["request_key"] = json!(" ")).is_err());
    assert!(ok(|v| v["request_key"] = json!("k".repeat(513))).is_err());
    assert!(ok(|v| v["contract_fingerprint"] = json!("not-a-fingerprint")).is_err());
    assert!(ok(|v| v["judgment"]["exit"] = json!("")).is_err());
    assert!(ok(|v| v["judgment"]["necessity"] = json!("x".repeat(2049))).is_err());
    assert!(ok(|v| v["judgment"]["alternatives"] = json!("password: hunter2hunter2")).is_err());
    assert!(serde_json::from_value::<AdmissionRequest>(json!({"work": "W", "extra": 1})).is_err());
}

// ---------------------------------------------------------------- evaluator

fn binding(contract: &str) -> AdmissionBinding {
    AdmissionBinding {
        contract_fingerprint: contract.into(),
        definition_fingerprint: Some(A.into()),
        examples_fingerprint: Some(A.into()),
        policy_fingerprint: A.into(),
        purpose: Some(WorkPurpose::Implementation),
        tier: Some(DeliveryTier::Required),
    }
}

fn report(state: &'static str, gaps: &[&'static str], intent_ok: bool) -> DefinitionReport {
    DefinitionReport {
        work: "W".into(),
        state,
        purpose: Some(WorkPurpose::Implementation),
        effective_tier: Some(DeliveryTier::Required),
        obligations: vec![ObligationStatus {
            obligation: "intent",
            state: if intent_ok { "satisfied" } else { "missing" },
            source: None,
        }],
        gaps: gaps
            .iter()
            .map(|code| DefinitionGap {
                code,
                detail: String::new(),
            })
            .collect(),
        notes: vec![],
        criterion_tiers: vec![],
        basis: "test",
    }
}

fn recorded(decision: AdmissionDecision, bound: AdmissionBinding) -> RecordedAdmission {
    RecordedAdmission {
        event: Id::new(),
        decision,
        bound,
        reason: Some("Out of scope for this increment".into()),
        proposer_session: None,
        decider_session: Some(Id::new()),
    }
}

struct Case {
    policy: Option<AdmissionPolicy>,
    tags: Vec<String>,
    completed: bool,
    current: AdmissionBinding,
    latest: Option<RecordedAdmission>,
    definition: DefinitionReport,
}

impl Case {
    fn new() -> Self {
        Self {
            policy: Some(governed()),
            tags: vec![],
            completed: false,
            current: binding(A),
            latest: None,
            definition: report("complete", &[], true),
        }
    }
    fn status(&self) -> AdmissionStatus {
        evaluate_admission(&AdmissionInputs {
            work: "W",
            tags: &self.tags,
            completed: self.completed,
            policy: self.policy.as_ref(),
            current: &self.current,
            latest: self.latest.as_ref(),
            definition: &DefinitionSummary::from(&self.definition),
        })
    }
}

fn codes(status: &AdmissionStatus) -> Vec<&'static str> {
    status.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn work_outside_the_policy_is_not_governed_and_owes_nothing() {
    let mut none = Case::new();
    none.policy = None;
    let status = none.status();
    assert_eq!(
        (status.state, status.mode, status.governed),
        ("not_governed", "not_governed", false)
    );
    assert!(status.diagnostics.is_empty() && status.current.is_none());

    let mut tagged = Case::new();
    tagged.policy = Some(policy(
        json!({"version": 1, "mode": "governed_enforce", "scope": {"tags": ["core"]}}),
    ));
    tagged.tags = tags(&["docs"]);
    let outside = tagged.status();
    assert!(!outside.governed && !outside.enforced && outside.diagnostics.is_empty());
    tagged.tags = tags(&["core"]);
    let inside = tagged.status();
    assert!(inside.governed && inside.enforced);
    assert_eq!(codes(&inside), ["admission_missing"]);
}

#[test]
fn observe_reports_the_state_without_enforcing_it() {
    let mut case = Case::new();
    case.policy = Some(policy(
        json!({"version": 1, "mode": "legacy_observe", "scope": "all"}),
    ));
    let status = case.status();
    assert!(status.governed && !status.enforced);
    assert_eq!(status.mode, "legacy_observe");
    assert_eq!(
        codes(&status),
        ["admission_missing"],
        "the gap is reported, not hidden"
    );
}

#[test]
fn missing_pending_denied_stale_and_approved_each_answer_with_their_own_code() {
    let mut case = Case::new();
    assert_eq!(
        (case.status().state, codes(&case.status())),
        ("missing", vec!["admission_missing"])
    );

    case.latest = Some(recorded(AdmissionDecision::Proposed, binding(A)));
    assert_eq!(
        (case.status().state, codes(&case.status())),
        ("pending", vec!["admission_pending"])
    );
    assert!(
        !case.status().facts.admitted,
        "a request is not an obtained decision"
    );

    case.latest = Some(recorded(AdmissionDecision::Denied, binding(A)));
    let denied = case.status();
    assert_eq!(
        (denied.state, codes(&denied)),
        ("denied", vec!["admission_denied"])
    );
    assert!(
        denied.diagnostics[0]
            .detail
            .contains("Out of scope for this increment")
    );

    case.latest = Some(recorded(AdmissionDecision::Approved, binding(A)));
    let approved = case.status();
    assert_eq!(approved.state, "approved");
    assert!(approved.diagnostics.is_empty() && approved.facts.admitted);
}

#[test]
fn an_approval_goes_stale_for_each_binding_that_changed() {
    for (label, edit) in [
        (
            "contract",
            (|b: &mut AdmissionBinding| b.contract_fingerprint = B.into())
                as fn(&mut AdmissionBinding),
        ),
        ("definition", |b| b.definition_fingerprint = Some(B.into())),
        ("examples", |b| b.examples_fingerprint = Some(B.into())),
        ("policy", |b| b.policy_fingerprint = B.into()),
    ] {
        let mut case = Case::new();
        let mut now = binding(A);
        edit(&mut now);
        case.current = now;
        case.latest = Some(recorded(AdmissionDecision::Approved, binding(A)));
        let status = case.status();
        assert_eq!(status.state, "stale", "{label}");
        let detail = &status.diagnostics[0].detail;
        assert!(detail.contains(label), "{label}: {detail}");
        for other in ["contract", "definition", "examples", "policy"]
            .into_iter()
            .filter(|o| *o != label)
        {
            assert!(
                !detail.contains(other),
                "{label} also named {other}: {detail}"
            );
        }
        assert!(!status.facts.admitted);
    }
    // Purpose and tier are recorded for the reader but are not separate bindings: they come from the definition.
    let mut case = Case::new();
    case.current.tier = Some(DeliveryTier::Later);
    case.latest = Some(recorded(AdmissionDecision::Approved, binding(A)));
    assert_eq!(case.status().state, "approved");
}

#[test]
fn entry_obligations_follow_the_declared_purpose_and_undeclared_work_is_an_implementation() {
    let mut case = Case::new();
    case.latest = Some(recorded(AdmissionDecision::Approved, binding(A)));
    // Gaps of the declared purpose block even an approved, current decision.
    case.definition = report(
        "gaps",
        &["examples_missing", "discrimination_missing"],
        true,
    );
    let blocked = case.status();
    assert_eq!(blocked.state, "approved");
    assert_eq!(codes(&blocked), ["admission_obligations_unmet"]);
    assert!(blocked.diagnostics[0].detail.contains("examples_missing"));
    assert!(
        blocked.diagnostics[0]
            .detail
            .contains("discrimination_missing")
    );
    assert!(!blocked.facts.admitted);
    // The same decision is enough when the purpose leaves no gap.
    case.definition = report("complete", &[], true);
    assert!(case.status().facts.admitted);
    // Work that declares no definition cannot lower its own bar by saying nothing.
    case.definition = report("not_declared", &[], false);
    let undeclared = case.status();
    assert_eq!(codes(&undeclared), ["admission_obligations_unmet"]);
    assert!(
        undeclared.diagnostics[0]
            .detail
            .contains("definition_not_declared")
    );
}

#[test]
fn an_unusable_policy_fails_closed_instead_of_governing_nothing() {
    let status = AdmissionStatus::policy_invalid("W");
    assert_eq!(
        (status.state, status.mode, status.enforced),
        ("policy_invalid", "policy_invalid", true)
    );
    assert_eq!(codes(&status), ["admission_policy_invalid"]);
    assert!(!status.facts.admitted);
    // Work no policy governs is the opposite: nothing is reported or enforced.
    let free = AdmissionStatus::ungoverned("W", "not_governed");
    assert!(!free.enforced && free.diagnostics.is_empty() && free.current.is_none());
}

#[test]
fn recording_adopting_admitting_and_completing_stay_separate_facts() {
    let mut case = Case::new();
    case.definition = report("complete", &[], true);
    let before = case.status();
    assert!(
        before.facts.recorded
            && before.facts.adopted
            && !before.facts.admitted
            && !before.facts.completed
    );

    case.latest = Some(recorded(AdmissionDecision::Approved, binding(A)));
    let admitted = case.status();
    assert!(admitted.facts.admitted && !admitted.facts.completed);

    // Completion is its own fact: it neither grants admission nor needs it to be reported.
    let mut done = Case::new();
    done.completed = true;
    done.definition = report("complete", &[], false);
    let status = done.status();
    assert!(status.facts.completed && !status.facts.admitted && !status.facts.adopted);
}

#[test]
fn independent_review_is_required_only_for_tagged_governed_work() {
    let mut case = Case::new();
    case.tags = tags(&["security"]);
    assert!(case.status().independent_review_required);
    case.tags = tags(&["docs"]);
    assert!(!case.status().independent_review_required);
    case.policy = None;
    case.tags = tags(&["security"]);
    assert!(
        !case.status().independent_review_required,
        "no policy, no review rule"
    );
}

fn declared_definition() -> WorkDefinition {
    serde_json::from_value(json!({
        "version": 1, "purpose": "research", "tier": "required",
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": A}],
        "contributes": [{"goal": "G", "outcome": "O", "claim": "Maps the failure modes", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One answer", "by": "agent-a"},
        "deliverable": "A list of failure modes"
    }))
    .unwrap()
}

#[test]
fn every_store_computes_the_same_binding_and_a_change_in_any_part_shows() {
    let policy = json!({"version": 1, "mode": "governed_enforce", "scope": "all"});
    let definition = declared_definition();
    let bound = admission_binding(A, &policy, Some(&definition)).unwrap();
    assert_eq!(bound.contract_fingerprint, A);
    assert_eq!(bound.purpose, Some(WorkPurpose::Research));
    assert_eq!(bound.tier, Some(DeliveryTier::Required));
    assert!(bound.definition_fingerprint.is_some() && bound.examples_fingerprint.is_some());
    // Without a declared definition nothing of it is bound.
    let bare = admission_binding(A, &policy, None).unwrap();
    assert_eq!(
        (
            &bare.definition_fingerprint,
            &bare.examples_fingerprint,
            bare.purpose,
            bare.tier
        ),
        (&None, &None, None, None)
    );
    // The same inputs always give the same binding.
    assert_eq!(
        bound,
        admission_binding(A, &policy, Some(&definition)).unwrap()
    );
    // Each part shows under its own name when it changes.
    let other_policy = json!({"version": 1, "mode": "legacy_observe", "scope": "all"});
    assert_eq!(
        bound.changed_from(&admission_binding(A, &other_policy, Some(&definition)).unwrap()),
        ["policy"]
    );
    assert_eq!(
        bound.changed_from(&admission_binding("sha256:other", &policy, Some(&definition)).unwrap()),
        ["contract"]
    );
    let mut edited = definition.clone();
    edited.deliverable = Some("A different deliverable".into());
    assert_eq!(
        bound.changed_from(&admission_binding(A, &policy, Some(&edited)).unwrap()),
        ["definition"]
    );
    assert_eq!(bound.changed_from(&bare), ["definition", "examples"]);
    // The reviewed examples are bound as a set of their own beside the definition that holds them.
    let mut reviewed = definition.clone();
    reviewed.examples.push(Example {
        criterion: "Saving failure keeps the user's input".into(),
        kind: ExampleKind::Positive,
        given: "the storage is read-only".into(),
        when: "the user saves".into(),
        then: "the draft stays in the editor".into(),
    });
    assert_eq!(
        bound.changed_from(&admission_binding(A, &policy, Some(&reviewed)).unwrap()),
        ["definition", "examples"]
    );
}

#[test]
fn a_definition_summary_keeps_what_admission_reads_and_survives_storage() {
    let summary =
        DefinitionSummary::from(&report("gaps", &["examples_missing", "tier_missing"], true));
    assert_eq!(
        summary,
        DefinitionSummary {
            declared: true,
            intent_adopted: true,
            gaps: vec!["examples_missing".into(), "tier_missing".into()],
        }
    );
    assert!(!DefinitionSummary::from(&report("not_declared", &[], false)).declared);
    assert!(!DefinitionSummary::from(&report("complete", &[], false)).intent_adopted);
    let stored = serde_json::to_string(&summary).unwrap();
    assert_eq!(
        serde_json::from_str::<DefinitionSummary>(&stored).unwrap(),
        summary
    );
    // A stored summary of another shape is refused rather than guessed at.
    assert!(serde_json::from_str::<DefinitionSummary>(r#"{"declared":true}"#).is_err());
}
