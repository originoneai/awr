#![cfg(feature = "pg-tests")]
//! Source coordination on a real isolated database. This is not authenticated
//! transport or business acceptance; the fixtures use trusted operator APIs.
mod common;
#[path = "cases/source_activation_impact.rs"]
mod source_activation_impact;
#[path = "cases/source_completion_invalidation.rs"]
mod source_completion_invalidation;
use awr_core::{Id, Workstream, WorkstreamCatalog, WorkstreamState};
use awr_team::{SourceActivationPlan, WorkContract, WorkId, WorkstreamBundle, WorkstreamContract};
use awr_team_pg::{
    CandidateRecord, IngestRequest, LeaseStore, PgError, ReadStore, SourceFile, SourceStore,
};
use common::{fresh_team_schema, test_config, with_app_role};
use std::sync::MutexGuard;
use tokio_postgres::Client;

const TENANT: &str = "ws-source-tenant";
const PROJECT: &str = "ws-source-project";

async fn setup() -> (MutexGuard<'static, ()>, Client, String, SourceStore) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES ('ws-source-tenant','Test','active');
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
          ('ws-source-tenant','author','agent','Author','active'),
          ('ws-source-tenant','reviewer','human','Reviewer','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
          VALUES('ws-source-tenant','ws-source-project','ws','team','epoch-1','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
          VALUES('ws-source-tenant','ws-source-project','reviewer','reviewer');
        INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
          VALUES('ws-source-tenant','ws-source-project','main','Main','active');").await.unwrap();
    let store = SourceStore::from_config(with_app_role(&test_config(), &db));
    (guard, admin, db, store)
}

fn bundle() -> WorkstreamBundle {
    let streams = [(1, "api"), (2, "sdk")]
        .into_iter()
        .map(|(id, key)| Workstream {
            id: Id::from(id),
            project_id: PROJECT.into(),
            external_key: key.into(),
            title: key.into(),
            state: WorkstreamState::Active,
            authority_version: 1,
            goal_keys: vec![key.into()],
            acceptance_contracts: vec![],
        })
        .collect();
    let contracts = [
        ("interface", 1, vec![]),
        ("sdk", 2, vec!["interface"]),
        ("integration", 1, vec!["sdk"]),
    ]
    .into_iter()
    .map(|(key, owner, dependencies)| WorkstreamContract {
        workstream_id: Id::from(owner),
        contract: WorkContract {
            dependency_acceptance: Default::default(),
            execution_settlement: None,
            codec: WorkContract::CODEC.into(),
            work_id: WorkId::new(key).unwrap(),
            external_key: key.into(),
            goals: vec!["ship".into()],
            hard_rules: vec!["preserve interfaces".into()],
            scope_paths: vec!["src".into()],
            acceptance: vec!["verified delivery".into()],
            required_dependencies: dependencies.into_iter().map(Into::into).collect(),
            completion_policy: "independent_review".into(),
            verification_requirements: vec!["report".into()],
        },
    })
    .collect();
    WorkstreamBundle {
        codec: WorkstreamBundle::CODEC.into(),
        catalog: WorkstreamCatalog {
            version: 1,
            project_id: PROJECT.into(),
            legacy_default: None,
            workstreams: streams,
        },
        contracts,
    }
}

fn package(bundle: &WorkstreamBundle) -> IngestRequest {
    IngestRequest {
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        actor_id: "author".into(),
        parser_version: "awr-team-workstreams/1".into(),
        files: vec![SourceFile {
            path: "workstreams.json".into(),
            bytes: serde_json::to_vec(bundle).unwrap(),
        }],
    }
}

fn legacy_package() -> IngestRequest {
    let mut request = package(&bundle());
    request.files = vec![SourceFile {
        path: "contract.json".into(),
        bytes: serde_json::to_vec(&bundle().contracts[0].contract).unwrap(),
    }];
    request
}

fn plan(candidate: &CandidateRecord) -> SourceActivationPlan {
    SourceActivationPlan {
        candidate_digest: candidate.manifest_digest.clone(),
        approved_candidate_digest: candidate.manifest_digest.clone(),
        parser_version: candidate.parser_version.clone(),
        expected_authority_epoch: candidate.base_epoch.clone(),
    }
}

async fn approved(store: &SourceStore, request: IngestRequest) -> CandidateRecord {
    let c = store.ingest(request).await.unwrap();
    store
        .approve(
            TENANT,
            PROJECT,
            &c.proposal_id,
            "reviewer",
            &c.manifest_digest,
        )
        .await
        .unwrap();
    c
}

async fn current_snapshot(admin: &Client) -> Option<String> {
    admin
        .query_one(
            "SELECT active_snapshot_id FROM awr_team.projects WHERE id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn explicit_v3_source_retains_policy_and_legacy_hash_without_execution_authority() {
    use awr_team::{ExecutionSettlementMode, ExecutionSettlementPolicy};

    let (_guard, admin, _, store) = setup().await;
    let legacy = bundle();
    let first = approved(&store, package(&legacy)).await;
    let baseline = store
        .activate_workstreams(TENANT, PROJECT, "author", &first.proposal_id, &plan(&first))
        .await
        .unwrap();
    let mut value = legacy.clone();
    value.codec = WorkstreamBundle::CODEC_V3.into();
    let contract = &mut value.contracts[0].contract;
    contract.codec = WorkContract::CODEC_V3.into();
    contract.completion_policy = ExecutionSettlementPolicy::COMPLETION_POLICY.into();
    contract.execution_settlement = Some(ExecutionSettlementPolicy {
        mode: ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "worker-a".into(),
    });
    let mut request = package(&value);
    request.parser_version = "awr-team-workstreams/3".into();
    let candidate = approved(&store, request).await;
    let active = store
        .activate_workstreams(
            TENANT,
            PROJECT,
            "author",
            &candidate.proposal_id,
            &plan(&candidate),
        )
        .await
        .unwrap();
    assert_ne!(
        active.contract_hashes["interface"],
        baseline.contract_hashes["interface"]
    );
    assert_eq!(active.projection_hash, value.hash().unwrap());
    assert_eq!(
        current_snapshot(&admin).await,
        Some(candidate.snapshot_id.clone())
    );
    assert!(matches!(
        store.current(TENANT, PROJECT, "interface").await,
        Err(PgError::Unsupported(_))
    ));
    for entry in &value.contracts {
        let expected = &entry.contract;
        let hash = expected.hash().unwrap();
        assert_eq!(active.contract_hashes[expected.work_id.as_str()], hash);
        let row = admin
            .query_one(
                "SELECT c.contract_json, c.contract_hash, s.parser_version
                 FROM awr_team.work_contracts c
                 JOIN awr_team.source_snapshots s
                 ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.snapshot_id
                 WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.work_id=$4",
                &[
                    &TENANT,
                    &PROJECT,
                    &candidate.snapshot_id,
                    &expected.work_id.as_str(),
                ],
            )
            .await
            .unwrap();
        let stored: serde_json::Value = row.get(0);
        assert_eq!(
            serde_json::from_value::<WorkContract>(stored).unwrap(),
            *expected
        );
        assert_eq!(row.get::<_, String>(1), hash);
        assert_eq!(row.get::<_, String>(2), "awr-team-workstreams/3");
        if expected.work_id.as_str() != "interface" {
            assert_eq!(hash, baseline.contract_hashes[expected.work_id.as_str()]);
        }
    }
    let counts = admin
        .query_one(
            "SELECT
         (SELECT count(*) FROM awr_team.executions),
         (SELECT count(*) FROM awr_team.execution_receipts),
         (SELECT count(*) FROM awr_team.completion_receipts),
         (SELECT count(*) FROM awr_team.resource_reservations),
         (SELECT count(*) FROM awr_team.workstream_grants)",
            &[],
        )
        .await
        .unwrap();
    for index in 0..5 {
        assert_eq!(
            counts.get::<_, i64>(index),
            0,
            "source publication cannot invent runtime authority"
        );
    }
    value.contracts[0].contract.execution_settlement = None;
    let mut invalid = package(&value);
    invalid.parser_version = "awr-team-workstreams/3".into();
    assert!(matches!(
        store.ingest(invalid).await,
        Err(PgError::Protocol(_))
    ));
    assert_eq!(current_snapshot(&admin).await, Some(candidate.snapshot_id));
    let snapshots: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.source_snapshots", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(snapshots, 2);
}

async fn wait_for_lock(admin: &Client, fragment: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = admin
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)",
                    &[&format!("%{fragment}%")],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected PostgreSQL lock wait");
}

#[tokio::test]
async fn admitted_legacy_session_commits_before_enablement_rechecks_history() {
    let (_guard, mut admin, db, store) = setup().await;
    let legacy = approved(&store, legacy_package()).await;
    store
        .activate(
            TENANT,
            PROJECT,
            "author",
            &legacy.proposal_id,
            &plan(&legacy),
        )
        .await
        .unwrap();
    let c = approved(&store, package(&bundle())).await;
    let observer = common::connect_config(&common::with_db(&test_config(), &db)).await;
    let lock = admin.transaction().await.unwrap();
    lock.query_one(
        "SELECT id FROM awr_team.projects WHERE id=$1 FOR UPDATE",
        &[&PROJECT],
    )
    .await
    .unwrap();
    let config = with_app_role(&test_config(), &db);
    let session = tokio::spawn(async move {
        LeaseStore::from_config(config)
            .start_session(
                TENANT,
                PROJECT,
                "author",
                "cli",
                "concurrent",
                "main",
                "interface",
            )
            .await
    });
    wait_for_lock(&observer, "projects").await;
    let activation = tokio::spawn(async move {
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await
    });
    wait_for_lock(&observer, "workstream_modes").await;
    lock.commit().await.unwrap();
    session.await.unwrap().unwrap();
    assert!(matches!(
        activation.await.unwrap(),
        Err(PgError::Unsupported(_))
    ));
    assert_eq!(current_snapshot(&admin).await, Some(legacy.snapshot_id));
}

#[tokio::test]
async fn migration_from_schema_nine_preserves_legacy_history_and_failed_ddl_is_atomic() {
    let (_guard, admin, _) = fresh_team_schema().await;
    // This database was created exclusively by this test process.
    admin
        .batch_execute("DROP SCHEMA awr_team CASCADE")
        .await
        .unwrap();
    for sql in [
        include_str!("../migrations/20260917000001_init.sql"),
        include_str!("../migrations/20260918000002_session_wait.sql"),
        include_str!("../migrations/20260918000003_graph_resources.sql"),
        include_str!("../migrations/20260918000004_execution_protocol.sql"),
        include_str!("../migrations/20260918000005_review_completion.sql"),
        include_str!("../migrations/20260918000006_import_restore.sql"),
        include_str!("../migrations/20260919000007_completion_integrity.sql"),
        include_str!("../migrations/20260920000008_execution_result_binding.sql"),
        include_str!("../migrations/20260920000009_import_integrity.sql"),
    ] {
        admin.batch_execute(sql).await.unwrap();
    }
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES ('old-tenant','Old','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
          VALUES('old-tenant','old-project','old','team','old-epoch','active');
        INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
          VALUES('old-tenant','old-project','old-session','main','old-work','actor','cli','conversation','ended');
        INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json)
          VALUES('old-tenant','old-project','old-event',1,0,'test.history','actor','old-work','{}');").await.unwrap();
    let migration = include_str!("../migrations/20260921000010_workstreams.sql");
    let injected = migration.replace(
        "UPDATE awr_team.schema_state",
        "SELECT 1/0; UPDATE awr_team.schema_state",
    );
    assert!(admin.batch_execute(&injected).await.is_err());
    admin.batch_execute("ROLLBACK").await.unwrap();
    let before = admin
        .query_one(
            "SELECT version, to_regclass('awr_team.workstream_modes')::text
        FROM awr_team.schema_state",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(before.get::<_, i32>(0), 9);
    assert_eq!(before.get::<_, Option<String>>(1), None);
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::migrate(&admin).await.unwrap();
    let after = admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),
        (SELECT count(*) FROM awr_team.workstream_modes WHERE NOT enabled),
        (SELECT count(*) FROM awr_team.sessions WHERE id='old-session' AND workstream_id IS NULL AND scope_id='main'),
        (SELECT count(*) FROM awr_team.events WHERE id='old-event' AND workstream_id IS NULL)", &[]).await.unwrap();
    assert_eq!(
        (
            after.get::<_, i32>(0),
            after.get::<_, i64>(1),
            after.get::<_, i64>(2),
            after.get::<_, i64>(3)
        ),
        (awr_team_pg::EXPECTED_SCHEMA_VERSION, 1, 1, 1)
    );
}

#[tokio::test]
async fn explicit_activation_installs_complete_dag_without_redefining_main_or_v1_hashes() {
    let (_guard, admin, db, store) = setup().await;
    let value = bundle();
    let candidate = approved(&store, package(&value)).await;
    assert_eq!(candidate.preview_hash, value.hash().unwrap());
    assert!(matches!(
        store
            .activate(
                TENANT,
                PROJECT,
                "author",
                &candidate.proposal_id,
                &plan(&candidate)
            )
            .await,
        Err(PgError::Unsupported(_))
    ));
    assert_eq!(current_snapshot(&admin).await, None);
    let result = store
        .activate_workstreams(
            TENANT,
            PROJECT,
            "author",
            &candidate.proposal_id,
            &plan(&candidate),
        )
        .await
        .unwrap();
    assert_eq!(result.projection_hash, value.hash().unwrap());
    for entry in &value.contracts {
        assert_eq!(
            result.contract_hashes[entry.contract.work_id.as_str()],
            entry.contract.hash().unwrap()
        );
    }
    let counts = admin
        .query_one(
            "SELECT
        (SELECT count(*) FROM awr_team.workstream_snapshot_ownership WHERE scope_id='main'),
        (SELECT count(*) FROM awr_team.work_contracts WHERE scope_id='main'),
        (SELECT count(*) FROM awr_team.dependency_edges WHERE required),
        (SELECT count(*) FROM awr_team.work_scopes),
        (SELECT count(*) FROM awr_team.workstream_grants)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        (
            counts.get::<_, i64>(0),
            counts.get::<_, i64>(1),
            counts.get::<_, i64>(2),
            counts.get::<_, i64>(3),
            counts.get::<_, i64>(4)
        ),
        (3, 3, 2, 1, 0)
    );
    let edges = admin
        .query(
            "SELECT from_work_id, to_work_id, relation
             FROM awr_team.dependency_edges ORDER BY from_work_id, to_work_id",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.get::<_, String>(0),
                row.get::<_, String>(1),
                row.get::<_, String>(2),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        edges,
        vec![
            ("integration".into(), "sdk".into(), "requires".into()),
            ("sdk".into(), "interface".into(), "requires".into()),
        ]
    );
    assert!(matches!(
        ReadStore::from_config(with_app_role(&test_config(), &db))
            .graph(TENANT, PROJECT)
            .await,
        Err(PgError::Unsupported(_))
    ));
    assert!(matches!(
        LeaseStore::from_config(with_app_role(&test_config(), &db))
            .start_session(
                TENANT,
                PROJECT,
                "author",
                "old-client",
                "conversation",
                "main",
                "interface"
            )
            .await,
        Err(PgError::Unsupported(_))
    ));
    let downgrade = approved(&store, legacy_package()).await;
    assert!(matches!(
        store
            .activate(
                TENANT,
                PROJECT,
                "author",
                &downgrade.proposal_id,
                &plan(&downgrade)
            )
            .await,
        Err(PgError::Unsupported(_))
    ));
    assert!(matches!(
        store
            .activate_workstreams(
                TENANT,
                PROJECT,
                "author",
                &downgrade.proposal_id,
                &plan(&downgrade)
            )
            .await,
        Err(PgError::Unsupported(_))
    ));
    assert_eq!(current_snapshot(&admin).await, Some(result.snapshot_id));
    let app = common::app_client(&db).await;
    let count: i64 = app
        .query_one("SELECT count(*) FROM awr_team.workstream_catalogs", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0, "unbound app connection must see no tenant data");
}

#[tokio::test]
async fn complete_bundle_rejects_missing_dependencies_cycles_and_ambiguous_codecs_before_ingest() {
    let (_guard, admin, _, store) = setup().await;
    let mut missing = bundle();
    missing.contracts[1].contract.required_dependencies = vec!["absent".into()];
    assert!(matches!(
        store.ingest(package(&missing)).await,
        Err(PgError::MissingDependency)
    ));
    let mut cycle = bundle();
    cycle.contracts[0].contract.required_dependencies = vec!["integration".into()];
    assert!(matches!(
        store.ingest(package(&cycle)).await,
        Err(PgError::DependencyCycle(_))
    ));
    let mut both = package(&bundle());
    both.files.extend(legacy_package().files);
    assert!(matches!(
        store.ingest(both).await,
        Err(PgError::Protocol(_))
    ));
    let n: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.source_snapshots", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(n, 0);
}

#[tokio::test]
async fn aborted_enablement_rolls_back_catalog_ownership_graph_and_mode_together() {
    let (_guard, admin, _, store) = setup().await;
    let legacy = approved(&store, legacy_package()).await;
    let old = store
        .activate(
            TENANT,
            PROJECT,
            "author",
            &legacy.proposal_id,
            &plan(&legacy),
        )
        .await
        .unwrap();
    let scoped = approved(&store, package(&bundle())).await;
    store
        .abort_workstreams_after_installing_projection(
            TENANT,
            PROJECT,
            "author",
            &scoped.proposal_id,
            &plan(&scoped),
        )
        .await
        .unwrap();
    assert_eq!(
        current_snapshot(&admin).await,
        Some(old.snapshot_id.clone())
    );
    let row = admin
        .query_one(
            "SELECT enabled,
        (SELECT count(*) FROM awr_team.workstream_catalogs),
        (SELECT count(*) FROM awr_team.workstream_ownership),
        (SELECT count(*) FROM awr_team.dependency_edges),
        (SELECT count(*) FROM awr_team.work_contracts) FROM awr_team.workstream_modes",
            &[],
        )
        .await
        .unwrap();
    assert!(!row.get::<_, bool>(0));
    assert_eq!(
        (
            row.get::<_, i64>(1),
            row.get::<_, i64>(2),
            row.get::<_, i64>(3),
            row.get::<_, i64>(4)
        ),
        (0, 0, 0, 1)
    );
    assert_eq!(
        store
            .current(TENANT, PROJECT, "interface")
            .await
            .unwrap()
            .contract_hash,
        old.contract_hash
    );
    let result = store
        .activate_workstreams(
            TENANT,
            PROJECT,
            "author",
            &scoped.proposal_id,
            &plan(&scoped),
        )
        .await
        .unwrap();
    assert_eq!(result.contract_hashes["interface"], old.contract_hash);
}

#[tokio::test]
async fn catalog_edits_retain_identity_authority_and_historical_ownership() {
    let (_guard, admin, _, store) = setup().await;
    let value = bundle();
    let first = approved(&store, package(&value)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &first.proposal_id, &plan(&first))
        .await
        .unwrap();
    let mut moved = value.clone();
    moved.contracts[0].workstream_id = Id::from(2);
    let candidate = approved(&store, package(&moved)).await;
    assert!(matches!(
        store
            .activate_workstreams(
                TENANT,
                PROJECT,
                "author",
                &candidate.proposal_id,
                &plan(&candidate)
            )
            .await,
        Err(PgError::Unsupported(_))
    ));
    let mut stale = value.clone();
    stale.catalog.workstreams[0].state = WorkstreamState::Paused;
    let candidate = approved(&store, package(&stale)).await;
    assert!(matches!(
        store
            .activate_workstreams(
                TENANT,
                PROJECT,
                "author",
                &candidate.proposal_id,
                &plan(&candidate)
            )
            .await,
        Err(PgError::Workstream(
            awr_core::WorkstreamError::StaleAuthority
        ))
    ));
    let mut removed = value.clone();
    removed.contracts.retain(|c| c.workstream_id == Id::from(1));
    removed.contracts[1].contract.required_dependencies.clear();
    removed.catalog.workstreams.pop();
    let candidate = approved(&store, package(&removed)).await;
    assert!(matches!(
        store
            .activate_workstreams(
                TENANT,
                PROJECT,
                "author",
                &candidate.proposal_id,
                &plan(&candidate)
            )
            .await,
        Err(PgError::Protocol(_))
    ));
    assert_eq!(current_snapshot(&admin).await, Some(first.snapshot_id));
    let mut rename = value;
    rename.catalog.workstreams[0].title = "API team".into();
    let candidate = approved(&store, package(&rename)).await;
    store
        .activate_workstreams(
            TENANT,
            PROJECT,
            "author",
            &candidate.proposal_id,
            &plan(&candidate),
        )
        .await
        .unwrap();
    assert_eq!(current_snapshot(&admin).await, Some(candidate.snapshot_id));
    let revisions: Vec<i64> = admin
        .query(
            "SELECT ownership_version FROM awr_team.workstream_ownership",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(revisions, vec![1, 1, 1]);
}

#[tokio::test]
async fn legacy_session_history_requires_explicit_migration_and_is_not_reassigned() {
    let (_guard, admin, db, store) = setup().await;
    let legacy = approved(&store, legacy_package()).await;
    store
        .activate(
            TENANT,
            PROJECT,
            "author",
            &legacy.proposal_id,
            &plan(&legacy),
        )
        .await
        .unwrap();
    LeaseStore::from_config(with_app_role(&test_config(), &db))
        .start_session(
            TENANT,
            PROJECT,
            "author",
            "cli",
            "history",
            "main",
            "interface",
        )
        .await
        .unwrap();
    let c = approved(&store, package(&bundle())).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::Unsupported(_))
    ));
    assert_eq!(current_snapshot(&admin).await, Some(legacy.snapshot_id));
    let stream: Option<String> = admin
        .query_one("SELECT workstream_id FROM awr_team.sessions", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(stream, None);
}

#[tokio::test]
async fn unknown_execution_prevents_scope_enablement_even_without_a_live_claim() {
    let (_guard, admin, _, store) = setup().await;
    let c = approved(&store, legacy_package()).await;
    let old = store
        .activate(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
        VALUES($1,$2,'uncertain','interface',1,$3,'author','unknown')", &[&TENANT,&PROJECT,&old.contract_hash]).await.unwrap();
    let c = approved(&store, package(&bundle())).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(current_snapshot(&admin).await, Some(old.snapshot_id));
}

#[tokio::test]
async fn modified_manifest_cannot_reuse_the_reviewed_digest() {
    let (_guard, admin, _, store) = setup().await;
    let c = approved(&store, package(&bundle())).await;
    admin.execute("UPDATE awr_team.source_snapshots SET source_ref_json=jsonb_set(source_ref_json,'{manifest,parser_version}','\"forged\"') WHERE id=$1", &[&c.snapshot_id]).await.unwrap();
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::SnapshotDrift(_))
    ));
    assert_eq!(current_snapshot(&admin).await, None);
}

