#![cfg(feature = "pg-tests")]
//! Actual Git/PG effects in isolated fixtures; no native business acceptance credit.
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod access;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "fixtures/local_git.rs"]
mod git;
#[path = "../../awr-team-pg/tests/fixtures/delivery_integration.rs"]
mod integration_fixture;

use access::*;
use awr_server::delivery_adapter::{
    LocalGitError, LocalGitIntegrationConfig, LocalGitIntegrationPollRequest, LocalGitIntegrator,
    local_git_integration::LocalGitAttemptOutcome,
};
use awr_team::delivery::*;
use awr_team_pg::*;
use integration_fixture::{SUPERVISOR, WORKER};
use serde_json::{Value, json};

async fn setup_local(
    repo: git::GitFixture,
    candidate: DeliveryCandidate,
) -> (
    git::GitFixture,
    integration_fixture::Fixture,
    LocalGitIntegrator,
) {
    let f = integration_fixture::setup_integration_with_candidate(Some(candidate)).await;
    let integrator = open(&repo).await;
    (repo, f, integrator)
}

async fn open(repo: &git::GitFixture) -> LocalGitIntegrator {
    LocalGitIntegrator::open(LocalGitIntegrationConfig {
        enabled: true,
        repository: repo.config.clone(),
    })
    .await
    .unwrap()
}

async fn ready(f: &integration_fixture::Fixture) -> (String, String) {
    let prepared = f.prepared().await;
    let id = prepared["integration_id"].as_str().unwrap().to_owned();
    let lease = f.leased(&id).await;
    (id, lease["lease_id"].as_str().unwrap().into())
}

fn poll(f: &integration_fixture::Fixture, id: &str, key: &str) -> LocalGitIntegrationPollRequest {
    LocalGitIntegrationPollRequest {
        request_id: key.into(),
        integration_id: id.into(),
        read_set: f.set.clone(),
        connector_version: "1".into(),
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
async fn sha1_fast_forward_is_real_version_bound_and_distinct_from_task_acceptance() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
        .await
        .unwrap();
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    assert_eq!(
        repo.bare(&["show", "refs/heads/main:src/api/result.json"]),
        integration_fixture::CONTENT
    );
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Applied);
    assert!(matches!(
        snapshot.report.command_outcome,
        Some(LocalGitAttemptOutcome::Exited { code: 0 })
    ));
    assert_eq!(
        git::sha(
            &integrator
                .report_bytes(&snapshot.report_artifact.sha256)
                .unwrap()
        ),
        snapshot.report_artifact.sha256
    );
    let confirmation = integrator
        .reconcile(&f.store, WORKER, poll(&f, &id, "confirm"))
        .await
        .unwrap();
    assert_eq!(confirmation["integration"]["data"]["state"], "confirmed");
    assert_eq!(confirmation["integration"]["data"]["current"], true);
    assert_eq!(confirmation["acceptance_ready"], false);
    assert_eq!(confirmation["source_synchronized"], false);
    assert_eq!(f.guards().await, 0);
    no_completion(&f).await;
}

#[tokio::test]
async fn sha256_missing_target_uses_zero_revision_cas_and_exact_bytes() {
    let repo = git::GitFixture::integration(true);
    repo.bare(&["update-ref", "-d", "refs/heads/main"]);
    let mut candidate = repo.integration_candidate();
    candidate.binding.target.precondition = TargetPrecondition::Missing;
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Applied);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    assert_eq!(repo.source.len(), 64);
    no_completion(&f).await;
}

#[tokio::test]
async fn maximum_length_dispatch_id_retains_bounded_inspection_and_retry_identity() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let request = f.dispatch_request(&"r".repeat(128), &id, &lease);
    let first = integrator
        .execute(&f.store, WORKER, request.clone())
        .await
        .unwrap();
    let replay = integrator.execute(&f.store, WORKER, request).await.unwrap();
    assert_eq!(first.report.outcome, IntegrationOutcome::Applied);
    assert!(first.report.inspection_id.len() <= 128);
    assert_eq!(first.report_artifact, replay.report_artifact);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
}

