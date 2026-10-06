#![cfg(feature = "pg-tests")]
//! Synthetic transport mechanisms, not native team business acceptance.
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

async fn read(f: &Fixture, token: &str, op: &str, id: Option<&str>) -> PgResult<Value> {
    let mut q = query(op);
    q.work_id = Some("a".into());
    if op == "review.inspect" {
        q.review_round_id = id.map(str::to_owned);
    } else {
        q.request_id = id.map(str::to_owned);
    }
    f.reads.query(TENANT, PROJECT, token, q).await
}

async fn public_prepare(f: &Fixture, key: &str) -> WorkstreamCommand {
    let current = prepare(&f.reads, SUPERVISOR, "a").await;
    let neutral = read(f, SUPERVISOR, "delivery.neutral.inspect", None)
        .await
        .unwrap()["data"]
        .clone();
    let connector = neutral["connectors"]
        .as_array()
        .expect("supervisor must discover current connector versions")
        .iter()
        .find(|c| c["enabled"] == true && c["fact_source"] == "adapter_observation")
        .unwrap();
    let review = read(
        f,
        SUPERVISOR,
        "review.inspect",
        Some(&f.request.review_round_id),
    )
    .await
    .unwrap()["data"]["review"]
        .clone();
    let candidate: DeliveryCandidate =
        serde_json::from_value(neutral["candidate"].clone()).unwrap();
    let args = json!({
        "source_snapshot_id":current["source_snapshot_id"],
        "connector_id":connector["connector_id"],"connector_version":connector["connector_version"],
        "candidate_digest":candidate.binding.digest().unwrap(),"selection_version":neutral["selection_version"],
        "evidence_id":review["evidence_id"],"review_round_id":review["round_id"],
        "review_decision_id":review["decisions"][0]["decision_id"],"operation":"fast_forward",
    });
    assert!(
        args["review_decision_id"].is_string(),
        "review decision must be discoverable"
    );
    command(&current, key, "delivery.integration.prepare", args)
}

#[tokio::test]
async fn supervisor_prepares_from_public_queries_and_inspects_the_original_request() {
    let f = setup_integration().await;
    let command = public_prepare(&f, "wire-prepare").await;
    let prepared = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, SUPERVISOR, command)
        .await
        .unwrap();
    assert_eq!(prepared["execution_authorized"], false);
    assert_eq!(prepared["receipt"]["data"]["state"], "prepared");
    let id = prepared["receipt"]["data"]["integration_id"]
        .as_str()
        .unwrap();
    let inspected = read(&f, SUPERVISOR, "delivery.integration.inspect", Some(id))
        .await
        .unwrap();
    let direct = f
        .store
        .inspect_integration(TENANT, PROJECT, SUPERVISOR, "a", id)
        .await
        .unwrap();
    assert_eq!(inspected["data"], direct);
    assert_eq!(direct["integration_request"]["request_id"], id);
    assert_eq!(direct["candidate"], json!(f.selection.candidate));
    assert_eq!(direct["execution_authorized"], false);
    assert_eq!(direct["acceptance_ready"], false);
    assert_eq!(f.guards().await, 1);

    let receipt = read(&f, REVIEWER, "command.inspect", Some("approve-review"))
        .await
        .unwrap();
    let review = read(
        &f,
        REVIEWER,
        "review.inspect",
        Some(&f.request.review_round_id),
    )
    .await
    .unwrap();
    assert_eq!(
        receipt["data"]["receipt"]["data"]["decision_id"],
        review["data"]["review"]["decisions"][0]["decision_id"]
    );
    assert!(receipt["data"]["receipt"]["data"]["decision_id"].is_string());
}

async fn counts(f: &Fixture) -> Vec<i64> {
    let row = f
        .admin
        .query_one(
            "SELECT (SELECT count(*) FROM awr_team.delivery_integration_intents),
        (SELECT count(*) FROM awr_team.delivery_sync_requests),
        (SELECT count(*) FROM awr_team.operations),
        (SELECT count(*) FROM awr_team.events),
        (SELECT count(*) FROM awr_team.completion_receipts)",
            &[],
        )
        .await
        .unwrap();
    (0..5).map(|i| row.get::<_, i64>(i)).collect()
}

async fn reject(f: &Fixture, key: &str, id: &str) -> WorkstreamCommand {
    let current = prepare(&f.reads, SUPERVISOR, "a").await;
    command(
        &current,
        key,
        "delivery.integration.reject_prepared",
        json!({
        "source_snapshot_id":current["source_snapshot_id"],"integration_id":id,
        "reason":"Withdraw this intent before repository dispatch."}),
    )
}

