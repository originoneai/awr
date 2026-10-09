#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::*;
use awr_team::delivery::*;
use awr_team_pg::*;
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

async fn read(store: &WorkstreamReadStore, op: &str) -> Value {
    let mut q = query(op);
    q.work_id = Some("a".into());
    store.query(TENANT, PROJECT, A, q).await.unwrap()
}

async fn issue(db: &str, id: &str, work: &str, actions: &[AuthorizedAction]) {
    let grant = AgentAuthorization {
        id: id.into(),
        authorizer_person_id: PersonId::new("member-a").unwrap(),
        responsible_person_id: PersonId::new("member-a").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: work.into(),
        },
        actions: actions.iter().copied().collect(),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1000,
        binding_id: Some("binding-a".into()),
    };
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: format!("issue-{id}"),
                authorization: grant,
            },
        )
        .await
        .unwrap();
}

async fn bind_agent(admin: &Client) {
    enable_writes(admin).await;
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
        UPDATE awr_team.project_memberships SET role='maintainer',business_roles='[\"reviewer\"]'::jsonb,agent_review=true WHERE actor_id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
          ('reader-tenant','reader-project','member-a','Simulated Member A','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"fixture-controller\"}'::jsonb);
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
          ('reader-tenant','reader-project','binding-a','member-a','agent','active')").await.unwrap();
}

async fn round(admin: &Client, state: &str) {
    admin.execute("INSERT INTO awr_team.review_rounds(tenant_id,project_id,id,work_id,round_index,bundle_hash,contract_hash,author_actor_id,state)
        SELECT tenant_id,project_id,'round-a',work_id,1,'bundle-a',contract_hash,'reviewer',$1 FROM awr_team.work_contracts WHERE work_id='a'",
        &[&state]).await.unwrap();
}

fn inbox_hint(page: &Value) -> Value {
    page["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["work_id"] == "a")
        .map(|v| v["guidance"].clone())
        .unwrap_or(Value::Null)
}

async fn assert_consistent(store: &WorkstreamReadStore, code: &str) {
    let page = store
        .query(TENANT, PROJECT, A, query("work.inbox"))
        .await
        .unwrap();
    let expected = inbox_hint(&page);
    assert_eq!(expected["code"], code);
    for op in ["work.observe", "work.prepare", "work.snapshot"] {
        let response = read(store, op).await;
        let hint = if op == "work.snapshot" {
            &response["data"]["observation"]["guidance"]
        } else {
            &response["data"]["guidance"]
        };
        assert_eq!(
            hint["code"], code,
            "{op} must agree with current inbox facts"
        );
        assert_eq!(hint["action"]["query"], expected["action"]["query"]);
        assert!(
            hint["because"]
                .as_array()
                .is_some_and(|v| !v.is_empty() && v.len() <= 3)
        );
        assert_eq!(hint["action"]["op"], hint["action"]["query"]["op"]);
    }
}

#[tokio::test]
async fn split_read_and_review_grants_use_the_same_current_guidance() {
    let (_g, admin, db, store) = setup().await;
    bind_agent(&admin).await;
    issue(&db, "read-a", "a", &[AuthorizedAction::Inspect]).await;
    issue(&db, "review-a", "a", &[AuthorizedAction::Review]).await;
    round(&admin, "open").await;
    assert_consistent(&store, "review").await;
}

#[tokio::test]
async fn advice_changes_never_change_required_context_or_grant_commands() {
    let (_g, admin, db, store) = setup().await;
    bind_agent(&admin).await;
    issue(&db, "read-a", "a", &[AuthorizedAction::Inspect]).await;
    let before = read(&store, "work.prepare").await;
    issue(&db, "review-c", "c", &[AuthorizedAction::Review]).await;
    round(&admin, "open").await;
    assert_eq!(
        read(&store, "work.observe").await["data"]["guidance"]["code"],
        "inspect_only"
    );
    issue(&db, "review-a", "a", &[AuthorizedAction::Review]).await;
    assert_consistent(&store, "review").await;
    let after = read(&store, "work.prepare").await;
    assert_eq!(
        before["data"]["context_hash"],
        after["data"]["context_hash"]
    );
    assert_eq!(
        before["data"]["context_complete"],
        after["data"]["context_complete"]
    );
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &after,
                    "no-development-grant",
                    "session.start",
                    json!({"conversation_id":"not-authorized"})
                )
            )
            .await,
        Err(PgError::Forbidden)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='review-a'",
        )
        .await
        .unwrap();
    let revoked = read(&store, "work.prepare").await;
    assert_eq!(
        revoked["data"]["context_hash"],
        before["data"]["context_hash"]
    );
    assert_eq!(revoked["data"]["guidance"]["code"], "inspect_only");
    assert!(
        inbox_hint(
            &store
                .query(TENANT, PROJECT, A, query("work.inbox"))
                .await
                .unwrap()
        )
        .is_null()
    );
}

