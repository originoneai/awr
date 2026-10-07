use awr_team::{
    CandidateState, DependencyAcceptanceMode, DraftChange, DraftDefinitionState, DraftOpKind,
    ExecutionSettlementMode, ExecutionSettlementPolicy, PLANNING_CODEC, PLANNING_CODEC_V2,
    PLANNING_CODEC_V3, PLANNING_CODEC_V4, PlanningApproval, PlanningCandidate, TaskDraft,
    build_candidate_diff, edit_candidate, planning_codec_for_changes,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn task() -> TaskDraft {
    TaskDraft {
        work_id: "API-1".into(),
        external_key: "API-1".into(),
        title: "Implement issue API".into(),
        goals: vec!["delivery".into()],
        scope_paths: vec!["src/api.rs".into()],
        acceptance: vec!["Issue API verified".into()],
        required_dependencies: vec!["UPSTREAM-1".into()],
        completion_policy: "independent_review".into(),
        dependency_acceptance: None,
        hard_rules: None,
        verification_requirements: None,
        execution_settlement: None,
        definition_state: DraftDefinitionState::Enabled,
        workstream: Some("backend".into()),
        split_from: None,
        split_children: vec![],
    }
}

fn candidate(changes: Vec<DraftChange>) -> PlanningCandidate {
    PlanningCandidate {
        codec: planning_codec_for_changes(&changes).into(),
        candidate_id: "candidate-golden".into(),
        project_id: "project-golden".into(),
        author_person_id: "planner-member".into(),
        author_actor_id: "planner-agent".into(),
        baseline_digest: "sha256:baseline".into(),
        baseline_epoch: "1".into(),
        draft_revision: 1,
        changes,
        suggestion_ids: vec![],
        state: CandidateState::Drafting,
        approval: None,
        allowed_spec_roots: vec!["src".into()],
        project_goal_keys: vec!["delivery".into()],
    }
}

fn create(task: TaskDraft) -> PlanningCandidate {
    candidate(vec![DraftChange {
        op: DraftOpKind::CreateTask,
        before: None,
        after: task,
    }])
}

fn settled(policy: &str) -> TaskDraft {
    let mut task = task();
    task.completion_policy = policy.into();
    task.verification_requirements = Some(vec!["Run API regression tests".into()]);
    task.execution_settlement = Some(ExecutionSettlementPolicy {
        mode: ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "workspace-a".into(),
    });
    task
}

#[test]
fn legacy_planning_bytes_and_digests_match_pre_extension_compiler_goldens() {
    let v1 = task();
    let mut v2 = v1.clone();
    v2.dependency_acceptance = Some(BTreeMap::from([(
        "UPSTREAM-1".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    )]));
    let mut v3 = v2.clone();
    v3.hard_rules = Some(vec!["Preserve issue identities".into()]);
    v3.verification_requirements = Some(vec!["Run API regression tests".into()]);
    // Captured from the unchanged V1/V2/V3 implementation with the locked compiler.
    for (task, codec, draft_sha, candidate_sha, digest) in [
        (
            v1,
            PLANNING_CODEC,
            "0162715d4c0594d6c0bc16cd1a4147444c91c0b7248b28e847701759989ef275",
            "2fc3013c7cf5ac5cbe162c54a6174fb26daa60857f9652af9bef87f7ad369b4a",
            "78ae1e8eebd504c8a03d1ddc76b247d9bf5d5d2258cb7950c0fb4c536f0f1572",
        ),
        (
            v2,
            PLANNING_CODEC_V2,
            "5a6490d3547ff2acaf9a5c864dfc93629c868531d325f1d6b20f1b96a4be1446",
            "fbf492ee0eab78f85a5ef670a301aed00b8bd40c2d561222cdc39862dee89601",
            "48e3eab9f127eaebd3da27c1e019893b7310420426ab1fb04ca1a2e1a6b4fd46",
        ),
        (
            v3,
            PLANNING_CODEC_V3,
            "c4732328ce2891f49d6d520c5399bcfb8cb4fc7124c1d20bce71581806a0263f",
            "a17a3c5f2fa06bc72d1b09b80eaaec2cbefe6bb11d7d62f6f6b06446e7200d12",
            "33f79654a1b8739a76d35eb51f643fc5eedd0d5986253d9a8aeedb8beb3a81e0",
        ),
    ] {
        let wire = serde_json::to_string(&task).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(wire.as_bytes())), draft_sha);
        let candidate = create(task);
        assert_eq!(candidate.codec, codec);
        assert_eq!(candidate.candidate_digest().unwrap(), digest);
        let wire = serde_json::to_string(&candidate).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(wire.as_bytes())),
            candidate_sha
        );
        let loaded: PlanningCandidate = serde_json::from_str(&wire).unwrap();
        assert_eq!(serde_json::to_string(&loaded).unwrap(), wire);
        assert_eq!(loaded.candidate_digest().unwrap(), digest);
    }
}

#[test]
fn both_workspace_review_declarations_require_closed_planning_v4() {
    for policy in [
        ExecutionSettlementPolicy::COMPLETION_POLICY,
        ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY,
    ] {
        let mut candidate = create(settled(policy));
        candidate.validate_structure().unwrap();
        assert_eq!(candidate.codec, PLANNING_CODEC_V4);
        let wire = serde_json::to_value(&candidate).unwrap();
        let loaded: PlanningCandidate = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(loaded).unwrap(), wire);
        for codec in [
            PLANNING_CODEC,
            PLANNING_CODEC_V2,
            PLANNING_CODEC_V3,
            "awr-team-planning-v5",
        ] {
            candidate.codec = codec.into();
            assert!(candidate.validate_structure().is_err());
        }
    }
}