#[tokio::test]
async fn concurrent_wire_retries_and_outcome_lookup_preserve_one_domain_receipt() {
    let f = setup_integration().await;
    let request = public_prepare(&f, "same-wire-key").await;
    let commands = f.reads.commands();
    let before = counts(&f).await;
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, SUPERVISOR, request.clone()),
        commands.execute(TENANT, PROJECT, SUPERVISOR, request.clone())
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a["receipt"]["data"], b["receipt"]["data"]);
    assert_ne!(a["replayed"], b["replayed"]);
    let after = counts(&f).await;
    assert_eq!(
        after
            .iter()
            .zip(&before)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>(),
        vec![1, 1, 0, 1, 0]
    );
    assert_eq!(f.guards().await, 1);
    let outcome = read(
        &f,
        SUPERVISOR,
        "delivery.neutral.outcome",
        Some("same-wire-key"),
    )
    .await
    .unwrap();
    assert_eq!(outcome["data"]["receipt"]["data"], a["receipt"]["data"]);
    let id = a["receipt"]["data"]["integration_id"].as_str().unwrap();
    assert_ne!(id, "same-wire-key");
    assert!(
        read(
            &f,
            SUPERVISOR,
            "delivery.integration.inspect",
            Some("same-wire-key")
        )
        .await
        .is_err()
    );
    assert!(
        read(&f, SUPERVISOR, "delivery.integration.inspect", Some(id))
            .await
            .is_ok()
    );
    let mut changed = request;
    changed.args["review_decision_id"] = json!("another-decision");
    assert!(matches!(
        commands.execute(TENANT, PROJECT, SUPERVISOR, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(counts(&f).await, after);
}

#[tokio::test]
async fn strict_wire_fields_and_unexposed_worker_operations_have_no_effects() {
    let f = setup_integration().await;
    let valid = public_prepare(&f, "strict-wire").await;
    let before = counts(&f).await;
    for field in [
        "read_set",
        "request_id",
        "actor_id",
        "approved",
        "passed",
        "repository_path",
        "unknown",
    ] {
        let mut bad = valid.clone();
        bad.args[field] = json!("injected");
        assert!(
            f.reads
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, bad)
                .await
                .is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("connector_version", json!("01")),
        ("selection_version", json!("0")),
        ("candidate_digest", json!("not-a-digest")),
        ("operation", json!("force")),
        ("review_decision_id", json!(null)),
        ("source_snapshot_id", json!("")),
    ] {
        let mut bad = valid.clone();
        bad.args[field] = value;
        assert!(
            f.reads
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, bad)
                .await
                .is_err(),
            "{field}"
        );
    }
    for op in [
        "delivery.integration.lease",
        "delivery.integration.dispatch",
        "delivery.integration.confirm",
    ] {
        assert!(!WorkstreamCommand::OPERATIONS.contains(&op));
        let mut bad = valid.clone();
        bad.op = op.into();
        assert!(matches!(
            f.reads
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, bad)
                .await,
            Err(PgError::Unsupported(_))
        ));
    }
    let mut stale = valid.clone();
    stale.expected_ownership_version = "2".into();
    assert!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, SUPERVISOR, stale)
            .await
            .is_err()
    );
    assert_eq!(counts(&f).await, before);
    assert_eq!(f.guards().await, 0);
}

#[tokio::test]
async fn preparation_and_replay_require_current_delivery_authority() {
    let f = setup_integration().await;
    let request = public_prepare(&f, "live-authority").await;
    for token in [A, B, NONE] {
        assert!(
            f.reads
                .commands()
                .execute(TENANT, PROJECT, token, request.clone())
                .await
                .is_err()
        );
    }
    let original = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, SUPERVISOR, request.clone())
        .await
        .unwrap();
    let before = counts(&f).await;
    for (change, restore) in [
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='integration-supervisor'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE client_id='cli-supervisor'",
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE client_id='cli-supervisor'",
        ),
        (
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='grant-supervisor'",
            "UPDATE awr_team.agent_authorizations SET status='active' WHERE id='grant-supervisor'",
        ),
        (
            "UPDATE awr_team.project_memberships SET business_roles='[\"developer\"]' WHERE actor_id='supervisor'",
            "UPDATE awr_team.project_memberships SET business_roles='[\"deliverer\"]' WHERE actor_id='supervisor'",
        ),
    ] {
        f.admin.batch_execute(change).await.unwrap();
        assert!(
            f.reads
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, request.clone())
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(counts(&f).await, before);
        f.admin.batch_execute(restore).await.unwrap();
    }
    let replay = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, SUPERVISOR, request)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"]["data"], original["receipt"]["data"]);
}

