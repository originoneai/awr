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
        definition_state: state,
        workstream: None,
        split_from: None,
        split_children: vec![],
    }
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
async fn writeback_storage_failure_retains_journal_and_resumes_the_same_request() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate_with_workstream(&store, "alpha").await;
    let tmp = tempfile_ledger();
    let path = tmp.root.join("ledger.yaml");
    let before = std::fs::read(&path).unwrap();
    // Deterministic filesystem failure, including when tests run as root.
    // The source remains readable, but the atomic replacement cannot be written.
    let obstruction = tmp.root.join(".ledger.yaml.tmcp022.tmp");
    std::fs::create_dir(&obstruction).unwrap();
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

    std::fs::remove_dir(&obstruction).unwrap();
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
