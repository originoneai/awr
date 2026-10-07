#![cfg(feature = "pg-tests")]
//! Actual Git effects/PG authority; provider API metadata is synthetic.
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
use awr_server::delivery_adapter::{github_integration::GitHubAttemptOutcome, *};
use awr_team::delivery::*;
use awr_team_pg::*;
use integration_fixture::WORKER;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

async fn setup_github() -> (
    receive::Fixture,
    integration_fixture::Fixture,
    GitHubIntegrator,
) {
    let repo = receive::Fixture::new();
    let mut f = integration_fixture::setup_integration_with_candidate(Some(repo.candidate())).await;
    // Identity infrastructure maps the existing worker through the public
    // authenticated domain command. No approval or check is fabricated here.
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "github-mapping".into(),
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
    GitHubAdapter::from_transport(repo.config.repository.clone(), repo.api.clone())
        .unwrap()
        .reconcile(
            &f.store,
            WORKER,
            GitHubPollRequest {
                request_id: "github-current-checks".into(),
                read_set: f.set.clone(),
                connector_version: "2".into(),
            },
        )
        .await
        .unwrap();
    let integrator = repo.integrator();
    (repo, f, integrator)
}

async fn ready(f: &integration_fixture::Fixture) -> (String, String) {
    let value = f.prepared().await;
    let id = value["integration_id"].as_str().unwrap().to_owned();
    let value = f.leased(&id).await;
    (id, value["lease_id"].as_str().unwrap().into())
}
fn poll(f: &integration_fixture::Fixture, id: &str, key: &str) -> GitHubIntegrationPollRequest {
    GitHubIntegrationPollRequest {
        request_id: key.into(),
        integration_id: id.into(),
        read_set: f.set.clone(),
        connector_version: "2".into(),
    }
}
async fn no_completion(f: &integration_fixture::Fixture) {
    let count: i64 = f
        .admin
        .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
}

#[tokio::test]
async fn real_guarded_receive_pack_is_version_bound_and_confirmed_separately_from_acceptance() {
    let (repo, f, integrator) = setup_github().await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
        .await
        .unwrap();
    assert_eq!(
        repo.repo.bare(&["rev-parse", "refs/heads/main"]),
        repo.repo.source
    );
    assert_eq!(
        repo.repo
            .bare(&["show", "refs/heads/main:src/api/result.json"]),
        integration_fixture::CONTENT
    );
    assert!(matches!(
        snapshot.report.command_outcome,
        Some(GitHubAttemptOutcome::Accepted)
    ));
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Applied);
    assert_eq!(
        git::sha(
            &integrator
                .report_bytes(&snapshot.report_artifact.sha256)
                .unwrap()
        ),
        snapshot.report_artifact.sha256
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    let receipt = integrator
        .reconcile(&f.store, WORKER, poll(&f, &id, "confirm"))
        .await
        .unwrap();
    assert_eq!(receipt["integration"]["data"]["state"], "confirmed");
    assert_eq!(receipt["integration"]["data"]["current"], true);
    assert_eq!(receipt["source_synchronized"], false);
    assert_eq!(receipt["acceptance_ready"], false);
    assert_eq!(f.guards().await, 0);
    no_completion(&f).await;
}

#[tokio::test]
async fn target_race_is_rejected_by_real_receive_pack_old_id_cas() {
    let (repo, f, integrator) = setup_github().await;
    repo.repo.git(&["switch", "--detach", &repo.repo.base]);
    repo.repo
        .commit("other.txt", b"independent target movement");
    let changed = repo.repo.git(&["rev-parse", "HEAD"]);
    repo.repo.git(&[
        "push",
        repo.repo.config.repository.to_str().unwrap(),
        "HEAD:refs/heads/other",
    ]);
    *repo.receive.drift_on_post.lock().unwrap() = Some(changed.clone());
    let (id, lease) = ready(&f).await;
    let result = integrator
        .execute(&f.store, WORKER, f.dispatch_request("race", &id, &lease))
        .await
        .unwrap();
    assert!(matches!(
        result.report.command_outcome,
        Some(GitHubAttemptOutcome::Rejected)
    ));
    assert_eq!(repo.repo.bare(&["rev-parse", "refs/heads/main"]), changed);
    assert_eq!(result.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    no_completion(&f).await;
}

#[tokio::test]
async fn concurrent_dispatch_and_reconstructed_replay_execute_one_actual_post() {
    let (repo, f, integrator) = setup_github().await;
    let (id, lease) = ready(&f).await;
    let (a, b) = tokio::join!(
        integrator.execute(&f.store, WORKER, f.dispatch_request("a", &id, &lease)),
        integrator.execute(&f.store, WORKER, f.dispatch_request("b", &id, &lease))
    );
    a.unwrap();
    b.unwrap();
    let store = DeliverySyncStore::from_config(f.config.clone());
    repo.integrator()
        .execute(&store, WORKER, f.dispatch_request("restart", &id, &lease))
        .await
        .unwrap();
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_dir(&repo.config.repository.report_directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|p| p.file_name().to_string_lossy().ends_with("-attempt.json"))
            .count(),
        1
    );
}

