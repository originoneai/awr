#![cfg(feature = "pg-tests")]
//! Synthetic worker/source mechanisms, not native business acceptance.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/delivery_publication.rs"]
mod publication;

use awr_core::Id;
use awr_team::delivery::{
    DELIVERY_PROTOCOL, DELIVERY_PROTOCOL_VERSION, DeliveryEnvelope, DeliveryRecord, FactProvenance,
    FactSource, VerificationOutcome, VerificationRun,
};
use awr_team_pg::*;
use fixture::*;
use publication::*;
use serde_json::{Value, json};
use std::time::Duration;

async fn queue(f: &Fixture) -> Value {
    f.store
        .sync_intents(TENANT, PROJECT, A, Id::from(1), 64, None)
        .await
        .unwrap()
}

async fn intent(f: &Fixture, kind: &str) -> Value {
    queue(f).await["intents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == kind && i["state"] == "pending")
        .unwrap()
        .clone()
}

fn claim_request(f: &Fixture, row: &Value, key: &str, worker: &str) -> ClaimDeliverySyncIntent {
    ClaimDeliverySyncIntent {
        request_id: key.into(),
        read_set: f.set.clone(),
        intent_id: row["intent_id"].as_str().unwrap().into(),
        worker_id: worker.into(),
        expected_fence: row["fence"].as_str().unwrap().into(),
        lease_seconds: 60,
    }
}

async fn take(f: &Fixture, row: &Value, key: &str, worker: &str) -> DeliverySyncLease {
    f.store
        .claim_sync_intent(TENANT, PROJECT, A, claim_request(f, row, key, worker))
        .await
        .unwrap()
        .unwrap()
}

async fn expire(f: &Fixture, lease: &DeliverySyncLease) {
    f.admin.execute("UPDATE awr_team.delivery_sync_intents SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1",
        &[&lease.intent_id()]).await.unwrap();
}

async fn row(f: &Fixture, lease: &DeliverySyncLease) -> Value {
    queue(f).await["intents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["intent_id"] == lease.intent_id())
        .unwrap()
        .clone()
}

async fn count(f: &Fixture, sql: &str) -> i64 {
    f.admin.query_one(sql, &[]).await.unwrap().get(0)
}

fn inspection(f: &Fixture, key: &str) -> ReserveDeliveryInspection {
    ReserveDeliveryInspection {
        request_id: key.into(),
        read_set: f.set.clone(),
        connector_id: "connector-a".into(),
        connector_version: "1".into(),
        candidate_digest: f.selection.candidate.binding.digest().unwrap(),
        lease_seconds: 60,
    }
}

fn observation(f: &Fixture, reserved: &Value, event: &str) -> IngestDeliveryFacts {
    IngestDeliveryFacts {
        request_id: format!("ingest-a-{event}"),
        read_set: f.set.clone(),
        connector_id: "connector-a".into(),
        inspection_id: reserved["data"]["inspection_id"].as_str().unwrap().into(),
        event_id: event.into(),
        records: vec![DeliveryEnvelope {
            protocol: DELIVERY_PROTOCOL.into(),
            protocol_version: DELIVERY_PROTOCOL_VERSION,
            record: DeliveryRecord::Verification(VerificationRun {
                binding: f.selection.candidate.binding.clone(),
                run_id: "run-a".into(),
                check: "report".into(),
                outcome: VerificationOutcome::Unknown,
                result_artifact: None,
                provenance: FactProvenance {
                    source: FactSource::CallerDeclared,
                    reference: "fixture://checks/result".into(),
                    observed_at_unix_ms: None,
                    recorded_at_unix_ms: 123,
                },
            }),
        }],
    }
}

#[tokio::test]
async fn pending_publication_defers_new_observations_but_preserves_replays_and_other_work() {
    let f = setup_publisher().await;
    f.admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        VALUES($1,$2,'session-d','main','d','agent','cli-a','conversation-d','active',$3,1)",
        &[&TENANT,&PROJECT,&f.set.workstream_id.to_string()]).await.unwrap();
    let other = select(&f.reads, &f.store, "d", "session-d").await;
    let source = take(&f, &intent(&f, "source").await, "claim", "worker-a").await;
    f.store
        .prepare_sync_source(TENANT, PROJECT, A, &source, 60)
        .await
        .unwrap();
    let before = f.bytes();
    let request = inspection(&f, "next-inspection");
    for _ in 0..2 {
        assert!(matches!(
            f.store
                .reserve_inspection(TENANT, PROJECT, A, request.clone())
                .await,
            Err(PgError::ResourceConflict)
        ));
    }
    assert!(denied(
        f.store
            .reserve_inspection(TENANT, PROJECT, B, request.clone())
            .await
    ));
    let initial = f
        .store
        .reserve_inspection(TENANT, PROJECT, A, inspection(&f, "inspect-a-initial"))
        .await
        .unwrap();
    let original = observation(&f, &initial, "initial");
    f.store
        .ingest_facts(TENANT, PROJECT, A, original.clone())
        .await
        .unwrap();
    let mut event_replay = original;
    event_replay.request_id = "same-event-new-request".into();
    assert_eq!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, event_replay)
            .await
            .unwrap()["data"]["event_replayed"],
        true
    );
    assert_eq!(
        count(
            &f,
            "SELECT inspection_generation FROM awr_team.delivery_connectors WHERE work_id='a'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        1
    );
    assert_eq!(f.bytes(), before);
    f.observe(&other, "independent").await;
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_fact_heads WHERE work_id='d'"
        )
        .await,
        1
    );
    assert_eq!(
        f.store
            .process_sync_intent(TENANT, PROJECT, A, &source, 60)
            .await
            .unwrap()["data"]["phase"],
        "confirmed"
    );
    assert_eq!(
        f.store
            .reserve_inspection(TENANT, PROJECT, A, request)
            .await
            .unwrap()["data"]["generation"],
        "2"
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn reserved_observation_waits_for_source_confirmation_without_losing_its_request() {
    let f = setup_publisher().await;
    let reserved = f
        .store
        .reserve_inspection(TENANT, PROJECT, A, inspection(&f, "already-in-flight"))
        .await
        .unwrap();
    let publication = f.prepare("prepare-between-reserve-and-ingest").await;
    let request = observation(&f, &reserved, "after-publication");
    let before = f.bytes();
    assert!(matches!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::ResourceConflict)
    ));
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_inbox").await,
        1
    );
    assert_eq!(f.bytes(), before);
    assert_eq!(
        f.store
            .write_source_publication(TENANT, PROJECT, A, f.step(&publication, "write-first"))
            .await
            .unwrap()["data"]["phase"],
        "source_written"
    );
    assert!(matches!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::ResourceConflict)
    ));
    f.store
        .confirm_source_publication(TENANT, PROJECT, A, f.step(&publication, "confirm-first"))
        .await
        .unwrap();
    assert_eq!(f.status().await["source_synchronized"], true);
    let ingested = f
        .store
        .ingest_facts(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    let replay = f
        .store
        .ingest_facts(TENANT, PROJECT, A, request)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], ingested["data"]);
    assert_eq!(
        replay["committed_project_revision"],
        ingested["committed_project_revision"]
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.delivery_facts").await,
        2
    );
    assert_eq!(f.status().await["source_synchronized"], false);
    let next = queue(&f).await["intents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "source" && i["state"] == "pending" && i["binding_current"] == true)
        .unwrap()
        .clone();
    let next = take(&f, &next, "new-facts", "worker-a").await;
    f.store
        .process_sync_intent(TENANT, PROJECT, A, &next, 60)
        .await
        .unwrap();
    assert_eq!(f.status().await["source_synchronized"], true);
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn lost_source_write_reply_blocks_new_generations_until_fenced_recovery() {
    let f = setup_publisher().await;
    let source = take(&f, &intent(&f, "source").await, "original", "worker-a").await;
    f.store
        .prepare_sync_source(TENANT, PROJECT, A, &source, 60)
        .await
        .unwrap();
    let before = f.bytes();
    f.admin.batch_execute("CREATE FUNCTION awr_team.lose_write_reply() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.event_type='delivery.source.write' THEN RAISE EXCEPTION 'synthetic reply loss'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER lose_write_reply BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.lose_write_reply()").await.unwrap();
    assert!(matches!(
        f.store.write_sync_source(TENANT, PROJECT, A, &source).await,
        Err(PgError::Db(_))
    ));
    let landed = f.bytes();
    assert_ne!(landed, before);
    let modified = std::fs::metadata(f.root.join("ledger.yaml"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(f.status().await["history"][0]["phase"], "pending");
    f.admin.batch_execute("DROP TRIGGER lose_write_reply ON awr_team.events;
        UPDATE awr_team.delivery_source_publications SET expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
    expire(&f, &source).await;
    let request = inspection(&f, "wait-for-unknown-effect");
    assert!(matches!(
        f.store
            .reserve_inspection(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::ResourceConflict)
    ));
    assert_eq!(
        count(
            &f,
            "SELECT inspection_generation FROM awr_team.delivery_connectors"
        )
        .await,
        1
    );
    let recovered = take(&f, &row(&f, &source).await, "recover-original", "worker-b").await;
    let restarted = f.restarted();
    assert_eq!(
        restarted
            .process_sync_intent(TENANT, PROJECT, A, &recovered, 60)
            .await
            .unwrap()["data"]["phase"],
        "confirmed"
    );
    assert_eq!(f.bytes(), landed);
    assert_eq!(
        std::fs::metadata(f.root.join("ledger.yaml"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert_eq!(f.status().await["source_synchronized"], true);
    assert_eq!(
        f.store
            .reserve_inspection(TENANT, PROJECT, A, request)
            .await
            .unwrap()["data"]["generation"],
        "2"
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        1
    );
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

#[tokio::test]
async fn concurrent_observation_and_source_preparation_cannot_strand_a_journal() {
    let f = setup_publisher().await;
    let source = take(&f, &intent(&f, "source").await, "claim", "worker-a").await;
    let (publication, observation) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            f.store.prepare_sync_source(TENANT, PROJECT, A, &source, 60),
            f.store
                .reserve_inspection(TENANT, PROJECT, A, inspection(&f, "concurrent"))
        )
    })
    .await
    .unwrap();
    match (publication, observation) {
        (Ok(_), Err(PgError::ResourceConflict)) => {
            f.store
                .process_sync_intent(TENANT, PROJECT, A, &source, 60)
                .await
                .unwrap();
            assert_eq!(f.status().await["source_synchronized"], true);
        }
        (Err(PgError::PreconditionsChanged), Ok(_)) => {
            assert_eq!(
                count(
                    &f,
                    "SELECT count(*) FROM awr_team.delivery_source_publications"
                )
                .await,
                0
            );
            assert_eq!(f.status().await["pending_publication_id"], Value::Null);
        }
        results => panic!("unexpected source/observation admission: {results:?}"),
    }
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
}

fn denied<T>(result: PgResult<T>) -> bool {
    matches!(
        result,
        Err(PgError::Forbidden | PgError::Workstream(awr_core::WorkstreamError::AccessDenied))
    )
}

#[tokio::test]
async fn refresh_and_confirmed_source_are_distinct_and_create_no_approval() {
    let f = setup_publisher().await;
    let before = f.bytes();
    let rows = queue(&f).await;
    assert_eq!(rows["intents"].as_array().unwrap().len(), 2);
    assert!(
        rows["intents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["binding_current"] == true)
    );
    let refresh = take(
        &f,
        &intent(&f, "refresh").await,
        "claim-refresh",
        "worker-a",
    )
    .await;
    let result = f
        .store
        .process_sync_intent(TENANT, PROJECT, A, &refresh, 60)
        .await
        .unwrap();
    assert_eq!(result["data"]["refresh_available"], true);
    assert_eq!(result["source_synchronized"], false);
    assert_eq!(result["acceptance_ready"], false);
    assert_eq!(f.bytes(), before);
    assert_eq!(f.status().await["source_synchronized"], false);
    let source = take(&f, &intent(&f, "source").await, "claim-source", "worker-a").await;
    let confirmed = tokio::time::timeout(
        Duration::from_secs(10),
        f.store.process_sync_intent(TENANT, PROJECT, A, &source, 60),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(confirmed["data"]["phase"], "confirmed");
    assert_eq!(row(&f, &source).await["state"], "succeeded");
    assert_eq!(f.status().await["source_synchronized"], true);
    assert_eq!(
        count(&f, "SELECT count(*) FROM awr_team.completion_receipts").await,
        0
    );
    let text = String::from_utf8(f.bytes()).unwrap();
    assert_eq!(text.matches("status: planned").count(), 4);
    assert!(text.starts_with("# Preserve this comment"));
    let audit: Value = f
        .admin
        .query_one(
            "SELECT jsonb_agg(to_jsonb(a)) FROM awr_team.ops_audit_records a",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    for value in [
        serde_json::to_string(&queue(&f).await).unwrap(),
        audit.to_string(),
        text,
    ] {
        assert!(!value.contains(A));
        assert!(!value.contains(B));
    }
}

#[tokio::test]
async fn concurrent_claims_and_stable_retry_have_one_effective_worker() {
    let f = setup_publisher().await;
    let i = intent(&f, "refresh").await;
    let a = claim_request(&f, &i, "claim-a", "worker-a");
    let b = claim_request(&f, &i, "claim-b", "worker-b");
    let (one, two) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            f.store.claim_sync_intent(TENANT, PROJECT, A, a),
            f.store.claim_sync_intent(TENANT, PROJECT, A, b)
        )
    })
    .await
    .unwrap();
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert!(matches!(
        one.as_ref().err().or(two.as_ref().err()),
        Some(PgError::StaleFence)
    ));
    let i = intent(&f, "source").await;
    let request = claim_request(&f, &i, "stable-claim", "worker-a");
    let (one, two) = tokio::join!(
        f.store
            .claim_sync_intent(TENANT, PROJECT, A, request.clone()),
        f.store
            .claim_sync_intent(TENANT, PROJECT, A, request.clone())
    );
    let lease = one.unwrap().unwrap();
    assert_eq!(lease.fence(), two.unwrap().unwrap().fence());
    assert_eq!(
        count(
            &f,
            "SELECT sum(attempts)::bigint FROM awr_team.delivery_sync_intents"
        )
        .await,
        2
    );
    let mut changed = request.clone();
    changed.worker_id = "worker-b".into();
    assert!(matches!(
        f.store.claim_sync_intent(TENANT, PROJECT, A, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    expire(&f, &lease).await;
    assert!(matches!(
        f.store.claim_sync_intent(TENANT, PROJECT, A, request).await,
        Err(PgError::LeaseExpired)
    ));
}

#[tokio::test]
async fn a_new_worker_fence_invalidates_old_acknowledgement_and_live_publication() {
    let f = setup_publisher().await;
    let refresh = take(&f, &intent(&f, "refresh").await, "refresh-a", "worker-a").await;
    let source = take(&f, &intent(&f, "source").await, "source-a", "worker-a").await;
    let step = f
        .store
        .prepare_sync_source(TENANT, PROJECT, A, &source, 300)
        .await
        .unwrap();
    f.store
        .write_sync_source(TENANT, PROJECT, A, &source)
        .await
        .unwrap();
    let landed = f.bytes();
    expire(&f, &source).await;
    expire(&f, &refresh).await;
    let source_b = take(&f, &row(&f, &source).await, "source-b", "worker-b").await;
    let refresh_b = take(&f, &row(&f, &refresh).await, "refresh-b", "worker-b").await;
    assert_eq!(source_b.fence(), "2");
    assert!(matches!(
        f.store
            .confirm_sync_source(TENANT, PROJECT, A, &source)
            .await,
        Err(PgError::StaleFence)
    ));
    assert!(matches!(
        f.store
            .acknowledge_sync_refresh(TENANT, PROJECT, A, &refresh)
            .await,
        Err(PgError::StaleFence)
    ));
    // A still-live publication lease is insufficient without the current worker.
    assert!(denied(
        f.store
            .confirm_source_publication(TENANT, PROJECT, A, step)
            .await
    ));
    f.restarted()
        .process_sync_intent(TENANT, PROJECT, A, &source_b, 60)
        .await
        .unwrap();
    f.restarted()
        .acknowledge_sync_refresh(TENANT, PROJECT, A, &refresh_b)
        .await
        .unwrap();
    assert_eq!(f.bytes(), landed);
    assert_eq!(row(&f, &source_b).await["state"], "succeeded");
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn direct_publication_apis_cannot_omit_the_worker_capability() {
    let f = setup_publisher().await;
    let lease = take(&f, &intent(&f, "source").await, "source", "worker-a").await;
    let step = f
        .store
        .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
        .await
        .unwrap();
    let before = f.bytes();
    assert!(denied(
        f.store
            .write_source_publication(TENANT, PROJECT, A, step.clone())
            .await
    ));
    assert!(denied(
        f.store
            .confirm_source_publication(TENANT, PROJECT, A, step.clone())
            .await
    ));
    assert!(denied(
        f.store
            .abandon_source_publication(TENANT, PROJECT, A, step.clone())
            .await
    ));
    assert!(denied(
        f.store
            .renew_source_publication(
                TENANT,
                PROJECT,
                A,
                RenewDeliveryPublicationLease {
                    step,
                    lease_seconds: 60
                }
            )
            .await
    ));
    assert_eq!(f.bytes(), before);
    assert!(denied(
        f.store
            .process_sync_intent("other-tenant", PROJECT, A, &lease, 60)
            .await
    ));
    assert!(denied(
        f.store
            .process_sync_intent(TENANT, "other-project", A, &lease, 60)
            .await
    ));
    assert!(denied(
        f.store
            .process_sync_intent(TENANT, PROJECT, B, &lease, 60)
            .await
    ));
    f.store
        .process_sync_intent(TENANT, PROJECT, A, &lease, 60)
        .await
        .unwrap();
}

#[tokio::test]
async fn expiry_inside_effect_transactions_rolls_back_preparation_ack_and_confirmation() {
    for operation in ["prepare", "ack", "confirm"] {
        let f = setup_publisher().await;
        let kind = if operation == "ack" {
            "refresh"
        } else {
            "source"
        };
        let lease = take(&f, &intent(&f, kind).await, "claim", "worker-a").await;
        if operation == "confirm" {
            f.store
                .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
                .await
                .unwrap();
            f.store
                .write_sync_source(TENANT, PROJECT, A, &lease)
                .await
                .unwrap();
        }
        let before = f.bytes();
        f.admin.execute("CREATE FUNCTION awr_team.expire_pump_during_effect() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN UPDATE awr_team.delivery_sync_intents SET expires_at=clock_timestamp()-interval '1 second'; RETURN NEW; END $$", &[])
            .await.unwrap();
        let trigger = match operation {
            "prepare" => {
                "CREATE TRIGGER expire_pump BEFORE INSERT ON awr_team.delivery_source_publications FOR EACH ROW EXECUTE FUNCTION awr_team.expire_pump_during_effect()"
            }
            "ack" => {
                "CREATE TRIGGER expire_pump BEFORE UPDATE ON awr_team.delivery_notifications FOR EACH ROW EXECUTE FUNCTION awr_team.expire_pump_during_effect()"
            }
            _ => {
                "CREATE TRIGGER expire_pump BEFORE UPDATE ON awr_team.delivery_source_publications FOR EACH ROW WHEN (NEW.phase='confirmed') EXECUTE FUNCTION awr_team.expire_pump_during_effect()"
            }
        };
        f.admin.batch_execute(trigger).await.unwrap();
        let result = match operation {
            "prepare" => f
                .store
                .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
                .await
                .map(|_| json!(null)),
            "ack" => {
                f.store
                    .acknowledge_sync_refresh(TENANT, PROJECT, A, &lease)
                    .await
            }
            _ => {
                f.store
                    .confirm_sync_source(TENANT, PROJECT, A, &lease)
                    .await
            }
        };
        assert!(
            matches!(result, Err(PgError::LeaseExpired)),
            "expiry must be rechecked inside {operation}: {result:?}"
        );
        assert_eq!(f.bytes(), before);
        assert_eq!(row(&f, &lease).await["state"], "leased");
        assert_eq!(
            count(
                &f,
                "SELECT count(*) FROM awr_team.delivery_notifications WHERE state='pending'"
            )
            .await,
            1
        );
        if operation == "prepare" {
            assert_eq!(
                count(
                    &f,
                    "SELECT count(*) FROM awr_team.delivery_source_publications"
                )
                .await,
                0
            );
            assert!(row(&f, &lease).await["publication_id"].is_null());
        } else if operation == "confirm" {
            assert_eq!(f.status().await["history"][0]["phase"], "source_written");
            assert_eq!(f.status().await["source_synchronized"], false);
        }
    }
}

#[tokio::test]
async fn a_missing_response_after_real_source_effect_recovers_without_another_write() {
    let f = setup_publisher().await;
    let lease = take(&f, &intent(&f, "source").await, "source-a", "worker-a").await;
    let p = f
        .store
        .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
        .await
        .unwrap();
    f.admin.batch_execute("CREATE FUNCTION awr_team.fail_pump_event() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.event_type='delivery.source.write' THEN RAISE EXCEPTION 'synthetic event failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER fail_pump_event BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.fail_pump_event()").await.unwrap();
    assert!(matches!(
        f.store.write_sync_source(TENANT, PROJECT, A, &lease).await,
        Err(PgError::Db(_))
    ));
    let landed = f.bytes();
    let history = f.status().await;
    assert_eq!(history["history"][0]["phase"], "pending");
    assert_eq!(
        landed,
        f.after_bytes(&json!({"publication_id":p.publication_id}))
            .await
    );
    let metadata = std::fs::metadata(f.root.join("ledger.yaml")).unwrap();
    f.admin
        .batch_execute("DROP TRIGGER fail_pump_event ON awr_team.events")
        .await
        .unwrap();
    expire(&f, &lease).await;
    let next = take(
        &f,
        &row(&f, &lease).await,
        "source-restart",
        "worker-after-restart",
    )
    .await;
    let result = f
        .restarted()
        .process_sync_intent(TENANT, PROJECT, A, &next, 60)
        .await
        .unwrap();
    assert_eq!(result["data"]["phase"], "confirmed");
    assert_eq!(f.bytes(), landed);
    let recovered = std::fs::metadata(f.root.join("ledger.yaml")).unwrap();
    assert_eq!(metadata.modified().unwrap(), recovered.modified().unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata.ino(), recovered.ino());
    }
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        1
    );
    assert_eq!(row(&f, &next).await["state"], "succeeded");
}

#[tokio::test]
async fn revoked_credentials_grants_and_non_manager_agents_cannot_settle_a_lease() {
    for change in [
        "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'",
        "UPDATE awr_team.workstream_grants SET can_manage=false,grant_version=grant_version+1 WHERE client_id='cli-a'",
        "UPDATE awr_team.actors SET kind='agent' WHERE id='agent'",
    ] {
        let f = setup_publisher().await;
        let lease = take(&f, &intent(&f, "source").await, "source-a", "worker-a").await;
        f.store
            .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
            .await
            .unwrap();
        let before = f.bytes();
        f.admin.batch_execute(change).await.unwrap();
        assert!(denied(
            f.store
                .process_sync_intent(TENANT, PROJECT, A, &lease, 60)
                .await
        ));
        assert_eq!(f.bytes(), before);
        assert_eq!(
            count(
                &f,
                "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE state='succeeded'"
            )
            .await,
            0
        );
    }
}

#[tokio::test]
async fn changed_connector_selection_execution_source_ownership_and_epoch_fail_closed() {
    for change in [
        "UPDATE awr_team.delivery_connectors SET version=version+1",
        "UPDATE awr_team.delivery_connectors SET enabled=false",
        "UPDATE awr_team.delivery_connectors SET inspection_generation=inspection_generation+1",
        "UPDATE awr_team.delivery_selections SET selection_version=selection_version+1",
        "UPDATE awr_team.work_runtime SET last_fence=last_fence+1 WHERE work_id='a'",
        "UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=ownership_version+1 WHERE work_id='a'",
        "UPDATE awr_team.projects SET coordinator_epoch='changed-epoch' WHERE tenant_id='reader-tenant'",
    ] {
        let f = setup_publisher().await;
        let lease = take(&f, &intent(&f, "source").await, "source-a", "worker-a").await;
        f.store
            .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
            .await
            .unwrap();
        let before = f.bytes();
        f.admin.batch_execute(change).await.unwrap();
        assert!(matches!(
            f.store
                .process_sync_intent(TENANT, PROJECT, A, &lease, 60)
                .await,
            Err(PgError::PreconditionsChanged | PgError::EpochChanged)
        ));
        assert_eq!(f.bytes(), before);
        // An associated journal remains observable; it is never discarded to
        // make an unknown effect eligible for a fresh write.
        assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE publication_id IS NOT NULL AND state='leased'").await, 1);
    }
}

#[tokio::test]
async fn stale_unwritten_intents_are_superseded_but_bound_publications_are_retained() {
    let f = setup_publisher().await;
    let refresh = intent(&f, "refresh").await;
    f.admin
        .batch_execute("UPDATE awr_team.delivery_connectors SET enabled=false")
        .await
        .unwrap();
    assert!(
        f.store
            .claim_sync_intent(
                TENANT,
                PROJECT,
                A,
                claim_request(&f, &refresh, "stale", "worker-a")
            )
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        queue(&f).await["intents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["intent_id"] == refresh["intent_id"])
            .unwrap()["state"],
        "superseded"
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_notifications WHERE state='pending'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) FROM awr_team.delivery_source_publications"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn backoff_input_bounds_scoped_pagination_and_rls_are_enforced() {
    let f = setup_publisher().await;
    let first = f
        .store
        .sync_intents(TENANT, PROJECT, A, Id::from(1), 1, None)
        .await
        .unwrap();
    assert_eq!(first["intents"].as_array().unwrap().len(), 1);
    assert_eq!(first["has_more"], true);
    let second = f
        .store
        .sync_intents(
            TENANT,
            PROJECT,
            A,
            Id::from(1),
            1,
            first["next_after"].as_str(),
        )
        .await
        .unwrap();
    assert_eq!(second["has_more"], false);
    assert_ne!(
        first["intents"][0]["intent_id"],
        second["intents"][0]["intent_id"]
    );
    assert!(denied(
        f.store
            .sync_intents(
                TENANT,
                PROJECT,
                A,
                Id::from(2),
                1,
                first["next_after"].as_str()
            )
            .await
    ));
    assert!(denied(
        f.store
            .sync_intents(TENANT, PROJECT, B, Id::from(1), 1, None)
            .await
    ));
    for limit in [0, 65] {
        assert!(matches!(
            f.store
                .sync_intents(TENANT, PROJECT, A, Id::from(1), limit, None)
                .await,
            Err(PgError::Protocol(_))
        ));
    }
    let i = intent(&f, "source").await;
    for (fence, ttl) in [("01", 60), ("-1", 60), ("0", 4), ("0", 301)] {
        let mut request = claim_request(&f, &i, "invalid", "worker-a");
        request.expected_fence = fence.into();
        request.lease_seconds = ttl;
        assert!(matches!(
            f.store.claim_sync_intent(TENANT, PROJECT, A, request).await,
            Err(PgError::Protocol(_))
        ));
    }
    let lease = take(&f, &i, "claim", "worker-a").await;
    assert!(matches!(
        f.store
            .defer_sync_intent(TENANT, PROJECT, A, &lease, "raw private error", 1)
            .await,
        Err(PgError::Protocol(_))
    ));
    f.store
        .defer_sync_intent(TENANT, PROJECT, A, &lease, "source_unavailable", 3600)
        .await
        .unwrap();
    let blocked = row(&f, &lease).await;
    assert_eq!(blocked["failure_code"], "source_unavailable");
    assert_eq!(blocked["retry_due"], false);
    assert!(matches!(
        f.store
            .claim_sync_intent(
                TENANT,
                PROJECT,
                A,
                claim_request(&f, &blocked, "too-early", "worker-b")
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    f.admin.batch_execute("UPDATE awr_team.delivery_sync_intents SET retry_at=clock_timestamp()-interval '1 second'").await.unwrap();
    let next = take(&f, &row(&f, &lease).await, "due-retry", "worker-b").await;
    assert_eq!(next.fence(), "2");
    f.store
        .process_sync_intent(TENANT, PROJECT, A, &next, 60)
        .await
        .unwrap();
    let (app, connection) = f.config.connect(tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    app.batch_execute("BEGIN; SELECT set_config('awr.tenant_id','other-tenant',true); SELECT set_config('awr.project_id','reader-project',true)").await.unwrap();
    assert_eq!(
        app.query_one("SELECT count(*) FROM awr_team.delivery_sync_intents", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    app.batch_execute("ROLLBACK").await.unwrap();
    assert!(f.admin.query_one("SELECT relrowsecurity AND relforcerowsecurity FROM pg_class WHERE oid='awr_team.delivery_sync_intents'::regclass", &[]).await.unwrap().get::<_, bool>(0));
}

#[tokio::test]
async fn source_drift_is_preserved_and_failed_worker_can_defer_without_discarding_its_journal() {
    let f = setup_publisher().await;
    let lease = take(&f, &intent(&f, "source").await, "source-a", "worker-a").await;
    f.store
        .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
        .await
        .unwrap();
    let mut changed = f.bytes();
    changed.extend_from_slice(b"\n# External source edit\n");
    std::fs::write(f.root.join("ledger.yaml"), &changed).unwrap();
    let result = f
        .store
        .write_sync_source(TENANT, PROJECT, A, &lease)
        .await
        .unwrap();
    assert_eq!(result["data"]["phase"], "conflict");
    assert_eq!(f.bytes(), changed);
    f.store
        .defer_sync_intent(TENANT, PROJECT, A, &lease, "source_conflict", 60)
        .await
        .unwrap();
    assert_eq!(row(&f, &lease).await["state"], "blocked");
    assert!(!row(&f, &lease).await["publication_id"].is_null());
    assert_eq!(f.status().await["source_synchronized"], false);
}

#[tokio::test]
async fn schema44_pending_notifications_backfill_exact_historical_bindings_atomically() {
    let f = setup_publisher().await;
    let before: Value = f
        .admin
        .query_one(
            "SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM awr_team.delivery_notifications n",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    f.admin.batch_execute("DROP TABLE awr_team.delivery_integration_target_guards,awr_team.delivery_integration_intents,awr_team.delivery_sync_intents;
        UPDATE awr_team.schema_state SET version=44").await.unwrap();
    let ddl = include_str!("../migrations/20261006000045_delivery_sync_pump.sql");
    assert!(
        f.admin
            .batch_execute(&ddl.replace(
                "UPDATE awr_team.schema_state",
                "SELECT 1/0; UPDATE awr_team.schema_state"
            ))
            .await
            .is_err()
    );
    f.admin.batch_execute("ROLLBACK").await.unwrap();
    let rolled_back = f.admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),to_regclass('awr_team.delivery_sync_intents')::text", &[]).await.unwrap();
    assert_eq!(rolled_back.get::<_, i32>(0), 44);
    assert!(rolled_back.get::<_, Option<String>>(1).is_none());
    migrate(&f.admin).await.unwrap();
    check_schema(&f.admin).await.unwrap();
    // Test-role SQL privileges are installed by the fixture after migration;
    // recreated tables need the same bootstrap without changing domain grants.
    f.admin
        .batch_execute(
            "GRANT SELECT,INSERT,UPDATE,DELETE ON awr_team.delivery_sync_intents TO awr_app",
        )
        .await
        .unwrap();
    let after = queue(&f).await;
    assert_eq!(after["intents"].as_array().unwrap().len(), 2);
    for i in after["intents"].as_array().unwrap() {
        assert_eq!(i["read_set"], json!(f.set));
        assert_eq!(i["binding_current"], true);
        assert_eq!(i["fence"], "0");
        assert_eq!(i["state"], "pending");
    }
    let unchanged: Value = f
        .admin
        .query_one(
            "SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM awr_team.delivery_notifications n",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, unchanged);
    migrate(&f.admin).await.unwrap();
    assert_eq!(queue(&f).await, after);
    let lease = take(&f, &intent(&f, "source").await, "backfilled", "worker-a").await;
    f.store
        .process_sync_intent(TENANT, PROJECT, A, &lease, 60)
        .await
        .unwrap();
}

#[tokio::test]
async fn credential_expiry_during_confirmation_cannot_settle_a_live_worker() {
    let f = setup_publisher().await;
    let lease = take(&f, &intent(&f, "source").await, "source-a", "worker-a").await;
    f.store
        .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
        .await
        .unwrap();
    f.store
        .write_sync_source(TENANT, PROJECT, A, &lease)
        .await
        .unwrap();
    let landed = f.bytes();
    f.admin.batch_execute("CREATE FUNCTION awr_team.expire_credential() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'; RETURN NEW; END $$;
        CREATE TRIGGER expire_credential BEFORE UPDATE ON awr_team.delivery_source_publications
        FOR EACH ROW WHEN (NEW.phase='confirmed') EXECUTE FUNCTION awr_team.expire_credential()").await.unwrap();
    assert!(denied(
        f.store
            .confirm_sync_source(TENANT, PROJECT, A, &lease)
            .await
    ));
    assert_eq!(f.bytes(), landed);
    assert_eq!(row(&f, &lease).await["state"], "leased");
    assert_eq!(f.status().await["history"][0]["phase"], "source_written");
    assert_eq!(f.status().await["source_synchronized"], false);
}

#[tokio::test]
async fn actual_source_activation_invalidates_old_worker_without_overwriting_new_source() {
    use awr_source::{PublishPrepOptions, prepare_publish_from_server_directory};
    let f = setup_publisher().await;
    let lease = take(&f, &intent(&f, "source").await, "source-a", "worker-a").await;
    f.store
        .prepare_sync_source(TENANT, PROJECT, A, &lease, 60)
        .await
        .unwrap();
    let changed = String::from_utf8(f.bytes()).unwrap().replace(
        "Synthetic publication fixture",
        "Updated publication fixture",
    );
    std::fs::write(f.root.join("ledger.yaml"), &changed).unwrap();
    let package = prepare_publish_from_server_directory(
        &f.root,
        "ledger.yaml",
        PROJECT,
        &PublishPrepOptions::default(),
    )
    .unwrap();
    let source = SourceStore::from_config(f.config.clone());
    let (candidate, _) = source
        .ingest_publish_candidate(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: package.parser_version,
            files: package
                .files
                .into_iter()
                .map(|p| SourceFile {
                    path: p.path,
                    bytes: p.bytes,
                })
                .collect(),
        })
        .await
        .unwrap();
    source
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            "reviewer",
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    let prepared = prepare(&f.reads, A, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "release-before-source-activation",
                "claim.release",
                json!({
                    "session_id":"session-a","expected_session_version":"1",
                    "claim_id":f.selection.claim_id,"expected_fence":f.selection.fence,
                    "expected_lease_version":f.selection.lease_version,
                }),
            ),
        )
        .await
        .unwrap();
    source
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &candidate.proposal_id,
            &awr_team::SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                parser_version: candidate.parser_version,
                expected_authority_epoch: candidate.base_epoch,
                approved_candidate_digest: candidate.manifest_digest,
            },
        )
        .await
        .unwrap();
    let snapshot: String = f
        .admin
        .query_one(
            "SELECT active_snapshot_id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_ne!(snapshot, f.set.source_snapshot_id);
    assert!(matches!(
        f.store
            .process_sync_intent(TENANT, PROJECT, A, &lease, 60)
            .await,
        Err(PgError::EpochChanged | PgError::PreconditionsChanged)
    ));
    assert_eq!(f.bytes(), changed.as_bytes());
    assert_eq!(count(&f, "SELECT count(*) FROM awr_team.delivery_sync_intents WHERE publication_id IS NOT NULL AND state='leased'").await, 1);
}