#[tokio::test]
async fn concurrent_callers_and_reconstructed_replay_never_dispatch_a_second_effect() {
    let repo = git::GitFixture::integration(false);
    repo.bare(&["config", "core.logAllRefUpdates", "true"]);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let (a, b) = tokio::join!(
        integrator.execute(
            &f.store,
            WORKER,
            f.dispatch_request("effect-a", &id, &lease)
        ),
        integrator.execute(
            &f.store,
            WORKER,
            f.dispatch_request("effect-b", &id, &lease)
        )
    );
    a.unwrap();
    b.unwrap();
    let store = DeliverySyncStore::from_config(f.config.clone());
    open(&repo)
        .await
        .execute(
            &store,
            WORKER,
            f.dispatch_request("effect-replay", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    assert_eq!(
        std::fs::read_to_string(repo.config.repository.join("logs/refs/heads/main"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_dir(&repo.config.report_directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|p| p.file_name().to_string_lossy().ends_with("-attempt.json"))
            .count(),
        1
    );
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn lost_dispatch_permit_without_an_attempt_keeps_unknown_and_never_retries_git() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, _) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    drop(f.dispatched(&id, &lease).await);
    let store = DeliverySyncStore::from_config(f.config.clone());
    let integrator = open(&repo).await;
    let snapshot = integrator
        .execute(
            &store,
            WORKER,
            f.dispatch_request("replay-after-loss", &id, &lease),
        )
        .await
        .unwrap();
    assert!(!snapshot.report.attempt_recorded);
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Unknown);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    let receipt = integrator
        .reconcile(&store, WORKER, poll(&f, &id, "unknown-confirm"))
        .await
        .unwrap();
    assert_eq!(receipt["integration"]["data"]["state"], "unknown");
    assert_eq!(f.guards().await, 1);
    no_completion(&f).await;
}

#[tokio::test]
async fn original_issuer_revocation_after_dispatch_is_rechecked_before_git() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let permit = f.dispatched(&id, &lease).await.permit.unwrap();
    f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'").await.unwrap();
    let snapshot = integrator
        .execute_permit(&f.store, WORKER, permit, "revoked-issuer")
        .await
        .unwrap();
    assert!(matches!(
        snapshot.report.command_outcome,
        Some(LocalGitAttemptOutcome::NotStarted {
            diagnostic: LocalGitError::AuthorizationUnavailable
        })
    ));
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    let receipt = integrator
        .reconcile(&f.store, WORKER, poll(&f, &id, "confirm-no-effect"))
        .await
        .unwrap();
    assert_eq!(receipt["integration"]["data"]["current"], false);
    assert_eq!(f.guards().await, 0);
    no_completion(&f).await;
}

#[tokio::test]
async fn revoked_worker_cannot_launch_or_record_new_connector_facts() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let permit = f.dispatched(&id, &lease).await.permit.unwrap();
    f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-worker'").await.unwrap();
    let snapshot = integrator
        .execute_permit(&f.store, WORKER, permit, "revoked-worker")
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert!(matches!(
        integrator
            .reconcile(&f.store, WORKER, poll(&f, &id, "revoked-facts"))
            .await,
        Err(LocalGitError::AuthorizationUnavailable)
    ));
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn rotated_worker_credential_cannot_reuse_the_original_dispatch_authority() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let permit = f.dispatched(&id, &lease).await.permit.unwrap();
    let rotated =
        "awr1.integration-worker.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    f.admin
        .execute(
            "UPDATE awr_team.credentials SET secret_hash=$1 WHERE id='integration-worker'",
            &[&workstream_credential_hash(rotated).unwrap()],
        )
        .await
        .unwrap();
    let snapshot = integrator
        .execute_permit(&f.store, rotated, permit, "rotated-worker")
        .await
        .unwrap();
    assert!(matches!(
        snapshot.report.command_outcome,
        Some(LocalGitAttemptOutcome::NotStarted {
            diagnostic: LocalGitError::AuthorizationUnavailable
        })
    ));
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[tokio::test]
async fn missing_approved_source_cannot_launch_a_repository_effect() {
    let repo = git::GitFixture::integration(false);
    let mut candidate = repo.integration_candidate();
    candidate.binding.source_revision.as_mut().unwrap().value = "a".repeat(40);
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("missing-source", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert!(matches!(
        snapshot.report.command_outcome,
        Some(LocalGitAttemptOutcome::NotStarted { .. })
    ));
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[tokio::test]
async fn expired_dispatch_lease_is_rechecked_before_launch() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let permit = f.dispatched(&id, &lease).await.permit.unwrap();
    f.admin.batch_execute("UPDATE awr_team.delivery_integration_intents SET lease_expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
    let snapshot = integrator
        .execute_permit(&f.store, WORKER, permit, "expired-lease")
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[tokio::test]
async fn changed_required_check_after_dispatch_refuses_the_original_permit() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let permit = f.dispatched(&id, &lease).await.permit.unwrap();
    f.check("failed-new-check", VerificationOutcome::Failed)
        .await;
    let snapshot = integrator
        .execute_permit(&f.store, WORKER, permit, "changed-check")
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[tokio::test]
async fn invalidated_review_after_dispatch_refuses_git_even_with_the_old_approved_receipt() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let permit = f.dispatched(&id, &lease).await.permit.unwrap();
    f.admin
        .batch_execute("UPDATE awr_team.review_rounds SET state='invalidated'")
        .await
        .unwrap();
    let snapshot = integrator
        .execute_permit(&f.store, WORKER, permit, "returned-review")
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[tokio::test]
async fn target_drift_does_not_overwrite_an_unrelated_revision() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    repo.git(&["checkout", "--detach", &repo.base]);
    repo.commit("unrelated.txt", b"Other work");
    let unrelated = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&[
        "push",
        "--force",
        repo.config.repository.to_str().unwrap(),
        "HEAD:refs/heads/main",
    ]);
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("target-drift", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), unrelated);
}

#[tokio::test]
async fn authentic_review_of_declared_bytes_does_not_bypass_actual_blob_verification() {
    let mut repo = git::GitFixture::integration(false);
    repo.commit("src/api/result.json", b"Unapproved different bytes");
    repo.source = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&[
        "push",
        repo.config.repository.to_str().unwrap(),
        "HEAD:refs/heads/candidate",
    ]);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("wrong-blob", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[tokio::test]
async fn symbolic_artifact_mode_cannot_be_integrated_as_a_regular_file() {
    let mut repo = git::GitFixture::integration(false);
    let blob = repo.git(&["rev-parse", "HEAD:src/api/result.json"]);
    repo.git(&[
        "update-index",
        "--cacheinfo",
        &format!("120000,{blob},src/api/result.json"),
    ]);
    repo.git(&["commit", "-m", "Synthetic symbolic artifact"]);
    repo.source = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&[
        "push",
        repo.config.repository.to_str().unwrap(),
        "HEAD:refs/heads/candidate",
    ]);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("symbolic-artifact", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[tokio::test]
async fn non_fast_forward_source_cannot_replace_the_approved_target() {
    let mut repo = git::GitFixture::integration(false);
    repo.git(&["checkout", "--orphan", "unrelated-candidate"]);
    repo.git(&["commit", "-m", "Synthetic independent root"]);
    repo.source = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&[
        "push",
        "--force",
        repo.config.repository.to_str().unwrap(),
        "HEAD:refs/heads/candidate",
    ]);
    // A legacy graft falsely makes the approved target an ancestor. The
    // configured adapter must use the actual commit graph instead.
    std::fs::write(
        repo.config.repository.join("info/grafts"),
        format!("{} {}\n", repo.source, repo.base),
    )
    .unwrap();
    repo.bare(&["merge-base", "--is-ancestor", &repo.base, &repo.source]);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("divergent-source", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Rejected);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[tokio::test]
async fn changed_source_and_connector_epoch_recover_the_original_version_as_history() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (_repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    integrator
        .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
        .await
        .unwrap();
    let catalog: Value = f
        .admin
        .query_one(
            "SELECT catalog_json FROM awr_team.workstream_catalogs WHERE snapshot_id=$1",
            &[&f.set.source_snapshot_id],
        )
        .await
        .unwrap()
        .get(0);
    let rows = f.admin.query("SELECT c.contract_json,o.workstream_id FROM awr_team.work_contracts c
        JOIN awr_team.workstream_snapshot_ownership o USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        WHERE c.snapshot_id=$1 ORDER BY c.work_id", &[&f.set.source_snapshot_id]).await.unwrap();
    let contracts = rows
        .into_iter()
        .map(|r| {
            let mut contract: awr_team::WorkContract = serde_json::from_value(r.get(0)).unwrap();
            if contract.work_id.as_str() == "a" {
                contract
                    .acceptance
                    .push("Verify the updated requirement".into());
            }
            awr_team::WorkstreamContract {
                workstream_id: r.get::<_, String>(1).parse().unwrap(),
                contract,
            }
        })
        .collect();
    let bundle = awr_team::WorkstreamBundle {
        codec: awr_team::WorkstreamBundle::CODEC_V3.into(),
        catalog: serde_json::from_value(catalog).unwrap(),
        contracts,
    };
    let source = SourceStore::from_config(f.config.clone());
    let changed = source
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: "workstreams/3".into(),
            files: vec![SourceFile {
                path: "workstreams.json".into(),
                bytes: serde_json::to_vec(&bundle).unwrap(),
            }],
        })
        .await
        .unwrap();
    source
        .approve(
            TENANT,
            PROJECT,
            &changed.proposal_id,
            "reviewer",
            &changed.manifest_digest,
        )
        .await
        .unwrap();
    integration_fixture::run(
        &f.reads,
        A,
        "release-before-source",
        "claim.release",
        json!({
        "session_id":"session-a","expected_session_version":"1","claim_id":f.selection.claim_id,
        "expected_fence":f.selection.fence,"expected_lease_version":f.selection.lease_version }),
    )
    .await;
    source
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &changed.proposal_id,
            &awr_team::SourceActivationPlan {
                candidate_digest: changed.manifest_digest.clone(),
                parser_version: changed.parser_version,
                expected_authority_epoch: changed.base_epoch,
                approved_candidate_digest: changed.manifest_digest,
            },
        )
        .await
        .unwrap();
    let set = integration_fixture::set(&prepare(&f.reads, WORKER, "a").await);
    assert_ne!(set.source_snapshot_id, f.set.source_snapshot_id);
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "configure-current-epoch".into(),
                read_set: set.clone(),
                expected_connector_version: "1".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "git".into(),
                    provider: "local_git".into(),
                    resource: integration_fixture::RESOURCE.into(),
                    principal_actor_id: "integrator".into(),
                    principal_client_id: "cli-worker".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let receipt = integrator
        .reconcile(
            &f.store,
            WORKER,
            LocalGitIntegrationPollRequest {
                request_id: "source-history".into(),
                integration_id: id,
                read_set: set,
                connector_version: "2".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        receipt["observation"]["data"]["observation_receipt"]["state"],
        "superseded"
    );
    assert_eq!(
        receipt["observation"]["data"]["observation_receipt"]["source_snapshot_id"],
        f.set.source_snapshot_id
    );
    assert_eq!(receipt["integration"]["data"]["state"], "confirmed");
    assert_eq!(receipt["integration"]["data"]["current"], false);
    assert_eq!(f.guards().await, 0);
    no_completion(&f).await;
}

#[tokio::test]
async fn conflicting_attempt_marker_refuses_the_only_permit_without_touching_git() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let permit = f.dispatched(&id, &lease).await.permit.unwrap();
    let key = git::sha(&serde_json::to_vec(&(TENANT, PROJECT, &id)).unwrap());
    std::fs::write(
        repo.config
            .report_directory
            .join(format!("integration-{key}-attempt.json")),
        b"conflicting immutable attempt",
    )
    .unwrap();
    assert!(matches!(
        integrator
            .execute_permit(&f.store, WORKER, permit, "conflicting-marker")
            .await,
        Err(LocalGitError::ReportConflict)
    ));
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn missing_archived_report_fails_exact_retry_and_new_inspection_recovers_actual_bytes() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
        .await
        .unwrap();
    std::fs::remove_file(repo.config.report_directory.join(format!(
        "integration-report-{}.json",
        snapshot.report_artifact.sha256
    )))
    .unwrap();
    assert!(matches!(
        integrator
            .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
            .await,
        Err(LocalGitError::ReportUnavailable)
    ));
    let fresh = integrator
        .query(&f.store, WORKER, &id, "new-observation-after-loss")
        .await
        .unwrap();
    assert_eq!(fresh.report.outcome, IntegrationOutcome::Applied);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
}

#[tokio::test]
async fn same_poll_retry_returns_the_durable_confirm_receipt_after_reconstruction() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    integrator
        .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
        .await
        .unwrap();
    let original = integrator
        .reconcile(&f.store, WORKER, poll(&f, &id, "stable-poll"))
        .await
        .unwrap();
    let store = DeliverySyncStore::from_config(f.config.clone());
    let replay = open(&repo)
        .await
        .reconcile(&store, WORKER, poll(&f, &id, "stable-poll"))
        .await
        .unwrap();
    assert_eq!(replay["integration"]["replayed"], true);
    assert_eq!(
        replay["integration"]["data"],
        original["integration"]["data"]
    );
    assert_eq!(
        replay["integration"]["committed_project_revision"],
        original["integration"]["committed_project_revision"]
    );
    assert_eq!(
        replay["observation"]["data"],
        original["observation"]["data"]
    );
}