#[tokio::test]
async fn current_review_rework_and_finalization_are_role_aware() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET assignment_grant=true WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    round(&admin, "open").await;
    assert_consistent(&store, "review").await;
    for state in ["rejected", "invalidated"] {
        admin
            .execute("UPDATE awr_team.review_rounds SET state=$1", &[&state])
            .await
            .unwrap();
        assert_consistent(&store, "rework").await;
    }
    admin.batch_execute("UPDATE awr_team.review_rounds SET state='approved';
        UPDATE awr_team.project_memberships SET role='maintainer',business_roles='[\"deliverer\"]'::jsonb WHERE actor_id='agent'").await.unwrap();
    assert_consistent(&store, "finalization").await;
}

#[tokio::test]
async fn historical_unsettled_effects_override_generic_session_advice() {
    let (_g, admin, _, store) = setup().await;
    admin.batch_execute("INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','old-effect','a','named','fixture-effect','unknown')").await.unwrap();
    assert_consistent(&store, "recovery").await;
    admin
        .batch_execute("UPDATE awr_team.resource_reservations SET state='released'")
        .await
        .unwrap();
    assert_eq!(
        read(&store, "work.observe").await["data"]["guidance"]["code"],
        "inspect_only"
    );
}

#[tokio::test]
async fn observe_budget_covers_the_complete_utf8_response_and_keeps_legacy_pr_advice() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    admin.batch_execute("INSERT INTO awr_team.pr_deliveries(tenant_id,project_id,id,work_id,repository,pr_number,pr_url,head_sha,contract_hash,fact_source,observed_at,registered_by_actor_id,state)
        SELECT tenant_id,project_id,'legacy-pr',work_id,'fixture/repository',1,'https://example.invalid/pr/1',repeat('a',40),contract_hash,'operator_recorded_observation','2026-10-09T00:00:00Z','agent','active'
        FROM awr_team.work_contracts WHERE work_id='a'").await.unwrap();
    let response = read(&store, "work.observe").await;
    assert_eq!(response["data"]["guidance"]["code"], "inspect_delivery");
    let bytes = serde_json::to_vec(&response).unwrap().len();
    let mut q = query("work.observe");
    q.work_id = Some("a".into());
    q.max_context_bytes = Some(bytes + 32);
    let bounded = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= bytes + 32);
    q.max_context_bytes = Some(1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ResponseTooLarge)
    ));
}

async fn supervisor(admin: &Client) {
    enable_writes(admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET assignment_grant=true WHERE actor_id='agent'",
        )
        .await
        .unwrap();
}

