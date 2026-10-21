//! AWR-TMCP-022: planning writeback activation gate and receipts on real PG.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team::{
    DraftChange, DraftDefinitionState, DraftOpKind, OrdinaryPlanningSelfApprovePolicy, TaskDraft,
};
use awr_team_pg::{DraftCandidateCreate, PgError, SourceStore, WritebackActivateRequest};
use fixture::*;
use std::path::PathBuf;

#[tokio::test]
async fn completed_writeback_rejects_changed_original_request() {
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_, _, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let request = workspace_activation(&tmp, receipt, "bound-original-request");
    store
        .activate_planning_writeback(TENANT, PROJECT, A, &request)
        .await
        .unwrap();
    let mut different = request.clone();
    different.stopped_work_ids.push("a".into());
    assert!(
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &different)
            .await
            .is_err(),
        "a completed receipt must not authorize a different canonical request"
    );
}

#[tokio::test]
async fn writeback_checks_actual_live_projection_before_source_bytes() {
    use awr_team_pg::WorkstreamReadStore;
    use serde_json::json;
    let (_g, _admin, db, store) = store_and_roles().await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let (_, _, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
    let p = prepare(&read, A, "a").await;
    let claim = read.commands().execute(TENANT, PROJECT, A, command(&p, "preflight-claim", "claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":"0","ttl_seconds":600})))
        .await.unwrap()["receipt"]["data"].clone();
    let p = prepare(&read, A, "a").await;
    let execution = read.commands().execute(TENANT, PROJECT, A, command(&p, "preflight-intent", "execution.prepare",
        json!({"session_id":"session-a","expected_session_version":"1","claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":"a".repeat(64),"declared_scope":["src/api"]})))
        .await.unwrap()["receipt"]["data"].clone();
    let p = prepare(&read, A, "a").await;
    read.commands().execute(TENANT, PROJECT, A, command(&p, "preflight-start", "execution.start",
        json!({"session_id":"session-a","expected_session_version":"1","execution_id":execution["execution_id"],
        "expected_execution_version":execution["execution_version"],"claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"})))
        .await.unwrap();
    let tmp = tempfile_ledger();
    let before = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let mut request = workspace_activation(&tmp, receipt, "preflight-live-source");
    request.stopped_work_ids = vec!["a".into(), "CLIENT-1".into()];
    assert!(
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &request)
            .await
            .is_err()
    );
    assert_eq!(
        awr_source::fingerprint(&std::fs::read(tmp.root.join("ledger.yaml")).unwrap()),
        awr_source::fingerprint(&before),
        "the real live-effect gate must run before the first source-byte write"
    );
}

fn draft(id: &str, deps: &[&str], state: DraftDefinitionState) -> TaskDraft {
    TaskDraft {
        work_id: id.into(),
        external_key: id.into(),
        title: format!("Task {id}"),
        goals: vec!["delivery".into()],
        scope_paths: vec!["specs/api.md".into()],
        acceptance: vec!["ok".into()],
        required_dependencies: deps.iter().map(|s| (*s).into()).collect(),
        completion_policy: "independent_review".into(),
        dependency_acceptance: None,
        hard_rules: None,
        verification_requirements: None,
        execution_settlement: None,
        definition_state: state,
        workstream: None,
        split_from: None,
        split_children: vec![],
    }
}

fn workspace_task(id: &str, policy: &str) -> TaskDraft {
    let mut task = draft(id, &[], DraftDefinitionState::Enabled);
    task.workstream = Some("alpha".into());
    task.completion_policy = policy.into();
    task.execution_settlement = Some(awr_team::ExecutionSettlementPolicy {
        mode: awr_team::ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: format!("workspace-{id}"),
    });
    task.hard_rules = Some(vec!["Preserve recorded history".into()]);
    task.verification_requirements = Some(vec!["Run persistence regressions".into()]);
    task
}

async fn create_workspace_candidate(store: &SourceStore, change: DraftChange) -> serde_json::Value {
    store
        .create_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &DraftCandidateCreate {
                changes: vec![change],
                suggestion_ids: vec![],
                allowed_spec_roots: vec!["specs".into()],
                project_goal_keys: vec!["delivery".into()],
                self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
                author_person_id: Some("agent".into()),
                predetermined_candidate_id: None,
            },
        )
        .await
        .unwrap()
}

async fn approve_workspace_candidate(store: &SourceStore, candidate: &serde_json::Value) -> String {
    let id = candidate["candidate_id"].as_str().unwrap();
    let digest = candidate["candidate_digest"].as_str().unwrap();
    store
        .approve_planning_candidate(TENANT, PROJECT, A, id, digest, Some("agent"))
        .await
        .unwrap();
    let published = store
        .publish_planning_candidate(TENANT, PROJECT, A, id, digest)
        .await
        .unwrap();
    published["receipt_id"].as_str().unwrap().into()
}

fn workspace_activation(
    tmp: &TmpLedger,
    receipt: String,
    request: &str,
) -> WritebackActivateRequest {
    WritebackActivateRequest {
        request_id: request.into(),
        publish_receipt_id: receipt,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "The guard serializes the shared PostgreSQL fixture for this entire async test."
)]
async fn reviewed_workspace_contracts_roundtrip_reload_retain_and_replace_to_work_prepare() {
    use awr_team::{
        ExecutionSettlementPolicy as Policy, PLANNING_CODEC, PLANNING_CODEC_V4, WorkContract,
        planning_codec_for_changes,
    };
    use awr_team_pg::WorkstreamReadStore;
    use serde_json::{Value, json};

    let (_g, admin, db, store) = store_and_roles().await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let tmp = tempfile_ledger();
    for (key, policy) in [
        ("ORDINARY-1", Policy::COMPLETION_POLICY),
        ("SIMULATED-1", Policy::SIMULATED_MEMBER_COMPLETION_POLICY),
    ] {
        let original = workspace_task(key, policy);
        let mut omitted = original.clone();
        omitted.execution_settlement = None;
        omitted.hard_rules = None;
        omitted.verification_requirements = None;
        let mut renamed = omitted.clone();
        renamed.title = "Retain existing execution requirements".into();
        let mut replacement = original.clone();
        replacement
            .execution_settlement
            .as_mut()
            .unwrap()
            .workspace_id = format!("replacement-{key}");
        replacement.verification_requirements = Some(vec!["Verify API and process reload".into()]);

        for (index, change, expected, codec) in [
            (
                0,
                DraftChange {
                    op: DraftOpKind::CreateTask,
                    before: None,
                    after: original.clone(),
                },
                original.clone(),
                PLANNING_CODEC_V4,
            ),
            (
                1,
                DraftChange {
                    op: DraftOpKind::EditFields,
                    before: Some(omitted),
                    after: renamed,
                },
                original.clone(),
                if policy == Policy::COMPLETION_POLICY {
                    PLANNING_CODEC
                } else {
                    PLANNING_CODEC_V4
                },
            ),
            (
                2,
                DraftChange {
                    op: DraftOpKind::EditFields,
                    before: Some(original),
                    after: replacement.clone(),
                },
                replacement,
                PLANNING_CODEC_V4,
            ),
        ] {
            let created = create_workspace_candidate(&store, change).await;
            let id = created["candidate_id"].as_str().unwrap();
            let saved: Value = admin
                .query_one(
                    "SELECT changes_json FROM awr_team.planning_candidates WHERE id=$1",
                    &[&id],
                )
                .await
                .unwrap()
                .get(0);
            assert_eq!(
                planning_codec_for_changes(
                    &serde_json::from_value::<Vec<DraftChange>>(saved).unwrap()
                ),
                codec
            );
            // A fresh store must load the same candidate from PG rather than process memory.
            let reloaded =
                SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
            let preview = reloaded
                .preview_planning_candidate(TENANT, PROJECT, A, id)
                .await
                .unwrap();
            assert_eq!(
                preview["diff"]["candidate_digest"],
                created["candidate_digest"]
            );
            let fields = preview["diff"]["field_diffs"].as_array().unwrap();
            assert_eq!(
                fields.iter().any(|d| d["field"] == "execution_settlement"),
                index != 1
            );
            let receipt = approve_workspace_candidate(&reloaded, &created).await;
            reloaded
                .activate_planning_writeback(
                    TENANT,
                    PROJECT,
                    A,
                    &workspace_activation(&tmp, receipt, &format!("workspace-{key}-{index}")),
                )
                .await
                .unwrap();
            let contract = activated_contract(&admin, key).await;
            assert_eq!(
                contract.codec,
                if policy == Policy::COMPLETION_POLICY {
                    WorkContract::CODEC_V3
                } else {
                    WorkContract::CODEC_V4
                }
            );
            assert_eq!(contract.execution_settlement, expected.execution_settlement);
            assert_eq!(contract.completion_policy, expected.completion_policy);
            assert_eq!(contract.scope_paths, expected.scope_paths);
            assert_eq!(
                contract.verification_requirements,
                expected.verification_requirements.unwrap()
            );
            let prepared = prepare(&read, A, key).await;
            assert_eq!(prepared["data"]["context_complete"], true);
            assert_eq!(
                prepared["data"]["published_contract"]["execution_settlement"],
                json!(contract.execution_settlement)
            );
            assert_eq!(
                prepared["data"]["published_contract"]["completion_policy"],
                policy
            );
            assert_eq!(
                prepared["data"]["published_contract"]["verification_requirements"],
                json!(contract.verification_requirements)
            );
            assert_eq!(prepared["data"]["execution_admission"], "not_evaluated");
            let source = std::fs::read_to_string(tmp.root.join("ledger.yaml")).unwrap();
            assert!(source.contains(&format!("id: {key}")));
            assert!(source.contains(&contract.execution_settlement.as_ref().unwrap().workspace_id));
        }
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "The guard serializes the shared PostgreSQL fixture for this entire async test."
)]
async fn workspace_source_stale_priors_and_incomplete_retained_contracts_fail_before_mutation() {
    use awr_team::ExecutionSettlementPolicy as Policy;
    let (_g, admin, _db, store) = store_and_roles().await;
    let tmp = tempfile_ledger();
    let task = workspace_task("SIMULATED-1", Policy::SIMULATED_MEMBER_COMPLETION_POLICY);
    let created = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: task.clone(),
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &created).await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "workspace-create"),
        )
        .await
        .unwrap();
    let source_bytes = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let source_contract = activated_contract(&admin, "SIMULATED-1").await;
    let mut after = task.clone();
    after.execution_settlement.as_mut().unwrap().workspace_id = "new-workspace".into();
    let mut omitted = task.clone();
    omitted.execution_settlement = None;
    let mut stale = task.clone();
    stale.execution_settlement.as_mut().unwrap().workspace_id = "unobserved-workspace".into();
    let mut retained_invalid = omitted.clone();
    retained_invalid.verification_requirements = Some(vec![]);
    for (index, change) in [
        DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(omitted.clone()),
            after: after.clone(),
        },
        DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(stale.clone()),
            after,
        },
        DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(stale),
            after: omitted,
        },
        DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(task),
            after: retained_invalid,
        },
    ]
    .into_iter()
    .enumerate()
    {
        let created = create_workspace_candidate(&store, change).await;
        let receipt = approve_workspace_candidate(&store, &created).await;
        let result = store
            .activate_planning_writeback(
                TENANT,
                PROJECT,
                A,
                &workspace_activation(&tmp, receipt, &format!("workspace-stale-{index}")),
            )
            .await;
        assert!(result.is_err(), "{result:?}");
        assert_eq!(
            std::fs::read(tmp.root.join("ledger.yaml")).unwrap(),
            source_bytes
        );
        assert_eq!(
            activated_contract(&admin, "SIMULATED-1").await,
            source_contract
        );
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "The guard serializes the shared PostgreSQL fixture for this entire async test."
)]
async fn workspace_candidate_edit_requires_fresh_digest_approval_after_reload() {
    use awr_team::ExecutionSettlementPolicy as Policy;
    let (_g, admin, db, store) = store_and_roles().await;
    let original = workspace_task("SIMULATED-1", Policy::SIMULATED_MEMBER_COMPLETION_POLICY);
    let mut change = DraftChange {
        op: DraftOpKind::CreateTask,
        before: None,
        after: original,
    };
    let created = create_workspace_candidate(&store, change.clone()).await;
    let id = created["candidate_id"].as_str().unwrap();
    let old = created["candidate_digest"].as_str().unwrap();
    store
        .approve_planning_candidate(TENANT, PROJECT, A, id, old, Some("agent"))
        .await
        .unwrap();
    change
        .after
        .execution_settlement
        .as_mut()
        .unwrap()
        .workspace_id = "reviewed-replacement".into();
    let edited = store
        .edit_planning_candidate(TENANT, PROJECT, A, id, vec![change])
        .await
        .unwrap();
    let new = edited["candidate_digest"].as_str().unwrap();
    assert_ne!(old, new);
    assert_eq!(edited["prior_approval_cleared"], true);
    let saved = admin
        .query_one(
            "SELECT candidate_digest,state FROM awr_team.planning_candidates WHERE id=$1",
            &[&id],
        )
        .await
        .unwrap();
    assert_eq!(saved.get::<_, String>(0), new);
    assert_eq!(saved.get::<_, String>(1), "drafting");
    let reloaded = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    assert!(
        reloaded
            .publish_planning_candidate(TENANT, PROJECT, A, id, old)
            .await
            .is_err()
    );
    assert!(
        reloaded
            .publish_planning_candidate(TENANT, PROJECT, A, id, new)
            .await
            .is_err()
    );
    reloaded
        .approve_planning_candidate(TENANT, PROJECT, A, id, new, Some("agent"))
        .await
        .unwrap();
    let published = reloaded
        .publish_planning_candidate(TENANT, PROJECT, A, id, new)
        .await
        .unwrap();
    assert!(published["receipt_id"].is_string());
}

