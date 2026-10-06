#![cfg(feature = "pg-tests")]
//! Isolated real Git/PG mechanism regressions; no native business acceptance credit.
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod access;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "fixtures/local_git.rs"]
mod git;
#[path = "../../awr-team-pg/tests/fixtures/delivery_integration.rs"]
mod integration_fixture;

use access::*;
use awr_server::{
    config::{LocalGitWorkerConfig, LocalGitWorkerSpec},
    delivery_adapter::{LocalGitIntegrationConfig, LocalGitIntegrator},
    service::{
        ProjectBinding, ServiceConfig,
        delivery_sync::WorkerState,
        delivery_workers::DeliveryWorkers,
        local_git_worker::{
            LocalGitWorkerFailure, LocalGitWorkerMonitor, LocalGitWorkerRuntime,
            LocalGitWorkerSnapshot,
        },
    },
};
use awr_team::delivery::*;
use awr_team_pg::*;
use integration_fixture::{SUPERVISOR, WORKER};
use serde_json::{Value, json};
use std::time::Duration;

fn service() -> ServiceConfig {
    ServiceConfig {
        version: 1,
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_hosts: vec![],
        allowed_web_origins: vec![],
        oauth: None,
        projects: vec![ProjectBinding {
            key: "team".into(),
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
        }],
    }
}
fn config(repo: &git::GitFixture, name: &str, enabled: bool) -> LocalGitWorkerConfig {
    let mut repository = repo.config.clone();
    repository.inspection_timeout_ms = 3000;
    LocalGitWorkerConfig {
        version: 1,
        workers: vec![LocalGitWorkerSpec {
            project: "team".into(),
            worker_id: name.into(),
            credential_env: "AWR_SYNTHETIC_GIT_WORKER".into(),
            integration_enabled: enabled,
            repository,
            poll_interval_ms: 100,
            lease_seconds: 10,
            operation_timeout_ms: 5000,
            max_backoff_ms: 1000,
            page_size: 4,
            max_pages_per_poll: 2,
            max_jobs_per_poll: 4,
        }],
    }
}
async fn start(
    f: &integration_fixture::Fixture,
    cfg: LocalGitWorkerConfig,
) -> LocalGitWorkerRuntime {
    LocalGitWorkerRuntime::start(
        &service(),
        Some(cfg),
        DeliverySyncStore::from_config(f.config.clone()),
        |_| Some(WORKER.into()),
    )
    .await
    .unwrap()
}
async fn wait(
    monitor: &LocalGitWorkerMonitor,
    condition: impl Fn(&LocalGitWorkerSnapshot) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if monitor.snapshots().first().is_some_and(&condition) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("local Git worker condition timed out");
}
async fn count(f: &integration_fixture::Fixture, sql: &str) -> i64 {
    f.admin.query_one(sql, &[]).await.unwrap().get(0)
}
fn no_secret(value: impl serde::Serialize) {
    let text = serde_json::to_string(&value).unwrap();
    for secret in [WORKER, SUPERVISOR, A, "AWR_SYNTHETIC_GIT_WORKER"] {
        assert!(!text.contains(secret));
    }
}

/// Replace the initial reference connector through its real configuration API.
/// Publish a new synthetic report fact before preparing any integration intent.
/// Physical local_git.manifest-before-review source contracts are tested separately.
async fn setup_local(repo: &git::GitFixture) -> integration_fixture::Fixture {
    let mut f =
        integration_fixture::setup_integration_with_candidate(Some(repo.integration_candidate()))
            .await;
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "configure-local-worker".into(),
                read_set: f.set.clone(),
                expected_connector_version: "1".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "git".into(),
                    provider: "local_git".into(),
                    resource: repo.config.resource.clone(),
                    principal_actor_id: "integrator".into(),
                    principal_client_id: "cli-worker".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    f.request.connector_version = "2".into();
    let inspection = f
        .store
        .reserve_inspection(
            TENANT,
            PROJECT,
            WORKER,
            ReserveDeliveryInspection {
                request_id: "reserve-worker-report".into(),
                read_set: f.set.clone(),
                connector_id: "git".into(),
                connector_version: "2".into(),
                candidate_digest: f.request.candidate_digest.clone(),
                lease_seconds: 60,
            },
        )
        .await
        .unwrap();
    f.store
        .ingest_facts(
            TENANT,
            PROJECT,
            WORKER,
            IngestDeliveryFacts {
                request_id: "ingest-worker-report".into(),
                read_set: f.set.clone(),
                connector_id: "git".into(),
                inspection_id: inspection["data"]["inspection_id"].as_str().unwrap().into(),
                event_id: "worker-report".into(),
                records: vec![DeliveryEnvelope {
                    protocol: DELIVERY_PROTOCOL.into(),
                    protocol_version: DELIVERY_PROTOCOL_VERSION,
                    record: DeliveryRecord::Verification(VerificationRun {
                        binding: f.selection.candidate.binding.clone(),
                        run_id: "worker-report".into(),
                        check: "report".into(),
                        outcome: VerificationOutcome::Passed,
                        result_artifact: Some(f.selection.candidate.manifest.entries[0].clone()),
                        provenance: FactProvenance {
                            source: FactSource::AdapterObservation,
                            reference: "fixture://worker-report".into(),
                            observed_at_unix_ms: None,
                            recorded_at_unix_ms: 1,
                        },
                    }),
                }],
            },
        )
        .await
        .unwrap();
    f
}

