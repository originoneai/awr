use awr_core::DeliveryVersionPolicy as VersionPolicy;
use awr_team::{
    CandidateState, CrossWorkstreamDependencyPolicy as CrossPolicy,
    CrossWorkstreamReviewAssurance as Assurance, DependencyAcceptanceMode as Mode, DraftChange,
    DraftDefinitionState, DraftOpKind, ExecutionSettlementMode,
    ExecutionSettlementPolicy as Settlement, PLANNING_CODEC_V6, PlanningApproval,
    PlanningCandidate, TaskDraft, WorkContract, WorkstreamBundle, build_candidate_diff,
    edit_candidate, planning_codec_for_changes,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn legacy() -> WorkstreamBundle {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/team-mcp/publish-prep/baseline-workstreams.json"
    ))
    .unwrap()
}

fn settlement() -> Settlement {
    Settlement {
        mode: ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "cross-worker".into(),
    }
}

fn old_contract(version: usize) -> WorkContract {
    let mut contract = legacy().contracts.remove(1).contract;
    if version > 1 {
        contract.dependency_acceptance.insert(
            "API-1".into(),
            if version == 5 {
                Mode::SimulatedMemberIndependent
            } else {
                Mode::AgentReviewedCallerAssertedReconciled
            },
        );
    }
    contract.codec = format!("awr-team-contract-v{version}");
    if matches!(version, 3 | 4) {
        contract.execution_settlement = Some(settlement());
        contract.verification_requirements = vec!["Verify integration bytes".into()];
        contract.completion_policy = if version == 4 {
            Settlement::SIMULATED_MEMBER_COMPLETION_POLICY
        } else {
            Settlement::COMPLETION_POLICY
        }
        .into();
    }
    contract
}

fn old_candidate(version: usize) -> PlanningCandidate {
    let contract = old_contract(version);
    let draft = TaskDraft {
        work_id: contract.work_id.as_str().into(),
        external_key: contract.external_key,
        title: "Integrate accepted API".into(),
        goals: contract.goals,
        scope_paths: contract.scope_paths,
        acceptance: contract.acceptance,
        required_dependencies: contract.required_dependencies,
        completion_policy: contract.completion_policy,
        dependency_acceptance: (version > 1).then_some(contract.dependency_acceptance),
        hard_rules: None,
        verification_requirements: matches!(version, 3 | 4).then_some(vec!["Verify bytes".into()]),
        execution_settlement: (version == 4).then_some(settlement()),
        definition_state: DraftDefinitionState::Enabled,
        workstream: Some("api".into()),
        split_from: None,
        split_children: vec![],
    };
    let changes = vec![DraftChange {
        op: DraftOpKind::CreateTask,
        before: None,
        after: draft,
    }];
    PlanningCandidate {
        codec: planning_codec_for_changes(&changes).into(),
        candidate_id: "cross-candidate".into(),
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
    }
}

fn wire_sha<T: serde::Serialize>(value: &T) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}