async fn store_and_roles() -> (
    std::sync::MutexGuard<'static, ()>,
    tokio_postgres::Client,
    String,
    SourceStore,
) {
    let (guard, admin, db, _read) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='admin', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
               ('reader-tenant','reader-project','API-1','API-1'),
               ('reader-tenant','reader-project','CLIENT-1','CLIENT-1'),
               ('reader-tenant','reader-project','OTHER-1','OTHER-1')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    (guard, admin, db, store)
}

async fn publish_candidate(store: &SourceStore) -> (String, String, String) {
    let create = DraftCandidateCreate {
        changes: vec![
            DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: draft("SHARED-1", &[], DraftDefinitionState::Draft),
            },
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("CLIENT-1", &["API-1"], DraftDefinitionState::Enabled)),
                after: draft(
                    "CLIENT-1",
                    &["API-1", "SHARED-1"],
                    DraftDefinitionState::Enabled,
                ),
            },
        ],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id: Some("agent".into()),
        predetermined_candidate_id: None,
    };
    let created = store
        .create_planning_candidate(TENANT, PROJECT, A, &create)
        .await
        .unwrap();
    let candidate_id = created["candidate_id"].as_str().unwrap().to_string();
    let digest = created["candidate_digest"].as_str().unwrap().to_string();
    store
        .approve_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest, Some("agent"))
        .await
        .unwrap();
    let published = store
        .publish_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest)
        .await
        .unwrap();
    let receipt_id = published["receipt_id"].as_str().unwrap().to_string();
    (candidate_id, digest, receipt_id)
}

async fn publish_candidate_with_workstream(
    store: &SourceStore,
    workstream: &str,
) -> (String, String, String) {
    let mut create_task = draft("SHARED-1", &[], DraftDefinitionState::Draft);
    create_task.workstream = Some(workstream.into());
    let create = DraftCandidateCreate {
        changes: vec![
            DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: create_task,
            },
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("CLIENT-1", &["API-1"], DraftDefinitionState::Enabled)),
                after: draft(
                    "CLIENT-1",
                    &["API-1", "SHARED-1"],
                    DraftDefinitionState::Enabled,
                ),
            },
        ],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id: Some("agent".into()),
        predetermined_candidate_id: None,
    };
    let created = store
        .create_planning_candidate(TENANT, PROJECT, A, &create)
        .await
        .unwrap();
    let candidate_id = created["candidate_id"].as_str().unwrap().to_string();
    let digest = created["candidate_digest"].as_str().unwrap().to_string();
    store
        .approve_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest, Some("agent"))
        .await
        .unwrap();
    let published = store
        .publish_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest)
        .await
        .unwrap();
    let receipt_id = published["receipt_id"].as_str().unwrap().to_string();
    (candidate_id, digest, receipt_id)
}

#[tokio::test]
async fn writeback_capabilities_and_runtime_fields_separated() {
    let caps = SourceStore::planning_writeback_capabilities();
    assert_eq!(caps["source_writeback"], "tmcp_022");
    assert_eq!(caps["project_claim_barrier_retained_when_unproven"], true);
    assert_eq!(
        caps["cancel_expiry_session_end_prove_process_stopped"],
        false
    );
    assert_eq!(caps["runtime_fields_writable_via_source"], false);
    assert_eq!(caps["source_status_is_completion_receipt"], false);
    assert_eq!(caps["idempotent_request_id"], true);
    let planning = SourceStore::planning_capabilities();
    assert_eq!(planning["source_writeback"], "tmcp_022");
}

#[tokio::test]
async fn unproven_impact_conservatively_refuses_with_recovery_actions() {
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate(&store).await;
    let tmp = tempfile_ledger();
    let err = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &WritebackActivateRequest {
                request_id: "req-unproven-1".into(),
                publish_receipt_id: receipt_id,
                source_root: tmp.root.clone(),
                ledger_relative_path: "ledger.yaml".into(),
                impact_proven: false,
                stopped_work_ids: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::ActivationImpactUnproven(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn an_unstarted_claim_is_not_an_external_effect_or_client_stop_proof() {
    use awr_team_pg::WorkstreamReadStore;
    use serde_json::json;
    let (_g, _admin, db, store) = store_and_roles().await;
    let (_, _, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let prepared = prepare(&read, A, "a").await;
    read.commands().execute(TENANT, PROJECT, A, command(&prepared, "unstarted-claim", "claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":"0","ttl_seconds":600})))
        .await.unwrap();
    let tmp = tempfile_ledger();
    let result = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "claim-is-not-effect"),
        )
        .await
        .unwrap();
    assert_eq!(result["source_writeback_pending"], false);
    assert!(
        result["affected_work_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "a")
    );
}

struct TmpLedger {
    root: PathBuf,
}

fn tempfile_ledger() -> TmpLedger {
    let root = std::env::temp_dir().join(format!(
        "awr-tmcp022-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    // Workstream ids/keys must retain the fixture catalog (Id::from(1/2)).
    let ledger = r#"workstreams:
  version: 1
  definitions:
    - id: 00000000000000000000000001
      external_key: alpha
      title: alpha
      state: active
      authority_version: 1
      goal_keys: [alpha]
      acceptance_contracts: []
    - id: 00000000000000000000000002
      external_key: private-beta
      title: private-beta
      state: active
      authority_version: 1
      goal_keys: [private-beta]
      acceptance_contracts: []
goals:
  - id: alpha
    title: Alpha
    status: active
  - id: private-beta
    title: Private
    status: active
work_items:
  - id: a
    title: Alpha work
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [verified]
    paths: [src]
    depends_on: []
  - id: b-private
    title: Private work
    status: planned
    workstream: private-beta
    goals: [private-beta]
    acceptance: [verified]
    paths: [src]
    depends_on: []
  - id: c
    title: Consumer
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [verified]
    paths: [src]
    depends_on: [b-private]
  - id: API-1
    title: Define
    status: completed
    workstream: alpha
    goals: [alpha]
    acceptance: [OpenAPI is reviewed]
    paths: [openapi.yaml]
    depends_on: []
  - id: CLIENT-1
    title: SDK
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [SDK smoke test passes]
    paths: [sdk/]
    depends_on: [API-1]
  - id: OTHER-1
    title: Unrelated
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [ok]
    paths: [other/]
    depends_on: []
"#;
    std::fs::write(root.join("ledger.yaml"), ledger).unwrap();
    TmpLedger { root }
}

#[tokio::test]
async fn refused_writeback_journal_is_durable_and_replay_stays_refused() {
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate(&store).await;
    let tmp = tempfile_ledger();
    let req = WritebackActivateRequest {
        request_id: "req-replay-1".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: false,
        stopped_work_ids: vec![],
    };
    let err1 = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap_err();
    assert!(
        matches!(err1, PgError::ActivationImpactUnproven(_)),
        "{err1:?}"
    );
    let err2 = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap_err();
    assert!(
        matches!(err2, PgError::ActivationImpactUnproven(_)),
        "{err2:?}"
    );
    // No activation receipt until success.
    let receipt = store
        .get_planning_activation_receipt(TENANT, PROJECT, A, "req-replay-1")
        .await
        .unwrap();
    assert!(receipt.is_none());
}

#[tokio::test]
async fn invalid_candidate_refuses_before_source_mutation() {
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate(&store).await;
    let tmp = tempfile_ledger();
    let before = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let req = WritebackActivateRequest {
        request_id: "req-validate-before-write".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    };
    let err = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("workstream"),
        "expected workstream ownership refusal, got {msg}"
    );
    let after = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    assert_eq!(
        before, after,
        "authoritative ledger must remain unchanged when validation fails"
    );
    let err2 = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap_err();
    let msg2 = format!("{err2:?}");
    assert!(
        msg2.contains("workstream"),
        "retry must stay a validation refusal, not fingerprint conflict: {msg2}"
    );
    assert!(
        !msg2.contains("fingerprint conflict"),
        "retry must not hit create fingerprint conflict: {msg2}"
    );
}

#[tokio::test]
async fn writeback_activate_success_and_idempotent_replay() {
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let req = WritebackActivateRequest {
        request_id: "req-success-1".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    };
    let first = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .expect("successful writeback activation");
    assert_eq!(first["already_recorded"], false);
    assert_eq!(first["source_bytes_written"], true);
    assert_eq!(first["source_writeback_pending"], false);
    let ledger = std::fs::read_to_string(tmp.root.join("ledger.yaml")).unwrap();
    assert!(ledger.contains("SHARED-1"));
    assert!(ledger.contains("workstream: alpha") || ledger.contains("workstream:alpha"));

    let second = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .expect("idempotent replay");
    assert_eq!(second["already_recorded"], true);
}

#[tokio::test]
async fn reviewed_policies_survive_reload_publish_activation_and_v1_omission() {
    use awr_team::{DependencyAcceptanceMode, PLANNING_CODEC, PLANNING_CODEC_V2, WorkContract};
    use serde_json::Value;
    use std::collections::BTreeMap;
    const POLICY: &str = "caller_managed_execution_and_agent_review";
    let (_g, admin, _db, store) = store_and_roles().await;
    let tmp = tempfile_ledger();
    let path = tmp.root.join("ledger.yaml");
    let ledger = std::fs::read_to_string(&path).unwrap();
    let client_end = "    depends_on: [API-1]\n  - id: OTHER-1";
    assert_eq!(ledger.matches(client_end).count(), 1);
    std::fs::write(&path, ledger.replace(client_end, &format!(
        "    depends_on: [API-1]\n    completion_policy: {POLICY}\n    dependency_acceptance:\n      API-1: agent_reviewed_caller_asserted_reconciled\n  - id: OTHER-1"
    ))).unwrap();

    let mut created_task = draft("SHARED-1", &[], DraftDefinitionState::Draft);
    created_task.workstream = Some("alpha".into());
    created_task.completion_policy = POLICY.into();
    let mut before = draft("CLIENT-1", &["API-1"], DraftDefinitionState::Enabled);
    before.completion_policy = POLICY.into();
    before.dependency_acceptance = Some(BTreeMap::from([(
        "API-1".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    )]));
    let mut after = before.clone();
    after.required_dependencies.push("SHARED-1".into());
    after.dependency_acceptance.as_mut().unwrap().insert(
        "SHARED-1".into(),
        DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
    );
    let expected_modes = after.dependency_acceptance.clone().unwrap();
    let created = store
        .create_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &DraftCandidateCreate {
                changes: vec![
                    DraftChange {
                        op: DraftOpKind::CreateTask,
                        before: None,
                        after: created_task,
                    },
                    DraftChange {
                        op: DraftOpKind::EditFields,
                        before: Some(before),
                        after: after.clone(),
                    },
                ],
                suggestion_ids: vec![],
                allowed_spec_roots: vec!["specs".into()],
                project_goal_keys: vec!["delivery".into()],
                self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
                author_person_id: Some("agent".into()),
                predetermined_candidate_id: None,
            },
        )
        .await
        .unwrap();
    let candidate_id = created["candidate_id"].as_str().unwrap();
    let digest = created["candidate_digest"].as_str().unwrap();
    let saved: Value = admin
        .query_one(
            "SELECT changes_json FROM awr_team.planning_candidates WHERE id=$1",
            &[&candidate_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        awr_team::planning_codec_for_changes(
            &serde_json::from_value::<Vec<DraftChange>>(saved).unwrap()
        ),
        PLANNING_CODEC_V2
    );
    // Every lifecycle operation reloads the candidate from PostgreSQL.
    let preview = store
        .preview_planning_candidate(TENANT, PROJECT, A, candidate_id)
        .await
        .unwrap();
    assert_eq!(preview["diff"]["candidate_digest"], digest);
    assert!(
        preview["diff"]["field_diffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["field"] == "dependency_acceptance")
    );
    store
        .approve_planning_candidate(TENANT, PROJECT, A, candidate_id, digest, Some("agent"))
        .await
        .unwrap();
    let published = store
        .publish_planning_candidate(TENANT, PROJECT, A, candidate_id, digest)
        .await
        .unwrap();
    let activate = WritebackActivateRequest {
        request_id: "reviewed-policy-activation".into(),
        publish_receipt_id: published["receipt_id"].as_str().unwrap().into(),
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    };
    store
        .activate_planning_writeback(TENANT, PROJECT, A, &activate)
        .await
        .unwrap();
    for (key, codec, modes) in [
        ("SHARED-1", WorkContract::CODEC, BTreeMap::new()),
        ("CLIENT-1", WorkContract::CODEC_V2, expected_modes.clone()),
    ] {
        let contract = activated_contract(&admin, key).await;
        assert_eq!(contract.completion_policy, POLICY);
        assert_eq!(contract.codec, codec);
        assert_eq!(contract.dependency_acceptance, modes);
    }
    assert_eq!(
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &activate)
            .await
            .unwrap()["already_recorded"],
        true
    );

    // A legacy V1 title edit does not erase or reinterpret the V2 source policy.
    after.dependency_acceptance = None;
    let mut renamed = after.clone();
    renamed.title = "Revised SDK".into();
    let created = store
        .create_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &DraftCandidateCreate {
                changes: vec![DraftChange {
                    op: DraftOpKind::EditFields,
                    before: Some(after),
                    after: renamed,
                }],
                suggestion_ids: vec![],
                allowed_spec_roots: vec!["specs".into()],
                project_goal_keys: vec!["delivery".into()],
                self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
                author_person_id: Some("agent".into()),
                predetermined_candidate_id: None,
            },
        )
        .await
        .unwrap();
    let candidate_id = created["candidate_id"].as_str().unwrap();
    let digest = created["candidate_digest"].as_str().unwrap();
    let saved: Value = admin
        .query_one(
            "SELECT changes_json FROM awr_team.planning_candidates WHERE id=$1",
            &[&candidate_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        awr_team::planning_codec_for_changes(
            &serde_json::from_value::<Vec<DraftChange>>(saved).unwrap()
        ),
        PLANNING_CODEC
    );
    store
        .approve_planning_candidate(TENANT, PROJECT, A, candidate_id, digest, Some("agent"))
        .await
        .unwrap();
    let published = store
        .publish_planning_candidate(TENANT, PROJECT, A, candidate_id, digest)
        .await
        .unwrap();
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &WritebackActivateRequest {
                request_id: "legacy-policy-retention".into(),
                publish_receipt_id: published["receipt_id"].as_str().unwrap().into(),
                source_root: tmp.root.clone(),
                ledger_relative_path: "ledger.yaml".into(),
                impact_proven: true,
                stopped_work_ids: vec![],
            },
        )
        .await
        .unwrap();
    let contract = activated_contract(&admin, "CLIENT-1").await;
    assert_eq!(contract.completion_policy, POLICY);
    assert_eq!(contract.dependency_acceptance, expected_modes);
    assert_eq!(contract.codec, WorkContract::CODEC_V2);
}

async fn activated_contract(admin: &tokio_postgres::Client, key: &str) -> awr_team::WorkContract {
    // Read only the active compiled contract in this process's isolated fixture.
    // SourceStore::current is intentionally unavailable in scoped Team mode.
    let row = admin
        .query_one(
            "SELECT c.contract_json,c.contract_hash
         FROM awr_team.projects p JOIN awr_team.work_contracts c
           ON c.tenant_id=p.tenant_id AND c.project_id=p.id AND c.snapshot_id=p.active_snapshot_id
         WHERE p.tenant_id=$1 AND p.id=$2 AND c.work_id=$3",
            &[&TENANT, &PROJECT, &key],
        )
        .await
        .unwrap();
    let contract: awr_team::WorkContract = serde_json::from_value(row.get(0)).unwrap();
    assert_eq!(contract.hash().unwrap(), row.get::<_, String>(1));
    contract
}

#[tokio::test]
async fn reviewed_context_lists_roundtrip_through_pg_to_real_work_prepare() {
    use awr_team::{PLANNING_CODEC, PLANNING_CODEC_V3, planning_codec_for_changes};
    use awr_team_pg::WorkstreamReadStore;
    use serde_json::{Value, json};

    let (_g, admin, db, store) = store_and_roles().await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let tmp = tempfile_ledger();
    let mut task = draft("SHARED-1", &[], DraftDefinitionState::Enabled);
    task.workstream = Some("alpha".into());
    task.hard_rules = Some(vec!["Preserve recorded history".into()]);
    task.verification_requirements = Some(vec!["Run persistence regressions".into()]);
    let original = task.clone();
    let mut legacy_before = task.clone();
    legacy_before.hard_rules = None;
    legacy_before.verification_requirements = None;
    let mut legacy_after = legacy_before.clone();
    legacy_after.title = "Renamed shared work".into();
    let mut replaced = task.clone();
    replaced.hard_rules = Some(vec!["Preserve all issue identities".into()]);
    replaced.verification_requirements = Some(vec!["Verify API and process reload".into()]);
    let mut cleared = replaced.clone();
    cleared.hard_rules = Some(vec![]);
    cleared.verification_requirements = Some(vec![]);

    for (index, change, expected, codec) in [
        (
            0,
            DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: task,
            },
            original.clone(),
            PLANNING_CODEC_V3,
        ),
        (
            1,
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(legacy_before),
                after: legacy_after,
            },
            original.clone(),
            PLANNING_CODEC,
        ),
        (
            2,
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(original),
                after: replaced.clone(),
            },
            replaced.clone(),
            PLANNING_CODEC_V3,
        ),
        (
            3,
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(replaced),
                after: cleared.clone(),
            },
            cleared,
            PLANNING_CODEC_V3,
        ),
    ] {
        let created =
            store
                .create_planning_candidate(
                    TENANT,
                    PROJECT,
                    A,
                    &DraftCandidateCreate {
                        changes: vec![change],
                        suggestion_ids: vec![],
                        allowed_spec_roots: vec!["specs".into()],
                        project_goal_keys: vec!["delivery".into()],
                        self_approve_policy: Some(
                            OrdinaryPlanningSelfApprovePolicy::ordinary_default(),
                        ),
                        author_person_id: Some("agent".into()),
                        predetermined_candidate_id: None,
                    },
                )
                .await
                .unwrap();
        let candidate_id = created["candidate_id"].as_str().unwrap();
        let digest = created["candidate_digest"].as_str().unwrap();
        let saved: Value = admin
            .query_one(
                "SELECT changes_json FROM awr_team.planning_candidates WHERE id=$1",
                &[&candidate_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            planning_codec_for_changes(&serde_json::from_value::<Vec<DraftChange>>(saved).unwrap()),
            codec
        );
        let preview = store
            .preview_planning_candidate(TENANT, PROJECT, A, candidate_id)
            .await
            .unwrap();
        assert_eq!(preview["diff"]["candidate_digest"], digest);
        let fields = preview["diff"]["field_diffs"].as_array().unwrap();
        for field in ["hard_rules", "verification_requirements"] {
            assert_eq!(fields.iter().any(|d| d["field"] == field), index != 1);
        }
        store
            .approve_planning_candidate(TENANT, PROJECT, A, candidate_id, digest, Some("agent"))
            .await
            .unwrap();
        let published = store
            .publish_planning_candidate(TENANT, PROJECT, A, candidate_id, digest)
            .await
            .unwrap();
        let request = WritebackActivateRequest {
            request_id: format!("required-context-{index}"),
            publish_receipt_id: published["receipt_id"].as_str().unwrap().into(),
            source_root: tmp.root.clone(),
            ledger_relative_path: "ledger.yaml".into(),
            impact_proven: true,
            stopped_work_ids: vec![],
        };
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &request)
            .await
            .unwrap();
        let contract = activated_contract(&admin, "SHARED-1").await;
        assert_eq!(contract.hard_rules, expected.hard_rules.unwrap());
        assert_eq!(
            contract.verification_requirements,
            expected.verification_requirements.unwrap()
        );
        let prepared = prepare(&read, A, "SHARED-1").await;
        assert_eq!(prepared["data"]["context_complete"], index != 3);
        assert_eq!(
            prepared["data"]["published_contract"]["hard_rules"],
            json!(contract.hard_rules)
        );
        assert_eq!(
            prepared["data"]["published_contract"]["verification_requirements"],
            json!(contract.verification_requirements)
        );
        assert_eq!(prepared["data"]["execution_admission"], "not_evaluated");
        if index == 3 {
            assert!(
                prepared["data"]["completeness_reasons"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("missing_hard_rules"))
            );
        }
        assert_eq!(
            store
                .activate_planning_writeback(TENANT, PROJECT, A, &request)
                .await
                .unwrap()["already_recorded"],
            true
        );
    }
}

