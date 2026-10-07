#![cfg(feature = "pg-tests")]
//! Actual isolated source/PG admission and synthetic provider transport boundaries.
#[path = "../../awr-team-pg/tests/fixtures/delivery_acceptance.rs"]
mod acceptance_fixture;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/local_git.rs"]
mod git;
#[path = "fixtures/github.rs"]
mod github;
#[path = "../../awr-team-pg/tests/fixtures/delivery_integration.rs"]
mod integration_fixture;
#[path = "../../awr-team-pg/tests/fixtures/delivery_publication.rs"]
mod publication;

use awr_server::{
    config::{
        DeliveryWorkerConfig, GitHubWorkerConfig, GitHubWorkerSpec, LocalGitWorkerConfig,
        LocalGitWorkerSpec,
    },
    service::{ProjectBinding, ServiceConfig, delivery_workers::DeliveryWorkers},
};
use awr_team::delivery::*;
use awr_team_pg::*;
use fixture::*;
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
            key: "one".into(),
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
        }],
    }
}
fn source() -> DeliveryWorkerConfig {
    toml::from_str(
        r#"version=1
[[workers]]
project="one"
worker_id="source"
workstreams=["00000000000000000000000001"]
credential_env="SYNTHETIC_SOURCE_CREDENTIAL"
poll_interval_ms=100
lease_seconds=10
operation_timeout_ms=5000
max_backoff_ms=1000
page_size=1
max_pages_per_poll=1
max_jobs_per_poll=1
"#,
    )
    .unwrap()
}
fn github_config(repo: &github::Fixture, connector: &str) -> GitHubWorkerConfig {
    let mut repository = repo.config.clone();
    repository.tenant_id = TENANT.into();
    repository.project_id = PROJECT.into();
    repository.connector_id = connector.into();
    repository.inspection_timeout_ms = 3000;
    GitHubWorkerConfig {
        version: 1,
        workers: vec![GitHubWorkerSpec {
            project: "one".into(),
            worker_id: "github".into(),
            credential_env: "SYNTHETIC_REPOSITORY_CREDENTIAL".into(),
            provider_credential_env: "SYNTHETIC_PROVIDER_CREDENTIAL".into(),
            integration_enabled: false,
            repository,
            poll_interval_ms: 100,
            lease_seconds: 10,
            operation_timeout_ms: 5000,
            max_backoff_ms: 1000,
            page_size: 1,
            max_pages_per_poll: 1,
            max_jobs_per_poll: 1,
        }],
    }
}
fn local_config(repo: &git::GitFixture, connector: &str) -> LocalGitWorkerConfig {
    let mut repository = repo.config.clone();
    repository.connector_id = connector.into();
    repository.inspection_timeout_ms = 3000;
    LocalGitWorkerConfig {
        version: 1,
        workers: vec![LocalGitWorkerSpec {
            project: "one".into(),
            worker_id: "local".into(),
            credential_env: "SYNTHETIC_REPOSITORY_CREDENTIAL".into(),
            integration_enabled: false,
            repository,
            poll_interval_ms: 100,
            lease_seconds: 10,
            operation_timeout_ms: 5000,
            max_backoff_ms: 1000,
            page_size: 1,
            max_pages_per_poll: 1,
            max_jobs_per_poll: 1,
        }],
    }
}
async fn count(f: &publication::Fixture, sql: &str) -> i64 {
    f.admin.query_one(sql, &[]).await.unwrap().get(0)
}
fn no_secret(value: impl serde::Serialize) {
    let value = serde_json::to_string(&value).unwrap();
    for secret in [A, B, "synthetic-provider-credential"] {
        assert!(!value.contains(secret));
    }
}

