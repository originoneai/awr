#![cfg(feature = "pg-tests")]
//! A work that delivers a selected candidate into a repository is complete only after the
//! integration of that candidate was confirmed for its current binding. Synthetic mechanism
//! checks: the adapter observations are recorded by the test, not made by a repository.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/delivery_integration.rs"]
mod integration_fixture;

use awr_team::delivery::*;
use awr_team_pg::*;
use fixture::*;
use integration_fixture::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

static CALLS: AtomicUsize = AtomicUsize::new(0);

/// Finalize or complete as the supervisor, under a request key of its own.
async fn complete(f: &Fixture, label: &str, op: &str) -> PgResult<Value> {
    let key = format!("{label}-{op}-{}", CALLS.fetch_add(1, Ordering::Relaxed));
    let p = prepare(&f.reads, SUPERVISOR, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            SUPERVISOR,
            command(
                &p,
                &key,
                op,
                json!({"session_id":"session-supervisor","expected_session_version":"1",
                    "evidence_id":f.evidence["evidence_id"],"context_complete":true}),
            ),
        )
        .await
}

async fn receipts(f: &Fixture) -> i64 {
    f.admin
        .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
        .await
        .unwrap()
        .get(0)
}

/// Both completion operations are refused for this reason, and nothing is recorded.
macro_rules! refused_until_confirmed {
    ($f:expr, $reason:literal) => {
        for op in ["delivery.finalize", "work.complete"] {
            let result = complete($f, $reason, op).await;
            assert!(
                matches!(
                    &result,
                    Err(PgError::IntegrationNotConfirmed { reason: $reason })
                ),
                "{op}: expected {}, got {result:?}",
                $reason
            );
            assert_eq!(receipts($f).await, 0, "{op} recorded a receipt");
        }
    };
}

async fn confirm(f: &Fixture, id: &str, key: &str, outcome: IntegrationOutcome) {
    let record = f.observation(id, outcome);
    let fact = f.ingest_record(key, record).await;
    f.store
        .confirm_integration(TENANT, PROJECT, WORKER, f.confirm_request(key, id, &fact))
        .await
        .unwrap();
}

async fn dispatched(f: &Fixture) -> String {
    let id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let lease = f.leased(&id).await;
    f.dispatched(&id, lease["lease_id"].as_str().unwrap()).await;
    id
}

#[tokio::test]
async fn completion_waits_in_every_unsettled_state_until_the_integration_is_confirmed() {
    let f = setup_simulated_member_integration().await;
    refused_until_confirmed!(&f, "integration_not_prepared");
    let id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    refused_until_confirmed!(&f, "integration_prepared");
    let lease = f.leased(&id).await;
    refused_until_confirmed!(&f, "integration_prepared");
    f.dispatched(&id, lease["lease_id"].as_str().unwrap()).await;
    refused_until_confirmed!(&f, "integration_dispatched");
    // An observation with an unknown outcome keeps the attempt open.
    confirm(&f, &id, "unknown-effect", IntegrationOutcome::Unknown).await;
    refused_until_confirmed!(&f, "integration_unknown");
    // A later applied observation of the same attempt settles it.
    confirm(&f, &id, "applied-effect", IntegrationOutcome::Applied).await;
    assert_eq!(receipts(&f).await, 0, "a confirmation never completes work");
    let done = complete(&f, "complete-after-confirmation", "work.complete")
        .await
        .unwrap();
    assert_eq!(done["receipt"]["data"]["task_complete"], true);
    assert_eq!(receipts(&f).await, 1);
}

#[tokio::test]
async fn finalization_accepts_a_confirmed_integration() {
    let f = setup_simulated_member_integration().await;
    let id = dispatched(&f).await;
    confirm(&f, &id, "applied-effect", IntegrationOutcome::Applied).await;
    let done = complete(&f, "finalize-after-confirmation", "delivery.finalize")
        .await
        .unwrap();
    assert_eq!(done["receipt"]["data"]["task_complete"], true);
    assert_eq!(receipts(&f).await, 1);
}

#[tokio::test]
async fn completion_refuses_a_withdrawn_integration() {
    let f = setup_simulated_member_integration().await;
    let id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.store
        .reject_prepared_integration(
            TENANT,
            PROJECT,
            SUPERVISOR,
            RejectPreparedDeliveryIntegration {
                request_id: "withdraw".into(),
                read_set: f.set.clone(),
                integration_id: id,
                reason: "The target is being reorganized.".into(),
            },
        )
        .await
        .unwrap();
    refused_until_confirmed!(&f, "integration_rejected");
}

