use awr_core::Id;
use awr_team::{DependencyAcceptanceMode, WorkContract, WorkId, WorkstreamBundle};
use serde_json::{Value, json};

const MODE: &str = "agent_reviewed_caller_asserted_reconciled";

fn legacy() -> WorkstreamBundle {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/team-mcp/publish-prep/baseline-workstreams.json"
    ))
    .unwrap()
}

fn opted_in() -> WorkContract {
    let mut c = legacy().contracts.remove(1).contract;
    c.codec = WorkContract::CODEC_V2.into();
    c.dependency_acceptance.insert(
        "API-1".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    );
    c
}

#[test]
fn v1_wire_and_hashes_remain_unchanged_and_reject_v2_fields() {
    let bundle = legacy();
    assert_eq!(
        bundle.hash().unwrap(),
        "ba85de4eb5f4a0c01cc5fe3357a6044b26531d18dd917665bf49ec92d22bc91e"
    );
    assert_eq!(
        bundle.contracts[0].contract.hash().unwrap(),
        "87647f506c58993aa4e4eaf8816b3fa37b731587e6568d8ddd5d789027524f83"
    );
    assert_eq!(
        bundle.contracts[1].contract.hash().unwrap(),
        "9865e859201d39dae2652e84aab642642db3d89536d585e7760fc81011596ea4"
    );
    let raw: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/team-mcp/publish-prep/baseline-workstreams.json"
    ))
    .unwrap();
    assert_eq!(json!(bundle), raw);
    for map in [json!({}), Value::Null, json!({"API-1":MODE})] {
        let mut c = raw["contracts"][1]["contract"].clone();
        c["dependency_acceptance"] = map;
        assert!(serde_json::from_value::<WorkContract>(c).is_err());
    }
}

#[test]
fn v2_hash_binds_exact_dependency_policy_and_ignores_map_order() {
    let a = opted_in();
    // Pre-V3 golden value: extending the domain must not reinterpret V2 receipts.
    assert_eq!(
        a.hash().unwrap(),
        "3ecee8564ce67b26a396b2af0936f6ee84610c5d4a74cda5310acbe5dc3d68b6"
    );
    let mut b = a.clone();
    b.required_dependencies.push("API-2".into());
    b.dependency_acceptance.insert(
        "API-2".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    );
    let mut c = b.clone();
    c.required_dependencies.reverse();
    assert_eq!(b.hash().unwrap(), c.hash().unwrap());
    c.dependency_acceptance.remove("API-1");
    assert_ne!(b.hash().unwrap(), c.hash().unwrap());
    assert_ne!(
        a.hash().unwrap(),
        legacy().contracts[1].contract.hash().unwrap()
    );
    assert_eq!(serde_json::from_value::<WorkContract>(json!(b)).unwrap(), b);
}

#[test]
fn v2_rejects_invalid_maps_and_duplicate_raw_keys() {
    let valid = json!(opted_in());
    for map in [
        Value::Null,
        json!({}),
        json!({"API-1":"trusted"}),
        json!({"absent":MODE}),
        json!({"CLIENT-1":MODE}),
    ] {
        let mut c = valid.clone();
        c["dependency_acceptance"] = map;
        assert!(serde_json::from_value::<WorkContract>(c).is_err());
    }
    let mut c = valid.clone();
    c.as_object_mut().unwrap().remove("dependency_acceptance");
    assert!(serde_json::from_value::<WorkContract>(c).is_err());
    let mut c = valid.clone();
    c["required_dependencies"] = json!(["API-1", "API-1"]);
    assert!(serde_json::from_value::<WorkContract>(c).is_err());
    let raw = valid.to_string().replace(
        &format!("\"API-1\":\"{MODE}\""),
        &format!("\"API-1\":\"{MODE}\",\"API-1\":\"{MODE}\""),
    );
    assert!(serde_json::from_str::<WorkContract>(&raw).is_err());
}

#[test]
fn explicit_v2_bundle_accepts_mixed_contracts_only_with_same_stream_input() {
    let mut b = legacy();
    b.contracts[1].contract = opted_in();
    b.contracts[1].workstream_id = b.contracts[0].workstream_id;
    assert!(b.validate("demo-project").is_err());
    b.codec = WorkstreamBundle::CODEC_V2.into();
    b.validate("demo-project").unwrap();
    assert_eq!(b.contracts[0].contract.codec, WorkContract::CODEC);
    b.contracts[0].workstream_id = Id::from(2);
    assert!(b.validate("demo-project").is_err());
    b.contracts[0].workstream_id = b.contracts[1].workstream_id;
    b.contracts[0].contract.work_id = WorkId::new("renamed").unwrap();
    assert!(b.validate("demo-project").is_err());
}
