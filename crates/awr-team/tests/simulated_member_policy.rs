use awr_team::{
    DependencyAcceptanceMode, ExecutionSettlementMode, ExecutionSettlementPolicy, WorkContract,
    WorkstreamBundle,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn legacy() -> WorkstreamBundle {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/team-mcp/publish-prep/baseline-workstreams.json"
    ))
    .unwrap()
}

fn agent_contract() -> WorkContract {
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

fn simulated_contract() -> WorkContract {
    let mut contract = agent_contract();
    contract.codec = WorkContract::CODEC_V4.into();
    contract.completion_policy =
        ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY.into();
    contract
}

#[test]
fn v4_round_trips_explicit_policy_without_identity_or_authority_claims() {
    let contract = simulated_contract();
    contract.validate().unwrap();
    let wire = json!(contract);
    assert_eq!(wire["codec"], WorkContract::CODEC_V4);
    assert_eq!(
        wire["completion_policy"],
        ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY
    );
    for field in [
        "member_id",
        "human_approval",
        "team_independent_acceptance",
        "physical_operator_count",
        "trusted_executor",
    ] {
        assert!(wire.get(field).is_none());
        let mut forged = wire.clone();
        forged[field] = json!(true);
        assert!(serde_json::from_value::<WorkContract>(forged).is_err());
    }
    assert_eq!(
        serde_json::from_value::<WorkContract>(wire).unwrap(),
        contract
    );
}

#[test]
fn old_codecs_cannot_silently_accept_the_new_named_policy() {
    let mut v2 = legacy().contracts.remove(1).contract;
    v2.codec = WorkContract::CODEC_V2.into();
    v2.dependency_acceptance.insert(
        "API-1".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    );
    for mut contract in [legacy().contracts.remove(0).contract, v2, agent_contract()] {
        contract.validate().unwrap();
        contract.completion_policy =
            ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY.into();
        assert!(contract.validate().is_err());
        assert!(contract.hash().is_err());
        assert!(serde_json::from_value::<WorkContract>(json!(contract)).is_err());
    }
}

#[test]
fn v4_requires_supported_settlement_scope_checks_and_exact_policy() {
    let base = simulated_contract();
    let mut cases = vec![];
    let mut absent = base.clone();
    absent.execution_settlement = None;
    cases.push(absent);
    let mut invalid = base.clone();
    invalid.execution_settlement.as_mut().unwrap().workspace_id = "worker/path".into();
    cases.push(invalid);
    for policy in [
        "independent_review",
        "ordinary_confirm",
        "personal_self_review",
        ExecutionSettlementPolicy::COMPLETION_POLICY,
    ] {
        let mut value = base.clone();
        value.completion_policy = policy.into();
        cases.push(value);
    }
    for entries in [vec![], vec![" ".into()]] {
        let mut value = base.clone();
        value.scope_paths = entries.clone();
        cases.push(value);
        let mut value = base.clone();
        value.verification_requirements = entries;
        cases.push(value);
    }
    for value in cases {
        assert!(value.validate().is_err());
        assert!(serde_json::from_value::<WorkContract>(json!(value)).is_err());
    }
    for extension in [
        Value::Null,
        json!({}),
        json!({"mode":"trusted_executor","workspace_id":"worker-a"}),
        json!({"mode":"independent_workspace_v1","workspace_id":"worker-a","human_approval":true}),
    ] {
        let mut value = json!(base);
        value["execution_settlement"] = extension;
        assert!(serde_json::from_value::<WorkContract>(value).is_err());
    }
    let duplicate = json!(base).to_string().replace(
        "\"completion_policy\":",
        "\"completion_policy\":\"independent_review\",\"completion_policy\":",
    );
    assert!(serde_json::from_str::<WorkContract>(&duplicate).is_err());
    let mut future = json!(base);
    future["codec"] = json!("awr-team-contract-v5");
    assert!(serde_json::from_value::<WorkContract>(future).is_err());
}

#[test]
fn legacy_contract_hashes_and_wire_bytes_remain_at_their_recorded_values() {
    // Recorded through the actual unmodified kernel before V4 was introduced.
    let bundle = legacy();
    let mut v2 = bundle.contracts[1].contract.clone();
    v2.codec = WorkContract::CODEC_V2.into();
    v2.dependency_acceptance.insert(
        "API-1".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    );
    for (contract, hash, wire_sha256) in [
        (
            bundle.contracts[0].contract.clone(),
            "87647f506c58993aa4e4eaf8816b3fa37b731587e6568d8ddd5d789027524f83",
            "706df74b107481041c12e3c764b23b176961757ecb0dc71f3e8181d37a8ceb36",
        ),
        (
            v2,
            "3ecee8564ce67b26a396b2af0936f6ee84610c5d4a74cda5310acbe5dc3d68b6",
            "6531c7a2bedd0a862b10fb919b073a69e078466c12648ad05d33a6c65fb45000",
        ),
        (
            agent_contract(),
            "638f12fe0170c31abaf9d91199e7ac6781ad4da96f6348ae39c6f7252d24c730",
            "c52c025693173173f029a074f884a3e684be500ced0c962a2dc58a883e071818",
        ),
    ] {
        assert_eq!(contract.hash().unwrap(), hash);
        let wire = serde_json::to_vec(&contract).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&wire)), wire_sha256);
        assert_eq!(
            serde_json::from_slice::<WorkContract>(&wire).unwrap(),
            contract
        );
    }
    assert_eq!(
        bundle.hash().unwrap(),
        "ba85de4eb5f4a0c01cc5fe3357a6044b26531d18dd917665bf49ec92d22bc91e"
    );
}