#[tokio::test]
async fn original_confirmation_snapshot_binds_the_exact_observation_and_server_recording_time() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let dispatch = f.dispatched(&id, &lease).await;
    drop(dispatch); // Lose the first capability without fabricating an effect.
    let receipt = integrator
        .reconcile(&f.store, WORKER, poll(&f, &id, "snapshot-confirm"))
        .await
        .unwrap();
    let events: i64 = f
        .admin
        .query_one("SELECT count(*) FROM awr_team.events", &[])
        .await
        .unwrap()
        .get(0);
    let view = f
        .store
        .inspect_integration(TENANT, PROJECT, WORKER, "a", &id)
        .await
        .unwrap();
    let proof = &view["confirmation"];
    let observation = &receipt["observation"]["data"]["observation_receipt"];
    assert_eq!(view["state"], "unknown");
    assert_eq!(proof["fact_id"], observation["fact_ids"][0]);
    assert_eq!(proof["receipt"], *observation);
    assert_eq!(proof["inspection_id"], observation["inspection_id"]);
    assert_eq!(proof["candidate_digest"], f.request.candidate_digest);
    assert_eq!(proof["connector_version"], "1");
    let envelope: DeliveryEnvelope = serde_json::from_value(proof["envelope"].clone()).unwrap();
    envelope.validate().unwrap();
    let DeliveryRecord::IntegrationObservation(record) = envelope.record else {
        panic!("wrong confirmation kind");
    };
    assert_eq!(record.request_id.unwrap().as_str(), id);
    assert_eq!(record.binding, f.selection.candidate.binding);
    assert_eq!(record.outcome, IntegrationOutcome::Unknown);
    assert_eq!(
        record.provenance.recorded_at_unix_ms,
        observation["recorded_at_unix_ms"].as_u64().unwrap()
    );
    let hash = record.provenance.reference.rsplit(':').next().unwrap();
    let report: awr_server::delivery_adapter::local_git_integration::LocalGitIntegrationReport =
        serde_json::from_slice(&integrator.report_bytes(hash).unwrap()).unwrap();
    assert_eq!(
        record.provenance.observed_at_unix_ms,
        Some(report.observed_at_unix_ms)
    );
    assert_eq!(report.integration_id, id);
    assert_eq!(report.inspection_id, proof["inspection_id"]);
    assert_eq!(
        f.admin
            .query_one("SELECT count(*) FROM awr_team.events", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        events
    );
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(f.guards().await, 1);
    no_completion(&f).await;
}