#[tokio::test]
async fn writeback_storage_failure_retains_journal_and_resumes_the_same_request() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let path = tmp.root.join("ledger.yaml");
    let before = std::fs::read(&path).unwrap();
    // Explicit read-only source protection is deterministic even when tests run
    // as root and retains the intent before the replacement is attempted.
    let permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    let req = WritebackActivateRequest {
        request_id: "req-storage-recovery".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    };
    let error = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap_err();
    assert!(error.source_storage_reason().is_some(), "{error:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        store
            .get_planning_activation_receipt(TENANT, PROJECT, A, &req.request_id)
            .await
            .unwrap()
            .is_none()
    );
    let phase: String = admin
        .query_one(
            "SELECT phase FROM awr_team.planning_writeback_journals
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &req.request_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        phase, "validated",
        "the request retains a recoverable journal"
    );

    std::fs::set_permissions(&path, permissions).unwrap();
    let resumed = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .expect("the original request resumes after storage is repaired");
    assert_eq!(resumed["already_recorded"], false);
    let replay = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(replay["already_recorded"], true);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("id: SHARED-1").count(), 1);
}

#[tokio::test]
async fn unreadable_writeback_source_is_storage_failure_not_invalid_input() {
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    std::fs::remove_file(tmp.root.join("ledger.yaml")).unwrap();
    let req = WritebackActivateRequest {
        request_id: "req-source-unreadable".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    };
    let error = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap_err();
    assert!(error.source_storage_reason().is_some(), "{error:?}");
}

#[tokio::test]
async fn cooperative_source_writer_blocks_planning_until_the_same_guard_is_released() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let guard = awr_source::LockedSourceFile::open(&tmp.root, "ledger.yaml").unwrap();
    let before = guard.read().unwrap();
    let req = WritebackActivateRequest {
        request_id: "req-shared-source-lock".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    };
    assert!(matches!(
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await,
        Err(PgError::WritebackRefused(_))
    ));
    assert_eq!(guard.read().unwrap(), before);
    assert_eq!(
        admin
            .query_one(
                "SELECT count(*) FROM awr_team.planning_activation_receipts WHERE request_id=$1",
                &[&req.request_id]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    drop(guard);
    store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(tmp.root.join("ledger.yaml"))
            .unwrap()
            .matches("id: SHARED-1")
            .count(),
        1
    );
}

#[tokio::test]
async fn planning_preserves_existing_delivery_reference_notes_in_the_sole_source() {
    use awr_source::{DeliverySourceNote, LockedSourceFile, prepare_delivery_source_note};
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    // A synthetic source metadata reference, not a domain completion receipt.
    let note = DeliverySourceNote {
        version: 1,
        publication_id: "synthetic-publication".into(),
        work_external_key: "a".into(),
        contract_snapshot_id: "synthetic-snapshot".into(),
        candidate_id: "candidate-a".into(),
        candidate_version: "1".into(),
        candidate_digest: "a".repeat(64),
        selection_version: "1".into(),
        metadata_revision: "1".into(),
        observation_receipt_ids: vec!["synthetic-observation".into()],
        fact_ids: vec!["synthetic-fact".into()],
        completion_reference: None,
    };
    let guard = LockedSourceFile::open(&tmp.root, "ledger.yaml").unwrap();
    let patch = prepare_delivery_source_note(&guard.read().unwrap(), &note).unwrap();
    guard
        .replace(&patch.before_fingerprint, &patch.after_bytes)
        .unwrap();
    drop(guard);
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &WritebackActivateRequest {
                request_id: "req-preserve-delivery-note".into(),
                publish_receipt_id: receipt_id,
                source_root: tmp.root.clone(),
                ledger_relative_path: "ledger.yaml".into(),
                impact_proven: true,
                stopped_work_ids: vec![],
            },
        )
        .await
        .unwrap();
    let after = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let check = prepare_delivery_source_note(&after, &note).unwrap();
    assert!(
        check.changed_external_keys.is_empty(),
        "the exact typed note is still present"
    );
    assert_eq!(check.after_bytes, after);
    let location =
        awr_source::SoleSourceLocation::server_directory(&tmp.root, "ledger.yaml").unwrap();
    let package = awr_source::prepare_publish_from_ledger_bytes(
        &location,
        &tmp.root,
        &after,
        PROJECT,
        &awr_source::PublishPrepOptions::default(),
    )
    .unwrap();
    assert_eq!(
        package
            .source_status_notes
            .iter()
            .find(|n| n.work_external_key == "a")
            .unwrap()
            .raw_status,
        "planned"
    );
}

