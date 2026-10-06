#![cfg(feature = "pg-tests")]
//! Synthetic mechanism regressions; no repository effects or native acceptance.
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

#[tokio::test]
async fn actual_member_review_and_checks_allow_one_dispatch_without_completing_work() {
    let f = setup_integration().await;
    let prepared = f.prepared().await;
    let id = prepared["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let dispatch = f.dispatched(id, lease["lease_id"].as_str().unwrap()).await;
    let permit = dispatch.permit.unwrap();
    assert_eq!(permit.candidate(), &f.selection.candidate);
    assert_eq!(permit.request().request_id.as_str(), id);
    assert_eq!(permit.connector_id(), "git");
    assert_eq!(permit.connector_version(), "1");
    assert_eq!(permit.eligibility_digest(), prepared["eligibility_digest"]);
    assert_eq!(permit.request().verified_checks[0].run_id, "initial-check");
    let fact = f
        .ingest_record("applied", f.observation(id, IntegrationOutcome::Applied))
        .await;
    let confirmation = f
        .store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("confirm", id, &fact),
        )
        .await
        .unwrap();
    assert_eq!(confirmation["data"]["state"], "confirmed");
    assert_eq!(confirmation["data"]["current"], true);
    assert_eq!(confirmation["acceptance_ready"], false);
    assert_eq!(f.guards().await, 0);
    let count: i64 = f
        .admin
        .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
    let approval: Value = f
        .admin
        .query_one("SELECT to_jsonb(d) FROM awr_team.review_decisions d", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(approval["approval_basis"], "agent_review");
    assert_eq!(approval["reviewer_person_id"], "member-b");
}

#[tokio::test]
async fn prepare_replay_conflict_and_same_target_contention_preserve_original_intent() {
    let f = setup_integration().await;
    let original = f.prepared().await;
    let replay = f
        .store
        .prepare_integration(TENANT, PROJECT, SUPERVISOR, f.request.clone())
        .await
        .unwrap();
    assert_eq!(replay["data"], original);
    assert_eq!(replay["replayed"], true);
    let mut changed = f.request.clone();
    changed.review_decision_id = "different".into();
    assert!(matches!(
        f.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, changed)
            .await,
        Err(PgError::IdempotencyConflict)
    ));
    let mut second = f.request.clone();
    second.request_id = "second".into();
    assert!(matches!(
        f.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, second)
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(f.guards().await, 1);
    let count: i64 = f
        .admin
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_integration_intents",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn concurrent_prepare_and_dispatch_have_one_target_and_one_permit() {
    let f = setup_integration().await;
    let mut second = f.request.clone();
    second.request_id = "competing".into();
    let (a, b) = tokio::join!(
        f.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, f.request.clone()),
        f.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, second)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let prepared = a.or(b).unwrap();
    let id = prepared["data"]["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let fence = lease["lease_id"].as_str().unwrap();
    let (a, b) = tokio::join!(
        f.store.dispatch_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.dispatch_request("effect-a", id, fence)
        ),
        f.store.dispatch_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.dispatch_request("effect-b", id, fence)
        )
    );
    assert_eq!(
        usize::from(a.unwrap().permit.is_some()) + usize::from(b.unwrap().permit.is_some()),
        1
    );
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn stale_and_expired_leases_cannot_dispatch_but_pre_dispatch_expiry_can_recover() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let first = f.leased(id).await;
    assert!(matches!(
        f.store
            .lease_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.lease_request("parallel-lease", id)
            )
            .await,
        Err(PgError::ClaimHeld)
    ));
    f.admin.execute("UPDATE awr_team.delivery_integration_intents SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1",&[&id]).await.unwrap();
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request("expired", id, first["lease_id"].as_str().unwrap())
            )
            .await,
        Err(PgError::LeaseExpired)
    ));
    let second = f
        .store
        .lease_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.lease_request("replacement-lease", id),
        )
        .await
        .unwrap();
    assert_ne!(first["lease_id"], second["data"]["lease_id"]);
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request("stale", id, first["lease_id"].as_str().unwrap())
            )
            .await,
        Err(PgError::StaleFence)
    ));
    assert!(
        f.dispatched(id, second["data"]["lease_id"].as_str().unwrap())
            .await
            .permit
            .is_some()
    );
}