#[tokio::test]
async fn expired_original_inspection_replay_changes_only_live_metadata_and_grants_no_effect() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, _integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    drop(f.dispatched(&id, &lease).await);
    let request = ReserveDeliveryInspection {
        request_id: "original-inspection-liveness".into(),
        read_set: f.set.clone(),
        connector_id: "git".into(),
        connector_version: "1".into(),
        candidate_digest: f.request.candidate_digest.clone(),
        lease_seconds: 5,
    };
    let original = f
        .store
        .reserve_integration_inspection(TENANT, PROJECT, WORKER, &id, request.clone())
        .await
        .unwrap();
    assert_eq!(original["inspection_lease"]["live"], true);
    let inspection = original["data"]["inspection_id"].as_str().unwrap();
    // Scoped fault injection, never delivery prerequisite construction.
    f.admin.execute("UPDATE awr_team.delivery_inspections SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1", &[&inspection]).await.unwrap();
    let store = DeliverySyncStore::from_config(f.config.clone());
    let replay = store
        .reserve_integration_inspection(TENANT, PROJECT, WORKER, &id, request)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], original["data"]);
    assert_eq!(
        replay["committed_project_revision"],
        original["committed_project_revision"]
    );
    assert_eq!(replay["inspection_lease"]["live"], false);
    assert_eq!(replay["inspection_lease"]["state_basis"], "at_read");
    let view = store
        .inspect_integration(TENANT, PROJECT, WORKER, "a", &id)
        .await
        .unwrap();
    assert_eq!(view["state"], "dispatched");
    assert_eq!(view["confirmation"], Value::Null);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(f.guards().await, 1);
    no_completion(&f).await;
}