// Capture durable facts separately from the source bytes. A retained intent is
// not an activation, and a lost response must not increment either fact twice.
async fn activation_facts(admin: &tokio_postgres::Client) -> (String, i64, i64, i64) {
    let row = admin.query_one(
        "SELECT active_snapshot_id, authority_epoch,
          (SELECT count(*) FROM awr_team.events WHERE tenant_id=$1 AND project_id=$2 AND event_type='source.activated'),
          (SELECT count(*) FROM awr_team.planning_activation_receipts WHERE tenant_id=$1 AND project_id=$2)
         FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
        &[&TENANT, &PROJECT],
    ).await.unwrap();
    (row.get(0), row.get(1), row.get(2), row.get(3))
}

#[tokio::test]
async fn durable_boundaries_recover_original_intent_and_commit_activation_once() {
    use awr_source::fingerprint;
    for (boundary, phase, written, completed) in [
        ("after_intent", "validated", false, false),
        ("after_source_write", "validated", true, false),
        ("after_source_written", "source_written", true, false),
        ("after_pg_activating", "pg_activating", true, false),
        ("before_final_commit", "pg_activating", true, false),
        ("after_final_commit", "completed", true, true),
    ] {
        let (_g, admin, db, store) = store_and_roles().await;
        let (_, _, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
        let tmp = tempfile_ledger();
        let req = workspace_activation(&tmp, receipt, boundary);
        let path = tmp.root.join("ledger.yaml");
        let before = fingerprint(&std::fs::read(&path).unwrap());
        let baseline = activation_facts(&admin).await;
        let error = store
            .activate_planning_writeback_abort_for_test(TENANT, PROJECT, A, &req, boundary)
            .await
            .unwrap_err();
        assert!(
            matches!(error, PgError::Protocol(ref text) if text.contains("injected writeback interruption")),
            "{boundary}: {error:?}"
        );
        // A new store has no in-memory recovery knowledge.
        let restarted =
            SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
        let status = restarted
            .get_planning_writeback_status(TENANT, PROJECT, A, &req.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status["phase"], phase, "{boundary}");
        assert_eq!(status["original_intent_bound"], true, "{boundary}");
        assert_eq!(status["pending"], !completed, "{boundary}");
        assert_eq!(status["applied"], completed, "{boundary}");
        let journal = admin.query_one(
            "SELECT before_fingerprint, after_fingerprint, intent_json FROM awr_team.planning_writeback_journals
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &req.request_id],
        ).await.unwrap();
        assert_eq!(journal.get::<_, String>(0), before);
        let after: String = journal.get(1);
        let intent: serde_json::Value = journal.get(2);
        assert_eq!(intent["request"], serde_json::to_value(&req).unwrap());
        assert_eq!(intent["actor_id"], "agent");
        assert_eq!(intent["client_id"], "cli-a");
        assert_eq!(
            fingerprint(&std::fs::read(&path).unwrap()),
            if written { after.clone() } else { before }
        );
        let interrupted = activation_facts(&admin).await;
        if completed {
            assert_ne!(interrupted.0, baseline.0);
            assert_eq!(
                (interrupted.1, interrupted.2, interrupted.3),
                (baseline.1 + 1, baseline.2 + 1, baseline.3 + 1)
            );
        } else {
            assert_eq!(
                interrupted, baseline,
                "activation/event/receipt must roll back together at {boundary}"
            );
            assert!(
                restarted
                    .get_planning_activation_receipt(TENANT, PROJECT, A, &req.request_id)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        let pending: bool = admin.query_one(
            "SELECT source_writeback_pending FROM awr_team.planning_publish_receipts WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &req.publish_receipt_id],
        ).await.unwrap().get(0);
        assert_eq!(pending, !completed, "{boundary}");
        let resumed = restarted
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap();
        assert_eq!(resumed["already_recorded"], completed);
        let final_facts = activation_facts(&admin).await;
        assert_eq!(
            (final_facts.1, final_facts.2, final_facts.3),
            (baseline.1 + 1, baseline.2 + 1, baseline.3 + 1)
        );
        let canonical_receipt = restarted
            .get_planning_activation_receipt(TENANT, PROJECT, A, &req.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(canonical_receipt["activated_snapshot_id"], final_facts.0);
        let file_time = std::fs::metadata(&path).unwrap().modified().unwrap();
        let replay = restarted
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap();
        assert_eq!(replay["already_recorded"], true);
        assert_eq!(
            replay["receipt"], canonical_receipt,
            "the exact stored receipt is returned"
        );
        assert_eq!(activation_facts(&admin).await, final_facts);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            file_time,
            "completed replay must not rewrite the source"
        );
        assert_eq!(fingerprint(&std::fs::read(&path).unwrap()), after);
        assert_eq!(
            std::fs::read_to_string(&path)
                .unwrap()
                .matches("id: SHARED-1")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn pending_and_completed_writeback_bind_every_request_field_and_actual_client() {
    let (_g, admin, _db, store) = store_and_roles().await;
    // The alternative client has the same live publisher permission and scope;
    // rejection must result from identity binding, not a missing grant.
    admin.batch_execute(
        "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
         VALUES('reader-tenant','reader-project','agent','cli-b','00000000000000000000000001',1,true,true);",
    ).await.unwrap();
    let (_, _, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let alternate = tempfile_ledger();
    let req = workspace_activation(&tmp, receipt, "request-and-client-binding");
    store
        .activate_planning_writeback_abort_for_test(TENANT, PROJECT, A, &req, "after_intent")
        .await
        .unwrap_err();
    for completed in [false, true] {
        let original = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
        let alternate_bytes = std::fs::read(alternate.root.join("ledger.yaml")).unwrap();
        let baseline = activation_facts(&admin).await;
        let mut variants = vec![];
        let mut changed = req.clone();
        changed.source_root = alternate.root.clone();
        variants.push(changed);
        let mut changed = req.clone();
        changed.ledger_relative_path = "other.yaml".into();
        variants.push(changed);
        let mut changed = req.clone();
        changed.publish_receipt_id = "another-publication".into();
        variants.push(changed);
        let mut changed = req.clone();
        changed.impact_proven = false;
        variants.push(changed);
        let mut changed = req.clone();
        changed.stopped_work_ids.push("CLIENT-1".into());
        variants.push(changed);
        for changed in variants {
            let error = store
                .activate_planning_writeback(TENANT, PROJECT, A, &changed)
                .await
                .unwrap_err();
            assert!(
                matches!(error, PgError::WritebackRefused(ref text) if text.contains("different original intent or identity")),
                "completed={completed}: {error:?}"
            );
        }
        let error = store
            .activate_planning_writeback(TENANT, PROJECT, B, &req)
            .await
            .unwrap_err();
        assert!(
            matches!(error, PgError::WritebackRefused(ref text) if text.contains("different original intent or identity")),
            "completed={completed}: {error:?}"
        );
        assert_eq!(activation_facts(&admin).await, baseline);
        assert_eq!(
            std::fs::read(tmp.root.join("ledger.yaml")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(alternate.root.join("ledger.yaml")).unwrap(),
            alternate_bytes
        );
        if !completed {
            store
                .activate_planning_writeback(TENANT, PROJECT, A, &req)
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn pending_writeback_preserves_drift_and_rechecks_revoked_permission_after_restart() {
    for drift in [false, true] {
        let (_g, admin, db, store) = store_and_roles().await;
        let (_, _, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
        let tmp = tempfile_ledger();
        let req = workspace_activation(&tmp, receipt, "recovery-live-authority");
        store
            .activate_planning_writeback_abort_for_test(
                TENANT,
                PROJECT,
                A,
                &req,
                "after_source_written",
            )
            .await
            .unwrap_err();
        let baseline = activation_facts(&admin).await;
        let path = tmp.root.join("ledger.yaml");
        if drift {
            let mut bytes = std::fs::read(&path).unwrap();
            bytes.extend_from_slice(b"\n# A concurrent author's content must remain intact.\n");
            std::fs::write(&path, bytes).unwrap();
        } else {
            admin.batch_execute("UPDATE awr_team.project_memberships SET role='reader',membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        let restarted =
            SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
        let error = restarted
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap_err();
        if drift {
            assert!(matches!(error, PgError::WritebackRefused(_)), "{error:?}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(activation_facts(&admin).await, baseline);
        let phase: String = admin.query_one("SELECT phase FROM awr_team.planning_writeback_journals WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3", &[&TENANT,&PROJECT,&req.request_id]).await.unwrap().get(0);
        assert_eq!(phase, "source_written");
        if !drift {
            admin.batch_execute("UPDATE awr_team.project_memberships SET role='admin',membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
            restarted
                .activate_planning_writeback(TENANT, PROJECT, A, &req)
                .await
                .unwrap();
            let facts = activation_facts(&admin).await;
            assert_eq!(
                (facts.1, facts.2, facts.3),
                (baseline.1 + 1, baseline.2 + 1, baseline.3 + 1)
            );
        }
    }
}

#[tokio::test]
async fn legacy_pending_journals_without_original_provenance_never_authorize_recovery() {
    use awr_source::apply_planning_changes_to_ledger;
    let (_g, admin, _db, store) = store_and_roles().await;
    let (candidate_id, digest, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let path = tmp.root.join("ledger.yaml");
    let before = std::fs::read(&path).unwrap();
    let mut created = draft("SHARED-1", &[], DraftDefinitionState::Draft);
    created.workstream = Some("alpha".into());
    let changes = vec![
        DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: created,
        },
        DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("CLIENT-1", &["API-1"], DraftDefinitionState::Enabled)),
            after: draft(
                "CLIENT-1",
                &["API-1", "SHARED-1"],
                DraftDefinitionState::Enabled,
            ),
        },
    ];
    let patch = apply_planning_changes_to_ledger(&before, &changes).unwrap();
    for (i, (phase, bytes)) in [
        ("validated", before.as_slice()),
        ("validated", patch.after_bytes.as_slice()),
        ("source_written", patch.after_bytes.as_slice()),
        ("pg_activating", patch.after_bytes.as_slice()),
        ("completed", patch.after_bytes.as_slice()),
        (
            "validated",
            b"# preserve unknown external bytes\n".as_slice(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        std::fs::write(&path, bytes).unwrap();
        let request = format!("legacy-unknown-{i}");
        // This deliberately retains an old unbound row; it is not a crash
        // fixture with fabricated new provenance and earns no recovery credit.
        admin.execute(
            "INSERT INTO awr_team.planning_writeback_journals(tenant_id,project_id,request_id,candidate_id,candidate_digest,
             publish_receipt_id,phase,before_fingerprint,after_fingerprint,publisher_actor_id,affected_work_ids,
             unrelated_work_ids,recovery_actions,body_json)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'agent','[]','[]','[]','{}')",
            &[&TENANT,&PROJECT,&request,&candidate_id,&digest,&receipt,&phase,&patch.before_fingerprint,&patch.after_fingerprint],
        ).await.unwrap();
        let baseline = activation_facts(&admin).await;
        let req = workspace_activation(&tmp, receipt.clone(), &request);
        let error = store
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap_err();
        assert!(
            matches!(error, PgError::WritebackRefused(ref text) if text.contains("original writeback intent is unknown")),
            "{error:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(activation_facts(&admin).await, baseline);
        let outcome = store
            .get_planning_writeback_status(TENANT, PROJECT, A, &request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome["phase"], phase);
        assert_eq!(outcome["original_intent_bound"], false);
        assert_eq!(outcome["applied"], false);
        assert_eq!(outcome["source_writeback_pending"], true);
        assert!(outcome["activation_receipt_id"].is_null());
    }
}

#[tokio::test]
async fn completed_outcome_requires_valid_intent_and_matching_activation_confirmation() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let (_, _, publication) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let req = workspace_activation(&tmp, publication, "outcome-provenance");
    let activated = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    let first = store
        .get_planning_writeback_status(TENANT, PROJECT, A, &req.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first["applied"], true);
    let original_id = first["activation_receipt_id"].as_str().unwrap();
    let bytes = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let baseline = activation_facts(&admin).await;
    admin.execute("UPDATE awr_team.planning_activation_receipts SET after_fingerprint=$2 WHERE request_id=$1",
        &[&req.request_id,&"changed-confirmation"]).await.unwrap();
    let inconsistent = store
        .get_planning_writeback_status(TENANT, PROJECT, A, &req.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inconsistent["original_intent_bound"], true);
    assert_eq!(inconsistent["applied"], false);
    assert!(matches!(
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap_err(),
        PgError::SourceDivergence
    ));
    admin.execute("UPDATE awr_team.planning_activation_receipts SET after_fingerprint=$2 WHERE request_id=$1",
        &[&req.request_id,&activated["after_fingerprint"].as_str().unwrap()]).await.unwrap();
    let snapshot = activated["activated_snapshot_id"].as_str().unwrap();
    let manifest: String = admin
        .query_one(
            "SELECT manifest_digest FROM awr_team.source_snapshots WHERE id=$1",
            &[&snapshot],
        )
        .await
        .unwrap()
        .get(0);
    admin
        .execute(
            "UPDATE awr_team.source_snapshots SET manifest_digest=$2 WHERE id=$1",
            &[&snapshot, &"changed-snapshot"],
        )
        .await
        .unwrap();
    let inconsistent = store
        .get_planning_writeback_status(TENANT, PROJECT, A, &req.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inconsistent["original_intent_bound"], true);
    assert_eq!(inconsistent["applied"], false);
    assert!(matches!(
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap_err(),
        PgError::SourceDivergence
    ));
    admin
        .execute(
            "UPDATE awr_team.source_snapshots SET manifest_digest=$2 WHERE id=$1",
            &[&snapshot, &manifest],
        )
        .await
        .unwrap();
    // Corrupt diagnostics must not promote a phase label to confirmed delivery.
    admin.execute("UPDATE awr_team.planning_writeback_journals SET audit_receipt_id=NULL WHERE request_id=$1",&[&req.request_id]).await.unwrap();
    let missing = store
        .get_planning_writeback_status(TENANT, PROJECT, A, &req.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(missing["original_intent_bound"], true);
    assert_eq!(missing["applied"], false);
    assert!(missing["activation_receipt_id"].is_null());
    admin.execute("UPDATE awr_team.planning_writeback_journals SET audit_receipt_id=$2,intent_hash=$3 WHERE request_id=$1",
        &[&req.request_id,&original_id,&"f".repeat(64)]).await.unwrap();
    let invalid = store
        .get_planning_writeback_status(TENANT, PROJECT, A, &req.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(invalid["original_intent_bound"], false);
    assert_eq!(invalid["applied"], false);
    assert!(
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(tmp.root.join("ledger.yaml")).unwrap(), bytes);
    assert_eq!(activation_facts(&admin).await, baseline);
}

async fn register_actual_ledger_baseline(store: &SourceStore, tmp: &TmpLedger) {
    use awr_source::{PublishPrepOptions, SoleSourceLocation, prepare_publish_from_ledger_bytes};
    use awr_team::SourceActivationPlan;
    use awr_team_pg::{IngestRequest, SourceFile};
    let location = SoleSourceLocation::server_directory(&tmp.root, "ledger.yaml").unwrap();
    let package = prepare_publish_from_ledger_bytes(
        &location,
        &tmp.root,
        &std::fs::read(tmp.root.join("ledger.yaml")).unwrap(),
        PROJECT,
        &PublishPrepOptions::default(),
    )
    .unwrap();
    let (candidate, _) = store
        .ingest_publish_candidate(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: package.parser_version,
            files: package
                .files
                .into_iter()
                .map(|f| SourceFile {
                    path: f.path,
                    bytes: f.bytes,
                })
                .collect(),
        })
        .await
        .unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            "reviewer",
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    store
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &candidate.proposal_id,
            &SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                approved_candidate_digest: candidate.manifest_digest,
                parser_version: candidate.parser_version,
                expected_authority_epoch: candidate.base_epoch,
            },
        )
        .await
        .unwrap();
}

async fn publish_real_scope_change(store: &SourceStore, admin: &tokio_postgres::Client) -> String {
    let contract = activated_contract(admin, "a").await;
    let mut before = draft("a", &[], DraftDefinitionState::Enabled);
    before.title = "Alpha work".into();
    before.goals = contract.goals;
    before.scope_paths = contract.scope_paths;
    before.acceptance = contract.acceptance;
    before.completion_policy = contract.completion_policy;
    before.workstream = Some("alpha".into());
    let mut after = before.clone();
    after.scope_paths = vec!["src/api".into()];
    let candidate = store
        .create_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &DraftCandidateCreate {
                changes: vec![DraftChange {
                    op: DraftOpKind::EditFields,
                    before: Some(before),
                    after,
                }],
                suggestion_ids: vec![],
                allowed_spec_roots: vec!["src".into()],
                project_goal_keys: vec!["alpha".into(), "private-beta".into()],
                self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
                author_person_id: Some("agent".into()),
                predetermined_candidate_id: None,
            },
        )
        .await
        .unwrap();
    approve_workspace_candidate(store, &candidate).await
}

async fn claim_and_prepare_scope(
    read: &awr_team_pg::WorkstreamReadStore,
    token: &str,
    work: &str,
    session: &str,
    key: &str,
    scope: &str,
) -> (serde_json::Value, serde_json::Value) {
    let claim = claim_scope(read, token, work, session, key).await;
    let prepared = read
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            scope_prepare_command(read, token, work, session, key, scope, &claim).await,
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    (claim, prepared)
}

async fn claim_scope(
    read: &awr_team_pg::WorkstreamReadStore,
    token: &str,
    work: &str,
    session: &str,
    key: &str,
) -> serde_json::Value {
    use serde_json::json;
    let p = prepare(read, token, work).await;
    read.commands().execute(TENANT, PROJECT, token, command(&p, &format!("{key}-claim"), "claim.acquire",
        json!({"session_id":session,"expected_session_version":"1","expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":600})))
        .await.unwrap()["receipt"]["data"].clone()
}

async fn scope_prepare_command(
    read: &awr_team_pg::WorkstreamReadStore,
    token: &str,
    work: &str,
    session: &str,
    key: &str,
    scope: &str,
    claim: &serde_json::Value,
) -> awr_team_pg::WorkstreamCommand {
    let p = prepare(read, token, work).await;
    command(
        &p,
        &format!("{key}-prepare"),
        "execution.prepare",
        serde_json::json!({
            "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":"a".repeat(64),"declared_scope":[scope]
        }),
    )
}

async fn scope_start_command(
    read: &awr_team_pg::WorkstreamReadStore,
    token: &str,
    work: &str,
    session: &str,
    key: &str,
    claim: &serde_json::Value,
    execution: &serde_json::Value,
) -> awr_team_pg::WorkstreamCommand {
    let p = prepare(read, token, work).await;
    command(
        &p,
        &format!("{key}-start"),
        "execution.start",
        serde_json::json!({
            "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "expected_work_version":p["data"]["runtime"]["work_version"],"execution_id":execution["execution_id"],
            "expected_execution_version":execution["execution_version"],"execution_mode":"caller_managed"
        }),
    )
}

#[tokio::test]
async fn pending_intent_fences_related_prepare_start_and_source_replacement_but_preserves_unrelated_execution()
 {
    use awr_team_pg::WorkstreamReadStore;
    for (boundary, prepared_before) in [
        ("after_intent", false),
        ("after_intent", true),
        ("after_source_written", false),
        ("after_source_written", true),
        ("after_pg_activating", false),
        ("after_pg_activating", true),
    ] {
        let (_g, admin, db, store) = store_and_roles().await;
        let tmp = tempfile_ledger();
        // Align the actual source first. Initial fixture differences must not
        // falsely classify unrelated work as affected by this later change.
        register_actual_ledger_baseline(&store, &tmp).await;
        admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1 WHERE client_id='cli-b'").await.unwrap();
        let read =
            WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
        let claim_a = claim_scope(&read, A, "a", "session-a", "related").await;
        let prepared_a = if prepared_before {
            Some(
                read.commands()
                    .execute(
                        TENANT,
                        PROJECT,
                        A,
                        scope_prepare_command(
                            &read,
                            A,
                            "a",
                            "session-a",
                            "before",
                            "src/api",
                            &claim_a,
                        )
                        .await,
                    )
                    .await
                    .unwrap()["receipt"]["data"]
                    .clone(),
            )
        } else {
            None
        };
        let publication = publish_real_scope_change(&store, &admin).await;
        let req = workspace_activation(&tmp, publication, boundary);
        store
            .activate_planning_writeback_abort_for_test(TENANT, PROJECT, A, &req, boundary)
            .await
            .unwrap_err();
        let intent: serde_json::Value = admin.query_one("SELECT intent_json FROM awr_team.planning_writeback_journals WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3", &[&TENANT,&PROJECT,&req.request_id]).await.unwrap().get(0);
        assert_eq!(intent["affected_work_ids"], serde_json::json!(["a"]));
        assert_eq!(intent["dependency_work_ids"], serde_json::json!(["a"]));
        let before = activation_facts(&admin).await;
        let cmd = if let Some(execution) = &prepared_a {
            scope_start_command(&read, A, "a", "session-a", "blocked", &claim_a, execution).await
        } else {
            scope_prepare_command(&read, A, "a", "session-a", "blocked", "src/api", &claim_a).await
        };
        let error = read
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap_err();
        assert!(
            matches!(error,PgError::ActionBlockedByInvalidation(ref key) if key==&format!("source_writeback:{boundary}")),
            "{boundary}: {error:?}"
        );
        // A different request cannot replace the same authority while the
        // original file intent is pending, even with identical caller flags.
        let mut competing = req.clone();
        competing.request_id = "competing-source-request".into();
        assert!(matches!(
            store
                .activate_planning_writeback(TENANT, PROJECT, A, &competing)
                .await,
            Err(PgError::WritebackRefused(_))
        ));
        assert_eq!(activation_facts(&admin).await, before);
        let (claim_b, prepared_b) =
            claim_and_prepare_scope(&read, B, "b-private", "session-b", "unrelated", "src/other")
                .await;
        let started_b = read
            .commands()
            .execute(
                TENANT,
                PROJECT,
                B,
                scope_start_command(
                    &read,
                    B,
                    "b-private",
                    "session-b",
                    "unrelated",
                    &claim_b,
                    &prepared_b,
                )
                .await,
            )
            .await
            .unwrap()["receipt"]["data"]
            .clone();
        assert_eq!(started_b["state"], "running");
        let restarted =
            SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
        restarted
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap();
        let states = admin
            .query("SELECT id,state FROM awr_team.executions ORDER BY id", &[])
            .await
            .unwrap();
        if let Some(execution) = &prepared_a {
            assert!(states.iter().any(|r| r.get::<_, String>(0)
                == execution["execution_id"].as_str().unwrap()
                && r.get::<_, String>(1) == "cancelled"));
        }
        assert!(states.iter().any(|r| r.get::<_, String>(0)
            == prepared_b["execution_id"].as_str().unwrap()
            && r.get::<_, String>(1) == "running"));
    }
}

#[tokio::test]
async fn abandoning_an_untouched_pending_writeback_reopens_public_execution_admission() {
    use awr_source::fingerprint;
    use awr_team_pg::WorkstreamReadStore;
    for prepared_before in [true, false] {
        let (_guard, mut admin, db, store) = store_and_roles().await;
        let tmp = tempfile_ledger();
        register_actual_ledger_baseline(&store, &tmp).await;
        let read =
            WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
        let case = if prepared_before {
            "prepared-before"
        } else {
            "prepared-after"
        };
        let claim = claim_scope(&read, A, "a", "session-a", case).await;
        let prepared = if prepared_before {
            Some(
                read.commands()
                    .execute(
                        TENANT,
                        PROJECT,
                        A,
                        scope_prepare_command(
                            &read,
                            A,
                            "a",
                            "session-a",
                            "before-close",
                            "src/api",
                            &claim,
                        )
                        .await,
                    )
                    .await
                    .unwrap()["receipt"]["data"]
                    .clone(),
            )
        } else {
            None
        };
        let publication = publish_real_scope_change(&store, &admin).await;
        let request_id = format!("post-close-{case}");
        let request = workspace_activation(&tmp, publication, &request_id);
        let source_before = fingerprint(&std::fs::read(tmp.root.join("ledger.yaml")).unwrap());
        let activation_before = activation_facts(&admin).await;
        store
            .activate_planning_writeback_abort_for_test(
                TENANT,
                PROJECT,
                A,
                &request,
                "after_intent",
            )
            .await
            .unwrap_err();
        assert_eq!(
            fingerprint(&std::fs::read(tmp.root.join("ledger.yaml")).unwrap()),
            source_before
        );
        assert_eq!(activation_facts(&admin).await, activation_before);

        let blocked = if let Some(execution) = prepared.as_ref() {
            scope_start_command(
                &read,
                A,
                "a",
                "session-a",
                "while-pending",
                &claim,
                execution,
            )
            .await
        } else {
            scope_prepare_command(
                &read,
                A,
                "a",
                "session-a",
                "while-pending",
                "src/api",
                &claim,
            )
            .await
        };
        let error = read
            .commands()
            .execute(TENANT, PROJECT, A, blocked)
            .await
            .unwrap_err();
        assert!(
            matches!(error, PgError::ActionBlockedByInvalidation(ref key)
                if key == &format!("source_writeback:{request_id}")),
            "{case}: {error:?}"
        );

        let page =
            awr_team_pg::OperatorWriteback::inspect(&mut admin, TENANT, PROJECT, Some(&request_id))
                .await
                .unwrap();
        let item = &page["items"][0];
        assert_eq!(item["phase"], "validated");
        assert_eq!(item["source_state"], "at_before");
        assert_eq!(item["source_fingerprint"], source_before);
        assert_eq!(item["abandon"]["allowed"], true);
        assert_eq!(item["abandon"]["retain_source_required"], false);
        let close = awr_team_pg::AbandonRequest {
            request_id: request_id.clone(),
            expected_phase: item["abandon"]["expected_phase"].as_str().unwrap().into(),
            expected_intent_hash: item["abandon"]["expected_intent_hash"]
                .as_str()
                .unwrap()
                .into(),
            expected_source_state: item["abandon"]["expected_source_state"]
                .as_str()
                .unwrap()
                .into(),
            expected_source_fingerprint: item["abandon"]["expected_source_fingerprint"]
                .as_str()
                .map(str::to_owned),
            retain_source: false,
            reason: "the original client cannot resume".into(),
        };
        let closed = awr_team_pg::OperatorWriteback::abandon(&mut admin, TENANT, PROJECT, &close)
            .await
            .unwrap();
        assert_eq!(closed["journal_phase_after"], "refused");
        assert_eq!(closed["decision"]["source_state"], "at_before");
        assert_eq!(closed["source_modified"], false);
        assert_eq!(closed["active_snapshot_modified"], false);
        assert_eq!(
            fingerprint(&std::fs::read(tmp.root.join("ledger.yaml")).unwrap()),
            source_before
        );
        assert_eq!(activation_facts(&admin).await, activation_before);

        let execution = match prepared {
            Some(execution) => execution,
            None => read
                .commands()
                .execute(
                    TENANT,
                    PROJECT,
                    A,
                    scope_prepare_command(
                        &read,
                        A,
                        "a",
                        "session-a",
                        "after-close",
                        "src/api",
                        &claim,
                    )
                    .await,
                )
                .await
                .unwrap()["receipt"]["data"]
                .clone(),
        };
        let started = read
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                scope_start_command(
                    &read,
                    A,
                    "a",
                    "session-a",
                    "after-close",
                    &claim,
                    &execution,
                )
                .await,
            )
            .await
            .unwrap()["receipt"]["data"]
            .clone();
        assert_eq!(started["execution_id"], execution["execution_id"]);
        assert_eq!(started["state"], "running");
        assert_eq!(
            fingerprint(&std::fs::read(tmp.root.join("ledger.yaml")).unwrap()),
            source_before
        );
        assert_eq!(activation_facts(&admin).await, activation_before);
    }
}

#[tokio::test]
async fn concurrent_fresh_stores_recover_one_original_intent_without_duplicate_activation() {
    let (_g, admin, db, store) = store_and_roles().await;
    let (_, _, publication) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let req = workspace_activation(&tmp, publication, "concurrent-recovery");
    store
        .activate_planning_writeback_abort_for_test(TENANT, PROJECT, A, &req, "after_intent")
        .await
        .unwrap_err();
    let baseline = activation_facts(&admin).await;
    let second = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let (a, b) = tokio::join!(
        store.activate_planning_writeback(TENANT, PROJECT, A, &req),
        second.activate_planning_writeback(TENANT, PROJECT, A, &req)
    );
    assert!(
        a.is_ok() || b.is_ok(),
        "at least one original request must finish: {a:?}, {b:?}"
    );
    // A nonblocking source-lock loser queries the durable outcome before a
    // replay. Once the winner completes, the original receipt is observable.
    let outcome = second
        .get_planning_writeback_status(TENANT, PROJECT, A, &req.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome["phase"], "completed");
    let replay = second
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(replay["already_recorded"], true);
    let current = activation_facts(&admin).await;
    assert_eq!(
        (current.1, current.2, current.3),
        (baseline.1 + 1, baseline.2 + 1, baseline.3 + 1)
    );
    assert_eq!(
        std::fs::read_to_string(tmp.root.join("ledger.yaml"))
            .unwrap()
            .matches("id: SHARED-1")
            .count(),
        1
    );
}

// ---- Closing a pending writeback that cannot be resumed (schema owner only) ----

/// A pending journal as the given interruption boundary leaves it.
struct Pending {
    _guard: std::sync::MutexGuard<'static, ()>,
    admin: tokio_postgres::Client,
    db: String,
    store: SourceStore,
    tmp: TmpLedger,
    req: WritebackActivateRequest,
    receipt: String,
    /// Fingerprint of the ledger before the request started.
    before: String,
}

async fn pending_at(boundary: &str) -> Pending {
    use awr_source::fingerprint;
    let (_guard, admin, db, store) = store_and_roles().await;
    let (_, _, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let req = workspace_activation(&tmp, receipt.clone(), "stuck-original");
    let before = fingerprint(&std::fs::read(tmp.root.join("ledger.yaml")).unwrap());
    store
        .activate_planning_writeback_abort_for_test(TENANT, PROJECT, A, &req, boundary)
        .await
        .unwrap_err();
    Pending {
        _guard,
        admin,
        db,
        store,
        tmp,
        req,
        receipt,
        before,
    }
}

impl Pending {
    fn ledger(&self) -> PathBuf {
        self.tmp.root.join("ledger.yaml")
    }

    async fn inspect(&mut self) -> serde_json::Value {
        awr_team_pg::OperatorWriteback::inspect(&mut self.admin, TENANT, PROJECT, None)
            .await
            .unwrap()
    }

    /// The close request an owner builds from the inspected item, exactly as shown.
    async fn close_request(&mut self, retain_source: bool) -> awr_team_pg::AbandonRequest {
        let page = self.inspect().await;
        let item = &page["items"][0];
        assert_eq!(item["request_id"], self.req.request_id);
        awr_team_pg::AbandonRequest {
            request_id: self.req.request_id.clone(),
            expected_phase: item["abandon"]["expected_phase"].as_str().unwrap().into(),
            expected_intent_hash: item["abandon"]["expected_intent_hash"]
                .as_str()
                .unwrap()
                .into(),
            expected_source_state: item["abandon"]["expected_source_state"]
                .as_str()
                .unwrap()
                .into(),
            expected_source_fingerprint: item["abandon"]["expected_source_fingerprint"]
                .as_str()
                .map(str::to_owned),
            retain_source,
            reason: "the original client is gone".into(),
        }
    }

    async fn close(
        &mut self,
        request: &awr_team_pg::AbandonRequest,
    ) -> Result<serde_json::Value, PgError> {
        awr_team_pg::OperatorWriteback::abandon(&mut self.admin, TENANT, PROJECT, request).await
    }

    async fn pending_journals(&self) -> i64 {
        self.admin.query_one(
            "SELECT count(*) FROM awr_team.planning_writeback_journals WHERE tenant_id=$1 AND project_id=$2
               AND phase IN ('validated','source_written','pg_activating')",
            &[&TENANT, &PROJECT],
        ).await.unwrap().get(0)
    }

    async fn status(&self) -> serde_json::Value {
        self.store
            .get_planning_writeback_status(TENANT, PROJECT, A, &self.req.request_id)
            .await
            .unwrap()
            .unwrap()
    }
}

fn is_refused(error: &PgError, needle: &str) -> bool {
    matches!(error, PgError::WritebackRefused(text) if text.contains(needle))
}

async fn abandonment_facts(
    admin: &tokio_postgres::Client,
    request_id: &str,
) -> (i64, i64, String, Option<String>, serde_json::Value, String) {
    let row = admin
        .query_one(
            "SELECT p.project_revision,
              (SELECT count(*) FROM awr_team.events e
               WHERE e.tenant_id=p.tenant_id AND e.project_id=p.id
                 AND e.event_type='operator.planning_writeback_abandon'),
              j.phase,j.refuse_reason,j.body_json,j.updated_at::text
             FROM awr_team.projects p
             JOIN awr_team.planning_writeback_journals j
               ON j.tenant_id=p.tenant_id AND j.project_id=p.id
             WHERE p.tenant_id=$1 AND p.id=$2 AND j.request_id=$3",
            &[&TENANT, &PROJECT, &request_id],
        )
        .await
        .unwrap();
    (
        row.get(0),
        row.get(1),
        row.get(2),
        row.get(3),
        row.get(4),
        row.get(5),
    )
}

#[tokio::test]
async fn the_owner_sees_each_pending_writeback_with_the_source_state_and_what_closing_needs() {
    let mut p = pending_at("after_intent").await;
    let page = p.inspect().await;
    assert_eq!(page["protocol"], "awr-operator-planning-writeback-v1");
    assert_eq!(page["read_only"], true);
    assert_eq!(page["mutation"], false);
    assert_eq!(page["source_modified"], false);
    assert_eq!(page["truncated"], false);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let item = &page["items"][0];
    assert_eq!(item["request_id"], "stuck-original");
    assert_eq!(item["phase"], "validated");
    assert_eq!(item["pending"], true);
    assert_eq!(item["original_intent_bound"], true);
    assert_eq!(item["source_state"], "at_before");
    assert_eq!(item["source_fingerprint"], p.before);
    assert_eq!(item["before_fingerprint"], p.before);
    assert_eq!(item["publish_receipt_id"], p.receipt);
    assert!(item["affected_work_count"].as_i64().unwrap() > 0);
    assert_eq!(item["abandon"]["allowed"], true);
    assert_eq!(item["abandon"]["retain_source_required"], false);
    assert_eq!(item["abandon"]["expected_phase"], "validated");
    assert_eq!(item["abandon"]["expected_source_fingerprint"], p.before);
    let stored: String = p
        .admin
        .query_one(
            "SELECT intent_hash FROM awr_team.planning_writeback_journals WHERE request_id='stuck-original'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(item["abandon"]["expected_intent_hash"], stored);
    // One request can be asked for by id in any phase; reading never writes.
    let one = awr_team_pg::OperatorWriteback::inspect(
        &mut p.admin,
        TENANT,
        PROJECT,
        Some("stuck-original"),
    )
    .await
    .unwrap();
    assert_eq!(one["items"][0]["request_id"], "stuck-original");
    assert_eq!(p.pending_journals().await, 1);
    // A project member's application role is not the schema owner.
    let mut app = common::app_client(&p.db).await;
    assert!(matches!(
        awr_team_pg::OperatorWriteback::inspect(&mut app, TENANT, PROJECT, None).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn closing_an_untouched_writeback_ends_the_fences_and_leaves_the_publication_retryable() {
    use awr_source::fingerprint;
    let mut p = pending_at("after_intent").await;
    let facts_before = activation_facts(&p.admin).await;
    // Until it is closed the pending request blocks every other activation of the project.
    let retry = workspace_activation(&p.tmp, p.receipt.clone(), "after-the-stuck-one");
    let blocked = p
        .store
        .activate_planning_writeback(TENANT, PROJECT, A, &retry)
        .await
        .unwrap_err();
    assert!(
        is_refused(&blocked, "must be queried and recovered first"),
        "{blocked:?}"
    );
    let close = p.close_request(false).await;
    let revision_before: i64 = p
        .admin
        .query_one(
            "SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let closed = p.close(&close).await.unwrap();
    assert_eq!(closed["replayed"], false);
    assert_eq!(closed["journal_phase_after"], "refused");
    assert_eq!(closed["source_modified"], false);
    assert_eq!(closed["active_snapshot_modified"], false);
    assert_eq!(closed["execution_authorized"], false);
    assert_eq!(closed["decision"]["previous_phase"], "validated");
    assert_eq!(closed["decision"]["source_state"], "at_before");
    assert_eq!(closed["decision"]["source_retained"], false);
    assert_eq!(closed["decision"]["reason"], "the original client is gone");
    // Nothing but the journal moved: bytes, active snapshot, authority epoch, receipts.
    assert_eq!(fingerprint(&std::fs::read(p.ledger()).unwrap()), p.before);
    assert_eq!(activation_facts(&p.admin).await, facts_before);
    assert_eq!(p.pending_journals().await, 0);
    let revision_after: i64 = p
        .admin
        .query_one(
            "SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(revision_after, revision_before + 1);
    let events: i64 = p
        .admin
        .query_one(
            "SELECT count(*) FROM awr_team.events WHERE tenant_id=$1 AND project_id=$2
               AND event_type='operator.planning_writeback_abandon' AND project_revision=$3",
            &[&TENANT, &PROJECT, &revision_after],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(events, 1);
    // The planning outcome says what happened without the owner's free text.
    let status = p.status().await;
    assert_eq!(status["phase"], "refused");
    assert_eq!(status["pending"], false);
    assert_eq!(status["applied"], false);
    assert_eq!(status["operator_closed"], true);
    assert!(!status.to_string().contains("the original client is gone"));
    // The original request can no longer resume, and says why.
    let resumed = p
        .store
        .activate_planning_writeback(TENANT, PROJECT, A, &p.req)
        .await
        .unwrap_err();
    assert!(is_refused(&resumed, "schema owner closed"), "{resumed:?}");
    // Asking again returns the recorded decision and changes nothing.
    let replay = p.close(&close).await.unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["decision"], closed["decision"]);
    let revision_replayed: i64 = p
        .admin
        .query_one(
            "SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(revision_replayed, revision_after);
    let events_replayed: i64 = p
        .admin
        .query_one(
            "SELECT count(*) FROM awr_team.events WHERE tenant_id=$1 AND project_id=$2
               AND event_type='operator.planning_writeback_abandon' AND project_revision=$3",
            &[&TENANT, &PROJECT, &revision_after],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        events_replayed, 1,
        "replay must not add another audit event"
    );
    // The source was never written, so the same approved publication goes through under a new request.
    let done = p
        .store
        .activate_planning_writeback(TENANT, PROJECT, A, &retry)
        .await
        .unwrap();
    assert_eq!(done["already_recorded"], false);
    let facts_after = activation_facts(&p.admin).await;
    assert_eq!(
        (facts_after.1, facts_after.2, facts_after.3),
        (facts_before.1 + 1, facts_before.2 + 1, facts_before.3 + 1)
    );
}

#[tokio::test]
async fn closing_needs_the_inspected_facts_and_refuses_everything_else() {
    let mut p = pending_at("after_intent").await;
    let good = p.close_request(false).await;
    // These changes happen after the owner inspected. The command must observe
    // current source and journal facts instead of trusting the stale token.
    let original_source = std::fs::read(p.ledger()).unwrap();
    std::fs::write(p.ledger(), b"# changed after operator inspection\n").unwrap();
    let mut stale_retained = good.clone();
    stale_retained.retain_source = true;
    assert!(matches!(
        p.close(&stale_retained).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert!(matches!(
        p.close(&good).await,
        Err(PgError::PreconditionsChanged)
    ));
    std::fs::write(p.ledger(), &original_source).unwrap();
    p.admin
        .execute(
            "UPDATE awr_team.planning_writeback_journals SET phase='source_written'
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &p.req.request_id],
        )
        .await
        .unwrap();
    assert!(matches!(
        p.close(&good).await,
        Err(PgError::PreconditionsChanged)
    ));
    p.admin
        .execute(
            "UPDATE awr_team.planning_writeback_journals SET phase='validated'
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &p.req.request_id],
        )
        .await
        .unwrap();
    let changed_hash = if good.expected_intent_hash == "0".repeat(64) {
        "1".repeat(64)
    } else {
        "0".repeat(64)
    };
    p.admin
        .execute(
            "UPDATE awr_team.planning_writeback_journals SET intent_hash=$1
             WHERE tenant_id=$2 AND project_id=$3 AND request_id=$4",
            &[&changed_hash, &TENANT, &PROJECT, &p.req.request_id],
        )
        .await
        .unwrap();
    assert!(matches!(
        p.close(&good).await,
        Err(PgError::PreconditionsChanged)
    ));
    p.admin
        .execute(
            "UPDATE awr_team.planning_writeback_journals SET intent_hash=$1
             WHERE tenant_id=$2 AND project_id=$3 AND request_id=$4",
            &[
                &good.expected_intent_hash,
                &TENANT,
                &PROJECT,
                &p.req.request_id,
            ],
        )
        .await
        .unwrap();
    let mut wrong_phase = good.clone();
    wrong_phase.expected_phase = "source_written".into();
    let mut wrong_hash = good.clone();
    wrong_hash.expected_intent_hash = "0".repeat(64);
    let mut unbound = good.clone();
    unbound.expected_intent_hash = "unbound".into();
    let mut wrong_state = good.clone();
    wrong_state.expected_source_state = "at_after".into();
    let mut wrong_fingerprint = good.clone();
    wrong_fingerprint.expected_source_fingerprint = Some(format!("sha256:{}", "0".repeat(64)));
    for (name, request) in [
        ("phase", &wrong_phase),
        ("intent hash", &wrong_hash),
        ("claimed unbound", &unbound),
        ("source state", &wrong_state),
        ("source fingerprint", &wrong_fingerprint),
    ] {
        assert!(
            matches!(p.close(request).await, Err(PgError::PreconditionsChanged)),
            "a different {name} must not close the journal"
        );
    }
    let mut malformed_hash = good.clone();
    malformed_hash.expected_intent_hash = "not-a-hash".into();
    let mut no_reason = good.clone();
    no_reason.reason = "  ".into();
    let mut unknown_phase = good.clone();
    unknown_phase.expected_phase = "completed".into();
    let mut missing_fingerprint = good.clone();
    missing_fingerprint.expected_source_fingerprint = None;
    for request in [
        &malformed_hash,
        &no_reason,
        &unknown_phase,
        &missing_fingerprint,
    ] {
        assert!(matches!(p.close(request).await, Err(PgError::Protocol(_))));
    }
    let mut missing = good.clone();
    missing.request_id = "never-existed".into();
    assert!(is_refused(
        &p.close(&missing).await.unwrap_err(),
        "no writeback journal"
    ));
    let mut app = common::app_client(&p.db).await;
    assert!(matches!(
        awr_team_pg::OperatorWriteback::abandon(&mut app, TENANT, PROJECT, &good).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(
        p.pending_journals().await,
        1,
        "no refused attempt closed it"
    );
    assert_eq!(p.status().await["phase"], "validated");
    p.close(&good).await.unwrap();
    assert_eq!(p.pending_journals().await, 0);
}

#[tokio::test]
async fn closing_rechecks_the_exact_drifted_fingerprint_inspected_by_the_owner() {
    use awr_source::fingerprint;
    let mut p = pending_at("after_intent").await;
    std::fs::write(p.ledger(), b"# first external edit\n").unwrap();
    let inspected = p.inspect().await;
    assert_eq!(inspected["items"][0]["source_state"], "drifted");
    let inspected_fingerprint = fingerprint(&std::fs::read(p.ledger()).unwrap());
    assert_eq!(
        inspected["items"][0]["source_fingerprint"],
        inspected_fingerprint
    );
    let close = p.close_request(true).await;
    let journal_before = abandonment_facts(&p.admin, &p.req.request_id).await;
    let activation_before = activation_facts(&p.admin).await;

    std::fs::write(p.ledger(), b"# different external edit\n").unwrap();
    let retained_bytes = std::fs::read(p.ledger()).unwrap();
    assert_ne!(fingerprint(&retained_bytes), inspected_fingerprint);
    let error = p.close(&close).await.unwrap_err();
    assert!(matches!(error, PgError::PreconditionsChanged), "{error:?}");

    assert_eq!(std::fs::read(p.ledger()).unwrap(), retained_bytes);
    assert_eq!(
        abandonment_facts(&p.admin, &p.req.request_id).await,
        journal_before
    );
    assert_eq!(activation_facts(&p.admin).await, activation_before);
    assert_eq!(p.pending_journals().await, 1);
}

#[tokio::test]
async fn an_unreadable_source_uses_an_explicit_null_fingerprint_token() {
    let mut p = pending_at("after_intent").await;
    std::fs::remove_file(p.ledger()).unwrap();
    let page = p.inspect().await;
    let item = &page["items"][0];
    assert_eq!(item["source_state"], "unreadable");
    assert_eq!(item["source_fingerprint"], serde_json::Value::Null);
    assert!(
        item["abandon"]
            .as_object()
            .unwrap()
            .contains_key("expected_source_fingerprint")
    );
    assert_eq!(
        item["abandon"]["expected_source_fingerprint"],
        serde_json::Value::Null
    );
    let close = p.close_request(true).await;
    assert_eq!(close.expected_source_fingerprint, None);
    let activation_before = activation_facts(&p.admin).await;
    let closed = p.close(&close).await.unwrap();
    assert_eq!(closed["decision"]["source_state"], "unreadable");
    assert_eq!(
        closed["decision"]["source_fingerprint"],
        serde_json::Value::Null
    );
    assert!(!p.ledger().exists());
    assert_eq!(activation_facts(&p.admin).await, activation_before);
    assert_eq!(p.pending_journals().await, 0);
}

#[tokio::test]
async fn a_pending_journal_with_an_operator_decision_is_not_a_successful_replay() {
    let mut p = pending_at("after_intent").await;
    let close = p.close_request(false).await;
    let revision = abandonment_facts(&p.admin, &p.req.request_id).await.0;
    let decision = serde_json::json!({
        "protocol": "awr-operator-planning-writeback-v1",
        "action": "abandon",
        "request_id": p.req.request_id,
        "previous_phase": "validated",
        "intent_hash": close.expected_intent_hash,
        "source_state": "at_before",
        "source_fingerprint": p.before,
        "source_retained": false,
        "operator_role": "postgres",
        "reason": "injected inconsistent decision",
        "project_revision": revision.to_string(),
    });
    p.admin
        .execute(
            "UPDATE awr_team.planning_writeback_journals
             SET body_json=jsonb_build_object('phase',phase,'operator_decision',$1::jsonb)
             WHERE tenant_id=$2 AND project_id=$3 AND request_id=$4",
            &[&decision, &TENANT, &PROJECT, &p.req.request_id],
        )
        .await
        .unwrap();
    let journal_before = abandonment_facts(&p.admin, &p.req.request_id).await;
    let activation_before = activation_facts(&p.admin).await;
    let source_before = std::fs::read(p.ledger()).unwrap();

    let error = p.close(&close).await.unwrap_err();
    assert!(
        is_refused(&error, "inconsistent with its journal phase"),
        "{error:?}"
    );
    assert_eq!(std::fs::read(p.ledger()).unwrap(), source_before);
    assert_eq!(
        abandonment_facts(&p.admin, &p.req.request_id).await,
        journal_before
    );
    assert_eq!(activation_facts(&p.admin).await, activation_before);
    assert_eq!(p.pending_journals().await, 1);
}

#[tokio::test]
async fn a_refused_journal_replays_only_a_well_formed_operator_decision() {
    let mut p = pending_at("after_intent").await;
    let close = p.close_request(false).await;
    p.close(&close).await.unwrap();
    p.admin
        .execute(
            "UPDATE awr_team.planning_writeback_journals
             SET body_json=jsonb_build_object('phase','refused','operator_decision','{}'::jsonb)
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &p.req.request_id],
        )
        .await
        .unwrap();
    let journal_before = abandonment_facts(&p.admin, &p.req.request_id).await;
    let activation_before = activation_facts(&p.admin).await;
    let source_before = std::fs::read(p.ledger()).unwrap();

    let error = p.close(&close).await.unwrap_err();
    assert!(is_refused(&error, "decision is malformed"), "{error:?}");
    assert_eq!(std::fs::read(p.ledger()).unwrap(), source_before);
    assert_eq!(
        abandonment_facts(&p.admin, &p.req.request_id).await,
        journal_before
    );
    assert_eq!(activation_facts(&p.admin).await, activation_before);
}

#[tokio::test]
async fn a_malformed_refused_decision_never_reports_owner_closure_or_changes_on_retry() {
    for corrupt_intent in [false, true] {
        let mut p = pending_at("after_intent").await;
        let close = p.close_request(false).await;
        p.close(&close).await.unwrap();
        p.admin
            .execute(
                "UPDATE awr_team.planning_writeback_journals
                 SET body_json=jsonb_build_object('phase','refused','operator_decision','{}'::jsonb)
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&TENANT, &PROJECT, &p.req.request_id],
            )
            .await
            .unwrap();
        if corrupt_intent {
            p.admin
                .execute(
                    "UPDATE awr_team.planning_writeback_journals
                     SET intent_json=jsonb_build_object('codec','corrupt-intent')
                     WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                    &[&TENANT, &PROJECT, &p.req.request_id],
                )
                .await
                .unwrap();
        }
        let journal_before = abandonment_facts(&p.admin, &p.req.request_id).await;
        let activation_before = activation_facts(&p.admin).await;
        let source_before = std::fs::read(p.ledger()).unwrap();

        let status = p.status().await;
        assert_eq!(status["phase"], "refused");
        assert_eq!(status["original_intent_bound"], !corrupt_intent);
        assert!(status.get("operator_closed").is_none());
        assert!(
            !status["next_step"]
                .as_str()
                .unwrap()
                .contains("schema owner closed")
        );

        let retry = p
            .store
            .activate_planning_writeback(TENANT, PROJECT, A, &p.req)
            .await
            .unwrap_err();
        assert!(!is_refused(&retry, "schema owner closed"), "{retry:?}");
        if corrupt_intent {
            assert!(
                is_refused(&retry, "original writeback intent is unknown"),
                "{retry:?}"
            );
        } else {
            assert!(
                matches!(retry, PgError::ActivationImpactUnproven(_)),
                "{retry:?}"
            );
        }
        let replay = p.close(&close).await.unwrap_err();
        assert!(is_refused(&replay, "decision is malformed"), "{replay:?}");

        assert_eq!(std::fs::read(p.ledger()).unwrap(), source_before);
        assert_eq!(
            abandonment_facts(&p.admin, &p.req.request_id).await,
            journal_before
        );
        assert_eq!(activation_facts(&p.admin).await, activation_before);
    }
}

#[tokio::test]
async fn a_finished_writeback_is_not_pending_and_cannot_be_closed() {
    let mut done = pending_at("after_final_commit").await;
    let page = awr_team_pg::OperatorWriteback::inspect(
        &mut done.admin,
        TENANT,
        PROJECT,
        Some("stuck-original"),
    )
    .await
    .unwrap();
    assert_eq!(page["items"][0]["phase"], "completed");
    assert!(page["items"][0].get("abandon").is_none());
    let listed = done.inspect().await;
    assert!(
        listed["items"].as_array().unwrap().is_empty(),
        "only pending journals are listed by default"
    );
    let finished = awr_team_pg::AbandonRequest {
        request_id: "stuck-original".into(),
        expected_phase: "pg_activating".into(),
        expected_intent_hash: page["items"][0]["intent_hash"].as_str().unwrap().into(),
        expected_source_state: "at_after".into(),
        expected_source_fingerprint: Some(format!("sha256:{}", "0".repeat(64))),
        retain_source: true,
        reason: "too late".into(),
    };
    assert!(is_refused(
        &done.close(&finished).await.unwrap_err(),
        "no longer pending"
    ));
    assert_eq!(done.status().await["applied"], true);
}

#[tokio::test]
async fn a_writeback_refused_by_the_original_client_cannot_be_operator_closed() {
    let (_guard, mut admin, _db, store) = store_and_roles().await;
    let (_, _, receipt) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let mut request = workspace_activation(&tmp, receipt, "client-refused");
    request.impact_proven = false;
    assert!(matches!(
        store
            .activate_planning_writeback(TENANT, PROJECT, A, &request)
            .await,
        Err(PgError::ActivationImpactUnproven(_))
    ));
    let page = awr_team_pg::OperatorWriteback::inspect(
        &mut admin,
        TENANT,
        PROJECT,
        Some("client-refused"),
    )
    .await
    .unwrap();
    let item = &page["items"][0];
    assert_eq!(item["phase"], "refused");
    assert_eq!(item["original_intent_bound"], true);
    assert!(item.get("abandon").is_none());
    let close = awr_team_pg::AbandonRequest {
        request_id: "client-refused".into(),
        expected_phase: "validated".into(),
        expected_intent_hash: item["intent_hash"].as_str().unwrap().into(),
        expected_source_state: "unknown".into(),
        expected_source_fingerprint: None,
        retain_source: true,
        reason: "must stay a client refusal".into(),
    };
    assert!(is_refused(
        &awr_team_pg::OperatorWriteback::abandon(&mut admin, TENANT, PROJECT, &close)
            .await
            .unwrap_err(),
        "no longer pending"
    ));
    let status = store
        .get_planning_writeback_status(TENANT, PROJECT, A, "client-refused")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status["phase"], "refused");
    assert!(status.get("operator_closed").is_none());
}

#[tokio::test]
async fn a_source_the_server_may_already_have_written_is_only_kept_never_touched() {
    use awr_source::fingerprint;
    for (boundary, phase) in [
        ("after_source_write", "validated"),
        ("after_source_written", "source_written"),
        ("after_pg_activating", "pg_activating"),
    ] {
        let mut p = pending_at(boundary).await;
        let written = fingerprint(&std::fs::read(p.ledger()).unwrap());
        assert_ne!(
            written, p.before,
            "{boundary}: the request wrote the source"
        );
        let facts = activation_facts(&p.admin).await;
        let page = p.inspect().await;
        let item = &page["items"][0];
        assert_eq!(item["phase"], phase, "{boundary}");
        assert_eq!(item["source_state"], "at_after", "{boundary}");
        assert_eq!(
            item["abandon"]["retain_source_required"], true,
            "{boundary}"
        );
        let without = p.close_request(false).await;
        let refused = p.close(&without).await.unwrap_err();
        assert!(
            is_refused(&refused, "retain_source"),
            "{boundary}: {refused:?}"
        );
        assert_eq!(p.pending_journals().await, 1, "{boundary}");
        let with = p.close_request(true).await;
        let closed = p.close(&with).await.unwrap();
        assert_eq!(closed["decision"]["source_retained"], true, "{boundary}");
        assert_eq!(closed["decision"]["source_state"], "at_after", "{boundary}");
        assert_eq!(closed["source_modified"], false, "{boundary}");
        // The bytes the request wrote stay exactly as they are; the active snapshot did not move.
        assert_eq!(
            fingerprint(&std::fs::read(p.ledger()).unwrap()),
            written,
            "{boundary}"
        );
        assert_eq!(activation_facts(&p.admin).await, facts, "{boundary}");
        assert_eq!(p.pending_journals().await, 0, "{boundary}");
        assert_eq!(p.status().await["operator_closed"], true, "{boundary}");
    }
    // Bytes that match neither fingerprint were changed from outside: kept the same way.
    let mut p = pending_at("after_intent").await;
    std::fs::write(p.ledger(), b"# edited by someone else\n").unwrap();
    let page = p.inspect().await;
    assert_eq!(page["items"][0]["source_state"], "drifted");
    let without = p.close_request(false).await;
    assert!(is_refused(
        &p.close(&without).await.unwrap_err(),
        "retain_source"
    ));
    let with = p.close_request(true).await;
    p.close(&with).await.unwrap();
    assert_eq!(
        std::fs::read(p.ledger()).unwrap(),
        b"# edited by someone else\n"
    );
}

#[tokio::test]
async fn a_cooperating_writer_holding_the_source_blocks_closing() {
    let mut p = pending_at("after_intent").await;
    let writer = awr_source::LockedSourceFile::open(&p.tmp.root, "ledger.yaml").unwrap();
    let page = p.inspect().await;
    let item = &page["items"][0];
    assert_eq!(item["source_state"], "writer_busy");
    assert_eq!(item["abandon"]["allowed"], false);
    let busy_request = awr_team_pg::AbandonRequest {
        request_id: p.req.request_id.clone(),
        expected_phase: item["abandon"]["expected_phase"].as_str().unwrap().into(),
        expected_intent_hash: item["abandon"]["expected_intent_hash"]
            .as_str()
            .unwrap()
            .into(),
        expected_source_state: item["abandon"]["expected_source_state"]
            .as_str()
            .unwrap()
            .into(),
        expected_source_fingerprint: item["abandon"]["expected_source_fingerprint"]
            .as_str()
            .map(str::to_owned),
        retain_source: true,
        reason: "the original client is gone".into(),
    };
    let busy = p.close(&busy_request).await.unwrap_err();
    assert!(is_refused(&busy, "holds the source lock"), "{busy:?}");
    assert_eq!(p.pending_journals().await, 1);
    drop(writer);
    assert!(matches!(
        p.close(&busy_request).await,
        Err(PgError::PreconditionsChanged)
    ));
    let good = p.close_request(false).await;
    p.close(&good).await.unwrap();
    assert_eq!(p.pending_journals().await, 0);
}

#[tokio::test]
async fn a_journal_without_a_recorded_intent_is_closed_only_by_naming_it_unbound() {
    use awr_source::fingerprint;
    let mut p = pending_at("after_source_written").await;
    let written = fingerprint(&std::fs::read(p.ledger()).unwrap());
    // How a journal written before the intent was recorded looks.
    p.admin
        .batch_execute(
            "UPDATE awr_team.planning_writeback_journals SET intent_hash=NULL,intent_json=NULL",
        )
        .await
        .unwrap();
    let page = p.inspect().await;
    let item = &page["items"][0];
    assert_eq!(item["original_intent_bound"], false);
    assert_eq!(item["source_state"], "unknown");
    assert_eq!(
        item["abandon"]["expected_source_fingerprint"],
        serde_json::Value::Null
    );
    assert_eq!(item["abandon"]["expected_intent_hash"], "unbound");
    assert_eq!(item["abandon"]["retain_source_required"], true);
    assert_eq!(p.status().await["original_intent_bound"], false);
    let mut real_hash = p.close_request(true).await;
    real_hash.expected_intent_hash = "a".repeat(64);
    assert!(matches!(
        p.close(&real_hash).await,
        Err(PgError::PreconditionsChanged)
    ));
    let without = p.close_request(false).await;
    assert!(is_refused(
        &p.close(&without).await.unwrap_err(),
        "retain_source"
    ));
    let with = p.close_request(true).await;
    assert_eq!(with.expected_source_fingerprint, None);
    let mut invented_fingerprint = with.clone();
    invented_fingerprint.expected_source_fingerprint = Some(format!("sha256:{}", "0".repeat(64)));
    assert!(matches!(
        p.close(&invented_fingerprint).await,
        Err(PgError::Protocol(_))
    ));
    let closed = p.close(&with).await.unwrap();
    assert_eq!(closed["decision"]["source_state"], "unknown");
    assert_eq!(closed["decision"]["intent_hash"], serde_json::Value::Null);
    assert_eq!(fingerprint(&std::fs::read(p.ledger()).unwrap()), written);
    assert_eq!(p.pending_journals().await, 0);
    let facts_before_retry = activation_facts(&p.admin).await;
    let retry = p
        .store
        .activate_planning_writeback(TENANT, PROJECT, A, &p.req)
        .await
        .unwrap_err();
    assert!(is_refused(&retry, "schema owner closed"), "{retry:?}");
    assert_eq!(fingerprint(&std::fs::read(p.ledger()).unwrap()), written);
    assert_eq!(activation_facts(&p.admin).await, facts_before_retry);
    let status = p.status().await;
    assert_eq!(status["original_intent_bound"], false);
    assert_eq!(status["operator_closed"], true);
    // Source activation is open again for the rest of the project.
    let other = workspace_activation(&p.tmp, p.receipt.clone(), "after-the-legacy-one");
    let attempt = p
        .store
        .activate_planning_writeback(TENANT, PROJECT, A, &other)
        .await;
    assert!(
        !matches!(&attempt, Err(error) if is_refused(error, "must be queried and recovered first")),
        "the closed journal must not fence the project: {attempt:?}"
    );
}

#[tokio::test]
async fn an_inconsistent_intent_with_a_valid_recorded_hash_uses_that_hash_only_as_the_close_token()
{
    use awr_source::fingerprint;
    let mut p = pending_at("after_source_written").await;
    let written = fingerprint(&std::fs::read(p.ledger()).unwrap());
    let recorded_hash: String = p
        .admin
        .query_one(
            "SELECT intent_hash FROM awr_team.planning_writeback_journals
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &p.req.request_id],
        )
        .await
        .unwrap()
        .get(0);
    p.admin
        .execute(
            "UPDATE awr_team.planning_writeback_journals
             SET intent_json=jsonb_build_object('codec','corrupt-intent')
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &p.req.request_id],
        )
        .await
        .unwrap();
    let page = p.inspect().await;
    let item = &page["items"][0];
    assert_eq!(item["original_intent_bound"], false);
    assert_eq!(item["intent_hash"], recorded_hash);
    assert_eq!(item["source_state"], "unknown");
    assert_eq!(item["source_fingerprint"], serde_json::Value::Null);
    assert_eq!(item["abandon"]["allowed"], true);
    assert_eq!(item["abandon"]["expected_intent_hash"], recorded_hash);
    assert_eq!(item["abandon"]["retain_source_required"], true);
    let without = p.close_request(false).await;
    assert!(is_refused(
        &p.close(&without).await.unwrap_err(),
        "retain_source"
    ));
    let with = p.close_request(true).await;
    let closed = p.close(&with).await.unwrap();
    assert_eq!(closed["decision"]["intent_hash"], recorded_hash);
    assert_eq!(closed["decision"]["source_state"], "unknown");
    assert_eq!(closed["decision"]["source_retained"], true);
    assert_eq!(fingerprint(&std::fs::read(p.ledger()).unwrap()), written);
    assert_eq!(p.pending_journals().await, 0);
    let facts_before_retry = activation_facts(&p.admin).await;
    let retry = p
        .store
        .activate_planning_writeback(TENANT, PROJECT, A, &p.req)
        .await
        .unwrap_err();
    assert!(is_refused(&retry, "schema owner closed"), "{retry:?}");
    assert_eq!(fingerprint(&std::fs::read(p.ledger()).unwrap()), written);
    assert_eq!(activation_facts(&p.admin).await, facts_before_retry);
    let status = p.status().await;
    assert_eq!(status["original_intent_bound"], false);
    assert_eq!(status["operator_closed"], true);
}

#[tokio::test]
async fn a_malformed_recorded_hash_is_visible_but_not_advertised_as_a_close_token() {
    let mut p = pending_at("after_intent").await;
    p.admin
        .execute(
            "UPDATE awr_team.planning_writeback_journals SET intent_hash='not-a-hash'
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &p.req.request_id],
        )
        .await
        .unwrap();
    let page = p.inspect().await;
    let item = &page["items"][0];
    assert_eq!(item["original_intent_bound"], false);
    assert_eq!(item["intent_hash"], serde_json::Value::Null);
    assert_eq!(item["source_state"], "unknown");
    assert_eq!(item["abandon"]["allowed"], false);
    assert_eq!(
        item["abandon"]["blocked_by"],
        "the recorded intent hash is malformed and cannot be used as a safe close token"
    );
    assert_eq!(
        item["abandon"]["expected_intent_hash"],
        serde_json::Value::Null
    );
    let close = awr_team_pg::AbandonRequest {
        request_id: p.req.request_id.clone(),
        expected_phase: "validated".into(),
        expected_intent_hash: "unbound".into(),
        expected_source_state: "unknown".into(),
        expected_source_fingerprint: None,
        retain_source: true,
        reason: "the original client is gone".into(),
    };
    let error = p.close(&close).await.unwrap_err();
    assert!(is_refused(&error, "recorded intent hash is malformed"));
    assert_eq!(p.pending_journals().await, 1);
}