#[test]
fn settlement_refuses_null_unknown_fields_modes_paths_and_duplicate_keys() {
    let task = settled(ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY);
    for settlement in [
        Value::Null,
        json!({}),
        json!({"mode":"future","workspace_id":"workspace-a"}),
        json!({"mode":"independent_workspace_v1","workspace_id":"/tmp/workspace"}),
        json!({"mode":"independent_workspace_v1","workspace_id":""}),
        json!({"mode":"independent_workspace_v1","workspace_id":"workspace-a","approve":true}),
    ] {
        let mut wire = json!(task);
        wire["execution_settlement"] = settlement;
        assert!(match serde_json::from_value::<TaskDraft>(wire) {
            Err(_) => true,
            Ok(task) => task.validate().is_err(),
        });
    }
    let wire = serde_json::to_string(&task).unwrap();
    let duplicate = format!(
        "{},\"execution_settlement\":{}}}",
        wire.strip_suffix('}').unwrap(),
        json!(task.execution_settlement)
    );
    assert!(serde_json::from_str::<TaskDraft>(&duplicate).is_err());
    let mut wire = json!(task);
    wire["execution_authorized"] = json!(true);
    assert!(serde_json::from_value::<TaskDraft>(wire).is_err());
}

#[test]
fn incomplete_new_declarations_refuse_without_reinterpreting_legacy_agent_policy() {
    for policy in [
        ExecutionSettlementPolicy::COMPLETION_POLICY,
        ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY,
    ] {
        let task = settled(policy);
        for field in ["scope_paths", "verification_requirements"] {
            let mut wire = json!(task);
            wire[field] = json!([]);
            let invalid: TaskDraft = serde_json::from_value(wire).unwrap();
            assert!(create(invalid).validate_structure().is_err());
        }
        let mut missing_checks = task.clone();
        missing_checks.verification_requirements = None;
        assert!(create(missing_checks).validate_structure().is_err());
        let mut missing_workspace = task;
        missing_workspace.execution_settlement = None;
        if policy == ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY {
            assert!(create(missing_workspace).validate_structure().is_err());
        } else {
            // Older ordinary Agent definitions need not opt into workspace settlement.
            assert_eq!(create(missing_workspace).codec, PLANNING_CODEC_V3);
        }
    }
    let mut wrong_policy = settled("independent_review");
    assert!(wrong_policy.validate().is_err());
    wrong_policy.execution_settlement = None;
    wrong_policy.validate().unwrap();
}

#[test]
fn edits_bind_workspace_changes_clear_approval_and_preview_omission_as_retention() {
    let task = settled(ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY);
    let mut approved = create(task.clone());
    let old_digest = approved.candidate_digest().unwrap();
    approved.state = CandidateState::Approved;
    approved.approval = Some(PlanningApproval {
        approval_id: "approval-a".into(),
        candidate_digest: old_digest.clone(),
        approver_person_id: "reviewer-member".into(),
        approver_actor_id: "reviewer-agent".into(),
        self_approved: false,
    });
    let mut after = task.clone();
    after.execution_settlement.as_mut().unwrap().workspace_id = "workspace-b".into();
    let edited = edit_candidate(
        approved,
        vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(task.clone()),
            after: after.clone(),
        }],
    )
    .unwrap();
    assert_eq!(edited.codec, PLANNING_CODEC_V4);
    assert_eq!(edited.state, CandidateState::Drafting);
    assert!(edited.approval.is_none());
    assert_ne!(edited.candidate_digest().unwrap(), old_digest);
    let preview = build_candidate_diff(&edited, vec![]).unwrap();
    let field = preview
        .field_diffs
        .iter()
        .find(|d| d.field == "execution_settlement")
        .unwrap();
    assert_eq!(field.before, Some(json!(task.execution_settlement)));
    assert_eq!(field.after, Some(json!(after.execution_settlement)));
    assert!(
        preview
            .review_requirements
            .contains(&"execution_contract_review".into())
    );
    after.execution_settlement = None;
    after.verification_requirements = None;
    let retained = candidate(vec![DraftChange {
        op: DraftOpKind::EditFields,
        before: Some(task),
        after,
    }]);
    assert_eq!(retained.codec, PLANNING_CODEC_V4);
    assert!(
        !build_candidate_diff(&retained, vec![])
            .unwrap()
            .field_diffs
            .iter()
            .any(|d| d.field == "execution_settlement")
    );
}

#[test]
fn named_simulation_policy_still_selects_v4_when_existing_fields_are_omitted() {
    let mut before = task();
    before.completion_policy = ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY.into();
    let mut after = before.clone();
    after.title = "Rename existing issue API".into();
    let mut candidate = candidate(vec![DraftChange {
        op: DraftOpKind::EditFields,
        before: Some(before),
        after,
    }]);
    candidate.validate_structure().unwrap();
    assert_eq!(candidate.codec, PLANNING_CODEC_V4);
    candidate.codec = PLANNING_CODEC_V3.into();
    assert!(candidate.validate_structure().is_err());
}

#[test]
fn workspace_declaration_cannot_downgrade_independent_human_review() {
    let before = task();
    let after = settled(ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY);
    assert!(
        candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(before),
            after
        }])
        .validate_structure()
        .is_err()
    );
}
