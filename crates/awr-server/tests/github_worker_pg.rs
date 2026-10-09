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
    config::{DeliveryWorkerConfig, DeliveryWorkerSpec, GitHubWorkerConfig, GitHubWorkerSpec},
    delivery_adapter::*,
    service::{
        ProjectBinding, ServiceConfig,
        delivery_sync::WorkerState,
        delivery_workers::DeliveryWorkers,
        github_webhook::{GitHubWebhookConfig, GitHubWebhookRuntime, GitHubWebhookSpec},
        github_worker::{
            GitHubWorkerFailure, GitHubWorkerMonitor, GitHubWorkerRuntime, GitHubWorkerSnapshot,
            GitHubWorkerTransports,
        },
    },
};
use awr_team::delivery::*;
use awr_team_pg::*;
use integration_fixture::{SUPERVISOR, WORKER};
use ring::hmac;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

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
    let integration_id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let one = start(&f, &repo, config(&repo, "one", true)).await;
    let two = start(&f, &repo, config(&repo, "two", true)).await;
    let observed = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            // Each runtime can observe the same durable confirmation. The real
            // receive-pack count below establishes uniqueness of the effect.
            if one.monitor().snapshots()[0].confirmed > 0
                || two.monitor().snapshots()[0].confirmed > 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "confirmation not observed: one={:?}, two={:?}, receive_pack_posts={}",
        one.monitor().snapshots(),
        two.monitor().snapshots(),
        repo.receive.posts.load(Ordering::SeqCst)
    );
    one.shutdown().await;
    two.shutdown().await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        repo.repo.bare(&["rev-parse", "refs/heads/main"]),
        repo.repo.source
    );
    let persisted = f
        .store
        .inspect_integration(TENANT, PROJECT, WORKER, "a", &integration_id)
        .await
        .unwrap();
    assert_eq!(persisted["state"], "confirmed");
    assert_eq!(persisted["confirmation_current"], true);
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

#[tokio::test]
async fn coalesced_wakeups_accelerate_fresh_queries_and_stop_immediately() {
    let (repo, f) = setup_github().await;
    let mut c = config(&repo, "wakeup", false);
    c.workers[0].poll_interval_ms = 5000;
    c.workers[0].max_backoff_ms = 10000;
    let runtime = start(&f, &repo, c).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged >= 1).await;
    let wakeup = monitor.wakeups().remove(0);
    assert_eq!(
        wakeup.scope().repository_id,
        repo.config.repository.repository_id
    );
    assert_eq!(wakeup.scope().connector_id, "git");
    let polls = monitor.snapshots()[0].polls;
    let begun = std::time::Instant::now();
    for _ in 0..100 {
        assert!(wakeup.notify());
    }
    wait(&monitor, |s| {
        s.current_unchanged >= 2 && s.wakeups_consumed >= 1
    })
    .await;
    assert!(begun.elapsed() < Duration::from_secs(4));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(monitor.snapshots()[0].polls, polls + 1);
    assert_eq!(monitor.snapshots()[0].wakeups_requested, 100);
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    runtime.request_stop();
    assert!(
        !wakeup.notify(),
        "stop must refuse a retained wake handle immediately"
    );
    runtime.shutdown().await;
    assert!(!wakeup.notify());
    no_completion(&f).await;
}