async fn claim(store: &WorkstreamReadStore, accept: bool) -> Value {
    let p = prepare(store, A, "a").await;
    let mut args = json!({"session_id":"session-a","expected_session_version":"1",
        "expected_responsibility_version":p["data"]["responsibility"]["version"],
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":3600});
    if accept {
        args["assignment_request_key"] =
            p["data"]["responsibility"]["pending"]["transfer_request_key"].clone();
    }
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "take-inbox",
                if accept {
                    "task.accept_assignment"
                } else {
                    "task.claim_available"
                },
                args,
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone()
}

async fn select_candidate(
    store: &WorkstreamReadStore,
    admin: &Client,
    db: &str,
) -> (DeliverySyncStore, DeliveryReadSet, DeliveryCandidate) {
    supervisor(admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let claim = claim(store, false).await;
    let p = prepare(store, A, "a").await;
    let set:DeliveryReadSet=serde_json::from_value(json!({"work_id":"a","workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],"authority_version":p["authority_version"],
        "ownership_version":p["data"]["ownership_version"],"contract_hash":p["data"]["contract_hash"]})).unwrap();
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: "package".into(),
            sha256: "e".repeat(64),
            byte_length: "12".into(),
            locator: "fixture://artifact/package".into(),
        }],
    };
    let candidate:DeliveryCandidate=serde_json::from_value(json!({"binding":{"tenant_id":TENANT,"project_id":PROJECT,
        "scope_id":"main","workstream_id":set.workstream_id,"work_id":"a","candidate_id":"candidate-a","candidate_version":"1",
        "contract_hash":set.contract_hash,"manifest_digest":manifest.digest().unwrap(),"source_revision":{"resource":"fixture://neutral-repository","format":"git_sha256","value":"a".repeat(64)},
        "required_checks":["report"],"target":{"resource":"fixture://neutral-repository","reference":"main","precondition":{"kind":"missing"}}},"manifest":manifest})).unwrap();
    let sync = DeliverySyncStore::from_config(common::with_app_role(&common::test_config(), db));
    sync.select_candidate(
        TENANT,
        PROJECT,
        A,
        SelectDeliveryCandidate {
            request_id: "select-inbox".into(),
            read_set: set.clone(),
            expected_selected_digest: None,
            session_id: "session-a".into(),
            claim_id: claim["claim_id"].as_str().unwrap().into(),
            fence: claim["fence"].as_str().unwrap().into(),
            lease_version: claim["lease_version"].as_str().unwrap().into(),
            candidate: candidate.clone(),
        },
    )
    .await
    .unwrap();
    sync.configure_connector(
        TENANT,
        PROJECT,
        A,
        ConfigureDeliveryConnector {
            request_id: "configure-inbox".into(),
            read_set: set.clone(),
            expected_connector_version: "0".into(),
            mapping: DeliveryConnectorMapping {
                connector_id: "neutral".into(),
                provider: "reference".into(),
                resource: "fixture://neutral-repository".into(),
                principal_actor_id: "agent".into(),
                principal_client_id: "cli-a".into(),
                fact_source: FactSource::CallerDeclared,
                enabled: true,
            },
        },
    )
    .await
    .unwrap();
    (sync, set, candidate)
}