#[tokio::test]
async fn changed_selection_recovers_original_applied_fact_as_history_only() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (_repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    integrator
        .execute(&f.store, WORKER, f.dispatch_request("effect", &id, &lease))
        .await
        .unwrap();
    let mut selected = f.selection.clone();
    selected.request_id = "select-next-version".into();
    selected.expected_selected_digest = Some(f.request.candidate_digest.clone());
    selected.candidate.binding.candidate_version = "2".into();
    f.store
        .select_candidate(TENANT, PROJECT, A, selected.clone())
        .await
        .unwrap();
    let receipt = integrator
        .reconcile(&f.store, WORKER, poll(&f, &id, "historical-confirm"))
        .await
        .unwrap();
    assert_eq!(
        receipt["observation"]["data"]["observation_receipt"]["state"],
        "superseded"
    );
    assert_eq!(receipt["integration"]["data"]["state"], "confirmed");
    assert_eq!(receipt["integration"]["data"]["current"], false);
    assert_eq!(f.guards().await, 0);
    let view = f.store.inspect(TENANT, PROJECT, WORKER, "a").await.unwrap();
    assert_eq!(view["candidate"], json!(selected.candidate));
    no_completion(&f).await;
}

#[tokio::test]
async fn failed_cas_after_launch_is_unknown_and_an_unlocked_target_does_not_authorize_retry() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let lock = repo.config.repository.join("refs/heads/main.lock");
    std::fs::write(&lock, b"Synthetic other writer").unwrap();
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("locked-cas", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Unknown);
    assert!(
        matches!(snapshot.report.command_outcome, Some(LocalGitAttemptOutcome::Exited { code }) if code != 0)
    );
    std::fs::remove_file(lock).unwrap();
    integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("after-unlock", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn unconfigured_principal_and_public_receipt_cannot_invoke_git() {
    let repo = git::GitFixture::integration(false);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    assert!(matches!(
        integrator
            .execute(
                &f.store,
                SUPERVISOR,
                f.dispatch_request("wrong-principal", &id, &lease)
            )
            .await,
        Err(LocalGitError::AuthorizationUnavailable)
    ));
    let view = f
        .store
        .inspect_integration(TENANT, PROJECT, SUPERVISOR, "a", &id)
        .await
        .unwrap();
    assert_eq!(view["execution_authorized"], false);
    assert!(serde_json::from_value::<DispatchDeliveryIntegration>(view).is_err());
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
}

#[cfg(unix)]
fn delayed_git(repo: &mut git::GitFixture, apply_first: bool) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    fn quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
    }
    let real = quote(&repo.config.git_executable);
    let marker = repo.root.join("command-launches");
    let command = if apply_first {
        format!("{real} \"$@\"\n")
    } else {
        String::new()
    };
    let wrapper = repo.root.join("fixed-test-git");
    std::fs::write(&wrapper, format!("#!/bin/sh\ncase \"$*\" in *'update-ref --no-deref'*)\nprintf x >> {}\n{command}exec sleep 2;; esac\nexec {real} \"$@\"\n", quote(&marker))).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    repo.config.git_executable = wrapper;
    repo.config.command_timeout_ms = 1000;
    marker
}