#[tokio::test]
async fn wire_preparation_cannot_bypass_changed_checks_approval_or_connector() {
    let f = setup_integration().await;
    let request = public_prepare(&f, "current-records").await;
    let before = counts(&f).await;
    f.admin
        .execute(
            "UPDATE awr_team.review_rounds SET state='invalidated' WHERE id=$1",
            &[&f.request.review_round_id],
        )
        .await
        .unwrap();
    assert!(matches!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, SUPERVISOR, request.clone())
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
    f.admin
        .batch_execute("UPDATE awr_team.delivery_connectors SET version=version+1")
        .await
        .unwrap();
    assert!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, SUPERVISOR, request.clone())
            .await
            .is_err()
    );
    f.admin
        .batch_execute("UPDATE awr_team.delivery_connectors SET version=version-1")
        .await
        .unwrap();
    assert_eq!(counts(&f).await, before);
    f.check("new-failed-check", VerificationOutcome::Failed)
        .await;
    let after_check = counts(&f).await;
    assert!(matches!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, SUPERVISOR, request)
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    assert_eq!(counts(&f).await, after_check);
    assert_eq!(f.guards().await, 0);
}

#[tokio::test]
async fn wire_rejection_releases_only_undispatched_targets_and_replays_original_receipt() {
    for leased in [false, true] {
        let f = setup_integration().await;
        let p = f
            .reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                SUPERVISOR,
                public_prepare(&f, "prepare-rejection").await,
            )
            .await
            .unwrap();
        let id = p["receipt"]["data"]["integration_id"].as_str().unwrap();
        if leased {
            f.leased(id).await;
        }
        let request = reject(&f, "withdraw", id).await;
        let original = f
            .reads
            .commands()
            .execute(TENANT, PROJECT, SUPERVISOR, request.clone())
            .await
            .unwrap();
        assert_eq!(original["receipt"]["data"]["before_dispatch"], true);
        assert_eq!(f.guards().await, 0);
        let after = counts(&f).await;
        let replay = f
            .reads
            .commands()
            .execute(TENANT, PROJECT, SUPERVISOR, request.clone())
            .await
            .unwrap();
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["receipt"]["data"], original["receipt"]["data"]);
        let mut changed = request;
        changed.args["reason"] = json!("A changed withdrawal reason.");
        assert!(matches!(
            f.reads
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, changed)
                .await,
            Err(PgError::IdempotencyConflict)
        ));
        assert_eq!(counts(&f).await, after);
        assert_eq!(
            read(&f, SUPERVISOR, "delivery.integration.inspect", Some(id))
                .await
                .unwrap()["data"]["state"],
            "rejected"
        );
    }
}

#[tokio::test]
async fn dispatched_and_unknown_attempts_remain_queryable_without_cancellation_or_redispatch() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    drop(
        f.dispatched(id, lease["lease_id"].as_str().unwrap())
            .await
            .permit,
    );
    for state in ["dispatched", "unknown"] {
        if state == "unknown" {
            let fact = f
                .ingest_record(
                    "unknown-effect",
                    f.observation(id, IntegrationOutcome::Unknown),
                )
                .await;
            f.store
                .confirm_integration(
                    TENANT,
                    PROJECT,
                    WORKER,
                    f.confirm_request("unknown-confirmation", id, &fact),
                )
                .await
                .unwrap();
        }
        let before = counts(&f).await;
        assert!(matches!(
            f.reads
                .commands()
                .execute(
                    TENANT,
                    PROJECT,
                    SUPERVISOR,
                    reject(&f, &format!("reject-{state}"), id).await
                )
                .await,
            Err(PgError::RecoveryBlocked)
        ));
        let value = read(&f, SUPERVISOR, "delivery.integration.inspect", Some(id))
            .await
            .unwrap();
        assert_eq!(value["data"]["state"], state);
        assert_eq!(value["data"]["execution_authorized"], false);
        assert_eq!(value["data"]["acceptance_ready"], false);
        assert_eq!(counts(&f).await, before);
        assert_eq!(f.guards().await, 1);
    }
    f.admin
        .batch_execute(
            "UPDATE awr_team.delivery_selections SET source_snapshot_id='historical';
        UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=2 WHERE work_id='a';
        UPDATE awr_team.review_rounds SET state='invalidated'",
        )
        .await
        .unwrap();
    let rebuilt = WorkstreamReadStore::from_config(f.config.clone());
    let mut q = query("delivery.integration.inspect");
    q.work_id = Some("a".into());
    q.request_id = Some(id.into());
    let v = rebuilt.query(TENANT, PROJECT, SUPERVISOR, q).await.unwrap();
    assert_eq!(v["data"]["candidate"], json!(f.selection.candidate));
    assert_eq!(v["data"]["original_read_set"], json!(f.set));
    assert_eq!(v["data"]["state"], "unknown");
    assert_eq!(
        v["data"]["request_binding"],
        "original_candidate_and_read_set"
    );
}

