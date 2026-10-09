#![cfg(feature = "pg-tests")]
//! Isolated mechanism regressions, not native team business acceptance.
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

fn schedule_query(limit: i32, cursor: Option<String>) -> DeliveryScheduleQuery {
    DeliveryScheduleQuery {
        work_id: "a".into(),
        connector_id: "git".into(),
        limit,
        cursor,
    }
}

async fn view(f: &Fixture) -> Value {
    f.store
        .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None))
        .await
        .unwrap()
}

async fn durable_state(f: &Fixture) -> Value {
    f.admin.query_one("SELECT jsonb_build_object(
        'project',(SELECT to_jsonb(p) FROM awr_team.projects p WHERE tenant_id='reader-tenant' AND id='reader-project'),
        'connector',(SELECT to_jsonb(c) FROM awr_team.delivery_connectors c WHERE id='git'),
        'intents',(SELECT COALESCE(jsonb_agg(to_jsonb(i) ORDER BY id),'[]') FROM awr_team.delivery_integration_intents i),
        'requests',(SELECT count(*) FROM awr_team.delivery_sync_requests),
        'inspections',(SELECT count(*) FROM awr_team.delivery_inspections),
        'inbox',(SELECT count(*) FROM awr_team.delivery_inbox),
        'events',(SELECT count(*) FROM awr_team.events),
        'completion',(SELECT count(*) FROM awr_team.completion_receipts))", &[]).await.unwrap().get(0)
}

async fn prepare_and_reject(f: &Fixture, key: &str) -> String {
    let mut request = f.request.clone();
    request.request_id = key.into();
    let prepared = f
        .store
        .prepare_integration(TENANT, PROJECT, SUPERVISOR, request)
        .await
        .unwrap();
    let id = prepared["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.store
        .reject_prepared_integration(
            TENANT,
            PROJECT,
            SUPERVISOR,
            RejectPreparedDeliveryIntegration {
                request_id: format!("reject-{key}"),
                read_set: f.set.clone(),
                integration_id: id.clone(),
                reason: "Withdraw before dispatch".into(),
            },
        )
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn current_schedule_derives_the_read_set_without_creating_a_permit_or_journal() {
    let f = setup_integration().await;
    let prepared = f.prepared().await;
    let before = durable_state(&f).await;
    let (first, second) = tokio::join!(
        f.store
            .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None)),
        f.store
            .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None))
    );
    let first = first.unwrap();
    assert_eq!(first, second.unwrap());
    assert_eq!(first["read_set"], json!(f.set));
    assert_eq!(first["selection_state"], "current");
    assert_eq!(
        first["selection"]["candidate"],
        json!(f.selection.candidate)
    );
    assert_eq!(first["connector"]["resource"], RESOURCE);
    assert_eq!(
        first["integrations"][0]["integration_id"],
        prepared["integration_id"]
    );
    assert_eq!(first["integrations"][0]["query_original"], false);
    assert_eq!(first["execution_authorized"], false);
    assert_eq!(first["acceptance_ready"], false);
    assert_eq!(first["source_synchronized"], false);
    assert_eq!(durable_state(&f).await, before);
    let serialized = first.to_string();
    for private in [
        "issuer_secret_hash",
        "issuer_credential_id",
        "issuer_authority_binding",
        "lease_authority_binding",
        SUPERVISOR,
        WORKER,
    ] {
        assert!(!serialized.contains(private));
    }
    assert!(!serialized.contains(&workstream_credential_hash(SUPERVISOR).unwrap()));
}