#[tokio::test]
async fn event_flood_cannot_shorten_provider_backoff_or_infer_delivery() {
    let (repo, f) = setup_github().await;
    repo.api
        .overrides
        .raw("", 429, br#"{"message":"Synthetic rate limit"}"#.to_vec());
    let mut c = config(&repo, "backoff-wakeup", false);
    c.workers[0].poll_interval_ms = 2000;
    c.workers[0].max_backoff_ms = 10000;
    let runtime = start(&f, &repo, c).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| {
        s.failure_code == Some(GitHubWorkerFailure::RateLimited)
    })
    .await;
    let wakeup = monitor.wakeups().remove(0);
    let before = monitor.snapshots()[0].polls;
    assert_eq!(monitor.snapshots()[0].retry_delay_ms, 4000);
    for _ in 0..100 {
        assert!(wakeup.notify());
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(monitor.snapshots()[0].polls, before);
    assert_eq!(monitor.snapshots()[0].wakeups_consumed, 0);
    repo.api.overrides.0.lock().unwrap().replies.remove("");
    wait(&monitor, |s| {
        s.failure_code.is_none() && s.current_unchanged >= 1
    })
    .await;
    assert_eq!(monitor.snapshots()[0].polls, before + 1);
    runtime.shutdown().await;
    assert!(!wakeup.notify());
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    no_completion(&f).await;
}

const NOTIFICATION_KEY: &str = "synthetic-github-notification-key-for-tests";
const ROTATED_KEY: &str = "rotated-synthetic-github-notification-key-for-tests";

/// Real HTTP, with an admitted monitor. Abort on unwinding as well as normal
/// shutdown so a failed assertion cannot leave a fixture listener behind.
struct NotificationHttp {
    url: String,
    client: reqwest::Client,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}
impl NotificationHttp {
    async fn start(config: &GitHubWorkerConfig, monitor: &GitHubWorkerMonitor, key: &str) -> Self {
        let listener = tokio::net::TcpListener::bind(service().listen)
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let notifications = GitHubWebhookRuntime::prepare(
            &service(),
            address,
            Some(GitHubWebhookConfig {
                version: 1,
                max_body_bytes: 65536,
                hooks: vec![GitHubWebhookSpec {
                    worker_id: config.workers[0].worker_id.clone(),
                    secret_env: "SYNTHETIC_GITHUB_NOTIFICATION_SECRET".into(),
                }],
            }),
            Some(config),
            |_| Some(key.into()),
        )
        .unwrap();
        notifications.connect(monitor);
        let router = notifications.router();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            url: format!(
                "http://{address}/v1/hooks/github/{}",
                config.workers[0].worker_id
            ),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            stop: Some(stop),
            task,
        }
    }
    fn request(&self, body: &[u8], delivery: &str, key: &str) -> reqwest::RequestBuilder {
        let signature = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes()), body);
        let hex: String = signature
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("x-github-event", "push")
            .header("x-github-delivery", delivery)
            .header("x-hub-signature-256", format!("sha256={hex}"))
            .body(body.to_vec())
    }
    async fn send(&self, body: &[u8], delivery: &str, key: &str, status: u16) -> Value {
        let response = self.request(body, delivery, key).send().await.unwrap();
        assert_eq!(response.status().as_u16(), status);
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["authoritative_fact"], false);
        no_secret(&value);
        assert!(!value.to_string().contains(key));
        value
    }
    async fn close(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), &mut self.task)
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for NotificationHttp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn notification_body(repo: &receive::Fixture) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"repository":{"id":repo.config.repository.repository_id},
        "approved":true,"completed":true,"after":"payload-is-not-a-revision"}),
    )
    .unwrap()
}

#[tokio::test]
async fn signed_http_duplicates_and_disorder_only_wake_the_exact_admitted_scope() {
    let (repo, f) = setup_github().await;
    let mut c = config(&repo, "http-wakeup", false);
    c.workers[0].poll_interval_ms = 5000;
    c.workers[0].max_backoff_ms = 10000;
    let runtime = start(&f, &repo, c.clone()).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged > 0).await;
    let before = f.store.inspect(TENANT, PROJECT, WORKER, "a").await.unwrap()["facts"].clone();
    let mut mismatched = c.clone();
    mismatched.workers[0].repository.work_id = "another-work".into();
    let mut unrelated = NotificationHttp::start(&mismatched, &monitor, NOTIFICATION_KEY).await;
    let body = notification_body(&repo);
    assert_eq!(
        unrelated
            .send(&body, "scope-mismatch", NOTIFICATION_KEY, 503)
            .await["code"],
        "WorkerUnavailable"
    );
    assert_eq!(monitor.snapshots()[0].wakeups_requested, 0);
    unrelated.close().await;
    let mut http = NotificationHttp::start(&c, &monitor, NOTIFICATION_KEY).await;
    let begun = Instant::now();
    let polls = monitor.snapshots()[0].polls;
    let mut concurrent = Vec::new();
    for _ in 0..16 {
        let request = http.request(&body, "newer-delivery", NOTIFICATION_KEY);
        concurrent.push(tokio::spawn(async move {
            let response = request.send().await.unwrap();
            assert_eq!(response.status().as_u16(), 202);
            response.json::<Value>().await.unwrap()
        }));
    }
    let mut actual_wakes = 0;
    for task in concurrent {
        let response = task.await.unwrap();
        assert_eq!(response["authoritative_fact"], false);
        if response["code"] == "QueryWakeupRequested" {
            actual_wakes += 1;
        } else {
            assert_eq!(response["code"], "NotificationCoalesced");
        }
    }
    assert_eq!(actual_wakes, 1);
    // An older delivery arrives later. Its false completion fields have no authority.
    http.send(&body, "older-delivery", NOTIFICATION_KEY, 202)
        .await;
    assert_eq!(monitor.snapshots()[0].wakeups_requested, 2);
    wait(&monitor, |s| {
        s.current_unchanged >= 2 && s.wakeups_consumed > 0
    })
    .await;
    assert!(begun.elapsed() < Duration::from_secs(4));
    assert_eq!(monitor.snapshots()[0].polls, polls + 1);
    assert_eq!(
        f.store.inspect(TENANT, PROJECT, WORKER, "a").await.unwrap()["facts"],
        before
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_integration_intents"
        )
        .await,
        0
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    no_completion(&f).await;
    runtime.request_stop();
    assert_eq!(
        http.send(&body, "after-stop", NOTIFICATION_KEY, 503).await["code"],
        "WorkerUnavailable"
    );
    runtime.shutdown().await;
    http.close().await;
}