#[tokio::test]
async fn reconstructed_store_and_lost_dispatch_reply_never_return_another_permit() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let fence = lease["lease_id"].as_str().unwrap();
    let original = f.dispatched(id, fence).await;
    assert!(original.permit.is_some());
    drop(original.permit);
    let rebuilt = DeliverySyncStore::from_config(f.config.clone());
    let same = rebuilt
        .dispatch_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.dispatch_request("dispatch", id, fence),
        )
        .await
        .unwrap();
    assert!(same.permit.is_none());
    assert_eq!(same.receipt["replayed"], true);
    let new_key = rebuilt
        .dispatch_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.dispatch_request("new-key", id, fence),
        )
        .await
        .unwrap();
    assert!(new_key.permit.is_none());
    assert_eq!(new_key.receipt, original.receipt);
    f.admin.execute("UPDATE awr_team.delivery_integration_intents SET lease_expires_at=clock_timestamp()-interval '1 hour' WHERE id=$1",&[&id]).await.unwrap();
    assert!(matches!(
        rebuilt
            .lease_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.lease_request("retry-after-dispatch", id)
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    let view = rebuilt
        .inspect_integration(TENANT, PROJECT, SUPERVISOR, "a", id)
        .await
        .unwrap();
    assert_eq!(view["state"], "dispatched");
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn unknown_observation_retains_guard_until_bound_terminal_fact_and_replays_confirmation() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    f.dispatched(id, lease["lease_id"].as_str().unwrap()).await;
    let unknown = f
        .ingest_record("unknown", f.observation(id, IntegrationOutcome::Unknown))
        .await;
    let receipt = f
        .store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("unknown-confirm", id, &unknown),
        )
        .await
        .unwrap();
    assert_eq!(receipt["data"]["state"], "unknown");
    assert_eq!(f.guards().await, 1);
    assert!(matches!(
        f.store
            .reject_prepared_integration(
                TENANT,
                PROJECT,
                SUPERVISOR,
                RejectPreparedDeliveryIntegration {
                    request_id: "unsafe-cancel".into(),
                    read_set: f.set.clone(),
                    integration_id: id.into(),
                    reason: "No target change was observed.".into()
                }
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    let fact = f
        .ingest_record(
            "later-applied",
            f.observation(id, IntegrationOutcome::Applied),
        )
        .await;
    let request = f.confirm_request("resolved", id, &fact);
    let receipt = f
        .store
        .confirm_integration(TENANT, PROJECT, WORKER, request.clone())
        .await
        .unwrap();
    let replay = DeliverySyncStore::from_config(f.config.clone())
        .confirm_integration(TENANT, PROJECT, WORKER, request)
        .await
        .unwrap();
    assert_eq!(receipt["data"], replay["data"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(f.guards().await, 0);
}

#[tokio::test]
async fn original_issuer_and_worker_credentials_are_live_at_dispatch() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let changes = [
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='integration-supervisor'",
        ),
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='integration-supervisor'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='integration-supervisor'",
        ),
        (
            "UPDATE awr_team.credentials SET secret_hash='sha256:rotated' WHERE id='integration-supervisor'",
            "",
        ),
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-worker'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='integration-worker'",
        ),
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='integration-worker'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='integration-worker'",
        ),
        (
            "UPDATE awr_team.credentials SET secret_hash='sha256:rotated' WHERE id='integration-worker'",
            "",
        ),
    ];
    for (i, (change, restore)) in changes.iter().enumerate() {
        f.admin.batch_execute(change).await.unwrap();
        assert!(matches!(
            f.store
                .dispatch_integration(
                    TENANT,
                    PROJECT,
                    WORKER,
                    f.dispatch_request(
                        &format!("revoked-{i}"),
                        id,
                        lease["lease_id"].as_str().unwrap()
                    )
                )
                .await,
            Err(PgError::Forbidden)
        ));
        if restore.is_empty() {
            let (credential, token) = if i == 2 {
                ("integration-supervisor", SUPERVISOR)
            } else {
                ("integration-worker", WORKER)
            };
            f.admin
                .execute(
                    "UPDATE awr_team.credentials SET secret_hash=$1 WHERE id=$2",
                    &[&workstream_credential_hash(token).unwrap(), &credential],
                )
                .await
                .unwrap();
        } else {
            f.admin.batch_execute(restore).await.unwrap();
        }
    }
    assert_eq!(f.guards().await, 1);
    assert!(
        f.dispatched(id, lease["lease_id"].as_str().unwrap())
            .await
            .permit
            .is_some()
    );
}

