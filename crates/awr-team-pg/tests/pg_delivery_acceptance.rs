#![cfg(feature = "pg-tests")]
//! Actual source/command/publication regressions; no native or repository-effect credit.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/delivery_integration.rs"]
mod integration_fixture;

#[path = "fixtures/delivery_acceptance.rs"]
mod acceptance_fixture;
use acceptance_fixture::*;
use awr_team_pg::*;
use fixture::*;
use integration_fixture::*;
use serde_json::{Value, json};

#[tokio::test]
async fn actual_acceptance_atomically_schedules_receipt_backed_source_sync_without_observers() {
    for simulated in [false, true] {
        let f = SourceFixture::new(simulated).await;
        let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
        let completed = f.finalize("domain-acceptance").await;
        let queue =
            f.f.store
                .sync_intents(TENANT, PROJECT, WORKER, awr_core::Id::from(1), 64, None)
                .await
                .unwrap();
        let rows = queue["intents"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            2,
            "Actual acceptance must schedule refresh and source without an external event"
        );
        assert!(rows.iter().all(|i| i["origin"] == "domain_acceptance"
            && i["completion_receipt_id"] == completed["receipt_id"]
            && i["binding_current"] == true
            && i["connector_id"].is_null()));
        assert_eq!(std::fs::read(f.root.0.join("ledger.yaml")).unwrap(), before);
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
async fn concurrent_exact_finalization_replays_one_receipt_and_one_domain_queue() {
    let f = SourceFixture::new(true).await;
    let p = prepare(&f.f.reads, SUPERVISOR, "a").await;
    let request = command(
        &p,
        "concurrent-finalize",
        "delivery.finalize",
        json!({"session_id":"session-supervisor","expected_session_version":"1",
            "evidence_id":f.f.evidence["evidence_id"],"context_complete":true}),
    );
    let commands = f.f.reads.commands();
    let (left, right) = tokio::join!(
        commands.execute(TENANT, PROJECT, SUPERVISOR, request.clone()),
        commands.execute(TENANT, PROJECT, SUPERVISOR, request)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left["receipt"]["data"], right["receipt"]["data"]);
    assert_eq!(f.queue().await["intents"].as_array().unwrap().len(), 2);
    for (table, count) in [
        ("completion_receipts", 1),
        ("delivery_notifications", 1),
        ("delivery_sync_intents", 2),
    ] {
        assert_eq!(
            f.f.admin
                .query_one(&format!("SELECT count(*) FROM awr_team.{table}"), &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            count
        );
    }
}

#[tokio::test]
async fn failed_domain_enqueue_rolls_back_actual_acceptance_and_all_queue_records() {
    let f = SourceFixture::new(false).await;
    let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    f.f.admin
        .batch_execute(
            "CREATE FUNCTION awr_team.reject_domain_queue() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'Synthetic domain queue transaction failure'; END $$;
        CREATE TRIGGER reject_domain_queue BEFORE INSERT ON awr_team.delivery_sync_intents
        FOR EACH ROW EXECUTE FUNCTION awr_team.reject_domain_queue()",
        )
        .await
        .unwrap();
    assert!(matches!(
        f.try_finalize("failed-queue").await,
        Err(PgError::Db(_))
    ));
    f.assert_no_completion().await;
    assert_eq!(std::fs::read(f.root.0.join("ledger.yaml")).unwrap(), before);
    f.f.admin
        .batch_execute("DROP TRIGGER reject_domain_queue ON awr_team.delivery_sync_intents")
        .await
        .unwrap();
    assert_eq!(
        f.finalize("recovered-queue").await["source_sync"]["state"],
        "queued"
    );
}

#[tokio::test]
async fn original_domain_receipt_reaches_source_after_reconstruction_and_lost_confirmation() {
    let f = SourceFixture::new(true).await;
    let completed = f.finalize("domain-finalize").await;
    let refresh = f.take("refresh", "refresh").await;
    let rebuilt = DeliverySyncStore::from_config(f.f.config.clone());
    let acknowledged = rebuilt
        .process_sync_intent(TENANT, PROJECT, WORKER, &refresh, 60)
        .await
        .unwrap();
    assert_eq!(acknowledged["data"]["refresh_available"], true);
    assert_eq!(acknowledged["data"]["source_synchronized"], false);
    let lease = f.take("source", "source").await;
    let journal = rebuilt
        .prepare_sync_source(TENANT, PROJECT, WORKER, &lease, 60)
        .await
        .unwrap();
    rebuilt
        .write_sync_source(TENANT, PROJECT, WORKER, &lease)
        .await
        .unwrap();
    let written = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    f.f.admin.batch_execute("UPDATE awr_team.delivery_sync_intents SET expires_at=clock_timestamp()-interval '1 second' WHERE kind='source';
        UPDATE awr_team.delivery_source_publications SET expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
    let renewed = f.take("source-reconnected", "source").await;
    assert_ne!(renewed.fence(), lease.fence());
    assert!(matches!(
        rebuilt
            .confirm_sync_source(TENANT, PROJECT, WORKER, &lease)
            .await,
        Err(PgError::StaleFence)
    ));
    let confirmed = rebuilt
        .process_sync_intent(TENANT, PROJECT, WORKER, &renewed, 60)
        .await
        .unwrap();
    assert_eq!(confirmed["data"]["phase"], "confirmed");
    assert_eq!(confirmed["data"]["publication_id"], journal.publication_id);
    assert_eq!(
        confirmed["data"]["confirmation"]["completion_reference"]["receipt_id"],
        completed["receipt_id"]
    );
    assert_eq!(
        std::fs::read(f.root.0.join("ledger.yaml")).unwrap(),
        written
    );
    assert_eq!(
        f.f.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert_eq!(
        f.queue().await["intents"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["state"] == "succeeded")
            .count(),
        2
    );
}

#[tokio::test]
async fn only_exact_pending_domain_work_continues_across_unrelated_source_activation() {
    for before_acceptance in [false, true] {
        let f = SourceFixture::new(true).await;
        let completed = if before_acceptance {
            f.publish_successor(Some("c")).await;
            f.finalize("after-unrelated-source").await
        } else {
            let completed = f.finalize("before-unrelated-source").await;
            f.publish_successor(Some("c")).await;
            completed
        };
        let current = prepare(&f.f.reads, WORKER, "a").await;
        let queue = f.queue().await;
        assert!(
            queue["intents"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| i["binding_current"] == true
                    && i["read_set"]["source_snapshot_id"] == current["source_snapshot_id"])
        );
        let selection: String =
            f.f.admin
                .query_one(
                    "SELECT source_snapshot_id FROM awr_team.delivery_selections WHERE work_id='a'",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
        assert_eq!(
            selection, f.f.set.source_snapshot_id,
            "Continuity must not rewrite the original reviewed selection"
        );
        let lease = f.take("continued-source", "source").await;
        let confirmed =
            f.f.store
                .process_sync_intent(TENANT, PROJECT, WORKER, &lease, 60)
                .await
                .unwrap();
        assert_eq!(confirmed["data"]["phase"], "confirmed");
        assert_eq!(
            confirmed["data"]["confirmation"]["completion_reference"]["receipt_id"],
            completed["receipt_id"]
        );
    }
}

#[tokio::test]
async fn domain_source_queue_refuses_changed_contracts_and_retains_original_bindings() {
    let f = SourceFixture::new(false).await;
    f.finalize("before-changed-source").await;
    let original = f.queue().await;
    f.publish_successor(Some("a")).await;
    let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    let queue = f.queue().await;
    assert!(
        queue["intents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["binding_current"] == false)
    );
    for (old, new) in original["intents"]
        .as_array()
        .unwrap()
        .iter()
        .zip(queue["intents"].as_array().unwrap())
    {
        assert_eq!(old["read_set"], new["read_set"]);
        assert_eq!(old["completion_receipt_id"], new["completion_receipt_id"]);
    }
    assert_eq!(std::fs::read(f.root.0.join("ledger.yaml")).unwrap(), before);
    assert_eq!(
        f.f.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn existing_domain_publication_is_never_rebound_after_an_unrelated_activation() {
    let f = SourceFixture::new(false).await;
    f.finalize("domain-before-journal").await;
    let lease = f.take("domain-journal", "source").await;
    let journal =
        f.f.store
            .prepare_sync_source(TENANT, PROJECT, WORKER, &lease, 60)
            .await
            .unwrap();
    f.publish_successor(Some("c")).await;
    let current_bytes = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    let queue = f.queue().await;
    let source = queue["intents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "source")
        .unwrap();
    assert_eq!(source["binding_current"], false);
    assert_eq!(
        source["read_set"]["source_snapshot_id"],
        f.f.set.source_snapshot_id
    );
    assert_eq!(source["publication_id"], journal.publication_id);
    assert!(matches!(
        f.f.store
            .process_sync_intent(TENANT, PROJECT, WORKER, &lease, 60)
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        std::fs::read(f.root.0.join("ledger.yaml")).unwrap(),
        current_bytes
    );
    assert_eq!(
        f.f.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn domain_source_conflict_retains_the_same_unknown_journal_and_external_bytes() {
    let f = SourceFixture::new(true).await;
    f.finalize("domain-conflict").await;
    let lease = f.take("domain-conflict-source", "source").await;
    let journal =
        f.f.store
            .prepare_sync_source(TENANT, PROJECT, WORKER, &lease, 60)
            .await
            .unwrap();
    f.f.store
        .write_sync_source(TENANT, PROJECT, WORKER, &lease)
        .await
        .unwrap();
    let path = f.root.0.join("ledger.yaml");
    let text = std::fs::read_to_string(&path).unwrap() + "\n# Concurrent external edit.\n";
    std::fs::write(&path, &text).unwrap();
    let conflict =
        f.f.store
            .confirm_sync_source(TENANT, PROJECT, WORKER, &lease)
            .await
            .unwrap();
    assert_eq!(conflict["data"]["phase"], "conflict");
    f.f.store
        .defer_sync_intent(TENANT, PROJECT, WORKER, &lease, "source_conflict", 1)
        .await
        .unwrap();
    let queue = f.queue().await;
    let source = queue["intents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "source")
        .unwrap();
    assert_eq!(source["state"], "blocked");
    assert_eq!(source["publication_id"], journal.publication_id);
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
    let pending: Option<String> =
        f.f.admin
            .query_one(
                "SELECT pending_publication_id FROM awr_team.delivery_source_cursors",
                &[],
            )
            .await
            .unwrap()
            .get(0);
    assert_eq!(pending.as_deref(), Some(journal.publication_id.as_str()));
}

#[tokio::test]
async fn current_domain_receipt_selection_and_original_contract_are_rechecked_before_effects() {
    for mutation in [
        "UPDATE awr_team.work_runtime SET state='ready',selected_completion_id=NULL WHERE work_id='a'",
        "UPDATE awr_team.delivery_selections SET selection_version=selection_version+1 WHERE work_id='a'",
        "UPDATE awr_team.work_runtime SET last_fence=last_fence+1 WHERE work_id='a'",
        "UPDATE awr_team.work_contracts SET contract_json='{}' WHERE work_id='a'",
    ] {
        let f = SourceFixture::new(false).await;
        f.finalize("domain-recheck").await;
        let lease = f.take("domain-recheck-source", "source").await;
        let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
        f.f.admin.batch_execute(mutation).await.unwrap();
        assert!(
            matches!(
                f.f.store
                    .process_sync_intent(TENANT, PROJECT, WORKER, &lease, 60)
                    .await,
                Err(PgError::PreconditionsChanged | PgError::SourceDivergence)
            ),
            "Changed domain binding or missing original definition cannot authorize an effect"
        );
        assert_eq!(std::fs::read(f.root.0.join("ledger.yaml")).unwrap(), before);
        assert_eq!(
            f.f.admin
                .query_one(
                    "SELECT count(*) FROM awr_team.delivery_source_publications",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
    }
}

#[tokio::test]
async fn missing_archived_domain_contract_cannot_rebind_a_pending_source_intent() {
    let f = SourceFixture::new(false).await;
    f.finalize("domain-before-missing-history").await;
    f.publish_successor(Some("c")).await;
    f.f.admin
        .execute(
            "DELETE FROM awr_team.workstream_snapshot_ownership
        WHERE snapshot_id=$1 AND work_id='a'",
            &[&f.f.set.source_snapshot_id],
        )
        .await
        .unwrap();
    f.f.admin
        .execute(
            "DELETE FROM awr_team.work_contracts
        WHERE snapshot_id=$1 AND work_id='a'",
            &[&f.f.set.source_snapshot_id],
        )
        .await
        .unwrap();
    let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    let queue = f.queue().await;
    assert!(
        queue["intents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["binding_current"] == false
                && i["read_set"]["source_snapshot_id"] == f.f.set.source_snapshot_id)
    );
    assert_eq!(std::fs::read(f.root.0.join("ledger.yaml")).unwrap(), before);
    assert_eq!(
        f.f.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn revoked_domain_source_worker_cannot_write_or_abandon_its_original_journal() {
    let f = SourceFixture::new(false).await;
    f.finalize("domain-before-revocation").await;
    let lease = f.take("domain-revoked-worker", "source").await;
    let journal =
        f.f.store
            .prepare_sync_source(TENANT, PROJECT, WORKER, &lease, 60)
            .await
            .unwrap();
    let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    f.f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE actor_id='integrator'")
        .await.unwrap();
    assert!(matches!(
        f.f.store
            .write_sync_source(TENANT, PROJECT, WORKER, &lease)
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(std::fs::read(f.root.0.join("ledger.yaml")).unwrap(), before);
    let pending: Option<String> =
        f.f.admin
            .query_one(
                "SELECT pending_publication_id FROM awr_team.delivery_source_cursors",
                &[],
            )
            .await
            .unwrap()
            .get(0);
    assert_eq!(pending.as_deref(), Some(journal.publication_id.as_str()));
}

#[tokio::test]
async fn every_manifest_entry_needs_original_candidate_bound_artifact_evidence() {
    let extra = "Supplementary public notes";
    let f = SourceFixture::new_with_extra(false, Some(extra)).await;
    assert!(matches!(
        f.try_finalize("missing-extra").await,
        Err(PgError::EvidenceInvalid)
    ));
    f.assert_no_completion().await;
    let submit = |payload| {
        json!({"session_id":"session-a","expected_session_version":"1",
        "artifact_text":extra,"dirty_tree":false,"payload":payload})
    };
    let unbound = integration_fixture::run(
        &f.f.reads,
        A,
        "unbound-extra",
        "evidence.submit",
        submit(json!({"passed":true})),
    )
    .await;
    assert!(matches!(
        f.try_finalize("unbound-extra-finalize").await,
        Err(PgError::EvidenceInvalid)
    ));
    f.assert_no_completion().await;
    let bound = integration_fixture::run(
        &f.f.reads,
        A,
        "bound-extra",
        "evidence.submit",
        submit(json!({"passed":true,"delivery_candidate_digest":f.f.request.candidate_digest})),
    )
    .await;
    assert_ne!(unbound["artifact_id"], bound["artifact_id"]);
    let completed = f.finalize("complete-manifest").await;
    assert_eq!(completed["evidence_id"], f.f.evidence["evidence_id"]);
    let publication =
        f.f.store
            .prepare_source_publication(
                TENANT,
                PROJECT,
                WORKER,
                f.request(
                    "prepare-manifest",
                    completed["receipt_id"].as_str().unwrap(),
                )
                .await,
            )
            .await
            .unwrap()["data"]
            .clone();
    f.f.store
        .write_source_publication(
            TENANT,
            PROJECT,
            WORKER,
            f.step("write-manifest", &publication),
        )
        .await
        .unwrap();
    assert_eq!(
        f.f.store
            .confirm_source_publication(
                TENANT,
                PROJECT,
                WORKER,
                f.step("confirm-manifest", &publication)
            )
            .await
            .unwrap()["data"]["phase"],
        "confirmed"
    );
}

#[tokio::test]
async fn actual_agent_and_simulated_acceptance_publish_exact_stored_artifact_references() {
    for simulated in [false, true] {
        let f = SourceFixture::new(simulated).await;
        let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
        let completed = f.finalize("finalize").await;
        assert_eq!(completed["task_complete"], true);
        assert_eq!(completed["human_approval"], false);
        assert_eq!(
            completed["delivery_candidate_digest"],
            f.f.request.candidate_digest
        );
        let receipt = completed["receipt_id"].as_str().unwrap();
        let bound: Option<String> = f
            .f
            .admin
            .query_one(
                "SELECT delivery_candidate_digest FROM awr_team.completion_receipts WHERE id=$1",
                &[&receipt],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            bound.as_deref(),
            Some(f.f.request.candidate_digest.as_str()),
            "Actual finalization must bind its new receipt to the reviewed candidate"
        );
        assert_ne!(f.f.evidence["artifact_id"], "package");
        let publication =
            f.f.store
                .prepare_source_publication(
                    TENANT,
                    PROJECT,
                    WORKER,
                    f.request("prepare", receipt).await,
                )
                .await
                .unwrap()["data"]
                .clone();
        assert_eq!(std::fs::read(f.root.0.join("ledger.yaml")).unwrap(), before);
        let rebuilt = DeliverySyncStore::from_config(f.f.config.clone());
        rebuilt
            .write_source_publication(TENANT, PROJECT, WORKER, f.step("write", &publication))
            .await
            .unwrap();
        let written = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
        let confirmed = rebuilt
            .confirm_source_publication(TENANT, PROJECT, WORKER, f.step("confirm", &publication))
            .await
            .unwrap();
        assert_eq!(confirmed["data"]["phase"], "confirmed");
        assert_eq!(
            std::fs::read(f.root.0.join("ledger.yaml")).unwrap(),
            written
        );
        let note: Value =
            f.f.admin
                .query_one(
                    "SELECT note_json FROM awr_team.delivery_source_publications WHERE id=$1",
                    &[&publication["publication_id"].as_str().unwrap()],
                )
                .await
                .unwrap()
                .get(0);
        assert_eq!(note["completion_reference"]["receipt_id"], receipt);
        assert_eq!(
            note["completion_reference"]["artifact_id"],
            f.f.evidence["artifact_id"]
        );
        let text = String::from_utf8(written).unwrap();
        assert!(text.starts_with("# Preserve this comment"));
        assert_eq!(text.matches("status: planned").count(), 3);
        assert_eq!(
            f.f.admin
                .query_one("SELECT count(*) FROM awr_team.delivery_fact_heads", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
        assert_eq!(
            f.f.admin
                .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            1
        );
    }
}

#[tokio::test]
async fn changed_candidate_and_missing_or_wrong_original_declarations_cannot_finalize() {
    for simulated in [false, true] {
        let f = SourceFixture::new(simulated).await;
        let mut changed = f.f.selection.clone();
        changed.request_id = "replacement".into();
        changed.expected_selected_digest = Some(f.f.request.candidate_digest.clone());
        changed.candidate.binding.candidate_version = "2".into();
        f.f.store
            .select_candidate(TENANT, PROJECT, A, changed)
            .await
            .unwrap();
        assert!(matches!(
            f.try_finalize("replaced").await,
            Err(PgError::EvidenceInvalid)
        ));
        f.assert_no_completion().await;
        drop(f);
        for declaration in [None, Some("d".repeat(64))] {
            let mut f = SourceFixture::new(simulated).await;
            let mut payload = json!({"passed":true});
            if let Some(value) = declaration {
                payload["delivery_candidate_digest"] = json!(value);
            }
            // Submit and independently approve a genuinely new evidence bundle.
            // Its hash is valid; absence or mismatch of its candidate is the failure.
            f.resubmit_reviewed_evidence(payload).await;
            assert!(matches!(
                f.try_finalize("undeclared").await,
                Err(PgError::EvidenceInvalid)
            ));
            f.assert_no_completion().await;
        }
    }
}

#[tokio::test]
async fn real_acceptance_keeps_no_candidate_compatibility_without_reconstructing_history() {
    let mut f = SourceFixture::new(false).await;
    let original_selection: Value =
        f.f.admin
            .query_one(
                "SELECT to_jsonb(s) FROM awr_team.delivery_selections s",
                &[],
            )
            .await
            .unwrap()
            .get(0);
    // Fault injection removes the optional delivery selection, not execution or
    // review. Resubmit through the actual ordinary Agent completion workflow.
    f.f.admin
        .batch_execute("DELETE FROM awr_team.delivery_selections")
        .await
        .unwrap();
    f.resubmit_reviewed_evidence(json!({"passed":true})).await;
    let receipt = f.finalize("without-delivery").await;
    assert!(receipt["delivery_candidate_digest"].is_null());
    assert_eq!(receipt["task_complete"], true);
    assert!(receipt.get("source_sync").is_none());
    assert_eq!(
        f.f.admin
            .query_one("SELECT count(*) FROM awr_team.delivery_sync_intents", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert!(
        f.f.admin
            .query_one(
                "SELECT delivery_candidate_digest FROM awr_team.completion_receipts WHERE id=$1",
                &[&receipt["receipt_id"].as_str().unwrap()],
            )
            .await
            .unwrap()
            .get::<_, Option<String>>(0)
            .is_none()
    );
    // Restoring the original selection cannot retroactively bind this real receipt.
    f.f.admin
        .execute(
            "INSERT INTO awr_team.delivery_selections SELECT * FROM
        jsonb_populate_record(NULL::awr_team.delivery_selections,$1)",
            &[&original_selection],
        )
        .await
        .unwrap();
    assert!(matches!(
        f.f.store
            .prepare_source_publication(
                TENANT,
                PROJECT,
                WORKER,
                f.request("unbound-history", receipt["receipt_id"].as_str().unwrap())
                    .await
            )
            .await,
        Err(PgError::EvidenceInvalid)
    ));
}

#[tokio::test]
async fn publication_rechecks_actual_receipt_artifact_execution_and_current_binding() {
    let f = SourceFixture::new(false).await;
    let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    let completed = f.finalize("finalize").await;
    let receipt = completed["receipt_id"].as_str().unwrap();
    let request = f.request("prepare", receipt).await;
    let artifact = f.f.evidence["artifact_id"].as_str().unwrap();
    assert!(
        f.f.admin
            .batch_execute("UPDATE awr_team.completion_receipts SET delivery_candidate_digest=NULL")
            .await
            .is_err(),
        "Queued receipt/candidate binding must also be protected by persistence"
    );
    let mutations = [
        "UPDATE awr_team.artifacts SET byte_length=byte_length+1",
        "UPDATE awr_team.artifacts SET content=convert_to('changed bytes','UTF8')",
        "UPDATE awr_team.executions SET result_digest=repeat('e',64)",
        "UPDATE awr_team.evidence SET payload_json=payload_json-'delivery_candidate_digest'",
        "UPDATE awr_team.work_runtime SET state='ready' WHERE work_id='a'",
        "UPDATE awr_team.delivery_candidates SET body_json=jsonb_set(body_json,'{binding,candidate_version}','\"2\"')",
    ];
    let candidate_body = json!(f.f.selection.candidate);
    let evidence_payload: Value =
        f.f.admin
            .query_one(
                "SELECT payload_json FROM awr_team.evidence WHERE id=$1",
                &[&f.f.evidence["evidence_id"].as_str().unwrap()],
            )
            .await
            .unwrap()
            .get(0);
    for mutation in mutations {
        // Corrupt persisted facts through the fixture administrator, then call
        // the authenticated publisher. No completion record is fabricated.
        f.f.admin.batch_execute(mutation).await.unwrap();
        assert!(
            matches!(
                f.f.store
                    .prepare_source_publication(TENANT, PROJECT, WORKER, request.clone())
                    .await,
                Err(PgError::EvidenceInvalid)
            ),
            "Persisted mismatches must refuse source publication: {mutation}"
        );
        assert_eq!(std::fs::read(f.root.0.join("ledger.yaml")).unwrap(), before);
        assert_eq!(
            f.f.admin
                .query_one(
                    "SELECT count(*) FROM awr_team.delivery_source_publications",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
        f.f.admin
            .execute(
                "UPDATE awr_team.completion_receipts SET delivery_candidate_digest=$1",
                &[&f.f.request.candidate_digest],
            )
            .await
            .unwrap();
        f.f.admin
            .execute(
                "UPDATE awr_team.artifacts SET byte_length=$1,content=$2 WHERE id=$3",
                &[&(CONTENT.len() as i64), &CONTENT.as_bytes(), &artifact],
            )
            .await
            .unwrap();
        f.f.admin
            .execute(
                "UPDATE awr_team.executions SET result_digest=$1",
                &[&f.f.selection.candidate.manifest.entries[0].sha256],
            )
            .await
            .unwrap();
        f.f.admin
            .execute(
                "UPDATE awr_team.evidence SET payload_json=$1 WHERE id=$2",
                &[
                    &evidence_payload,
                    &f.f.evidence["evidence_id"].as_str().unwrap(),
                ],
            )
            .await
            .unwrap();
        f.f.admin.execute("UPDATE awr_team.work_runtime SET state='completed',selected_completion_id=$1 WHERE work_id='a'",
            &[&receipt]).await.unwrap();
        f.f.admin
            .execute(
                "UPDATE awr_team.delivery_candidates SET body_json=$1",
                &[&candidate_body],
            )
            .await
            .unwrap();
    }
    let publication =
        f.f.store
            .prepare_source_publication(TENANT, PROJECT, WORKER, request)
            .await
            .unwrap()["data"]
            .clone();
    // The actual source write happened. Lose its lease before confirmation,
    // reconstruct the store and recover this exact effect without another write.
    f.f.store
        .write_source_publication(TENANT, PROJECT, WORKER, f.step("write", &publication))
        .await
        .unwrap();
    let written = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    f.f.admin.batch_execute("UPDATE awr_team.delivery_source_publications SET expires_at=clock_timestamp()-interval '1 second'")
        .await.unwrap();
    let recovered = DeliverySyncStore::from_config(f.f.config.clone())
        .confirm_source_publication(
            TENANT,
            PROJECT,
            WORKER,
            f.step("expired-confirm", &publication),
        )
        .await;
    assert!(matches!(recovered, Err(PgError::LeaseExpired)));
    assert_eq!(
        std::fs::read(f.root.0.join("ledger.yaml")).unwrap(),
        written
    );
    assert_eq!(
        f.f.admin
            .query_one(
                "SELECT metadata_revision FROM awr_team.delivery_source_cursors",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        f.f.admin
            .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    let rebuilt = DeliverySyncStore::from_config(f.f.config.clone());
    let renewed = rebuilt
        .renew_source_publication(
            TENANT,
            PROJECT,
            WORKER,
            RenewDeliveryPublicationLease {
                step: f.step("renew", &publication),
                lease_seconds: 60,
            },
        )
        .await
        .unwrap()["data"]
        .clone();
    assert_ne!(renewed["fence"], publication["fence"]);
    assert!(matches!(
        rebuilt
            .confirm_source_publication(TENANT, PROJECT, WORKER, f.step("old-fence", &publication))
            .await,
        Err(PgError::StaleFence)
    ));
    let confirmed = rebuilt
        .confirm_source_publication(TENANT, PROJECT, WORKER, f.step("confirm", &renewed))
        .await
        .unwrap();
    assert_eq!(confirmed["data"]["phase"], "confirmed");
    assert_eq!(
        std::fs::read(f.root.0.join("ledger.yaml")).unwrap(),
        written
    );
    assert_eq!(
        f.f.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}