#[cfg(unix)]
#[tokio::test]
async fn target_race_after_preflight_is_rejected_by_actual_git_compare_and_swap() {
    use std::os::unix::fs::PermissionsExt;
    let mut repo = git::GitFixture::integration(false);
    repo.git(&["checkout", "--detach", &repo.base]);
    repo.commit("concurrent.txt", b"Concurrent delivery");
    let other = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&[
        "push",
        repo.config.repository.to_str().unwrap(),
        "HEAD:refs/heads/other",
    ]);
    let real = format!(
        "'{}'",
        repo.config
            .git_executable
            .to_str()
            .unwrap()
            .replace('\'', "'\\''")
    );
    let wrapper = repo.root.join("fixed-racing-git");
    std::fs::write(&wrapper, format!("#!/bin/sh\ncase \"$*\" in *'update-ref --no-deref'*)\n{real} --git-dir=. update-ref --no-deref refs/heads/main {other} {} || exit 1;; esac\nexec {real} \"$@\"\n", repo.base)).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    repo.config.git_executable = wrapper;
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("target-race", &id, &lease),
        )
        .await
        .unwrap();
    assert!(
        matches!(snapshot.report.command_outcome, Some(LocalGitAttemptOutcome::Exited { code }) if code != 0)
    );
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Unknown);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), other);
    assert_eq!(f.guards().await, 1);
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_after_real_effect_and_restart_recovers_by_query_without_another_launch() {
    let mut repo = git::GitFixture::integration(false);
    let marker = delayed_git(&mut repo, true);
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("lost-response", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Applied);
    assert!(matches!(
        snapshot.report.command_outcome,
        Some(LocalGitAttemptOutcome::Uncertain {
            diagnostic: LocalGitError::TimedOut
        })
    ));
    let store = DeliverySyncStore::from_config(f.config.clone());
    let restarted = open(&repo).await;
    restarted
        .execute(
            &store,
            WORKER,
            f.dispatch_request("restart-replay", &id, &lease),
        )
        .await
        .unwrap();
    let receipt = restarted
        .reconcile(&store, WORKER, poll(&f, &id, "restart-confirm"))
        .await
        .unwrap();
    assert_eq!(receipt["integration"]["data"]["state"], "confirmed");
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "x");
    assert_eq!(f.guards().await, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_future_loses_result_but_durable_attempt_blocks_a_second_launch() {
    let mut repo = git::GitFixture::integration(false);
    let marker = delayed_git(&mut repo, false);
    repo.config.command_timeout_ms = 3000;
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let mut effect = Box::pin(integrator.execute(
        &f.store,
        WORKER,
        f.dispatch_request("cancelled-effect", &id, &lease),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !marker.exists() {
            tokio::select! {
                result = &mut effect => panic!("effect unexpectedly finished: {:?}", result.err()),
                _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {},
            }
        }
    })
    .await
    .unwrap();
    drop(effect);
    let store = DeliverySyncStore::from_config(f.config.clone());
    let restarted = open(&repo).await;
    let snapshot = restarted
        .execute(
            &store,
            WORKER,
            f.dispatch_request("cancelled-replay", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Unknown);
    assert!(snapshot.report.command_outcome.is_none());
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.base);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "x");
    assert_eq!(f.guards().await, 1);
}

#[cfg(unix)]
#[tokio::test]
async fn repository_reference_hook_and_ambient_network_settings_do_not_execute() {
    use std::os::unix::fs::PermissionsExt;
    let repo = git::GitFixture::integration(false);
    let hook = repo.config.repository.join("hooks/reference-transaction");
    let marker = repo.root.join("hook-executed");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    let candidate = repo.integration_candidate();
    let (repo, f, integrator) = setup_local(repo, candidate).await;
    let (id, lease) = ready(&f).await;
    let snapshot = integrator
        .execute(
            &f.store,
            WORKER,
            f.dispatch_request("hooks-disabled", &id, &lease),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.report.outcome, IntegrationOutcome::Applied);
    assert_eq!(repo.bare(&["rev-parse", "refs/heads/main"]), repo.source);
    assert!(!marker.exists());
}
