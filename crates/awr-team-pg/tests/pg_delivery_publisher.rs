#![cfg(feature = "pg-tests")]
//! Synthetic filesystem/PG mechanisms. No native business-acceptance credit.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_source::{
    DeliverySourceNote, PublishPrepOptions, fingerprint, prepare_delivery_source_note,
    prepare_publish_from_server_directory,
};
use awr_team::{SourceActivationPlan, delivery::*};
use awr_team_pg::*;
use fixture::*;
use serde_json::{Value, json};

#[path = "fixtures/delivery_publication.rs"]
mod publication;
use publication::*;

#[tokio::test]
async fn publication_replays_and_restarts_preserve_contract_status_and_source_bytes() {
    let f = setup_publisher().await;
    let before = f.bytes();
    let source=f.admin.query_one("SELECT active_snapshot_id,authority_epoch FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
        &[&TENANT,&PROJECT]).await.unwrap();
    let request = f.request("prepare").await;
    let p = f
        .store
        .prepare_source_publication(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap()["data"]
        .clone();
    assert_eq!(f.bytes(), before);
    let replay = f
        .restarted()
        .prepare_source_publication(TENANT, PROJECT, A, request)
        .await
        .unwrap();
    assert_eq!(replay["data"], p);
    let written = f
        .restarted()
        .write_source_publication(TENANT, PROJECT, A, f.step(&p, "write"))
        .await
        .unwrap();
    assert_eq!(written["data"]["phase"], "source_written");
    assert_eq!(written["source_synchronized"], false);
    let after = f.bytes();
    let confirmed = f
        .restarted()
        .confirm_source_publication(TENANT, PROJECT, A, f.step(&p, "confirm"))
        .await
        .unwrap();
    assert_eq!(confirmed["data"]["phase"], "confirmed");
    assert_eq!(f.bytes(), after);
    let status = f.status().await;
    assert_eq!(status["source_synchronized"], true);
    assert_eq!(status["metadata_revision"], "1");
    let current=f.admin.query_one("SELECT active_snapshot_id,authority_epoch FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
        &[&TENANT,&PROJECT]).await.unwrap();
    assert_eq!(current.get::<_, String>(0), source.get::<_, String>(0));
    assert_eq!(current.get::<_, i64>(1), source.get::<_, i64>(1));
    let text = String::from_utf8(after).unwrap();
    assert!(text.starts_with("# Preserve this comment"));
    assert_eq!(text.matches("status: planned").count(), 4);
    let note: DeliverySourceNote = serde_json::from_value(
        f.admin
            .query_one(
                "SELECT note_json FROM awr_team.delivery_source_publications WHERE id=$1",
                &[&p["publication_id"].as_str().unwrap()],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    let replay = prepare_delivery_source_note(text.as_bytes(), &note).unwrap();
    assert_eq!(replay.before_bytes, replay.after_bytes);
    assert!(replay.changed_external_keys.is_empty());
    assert_eq!(
        f.admin
            .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn crash_after_filesystem_effect_recovers_by_observation_without_reapplying() {
    let f = setup_publisher().await;
    let p = f.prepare("prepare").await;
    let after = f.after_bytes(&p).await;
    // Persisted intent exists, but the process died before its PG phase update.
    std::fs::write(f.root.join("ledger.yaml"), &after).unwrap();
    let written = f
        .restarted()
        .write_source_publication(TENANT, PROJECT, A, f.step(&p, "recover-write"))
        .await
        .unwrap();
    assert_eq!(written["data"]["phase"], "source_written");
    assert_eq!(f.bytes(), after);
    let result = f
        .restarted()
        .confirm_source_publication(TENANT, PROJECT, A, f.step(&p, "confirm"))
        .await
        .unwrap();
    assert_eq!(result["data"]["phase"], "confirmed");
    assert_eq!(f.bytes(), after);
}

#[tokio::test]
async fn prepare_is_atomic_across_concurrent_requests_and_keeps_one_unknown_slot() {
    let f = setup_publisher().await;
    let a = f.request("prepare-a").await;
    let mut b = a.clone();
    b.request_id = "prepare-b".into();
    let (a, b) = tokio::join!(
        f.store.prepare_source_publication(TENANT, PROJECT, A, a),
        f.store.prepare_source_publication(TENANT, PROJECT, A, b)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.err().or(b.err()),
        Some(PgError::ResourceConflict)
    ));
    assert_eq!(
        f.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    let p = f.status().await["history"][0].clone();
    f.admin.execute("UPDATE awr_team.delivery_source_publications SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1",
        &[&p["publication_id"].as_str().unwrap()]).await.unwrap();
    assert!(matches!(
        f.store
            .write_source_publication(TENANT, PROJECT, A, f.step(&p, "expired"))
            .await,
        Err(PgError::LeaseExpired)
    ));
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, f.request("cannot-steal").await)
            .await,
        Err(PgError::ResourceConflict)
    ));
    let renewed = f
        .restarted()
        .renew_source_publication(
            TENANT,
            PROJECT,
            A,
            RenewDeliveryPublicationLease {
                step: f.step(&p, "renew"),
                lease_seconds: 60,
            },
        )
        .await
        .unwrap()["data"]
        .clone();
    assert_ne!(renewed["fence"], p["fence"]);
    assert!(matches!(
        f.store
            .write_source_publication(TENANT, PROJECT, A, f.step(&p, "old-fence"))
            .await,
        Err(PgError::StaleFence)
    ));
    assert_eq!(
        f.store
            .write_source_publication(TENANT, PROJECT, A, f.step(&renewed, "write"))
            .await
            .unwrap()["data"]["phase"],
        "source_written"
    );
}

#[tokio::test]
async fn external_drift_is_retained_and_confirmed_history_is_not_present_currentness() {
    let f = setup_publisher().await;
    let p = f.prepare("prepare").await;
    f.store
        .write_source_publication(TENANT, PROJECT, A, f.step(&p, "write"))
        .await
        .unwrap();
    let command = f.step(&p, "confirm");
    f.store
        .confirm_source_publication(TENANT, PROJECT, A, command.clone())
        .await
        .unwrap();
    let mut drift = f.bytes();
    drift.extend_from_slice(b"\n# An external edit\n");
    std::fs::write(f.root.join("ledger.yaml"), &drift).unwrap();
    let replay = f
        .store
        .confirm_source_publication(TENANT, PROJECT, A, command)
        .await
        .unwrap();
    assert_eq!(replay["data"]["phase"], "confirmed");
    assert_eq!(replay["state_basis"], "at_commit");
    let status = f.status().await;
    assert_eq!(status["source_synchronized"], false);
    assert_eq!(status["history"][0]["source_synchronized_at_commit"], true);
    assert_eq!(status["history"][0]["source_current"], false);
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, f.request("drift-refused").await)
            .await,
        Err(PgError::SourceDivergence)
    ));
    assert_eq!(f.bytes(), drift);
}