#[tokio::test]
async fn lost_reply_and_malformed_acknowledgement_recover_actual_original_effect_without_repost() {
    for malformed in [false, true] {
        let (repo, f, integrator) = setup_github().await;
        repo.receive.lose_reply.store(!malformed, Ordering::SeqCst);
        repo.receive
            .malformed_reply
            .store(malformed, Ordering::SeqCst);
        let (id, lease) = ready(&f).await;
        let result = integrator
            .execute(&f.store, WORKER, f.dispatch_request("lost", &id, &lease))
            .await
            .unwrap();
        assert!(matches!(
            result.report.command_outcome,
            Some(GitHubAttemptOutcome::Uncertain { .. })
        ));
        assert_eq!(result.report.outcome, IntegrationOutcome::Applied);
        let restarted = repo.integrator();
        assert_eq!(
            restarted
                .query(&f.store, WORKER, &id, "fresh-recovery")
                .await
                .unwrap()
                .report
                .outcome,
            IntegrationOutcome::Applied
        );
        let receipt = restarted
            .reconcile(&f.store, WORKER, poll(&f, &id, "recovery-confirm"))
            .await
            .unwrap();
        assert_eq!(receipt["integration"]["data"]["state"], "confirmed");
        assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
        no_completion(&f).await;
    }
}

#[tokio::test]
async fn missing_permit_reply_never_creates_a_later_effect_and_keeps_unknown_guard() {
    let (repo, f, _) = setup_github().await;
    let (id, lease) = ready(&f).await;
    drop(f.dispatched(&id, &lease).await);
    let integrator = repo.integrator();
    let result = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("lost-permit", &id, &lease),
        )
        .await
        .unwrap();
    assert!(!result.report.attempt_recorded);
    assert_eq!(result.report.outcome, IntegrationOutcome::Unknown);
    let confirmed = integrator
        .reconcile(&f.store, WORKER, poll(&f, &id, "unknown"))
        .await
        .unwrap();
    assert_eq!(confirmed["integration"]["data"]["state"], "unknown");
    assert_eq!(f.guards().await, 1);
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fresh_policy_permission_and_ancestry_refuse_before_any_post() {
    for (route, value, diagnostic) in [
        (
            "",
            json!({"id":7,"full_name":"acme/demo","archived":false,"permissions":{"push":false}}),
            GitHubError::ProviderAuthorizationUnavailable,
        ),
        (
            "",
            json!({"id":7,"full_name":"acme/demo","archived":true,"permissions":{"push":true}}),
            GitHubError::ProviderPolicyUnsupported,
        ),
        (
            "/rules/branches/main",
            json!([{"type":"required_pull_request"}]),
            GitHubError::ProviderPolicyUnsupported,
        ),
    ] {
        let (repo, f, integrator) = setup_github().await;
        repo.api.overrides.set(route, value);
        let (id, lease) = ready(&f).await;
        let result = integrator
            .execute(&f.store, WORKER, f.dispatch_request("policy", &id, &lease))
            .await
            .unwrap();
        assert!(
            matches!(result.report.command_outcome, Some(GitHubAttemptOutcome::NotStarted { diagnostic:d }) if d==diagnostic)
        );
        assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
        assert_eq!(
            repo.repo.bare(&["rev-parse", "refs/heads/main"]),
            repo.repo.base
        );
    }
    let (repo, f, integrator) = setup_github().await;
    repo.api.overrides.set(
        "/branches/main",
        json!({"name":"main","protected":true,"commit":{"sha":repo.repo.base}}),
    );
    let (id, lease) = ready(&f).await;
    let result = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("protected", &id, &lease),
        )
        .await
        .unwrap();
    assert!(matches!(
        result.report.command_outcome,
        Some(GitHubAttemptOutcome::NotStarted {
            diagnostic: GitHubError::ProviderPolicyUnsupported
        })
    ));
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sealed_permit_rechecks_revoked_issuer_and_changed_review_checks() {
    for revoke in [true, false] {
        let (repo, f, integrator) = setup_github().await;
        let (id, lease) = ready(&f).await;
        let permit = f.dispatched(&id, &lease).await.permit.unwrap();
        if revoke {
            f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'").await.unwrap();
        } else {
            // A new current check head invalidates the old bound approval.
            let reserved = f
                .store
                .reserve_inspection(
                    TENANT,
                    PROJECT,
                    WORKER,
                    ReserveDeliveryInspection {
                        request_id: "failed-reserve".into(),
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
                        request_id: "failed-ingest".into(),
                        read_set: f.set.clone(),
                        connector_id: "git".into(),
                        inspection_id: reserved["data"]["inspection_id"].as_str().unwrap().into(),
                        event_id: "failed-check".into(),
                        records: vec![DeliveryEnvelope {
                            protocol: DELIVERY_PROTOCOL.into(),
                            protocol_version: DELIVERY_PROTOCOL_VERSION,
                            record: DeliveryRecord::Verification(VerificationRun {
                                binding: f.selection.candidate.binding.clone(),
                                run_id: "failed".into(),
                                check: "report".into(),
                                result_artifact: None,
                                outcome: VerificationOutcome::Failed,
                                provenance: FactProvenance {
                                    source: FactSource::AdapterObservation,
                                    reference: "fixture://changed-check".into(),
                                    observed_at_unix_ms: None,
                                    recorded_at_unix_ms: 2,
                                },
                            }),
                        }],
                    },
                )
                .await
                .unwrap();
        }
        let result = integrator
            .execute_permit(&f.store, WORKER, permit, "recheck")
            .await
            .unwrap();
        assert!(matches!(
            result.report.command_outcome,
            Some(GitHubAttemptOutcome::NotStarted { .. })
        ));
        assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
        assert_eq!(
            repo.repo.bare(&["rev-parse", "refs/heads/main"]),
            repo.repo.base
        );
    }
}

