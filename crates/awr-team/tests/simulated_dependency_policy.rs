use awr_team::{
    CandidateState, DependencyAcceptanceMode as Mode, DraftChange, DraftDefinitionState,
    DraftOpKind, ExecutionSettlementMode, ExecutionSettlementPolicy as Settlement, PLANNING_CODEC,
    PLANNING_CODEC_V2, PLANNING_CODEC_V3, PLANNING_CODEC_V4, PLANNING_CODEC_V5, PlanningApproval,
    PlanningCandidate, TaskDraft, WorkContract, WorkstreamBundle, build_candidate_diff,
    edit_candidate, planning_codec_for_changes,
};
use serde_json::{Value, json};

fn legacy() -> WorkstreamBundle {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/team-mcp/publish-prep/baseline-workstreams.json"
    ))
    .unwrap()
}

fn consumer() -> WorkContract {
    let mut contract = legacy().contracts.remove(1).contract;
    contract.codec = WorkContract::CODEC_V5.into();
    contract
        .dependency_acceptance
        .insert("API-1".into(), Mode::SimulatedMemberIndependent);
    contract
}

fn settlement() -> Settlement {
    Settlement {
        mode: ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "worker-c".into(),
    }
}

#[test]
fn dependency_assurance_is_independent_of_the_consumers_own_completion_policy() {
    for policy in [
        "independent_review",
        "ordinary_confirm",
        Settlement::COMPLETION_POLICY,
        Settlement::SIMULATED_MEMBER_COMPLETION_POLICY,
    ] {
        let mut contract = consumer();
        contract.completion_policy = policy.into();
        if policy == Settlement::SIMULATED_MEMBER_COMPLETION_POLICY {
            contract.execution_settlement = Some(settlement());
            contract.verification_requirements = vec!["Verify integration bytes".into()];
        }
        contract.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<WorkContract>(json!(contract)).unwrap(),
            contract
        );
        let before = contract.hash().unwrap();
        contract.required_dependencies.push("other".into());
        assert_ne!(contract.hash().unwrap(), before);
        let before = contract.hash().unwrap();
        contract
            .dependency_acceptance
            .insert("other".into(), Mode::AgentReviewedCallerAssertedReconciled);
        assert_ne!(contract.hash().unwrap(), before);
    }
    let mut contract = consumer();
    contract.completion_policy = Settlement::SIMULATED_MEMBER_COMPLETION_POLICY.into();
    assert!(contract.validate().is_err());
    contract.execution_settlement = Some(settlement());
    assert!(contract.validate().is_err());
    contract.verification_requirements = vec!["Verify integration bytes".into()];
    contract.validate().unwrap();
    contract.completion_policy = "independent_review".into();
    assert!(contract.validate().is_err());
}

#[test]
fn simulated_dependency_requires_explicit_v5_and_closed_unique_required_edges() {
    for codec in [
        WorkContract::CODEC,
        WorkContract::CODEC_V2,
        WorkContract::CODEC_V3,
        WorkContract::CODEC_V4,
    ] {
        let mut contract = consumer();
        contract.codec = codec.into();
        assert!(contract.hash().is_err());
        assert!(serde_json::from_value::<WorkContract>(json!(contract)).is_err());
    }
    let base = json!(consumer());
    for modes in [
        Value::Null,
        json!({}),
        json!({"API-1":"unknown"}),
        json!({"absent":"simulated_member_independent"}),
        json!({"CLIENT-1":"simulated_member_independent"}),
        json!({"API-1":"agent_reviewed_caller_asserted_reconciled"}),
    ] {
        let mut raw = base.clone();
        raw["dependency_acceptance"] = modes;
        assert!(serde_json::from_value::<WorkContract>(raw).is_err());
    }
    let mut raw = base.clone();
    raw.as_object_mut().unwrap().remove("dependency_acceptance");
    assert!(serde_json::from_value::<WorkContract>(raw).is_err());
    let duplicate = base.to_string().replace(
        "\"API-1\":\"simulated_member_independent\"",
        "\"API-1\":\"simulated_member_independent\",\"API-1\":\"simulated_member_independent\"",
    );
    assert!(serde_json::from_str::<WorkContract>(&duplicate).is_err());
    for (field, value) in [
        ("required_dependencies", json!(["API-1", "API-1"])),
        ("execution_settlement", Value::Null),
        ("codec", json!("future")),
        ("human_approval", json!(true)),
    ] {
        let mut raw = base.clone();
        raw[field] = value;
        assert!(serde_json::from_value::<WorkContract>(raw).is_err());
    }
}

