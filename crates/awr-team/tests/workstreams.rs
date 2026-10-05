use awr_core::{Id, Workstream, WorkstreamCatalog, WorkstreamState};
use awr_team::{WorkContract, WorkId, WorkstreamBundle, WorkstreamContract};

fn bundle() -> WorkstreamBundle {
    let stream = Workstream {
        id: Id::from(1),
        project_id: "team-project".into(),
        external_key: "A".into(),
        title: "Interface".into(),
        state: WorkstreamState::Active,
        authority_version: 1,
        goal_keys: vec!["goal-b".into(), "goal-a".into()],
        acceptance_contracts: vec![],
    };
    let contract = WorkContract {
        dependency_acceptance: Default::default(),
        execution_settlement: None,
        codec: WorkContract::CODEC.into(),
        work_id: WorkId::new("interface").unwrap(),
        external_key: "API".into(),
        goals: vec!["goal-a".into()],
        hard_rules: vec![],
        scope_paths: vec!["src".into()],
        acceptance: vec!["interface verified".into()],
        required_dependencies: vec![],
        completion_policy: "review".into(),
        verification_requirements: vec![],
    };
    WorkstreamBundle {
        codec: WorkstreamBundle::CODEC.into(),
        catalog: WorkstreamCatalog {
            version: 1,
            project_id: "team-project".into(),
            legacy_default: None,
            workstreams: vec![stream],
        },
        contracts: vec![WorkstreamContract {
            workstream_id: Id::from(1),
            contract,
        }],
    }
}

#[test]
fn aggregate_hash_is_order_independent_and_preserves_nested_v1_hashes() {
    let mut value = bundle();
    let original = value.contracts[0].contract.hash().unwrap();
    let aggregate = value.hash().unwrap();
    value.catalog.workstreams[0].goal_keys.reverse();
    assert_eq!(aggregate, value.hash().unwrap());
    assert_eq!(original, value.contracts[0].contract.hash().unwrap());
    value.catalog.workstreams[0].title = "API team".into();
    assert_ne!(aggregate, value.hash().unwrap());
    assert_eq!(original, value.contracts[0].contract.hash().unwrap());
}

#[test]
fn complete_source_rejects_duplicate_ownership_unknown_scopes_and_project_mismatch() {
    let original = bundle();
    assert!(original.validate("another-project").is_err());
    let mut duplicate = original.clone();
    duplicate.contracts.push(duplicate.contracts[0].clone());
    assert!(duplicate.validate("team-project").is_err());
    let mut unknown = original.clone();
    unknown.contracts[0].workstream_id = Id::from(2);
    assert!(unknown.validate("team-project").is_err());
    let mut version = original.clone();
    version.codec = "next-codec".into();
    assert!(version.validate("team-project").is_err());
    let mut json = serde_json::to_value(original).unwrap();
    json["grants"] = serde_json::json!([{"can_read":true}]);
    assert!(serde_json::from_value::<WorkstreamBundle>(json).is_err());
}