#[test]
fn v4_hash_binds_settlement_checks_scope_and_dependency_policy() {
    let base = simulated_contract();
    let hash = base.hash().unwrap();
    assert_ne!(hash, agent_contract().hash().unwrap());
    let mut workspace = base.clone();
    workspace
        .execution_settlement
        .as_mut()
        .unwrap()
        .workspace_id = "worker-b".into();
    let mut paths = base.clone();
    paths.scope_paths.push("src/other.rs".into());
    let mut checks = base.clone();
    checks
        .verification_requirements
        .push("another check".into());
    for changed in [workspace, paths.clone(), checks] {
        assert_ne!(changed.hash().unwrap(), hash);
    }
    paths.scope_paths.reverse();
    let mut reordered = paths.clone();
    reordered.scope_paths.reverse();
    assert_eq!(paths.hash().unwrap(), reordered.hash().unwrap());
    let mut dependency = base;
    dependency.required_dependencies = vec!["predecessor".into()];
    let before = dependency.hash().unwrap();
    dependency.dependency_acceptance.insert(
        "predecessor".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    );
    assert_ne!(dependency.hash().unwrap(), before);
    dependency.required_dependencies.push("predecessor".into());
    assert!(dependency.hash().is_err());
}

#[test]
fn mixed_v4_bundle_preserves_older_hashes_and_requires_explicit_upgrade() {
    let mut bundle = legacy();
    let unchanged = bundle.contracts[1].contract.clone();
    let unchanged_hash = unchanged.hash().unwrap();
    bundle.contracts[0].contract = simulated_contract();
    for codec in [
        WorkstreamBundle::CODEC,
        WorkstreamBundle::CODEC_V2,
        WorkstreamBundle::CODEC_V3,
    ] {
        bundle.codec = codec.into();
        assert!(bundle.validate("demo-project").is_err());
    }
    bundle.codec = WorkstreamBundle::CODEC_V4.into();
    bundle.validate("demo-project").unwrap();
    let wire = serde_json::to_vec(&bundle).unwrap();
    let decoded: WorkstreamBundle = serde_json::from_slice(&wire).unwrap();
    assert_eq!(decoded.hash().unwrap(), bundle.hash().unwrap());
    assert_eq!(decoded.contracts[1].contract, unchanged);
    assert_eq!(
        decoded.contracts[1].contract.hash().unwrap(),
        unchanged_hash
    );
    let older = agent_contract();
    let older_hash = older.hash().unwrap();
    bundle.contracts[0].contract = older;
    bundle.validate("demo-project").unwrap();
    assert_eq!(bundle.contracts[0].contract.hash().unwrap(), older_hash);
}

#[test]
fn v4_does_not_expand_legacy_cross_stream_dependency_acceptance() {
    let mut bundle = legacy();
    assert_ne!(
        bundle.contracts[0].workstream_id,
        bundle.contracts[1].workstream_id
    );
    bundle.codec = WorkstreamBundle::CODEC_V4.into();
    bundle.contracts[0].contract = simulated_contract();
    let consumer = &mut bundle.contracts[1].contract;
    consumer.codec = WorkContract::CODEC_V2.into();
    consumer.dependency_acceptance.insert(
        "API-1".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    );
    assert!(
        bundle
            .validate("demo-project")
            .unwrap_err()
            .to_string()
            .contains("cross-stream adoption is unsupported")
    );
}