#[test]
fn v5_bundle_keeps_existing_hashes_and_the_same_stream_boundary() {
    let mut bundle = legacy();
    let upstream = bundle.contracts[0].contract.clone();
    let old_hash = upstream.hash().unwrap();
    bundle.contracts[1].contract = consumer();
    bundle.contracts[1].workstream_id = bundle.contracts[0].workstream_id;
    for codec in [
        WorkstreamBundle::CODEC,
        WorkstreamBundle::CODEC_V2,
        WorkstreamBundle::CODEC_V3,
        WorkstreamBundle::CODEC_V4,
    ] {
        bundle.codec = codec.into();
        assert!(bundle.validate("demo-project").is_err());
    }
    bundle.codec = WorkstreamBundle::CODEC_V5.into();
    bundle.validate("demo-project").unwrap();
    let loaded: WorkstreamBundle = serde_json::from_value(json!(bundle)).unwrap();
    assert_eq!(loaded.hash().unwrap(), bundle.hash().unwrap());
    assert_eq!(loaded.contracts[0].contract, upstream);
    assert_eq!(loaded.contracts[0].contract.hash().unwrap(), old_hash);
    bundle.contracts[0].workstream_id = awr_core::Id::from(2);
    assert!(bundle.validate("demo-project").is_err());
}

fn draft() -> TaskDraft {
    TaskDraft {
        work_id: "CLIENT-1".into(),
        external_key: "CLIENT-1".into(),
        title: "Integrate accepted API".into(),
        goals: vec!["delivery".into()],
        scope_paths: vec!["src/client".into()],
        acceptance: vec!["Integration verified".into()],
        required_dependencies: vec!["API-1".into()],
        completion_policy: "independent_review".into(),
        dependency_acceptance: Some(consumer().dependency_acceptance),
        hard_rules: None,
        verification_requirements: None,
        execution_settlement: None,
        definition_state: DraftDefinitionState::Enabled,
        workstream: Some("api".into()),
        split_from: None,
        split_children: vec![],
    }
}

#[test]
fn planning_v5_requires_reviewed_per_edge_changes_and_invalidates_old_approval() {
    let changes = vec![DraftChange {
        op: DraftOpKind::CreateTask,
        before: None,
        after: draft(),
    }];
    let mut candidate = PlanningCandidate {
        codec: planning_codec_for_changes(&changes).into(),
        candidate_id: "candidate".into(),
        project_id: "demo-project".into(),
        author_person_id: "planner".into(),
        author_actor_id: "agent-planner".into(),
        baseline_digest: "sha256:baseline".into(),
        baseline_epoch: "1".into(),
        draft_revision: 1,
        changes,
        suggestion_ids: vec![],
        state: CandidateState::Drafting,
        approval: None,
        allowed_spec_roots: vec!["src".into()],
        project_goal_keys: vec!["delivery".into()],
    };
    candidate.validate_structure().unwrap();
    assert_eq!(candidate.codec, PLANNING_CODEC_V5);
    let digest = candidate.candidate_digest().unwrap();
    for codec in [
        PLANNING_CODEC,
        PLANNING_CODEC_V2,
        PLANNING_CODEC_V3,
        PLANNING_CODEC_V4,
    ] {
        let mut old = candidate.clone();
        old.codec = codec.into();
        assert!(old.candidate_digest().is_err());
    }
    candidate.state = CandidateState::Approved;
    candidate.approval = Some(PlanningApproval {
        approval_id: "approval".into(),
        candidate_digest: digest.clone(),
        approver_person_id: "reviewer".into(),
        approver_actor_id: "agent-reviewer".into(),
        self_approved: false,
    });
    let before = draft();
    let mut after = before.clone();
    after
        .dependency_acceptance
        .as_mut()
        .unwrap()
        .insert("API-1".into(), Mode::AgentReviewedCallerAssertedReconciled);
    let edited = edit_candidate(
        candidate.clone(),
        vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(before.clone()),
            after: after.clone(),
        }],
    )
    .unwrap();
    assert_eq!(edited.codec, PLANNING_CODEC_V5);
    assert_eq!(edited.state, CandidateState::Drafting);
    assert!(edited.approval.is_none());
    assert_ne!(edited.candidate_digest().unwrap(), digest);
    let diff = build_candidate_diff(&edited, vec![]).unwrap();
    assert!(
        diff.field_diffs
            .iter()
            .any(|d| d.field == "dependency_acceptance")
    );
    assert!(
        diff.review_requirements
            .contains(&"explicit_dependency_assurance_review".into())
    );
    after.dependency_acceptance = None;
    let retained = edit_candidate(
        candidate,
        vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(before),
            after,
        }],
    )
    .unwrap();
    assert!(
        !build_candidate_diff(&retained, vec![])
            .unwrap()
            .field_diffs
            .iter()
            .any(|d| d.field == "dependency_acceptance")
    );
}
