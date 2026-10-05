use awr_team::{
    DependencyAcceptanceMode, ExecutionSettlementMode, ExecutionSettlementPolicy, WorkContract,
    WorkstreamBundle,
};
use serde_json::{Value, json};

fn legacy() -> WorkstreamBundle {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/team-mcp/publish-prep/baseline-workstreams.json"
    ))
    .unwrap()
}

fn opted_in() -> WorkContract {
    let mut contract = legacy().contracts.remove(0).contract;
    contract.codec = WorkContract::CODEC_V3.into();
    contract.completion_policy = ExecutionSettlementPolicy::COMPLETION_POLICY.into();
    contract.verification_requirements = vec!["independently verify artifact bytes".into()];
    contract.execution_settlement = Some(ExecutionSettlementPolicy {
        mode: ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "worker-a".into(),
    });
    contract
}

#[test]
fn explicit_v3_round_trips_without_inventing_executor_or_human_trust() {
    let contract = opted_in();
    let wire = json!(contract);
    assert_eq!(
        wire["execution_settlement"],
        json!({"mode":"independent_workspace_v1","workspace_id":"worker-a"})
    );
    assert!(wire.get("trusted_executor").is_none());
    assert!(wire.get("human_approval").is_none());
    assert_eq!(
        serde_json::from_value::<WorkContract>(wire).unwrap(),
        contract
    );
    contract.hash().unwrap();
}

#[test]
fn legacy_codecs_reject_settlement_requests_even_when_empty_or_null() {
    let bundle = legacy();
    let v1 = json!(bundle.contracts[0].contract);
    let mut v2 = json!(bundle.contracts[1].contract);
    v2["codec"] = json!(WorkContract::CODEC_V2);
    v2["dependency_acceptance"] = json!({"API-1":"agent_reviewed_caller_asserted_reconciled"});
    for base in [v1, v2] {
        assert!(serde_json::from_value::<WorkContract>(base.clone()).is_ok());
        for extension in [
            Value::Null,
            json!({}),
            json!(opted_in().execution_settlement),
        ] {
            let mut value = base.clone();
            value["execution_settlement"] = extension;
            assert!(serde_json::from_value::<WorkContract>(value).is_err());
        }
        let mut typed = serde_json::from_value::<WorkContract>(base).unwrap();
        typed.execution_settlement = opted_in().execution_settlement;
        assert!(typed.validate().is_err());
        assert!(typed.hash().is_err());
    }
}

#[test]
fn v3_requires_explicit_scoped_policy_and_independent_artifact_review() {
    let base = opted_in();
    let mut cases = vec![];
    let mut omitted = base.clone();
    omitted.execution_settlement = None;
    cases.push(omitted);
    for policy in [
        "ordinary_confirm",
        "personal_self_review",
        "trusted_execution_and_review",
    ] {
        let mut value = base.clone();
        value.completion_policy = policy.into();
        cases.push(value);
    }
    for paths in [vec![], vec![" ".into()]] {
        let mut value = base.clone();
        value.scope_paths = paths;
        cases.push(value);
    }
    for checks in [vec![], vec![" ".into()]] {
        let mut value = base.clone();
        value.verification_requirements = checks;
        cases.push(value);
    }
    for value in cases {
        assert!(value.validate().is_err());
        assert!(serde_json::from_value::<WorkContract>(json!(value)).is_err());
    }
    for codec in ["awr-team-contract-v4", "future"] {
        let mut wire = json!(base);
        wire["codec"] = json!(codec);
        assert!(serde_json::from_value::<WorkContract>(wire).is_err());
    }
}

#[test]
fn workspace_identity_is_opaque_and_bounded() {
    let mut policy = opted_in().execution_settlement.unwrap();
    for id in [
        "",
        "..",
        "a/b",
        "a\\b",
        "http://host",
        " worker",
        "-worker",
        "工位",
        "a\n",
    ] {
        policy.workspace_id = id.into();
        assert!(policy.validate().is_err(), "invalid identity accepted");
    }
    policy.workspace_id = "a".repeat(129);
    assert!(policy.validate().is_err());
    for id in ["1", "host:worker-01.v1", "worker_a", &"a".repeat(128)] {
        policy.workspace_id = id.into();
        policy.validate().unwrap();
    }
}