#[tokio::test]
async fn signed_http_notifications_cannot_restore_revoked_credentials_or_disabled_mapping() {
    for revoke in [false, true] {
        let (repo, f) = setup_github().await;
        let mut c = config(&repo, "http-authorization", false);
        c.workers[0].poll_interval_ms = 5000;
        c.workers[0].max_backoff_ms = 10000;
        let runtime = start(&f, &repo, c.clone()).await;
        let monitor = runtime.monitor();
        wait(&monitor, |s| s.current_unchanged > 0).await;
        let mut http = NotificationHttp::start(&c, &monitor, NOTIFICATION_KEY).await;
        if revoke {
            f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-worker'").await.unwrap();
        } else {
            f.store
                .configure_connector(
                    TENANT,
                    PROJECT,
                    WORKER,
                    ConfigureDeliveryConnector {
                        request_id: "disable-http-worker".into(),
                        read_set: f.set.clone(),
                        expected_connector_version: "2".into(),
                        mapping: DeliveryConnectorMapping {
                            connector_id: "git".into(),
                            provider: "github".into(),
                            resource: repo.config.repository.resource.clone(),
                            principal_actor_id: "integrator".into(),
                            principal_client_id: "cli-worker".into(),
                            fact_source: FactSource::AdapterObservation,
                            enabled: false,
                        },
                    },
                )
                .await
                .unwrap();
        }
        let calls = repo.api.calls.lock().unwrap().len();
        let facts = count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await;
        // Authentication of the notification is separate from current worker authority.
        http.send(
            &notification_body(&repo),
            "after-revocation",
            NOTIFICATION_KEY,
            202,
        )
        .await;
        wait(&monitor, |s| {
            s.failure_code == Some(GitHubWorkerFailure::AuthorizationUnavailable)
        })
        .await;
        assert_eq!(repo.api.calls.lock().unwrap().len(), calls);
        assert_eq!(
            count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
            facts
        );
        assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
        no_completion(&f).await;
        runtime.shutdown().await;
        http.close().await;
    }
}

#[tokio::test]
async fn signed_http_restart_rotates_secrets_without_reusing_old_wake_handles() {
    let (repo, f) = setup_github().await;
    let mut c = config(&repo, "http-rotation", false);
    c.workers[0].poll_interval_ms = 5000;
    c.workers[0].max_backoff_ms = 10000;
    let runtime = start(&f, &repo, c.clone()).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged > 0).await;
    let old_handle = monitor.wakeups().remove(0);
    let mut old_http = NotificationHttp::start(&c, &monitor, NOTIFICATION_KEY).await;
    runtime.shutdown().await;
    assert!(!old_handle.notify());
    old_http
        .send(
            &notification_body(&repo),
            "stopped-old-route",
            NOTIFICATION_KEY,
            503,
        )
        .await;
    old_http.close().await;
    let runtime = start(&f, &repo, c.clone()).await;
    let monitor = runtime.monitor();
    wait(&monitor, |s| s.current_unchanged > 0).await;
    let mut http = NotificationHttp::start(&c, &monitor, ROTATED_KEY).await;
    http.send(
        &notification_body(&repo),
        "old-signature",
        NOTIFICATION_KEY,
        403,
    )
    .await;
    assert_eq!(monitor.snapshots()[0].wakeups_requested, 0);
    http.send(&notification_body(&repo), "new-signature", ROTATED_KEY, 202)
        .await;
    wait(&monitor, |s| {
        s.current_unchanged >= 2 && s.wakeups_consumed > 0
    })
    .await;
    assert!(!old_handle.notify());
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    no_completion(&f).await;
    runtime.shutdown().await;
    http.close().await;
}

