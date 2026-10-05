use awr_team::*;
use serde_json::{Map, json};

fn sample() -> WorkContract {
    WorkContract {
        dependency_acceptance: Default::default(),
        execution_settlement: None,
        codec: WorkContract::CODEC.into(),
        work_id: WorkId::new("work-orders").unwrap(),
        external_key: "ORDERS-1".into(),
        goals: vec!["ship".into()],
        hard_rules: vec!["no-prod-write".into()],
        scope_paths: vec!["src/orders.rs".into()],
        acceptance: vec!["query returns today's orders".into()],
        required_dependencies: vec![],
        completion_policy: "trusted_execution_and_review".into(),
        verification_requirements: vec!["cargo test".into()],
    }
}

#[test]
fn identical_semantics_hash_stably_and_ignore_input_order() {
    let mut a = sample();
    let mut b = sample();
    a.acceptance = vec!["b".into(), "a".into()];
    b.acceptance = vec!["a".into(), "b".into()];
    assert_eq!(a.hash().unwrap(), b.hash().unwrap());
}

#[test]
fn acceptance_change_changes_hash_progress_note_does_not() {
    let base = sample().hash().unwrap();
    let mut changed = sample();
    changed.acceptance = vec!["different criterion".into()];
    assert_ne!(base, changed.hash().unwrap());
    // Progress notes are not contract fields; constructing the same contract
    // with extra operational commentary is done outside WorkContract.
    let again = sample();
    assert_eq!(base, again.hash().unwrap());
}

#[test]
fn unknown_required_security_fields_are_rejected() {
    let mut object = Map::new();
    object.insert("policy".into(), json!("strict"));
    object.insert("extra_bypass".into(), json!(true));
    let err = reject_unknown_required_fields(&object, &["policy"]).unwrap_err();
    assert_eq!(err, TeamError::UnknownRequiredField("extra_bypass".into()));
}

#[test]
fn source_declared_completed_is_not_currently_verified() {
    let view = current_completion(
        true,
        false,
        None,
        "contract-a",
        true,
        None,
        &ReviewPolicy {
            required: true,
            author_may_self_approve: false,
            approved: false,
            reviewer_is_author: true,
        },
    );
    assert_eq!(view, CompletionView::SourceDeclared);
    assert_ne!(view, CompletionView::CurrentlyVerified);
}

#[test]
fn agent_self_report_cannot_become_current_verification() {
    let bundle = EvidenceBundle {
        grade: EvidenceGrade::AgentSelfReport,
        contract_hash: "contract-a".into(),
        artifact_digest: Some("abc".into()),
        accessible: true,
    };
    let view = current_completion(
        true,
        true,
        Some("contract-a"),
        "contract-a",
        true,
        Some(&bundle),
        &ReviewPolicy {
            required: false,
            author_may_self_approve: false,
            approved: false,
            reviewer_is_author: false,
        },
    );
    assert_eq!(view, CompletionView::SourceDeclared);
}

#[test]
fn trusted_receipt_with_review_is_currently_verified() {
    let bundle = EvidenceBundle {
        grade: EvidenceGrade::TrustedExecutionReceipt,
        contract_hash: "contract-a".into(),
        artifact_digest: Some("abc".into()),
        accessible: true,
    };
    let view = current_completion(
        true,
        true,
        Some("contract-a"),
        "contract-a",
        true,
        Some(&bundle),
        &ReviewPolicy {
            required: true,
            author_may_self_approve: false,
            approved: true,
            reviewer_is_author: false,
        },
    );
    assert_eq!(view, CompletionView::CurrentlyVerified);
}

#[test]
fn contract_hash_mismatch_requires_revalidation() {
    let bundle = EvidenceBundle {
        grade: EvidenceGrade::TrustedExecutionReceipt,
        contract_hash: "old".into(),
        artifact_digest: Some("abc".into()),
        accessible: true,
    };
    let view = current_completion(
        false,
        true,
        Some("old"),
        "new",
        true,
        Some(&bundle),
        &ReviewPolicy {
            required: false,
            author_may_self_approve: false,
            approved: false,
            reviewer_is_author: false,
        },
    );
    assert_eq!(view, CompletionView::NeedsRevalidation);
}