#[tokio::test]
async fn issuer_delegation_membership_and_worker_grant_version_changes_fence_dispatch() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let changes = [
        (
            "UPDATE awr_team.project_memberships SET role='developer' WHERE actor_id='supervisor'",
            "UPDATE awr_team.project_memberships SET role='maintainer' WHERE actor_id='supervisor'",
        ),
        (
            "UPDATE awr_team.project_memberships SET membership_version=membership_version+1 WHERE actor_id='supervisor'",
            "UPDATE awr_team.project_memberships SET membership_version=membership_version-1 WHERE actor_id='supervisor'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE client_id='cli-supervisor'",
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE client_id='cli-supervisor'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-worker'",
            "UPDATE awr_team.workstream_grants SET grant_version=grant_version-1 WHERE client_id='cli-worker'",
        ),
        (
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='grant-supervisor'",
            "UPDATE awr_team.agent_authorizations SET status='active' WHERE id='grant-supervisor'",
        ),
    ];
    for (i, (change, restore)) in changes.iter().enumerate() {
        f.admin.batch_execute(change).await.unwrap();
        assert!(
            f.store
                .dispatch_integration(
                    TENANT,
                    PROJECT,
                    WORKER,
                    f.dispatch_request(
                        &format!("authorization-change-{i}"),
                        id,
                        lease["lease_id"].as_str().unwrap()
                    )
                )
                .await
                .is_err()
        );
        f.admin.batch_execute(restore).await.unwrap();
    }
    assert!(
        f.dispatched(id, lease["lease_id"].as_str().unwrap())
            .await
            .permit
            .is_some()
    );
}

