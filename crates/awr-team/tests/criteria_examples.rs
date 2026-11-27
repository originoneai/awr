use awr_core::ExampleKind;
use awr_team::{
    CandidateState, ContractExample, DraftChange, DraftDefinitionState, DraftOpKind,
    PLANNING_CODEC_V7, PlanningCandidate, TaskDraft, WorkContract, WorkId, WorkstreamBundle,
    WorkstreamContract, build_candidate_diff, planning_codec_for_changes,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn example(
    id: Option<&str>,
    criterion: &str,
    kind: ExampleKind,
    check_id: Option<&str>,
) -> ContractExample {
    ContractExample {
        id: id.map(str::to_owned),
        criterion: criterion.into(),
        kind,
        given: "stored records exist".into(),
        when: "filter executes".into(),
        then: "matching records return without mutating input".into(),
        check_id: check_id.map(str::to_owned),
    }
}

fn v7_contract(examples: Vec<ContractExample>) -> WorkContract {
    WorkContract {
        codec: WorkContract::CODEC_V7.into(),
        work_id: WorkId::new("work-1").unwrap(),
        external_key: "WORK-1".into(),
        goals: vec!["G-1".into()],
        hard_rules: vec!["Keep inputs immutable".into()],
        scope_paths: vec!["src/filter.rs".into()],
        acceptance: vec![
            "Filter matches status and keyword".into(),
            "Empty query returns all records".into(),
        ],
        required_dependencies: vec![],
        completion_policy: "independent_review".into(),
        verification_requirements: vec!["unit.filter".into()],
        dependency_acceptance: BTreeMap::new(),
        execution_settlement: None,
        examples: Some(examples),
    }
}

fn task_draft(examples: Option<Vec<ContractExample>>) -> TaskDraft {
    TaskDraft {
        work_id: "work-1".into(),
        external_key: "WORK-1".into(),
        title: "Filter API".into(),
        goals: vec!["G-1".into()],
        scope_paths: vec!["src/filter.rs".into()],
        acceptance: vec![
            "Filter matches status and keyword".into(),
            "Empty query returns all records".into(),
        ],
        required_dependencies: vec![],
        completion_policy: "independent_review".into(),
        dependency_acceptance: None,
        hard_rules: Some(vec!["Keep inputs immutable".into()]),
        verification_requirements: Some(vec!["unit.filter".into()]),
        execution_settlement: None,
        examples,
        definition_state: DraftDefinitionState::Enabled,
        workstream: Some("backend".into()),
        split_from: None,
        split_children: vec![],
    }
}

#[test]
fn v7_contract_examples_round_trip_and_change_hash_when_binding_changes() {
    let pos = example(
        Some("filter-pos"),
        "Filter matches status and keyword",
        ExampleKind::Positive,
        Some("unit.filter.pos"),
    );
    let neg = example(
        Some("filter-neg"),
        "Filter matches status and keyword",
        ExampleKind::Negative,
        Some("unit.filter.neg"),
    );
    let first = v7_contract(vec![pos.clone(), neg.clone()]);
    let reordered = v7_contract(vec![neg.clone(), pos.clone()]);
    assert_eq!(
        first.hash().unwrap(),
        reordered.hash().unwrap(),
        "reordering identical examples keeps the canonical contract hash stable"
    );

    let wire = json!(first);
    let decoded: WorkContract = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded, first);

    let mut rebound = first.clone();
    rebound.examples.as_mut().unwrap()[0].check_id = Some("unit.filter.v2".into());
    assert_ne!(
        first.hash().unwrap(),
        rebound.hash().unwrap(),
        "changing a bound check_id must change contract_hash to invalidate prior evidence"
    );
}