#[tokio::test]
async fn original_recovery_ignores_new_pr_head_and_check_but_keeps_original_candidate() {
    let (repo, f, integrator) = setup_github().await;
    let (id, lease) = ready(&f).await;
    integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("original", &id, &lease),
        )
        .await
        .unwrap();
    let mut selection = f.selection.clone();
    selection.request_id = "new-candidate".into();
    selection.expected_selected_digest = Some(f.request.candidate_digest.clone());
    selection.candidate.binding.candidate_version = "2".into();
    f.store
        .select_candidate(TENANT, PROJECT, A, selection)
        .await
        .unwrap();
    repo.api
        .overrides
        .set("/pulls/11", json!({"head":{"sha":"e".repeat(40)}}));
    repo.api.overrides.raw(
        &format!(
            "/commits/{}/check-runs?filter=latest&per_page=100&page=1",
            repo.repo.source
        ),
        500,
        b"unavailable current checks".to_vec(),
    );
    let calls = repo.api.calls.lock().unwrap().len();
    let result = repo
        .integrator()
        .query(&f.store, WORKER, &id, "historical")
        .await
        .unwrap();
    assert_eq!(result.report.candidate_digest, f.request.candidate_digest);
    assert_eq!(result.report.outcome, IntegrationOutcome::Applied);
    assert!(
        repo.api.calls.lock().unwrap()[calls..]
            .iter()
            .all(|s| !s.contains("pulls") && !s.contains("check-runs"))
    );
    let value = repo
        .integrator()
        .reconcile(&f.store, WORKER, poll(&f, &id, "historical-confirm"))
        .await
        .unwrap();
    assert_eq!(value["integration"]["data"]["current"], false);
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    no_completion(&f).await;
}

#[tokio::test]
async fn wrong_current_mapping_and_revoked_worker_refuse_even_cached_reports() {
    let (repo, f, integrator) = setup_github().await;
    let (id, lease) = ready(&f).await;
    let mut config = repo.config.clone();
    config.repository.resource = "fixture://other".into();
    let wrong =
        GitHubIntegrator::from_transports(config, repo.api.clone(), repo.receive.clone()).unwrap();
    assert_eq!(
        wrong
            .execute(&f.store, WORKER, f.dispatch_request("wrong", &id, &lease))
            .await
            .err(),
        Some(GitHubError::BindingMismatch)
    );
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    integrator
        .execute(&f.store, WORKER, f.dispatch_request("valid", &id, &lease))
        .await
        .unwrap();
    let calls = repo.api.calls.lock().unwrap().len();
    f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-worker'").await.unwrap();
    assert_eq!(
        integrator
            .query(&f.store, WORKER, &id, "after-revoke")
            .await
            .err(),
        Some(GitHubError::AuthorizationUnavailable)
    );
    assert_eq!(calls, repo.api.calls.lock().unwrap().len());
}