#[tokio::test]
async fn independent_work_publications_preserve_each_others_current_notes() {
    let f = setup_publisher().await;
    f.admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        VALUES($1,$2,'session-d','main','d','agent','cli-a','conversation-d','active',$3,1)",
        &[&TENANT,&PROJECT,&f.set.workstream_id.to_string()]).await.unwrap();
    let other = select(&f.reads, &f.store, "d", "session-d").await;
    f.observe(&other, "other-initial").await;
    let a = f.publish("publish-a", &f.selection).await;
    let b = f.publish("publish-d", &other).await;
    assert_eq!(a["data"]["metadata_revision"], "1");
    assert_eq!(b["data"]["metadata_revision"], "2");
    let status_a = f.status().await;
    let status_d = f
        .store
        .source_publication_status(TENANT, PROJECT, A, "d")
        .await
        .unwrap();
    assert_eq!(status_a["source_synchronized"], true);
    assert_eq!(status_d["source_synchronized"], true);
    assert_eq!(status_a["metadata_revision"], "2");
    assert_ne!(
        status_a["confirmed_fingerprint"],
        a["data"]["after_fingerprint"]
    );
    assert_eq!(status_a["history"].as_array().unwrap().len(), 1);
    assert_eq!(status_d["history"].as_array().unwrap().len(), 1);
    assert_eq!(status_a["history"][0]["work_id"], "a");
    assert_eq!(status_d["history"][0]["work_id"], "d");
    assert!(
        !status_a.to_string().contains("Preserve compatibility.")
            && !status_d.to_string().contains("docs/spec.md")
    );
}