#[test]
fn strict_policy_wire_rejects_unknown_modes_trust_fields_and_duplicate_keys() {
    let wire = json!(opted_in());
    for policy in [
        Value::Null,
        json!({}),
        json!({"mode":"trusted_executor","workspace_id":"worker-a"}),
        json!({"mode":"independent_workspace_v1","workspace_id":true}),
        json!({"mode":"independent_workspace_v1","workspace_id":"worker-a","trusted_executor":true}),
        json!({"mode":"independent_workspace_v1","workspace_id":"worker-a","human_approval":true}),
    ] {
        let mut value = wire.clone();
        value["execution_settlement"] = policy;
        assert!(serde_json::from_value::<WorkContract>(value).is_err());
    }
    let mut omitted = wire.clone();
    omitted
        .as_object_mut()
        .unwrap()
        .remove("execution_settlement");
    assert!(serde_json::from_value::<WorkContract>(omitted).is_err());
    let duplicate = wire.to_string().replace(
        "\"workspace_id\":\"worker-a\"",
        "\"workspace_id\":\"worker-a\",\"workspace_id\":\"worker-a\"",
    );
    assert!(serde_json::from_str::<WorkContract>(&duplicate).is_err());
    let duplicate = wire.to_string().replace(
        "\"mode\":\"independent_workspace_v1\"",
        "\"mode\":\"independent_workspace_v1\",\"mode\":\"independent_workspace_v1\"",
    );
    assert!(serde_json::from_str::<WorkContract>(&duplicate).is_err());
    let duplicate = wire.to_string().replace(
        "\"execution_settlement\":",
        "\"execution_settlement\":null,\"execution_settlement\":",
    );
    assert!(serde_json::from_str::<WorkContract>(&duplicate).is_err());
}

#[test]
fn v3_hash_binds_workspace_scope_verification_and_dependency_policy() {
    let base = opted_in();
    let hash = base.hash().unwrap();
    let mut workspace = base.clone();
    workspace
        .execution_settlement
        .as_mut()
        .unwrap()
        .workspace_id = "worker-b".into();
    assert_ne!(workspace.hash().unwrap(), hash);
    let mut paths = base.clone();
    paths.scope_paths.push("src/other.rs".into());
    assert_ne!(paths.hash().unwrap(), hash);
    let mut checks = base.clone();
    checks
        .verification_requirements
        .push("verify second criterion".into());
    assert_ne!(checks.hash().unwrap(), hash);
    let mut ordered = paths.clone();
    ordered.scope_paths.reverse();
    assert_eq!(ordered.hash().unwrap(), paths.hash().unwrap());
    let mut dependency = base.clone();
    dependency.required_dependencies = vec!["predecessor".into()];
    let default_hash = dependency.hash().unwrap();
    dependency.dependency_acceptance.insert(
        "predecessor".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    );
    assert_ne!(dependency.hash().unwrap(), default_hash);
    dependency.required_dependencies.push("predecessor".into());
    assert!(dependency.hash().is_err());
}

#[test]
fn mixed_v3_bundle_needs_explicit_bundle_upgrade_and_preserves_legacy_contract() {
    let mut bundle = legacy();
    let unchanged = bundle.contracts[1].contract.clone();
    let unchanged_hash = unchanged.hash().unwrap();
    bundle.contracts[0].contract = opted_in();
    for codec in [WorkstreamBundle::CODEC, WorkstreamBundle::CODEC_V2] {
        bundle.codec = codec.into();
        assert!(bundle.validate("demo-project").is_err());
    }
    bundle.codec = WorkstreamBundle::CODEC_V3.into();
    bundle.validate("demo-project").unwrap();
    assert_eq!(bundle.contracts[1].contract, unchanged);
    assert_eq!(bundle.contracts[1].contract.hash().unwrap(), unchanged_hash);
    let decoded = serde_json::from_value::<WorkstreamBundle>(json!(bundle)).unwrap();
    assert_eq!(decoded.hash().unwrap(), bundle.hash().unwrap());
}
