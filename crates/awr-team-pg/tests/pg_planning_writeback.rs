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
async fn affected_live_claim_requires_explicit_stop() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate(&store).await;
    // Plant an active claim on CLIENT-1 (affected).
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,last_fence)
             VALUES ('reader-tenant','reader-project','main','CLIENT-1','claimed',0)
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.sessions(
                tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
             VALUES ('reader-tenant','reader-project','sess-wb','main','CLIENT-1','agent','cli-a','c','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.claims(
                tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,state,expires_at)
             VALUES (
                'reader-tenant','reader-project','claim-wb','main','CLIENT-1','sess-wb','agent',1,
                'active', clock_timestamp() + interval '1 hour')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let tmp = tempfile_ledger();
    let err = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &WritebackActivateRequest {
                request_id: "req-live-claim-1".into(),
                publish_receipt_id: receipt_id,
                source_root: tmp.root.clone(),
                ledger_relative_path: "ledger.yaml".into(),
                impact_proven: true,
                stopped_work_ids: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::WritebackRefused(_)), "{err:?}");
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

#[tokio::test]
async fn source_written_phase_resumes_without_reapplying_creates() {
    use awr_source::{apply_planning_changes_to_ledger, fingerprint};
    let (_g, admin, _db, store) = store_and_roles().await;
    let (candidate_id, digest, receipt_id) =
        publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let before_bytes = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let mut create_task = draft("SHARED-1", &[], DraftDefinitionState::Draft);
    create_task.workstream = Some("alpha".into());
    let changes = vec![
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
    ];
    let patch = apply_planning_changes_to_ledger(&before_bytes, &changes).unwrap();
    // Simulate crash after authoritative source write: ledger already contains
    // CreateTask, journal durable at source_written, PG snapshot not activated.
    std::fs::write(tmp.root.join("ledger.yaml"), &patch.after_bytes).unwrap();
    assert_eq!(fingerprint(&patch.after_bytes), patch.after_fingerprint);

    let body = serde_json::json!({"phase": "source_written"});
    admin
        .execute(
            "INSERT INTO awr_team.planning_writeback_journals(
                tenant_id, project_id, request_id, candidate_id, candidate_digest,
                publish_receipt_id, phase, before_fingerprint, after_fingerprint,
                publisher_actor_id, affected_work_ids, unrelated_work_ids,
                recovery_actions, body_json)
             VALUES (
                'reader-tenant','reader-project','req-resume-1',$1,$2,$3,
                'source_written',$4,$5,'agent','[]'::jsonb,'[]'::jsonb,
                '[]'::jsonb,$6)",
            &[
                &candidate_id,
                &digest,
                &receipt_id,
                &patch.before_fingerprint,
                &patch.after_fingerprint,
                &body,
            ],
        )
        .await
        .unwrap();

    let req = WritebackActivateRequest {
        request_id: "req-resume-1".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    };
    let resumed = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .expect("resume from source_written must complete without re-applying creates");
    assert_eq!(resumed["already_recorded"], false);
    assert_eq!(resumed["after_fingerprint"], patch.after_fingerprint);
    // Ledger still has exactly one SHARED-1 identity (no duplicate create on resume).
    let text = std::fs::read_to_string(tmp.root.join("ledger.yaml")).unwrap();
    assert_eq!(
        text.matches("id: SHARED-1").count(),
        1,
        "CreateTask must not be re-applied on resume: {text}"
    );
}

#[tokio::test]
async fn validated_phase_with_written_ledger_does_not_reapply_creates() {
    use awr_source::{apply_planning_changes_to_ledger, fingerprint};
    let (_g, admin, _db, store) = store_and_roles().await;
    let (candidate_id, digest, receipt_id) =
        publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let before_bytes = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let mut create_task = draft("SHARED-1", &[], DraftDefinitionState::Draft);
    create_task.workstream = Some("alpha".into());
    let changes = vec![
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
    ];
    let patch = apply_planning_changes_to_ledger(&before_bytes, &changes).unwrap();
    // Crash window: journal committed as validated, then the ledger write landed,
    // but phase never advanced to source_written.
    std::fs::write(tmp.root.join("ledger.yaml"), &patch.after_bytes).unwrap();
    assert_eq!(fingerprint(&patch.after_bytes), patch.after_fingerprint);

    let body = serde_json::json!({"phase": "validated"});
    admin
        .execute(
            "INSERT INTO awr_team.planning_writeback_journals(
                tenant_id, project_id, request_id, candidate_id, candidate_digest,
                publish_receipt_id, phase, before_fingerprint, after_fingerprint,
                publisher_actor_id, affected_work_ids, unrelated_work_ids,
                recovery_actions, body_json)
             VALUES (
                'reader-tenant','reader-project','req-resume-validated',$1,$2,$3,
                'validated',$4,$5,'agent','[]'::jsonb,'[]'::jsonb,
                '[]'::jsonb,$6)",
            &[
                &candidate_id,
                &digest,
                &receipt_id,
                &patch.before_fingerprint,
                &patch.after_fingerprint,
                &body,
            ],
        )
        .await
        .unwrap();

    let req = WritebackActivateRequest {
        request_id: "req-resume-validated".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: true,
        stopped_work_ids: vec![],
    };
    let resumed = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .expect("resume from validated+written ledger must not re-apply creates");
    assert_eq!(resumed["already_recorded"], false);
    assert_eq!(resumed["after_fingerprint"], patch.after_fingerprint);
    let text = std::fs::read_to_string(tmp.root.join("ledger.yaml")).unwrap();
    assert_eq!(
        text.matches("id: SHARED-1").count(),
        1,
        "CreateTask must not be re-applied after validated write: {text}"
    );
}