#[tokio::test]
async fn unmapped_members_clients_and_scopes_cannot_schedule_the_worker() {
    let f = setup_integration().await;
    for token in [A, B, NONE, SUPERVISOR, REVIEWER, "invalid"] {
        assert!(
            f.store
                .schedule(TENANT, PROJECT, token, schedule_query(32, None))
                .await
                .is_err()
        );
    }
    for (tenant, project, work, connector) in [
        ("other-tenant", PROJECT, "a", "git"),
        (TENANT, "other-project", "a", "git"),
        (TENANT, PROJECT, "c", "git"),
        (TENANT, PROJECT, "b-private", "git"),
        (TENANT, PROJECT, "a", "missing"),
    ] {
        let mut q = schedule_query(32, None);
        q.work_id = work.into();
        q.connector_id = connector.into();
        assert!(f.store.schedule(tenant, project, WORKER, q).await.is_err());
    }
    f.admin
        .batch_execute("UPDATE awr_team.delivery_connectors SET principal_client_id='other-client'")
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None))
            .await,
        Err(PgError::Forbidden)
    ));
    f.admin.batch_execute("UPDATE awr_team.delivery_connectors SET principal_client_id='cli-worker',principal_actor_id='reviewer'").await.unwrap();
    assert!(matches!(
        f.store
            .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None))
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn disabled_mapping_wrong_epoch_or_non_adapter_origin_refuses_admission() {
    let f = setup_integration().await;
    for (change, restore) in [
        (
            "UPDATE awr_team.delivery_connectors SET enabled=false",
            "UPDATE awr_team.delivery_connectors SET enabled=true",
        ),
        (
            "UPDATE awr_team.delivery_connectors SET fact_source='operator_recorded'",
            "UPDATE awr_team.delivery_connectors SET fact_source='adapter_observation'",
        ),
        (
            "UPDATE awr_team.delivery_connectors SET coordinator_epoch='old'",
            "UPDATE awr_team.delivery_connectors SET coordinator_epoch='epoch-a'",
        ),
    ] {
        f.admin.batch_execute(change).await.unwrap();
        assert!(
            f.store
                .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None))
                .await
                .is_err()
        );
        f.admin.batch_execute(restore).await.unwrap();
        assert_eq!(view(&f).await["selection_state"], "current");
    }
}

#[tokio::test]
async fn credential_and_live_read_grant_revocation_apply_on_every_poll() {
    let f = setup_integration().await;
    for (change, restore) in [
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-worker'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='integration-worker'",
        ),
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='integration-worker'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='integration-worker'",
        ),
        (
            "UPDATE awr_team.actors SET status='disabled' WHERE id='integrator'",
            "UPDATE awr_team.actors SET status='active' WHERE id='integrator'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET active=false WHERE client_id='cli-worker'",
            "UPDATE awr_team.workstream_grants SET active=true WHERE client_id='cli-worker'",
        ),
        (
            "UPDATE awr_team.projects SET status='frozen' WHERE tenant_id='reader-tenant' AND id='reader-project'",
            "UPDATE awr_team.projects SET status='active' WHERE tenant_id='reader-tenant' AND id='reader-project'",
        ),
    ] {
        f.admin.batch_execute(change).await.unwrap();
        assert!(
            f.store
                .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None))
                .await
                .is_err()
        );
        f.admin.batch_execute(restore).await.unwrap();
        assert_eq!(view(&f).await["selection_state"], "current");
    }
    // Reading the schedule never grants an effect, even to a read-only principal.
    f.admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,can_manage=false WHERE client_id='cli-worker'").await.unwrap();
    assert_eq!(view(&f).await["execution_authorized"], false);
}

#[tokio::test]
async fn missing_selection_retains_the_original_prepared_intent_as_history() {
    let f = setup_integration().await;
    let id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.admin
        .batch_execute("DELETE FROM awr_team.delivery_selections")
        .await
        .unwrap();
    let before = durable_state(&f).await;
    let result = view(&f).await;
    assert_eq!(result["selection_state"], "missing");
    assert!(result["selection"].is_null());
    assert_eq!(result["integrations"][0]["integration_id"], id);
    assert_eq!(result["integrations"][0]["state"], "prepared");
    assert_eq!(result["integrations"][0]["original_read_set"], json!(f.set));
    assert_eq!(durable_state(&f).await, before);
}