fn source_worker(name: &str) -> DeliveryWorkerConfig {
    DeliveryWorkerConfig {
        version: 1,
        workers: vec![DeliveryWorkerSpec {
            project: "team".into(),
            worker_id: format!("source-{name}"),
            workstreams: vec![awr_core::Id::from(1)],
            credential_env: "AWR_SYNTHETIC_GITHUB_WORKER".into(),
            poll_interval_ms: 100,
            lease_seconds: 5,
            operation_timeout_ms: 2000,
            max_backoff_ms: 1000,
            page_size: 4,
            max_pages_per_poll: 2,
            max_jobs_per_poll: 4,
        }],
    }
}

fn snapshot_contention(error: &PgError) -> bool {
    matches!(error, PgError::Db(error) if error.code().is_some_and(|c| c.code() == "40001" || c.code() == "40P01"))
}

async fn synchronized_source(f: &integration_fixture::Fixture, root: &Path) -> bool {
    let status = match f
        .store
        .source_publication_status(TENANT, PROJECT, SUPERVISOR, "a")
        .await
    {
        Ok(status) => status,
        Err(error) if snapshot_contention(&error) => return false,
        Err(error) => panic!("physical source status failed: {error}"),
    };
    let bytes = fs::read(root.join("ledger.yaml")).unwrap();
    [
        "source_synchronized",
        "projection_current",
        "cursor_current",
    ]
    .iter()
    .all(|key| status[key] == true)
        && status["source_fingerprint"] == awr_source::fingerprint(&bytes)
        && status["confirmed_fingerprint"] == status["source_fingerprint"]
}

async fn actual_target_observed(f: &integration_fixture::Fixture, repo: &receive::Fixture) -> bool {
    let state = match f.store.inspect(TENANT, PROJECT, WORKER, "a").await {
        Ok(state) => state,
        Err(error) if snapshot_contention(&error) => return false,
        Err(error) => panic!("physical target observation failed: {error}"),
    };
    state["facts"].as_array().unwrap().iter().any(|fact| {
        let observation = &fact["observation"];
        fact["current"] == true
            && observation["kind"] == "integration_observation"
            && observation["outcome"] == "applied"
            && observation["result_revision"]["value"] == repo.repo.source
            && observation["provenance"]["reference"]
                .as_str()
                .is_some_and(|reference| reference.starts_with("awr-github-report:"))
    })
}