#[tokio::test]
async fn neutral_candidate_checks_and_reselection_share_facts_without_inferred_acceptance() {
    let (_g, admin, db, store) = setup().await;
    let (sync, set, candidate) = select_candidate(&store, &admin, &db).await;
    assert_consistent(&store, "verification").await;
    let before = read(&store, "work.prepare").await;
    let reserved = sync
        .reserve_inspection(
            TENANT,
            PROJECT,
            A,
            ReserveDeliveryInspection {
                request_id: "inspect-feedback".into(),
                read_set: set.clone(),
                connector_id: "neutral".into(),
                connector_version: "1".into(),
                candidate_digest: candidate.binding.digest().unwrap(),
                lease_seconds: 60,
            },
        )
        .await
        .unwrap();
    sync.ingest_facts(
        TENANT,
        PROJECT,
        A,
        IngestDeliveryFacts {
            request_id: "ingest-feedback".into(),
            read_set: set,
            connector_id: "neutral".into(),
            inspection_id: reserved["data"]["inspection_id"].as_str().unwrap().into(),
            event_id: "report-event".into(),
            records: vec![DeliveryEnvelope {
                protocol: DELIVERY_PROTOCOL.into(),
                protocol_version: DELIVERY_PROTOCOL_VERSION,
                record: DeliveryRecord::Verification(VerificationRun {
                    binding: candidate.binding.clone(),
                    run_id: "run-a".into(),
                    check: "report".into(),
                    outcome: VerificationOutcome::Passed,
                    result_artifact: Some(candidate.manifest.entries[0].clone()),
                    provenance: FactProvenance {
                        source: FactSource::CallerDeclared,
                        reference: "fixture://check/report".into(),
                        observed_at_unix_ms: None,
                        recorded_at_unix_ms: 123,
                    },
                }),
            }],
        },
    )
    .await
    .unwrap();
    assert_consistent(&store, "verification").await;
    let after = read(&store, "work.prepare").await;
    assert_eq!(
        before["data"]["context_hash"],
        after["data"]["context_hash"]
    );
    let observed = read(&store, "work.observe").await;
    assert_eq!(
        observed["data"]["collaboration"]["verification"][0]["outcome"],
        "passed"
    );
    assert_eq!(
        observed["data"]["collaboration"]["acceptance_inferred"],
        false
    );
    assert_eq!(
        read(&store, "work.snapshot").await["data"]["observation"]["collaboration"],
        observed["data"]["collaboration"]
    );
    admin
        .batch_execute(
            "UPDATE awr_team.delivery_selections SET ownership_version=ownership_version+1",
        )
        .await
        .unwrap();
    assert_consistent(&store, "delivery_changed").await;
}