#[tokio::test]
async fn ownership_fence_and_contract_changes_make_selection_stale_without_rebinding_history() {
    let f = setup_integration().await;
    f.prepared().await;
    f.admin.batch_execute("UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=2 WHERE work_id='a'").await.unwrap();
    let result = view(&f).await;
    assert_eq!(result["read_set"]["ownership_version"], "2");
    assert_eq!(result["selection_state"], "stale");
    assert_eq!(
        result["integrations"][0]["original_read_set"]["ownership_version"],
        "1"
    );
    f.admin.batch_execute("UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=1 WHERE work_id='a';
        UPDATE awr_team.work_runtime SET last_fence=last_fence+1 WHERE work_id='a'").await.unwrap();
    assert_eq!(view(&f).await["selection_state"], "stale");
    f.admin
        .execute(
            "UPDATE awr_team.work_runtime SET last_fence=$1 WHERE work_id='a'",
            &[&f.selection.fence.parse::<i64>().unwrap()],
        )
        .await
        .unwrap();
    let mut contract: awr_team::WorkContract = serde_json::from_value(
        f.admin
            .query_one(
                "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='a'",
                &[],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    contract
        .acceptance
        .push("Verify another requirement".into());
    f.admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='a'",&[&json!(contract),&contract.hash().unwrap()]).await.unwrap();
    let result = view(&f).await;
    assert_eq!(result["selection_state"], "stale");
    assert_eq!(
        result["read_set"]["contract_hash"],
        contract.hash().unwrap()
    );
    assert_eq!(
        result["integrations"][0]["original_read_set"]["contract_hash"],
        f.set.contract_hash
    );
}

#[tokio::test]
async fn original_dispatched_request_remains_query_only_after_revoked_issuer_and_expired_lease() {
    let f = setup_integration().await;
    let prepared = f.prepared().await;
    let id = prepared["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    assert!(
        f.dispatched(id, lease["lease_id"].as_str().unwrap())
            .await
            .permit
            .is_some()
    );
    let fact = f
        .ingest_record("unknown", f.observation(id, IntegrationOutcome::Unknown))
        .await;
    f.store
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("confirm-unknown", id, &fact),
        )
        .await
        .unwrap();
    f.admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='integration-supervisor';
        UPDATE awr_team.delivery_integration_intents SET lease_expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
    let before = durable_state(&f).await;
    let item = &view(&f).await["integrations"][0];
    assert_eq!(item["state"], "unknown");
    assert_eq!(item["query_original"], true);
    assert_eq!(item["lease_live"], false);
    assert_eq!(item["execution_authorized"], false);
    assert_eq!(item["integration_request"]["request_id"], id);
    assert_eq!(item["candidate"], json!(f.selection.candidate));
    assert_eq!(f.guards().await, 1);
    assert_eq!(durable_state(&f).await, before);
}

#[tokio::test]
async fn source_activation_uses_new_admission_while_retaining_original_dispatch_binding() {
    let f = setup_integration().await;
    let id = f.prepared().await["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let lease = f.leased(&id).await;
    f.dispatched(&id, lease["lease_id"].as_str().unwrap()).await;
    let catalog: Value = f
        .admin
        .query_one(
            "SELECT catalog_json FROM awr_team.workstream_catalogs WHERE snapshot_id=$1",
            &[&f.set.source_snapshot_id],
        )
        .await
        .unwrap()
        .get(0);
    let rows=f.admin.query("SELECT c.contract_json,o.workstream_id FROM awr_team.work_contracts c
        JOIN awr_team.workstream_snapshot_ownership o USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        WHERE c.snapshot_id=$1 ORDER BY c.work_id",&[&f.set.source_snapshot_id]).await.unwrap();
    let contracts = rows
        .into_iter()
        .map(|r| {
            let mut contract: awr_team::WorkContract = serde_json::from_value(r.get(0)).unwrap();
            // The dispatched integration belongs to `a`. Source admission derives its
            // impact from persisted facts and refuses to change a contract while an
            // integration of that work is unsettled, so the change goes to an unrelated
            // work: the activation proceeds and the dispatched `a` keeps its binding.
            if contract.work_id.as_str() == "b-private" {
                contract
                    .acceptance
                    .push("Check the new source requirement".into());
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
    integration_fixture::run(&f.reads,A,"release-before-source","claim.release",json!({"session_id":"session-a","expected_session_version":"1",
        "claim_id":f.selection.claim_id,"expected_fence":f.selection.fence,"expected_lease_version":f.selection.lease_version})).await;
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
    // Source authority changes do not rotate the service coordinator epoch.
    let changed_view = view(&f).await;
    assert_eq!(changed_view["selection_state"], "stale");
    assert_ne!(
        changed_view["read_set"]["source_snapshot_id"],
        f.set.source_snapshot_id
    );
    assert_eq!(
        changed_view["read_set"]["coordinator_epoch"],
        f.set.coordinator_epoch
    );
    let current = integration_fixture::set(&prepare(&f.reads, WORKER, "a").await);
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "readmit-current-source".into(),
                read_set: current.clone(),
                expected_connector_version: "1".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "git".into(),
                    provider: "local_git".into(),
                    resource: RESOURCE.into(),
                    principal_actor_id: "integrator".into(),
                    principal_client_id: "cli-worker".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let before = durable_state(&f).await;
    let result = view(&f).await;
    assert_eq!(result["read_set"], json!(current));
    assert_ne!(
        result["read_set"]["source_snapshot_id"],
        f.set.source_snapshot_id
    );
    assert_eq!(result["selection_state"], "stale");
    assert_eq!(result["integrations"][0]["original_read_set"], json!(f.set));
    assert_eq!(result["integrations"][0]["original_connector_version"], "1");
    assert_eq!(result["integrations"][0]["integration_id"], id);
    assert_eq!(result["integrations"][0]["query_original"], true);
    assert_eq!(durable_state(&f).await, before);
}

#[tokio::test]
async fn bounded_pages_preserve_order_and_exclude_new_intents_until_the_next_scan() {
    let f = setup_integration().await;
    let mut expected = Vec::new();
    for i in 0..4 {
        expected.push(prepare_and_reject(&f, &format!("history-{i}")).await);
    }
    let first = f
        .store
        .schedule(TENANT, PROJECT, WORKER, schedule_query(1, None))
        .await
        .unwrap();
    assert_eq!(first["integrations"].as_array().unwrap().len(), 1);
    assert_eq!(first["integrations"][0]["integration_id"], expected[0]);
    let new = prepare_and_reject(&f, "new-history").await;
    let mut collected = vec![expected[0].clone()];
    let mut cursor = first["next_cursor"].as_str().map(str::to_owned);
    while let Some(c) = cursor {
        let page = f
            .store
            .schedule(TENANT, PROJECT, WORKER, schedule_query(1, Some(c)))
            .await
            .unwrap();
        for item in page["integrations"].as_array().unwrap() {
            collected.push(item["integration_id"].as_str().unwrap().into());
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
    }
    assert_eq!(collected, expected);
    assert_eq!(
        view(&f).await["integrations"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["integration_id"],
        new
    );
}

#[tokio::test]
async fn cursor_is_bound_to_scope_mapping_selection_and_live_authority() {
    let f = setup_integration().await;
    prepare_and_reject(&f, "history-1").await;
    prepare_and_reject(&f, "history-2").await;
    let first = f
        .store
        .schedule(TENANT, PROJECT, WORKER, schedule_query(1, None))
        .await
        .unwrap();
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    for (change, restore) in [
        (
            "UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-worker'",
            "UPDATE awr_team.workstream_grants SET grant_version=grant_version-1 WHERE client_id='cli-worker'",
        ),
        (
            "UPDATE awr_team.delivery_connectors SET version=version+1",
            "UPDATE awr_team.delivery_connectors SET version=version-1",
        ),
        (
            "UPDATE awr_team.delivery_selections SET selection_version=selection_version+1",
            "UPDATE awr_team.delivery_selections SET selection_version=selection_version-1",
        ),
        (
            "UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=2 WHERE work_id='a'",
            "UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=1 WHERE work_id='a'",
        ),
    ] {
        f.admin.batch_execute(change).await.unwrap();
        assert!(matches!(
            f.store
                .schedule(
                    TENANT,
                    PROJECT,
                    WORKER,
                    schedule_query(1, Some(cursor.clone()))
                )
                .await,
            Err(PgError::CursorExpired)
        ));
        f.admin.batch_execute(restore).await.unwrap();
    }
    let mut tampered: Value = serde_json::from_str(&cursor).unwrap();
    tampered["after_id"] = json!("another-work-intent");
    assert!(matches!(
        f.store
            .schedule(
                TENANT,
                PROJECT,
                WORKER,
                schedule_query(1, Some(tampered.to_string()))
            )
            .await,
        Err(PgError::CursorExpired)
    ));
    let mut q = schedule_query(1, Some(cursor));
    q.work_id = "c".into();
    assert!(f.store.schedule(TENANT, PROJECT, WORKER, q).await.is_err());
}

#[tokio::test]
async fn resource_remapping_does_not_schedule_an_old_repository_effect() {
    let f = setup_integration().await;
    f.prepared().await;
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "remap".into(),
                read_set: f.set.clone(),
                expected_connector_version: "1".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "git".into(),
                    provider: "other_provider".into(),
                    resource: "fixture://different-repository".into(),
                    principal_actor_id: "integrator".into(),
                    principal_client_id: "cli-worker".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let result = view(&f).await;
    assert_eq!(result["connector"]["provider"], "other_provider");
    assert_eq!(result["selection_state"], "stale");
    assert!(result["integrations"].as_array().unwrap().is_empty());
    assert_eq!(f.guards().await, 1);
}

#[tokio::test]
async fn forged_read_sets_flags_malformed_cursors_and_excessive_limits_are_refused() {
    let f = setup_integration().await;
    for field in ["read_set", "actor_id", "approved", "execution_authorized"] {
        let mut value = json!(schedule_query(1, None));
        value[field] = json!(true);
        assert!(serde_json::from_value::<DeliveryScheduleQuery>(value).is_err());
    }
    for limit in [0, 33, -1] {
        assert!(
            f.store
                .schedule(TENANT, PROJECT, WORKER, schedule_query(limit, None))
                .await
                .is_err()
        );
    }
    for cursor in [
        "invalid".into(),
        "x".repeat(4097),
        "{\"binding\":\"forged\",\"after_id\":\"a\",\"upper_id\":\"b\"}".into(),
    ] {
        assert!(matches!(
            f.store
                .schedule(TENANT, PROJECT, WORKER, schedule_query(1, Some(cursor)))
                .await,
            Err(PgError::CursorExpired)
        ));
    }
}

#[tokio::test]
async fn corrupted_current_contract_and_candidate_fail_closed_without_effects() {
    let f = setup_integration().await;
    f.admin
        .batch_execute(
            "UPDATE awr_team.work_contracts SET contract_hash=repeat('b',64) WHERE work_id='a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None))
            .await,
        Err(PgError::SourceDivergence)
    ));
    f.admin
        .execute(
            "UPDATE awr_team.work_contracts SET contract_hash=$1 WHERE work_id='a'",
            &[&f.set.contract_hash],
        )
        .await
        .unwrap();
    f.admin.batch_execute("UPDATE awr_team.delivery_candidates SET body_json=jsonb_set(body_json,'{binding,candidate_version}','\"changed\"')").await.unwrap();
    assert!(matches!(
        f.store
            .schedule(TENANT, PROJECT, WORKER, schedule_query(32, None))
            .await,
        Err(PgError::SourceDivergence)
    ));
}

#[tokio::test]
async fn leased_schedule_observes_expiry_without_renewing_or_reclaiming() {
    let f = setup_integration().await;
    let prepared = f.prepared().await;
    let id = prepared["integration_id"].as_str().unwrap();
    let lease = f.leased(id).await;
    let before = durable_state(&f).await;
    let result = view(&f).await;
    assert_eq!(result["integrations"][0]["state"], "leased");
    assert_eq!(result["integrations"][0]["lease_id"], lease["lease_id"]);
    assert_eq!(result["integrations"][0]["lease_live"], true);
    assert_eq!(result["integrations"][0]["query_original"], false);
    assert_eq!(durable_state(&f).await, before);
    f.admin.batch_execute("UPDATE awr_team.delivery_integration_intents SET lease_expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
    let before = durable_state(&f).await;
    let result = view(&f).await;
    assert_eq!(result["integrations"][0]["lease_live"], false);
    assert_eq!(result["integrations"][0]["lease_id"], lease["lease_id"]);
    assert_eq!(durable_state(&f).await, before);
}

#[tokio::test]
async fn response_byte_bound_returns_every_large_record_through_continuations() {
    use sha2::{Digest, Sha256};
    let manifest = ArtifactManifest {
        entries: (0..24)
            .map(|i| ArtifactEntry {
                artifact_id: format!("package-{i}"),
                sha256: format!("{:x}", Sha256::digest(CONTENT)),
                byte_length: CONTENT.len().to_string(),
                locator: format!("git-path:src/api/{i}/{}", "measurement/".repeat(125)),
            })
            .collect(),
    };
    let candidate: DeliveryCandidate = serde_json::from_value(json!({"binding":{
        "tenant_id":TENANT,"project_id":PROJECT,"scope_id":"main","workstream_id":awr_core::Id::from(1).to_string(),
        "work_id":"a","candidate_id":"large-candidate","candidate_version":"1","contract_hash":"b".repeat(64),
        "manifest_digest":manifest.digest().unwrap(),"source_revision":{"resource":RESOURCE,"format":"git_sha256","value":"a".repeat(64)},
        "required_checks":["report"],"target":{"resource":RESOURCE,"reference":"main","precondition":{"kind":"missing"}}},"manifest":manifest})).unwrap();
    let f = setup_integration_with_candidate(Some(candidate)).await;
    let mut expected = Vec::new();
    for i in 0..9 {
        expected.push(prepare_and_reject(&f, &format!("large-history-{i}")).await);
    }
    let before = durable_state(&f).await;
    let mut cursor = None;
    let mut actual = Vec::new();
    let mut pages = 0;
    loop {
        let result = f
            .store
            .schedule(TENANT, PROJECT, WORKER, schedule_query(32, cursor))
            .await
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() <= 262144);
        for item in result["integrations"].as_array().unwrap() {
            actual.push(item["integration_id"].as_str().unwrap().to_owned());
        }
        pages += 1;
        assert!(pages <= expected.len());
        cursor = result["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
        assert_eq!(result["truncated"], true);
    }
    assert!(pages > 1);
    assert_eq!(actual, expected);
    assert_eq!(durable_state(&f).await, before);
}