#[tokio::test]
async fn completion_explains_the_newest_attempt_of_the_selection() {
    let f = setup_simulated_member_integration().await;
    let first = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.store
        .reject_prepared_integration(
            TENANT,
            PROJECT,
            SUPERVISOR,
            RejectPreparedDeliveryIntegration {
                request_id: "withdraw".into(),
                read_set: f.set.clone(),
                integration_id: first.clone(),
                reason: "The target is being reorganized.".into(),
            },
        )
        .await
        .unwrap();
    refused_until_confirmed!(&f, "integration_rejected");

    // A new attempt for the same selection is what the refusal names, not the withdrawn one.
    let mut again = f.request.clone();
    again.request_id = "prepare-after-withdrawal".into();
    let second = f
        .store
        .prepare_integration(TENANT, PROJECT, SUPERVISOR, again)
        .await
        .unwrap()["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(second, first, "the second attempt is a new integration");
    refused_until_confirmed!(&f, "integration_prepared");
    let lease = f.leased(&second).await;
    refused_until_confirmed!(&f, "integration_prepared");
    f.dispatched(&second, lease["lease_id"].as_str().unwrap())
        .await;
    refused_until_confirmed!(&f, "integration_dispatched");

    // Once the new attempt is confirmed, the withdrawn one does not stand in the way.
    confirm(&f, &second, "applied-effect", IntegrationOutcome::Applied).await;
    let done = complete(&f, "complete-after-second-attempt", "work.complete")
        .await
        .unwrap();
    assert_eq!(done["receipt"]["data"]["task_complete"], true);
    assert_eq!(receipts(&f).await, 1);
}

#[tokio::test]
async fn completion_names_a_changed_target_only_when_the_repository_refused_for_it() {
    // The repository refused and reports a target other than the one the candidate expected.
    let f = setup_simulated_member_integration().await;
    let id = dispatched(&f).await;
    let DeliveryRecord::IntegrationObservation(mut seen) =
        f.observation(&id, IntegrationOutcome::Rejected)
    else {
        unreachable!()
    };
    seen.result_revision = Some(RevisionRef {
        resource: RESOURCE.into(),
        format: RevisionFormat::GitSha256,
        value: "c".repeat(64),
    });
    let fact = f
        .ingest_record("moved", DeliveryRecord::IntegrationObservation(seen))
        .await;
    f.store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("moved", &id, &fact),
        )
        .await
        .unwrap();
    refused_until_confirmed!(&f, "integration_target_changed");

    // The repository refused without reporting another target.
    drop(f);
    let f = setup_simulated_member_integration().await;
    let id = dispatched(&f).await;
    confirm(&f, &id, "refused", IntegrationOutcome::Rejected).await;
    refused_until_confirmed!(&f, "integration_rejected");
}

#[tokio::test]
async fn a_confirmation_that_was_not_current_when_recorded_does_not_complete() {
    let f = setup_simulated_member_integration().await;
    let id = dispatched(&f).await;
    let fact = f
        .ingest_record("applied", f.observation(&id, IntegrationOutcome::Applied))
        .await;
    // The issuer's authority lapses while the effect is recorded, then returns.
    f.admin
        .batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'")
        .await
        .unwrap();
    let receipt = f
        .store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("historical", &id, &fact),
        )
        .await
        .unwrap();
    assert_eq!(receipt["data"]["state"], "confirmed");
    assert_eq!(receipt["data"]["current"], false);
    f.admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='integration-supervisor'",
        )
        .await
        .unwrap();
    refused_until_confirmed!(&f, "integration_confirmation_not_current");
}

#[tokio::test]
async fn a_new_attempt_follows_a_confirmation_that_was_not_current() {
    let f = setup_simulated_member_integration().await;
    let first = dispatched(&f).await;
    let fact = f
        .ingest_record(
            "applied",
            f.observation(&first, IntegrationOutcome::Applied),
        )
        .await;
    // The issuer's authority lapses while the effect is recorded, then returns.
    f.admin
        .batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'")
        .await
        .unwrap();
    f.store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("historical", &first, &fact),
        )
        .await
        .unwrap();
    f.admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='integration-supervisor'",
        )
        .await
        .unwrap();
    refused_until_confirmed!(&f, "integration_confirmation_not_current");

    // A new attempt for the same selection is what the refusal names from then on.
    let mut again = f.request.clone();
    again.request_id = "prepare-after-stale-confirmation".into();
    let second = f
        .store
        .prepare_integration(TENANT, PROJECT, SUPERVISOR, again)
        .await
        .unwrap()["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(second, first, "the second attempt is a new integration");
    refused_until_confirmed!(&f, "integration_prepared");
    let lease = f
        .store
        .lease_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.lease_request("lease-second", &second),
        )
        .await
        .unwrap()["data"]["lease_id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.store
        .dispatch_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.dispatch_request("dispatch-second", &second, &lease),
        )
        .await
        .unwrap();
    refused_until_confirmed!(&f, "integration_dispatched");
    confirm(&f, &second, "applied-again", IntegrationOutcome::Applied).await;
    let done = complete(&f, "complete-after-fresh-confirmation", "delivery.finalize")
        .await
        .unwrap();
    assert_eq!(done["receipt"]["data"]["task_complete"], true);
    assert_eq!(receipts(&f).await, 1);
}

#[tokio::test]
async fn work_without_a_delivery_channel_completes_as_before() {
    let f = setup_simulated_member_integration().await;
    // The project declared no repository connector for this work.
    f.admin
        .batch_execute("UPDATE awr_team.delivery_connectors SET work_id='c' WHERE work_id='a'")
        .await
        .unwrap();
    let done = complete(&f, "finalize-without-channel", "delivery.finalize")
        .await
        .unwrap();
    assert_eq!(done["receipt"]["data"]["task_complete"], true);
    assert_eq!(receipts(&f).await, 1);
}