#[tokio::test]
async fn changed_observations_refuse_stale_intents_and_allow_safe_unwritten_withdrawal() {
    let f = setup_publisher().await;
    let before = f.bytes();
    f.observe(&f.selection, "new-observations").await;
    let p = f.prepare("prepare-old-observations").await;
    // Normal new observations now wait for pending publication. Inject a
    // historical head rollback explicitly to retain the stale-note safeguard.
    // Both facts were admitted normally before preparation; no prerequisite is
    // synthesized and the fixture never bypasses a production API for delivery.
    f.admin.batch_execute("UPDATE awr_team.delivery_fact_heads h SET fact_id=(
        SELECT f.id FROM awr_team.delivery_facts f JOIN awr_team.delivery_inbox i
        ON (i.tenant_id,i.project_id,i.id)=(f.tenant_id,f.project_id,f.inbox_id)
        WHERE i.tenant_id=h.tenant_id AND i.project_id=h.project_id AND i.connector_id=h.connector_id
        AND i.event_id='initial' AND f.slot=h.slot) WHERE h.work_id='a'").await.unwrap();
    assert!(matches!(
        f.store
            .write_source_publication(TENANT, PROJECT, A, f.step(&p, "stale-write"))
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(f.bytes(), before);
    let step = f.step(&p, "withdraw");
    let withdrawn = f
        .restarted()
        .abandon_source_publication(TENANT, PROJECT, A, step.clone())
        .await
        .unwrap();
    assert_eq!(withdrawn["data"]["source_bytes_written"], false);
    assert_eq!(withdrawn["data"]["failure_code"], "withdrawn");
    assert_eq!(f.status().await["pending_publication_id"], Value::Null);
    assert_eq!(f.bytes(), before);
    assert_eq!(
        f.store
            .abandon_source_publication(TENANT, PROJECT, A, step)
            .await
            .unwrap()["replayed"],
        true
    );
    f.observe(&f.selection, "after-withdrawal").await;
    f.publish("publish-new-observations", &f.selection).await;
    f.observe(&f.selection, "later-observations").await;
    let status = f.status().await;
    assert_eq!(status["cursor_current"], true);
    assert_eq!(status["source_synchronized"], false);
    assert_eq!(status["history"][0]["source_synchronized_at_commit"], true);
}

#[tokio::test]
async fn wrong_scope_non_manager_agent_and_revoked_credential_cannot_publish() {
    let f = setup_publisher().await;
    let before = f.bytes();
    let request = f.request("prepare").await;
    for (tenant, project, token) in [
        (TENANT, PROJECT, B),
        ("other-tenant", PROJECT, A),
        (TENANT, "missing-project", A),
    ] {
        let result = f
            .store
            .prepare_source_publication(tenant, project, token, request.clone())
            .await;
        assert!(
            access_denied(&result),
            "scope {tenant}/{project}: {result:?}"
        );
    }
    assert!(access_denied(
        &f.store
            .source_publication_status(TENANT, PROJECT, B, "a")
            .await
    ));
    f.admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=false WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(access_denied(
        &f.store
            .prepare_source_publication(TENANT, PROJECT, A, request.clone())
            .await
    ));
    f.admin
        .batch_execute("UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'; UPDATE awr_team.actors SET kind='agent' WHERE id='agent'")
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    f.admin
        .batch_execute("UPDATE awr_team.actors SET kind='human' WHERE id='agent'")
        .await
        .unwrap();
    let p = f
        .store
        .prepare_source_publication(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap()["data"]
        .clone();
    f.admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, request)
            .await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        f.store
            .write_source_publication(TENANT, PROJECT, A, f.step(&p, "revoked-write"))
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(f.bytes(), before);
    assert_eq!(
        f.admin
            .query_one(
                "SELECT phase FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "pending"
    );
}

#[tokio::test]
async fn missing_facts_receipts_and_changed_requests_do_not_create_false_publications() {
    let f = setup_publisher().await;
    let before = f.bytes();
    let mut request = f.request("prepare").await;
    let mut caller_flags = json!(request);
    caller_flags["verified"] = json!(true);
    assert!(serde_json::from_value::<PrepareDeliverySourcePublication>(caller_flags).is_err());
    request.completion_receipt_id = Some("external-merge-is-not-completion".into());
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    request.completion_receipt_id = None;
    f.admin
        .batch_execute("DELETE FROM awr_team.delivery_fact_heads")
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    assert_eq!(
        f.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    f.observe(&f.selection, "restored-observations").await;
    f.store
        .prepare_source_publication(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    request.lease_seconds = 120;
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, request)
            .await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(f.bytes(), before);
}

#[tokio::test]
async fn read_only_write_failure_is_queryable_and_recovery_uses_the_same_intent() {
    let f = setup_publisher().await;
    let before = f.bytes();
    let p = f.prepare("prepare").await;
    let path = f.root.join("ledger.yaml");
    let permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    let command = f.step(&p, "write-readonly");
    let failed = f
        .store
        .write_source_publication(TENANT, PROJECT, A, command.clone())
        .await;
    std::fs::set_permissions(&path, permissions).unwrap();
    let failed = failed.unwrap();
    assert_eq!(failed["data"]["phase"], "failed");
    assert_eq!(failed["data"]["failure_code"], "write_failed");
    assert_eq!(failed["data"]["source_written_observed"], false);
    assert_eq!(f.bytes(), before);
    assert_eq!(
        f.restarted()
            .write_source_publication(TENANT, PROJECT, A, command)
            .await
            .unwrap()["data"]["phase"],
        "failed"
    );
    assert_eq!(f.bytes(), before);
    assert_eq!(
        f.restarted()
            .write_source_publication(TENANT, PROJECT, A, f.step(&p, "retry-same-intent"))
            .await
            .unwrap()["data"]["phase"],
        "source_written"
    );
    f.store
        .confirm_source_publication(TENANT, PROJECT, A, f.step(&p, "confirm"))
        .await
        .unwrap();
    assert_eq!(f.status().await["source_synchronized"], true);
}

#[tokio::test]
async fn observed_landing_remains_known_when_projection_fails_and_source_is_rolled_back() {
    let f = setup_publisher().await;
    let before = f.bytes();
    let p = f.prepare("prepare").await;
    std::fs::write(f.root.join("ledger.yaml"), f.after_bytes(&p).await).unwrap();
    let spec = f.root.join("docs/spec.md");
    let original = std::fs::read(&spec).unwrap();
    std::fs::write(&spec, "# Changed contract\nRequire another result.\n").unwrap();
    let result = f
        .restarted()
        .confirm_source_publication(TENANT, PROJECT, A, f.step(&p, "observe-drift"))
        .await
        .unwrap();
    assert_eq!(result["data"]["phase"], "conflict");
    assert_eq!(result["data"]["failure_code"], "projection_changed");
    assert_eq!(result["data"]["source_written_observed"], true);
    std::fs::write(&spec, original).unwrap();
    std::fs::write(f.root.join("ledger.yaml"), &before).unwrap();
    assert!(matches!(
        f.store
            .abandon_source_publication(TENANT, PROJECT, A, f.step(&p, "cannot-withdraw"))
            .await,
        Err(PgError::SourceDivergence)
    ));
    assert!(matches!(
        f.store
            .write_source_publication(TENANT, PROJECT, A, f.step(&p, "cannot-reapply"))
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(f.bytes(), before);
}

#[tokio::test]
async fn external_changes_before_write_and_replaced_source_lock_are_not_overwritten() {
    for replace_lock in [false, true] {
        let f = setup_publisher().await;
        let p = f.prepare("prepare").await;
        if replace_lock {
            let lock = std::fs::read_dir(&f.root)
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| p.file_name().unwrap().to_string_lossy().ends_with(".lock"))
                .unwrap();
            // Keep the old inode alive so replacement cannot reuse its identity.
            std::fs::rename(&lock, lock.with_extension("old-lock")).unwrap();
            std::fs::write(lock, b"").unwrap();
        } else {
            let mut drift = f.bytes();
            drift.extend_from_slice(b"\n# External update\n");
            std::fs::write(f.root.join("ledger.yaml"), drift).unwrap();
        }
        let before = f.bytes();
        let result = f
            .store
            .write_source_publication(TENANT, PROJECT, A, f.step(&p, "write"))
            .await
            .unwrap();
        assert_eq!(result["data"]["phase"], "conflict");
        assert_eq!(
            result["data"]["failure_code"],
            if replace_lock {
                "source_identity_changed"
            } else {
                "source_drift"
            }
        );
        assert_eq!(f.bytes(), before);
        assert_eq!(f.status().await["source_synchronized"], false);
    }
}

#[tokio::test]
async fn database_failure_before_and_after_source_effect_recovers_without_rewriting() {
    let f = setup_publisher().await;
    let before = f.bytes();
    f.admin.batch_execute("CREATE FUNCTION fail_publication_event() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.event_type LIKE 'delivery.source.%' THEN RAISE EXCEPTION 'synthetic event failure'; END IF;
        RETURN NEW; END $$;
        CREATE TRIGGER fail_publication_event BEFORE INSERT ON awr_team.events
        FOR EACH ROW EXECUTE FUNCTION fail_publication_event()").await.unwrap();
    let request = f.request("prepare").await;
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(f.bytes(), before);
    assert_eq!(
        f.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_source_publications",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    f.admin
        .batch_execute("DROP TRIGGER fail_publication_event ON awr_team.events")
        .await
        .unwrap();
    let p = f
        .restarted()
        .prepare_source_publication(TENANT, PROJECT, A, request)
        .await
        .unwrap()["data"]
        .clone();
    f.admin
        .batch_execute(
            "CREATE TRIGGER fail_publication_event BEFORE INSERT ON awr_team.events
        FOR EACH ROW EXECUTE FUNCTION fail_publication_event()",
        )
        .await
        .unwrap();
    let write = f.step(&p, "write");
    assert!(matches!(
        f.store
            .write_source_publication(TENANT, PROJECT, A, write.clone())
            .await,
        Err(PgError::Db(_))
    ));
    let landed = f.bytes();
    assert_eq!(landed, f.after_bytes(&p).await);
    let metadata = std::fs::metadata(f.root.join("ledger.yaml")).unwrap();
    assert_eq!(f.status().await["history"][0]["phase"], "pending");
    f.admin
        .batch_execute("DROP TRIGGER fail_publication_event ON awr_team.events")
        .await
        .unwrap();
    let recovered = f
        .restarted()
        .write_source_publication(TENANT, PROJECT, A, write)
        .await
        .unwrap();
    assert_eq!(recovered["data"]["phase"], "source_written");
    assert_eq!(f.bytes(), landed);
    let unchanged = std::fs::metadata(f.root.join("ledger.yaml")).unwrap();
    assert_eq!(metadata.modified().unwrap(), unchanged.modified().unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata.ino(), unchanged.ino());
    }
    f.store
        .confirm_source_publication(TENANT, PROJECT, A, f.step(&p, "confirm"))
        .await
        .unwrap();
    assert_eq!(f.status().await["source_synchronized"], true);
}

#[tokio::test]
async fn completion_notes_require_selected_candidate_bound_evidence_and_actual_artifact() {
    let f = setup_publisher().await;
    let content = b"synthetic output\n";
    let mut selection = f.selection.clone();
    selection.request_id = "select-evidence-candidate".into();
    selection.expected_selected_digest = Some(selection.candidate.binding.digest().unwrap());
    selection.candidate.binding.candidate_version = "2".into();
    f.store
        .select_candidate(TENANT, PROJECT, A, selection.clone())
        .await
        .unwrap();
    f.observe(&selection, "candidate-evidence").await;
    let digest = selection.candidate.binding.digest().unwrap();
    let prepared = prepare(&f.reads, A, "a").await;
    let evidence = f
        .reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "submit-evidence",
                "evidence.submit",
                json!({
                    "session_id":"session-a","expected_session_version":"1",
                    "payload":{"criterion":"verified","passed":true,"delivery_candidate_digest":digest},
                    "artifact_text":std::str::from_utf8(content).unwrap(),"dirty_tree":false
                }),
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let artifact = evidence["artifact_id"].as_str().unwrap().to_owned();
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_owned();
    let evidence_digest = evidence["digest"].as_str().unwrap().to_owned();
    assert_ne!(
        artifact,
        selection.candidate.manifest.entries[0].artifact_id
    );
    let dependencies = fingerprint(b"[]")[7..].to_owned();
    // Synthetic domain fixture: represents the neutral finalizer's selected
    // receipt. Publication must validate it; it never creates or approves one.
    f.admin.execute("INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,
        result_digest,dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,evidence_id,delivery_candidate_digest)
        VALUES($1,$2,'completion-a','a','main',$3,$4,$5,$4,'ordinary_confirm','[]',$6,$7)",
        &[&TENANT,&PROJECT,&f.set.contract_hash,&evidence_digest,&dependencies,&evidence_id,&digest]).await.unwrap();
    f.admin.batch_execute("UPDATE awr_team.work_runtime SET state='completed',selected_completion_id='completion-a' WHERE work_id='a'")
        .await.unwrap();
    let mut request = f.request_for("prepare-completion", &selection).await;
    request.expected_selection_version = "2".into();
    request.completion_receipt_id = Some("completion-a".into());
    f.admin.batch_execute("UPDATE awr_team.completion_receipts SET delivery_candidate_digest=NULL WHERE id='completion-a'")
        .await.unwrap();
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    f.admin.execute("UPDATE awr_team.completion_receipts SET delivery_candidate_digest=$1 WHERE id='completion-a'", &[&digest])
        .await.unwrap();
    f.admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$1 WHERE id=$2",
            &[&b"changed artifact".as_slice(), &artifact],
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .prepare_source_publication(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    f.admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$1 WHERE id=$2",
            &[&content.as_slice(), &artifact],
        )
        .await
        .unwrap();
    let p = f
        .store
        .prepare_source_publication(TENANT, PROJECT, A, request)
        .await
        .unwrap()["data"]
        .clone();
    f.store
        .write_source_publication(TENANT, PROJECT, A, f.step(&p, "write-completion"))
        .await
        .unwrap();
    let result = f
        .store
        .confirm_source_publication(TENANT, PROJECT, A, f.step(&p, "confirm-completion"))
        .await
        .unwrap();
    assert_eq!(result["data"]["phase"], "confirmed");
    assert_eq!(
        result["data"]["confirmation"]["completion_reference"]["receipt_id"],
        "completion-a"
    );
    assert_eq!(
        result["data"]["confirmation"]["completion_reference"]["artifact_id"],
        artifact
    );
    assert_eq!(f.status().await["source_synchronized"], true);
    assert_eq!(
        f.admin
            .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert_eq!(
        String::from_utf8(f.bytes())
            .unwrap()
            .matches("status: planned")
            .count(),
        4
    );
}

#[tokio::test]
async fn publication_tables_enforce_rls_and_unsupported_writers_are_explicit() {
    let f = setup_publisher().await;
    let before = f.bytes();
    f.prepare("prepare").await;
    let app = common::connect_config(&f.config).await;
    let counts = app
        .query_one(
            "SELECT (SELECT count(*) FROM awr_team.delivery_source_cursors),
        (SELECT count(*) FROM awr_team.delivery_source_publications)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(counts.get::<_, i64>(0), 0);
    assert_eq!(counts.get::<_, i64>(1), 0);
    app.batch_execute(
        "BEGIN; SELECT set_config('awr.tenant_id','other-tenant',true);
        SELECT set_config('awr.project_id','reader-project',true)",
    )
    .await
    .unwrap();
    assert_eq!(
        app.query_one(
            "SELECT count(*) FROM awr_team.delivery_source_publications",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    app.batch_execute("ROLLBACK").await.unwrap();
    let policies = f.admin.query_one("SELECT count(*) FROM pg_class WHERE oid IN
        ('awr_team.delivery_source_cursors'::regclass,'awr_team.delivery_source_publications'::regclass)
        AND relrowsecurity AND relforcerowsecurity", &[]).await.unwrap();
    assert_eq!(policies.get::<_, i64>(0), 2);
    f.admin
        .execute(
            "UPDATE awr_team.source_snapshots SET source_ref_json=jsonb_set(source_ref_json,
        '{sole_source,kind}','\"private_management_repo\"') WHERE id=$1",
            &[&f.set.source_snapshot_id],
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .source_publication_status(TENANT, PROJECT, A, "a")
            .await,
        Err(PgError::Unsupported(_))
    ));
    assert_eq!(f.bytes(), before);
}

#[tokio::test]
async fn confirmed_note_is_not_current_after_lock_identity_changes_and_query_does_not_repair_it() {
    for replace in [false, true] {
        let f = setup_publisher().await;
        f.publish("publish", &f.selection).await;
        let lock = std::fs::read_dir(&f.root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.file_name().unwrap().to_string_lossy().ends_with(".lock"))
            .unwrap();
        std::fs::rename(&lock, lock.with_extension("old-lock")).unwrap();
        if replace {
            std::fs::write(&lock, b"").unwrap();
        }
        let before = f.bytes();
        let count = std::fs::read_dir(&f.root).unwrap().count();
        let status = f.status().await;
        assert_eq!(status["source_synchronized"], false);
        assert_eq!(status["history"][0]["source_synchronized_at_commit"], true);
        assert_eq!(status["history"][0]["source_current"], false);
        assert_eq!(std::fs::read_dir(&f.root).unwrap().count(), count);
        assert_eq!(lock.exists(), replace);
        assert_eq!(f.bytes(), before);
    }
}