#[tokio::test]
async fn source_graph_sidecar_is_not_silently_dropped() {
    let (_guard, admin, _, store) = setup().await;
    let mut request = package(&bundle());
    request.files.push(SourceFile {
        path: "graph.json".into(),
        bytes: serde_json::to_vec(&serde_json::json!({"edges":[
        {"from":"interface","to":"integration","relation":"depends_on","required":true}]}))
        .unwrap(),
    });
    let c = approved(&store, request).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::Protocol(_))
    ));
    assert_eq!(current_snapshot(&admin).await, None);
}

#[tokio::test]
async fn enablement_waits_for_admitted_legacy_reader_and_later_reads_are_denied() {
    let (_guard, admin, db, store) = setup().await;
    let c = approved(&store, legacy_package()).await;
    store
        .activate(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
        .await
        .unwrap();
    let c = approved(&store, package(&bundle())).await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let read_barrier = barrier.clone();
    let config = with_app_role(&test_config(), &db);
    let reader = tokio::spawn(async move {
        ReadStore::from_config(config)
            .prepare_with_sync_point(TENANT, PROJECT, "interface", None, &read_barrier)
            .await
    });
    barrier.wait().await;
    let activation = tokio::spawn(async move {
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await
    });
    // Observe the lock wait in PostgreSQL before letting the old read finish;
    // timing alone is not evidence that enablement and admission serialize.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = admin
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock'
                AND query LIKE '%workstream_modes%' AND query LIKE '%FOR UPDATE%')",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("enablement must wait on the admission row");
    barrier.wait().await;
    assert_eq!(reader.await.unwrap().unwrap().work_id, "interface");
    activation.await.unwrap().unwrap();
    assert!(matches!(
        ReadStore::from_config(with_app_role(&test_config(), &db))
            .prepare(TENANT, PROJECT, "interface", None)
            .await,
        Err(PgError::Unsupported(_))
    ));
}
