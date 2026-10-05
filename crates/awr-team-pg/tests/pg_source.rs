#![cfg(feature = "pg-tests")]
//! TEAM-P3 source ingest/approval/activation tests on real PostgreSQL.
//! Fixture isolation comes from tests/common (CR #37 P2-4): this file never
//! reads the runtime AWR_TEAM_DATABASE_URL and only cleans the database this
//! process created.

use awr_team::{SourceActivationPlan, WorkContract, WorkId};
use awr_team_pg::{IngestRequest, PgError, SourceFile, SourceStore, validate_source_path};
use std::sync::MutexGuard;
use tokio_postgres::Client;

mod common;
use common::{fresh_team_schema, test_config, with_app_role};

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";
const AUTHOR: &str = "actor-a";
const REVIEWER: &str = "actor-b";
const READER: &str = "actor-c";
const WORKER: &str = "actor-e";
const DISABLED: &str = "actor-d";
const PARSER_V1: &str = "awr-team-source/1";
const PARSER_V2: &str = "awr-team-source/2";

async fn setup() -> (MutexGuard<'static, ()>, Client, SourceStore) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
                ('tenant-a','actor-a','agent','A','active'),
                ('tenant-a','actor-b','human','B','active'),
                ('tenant-a','actor-c','human','C','active'),
                ('tenant-a','actor-d','human','D','disabled'),
                ('tenant-a','actor-e','human','E','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
                ('tenant-a','project-a','actor-b','reviewer'),
                ('tenant-a','project-a','actor-c','reader'),
                ('tenant-a','project-a','actor-d','reviewer'),
                ('tenant-a','project-a','actor-e','worker');
             INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                VALUES ('tenant-a','project-a','main','main','active');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                VALUES ('tenant-a','project-a','work-a','W');",
        )
        .await
        .unwrap();
    let store = SourceStore::from_config(with_app_role(&test_config(), &db));
    (guard, admin, store)
}

fn contract_bytes(acceptance: &str) -> Vec<u8> {
    let contract = WorkContract {
        dependency_acceptance: Default::default(),
        execution_settlement: None,
        codec: WorkContract::CODEC.into(),
        work_id: WorkId::new("work-a").unwrap(),
        external_key: "W".into(),
        goals: vec!["g".into()],
        hard_rules: vec!["r".into()],
        scope_paths: vec!["src".into()],
        acceptance: vec![acceptance.into()],
        required_dependencies: vec![],
        completion_policy: "evidence".into(),
        verification_requirements: vec!["report".into()],
    };
    serde_json::to_vec(&contract).unwrap()
}

fn package(parser: &str, acceptance: &str) -> IngestRequest {
    IngestRequest {
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        actor_id: AUTHOR.into(),
        parser_version: parser.into(),
        files: vec![SourceFile {
            path: "contract.json".into(),
            bytes: contract_bytes(acceptance),
        }],
    }
}

fn plan(candidate: &awr_team_pg::CandidateRecord, epoch: &str) -> SourceActivationPlan {
    SourceActivationPlan {
        candidate_digest: candidate.manifest_digest.clone(),
        parser_version: candidate.parser_version.clone(),
        expected_authority_epoch: epoch.into(),
        approved_candidate_digest: candidate.manifest_digest.clone(),
    }
}

#[test]
fn path_safety_is_enforced_without_postgres() {
    assert!(validate_source_path("../etc/passwd").is_err());
    assert!(validate_source_path("/abs").is_err());
    assert!(validate_source_path("ok/file.yaml").is_ok());
}

#[tokio::test]
async fn unactivated_candidate_is_not_current_contract() {
    let (_lock, _, store) = setup().await;
    let candidate = store.ingest(package(PARSER_V1, "a")).await.unwrap();
    let err = store
        .contract_for_snapshot(TENANT, PROJECT, &candidate.snapshot_id, "work-a")
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::InactiveCandidate));
}

#[tokio::test]
async fn crash_between_projection_write_and_commit_keeps_old_source() {
    let (_lock, _, store) = setup().await;
    let first = store.ingest(package(PARSER_V1, "a")).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &first.proposal_id,
            REVIEWER,
            &first.manifest_digest,
        )
        .await
        .unwrap();
    let current = store
        .activate(
            TENANT,
            PROJECT,
            AUTHOR,
            &first.proposal_id,
            &plan(&first, "0"),
        )
        .await
        .unwrap();
    let second = store.ingest(package(PARSER_V1, "b")).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &second.proposal_id,
            REVIEWER,
            &second.manifest_digest,
        )
        .await
        .unwrap();
    store
        .abort_after_installing_projection(
            TENANT,
            PROJECT,
            AUTHOR,
            &second.proposal_id,
            &plan(&second, "1"),
        )
        .await
        .unwrap();
    let still = store.current(TENANT, PROJECT, "work-a").await.unwrap();
    assert_eq!(still.snapshot_id, current.snapshot_id);
    assert_eq!(still.contract_hash, current.contract_hash);
}