#[tokio::test]
async fn unknown_integration_always_inspects_the_original_request_after_reselection() {
    let (_g, admin, db, store) = setup().await;
    select_candidate(&store, &admin, &db).await;
    admin.batch_execute("INSERT INTO awr_team.delivery_integration_intents(tenant_id,project_id,id,work_id,connector_id,request_json,eligibility_json,eligibility_digest,
        issuer_credential_id,issuer_secret_hash,issuer_authority_binding,state,dispatched_at)
        VALUES('reader-tenant','reader-project','original-intent','a','neutral','{}','{}',repeat('a',64),'reader-a','sha256:'||repeat('b',64),repeat('c',64),'unknown',clock_timestamp())").await.unwrap();
    for _ in 0..2 {
        assert_consistent(&store, "integration_unknown").await;
        let observed = read(&store, "work.observe").await;
        assert_eq!(
            observed["data"]["guidance"]["action"]["query"]["request_id"],
            "original-intent"
        );
        assert!(!observed.to_string().contains("issuer_secret_hash"));
        admin
            .batch_execute(
                "UPDATE awr_team.delivery_selections SET selection_version=selection_version+1",
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn completed_delivery_has_source_followup_until_confirmed_without_resuming_execution() {
    let (_g, admin, db, store) = setup().await;
    let (_, _, candidate) = select_candidate(&store, &admin, &db).await;
    let digest = candidate.binding.digest().unwrap();
    admin.execute("INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,delivery_candidate_digest)
        SELECT tenant_id,project_id,'fixture-completion',work_id,'main',contract_hash,'result','deps','evidence','review','[]',$1 FROM awr_team.work_contracts WHERE work_id='a'",
        &[&digest]).await.unwrap();
    admin.batch_execute("UPDATE awr_team.work_runtime SET state='completed',selected_completion_id='fixture-completion' WHERE work_id='a'").await.unwrap();
    assert_consistent(&store, "source_publication").await;
    admin.batch_execute("INSERT INTO awr_team.delivery_source_cursors(tenant_id,project_id,source_snapshot_id,confirmed_fingerprint)
        SELECT tenant_id,project_id,source_snapshot_id,'sha256:'||repeat('a',64) FROM awr_team.delivery_selections WHERE work_id='a';
        INSERT INTO awr_team.delivery_source_publications(tenant_id,project_id,id,source_snapshot_id,work_id,actor_id,client_id,authority_binding,read_set_json,
            candidate_digest,selection_version,metadata_revision,note_json,filesystem_identity_json,before_fingerprint,after_fingerprint,before_bytes,after_bytes,
            projection_json,fence,expires_at,phase,confirmation_json)
        SELECT tenant_id,project_id,'fixture-publication',source_snapshot_id,work_id,'agent','cli-a',repeat('c',64),'{}',binding_digest,selection_version,1,'{}','{}',
            'sha256:'||repeat('a',64),'sha256:'||repeat('b',64),'','', '{}',1,clock_timestamp()+interval '1 minute','confirmed','{}'
        FROM awr_team.delivery_selections WHERE work_id='a'").await.unwrap();
    assert_eq!(
        read(&store, "work.observe").await["data"]["guidance"]["code"],
        "inspect_completion"
    );
    admin.batch_execute("UPDATE awr_team.work_runtime SET state='cancelled',selected_completion_id=NULL WHERE work_id='a'").await.unwrap();
    assert_eq!(
        read(&store, "work.observe").await["data"]["guidance"]["code"],
        "work_inactive"
    );
}

#[tokio::test]
async fn required_context_restoration_precedes_current_review_advice() {
    let (_g, admin, _, store) = setup_with_specs(vec![]).await;
    supervisor(&admin).await;
    round(&admin, "open").await;
    for op in ["work.prepare", "work.snapshot"] {
        let response = read(&store, op).await;
        assert_eq!(response["data"]["context_complete"], false);
        assert_eq!(response["data"]["guidance"]["code"], "restore_context");
        if op == "work.snapshot" {
            assert_eq!(
                response["data"]["guidance"],
                response["data"]["observation"]["guidance"]
            );
        }
    }
}

#[tokio::test]
async fn employee_assignment_and_pool_intake_keep_their_own_session_chain() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let p = read(&store, "work.prepare").await;
    assert_eq!(p["data"]["guidance"]["code"], "declare_client");
    assert_eq!(p["data"]["responsibility"]["relation"], "pool");
    claim(&store, false).await;
    let observed = read(&store, "work.observe").await;
    assert_eq!(observed["data"]["guidance"]["code"], "declare_client");
    assert_eq!(
        observed["data"]["responsibility"]["relation"],
        "owned_by_me"
    );
}

#[tokio::test]
async fn unresolved_effects_precede_renewal_even_when_the_current_claim_is_live() {
    let (_g, admin, db, store) = setup().await;
    select_candidate(&store, &admin, &db).await;
    admin.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()+interval '45 seconds';
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','old-effect','a','named','fixture-effect','unknown')").await.unwrap();
    assert_consistent(&store, "recovery").await;
    admin.batch_execute("UPDATE awr_team.resource_reservations SET state='released';
        INSERT INTO awr_team.delivery_integration_intents(tenant_id,project_id,id,work_id,connector_id,request_json,eligibility_json,eligibility_digest,
            issuer_credential_id,issuer_secret_hash,issuer_authority_binding,state,dispatched_at)
        VALUES('reader-tenant','reader-project','original-intent','a','neutral','{}','{}',repeat('a',64),'reader-a','sha256:'||repeat('b',64),repeat('c',64),'unknown',clock_timestamp())").await.unwrap();
    assert_consistent(&store, "integration_unknown").await;
    admin
        .batch_execute("UPDATE awr_team.delivery_integration_intents SET state='rejected'")
        .await
        .unwrap();
    assert_eq!(
        read(&store, "work.observe").await["data"]["guidance"]["code"],
        "renew_claim"
    );
}