#[test]
fn old_codec_bytes_and_hashes_match_pre_v6_goldens() {
    let goldens: serde_json::Value = serde_json::from_str(r#"[{"bundle_hash":"bc1bbc3697e0ffce69b12927b17323c1d4081802094b2ff365ceadf77451c5b9","bundle_wire":"7376206a962b3c3225962c1a4ed1c90ed3d6c47db3952625b67bbc6a16ddf24f","contract_hash":"9865e859201d39dae2652e84aab642642db3d89536d585e7760fc81011596ea4","contract_wire":"73b9d2821e641a88c5346364d7cd4820bac66324b2104e043918096b624dfc38","planning_hash":"9c85b4bf2fdeaf1176b95770d4c80383bb8e8374f2d098bb3bd4bc03934db25c","planning_wire":"bebaa77cb65ec9b110bdb0f03d664818593611fa071f35d99b05eab3e126aa94","version":1},{"bundle_hash":"031c5fa207c0901de18a3370ad128da3d68c474d993ce5e31fdf83950668b58b","bundle_wire":"f022974fe29c9be881d49f3b042bb0efacaf78df235ab3be70d79a76e9c2ff57","contract_hash":"3ecee8564ce67b26a396b2af0936f6ee84610c5d4a74cda5310acbe5dc3d68b6","contract_wire":"6531c7a2bedd0a862b10fb919b073a69e078466c12648ad05d33a6c65fb45000","planning_hash":"3783e671a3f99e2adb23df57a83a71c038ffffec66247e77097dd5d14b604668","planning_wire":"f62af4fd656e12312f130252ad27cbc5074d28d2909effaa6cb79ea7deb6d6b7","version":2},{"bundle_hash":"34418da4c1e347ee34f687065c6efe59026417891701ddc29c0e533f3bbe1adf","bundle_wire":"d042a2692bf7c2f18e2aba1a222ab1b774492c2c3549840591f1ec5d05c191e4","contract_hash":"55030a704dc53c16873ca94aa802699d3807e741e6388906c27ca152bf99fb02","contract_wire":"55f673f22a1d15307ae2310aa8dec4901675c3d1d285ebc30ff78a6cb38c0d78","planning_hash":"54ffad91e05fbf6209dd041f79aec08b7b415e89e1c026570cf4558aa47174fe","planning_wire":"c1e12c36f6aec64ac1160b7865523d2fe52ef6b75c502b47f1f73b4d4ec22a6f","version":3},{"bundle_hash":"14823d6d7f2a466b96ee15b57c7c14cbd20e4db1648ff087a87b697263b2246f","bundle_wire":"6855fae9bdedb6852d7dc247401a13c4132142c63a517abb0f7de198cd5fa32c","contract_hash":"c69b450ca401a26b2b39388c536664d985e017580f969a44713883fa3b122696","contract_wire":"3e797fbe5f869ce59817611f2626afbab56658af3427aa214466246981f965a1","planning_hash":"0c7b125848efa6157580a9f0245b2f8b2d83e24f12672722c6b78f215da26e4d","planning_wire":"34e159e943450bab04b31149c514457b0cdbffb68a00f5883938729d42125609","version":4},{"bundle_hash":"d717693c5cb28de53e5f5841c0307551d3d35ee97c8ac163076832651b7cbd39","bundle_wire":"01d35739bcd6cc0023c13b990a5d2683eb4aeab0811767bdf88beeb42e92491f","contract_hash":"7024fc0b2b3b64b9bf6d1a71633c314700148c1c00052002c53785cd9cb905b6","contract_wire":"e01272df9cc64c1744bca7edda3af8672c825e8c817a0c2f39bbfae5c25fe897","planning_hash":"e82969db26e89e653849926b49842ccf793473b5afbdfc5002062e70f2c6dec7","planning_wire":"5cb30406b417c5854479613ef6ba2ef3df66b18ccdb9258534b77f7cb7eb606b","version":5}]"#).unwrap();
    for version in 1..=5 {
        let contract = old_contract(version);
        let candidate = old_candidate(version);
        let mut bundle = legacy();
        bundle.codec = format!("awr-team-workstreams-v{version}");
        bundle.contracts[1].contract = contract.clone();
        bundle.contracts[1].workstream_id = bundle.contracts[0].workstream_id;
        assert_eq!(
            json!({
                "version":version,
                "contract_wire":wire_sha(&contract),
                "contract_hash":contract.hash().unwrap(),
                "bundle_wire":wire_sha(&bundle),
                "bundle_hash":bundle.hash().unwrap(),
                "planning_wire":wire_sha(&candidate),
                "planning_hash":candidate.candidate_digest().unwrap(),
            }),
            goldens[version - 1]
        );
    }
}

#[test]
fn cross_workstream_policy_should_be_representable() {
    let bundle = legacy();
    assert_ne!(
        bundle.contracts[0].workstream_id,
        bundle.contracts[1].workstream_id
    );
    let mut value = json!(bundle.contracts[1].contract);
    value["codec"] = json!("awr-team-contract-v6");
    value["dependency_acceptance"] = json!({
        "API-1":{"cross_workstream":{
            "review_assurance":"simulated_member_independent",
            "version_policy":"fixed_delivery"
        }}
    });
    serde_json::from_value::<WorkContract>(value).unwrap();
}

fn cross_mode(assurance: Assurance, version: VersionPolicy) -> Mode {
    Mode::CrossWorkstream(CrossPolicy {
        review_assurance: assurance,
        version_policy: version,
    })
}

fn cross_contract() -> WorkContract {
    let mut contract = old_contract(1);
    contract.codec = WorkContract::CODEC_V6.into();
    contract.dependency_acceptance.insert(
        "API-1".into(),
        cross_mode(
            Assurance::SimulatedMemberIndependent,
            VersionPolicy::FixedDelivery,
        ),
    );
    contract
}

fn cross_bundle() -> WorkstreamBundle {
    let mut bundle = legacy();
    bundle.codec = WorkstreamBundle::CODEC_V6.into();
    bundle.contracts[1].contract = cross_contract();
    bundle
}

#[test]
fn review_and_version_choices_are_independently_bound() {
    let mut hashes = std::collections::BTreeSet::new();
    for assurance in [
        Assurance::TeamIndependent,
        Assurance::SimulatedMemberIndependent,
    ] {
        for version in [VersionPolicy::FixedDelivery, VersionPolicy::CurrentContract] {
            let mut contract = cross_contract();
            contract
                .dependency_acceptance
                .insert("API-1".into(), cross_mode(assurance, version));
            assert_eq!(
                serde_json::from_value::<WorkContract>(json!(contract)).unwrap(),
                contract
            );
            assert!(hashes.insert(contract.hash().unwrap()));
        }
    }
    assert_eq!(hashes.len(), 4);
    for policy in [
        "independent_review",
        "ordinary_confirm",
        Settlement::COMPLETION_POLICY,
        Settlement::SIMULATED_MEMBER_COMPLETION_POLICY,
    ] {
        let mut contract = cross_contract();
        contract.completion_policy = policy.into();
        if policy == Settlement::SIMULATED_MEMBER_COMPLETION_POLICY {
            assert!(contract.validate().is_err());
            contract.execution_settlement = Some(settlement());
            assert!(contract.validate().is_err());
            contract.verification_requirements = vec!["Verify integration".into()];
        }
        contract.validate().unwrap();
    }
}

#[test]
fn v6_policy_is_closed_and_cannot_enter_old_codecs() {
    for codec in [
        WorkContract::CODEC,
        WorkContract::CODEC_V2,
        WorkContract::CODEC_V3,
        WorkContract::CODEC_V4,
        WorkContract::CODEC_V5,
    ] {
        let mut contract = cross_contract();
        contract.codec = codec.into();
        assert!(contract.hash().is_err());
        assert!(serde_json::from_value::<WorkContract>(json!(contract)).is_err());
    }
    for mode in [
        Value::Null,
        json!("cross_workstream"),
        json!({"cross_workstream":null}),
        json!({"cross_workstream":{}}),
        json!({"cross_workstream":{"review_assurance":"unknown","version_policy":"fixed_delivery"}}),
        json!({"cross_workstream":{"review_assurance":"team_independent","version_policy":"latest"}}),
        json!({"cross_workstream":{"review_assurance":"team_independent","version_policy":null}}),
        json!({"cross_workstream":{"review_assurance":null,"version_policy":"fixed_delivery"}}),
        json!({"cross_workstream":{"review_assurance":"team_independent","version_policy":"fixed_delivery","human_approval":true}}),
    ] {
        let mut value = json!(cross_contract());
        value["dependency_acceptance"] = json!({"API-1":mode});
        assert!(serde_json::from_value::<WorkContract>(value).is_err());
    }
    let mode = json!(cross_mode(
        Assurance::TeamIndependent,
        VersionPolicy::FixedDelivery
    ));
    for map in [
        Value::Null,
        json!({}),
        json!({"missing":mode}),
        json!({"CLIENT-1":mode}),
    ] {
        let mut value = json!(cross_contract());
        value["dependency_acceptance"] = map;
        assert!(serde_json::from_value::<WorkContract>(value).is_err());
    }
    let mut value = json!(cross_contract());
    value
        .as_object_mut()
        .unwrap()
        .remove("dependency_acceptance");
    assert!(serde_json::from_value::<WorkContract>(value.clone()).is_err());
    let text = value.to_string();
    let duplicate = format!(
        "{},\"dependency_acceptance\":{{\"API-1\":{mode},\"API-1\":{mode}",
        &text[..text.len() - 1]
    ) + "}}";
    assert!(serde_json::from_str::<WorkContract>(&duplicate).is_err());
    let mut contract = cross_contract();
    contract.required_dependencies.push("API-1".into());
    assert!(contract.validate().is_err());
}

#[test]
fn v6_bundle_requires_distinct_owned_streams_and_a_complete_acyclic_graph() {
    let bundle = cross_bundle();
    bundle.validate("demo-project").unwrap();
    assert_eq!(
        serde_json::from_value::<WorkstreamBundle>(json!(bundle))
            .unwrap()
            .hash()
            .unwrap(),
        bundle.hash().unwrap()
    );
    for codec in [
        WorkstreamBundle::CODEC,
        WorkstreamBundle::CODEC_V2,
        WorkstreamBundle::CODEC_V3,
        WorkstreamBundle::CODEC_V4,
        WorkstreamBundle::CODEC_V5,
    ] {
        let mut old = bundle.clone();
        old.codec = codec.into();
        assert!(old.validate("demo-project").is_err());
    }
    let mut invalid = bundle.clone();
    invalid.contracts[1].workstream_id = invalid.contracts[0].workstream_id;
    assert!(invalid.validate("demo-project").is_err());
    let mut invalid = bundle.clone();
    invalid.contracts[0].workstream_id = awr_core::Id::from(99);
    assert!(invalid.validate("demo-project").is_err());
    assert!(bundle.validate("another-project").is_err());
    let mut missing = bundle.clone();
    missing.contracts[1]
        .contract
        .required_dependencies
        .push("missing".into());
    assert!(missing.validate("demo-project").is_err());
    // A V6 graph also validates old-codec nodes without changing standalone
    // V1 semantics. Duplicate same-stream edges must not hide in those nodes.
    let mut duplicate = bundle.clone();
    let mut provider = duplicate.contracts[0].clone();
    provider.contract.work_id = awr_team::WorkId::new("API-2").unwrap();
    provider.contract.external_key = "API-2".into();
    duplicate.contracts.push(provider);
    duplicate.contracts[0].contract.required_dependencies = vec!["API-2".into(), "API-2".into()];
    duplicate.contracts[0].contract.validate().unwrap();
    assert!(duplicate.validate("demo-project").is_err());
    let mut untyped = bundle.clone();
    let mut extra = untyped.contracts[0].clone();
    extra.contract.work_id = awr_team::WorkId::new("API-2").unwrap();
    extra.contract.external_key = "API-2".into();
    untyped.contracts.push(extra);
    untyped.contracts[1]
        .contract
        .required_dependencies
        .push("API-2".into());
    assert!(untyped.validate("demo-project").is_err());
    let mut cycle = bundle.clone();
    cycle.contracts[0].contract.codec = WorkContract::CODEC_V6.into();
    cycle.contracts[0].contract.required_dependencies = vec!["CLIENT-1".into()];
    cycle.contracts[0].contract.dependency_acceptance.insert(
        "CLIENT-1".into(),
        cross_mode(Assurance::TeamIndependent, VersionPolicy::CurrentContract),
    );
    assert!(
        cycle
            .validate("demo-project")
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
    let mut empty_v6 = legacy();
    empty_v6.codec = WorkstreamBundle::CODEC_V6.into();
    assert!(empty_v6.validate("demo-project").is_err());
}

#[test]
fn mixed_same_and_cross_stream_modes_keep_their_distinct_boundaries() {
    let mut bundle = cross_bundle();
    let mut same = bundle.contracts[1].clone();
    same.contract.work_id = awr_team::WorkId::new("CLIENT-2").unwrap();
    same.contract.external_key = "CLIENT-2".into();
    bundle.contracts.push(same);
    bundle.contracts[1]
        .contract
        .required_dependencies
        .push("CLIENT-2".into());
    bundle.contracts[1]
        .contract
        .dependency_acceptance
        .insert("CLIENT-2".into(), Mode::SimulatedMemberIndependent);
    bundle.validate("demo-project").unwrap();
    bundle.contracts[1].contract.dependency_acceptance.insert(
        "CLIENT-2".into(),
        cross_mode(Assurance::TeamIndependent, VersionPolicy::FixedDelivery),
    );
    assert!(bundle.validate("demo-project").is_err());
    bundle.contracts[1].contract.dependency_acceptance.insert(
        "CLIENT-2".into(),
        Mode::AgentReviewedCallerAssertedReconciled,
    );
    bundle.contracts[1]
        .contract
        .dependency_acceptance
        .insert("API-1".into(), Mode::SimulatedMemberIndependent);
    assert!(bundle.validate("demo-project").is_err());
}

#[test]
fn planning_tracks_prior_policy_and_invalidates_approval_on_policy_edits() {
    let mut candidate = old_candidate(1);
    let draft = &mut candidate.changes[0].after;
    draft.workstream = Some("client".into());
    draft.dependency_acceptance = Some(cross_contract().dependency_acceptance);
    candidate.codec = planning_codec_for_changes(&candidate.changes).into();
    assert_eq!(candidate.codec, PLANNING_CODEC_V6);
    candidate.validate_structure().unwrap();
    let digest = candidate.candidate_digest().unwrap();
    candidate.state = CandidateState::Approved;
    candidate.approval = Some(PlanningApproval {
        approval_id: "approval".into(),
        candidate_digest: digest.clone(),
        approver_person_id: "reviewer".into(),
        approver_actor_id: "review-agent".into(),
        self_approved: false,
    });
    let before = candidate.changes[0].after.clone();
    let mut after = before.clone();
    after.dependency_acceptance.as_mut().unwrap().insert(
        "API-1".into(),
        cross_mode(Assurance::TeamIndependent, VersionPolicy::CurrentContract),
    );
    let changed = edit_candidate(
        candidate.clone(),
        vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(before.clone()),
            after: after.clone(),
        }],
    )
    .unwrap();
    assert_eq!(changed.codec, PLANNING_CODEC_V6);
    assert!(changed.approval.is_none());
    assert_eq!(changed.state, CandidateState::Drafting);
    assert_ne!(changed.candidate_digest().unwrap(), digest);
    assert!(
        build_candidate_diff(&changed, vec![])
            .unwrap()
            .review_requirements
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
    assert_eq!(retained.codec, PLANNING_CODEC_V6);
    assert!(
        !build_candidate_diff(&retained, vec![])
            .unwrap()
            .field_diffs
            .iter()
            .any(|d| d.field == "dependency_acceptance")
    );
    for version in 1..=5 {
        let mut old = changed.clone();
        old.codec = format!("awr-team-planning-v{version}");
        assert!(old.validate_structure().is_err());
    }
}