// Strengthened (CR #37 note): BOTH candidates are legitimately approved;
// the only broken field is the approved digest, so the failure must be
// precisely StaleApproval.
#[tokio::test]
async fn old_approval_cannot_activate_a_new_digest() {
    let (_lock, _, store) = setup().await;
    let first = store.ingest(package(PARSER_V1, "a")).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &first.proposal_id,
            REVIEWER,
            &first.manifest_digest,
        )
        .await
        .unwrap();
    let second = store.ingest(package(PARSER_V1, "b")).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &second.proposal_id,
            REVIEWER,
            &second.manifest_digest,
        )
        .await
        .unwrap();
    let mut stolen = plan(&second, "0");
    stolen.approved_candidate_digest = first.manifest_digest.clone();
    stolen.candidate_digest = second.manifest_digest.clone();
    let err = store
        .activate(TENANT, PROJECT, AUTHOR, &second.proposal_id, &stolen)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::StaleApproval), "got {err}");
    let err = store.current(TENANT, PROJECT, "work-a").await.unwrap_err();
    assert!(matches!(err, PgError::InactiveCandidate));
}

// Strengthened (CR #37 note): the upgraded candidate is approved and only
// the parser version is wrong, so the failure must be precisely
// ParserMismatch.
#[tokio::test]
async fn parser_upgrade_does_not_silently_change_active_contract() {
    let (_lock, _, store) = setup().await;
    let first = store.ingest(package(PARSER_V1, "a")).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &first.proposal_id,
            REVIEWER,
            &first.manifest_digest,
        )
        .await
        .unwrap();
    let active = store
        .activate(
            TENANT,
            PROJECT,
            AUTHOR,
            &first.proposal_id,
            &plan(&first, "0"),
        )
        .await
        .unwrap();
    let upgraded = store.ingest(package(PARSER_V2, "a")).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &upgraded.proposal_id,
            REVIEWER,
            &upgraded.manifest_digest,
        )
        .await
        .unwrap();
    assert_ne!(upgraded.manifest_digest, first.manifest_digest);
    assert_ne!(upgraded.parser_version, first.parser_version);
    let mut reused = plan(&upgraded, "1");
    reused.parser_version = PARSER_V1.into();
    let err = store
        .activate(TENANT, PROJECT, AUTHOR, &upgraded.proposal_id, &reused)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::ParserMismatch), "got {err}");
    let still = store.current(TENANT, PROJECT, "work-a").await.unwrap();
    assert_eq!(still.snapshot_id, active.snapshot_id);
    assert_eq!(still.parser_version, PARSER_V1);
}

#[tokio::test]
async fn author_cannot_approve_own_candidate_and_unsafe_paths_never_land() {
    let (_lock, admin, store) = setup().await;
    let candidate = store.ingest(package(PARSER_V1, "a")).await.unwrap();
    let err = store
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            AUTHOR,
            &candidate.manifest_digest,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::AuthorCannotApprove));
    let mut unsafe_pkg = package(PARSER_V1, "a");
    unsafe_pkg.files[0].path = "../secret.json".into();
    let err = store.ingest(unsafe_pkg).await.unwrap_err();
    assert!(matches!(err, PgError::UnsafeSourcePath(_)));
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.source_snapshots", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

// CR #37 P2-1: reviewer identity is enforced — nonexistent accounts,
// disabled accounts and members without an approval role are all rejected;
// source-approval memberships succeed. This trusted operator source API is
// distinct from authenticated independent review.decide authority.
#[tokio::test]
async fn reviewer_identity_is_enforced() {
    let (_lock, _, store) = setup().await;
    for (reviewer, why) in [
        ("ghost-reviewer", "nonexistent account"),
        (DISABLED, "disabled account"),
        (READER, "membership without source approval role"),
    ] {
        let candidate = store.ingest(package(PARSER_V1, "a")).await.unwrap();
        let err = store
            .approve(
                TENANT,
                PROJECT,
                &candidate.proposal_id,
                reviewer,
                &candidate.manifest_digest,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PgError::Forbidden), "{why}: got {err}");
    }
    for reviewer in [REVIEWER, WORKER] {
        let candidate = store.ingest(package(PARSER_V1, "a")).await.unwrap();
        store
            .approve(
                TENANT,
                PROJECT,
                &candidate.proposal_id,
                reviewer,
                &candidate.manifest_digest,
            )
            .await
            .expect("valid independent source approval membership must succeed");
    }
}

// CR #37 P2-2: A and B are generated from the same baseline and both
// approved; after A activates, B cannot be pushed over it by refreshing the
// expected epoch parameter — its candidate baseline is stale.
#[tokio::test]
async fn stale_base_epoch_cannot_cover_newer_source() {
    let (_lock, _, store) = setup().await;
    let a = store.ingest(package(PARSER_V1, "a")).await.unwrap();
    let b = store.ingest(package(PARSER_V1, "b")).await.unwrap();
    for candidate in [&a, &b] {
        store
            .approve(
                TENANT,
                PROJECT,
                &candidate.proposal_id,
                REVIEWER,
                &candidate.manifest_digest,
            )
            .await
            .unwrap();
    }
    let active = store
        .activate(TENANT, PROJECT, AUTHOR, &a.proposal_id, &plan(&a, "0"))
        .await
        .unwrap();
    assert_eq!(active.authority_epoch, "1");
    let err = store
        .activate(TENANT, PROJECT, AUTHOR, &b.proposal_id, &plan(&b, "1"))
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::EpochMismatch), "got {err}");
    let still = store.current(TENANT, PROJECT, "work-a").await.unwrap();
    assert_eq!(still.snapshot_id, active.snapshot_id);
}