#[tokio::test]
async fn cancelled_preflight_blocks_late_launch_and_restart_never_reposts() {
    let (repo, f, integrator) = setup_github().await;
    repo.api.overrides.set(
        "",
        json!({"id":7,"full_name":"acme/demo","archived":false,"permissions":{"push":true}}),
    );
    repo.api.overrides.0.lock().unwrap().delay_ms = 150;
    let (id, lease) = ready(&f).await;
    let request = f.dispatch_request("cancel", &id, &lease);
    {
        let pending = integrator.execute(&f.store, WORKER, request);
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("expected cancellation during slow preflight"),
            _ = async {
                loop {
                    let marked = std::fs::read_dir(&repo.config.repository.report_directory).unwrap().filter_map(Result::ok)
                        .any(|p| p.file_name().to_string_lossy().ends_with("-attempt.json"));
                    if marked && !repo.api.calls.lock().unwrap().is_empty() { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            } => {}
        }
    }
    repo.api.overrides.0.lock().unwrap().delay_ms = 0;
    let snapshot = repo
        .integrator()
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("after-cancel", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Unknown);
    assert!(snapshot.report.attempt_recorded);
    assert!(snapshot.report.command_outcome.is_none());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    assert_eq!(
        repo.repo.bare(&["rev-parse", "refs/heads/main"]),
        repo.repo.base
    );
}

#[tokio::test]
async fn actual_server_policy_and_unrecognized_or_missing_policy_refuse_effects() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let (repo, f, integrator) = setup_github().await;
        let hook = repo.repo.config.repository.join("hooks/pre-receive");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (id, lease) = ready(&f).await;
        let snapshot = integrator
            .execute(
                &f.store,
                WORKER,
                f.dispatch_request("live-policy", &id, &lease),
            )
            .await
            .unwrap();
        assert!(matches!(
            snapshot.report.command_outcome,
            Some(GitHubAttemptOutcome::Rejected)
        ));
        assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
        assert_eq!(
            repo.repo.bare(&["rev-parse", "refs/heads/main"]),
            repo.repo.base
        );
    }
    for status in [404, 403, 500] {
        let (repo, f, integrator) = setup_github().await;
        repo.api.overrides.raw(
            "/rules/branches/main",
            status,
            b"private policy error".to_vec(),
        );
        let (id, lease) = ready(&f).await;
        let snapshot = integrator
            .execute(
                &f.store,
                WORKER,
                f.dispatch_request("missing-policy", &id, &lease),
            )
            .await
            .unwrap();
        assert!(matches!(
            snapshot.report.command_outcome,
            Some(GitHubAttemptOutcome::NotStarted { .. })
        ));
        assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn cancelled_launched_post_is_recovered_from_target_bytes_without_second_effect() {
    let (repo, f, integrator) = setup_github().await;
    repo.receive.post_delay_ms.store(200, Ordering::SeqCst);
    let (id, lease) = ready(&f).await;
    {
        let pending = integrator.execute(
            &f.store,
            WORKER,
            f.dispatch_request("cancel-launched", &id, &lease),
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("expected cancellation during launched receive-pack"),
            _ = async {
                loop {
                    if repo.receive.posts.load(Ordering::SeqCst) == 1 { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            } => {}
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let result = repo
        .integrator()
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("cancel-recovery", &id, &lease),
        )
        .await
        .unwrap();
    assert!(result.report.command_outcome.is_none());
    assert_eq!(result.report.outcome, IntegrationOutcome::Applied);
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn original_source_inclusion_with_changed_target_artifact_stays_unknown() {
    let (repo, f, integrator) = setup_github().await;
    let (id, lease) = ready(&f).await;
    integrator
        .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
        .await
        .unwrap();
    repo.repo
        .commit("src/api/result.json", b"modified downstream bytes");
    repo.repo.push_main();
    let result = integrator
        .query(&f.store, WORKER, &id, "changed-bytes")
        .await
        .unwrap();
    assert_eq!(
        result
            .report
            .observation
            .as_ref()
            .unwrap()
            .graph_contains_source,
        Some(true)
    );
    assert_eq!(result.report.outcome, IntegrationOutcome::Unknown);
    assert_eq!(repo.receive.posts.load(Ordering::SeqCst), 1);
    let receipt = integrator
        .reconcile(&f.store, WORKER, poll(&f, &id, "changed-confirm"))
        .await
        .unwrap();
    assert_eq!(receipt["integration"]["data"]["state"], "unknown");
    assert_eq!(f.guards().await, 1);
    no_completion(&f).await;
}