async fn dispatched_without_effect(f: &integration_fixture::Fixture) -> String {
    let prepared = f.prepared().await;
    let id = prepared["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let permit = f.dispatched(id, lease["lease_id"].as_str().unwrap()).await;
    assert!(permit.permit.is_some());
    drop(permit); // A lost capability/reply proves no permission to redispatch.
    id.into()
}

#[tokio::test]
async fn absent_and_empty_groups_do_not_lookup_credentials_repositories_or_pg() {
    for cfg in [
        None,
        Some(LocalGitWorkerConfig {
            version: 1,
            workers: vec![],
        }),
    ] {
        let runtime = LocalGitWorkerRuntime::start(
            &service(),
            cfg,
            DeliverySyncStore::new("unconfigured"),
            |_| panic!("disabled workers must not resolve credentials"),
        )
        .await
        .unwrap();
        assert!(runtime.monitor().snapshots().is_empty());
        runtime.shutdown().await;
    }
    let runtime = DeliveryWorkers::start(
        &service(),
        None,
        Some(LocalGitWorkerConfig {
            version: 1,
            workers: vec![],
        }),
        DeliverySyncStore::new("unconfigured"),
        |_| panic!("empty groups must not resolve credentials"),
    )
    .await
    .unwrap();
    assert!(runtime.source_monitor().snapshots().is_empty());
    assert!(runtime.git_monitor().snapshots().is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn scope_limits_and_raw_credentials_fail_before_any_startup_access() {
    let repo = git::GitFixture::integration(false);
    let original = config(&repo, "limits", true);
    for mutate in [
        |c: &mut LocalGitWorkerConfig| c.version = 2,
        |c: &mut LocalGitWorkerConfig| c.workers[0].repository.project_id = "other".into(),
        |c: &mut LocalGitWorkerConfig| c.workers[0].repository.workstream_id = "invalid".into(),
        |c: &mut LocalGitWorkerConfig| c.workers[0].credential_env = "awr1.raw.value".into(),
        |c: &mut LocalGitWorkerConfig| c.workers[0].page_size = 33,
        |c: &mut LocalGitWorkerConfig| c.workers[0].operation_timeout_ms = 10000,
        |c: &mut LocalGitWorkerConfig| c.workers[0].max_jobs_per_poll = 0,
        |c: &mut LocalGitWorkerConfig| c.workers.push(c.workers[0].clone()),
    ] {
        let mut cfg = original.clone();
        mutate(&mut cfg);
        let result = LocalGitWorkerRuntime::start(
            &service(),
            Some(cfg),
            DeliverySyncStore::new("unconfigured"),
            |_| panic!("invalid configuration must precede access"),
        )
        .await;
        assert!(result.is_err());
        no_secret(result.err().unwrap());
    }
}

#[tokio::test]
async fn startup_read_permission_does_not_certify_observation_or_finalizer_permission() {
    let repo = git::GitFixture::integration(false);
    let f = setup_local(&repo).await;
    f.admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,can_manage=false WHERE client_id='cli-worker'").await.unwrap();
    let before = count(&f, "SELECT count(*) FROM awr_team.delivery_sync_requests").await;
    assert!(
        f.store
            .schedule(
                TENANT,
                PROJECT,
                WORKER,
                DeliveryScheduleQuery {
                    work_id: "a".into(),
                    connector_id: "git".into(),
                    cursor: None,
                    limit: 1
                }
            )
            .await
            .is_ok()
    );
    assert!(
        LocalGitWorkerRuntime::start(
            &service(),
            Some(config(&repo, "denied", false)),
            DeliverySyncStore::from_config(f.config.clone()),
            |_| Some(WORKER.into())
        )
        .await
        .is_err()
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_sync_requests").await,
        before
    );
    assert_eq!(f.guards().await, 0);
    f.admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=true WHERE client_id='cli-worker';
        UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]' WHERE actor_id='integrator'").await.unwrap();
    let observer = start(&f, config(&repo, "observer-only", false)).await;
    observer.shutdown().await;
    assert!(
        LocalGitWorkerRuntime::start(
            &service(),
            Some(config(&repo, "no-finalize", true)),
            DeliverySyncStore::from_config(f.config.clone()),
            |_| Some(WORKER.into())
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn observation_only_mode_never_prepares_or_executes_even_with_approved_evidence() {
    let repo = git::GitFixture::integration(false);
    let f = setup_local(&repo).await;
    let runtime = start(&f, config(&repo, "observe", false)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.current_changed > 0 && s.current_unchanged >= 2
    })
    .await;
    runtime.shutdown().await;
    no_secret(monitor.snapshots());
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_integration_intents"
        )
        .await,
        0
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn prepared_intent_is_integrated_once_and_reconstruction_preserves_terminal_history() {
    let repo = git::GitFixture::integration(false);
    repo.bare(&["config", "core.logAllRefUpdates", "true"]);
    let f = setup_local(&repo).await;
    let prepared = f.prepared().await;
    let runtime = start(&f, config(&repo, "integrate", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.confirmed == 1 && s.current_unchanged >= 2).await;
    runtime.shutdown().await;
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    assert_eq!(f.guards().await, 0);
    let view = f
        .store
        .inspect_integration(
            TENANT,
            PROJECT,
            WORKER,
            "a",
            prepared["integration_id"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(view["state"], "confirmed");
    assert_eq!(view["confirmation_current"], true);
    let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
    let second = start(&f, config(&repo, "restart", true)).await;
    let monitor = second.monitor();
    wait(&monitor, |s| {
        s.terminal_intents_observed >= 2 && s.current_unchanged >= 2
    })
    .await;
    second.shutdown().await;
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        facts
    );
    assert_eq!(
        repo.bare(&["reflog", "show", "--format=%H", "refs/heads/main"])
            .lines()
            .count(),
        1
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
    no_secret(monitor.snapshots());
}

#[tokio::test]
async fn concurrent_workers_share_original_guard_and_cannot_repeat_the_git_effect() {
    let repo = git::GitFixture::integration(false);
    repo.bare(&["config", "core.logAllRefUpdates", "true"]);
    let f = setup_local(&repo).await;
    f.prepared().await;
    let (first, second) = tokio::join!(
        start(&f, config(&repo, "one", true)),
        start(&f, config(&repo, "two", true))
    );
    tokio::time::timeout(Duration::from_secs(20), async {
        while f.guards().await != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    first.shutdown().await;
    second.shutdown().await;
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    assert_eq!(
        repo.bare(&["reflog", "show", "--format=%H", "refs/heads/main"])
            .lines()
            .count(),
        1
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn unchanged_unknown_queries_survive_restart_without_fact_or_confirmation_churn() {
    let repo = git::GitFixture::integration(false);
    let f = setup_local(&repo).await;
    let id = dispatched_without_effect(&f).await;
    let runtime = start(&f, config(&repo, "unknown", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.original_unchanged >= 2).await;
    runtime.shutdown().await;
    let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
    let requests = count(&f, "SELECT count(*) FROM awr_team.delivery_sync_requests").await;
    let before = f
        .store
        .inspect_integration(TENANT, PROJECT, WORKER, "a", &id)
        .await
        .unwrap();
    assert_eq!(before["state"], "unknown");
    assert_eq!(f.guards().await, 1);
    let runtime = start(&f, config(&repo, "unknown-restart", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.original_unchanged >= 2 && s.current_unchanged >= 2
    })
    .await;
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        facts
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_sync_requests").await,
        requests
    );
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    // An external application is observed; it is not permission for a second launch.
    repo.bare(&["update-ref", "refs/heads/main", &repo.source, &repo.base]);
    wait(&monitor, |s| s.confirmed == 1).await;
    runtime.shutdown().await;
    assert_eq!(f.guards().await, 0);
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn missing_current_selection_does_not_hide_dispatched_original_requests() {
    let repo = git::GitFixture::integration(false);
    let f = setup_local(&repo).await;
    let id = dispatched_without_effect(&f).await;
    // Negative state injection removes current admission; it supplies no delivery prerequisite.
    f.admin
        .batch_execute("DELETE FROM awr_team.delivery_selections WHERE work_id='a'")
        .await
        .unwrap();
    repo.bare(&["update-ref", "refs/heads/main", &repo.source, &repo.base]);
    let runtime = start(&f, config(&repo, "historical", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.historical_confirmations == 1 && s.selection_waits > 0
    })
    .await;
    runtime.shutdown().await;
    let view = f
        .store
        .inspect_integration(TENANT, PROJECT, WORKER, "a", &id)
        .await
        .unwrap();
    assert_eq!(view["state"], "confirmed");
    assert_eq!(view["confirmation_current"], false);
    assert_eq!(monitor.snapshots()[0].current_changed, 0);
    assert_eq!(f.guards().await, 0);
}

#[tokio::test]
async fn tiny_job_budget_visits_queue_tail_after_terminal_history() {
    let repo = git::GitFixture::integration(false);
    let f = setup_local(&repo).await;
    for index in 0..5 {
        let mut request = f.request.clone();
        request.request_id = format!("prepare-history-{index}");
        let prepared = f
            .store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, request)
            .await
            .unwrap();
        f.store
            .reject_prepared_integration(
                TENANT,
                PROJECT,
                SUPERVISOR,
                RejectPreparedDeliveryIntegration {
                    request_id: format!("reject-history-{index}"),
                    read_set: f.set.clone(),
                    integration_id: prepared["data"]["integration_id"].as_str().unwrap().into(),
                    reason: "Withdraw before dispatch".into(),
                },
            )
            .await
            .unwrap();
    }
    f.prepared().await;
    let mut cfg = config(&repo, "bounded-tail", true);
    cfg.workers[0].page_size = 4;
    cfg.workers[0].max_jobs_per_poll = 1;
    cfg.workers[0].max_pages_per_poll = 1;
    let runtime = start(&f, cfg).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.confirmed == 1 && s.terminal_intents_observed >= 5
    })
    .await;
    runtime.shutdown().await;
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
}

#[tokio::test]
async fn revocation_drop_and_explicit_credential_rotation_preserve_original_work() {
    let repo = git::GitFixture::integration(false);
    let f = setup_local(&repo).await;
    let runtime = start(&f, config(&repo, "revoked", false)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged > 0).await;
    f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-worker'").await.unwrap();
    wait(&monitor, |s| {
        s.failure_code == Some(LocalGitWorkerFailure::AuthorizationUnavailable)
    })
    .await;
    let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
    drop(runtime);
    wait(&monitor, |s| s.state == WorkerState::Stopped).await;
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        facts
    );
    let rotated =
        "awr1.integration-worker.1111111111111111111111111111111111111111111111111111111111111111";
    f.admin.execute("UPDATE awr_team.credentials SET secret_hash=$1,revoked_at=NULL WHERE id='integration-worker'", &[&workstream_credential_hash(rotated).unwrap()]).await.unwrap();
    assert!(
        LocalGitWorkerRuntime::start(
            &service(),
            Some(config(&repo, "old-secret", false)),
            DeliverySyncStore::from_config(f.config.clone()),
            |_| Some(WORKER.into())
        )
        .await
        .is_err()
    );
    let runtime = LocalGitWorkerRuntime::start(
        &service(),
        Some(config(&repo, "rotated", false)),
        DeliverySyncStore::from_config(f.config.clone()),
        |_| Some(rotated.into()),
    )
    .await
    .unwrap();
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged > 0).await;
    runtime.shutdown().await;
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_after_real_effect_and_restart_queries_original_without_another_launch() {
    use std::os::unix::fs::PermissionsExt;
    fn quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
    }
    let mut repo = git::GitFixture::integration(false);
    let marker = repo.root.join("actual-launches");
    let real = quote(&repo.config.git_executable);
    let wrapper = repo.root.join("fixed-test-git");
    std::fs::write(&wrapper, format!("#!/bin/sh\ncase \"$*\" in *'update-ref --no-deref'*)\n{real} \"$@\" || exit $?\nprintf x >> {}\nexec sleep 2;; esac\nexec {real} \"$@\"\n", quote(&marker))).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    repo.config.git_executable = wrapper;
    repo.config.command_timeout_ms = 1000;
    let f = setup_local(&repo).await;
    f.prepared().await;
    let runtime = start(&f, config(&repo, "cancel", true)).await;
    let monitor = runtime.monitor();
    tokio::time::timeout(Duration::from_secs(15), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    runtime.shutdown().await;
    wait(&monitor, |s| s.state == WorkerState::Stopped).await;
    assert_eq!(std::fs::read(&marker).unwrap(), b"x");
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    let runtime = start(&f, config(&repo, "recover-cancel", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.confirmed == 1).await;
    runtime.shutdown().await;
    assert_eq!(std::fs::read(&marker).unwrap(), b"x");
    assert_eq!(f.guards().await, 0);
}

#[tokio::test]
async fn damaged_original_proof_is_reobserved_without_redispatch_or_overwriting_archive() {
    let repo = git::GitFixture::integration(false);
    let f = setup_local(&repo).await;
    let id = dispatched_without_effect(&f).await;
    let integrator = LocalGitIntegrator::open(LocalGitIntegrationConfig {
        enabled: true,
        repository: repo.config.clone(),
    })
    .await
    .unwrap();
    let first = integrator
        .reconcile_original_current(&f.store, WORKER, &id)
        .await
        .unwrap();
    assert_eq!(first["unchanged"], false);
    let view = f
        .store
        .inspect_integration(TENANT, PROJECT, WORKER, "a", &id)
        .await
        .unwrap();
    let reference = view["confirmation"]["envelope"]["record"]["data"]["provenance"]["reference"]
        .as_str()
        .unwrap();
    let digest = reference.rsplit(':').next().unwrap();
    let path = repo
        .config
        .report_directory
        .join(format!("integration-report-{digest}.json"));
    std::fs::write(&path, b"damaged synthetic proof").unwrap();
    let observed = integrator
        .reconcile_original_current(&f.store, WORKER, &id)
        .await
        .unwrap();
    assert_eq!(observed["unchanged"], false);
    assert_eq!(std::fs::read(&path).unwrap(), b"damaged synthetic proof");
    let repeated = integrator
        .reconcile_original_current(
            &DeliverySyncStore::from_config(f.config.clone()),
            WORKER,
            &id,
        )
        .await
        .unwrap();
    assert_eq!(repeated["unchanged"], true);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn abandoned_original_observation_recovers_after_expiry_without_renewing_the_effect() {
    let repo = git::GitFixture::integration(false);
    let f = setup_local(&repo).await;
    let id = dispatched_without_effect(&f).await;
    let integrator = LocalGitIntegrator::open(LocalGitIntegrationConfig {
        enabled: true,
        repository: repo.config.clone(),
    })
    .await
    .unwrap();
    f.admin.batch_execute("CREATE FUNCTION awr_team.lose_original_observation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic ingestion interruption'; END $$;
        CREATE TRIGGER lose_original_observation BEFORE INSERT ON awr_team.delivery_inbox FOR EACH ROW EXECUTE FUNCTION awr_team.lose_original_observation()").await.unwrap();
    let before = count(&f, "SELECT count(*) FROM awr_team.delivery_inspections").await;
    assert!(
        integrator
            .reconcile_original_current(&f.store, WORKER, &id)
            .await
            .is_err()
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_inspections").await,
        before + 1
    );
    let abandoned: String = f
        .admin
        .query_one(
            "SELECT id FROM awr_team.delivery_inspections ORDER BY generation DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    // The failed reservation stays immutable; expire only its lease for recovery.
    f.admin.execute("UPDATE awr_team.delivery_inspections SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1", &[&abandoned]).await.unwrap();
    f.admin
        .batch_execute("DROP TRIGGER lose_original_observation ON awr_team.delivery_inbox")
        .await
        .unwrap();
    let restored = DeliverySyncStore::from_config(f.config.clone());
    let recovered = integrator
        .reconcile_original_current(&restored, WORKER, &id)
        .await
        .unwrap();
    assert_eq!(recovered["unchanged"], false);
    assert_eq!(recovered["integration"]["data"]["state"], "unknown");
    assert_ne!(
        recovered["observation"]["data"]["observation_receipt"]["inspection_id"],
        abandoned
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_inspections").await,
        before + 2
    );
    let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
    let repeated = integrator
        .reconcile_original_current(&restored, WORKER, &id)
        .await
        .unwrap();
    assert_eq!(repeated["unchanged"], true);
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        facts
    );
    assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_integration_intents WHERE dispatch_receipt_json IS NOT NULL").await, 1);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(f.guards().await, 1);
}