async fn wait_physical(mut condition: impl AsyncFnMut() -> bool, seconds: u64) -> bool {
    tokio::time::timeout(Duration::from_secs(seconds), async {
        while !condition().await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .is_ok()
}

/// Each sample creates independent real repository/source bytes and a fresh
/// isolated PG fixture. Provider identity and CI metadata remain synthetic.
/// Read-only diagnostics observe closure; they never perform recovery/writeback.
async fn physical_source_sample(
    event: bool,
    sample: usize,
    roots: &mut std::collections::BTreeSet<std::path::PathBuf>,
) -> Value {
    let repo = receive::Fixture::new();
    let root = repo.repo.source_contract();
    assert!(
        roots.insert(root.clone()),
        "samples must not reuse physical sources"
    );
    let path = root.join("ledger.yaml");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("[local_git.manifest]", "[github.manifest]");
    fs::write(&path, text).unwrap();
    let f = integration_fixture::setup_source_integration_with_checks(
        repo.candidate(),
        &root,
        &["github.manifest".into()],
    )
    .await;
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "physical-github-mapping".into(),
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
    let name = format!("physical-{}-{sample}", if event { "event" } else { "poll" });
    let mut c = config(&repo, &name, false);
    c.workers[0].poll_interval_ms = 20000;
    c.workers[0].max_backoff_ms = 60000;
    let workers = DeliveryWorkers::start_with_github_transports(
        &service(),
        Some(source_worker(&name)),
        None,
        Some(c.clone()),
        DeliverySyncStore::from_config(f.config.clone()),
        |name| lookup(name, WORKER),
        |_, _| Ok(Some(transports(&repo))),
    )
    .await
    .unwrap();
    let monitor = workers.github_monitor();
    let mut http = NotificationHttp::start(&c, &monitor, NOTIFICATION_KEY).await;
    assert!(
        wait_physical(
            async || {
                monitor.snapshots()[0].current_changed > 0 && synchronized_source(&f, &root).await
            },
            25
        )
        .await,
        "baseline must be physically synchronized before measurement"
    );
    assert_eq!(
        repo.repo.bare(&["rev-parse", "refs/heads/main"]),
        repo.repo.base
    );
    let before = fs::read(&path).unwrap();
    let changed = Instant::now();
    // External repository effect, outside AWR. The observer must discover it.
    repo.repo.bare(&[
        "update-ref",
        "refs/heads/main",
        &repo.repo.source,
        &repo.repo.base,
    ]);
    let notified = if event {
        let at = Instant::now();
        http.send(
            &notification_body(&repo),
            "physical-delivery",
            NOTIFICATION_KEY,
            202,
        )
        .await;
        Some(at)
    } else {
        None
    };
    let target_seconds = if event { 10 } else { 60 };
    let closed = wait_physical(
        async || {
            fs::read(&path).unwrap() != before
                && actual_target_observed(&f, &repo).await
                && synchronized_source(&f, &root).await
        },
        target_seconds + 5,
    )
    .await;
    let repository_to_source_ms = changed.elapsed().as_secs_f64() * 1000.0;
    let notification_to_source_ms = notified.map(|at| at.elapsed().as_secs_f64() * 1000.0);
    workers.shutdown().await;
    http.close().await;
    let after = fs::read(&path).unwrap();
    assert_eq!(
        std::str::from_utf8(&after)
            .unwrap()
            .split_once("  - id: c\n")
            .unwrap()
            .1,
        std::str::from_utf8(&before)
            .unwrap()
            .split_once("  - id: c\n")
            .unwrap()
            .1
    );
    assert!(after.starts_with(b"# Preserve operator comments and unrelated work.\n"));
    let work = std::str::from_utf8(&after)
        .unwrap()
        .split_once("  - id: a\n")
        .unwrap()
        .1
        .split_once("  - id: c\n")
        .unwrap()
        .0;
    assert!(
        work.contains("    status: planned\n"),
        "observation must not finalize acceptance"
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_integration_intents"
        )
        .await,
        0
    );
    no_completion(&f).await;
    let result = json!({
        "sample":sample, "mode":if event { "event" } else { "poll" },
        "closed":closed, "target_seconds":target_seconds,
        "exceeded":!closed || repository_to_source_ms > (target_seconds * 1000) as f64,
        "repository_to_source_ms":repository_to_source_ms,
        "notification_to_source_ms":notification_to_source_ms,
        "github_poll_interval_ms":20000, "source_poll_interval_ms":100,
        "worker":monitor.snapshots()[0], "repository_effect_posts":0,
        "completion_receipts":0, "native_acceptance_credit":0,
    });
    println!("PHYSICAL_SOURCE_SAMPLE {result}");
    result
}

fn timing_distribution(samples: &[Value], mode: &str) -> Value {
    let chosen: Vec<_> = samples
        .iter()
        .filter(|sample| sample["mode"] == mode)
        .collect();
    let mut times: Vec<_> = chosen
        .iter()
        .map(|sample| sample["repository_to_source_ms"].as_f64().unwrap())
        .collect();
    times.sort_by(f64::total_cmp);
    let n = times.len();
    let median = if n % 2 == 0 {
        (times[n / 2 - 1] + times[n / 2]) / 2.0
    } else {
        times[n / 2]
    };
    json!({"samples":n, "failed":chosen.iter().filter(|sample| sample["closed"] != true).count(),
        "exceedances":chosen.iter().filter(|sample| sample["exceeded"] == true).count(),
        "min_ms":times[0],"median_ms":median,"p95_ms":times[(95 * n).div_ceil(100) - 1],
        "max_ms":times[n-1],"p95_method":"nearest_rank"})
}

#[tokio::test]
async fn signed_event_and_missed_event_polling_confirm_physical_source_with_measured_distributions()
{
    let mut samples = Vec::new();
    let mut roots = std::collections::BTreeSet::new();
    for event in [true, false] {
        for sample in 1..=3 {
            samples.push(physical_source_sample(event, sample, &mut roots).await);
        }
    }
    let report = json!({
        "scope":"isolated_real_http_pg_git_source_with_synthetic_provider_metadata",
        "production_sla_claim":false, "native_acceptance_credit":0,
        "event":timing_distribution(&samples,"event"),
        "poll":timing_distribution(&samples,"poll"),
        "samples":samples,
    });
    println!("PHYSICAL_SOURCE_DISTRIBUTION {report}");
    assert!(
        samples
            .iter()
            .all(|sample| sample["closed"] == true && sample["exceeded"] == false),
        "retain failures/exceedances; these small isolated distributions are not a production SLA"
    );
}