#[tokio::test]
async fn configured_source_worker_confirms_actual_acceptance_without_repository_observers() {
    for simulated in [false, true] {
        let f = acceptance_fixture::SourceFixture::new(simulated).await;
        let completed = f.finalize("worker-acceptance").await;
        let runtime = DeliveryWorkers::start(
            &service(),
            Some(source()),
            None,
            DeliverySyncStore::from_config(f.f.config.clone()),
            |_| Some(integration_fixture::WORKER.into()),
        )
        .await
        .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let count: i64 = f
                .f
                .admin
                .query_one(
                    "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE state='succeeded'",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            if count == 2 && runtime.source_monitor().snapshots()[0].synchronized == 2 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Configured source worker did not confirm the real receipt"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let source_monitor = runtime.source_monitor();
        let git_monitor = runtime.git_monitor();
        let github_monitor = runtime.github_monitor();
        runtime.shutdown().await;
        let rows = f.queue().await;
        assert!(
            rows["intents"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| i["origin"] == "domain_acceptance"
                    && i["completion_receipt_id"] == completed["receipt_id"])
        );
        assert!(git_monitor.snapshots().is_empty());
        assert!(github_monitor.snapshots().is_empty());
        let snapshots = source_monitor.snapshots();
        assert_eq!(snapshots[0].synchronized, 2);
        no_secret(&snapshots);
        let text = std::fs::read_to_string(f.root.0.join("ledger.yaml")).unwrap();
        assert!(text.contains(completed["receipt_id"].as_str().unwrap()));
        assert!(text.starts_with("# Preserve this comment"));
        assert_eq!(
            f.f.admin
                .query_one("SELECT count(*) FROM awr_team.delivery_inbox", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
    }
}

#[tokio::test]
async fn absent_and_empty_three_groups_preserve_legacy_and_source_only_entries() {
    for empty in [false, true] {
        let runtime = DeliveryWorkers::start_with_github_transports(
            &service(),
            empty.then_some(DeliveryWorkerConfig {
                version: 1,
                workers: vec![],
            }),
            empty.then_some(LocalGitWorkerConfig {
                version: 1,
                workers: vec![],
            }),
            empty.then_some(GitHubWorkerConfig {
                version: 1,
                workers: vec![],
            }),
            DeliverySyncStore::new("unconfigured"),
            |_| panic!("empty credential lookup"),
            |_, _| panic!("empty transport lookup"),
        )
        .await
        .unwrap();
        assert!(runtime.source_monitor().snapshots().is_empty());
        assert!(runtime.git_monitor().snapshots().is_empty());
        assert!(runtime.github_monitor().snapshots().is_empty());
        runtime.shutdown().await;
    }
    let runtime = DeliveryWorkers::start(
        &service(),
        None,
        None,
        DeliverySyncStore::new("unconfigured"),
        |_| panic!("legacy empty credential lookup"),
    )
    .await
    .unwrap();
    runtime.shutdown().await;
    let source = awr_server::service::delivery_sync::DeliveryWorkerRuntime::start(
        &service(),
        None,
        DeliverySyncStore::new("unconfigured"),
        |_| panic!("source empty lookup"),
    )
    .await
    .unwrap();
    let combined = DeliveryWorkers::from(source);
    assert!(combined.github_monitor().snapshots().is_empty());
    combined.shutdown().await;
}
#[tokio::test]
async fn all_groups_are_validated_before_any_credentials_and_duplicate_observers_are_refused() {
    let provider = github::Fixture::new();
    let repo = git::GitFixture::integration(false);
    let mut bad = github_config(&provider, "github");
    bad.workers[0].max_jobs_per_poll = 0;
    let r = DeliveryWorkers::start_with_github_transports(
        &service(),
        Some(source()),
        Some(local_config(&repo, "local")),
        Some(bad),
        DeliverySyncStore::new("unconfigured"),
        |_| panic!("invalid group credential lookup"),
        |_, _| panic!("invalid group transport lookup"),
    )
    .await;
    assert!(r.is_err());
    let r = DeliveryWorkers::start_with_github_transports(
        &service(),
        Some(source()),
        Some(local_config(&repo, "duplicate")),
        Some(github_config(&provider, "duplicate")),
        DeliverySyncStore::new("unconfigured"),
        |_| panic!("duplicate credential lookup"),
        |_, _| panic!("duplicate transport lookup"),
    )
    .await;
    assert_eq!(r.err().unwrap(), "duplicate delivery observer scope");
    assert_eq!(provider.api.calls(), 0);
}
#[tokio::test]
async fn denied_github_group_cannot_start_an_already_admitted_physical_source_publisher() {
    let f = publication::setup_publisher().await;
    let provider = github::Fixture::new();
    let before = f.bytes();
    let mut lookups = Vec::new();
    let r = DeliveryWorkers::start_with_github_transports(
        &service(),
        Some(source()),
        None,
        Some(github_config(&provider, "unmapped")),
        f.restarted(),
        |name| {
            lookups.push(name.to_owned());
            Some(
                if name == "SYNTHETIC_SOURCE_CREDENTIAL" {
                    A
                } else {
                    B
                }
                .into(),
            )
        },
        |_, _| panic!("denied scope transport lookup"),
    )
    .await;
    assert!(r.is_err());
    no_secret(r.err().unwrap());
    assert_eq!(
        lookups,
        [
            "SYNTHETIC_SOURCE_CREDENTIAL",
            "SYNTHETIC_REPOSITORY_CREDENTIAL"
        ]
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(f.bytes(), before);
    assert_eq!(
        count(
            &f,
            "SELECT sum(attempts)::bigint FROM awr_team.delivery_sync_intents"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        0
    );
    assert_eq!(provider.api.calls(), 0);
}

#[tokio::test]
async fn last_group_denial_preserves_both_admitted_source_and_local_git_groups() {
    let f = publication::setup_publisher().await;
    let repo = git::GitFixture::integration(false);
    let provider = github::Fixture::new();
    let credential =
        "awr1.combined-worker.9999999999999999999999999999999999999999999999999999999999999999";
    // Explicit synthetic identity infrastructure. Startup itself creates no authority.
    f.admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
        ('reader-tenant','combined-system','system','Synthetic combined worker','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','reader-project','combined-system','admin');",
        )
        .await
        .unwrap();
    f.admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
        VALUES($1,'combined-worker','combined-system','combined-client',$2)",
            &[&TENANT, &workstream_credential_hash(credential).unwrap()],
        )
        .await
        .unwrap();
    f.admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage)
        VALUES($1,$2,'combined-system','combined-client',$3,2,true,true,true)",
        &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            credential,
            ConfigureDeliveryConnector {
                request_id: "combined-local-mapping".into(),
                read_set: f.set.clone(),
                expected_connector_version: "0".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "local".into(),
                    provider: "local_git".into(),
                    resource: repo.config.resource.clone(),
                    principal_actor_id: "combined-system".into(),
                    principal_client_id: "combined-client".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let before = f.bytes();
    let mut lookups = Vec::new();
    let r = DeliveryWorkers::start_with_github_transports(
        &service(),
        Some(source()),
        Some(local_config(&repo, "local")),
        Some(github_config(&provider, "unmapped")),
        f.restarted(),
        |name| {
            lookups.push(name.to_owned());
            Some(
                if name == "SYNTHETIC_SOURCE_CREDENTIAL" {
                    A
                } else {
                    credential
                }
                .into(),
            )
        },
        |_, _| panic!("denied last group transport lookup"),
    )
    .await;
    assert!(r.is_err());
    no_secret(r.err().unwrap());
    assert_eq!(
        lookups,
        [
            "SYNTHETIC_SOURCE_CREDENTIAL",
            "SYNTHETIC_REPOSITORY_CREDENTIAL",
            "SYNTHETIC_REPOSITORY_CREDENTIAL"
        ]
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(f.bytes(), before);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(
        count(
            &f,
            "SELECT sum(attempts)::bigint FROM awr_team.delivery_sync_intents"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        0
    );
    assert_eq!(provider.api.calls(), 0);
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.credentials WHERE id='combined-worker'"
        )
        .await,
        1
    );
}