// CR #37 P2-3: text-only input contract — invalid UTF-8 sidecar files are
// rejected before persistence.
#[tokio::test]
async fn invalid_utf8_sidecar_is_rejected_before_persistence() {
    let (_lock, admin, store) = setup().await;
    let mut pkg = package(PARSER_V1, "a");
    pkg.files.push(SourceFile {
        path: "notes.bin".into(),
        bytes: vec![0xff],
    });
    let err = store.ingest(pkg).await.unwrap_err();
    assert!(matches!(err, PgError::InvalidUtf8(_)), "got {err}");
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.source_snapshots", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0, "rejected package left a snapshot");
}

// CR #37 P2-3 positive round-trip: valid UTF-8 (including CJK) survives
// ingest and activation byte-identically.
#[tokio::test]
async fn valid_utf8_round_trips_byte_identically() {
    let (_lock, admin, store) = setup().await;
    let mut pkg = package(PARSER_V1, "a");
    let chinese = "验收标准：今天的订单查询正确返回。";
    pkg.files.push(SourceFile {
        path: "notes.md".into(),
        bytes: chinese.as_bytes().to_vec(),
    });
    let candidate = store.ingest(pkg).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            REVIEWER,
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    store
        .activate(
            TENANT,
            PROJECT,
            AUTHOR,
            &candidate.proposal_id,
            &plan(&candidate, "0"),
        )
        .await
        .unwrap();
    let stored: String = admin
        .query_one(
            "SELECT f->>'text' FROM awr_team.source_snapshots s,
             LATERAL jsonb_array_elements(s.source_ref_json->'files') f
             WHERE s.id=$1 AND f->>'path'='notes.md'",
            &[&candidate.snapshot_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored, chinese);
}

// CR #37 P2-3 fail-closed: content tampered after ingest no longer matches
// the recorded digest, so activation refuses to proceed.
#[tokio::test]
async fn tampered_snapshot_text_fails_closed() {
    let (_lock, admin, store) = setup().await;
    let candidate = store.ingest(package(PARSER_V1, "a")).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            REVIEWER,
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    // Swap in a DIFFERENT but still valid contract, keeping the recorded
    // digest/length metadata untouched. Without the digest check the file
    // would parse and activate fine, so this test only passes while the
    // integrity check exists (CR #54 P3).
    let swapped = String::from_utf8(contract_bytes("b")).unwrap();
    admin
        .execute(
            "UPDATE awr_team.source_snapshots
             SET source_ref_json = jsonb_set(source_ref_json, '{files,0,text}', to_jsonb($2::text))
             WHERE id=$1",
            &[&candidate.snapshot_id, &swapped],
        )
        .await
        .unwrap();
    let err = store
        .activate(
            TENANT,
            PROJECT,
            AUTHOR,
            &candidate.proposal_id,
            &plan(&candidate, "0"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::SnapshotDrift(_)), "got {err}");
}

// CR #54 P2: an approval written by the PRE-FIX API (ghost reviewer, valid
// digest, proposal marked approved) must not activate after the upgrade;
// adding a valid independent approval then lets the same candidate through.
#[tokio::test]
async fn legacy_ghost_approval_cannot_activate_after_upgrade() {
    let (_lock, admin, store) = setup().await;
    let candidate = store.ingest(package(PARSER_V1, "a")).await.unwrap();
    // Reproduce exactly what the pre-fix approve() accepted: a reviewer
    // string that does not exist, a matching digest, state approved.
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.source_approvals(
                tenant_id, project_id, id, proposal_id, candidate_digest,
                reviewer_actor_id, decision)
             VALUES ('tenant-a','project-a','legacy-approval-1','{}','{}','ghost-reviewer','approve');
             UPDATE awr_team.source_proposals SET state='approved'
             WHERE id='{}';",
            candidate.proposal_id, candidate.manifest_digest, candidate.proposal_id
        ))
        .await
        .unwrap();
    let err = store
        .activate(
            TENANT,
            PROJECT,
            AUTHOR,
            &candidate.proposal_id,
            &plan(&candidate, "0"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "got {err}");
    let blocked = store.current(TENANT, PROJECT, "work-a").await.unwrap_err();
    assert!(matches!(blocked, PgError::InactiveCandidate));
    // A valid independent approval supersedes the ghost record (latest wins).
    store
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            REVIEWER,
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    store
        .activate(
            TENANT,
            PROJECT,
            AUTHOR,
            &candidate.proposal_id,
            &plan(&candidate, "0"),
        )
        .await
        .expect("candidate with a valid approval must activate");
}