#[tokio::test]
async fn developer_cannot_integrate_and_caller_cannot_supply_private_authority() {
    let f = setup_integration().await;
    assert!(matches!(
        f.store
            .prepare_integration(TENANT, PROJECT, A, f.request.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    assert!(
        f.store
            .prepare_integration(TENANT, PROJECT, B, f.request.clone())
            .await
            .is_err()
    );
    let mut value = json!(f.request);
    value["issuer_secret_hash"] = json!("forged");
    assert!(serde_json::from_value::<PrepareDeliveryIntegration>(value).is_err());
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    assert!(matches!(
        f.store
            .lease_integration(
                TENANT,
                PROJECT,
                SUPERVISOR,
                f.lease_request("caller-worker", id)
            )
            .await,
        Err(PgError::Forbidden)
    ));
    let view = f
        .store
        .inspect_integration(TENANT, PROJECT, SUPERVISOR, "a", id)
        .await
        .unwrap()
        .to_string();
    for private in [
        "issuer_secret_hash",
        "issuer_credential_id",
        SUPERVISOR,
        WORKER,
    ] {
        assert!(!view.contains(private));
    }
    let audit:String=f.admin.query_one("SELECT COALESCE(jsonb_agg(payload_json)::text,'[]') FROM awr_team.events WHERE event_type LIKE 'delivery.integration.%'",&[]).await.unwrap().get(0);
    assert!(!audit.contains(&workstream_credential_hash(SUPERVISOR).unwrap()));
}

#[tokio::test]
async fn artifact_evidence_approval_and_check_changes_refuse_new_effects() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let artifact = f.evidence["artifact_id"].as_str().unwrap();
    f.admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$1 WHERE id=$2",
            &[&b"changed".as_slice(), &artifact],
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request("bytes-changed", id, lease["lease_id"].as_str().unwrap())
            )
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    f.admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$1 WHERE id=$2",
            &[&CONTENT.as_bytes(), &artifact],
        )
        .await
        .unwrap();
    f.admin
        .execute(
            "UPDATE awr_team.review_rounds SET state='invalidated' WHERE id=$1",
            &[&f.request.review_round_id],
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request("approval-invalid", id, lease["lease_id"].as_str().unwrap())
            )
            .await,
        Err(PgError::ReviewRequired)
    ));
    f.admin
        .execute(
            "UPDATE awr_team.review_rounds SET state='approved' WHERE id=$1",
            &[&f.request.review_round_id],
        )
        .await
        .unwrap();
    f.check("failed-after-approval", VerificationOutcome::Failed)
        .await;
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request("check-failed", id, lease["lease_id"].as_str().unwrap())
            )
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    f.check("passed-new-version", VerificationOutcome::Passed)
        .await;
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request("checks-changed", id, lease["lease_id"].as_str().unwrap())
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn changed_selection_connector_source_and_epoch_fence_prepared_approval() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let changes = [
        (
            "UPDATE awr_team.delivery_selections SET selection_version=selection_version+1",
            "UPDATE awr_team.delivery_selections SET selection_version=selection_version-1",
        ),
        (
            "UPDATE awr_team.delivery_connectors SET version=version+1",
            "UPDATE awr_team.delivery_connectors SET version=version-1",
        ),
        (
            "UPDATE awr_team.delivery_connectors SET enabled=false",
            "UPDATE awr_team.delivery_connectors SET enabled=true",
        ),
        (
            "UPDATE awr_team.projects SET coordinator_epoch='different' WHERE tenant_id='reader-tenant'",
            "UPDATE awr_team.projects SET coordinator_epoch='epoch-a' WHERE tenant_id='reader-tenant'",
        ),
        (
            "UPDATE awr_team.delivery_selections SET source_snapshot_id='historical'",
            "",
        ),
    ];
    for (i, (change, restore)) in changes.iter().enumerate() {
        f.admin.batch_execute(change).await.unwrap();
        assert!(
            f.store
                .dispatch_integration(
                    TENANT,
                    PROJECT,
                    WORKER,
                    f.dispatch_request(
                        &format!("drift-{i}"),
                        id,
                        lease["lease_id"].as_str().unwrap()
                    )
                )
                .await
                .is_err()
        );
        if !restore.is_empty() {
            f.admin.batch_execute(restore).await.unwrap();
        }
    }
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn stale_issuer_after_effect_preserves_historical_repository_fact() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    f.dispatched(id, lease["lease_id"].as_str().unwrap()).await;
    let fact = f
        .ingest_record("applied", f.observation(id, IntegrationOutcome::Applied))
        .await;
    f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'").await.unwrap();
    let receipt = f
        .store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("historical", id, &fact),
        )
        .await
        .unwrap();
    assert_eq!(receipt["data"]["state"], "confirmed");
    assert_eq!(receipt["data"]["current"], false);
    assert_eq!(f.guards().await, 0);
    let count: i64 = f
        .admin
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_facts WHERE id=$1",
            &[&fact],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn only_exact_authenticated_integration_observation_resolves_dispatch() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    f.dispatched(id, lease["lease_id"].as_str().unwrap()).await;
    let check = f
        .check("unrelated-check", VerificationOutcome::Passed)
        .await;
    assert!(matches!(
        f.store
            .confirm_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.confirm_request("not-integration", id, &check)
            )
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    let wrong = f
        .ingest_record(
            "other-request",
            f.observation("unrelated-request", IntegrationOutcome::Applied),
        )
        .await;
    assert!(matches!(
        f.store
            .confirm_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.confirm_request("wrong-binding", id, &wrong)
            )
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn cancelling_a_pre_dispatch_intent_releases_target_without_authorizing_effect() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let request = RejectPreparedDeliveryIntegration {
        request_id: "cancel".into(),
        read_set: f.set.clone(),
        integration_id: id.into(),
        reason: "Review requires a replacement.".into(),
    };
    let receipt = f
        .store
        .reject_prepared_integration(TENANT, PROJECT, SUPERVISOR, request.clone())
        .await
        .unwrap();
    assert_eq!(receipt["data"]["before_dispatch"], true);
    assert_eq!(f.guards().await, 0);
    assert_eq!(
        f.store
            .reject_prepared_integration(TENANT, PROJECT, SUPERVISOR, request)
            .await
            .unwrap()["replayed"],
        true
    );
    let mut replacement = f.request.clone();
    replacement.request_id = "replacement".into();
    assert!(
        f.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, replacement)
            .await
            .is_ok()
    );
    assert!(matches!(
        f.store
            .lease_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.lease_request("cancelled-retry", id)
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
}

#[tokio::test]
async fn forged_missing_or_caller_declared_eligibility_never_creates_an_intent() {
    let f = setup_integration().await;
    for (i, field) in [
        "evidence_id",
        "review_round_id",
        "review_decision_id",
        "candidate_digest",
        "selection_version",
    ]
    .iter()
    .enumerate()
    {
        let mut value = json!(f.request);
        value["request_id"] = json!(format!("forged-{i}"));
        value[*field] = json!(if *field == "candidate_digest" {
            "f".repeat(64)
        } else if *field == "selection_version" {
            "2".into()
        } else {
            "absent".into()
        });
        assert!(
            f.store
                .prepare_integration(
                    TENANT,
                    PROJECT,
                    SUPERVISOR,
                    serde_json::from_value(value).unwrap()
                )
                .await
                .is_err()
        );
        assert_eq!(f.guards().await, 0);
    }
    let original: Value = f
        .admin
        .query_one(
            "SELECT payload_json FROM awr_team.evidence WHERE id=$1",
            &[&f.request.evidence_id],
        )
        .await
        .unwrap()
        .get(0);
    f.admin.execute("UPDATE awr_team.evidence SET payload_json=payload_json-'delivery_candidate_digest' WHERE id=$1",&[&f.request.evidence_id]).await.unwrap();
    assert!(matches!(
        f.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, f.request.clone())
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    f.admin
        .execute(
            "UPDATE awr_team.evidence SET payload_json=$1 WHERE id=$2",
            &[&original, &f.request.evidence_id],
        )
        .await
        .unwrap();
    f.admin.batch_execute("UPDATE awr_team.delivery_facts SET envelope_json=jsonb_set(envelope_json,'{record,data,provenance,source}','\"caller_declared\"')").await.unwrap();
    assert!(matches!(
        f.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, f.request.clone())
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    f.admin
        .batch_execute("DELETE FROM awr_team.delivery_fact_heads")
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, f.request.clone())
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    assert_eq!(f.guards().await, 0);
    let mut value = json!(f.request);
    value["approved"] = json!(true);
    assert!(serde_json::from_value::<PrepareDeliveryIntegration>(value).is_err());
}

#[tokio::test]
async fn delegation_record_changes_and_rotated_worker_identity_refuse_old_lease() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let original: Value = f
        .admin
        .query_one(
            "SELECT body_json FROM awr_team.agent_authorizations WHERE id='grant-supervisor'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    f.admin.batch_execute("UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{expires_at_ms}','9007199254740991') WHERE id='grant-supervisor'").await.unwrap();
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request(
                    "changed-delegation",
                    id,
                    lease["lease_id"].as_str().unwrap()
                )
            )
            .await,
        Err(PgError::Forbidden)
    ));
    f.admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='grant-supervisor'",
            &[&original],
        )
        .await
        .unwrap();
    let rotated =
        "awr1.integration-worker.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    f.admin
        .execute(
            "UPDATE awr_team.credentials SET secret_hash=$1 WHERE id='integration-worker'",
            &[&workstream_credential_hash(rotated).unwrap()],
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                rotated,
                f.dispatch_request("rotated-worker", id, lease["lease_id"].as_str().unwrap())
            )
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn target_guard_is_tenant_wide_while_other_project_intents_remain_hidden() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    // Seed only persistence preconditions in a second synthetic project. Its
    // ownership conflict is checked through the real non-owner RLS role.
    f.admin.batch_execute("INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('reader-tenant','other-project','other','team','epoch-other','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES('reader-tenant','other-project','other-work','other-work');
        INSERT INTO awr_team.delivery_connectors(tenant_id,project_id,id,scope_id,work_id,workstream_id,provider,resource,principal_actor_id,principal_client_id,
            fact_source,version,coordinator_epoch,enabled,configured_by_actor_id)
        SELECT tenant_id,'other-project',id,scope_id,'other-work',workstream_id,provider,resource,principal_actor_id,principal_client_id,
            fact_source,version,'epoch-other',enabled,configured_by_actor_id FROM awr_team.delivery_connectors;
        INSERT INTO awr_team.delivery_integration_intents(tenant_id,project_id,id,work_id,connector_id,request_json,eligibility_json,eligibility_digest,
            issuer_credential_id,issuer_secret_hash,issuer_authority_binding,state)
        SELECT tenant_id,'other-project','other-intent','other-work',connector_id,request_json,eligibility_json,eligibility_digest,
            issuer_credential_id,issuer_secret_hash,issuer_authority_binding,'prepared' FROM awr_team.delivery_integration_intents;").await.unwrap();
    let (mut client, connection) = f.config.connect(tokio_postgres::NoTls).await.unwrap();
    let driver = tokio::spawn(async move { connection.await.unwrap() });
    let tx = client.transaction().await.unwrap();
    tx.execute(
        "SELECT set_config('awr.tenant_id',$1,true),set_config('awr.project_id',$2,true)",
        &[&TENANT, &"other-project"],
    )
    .await
    .unwrap();
    let visible: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_integration_target_guards",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(visible, 0);
    let acquired=tx.execute("INSERT INTO awr_team.delivery_integration_target_guards(tenant_id,resource,reference,project_id,intent_id)
        VALUES($1,$2,'main','other-project','other-intent') ON CONFLICT(tenant_id,resource,reference) DO NOTHING",&[&TENANT,&RESOURCE]).await.unwrap();
    assert_eq!(acquired, 0);
    tx.commit().await.unwrap();
    drop(client);
    driver.await.unwrap();
    let original: String = f
        .admin
        .query_one(
            "SELECT intent_id FROM awr_team.delivery_integration_target_guards",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(original, id);
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn pre_dispatch_observation_and_task_recovery_block_cannot_authorize_an_effect() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let before = f
        .ingest_record(
            "before-dispatch",
            f.observation(id, IntegrationOutcome::Applied),
        )
        .await;
    let lease = f.leased(id).await;
    f.admin
        .batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=true WHERE work_id='a'")
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request("blocked", id, lease["lease_id"].as_str().unwrap())
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    f.admin
        .batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=false WHERE work_id='a'")
        .await
        .unwrap();
    f.dispatched(id, lease["lease_id"].as_str().unwrap()).await;
    assert!(matches!(
        f.store
            .confirm_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.confirm_request("old-observation", id, &before)
            )
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn a_bound_post_dispatch_rejection_resolves_only_the_original_effect() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    f.dispatched(id, lease["lease_id"].as_str().unwrap()).await;
    let fact = f
        .ingest_record("rejected", f.observation(id, IntegrationOutcome::Rejected))
        .await;
    let receipt = f
        .store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("rejection", id, &fact),
        )
        .await
        .unwrap();
    assert_eq!(receipt["data"]["state"], "rejected");
    assert_eq!(f.guards().await, 0);
    assert!(matches!(
        f.store
            .lease_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.lease_request("retry-rejected", id)
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    let mut new = f.request.clone();
    new.request_id = "separate-request".into();
    let next = f
        .store
        .prepare_integration(TENANT, PROJECT, SUPERVISOR, new)
        .await
        .unwrap();
    assert_ne!(next["data"]["integration_id"], id);
}

#[tokio::test]
async fn expiry_during_dispatch_rolls_back_receipt_audit_and_permit() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    f.admin.batch_execute("CREATE FUNCTION awr_team.delay_integration_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type='delivery.integration.dispatch' THEN PERFORM pg_sleep(0.15); END IF; RETURN NEW; END $$;
        CREATE TRIGGER delay_integration_dispatch BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.delay_integration_dispatch();").await.unwrap();
    for (i, credential) in [
        Some("integration-supervisor"),
        Some("integration-worker"),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(credential) = credential {
            f.admin.execute("UPDATE awr_team.credentials SET expires_at=clock_timestamp()+interval '0.1 second' WHERE id=$1",&[&credential]).await.unwrap();
        } else {
            f.admin.execute("UPDATE awr_team.delivery_integration_intents SET lease_expires_at=clock_timestamp()+interval '0.1 second' WHERE id=$1",&[&id]).await.unwrap();
        }
        let result = f
            .store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                f.dispatch_request(
                    &format!("expiry-boundary-{i}"),
                    id,
                    lease["lease_id"].as_str().unwrap(),
                ),
            )
            .await;
        assert!(matches!(
            result,
            Err(PgError::Forbidden) | Err(PgError::LeaseExpired)
        ));
        f.admin
            .batch_execute("UPDATE awr_team.credentials SET expires_at=NULL")
            .await
            .unwrap();
        let state: String = f
            .admin
            .query_one(
                "SELECT state FROM awr_team.delivery_integration_intents WHERE id=$1",
                &[&id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "leased");
        let recorded: i64 = f
            .admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_sync_requests WHERE request_id=$1",
                &[&format!("expiry-boundary-{i}")],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(recorded, 0);
        let events:i64=f.admin.query_one("SELECT count(*) FROM awr_team.events WHERE event_type='delivery.integration.dispatch'",&[]).await.unwrap().get(0);
        assert_eq!(events, 0);
        assert_eq!(f.guards().await, 1);
    }
}
