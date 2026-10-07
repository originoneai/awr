#![cfg(feature = "pg-tests")]
//! Actual isolated PG/Git lifecycle regressions; provider metadata is synthetic.
//! These tests do not count as native member business acceptance.
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod access;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "fixtures/local_git.rs"]
mod git;
#[path = "fixtures/github.rs"]
mod github;
#[path = "../../awr-team-pg/tests/fixtures/delivery_integration.rs"]
mod integration_fixture;
#[path = "fixtures/github_receive.rs"]
mod receive;

use access::*;
use awr_server::{
    config::{GitHubWorkerConfig, GitHubWorkerSpec},
    delivery_adapter::*,
    service::{
        ProjectBinding, ServiceConfig,
        delivery_sync::WorkerState,
        delivery_workers::DeliveryWorkers,
        github_worker::{
            GitHubWorkerFailure, GitHubWorkerMonitor, GitHubWorkerRuntime, GitHubWorkerSnapshot,
            GitHubWorkerTransports,
        },
    },
};
use awr_team::delivery::*;
use awr_team_pg::*;
use integration_fixture::{SUPERVISOR, WORKER};
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};

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
fn config(repo: &receive::Fixture, name: &str, enabled: bool) -> GitHubWorkerConfig {
    let mut repository = repo.config.repository.clone();
    repository.request_timeout_ms = 1000;
    repository.inspection_timeout_ms = 3000;
    GitHubWorkerConfig {
        version: 1,
        workers: vec![GitHubWorkerSpec {
            project: "team".into(),
            worker_id: name.into(),
            credential_env: "AWR_SYNTHETIC_GITHUB_WORKER".into(),
            provider_credential_env: "GITHUB_SYNTHETIC_PROVIDER".into(),
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
async fn setup_github() -> (receive::Fixture, integration_fixture::Fixture) {
    let repo = receive::Fixture::new();
    let mut f = integration_fixture::setup_integration_with_candidate(Some(repo.candidate())).await;
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "github-worker-mapping".into(),
                read_set: f.set.clone(),
                expected_connector_version: "1".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "git".into(),
                    provider: "github".into(),
                    resource: repo.config.repository.resource.clone(),
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
    // Match the worker configuration digest so the first stable poll can reuse proof.
    GitHubAdapter::from_transport(
        config(&repo, "setup", false).workers[0].repository.clone(),
        repo.api.clone(),
    )
    .unwrap()
    .reconcile_current(&f.store, WORKER)
    .await
    .unwrap();
    (repo, f)
}
fn lookup(name: &str, token: &str) -> Option<String> {
    match name {
        "AWR_SYNTHETIC_GITHUB_WORKER" => Some(token.into()),
        "GITHUB_SYNTHETIC_PROVIDER" => Some("synthetic-provider-credential".into()),
        _ => None,
    }
}
fn transports(repo: &receive::Fixture) -> GitHubWorkerTransports {
    GitHubWorkerTransports {
        api: repo.api.clone(),
        receive: Some(repo.receive.clone()),
    }
}
async fn start(
    f: &integration_fixture::Fixture,
    repo: &receive::Fixture,
    c: GitHubWorkerConfig,
) -> GitHubWorkerRuntime {
    GitHubWorkerRuntime::start_with_transports(
        &service(),
        Some(c),
        DeliverySyncStore::from_config(f.config.clone()),
        |name| lookup(name, WORKER),
        |_, _| Ok(Some(transports(repo))),
    )
    .await
    .unwrap()
}
async fn wait(monitor: &GitHubWorkerMonitor, condition: impl Fn(&GitHubWorkerSnapshot) -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if monitor.snapshots().first().is_some_and(&condition) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("GitHub worker condition timed out");
}
async fn count(f: &integration_fixture::Fixture, sql: &str) -> i64 {
    f.admin.query_one(sql, &[]).await.unwrap().get(0)
}
fn no_secret(v: impl serde::Serialize) {
    let text = serde_json::to_string(&v).unwrap();
    for private in [
        WORKER,
        SUPERVISOR,
        A,
        "synthetic-provider-credential",
        "AWR_SYNTHETIC_GITHUB_WORKER",
        "GITHUB_SYNTHETIC_PROVIDER",
    ] {
        assert!(!text.contains(private));
    }
}
async fn no_completion(f: &integration_fixture::Fixture) {
    assert_eq!(
        count(f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn absent_and_empty_workers_never_resolve_credentials_provider_or_pg() {
    for c in [
        None,
        Some(GitHubWorkerConfig {
            version: 1,
            workers: vec![],
        }),
    ] {
        let runtime = GitHubWorkerRuntime::start_with_transports(
            &service(),
            c,
            DeliverySyncStore::new("unconfigured"),
            |_| panic!("absent credential lookup"),
            |_, _| panic!("absent transport lookup"),
        )
        .await
        .unwrap();
        assert!(runtime.monitor().snapshots().is_empty());
        runtime.shutdown().await;
    }
}
#[tokio::test]
async fn invalid_configuration_is_refused_before_any_credential_or_transport_lookup() {
    let repo = receive::Fixture::new();
    for mutate in [
        |c: &mut GitHubWorkerConfig| {
            c.workers[0].provider_credential_env = c.workers[0].credential_env.clone()
        },
        |c: &mut GitHubWorkerConfig| {
            c.workers[0].repository.api_base_url = "http://example.invalid".into()
        },
        |c: &mut GitHubWorkerConfig| c.workers[0].repository.project_id = "other".into(),
        |c: &mut GitHubWorkerConfig| c.workers[0].max_jobs_per_poll = 0,
    ] {
        let mut c = config(&repo, "invalid", false);
        mutate(&mut c);
        let r = GitHubWorkerRuntime::start_with_transports(
            &service(),
            Some(c),
            DeliverySyncStore::new("unconfigured"),
            |_| panic!("invalid credential lookup"),
            |_, _| panic!("invalid transport lookup"),
        )
        .await;
        assert!(r.is_err());
        no_secret(r.err().unwrap());
    }
    assert!(repo.api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn a_later_denied_worker_leaves_no_partial_loop_or_repository_effect() {
    let (repo, f) = setup_github().await;
    f.prepared().await;
    let mut c = config(&repo, "admitted", true);
    let mut denied = c.workers[0].clone();
    denied.worker_id = "denied".into();
    denied.repository.connector_id = "unmapped".into();
    c.workers.push(denied);
    let calls = repo.api.calls.lock().unwrap().len();
    let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
    let guards = f.guards().await;
    let r = GitHubWorkerRuntime::start_with_transports(
        &service(),
        Some(c),
        DeliverySyncStore::from_config(f.config.clone()),
        |name| lookup(name, WORKER),
        |_, _| Ok(Some(transports(&repo))),
    )
    .await;
    assert!(r.is_err());
    no_secret(r.err().unwrap());
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(repo.api.calls.lock().unwrap().len(), calls);
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        facts
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    assert_eq!(f.guards().await, guards);
    no_completion(&f).await;
}
#[tokio::test]
async fn observation_mode_preserves_approval_without_leasing_an_approved_intent() {
    let (repo, f) = setup_github().await;
    let id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let facts = f.store.inspect(TENANT, PROJECT, WORKER, "a").await.unwrap()["facts"].clone();
    let decisions = count(&f, "SELECT count(*) FROM awr_team.review_decisions").await;
    let runtime = start(&f, &repo, config(&repo, "observe", false)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged >= 2).await;
    runtime.shutdown().await;
    assert_eq!(
        f.store.inspect(TENANT, PROJECT, WORKER, "a").await.unwrap()["facts"],
        facts
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.review_decisions").await,
        decisions
    );
    assert_eq!(
        f.store
            .inspect_integration(TENANT, PROJECT, WORKER, "a", &id)
            .await
            .unwrap()["state"],
        "prepared"
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    assert_eq!(monitor.snapshots()[0].dispatch_attempts, 0);
    no_secret(monitor.snapshots());
    no_completion(&f).await;
}
#[tokio::test]
async fn prepared_integration_executes_once_and_restart_preserves_terminal_history() {
    let (repo, f) = setup_github().await;
    f.prepared().await;
    let runtime = start(&f, &repo, config(&repo, "integrate", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.confirmed == 1).await;
    runtime.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        repo.repo.bare(&["rev-parse", "refs/heads/main"]),
        repo.repo.source
    );
    assert_eq!(f.guards().await, 0);
    let runtime = start(&f, &repo, config(&repo, "restart", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.terminal_intents_observed >= 2 && s.current_unchanged > 0
    })
    .await;
    runtime.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    assert_eq!(monitor.snapshots()[0].dispatch_attempts, 0);
    no_completion(&f).await;
}
#[tokio::test]
async fn concurrent_runtimes_share_one_durable_guard_and_one_actual_receive_pack_post() {
    let (repo, f) = setup_github().await;
    f.prepared().await;
    let one = start(&f, &repo, config(&repo, "one", true)).await;
    let two = start(&f, &repo, config(&repo, "two", true)).await;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if one.monitor().snapshots()[0].confirmed + two.monitor().snapshots()[0].confirmed == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    one.shutdown().await;
    two.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    assert_eq!(f.guards().await, 0);
    no_completion(&f).await;
}
#[tokio::test]
async fn unknown_original_is_queried_even_when_current_selection_is_missing() {
    let (repo, f) = setup_github().await;
    let id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let lease = f.leased(&id).await;
    drop(f.dispatched(&id, lease["lease_id"].as_str().unwrap()).await);
    f.admin
        .batch_execute("DELETE FROM awr_team.delivery_selections WHERE work_id='a'")
        .await
        .unwrap();
    let runtime = start(&f, &repo, config(&repo, "unknown", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.unknown > 0 && s.original_unchanged > 0 && s.selection_waits > 0
    })
    .await;
    runtime.shutdown().await;
    let original = f
        .store
        .inspect_integration(TENANT, PROJECT, WORKER, "a", &id)
        .await
        .unwrap();
    let runtime = start(&f, &repo, config(&repo, "unknown-restart", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.original_unchanged > 0).await;
    runtime.shutdown().await;
    assert_eq!(
        f.store
            .inspect_integration(TENANT, PROJECT, WORKER, "a", &id)
            .await
            .unwrap()["confirmation"],
        original["confirmation"]
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    assert_eq!(f.guards().await, 1);
    no_completion(&f).await;
}
#[tokio::test]
async fn lost_provider_reply_confirms_real_bytes_without_a_second_post() {
    let (repo, f) = setup_github().await;
    f.prepared().await;
    repo.receive.lose_reply.store(true, Ordering::SeqCst);
    let runtime = start(&f, &repo, config(&repo, "lost", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.confirmed == 1).await;
    runtime.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    let runtime = start(&f, &repo, config(&repo, "lost-restart", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.terminal_intents_observed > 0).await;
    runtime.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    no_completion(&f).await;
}
#[tokio::test]
async fn bounded_pages_visit_queue_tail_after_terminal_history() {
    let (repo, f) = setup_github().await;
    for index in 0..3 {
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
    let mut c = config(&repo, "tail", true);
    c.workers[0].page_size = 4;
    c.workers[0].max_jobs_per_poll = 1;
    c.workers[0].max_pages_per_poll = 1;
    let runtime = start(&f, &repo, c).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.confirmed == 1 && s.terminal_intents_observed >= 3
    })
    .await;
    runtime.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn revocation_drop_and_explicit_rotation_require_new_authenticated_credentials() {
    let (repo, f) = setup_github().await;
    let runtime = start(&f, &repo, config(&repo, "revoked", false)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged > 0).await;
    f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-worker'").await.unwrap();
    wait(&monitor, |s| {
        s.failure_code == Some(GitHubWorkerFailure::AuthorizationUnavailable)
    })
    .await;
    let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
    drop(runtime);
    wait(&monitor, |s| s.state == WorkerState::Stopped).await;
    let rotated =
        "awr1.integration-worker.1111111111111111111111111111111111111111111111111111111111111111";
    f.admin.execute("UPDATE awr_team.credentials SET secret_hash=$1,revoked_at=NULL WHERE id='integration-worker'", &[&workstream_credential_hash(rotated).unwrap()]).await.unwrap();
    let old = GitHubWorkerRuntime::start_with_transports(
        &service(),
        Some(config(&repo, "old", false)),
        DeliverySyncStore::from_config(f.config.clone()),
        |name| lookup(name, WORKER),
        |_, _| Ok(Some(transports(&repo))),
    )
    .await;
    assert!(old.is_err());
    let runtime = GitHubWorkerRuntime::start_with_transports(
        &service(),
        Some(config(&repo, "rotated", false)),
        DeliverySyncStore::from_config(f.config.clone()),
        |name| lookup(name, rotated),
        |_, _| Ok(Some(transports(&repo))),
    )
    .await
    .unwrap();
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged > 0).await;
    runtime.shutdown().await;
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        facts
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    no_secret(monitor.snapshots());
}
#[tokio::test]
async fn provider_rate_limit_uses_bounded_backoff_and_recovers_without_fact_churn() {
    let (repo, f) = setup_github().await;
    let facts = f.store.inspect(TENANT, PROJECT, WORKER, "a").await.unwrap()["facts"].clone();
    repo.api
        .overrides
        .raw("", 429, b"synthetic-provider-credential".to_vec());
    let runtime = start(&f, &repo, config(&repo, "rate-limit", false)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.failure_code == Some(GitHubWorkerFailure::RateLimited) && s.failures >= 2
    })
    .await;
    assert!((200..=1000).contains(&monitor.snapshots()[0].retry_delay_ms));
    no_secret(monitor.snapshots());
    repo.api.overrides.0.lock().unwrap().replies.remove("");
    wait(&monitor, |s| {
        s.failure_code.is_none() && s.current_unchanged > 0
    })
    .await;
    runtime.shutdown().await;
    assert_eq!(
        f.store.inspect(TENANT, PROJECT, WORKER, "a").await.unwrap()["facts"],
        facts
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn changed_provider_check_invalidates_old_approval_and_stale_prepared_effect() {
    let (repo, f) = setup_github().await;
    f.prepared().await;
    let run = json!({"id":2,"head_sha":repo.repo.source,"name":"CI","app":{"id":9},"status":"completed","conclusion":"success"});
    repo.api.overrides.set(
        &format!(
            "/commits/{}/check-runs?filter=latest&per_page=100&page=1",
            repo.repo.source
        ),
        json!({"total_count":1,"check_runs":[run.clone()]}),
    );
    repo.api.overrides.set("/check-runs/2", run);
    let runtime = start(&f, &repo, config(&repo, "changed-check", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_changed > 0 && s.failures > 0).await;
    runtime.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    no_completion(&f).await;
}
#[tokio::test]
async fn cancelled_launched_post_is_recovered_by_original_query_after_restart() {
    let (repo, f) = setup_github().await;
    f.prepared().await;
    repo.receive.post_delay_ms.store(1500, Ordering::SeqCst);
    let runtime = start(&f, &repo, config(&repo, "cancel-post", true)).await;
    let monitor = runtime.monitor();
    tokio::time::timeout(Duration::from_secs(15), async {
        while repo.repo.bare(&["rev-parse", "refs/heads/main"]) != repo.repo.source {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(runtime);
    wait(&monitor, |s| s.state == WorkerState::Stopped).await;
    let runtime = start(&f, &repo, config(&repo, "recover-post", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.confirmed == 1).await;
    runtime.shutdown().await;
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    assert_eq!(f.guards().await, 0);
    no_completion(&f).await;
}
#[tokio::test]
async fn shared_http_shutdown_and_abort_stop_github_loops() {
    for abort in [false, true] {
        let (repo, f) = setup_github().await;
        let workers = DeliveryWorkers::start_with_github_transports(
            &service(),
            None,
            None,
            Some(config(&repo, "http", false)),
            DeliverySyncStore::from_config(f.config.clone()),
            |name| lookup(name, WORKER),
            |_, _| Ok(Some(transports(&repo))),
        )
        .await
        .unwrap();
        let monitor = workers.github_monitor();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = awr_server::service::router(
            service(),
            address,
            WorkstreamReadStore::from_config(f.config.clone()),
        )
        .unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(awr_server::service::serve_router_with_delivery_workers(
            listener,
            router,
            workers,
            async {
                let _ = stopped.await;
            },
        ));
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let body = json!({"protocol_version":1,"op":"delivery.schedule","work_id":"a","connector_id":"git","limit":1});
        let response = http
            .post(format!("http://{address}/v1/projects/team/query"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        wait(&monitor, |s| s.current_unchanged > 0).await;
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            stop.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        wait(&monitor, |s| s.state == WorkerState::Stopped).await;
        let polls = monitor.snapshots()[0].polls;
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(monitor.snapshots()[0].polls, polls);
        assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn live_pre_dispatch_lease_is_observed_until_proven_expired() {
    let (repo, f) = setup_github().await;
    let id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.leased(&id).await;
    let runtime = start(&f, &repo, config(&repo, "live-lease", true)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.live_leases_observed >= 2).await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    assert_eq!(monitor.snapshots()[0].dispatch_attempts, 0);
    // Actual DB expiry establishes the recovery precondition; no state or approval is forged.
    f.admin.execute("UPDATE awr_team.delivery_integration_intents SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1", &[&id]).await.unwrap();
    wait(&monitor, |s| s.confirmed == 1).await;
    runtime.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    no_completion(&f).await;
}
#[tokio::test]
async fn disabled_mapping_stops_provider_reads_and_cannot_be_reopened() {
    let (repo, f) = setup_github().await;
    let runtime = start(&f, &repo, config(&repo, "mapping", false)).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged >= 2).await;
    let mapping = DeliveryConnectorMapping {
        connector_id: "git".into(),
        provider: "github".into(),
        resource: repo.config.repository.resource.clone(),
        principal_actor_id: "integrator".into(),
        principal_client_id: "cli-worker".into(),
        fact_source: FactSource::AdapterObservation,
        enabled: false,
    };
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "disable-worker-mapping".into(),
                read_set: f.set.clone(),
                expected_connector_version: "2".into(),
                mapping,
            },
        )
        .await
        .unwrap();
    wait(&monitor, |s| {
        s.failure_code == Some(GitHubWorkerFailure::AuthorizationUnavailable)
    })
    .await;
    let calls = repo.api.calls.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(repo.api.calls.lock().unwrap().len(), calls);
    runtime.shutdown().await;
    let r = GitHubWorkerRuntime::start_with_transports(
        &service(),
        Some(config(&repo, "disabled", false)),
        DeliverySyncStore::from_config(f.config.clone()),
        |name| lookup(name, WORKER),
        |_, _| panic!("disabled mapping must refuse before transport lookup"),
    )
    .await;
    assert!(r.is_err());
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    no_completion(&f).await;
}
#[tokio::test]
async fn missing_or_invalid_provider_credentials_cannot_start_a_partially_admitted_group() {
    let (repo, f) = setup_github().await;
    let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
    let calls = repo.api.calls.lock().unwrap().len();
    for supplied in [
        None,
        Some(String::new()),
        Some("synthetic\ninvalid".into()),
        Some("x".repeat(4097)),
    ] {
        let r = GitHubWorkerRuntime::start_with_transports(
            &service(),
            Some(config(&repo, "provider", false)),
            DeliverySyncStore::from_config(f.config.clone()),
            |name| {
                if name == "AWR_SYNTHETIC_GITHUB_WORKER" {
                    Some(WORKER.into())
                } else {
                    supplied.clone()
                }
            },
            |_, _| panic!("invalid provider must refuse before transport lookup"),
        )
        .await;
        assert!(r.is_err());
        no_secret(r.err().unwrap());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        facts
    );
    assert_eq!(repo.api.calls.lock().unwrap().len(), calls);
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
}