#[test]
fn legacy_codecs_reject_examples_and_v7_rejects_invalid_examples() {
    let mut legacy = v7_contract(vec![]);
    legacy.codec = WorkContract::CODEC.into();
    assert!(legacy.validate().is_err());

    let mut wire = json!(v7_contract(vec![]));
    wire["codec"] = json!(WorkContract::CODEC);
    assert!(serde_json::from_value::<WorkContract>(wire).is_err());

    let mut null_examples = json!(v7_contract(vec![]));
    null_examples["examples"] = Value::Null;
    assert!(serde_json::from_value::<WorkContract>(null_examples).is_err());

    // Dangling criterion reference is refused.
    let unknown_criterion = v7_contract(vec![example(
        Some("bad-crit"),
        "Criterion not in acceptance",
        ExampleKind::Positive,
        Some("unit.check"),
    )]);
    assert!(unknown_criterion.validate().is_err());

    // Duplicate example id is refused.
    let dup_id = v7_contract(vec![
        example(
            Some("same-id"),
            "Filter matches status and keyword",
            ExampleKind::Positive,
            Some("unit.a"),
        ),
        example(
            Some("same-id"),
            "Filter matches status and keyword",
            ExampleKind::Negative,
            Some("unit.b"),
        ),
    ]);
    assert!(dup_id.validate().is_err());

    // Invalid id pattern (uppercase) and invalid check_id (control char / blank) are refused.
    let bad_id = v7_contract(vec![example(
        Some("UPPER"),
        "Filter matches status and keyword",
        ExampleKind::Positive,
        Some("unit.a"),
    )]);
    assert!(bad_id.validate().is_err());

    let bad_check = v7_contract(vec![example(
        Some("ok-id"),
        "Filter matches status and keyword",
        ExampleKind::Positive,
        Some("  "),
    )]);
    assert!(bad_check.validate().is_err());

    // Setting check_id without id is refused on both WorkContract and TaskDraft
    // (A-evidence §3: examples that bind a check require an explicit id).
    let check_without_id = example(
        None,
        "Filter matches status and keyword",
        ExampleKind::Positive,
        Some("unit.filter.pos"),
    );
    assert!(
        v7_contract(vec![check_without_id.clone()])
            .validate()
            .is_err()
    );
    assert!(task_draft(Some(vec![check_without_id])).validate().is_err());

    // Unbound examples may still omit id, and an explicit empty V7 examples list is valid.
    let unbound_without_id = example(
        None,
        "Empty query returns all records",
        ExampleKind::Positive,
        None,
    );
    v7_contract(vec![unbound_without_id.clone()])
        .validate()
        .unwrap();
    task_draft(Some(vec![unbound_without_id]))
        .validate()
        .unwrap();
    v7_contract(vec![]).validate().unwrap();
}

#[test]
fn planning_candidate_uses_v7_codec_and_diffs_examples() {
    let before = task_draft(None);
    let after = task_draft(Some(vec![
        example(
            Some("filter-pos"),
            "Filter matches status and keyword",
            ExampleKind::Positive,
            Some("unit.filter.pos"),
        ),
        example(
            Some("filter-neg"),
            "Filter matches status and keyword",
            ExampleKind::Negative,
            Some("unit.filter.neg"),
        ),
    ]));
    let changes = vec![DraftChange {
        op: DraftOpKind::EditFields,
        before: Some(before),
        after,
    }];
    assert_eq!(planning_codec_for_changes(&changes), PLANNING_CODEC_V7);
    let candidate = PlanningCandidate {
        codec: PLANNING_CODEC_V7.into(),
        candidate_id: "cand-v7".into(),
        project_id: "proj-1".into(),
        author_person_id: "alice".into(),
        author_actor_id: "alice".into(),
        baseline_digest: "sha256:baseline".into(),
        baseline_epoch: "1".into(),
        draft_revision: 1,
        changes,
        suggestion_ids: vec![],
        state: CandidateState::Drafting,
        approval: None,
        allowed_spec_roots: vec!["src".into()],
        project_goal_keys: vec!["G-1".into()],
    };
    let diff = build_candidate_diff(&candidate, vec![]).unwrap();
    assert!(diff.field_diffs.iter().any(|d| d.field == "examples"));
    assert!(
        diff.review_requirements
            .iter()
            .any(|r| r == "acceptance_examples_review")
    );
}

#[test]
fn workstream_bundle_v7_validates_v7_contracts_and_rejects_in_v6() {
    let mut bundle: WorkstreamBundle = serde_json::from_str(include_str!(
        "../../../tests/fixtures/team-mcp/publish-prep/baseline-workstreams.json"
    ))
    .unwrap();
    let ws_id = bundle.contracts[0].workstream_id;
    let contract = v7_contract(vec![example(
        Some("filter-pos"),
        "Filter matches status and keyword",
        ExampleKind::Positive,
        Some("unit.filter.pos"),
    )]);
    bundle.contracts = vec![WorkstreamContract {
        workstream_id: ws_id,
        contract,
    }];
    bundle.codec = WorkstreamBundle::CODEC_V6.into();
    assert!(bundle.validate(&bundle.catalog.project_id).is_err());

    bundle.codec = WorkstreamBundle::CODEC_V7.into();
    bundle.validate(&bundle.catalog.project_id).unwrap();
    assert!(!bundle.hash().unwrap().is_empty());
}