#[tokio::test]
async fn validated_phase_with_unwritten_ledger_applies_the_patch_once() {
    use awr_source::{apply_planning_changes_to_ledger, fingerprint};
    let (_g, admin, _db, store) = store_and_roles().await;
    let (candidate_id, digest, receipt_id) =
        publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let before_bytes = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let mut create_task = draft("SHARED-1", &[], DraftDefinitionState::Draft);
    create_task.workstream = Some("alpha".into());
    let changes = vec![
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
    ];
    let patch = apply_planning_changes_to_ledger(&before_bytes, &changes).unwrap();
    assert_eq!(fingerprint(&before_bytes), patch.before_fingerprint);
    assert_ne!(fingerprint(&before_bytes), patch.after_fingerprint);

    let body = serde_json::json!({"phase": "validated"});
    admin
        .execute(
            "INSERT INTO awr_team.planning_writeback_journals(
                tenant_id, project_id, request_id, candidate_id, candidate_digest,
                publish_receipt_id, phase, before_fingerprint, after_fingerprint,
                publisher_actor_id, affected_work_ids, unrelated_work_ids,
                recovery_actions, body_json)
             VALUES (
                'reader-tenant','reader-project','req-resume-unwritten',$1,$2,$3,
                'validated',$4,$5,'agent','[]'::jsonb,'[]'::jsonb,
                '[]'::jsonb,$6)",
            &[
                &candidate_id,
                &digest,
                &receipt_id,
                &patch.before_fingerprint,
                &patch.after_fingerprint,
                &body,
            ],
        )
        .await
        .unwrap();

    let resumed = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &WritebackActivateRequest {
                request_id: "req-resume-unwritten".into(),
                publish_receipt_id: receipt_id,
                source_root: tmp.root.clone(),
                ledger_relative_path: "ledger.yaml".into(),
                impact_proven: true,
                stopped_work_ids: vec![],
            },
        )
        .await
        .expect("resume from validated+unwritten ledger must write the patch once");
    assert_eq!(resumed["after_fingerprint"], patch.after_fingerprint);
    let text = std::fs::read_to_string(tmp.root.join("ledger.yaml")).unwrap();
    assert_eq!(
        text.matches("id: SHARED-1").count(),
        1,
        "CreateTask must be applied once when the validated write never landed: {text}"
    );
    assert_eq!(fingerprint(text.as_bytes()), patch.after_fingerprint);
}

#[tokio::test]
async fn validated_phase_refuses_ledger_bytes_matching_neither_fingerprint() {
    use awr_source::{apply_planning_changes_to_ledger, fingerprint};
    let (_g, admin, _db, store) = store_and_roles().await;
    let (candidate_id, digest, receipt_id) =
        publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let before_bytes = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let mut create_task = draft("SHARED-1", &[], DraftDefinitionState::Draft);
    create_task.workstream = Some("alpha".into());
    let changes = vec![
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
    ];
    let patch = apply_planning_changes_to_ledger(&before_bytes, &changes).unwrap();
    let foreign = b"workstreams:\n  version: 9\n  definitions: []\nitems: []\n";
    std::fs::write(tmp.root.join("ledger.yaml"), foreign).unwrap();
    assert_ne!(fingerprint(foreign), patch.before_fingerprint);
    assert_ne!(fingerprint(foreign), patch.after_fingerprint);

    let body = serde_json::json!({"phase": "validated"});
    admin
        .execute(
            "INSERT INTO awr_team.planning_writeback_journals(
                tenant_id, project_id, request_id, candidate_id, candidate_digest,
                publish_receipt_id, phase, before_fingerprint, after_fingerprint,
                publisher_actor_id, affected_work_ids, unrelated_work_ids,
                recovery_actions, body_json)
             VALUES (
                'reader-tenant','reader-project','req-resume-foreign',$1,$2,$3,
                'validated',$4,$5,'agent','[]'::jsonb,'[]'::jsonb,
                '[]'::jsonb,$6)",
            &[
                &candidate_id,
                &digest,
                &receipt_id,
                &patch.before_fingerprint,
                &patch.after_fingerprint,
                &body,
            ],
        )
        .await
        .unwrap();

    let err = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &WritebackActivateRequest {
                request_id: "req-resume-foreign".into(),
                publish_receipt_id: receipt_id,
                source_root: tmp.root.clone(),
                ledger_relative_path: "ledger.yaml".into(),
                impact_proven: true,
                stopped_work_ids: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Protocol(ref message) if message.contains("refusing overwrite")),
        "{err:?}"
    );
    assert_eq!(
        std::fs::read(tmp.root.join("ledger.yaml")).unwrap(),
        foreign
    );
}