#[tokio::test]
async fn original_intent_query_is_scoped_read_only_and_rechecks_current_access() {
    let f = setup_integration().await;
    let p = f.prepared().await;
    let id = p["integration_id"].as_str().unwrap();
    let mut valid = query("delivery.integration.inspect");
    valid.work_id = Some("a".into());
    valid.request_id = Some(id.into());
    let before = counts(&f).await;
    for token in [B, NONE] {
        assert!(
            f.reads
                .query(TENANT, PROJECT, token, valid.clone())
                .await
                .is_err()
        );
    }
    let mut missing = valid.clone();
    missing.request_id = None;
    let mut session = valid.clone();
    session.session_id = Some("session-a".into());
    let mut other = valid.clone();
    other.work_id = Some("b".into());
    let mut extra = valid.clone();
    extra.review_round_id = Some(f.request.review_round_id.clone());
    for q in [missing, session, other, extra] {
        assert!(f.reads.query(TENANT, PROJECT, SUPERVISOR, q).await.is_err());
    }
    assert!(
        f.reads
            .query("another-tenant", PROJECT, SUPERVISOR, valid.clone())
            .await
            .is_err()
    );
    assert!(
        f.reads
            .query(TENANT, "another-project", SUPERVISOR, valid.clone())
            .await
            .is_err()
    );
    f.admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_read=false,can_write=false WHERE client_id='cli-supervisor'",
        )
        .await
        .unwrap();
    assert!(
        f.reads
            .query(TENANT, PROJECT, SUPERVISOR, valid.clone())
            .await
            .is_err()
    );
    f.admin.batch_execute("UPDATE awr_team.workstream_grants SET can_read=true,can_write=true WHERE client_id='cli-supervisor';
        UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor'").await.unwrap();
    assert!(
        f.reads
            .query(TENANT, PROJECT, SUPERVISOR, valid)
            .await
            .is_err()
    );
    assert_eq!(counts(&f).await, before);
}

#[tokio::test]
async fn connector_discovery_is_work_scoped_bounded_and_contains_no_private_authority() {
    let f = setup_integration().await;
    f.admin.execute("INSERT INTO awr_team.delivery_connectors(tenant_id,project_id,id,scope_id,work_id,workstream_id,
        provider,resource,principal_actor_id,principal_client_id,fact_source,version,coordinator_epoch,enabled,configured_by_actor_id)
        SELECT tenant_id,project_id,'extra-'||n,scope_id,work_id,workstream_id,provider,resource,
        principal_actor_id,principal_client_id,fact_source,version,coordinator_epoch,enabled,configured_by_actor_id
        FROM awr_team.delivery_connectors CROSS JOIN generate_series(1,34) n WHERE id='git'", &[]).await.unwrap();
    f.admin.execute("INSERT INTO awr_team.delivery_connectors(tenant_id,project_id,id,scope_id,work_id,workstream_id,
        provider,resource,principal_actor_id,principal_client_id,fact_source,version,coordinator_epoch,enabled,configured_by_actor_id)
        SELECT tenant_id,project_id,'private-other-work',scope_id,'b-private',$1,provider,'fixture://other-work',
        principal_actor_id,principal_client_id,fact_source,version,coordinator_epoch,enabled,configured_by_actor_id
        FROM awr_team.delivery_connectors WHERE id='git'", &[&awr_core::Id::from(2).to_string()]).await.unwrap();
    let before = counts(&f).await;
    let data = read(&f, SUPERVISOR, "delivery.neutral.inspect", None)
        .await
        .unwrap()["data"]
        .clone();
    assert_eq!(data["connectors"].as_array().unwrap().len(), 32);
    assert_eq!(data["connectors_truncated"], true);
    for c in data["connectors"].as_array().unwrap() {
        assert!(c["connector_version"].is_string());
        assert_eq!(c["current_epoch"], true);
        assert!(c.get("principal_actor_id").is_none());
        assert!(c.get("principal_client_id").is_none());
        assert!(c.get("credential_id").is_none());
    }
    let serialized = serde_json::to_string(&data).unwrap();
    assert!(!serialized.contains("private-other-work"));
    assert!(!serialized.contains("fixture://other-work"));
    for token in [SUPERVISOR, REVIEWER, WORKER] {
        assert!(!serialized.contains(token));
    }
    assert_eq!(counts(&f).await, before);
}
