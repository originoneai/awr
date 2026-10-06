#![cfg(feature = "pg-tests")]
//! Synthetic mechanism regressions, not native client or business acceptance.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team::delivery::*;
use awr_team_pg::*;
use fixture::*;
use serde_json::{Value, json};
use std::sync::MutexGuard;
use tokio_postgres::Client;

const RESOURCE: &str = "fixture://neutral-repository";
const SUPERVISOR: &str =
    "awr1.delivery-supervisor.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const OBSERVER: &str =
    "awr1.delivery-observer.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    admin: Client,
    config: tokio_postgres::Config,
    store: DeliverySyncStore,
    set: DeliveryReadSet,
    selection: SelectDeliveryCandidate,
}

fn read_set(p: &Value) -> DeliveryReadSet {
    serde_json::from_value(json!({"work_id":"a","workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],
        "authority_version":p["authority_version"],"ownership_version":p["data"]["ownership_version"],
        "contract_hash":p["data"]["contract_hash"]})).unwrap()
}

async fn setup_sync() -> Fixture {
    setup_sync_with_agent(false).await
}

async fn setup_sync_with_agent(agent: bool) -> Fixture {
    let (guard, admin, db, reads) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    if agent {
        admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
            UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]'::jsonb WHERE actor_id='agent';
            UPDATE awr_team.project_memberships SET role='admin' WHERE actor_id='reviewer';
            INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity)
            VALUES('reader-tenant','reader-project','member-a','Simulated A','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"synthetic-controller\"}'::jsonb);
            INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
            VALUES('reader-tenant','reader-project','member-binding','member-a','agent','active')").await.unwrap();
        admin.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,'delivery-supervisor','reviewer','cli-supervisor',$2)",
            &[&TENANT,&workstream_credential_hash(SUPERVISOR).unwrap()]).await.unwrap();
        admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage)
            VALUES($1,$2,'reviewer','cli-supervisor',$3,1,true,true,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
        let grant = awr_core::AgentAuthorization {
            id: "delivery-grant".into(),
            authorizer_person_id: awr_core::PersonId::new("member-a").unwrap(),
            responsible_person_id: awr_core::PersonId::new("member-a").unwrap(),
            subject_kind: awr_core::ExecutionSubjectKind::Agent,
            subject_id: "agent".into(),
            client_id: "cli-a".into(),
            session_id: None,
            model_id: None,
            scope: awr_core::AuthorizationScope::Workstream {
                project_id: PROJECT.into(),
                workstream_id: awr_core::Id::from(1).to_string(),
            },
            actions: std::collections::BTreeSet::from([
                awr_core::AuthorizedAction::Inspect,
                awr_core::AuthorizedAction::StartWork,
                awr_core::AuthorizedAction::ClaimCoordination,
            ]),
            expires_at_ms: None,
            status: awr_core::AuthorizationStatus::Active,
            revoked_at_ms: None,
            revoked_by: None,
            verifiable_capabilities: vec![],
            self_reported_skill_hints: vec![],
            parent_authorization_id: None,
            maintainer_person_id: None,
            created_at_ms: 1000,
            binding_id: Some("member-binding".into()),
        };
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db))
            .issue(
                TENANT,
                PROJECT,
                &awr_core::IssueAuthorizationRequest {
                    request_key: "delivery-grant-issue".into(),
                    authorization: grant,
                },
            )
            .await
            .unwrap();
    }
    let p = prepare(&reads, A, "a").await;
    let claim = reads.commands().execute(TENANT,PROJECT,A,command(&p,"take-delivery","task.claim_available",
        json!({"session_id":"session-a","expected_session_version":"1",
            "expected_responsibility_version":p["data"]["responsibility"]["version"],
            "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":3600})))
        .await.unwrap()["receipt"]["data"].clone();
    let p = prepare(&reads, A, "a").await;
    let set = read_set(&p);
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: "package".into(),
            sha256: "e".repeat(64),
            byte_length: "12".into(),
            locator: "fixture://artifact/package".into(),
        }],
    };
    let candidate: DeliveryCandidate = serde_json::from_value(json!({"binding":{
        "tenant_id":TENANT,"project_id":PROJECT,"scope_id":"main","workstream_id":set.workstream_id,
        "work_id":"a","candidate_id":"candidate-a","candidate_version":"1","contract_hash":set.contract_hash,
        "manifest_digest":manifest.digest().unwrap(),"source_revision":{"resource":RESOURCE,"format":"git_sha256","value":"a".repeat(64)},
        "required_checks":["report"],"target":{"resource":RESOURCE,"reference":"main","precondition":{"kind":"missing"}}
    },"manifest":manifest})).unwrap();
    let selection = SelectDeliveryCandidate {
        request_id: "select-a".into(),
        read_set: set.clone(),
        expected_selected_digest: None,
        session_id: "session-a".into(),
        claim_id: claim["claim_id"].as_str().unwrap().into(),
        fence: claim["fence"].as_str().unwrap().into(),
        lease_version: claim["lease_version"].as_str().unwrap().into(),
        candidate,
    };
    let config = common::with_app_role(&common::test_config(), &db);
    let store = DeliverySyncStore::from_config(config.clone());
    store
        .select_candidate(TENANT, PROJECT, A, selection.clone())
        .await
        .unwrap();
    let f = Fixture {
        _guard: guard,
        admin,
        config,
        store,
        set,
        selection,
    };
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            if agent { SUPERVISOR } else { A },
            f.configure("configure-a", "0", true),
        )
        .await
        .unwrap();
    f
}

#[tokio::test]
async fn delegated_developer_can_submit_but_cannot_configure_or_continue_after_revocation() {
    let f = setup_sync_with_agent(true).await;
    let reserved = f.reserve("delegated").await;
    let request = f.ingest(&reserved, "delegated-event");
    assert_eq!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request.clone())
            .await
            .unwrap()["data"]["observation_receipt"]["state"],
        "applied"
    );
    let before = f.snapshot().await;
    assert!(matches!(
        f.store
            .configure_connector(
                TENANT,
                PROJECT,
                A,
                f.configure("agent-cannot-configure", "1", true)
            )
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(f.snapshot().await, before);
    f.admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='delivery-grant'",
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store.ingest_facts(TENANT, PROJECT, A, request).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn configured_system_observes_without_acquiring_or_selecting_the_task() {
    let f = setup_sync().await;
    f.admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
        VALUES('reader-tenant','observer','system','Synthetic observer','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','reader-project','observer','developer')",
        )
        .await
        .unwrap();
    f.admin.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,'delivery-observer','observer','cli-observer',$2)",
        &[&TENANT,&workstream_credential_hash(OBSERVER).unwrap()]).await.unwrap();
    f.admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'observer','cli-observer',$3,1,true,true)",&[&TENANT,&PROJECT,&f.set.workstream_id.to_string()]).await.unwrap();
    let mut config = f.configure("configure-observer", "1", true);
    config.mapping.principal_actor_id = "observer".into();
    config.mapping.principal_client_id = "cli-observer".into();
    config.mapping.fact_source = FactSource::AdapterObservation;
    f.store
        .configure_connector(TENANT, PROJECT, A, config)
        .await
        .unwrap();
    let mut r = f.reserve_request("adapter-query");
    r.connector_version = "2".into();
    let reserved = f
        .store
        .reserve_inspection(TENANT, PROJECT, OBSERVER, r)
        .await
        .unwrap()["data"]
        .clone();
    let mut facts = f.ingest(&reserved, "observed");
    if let DeliveryRecord::Verification(v) = &mut facts.records[0].record {
        v.provenance.source = FactSource::AdapterObservation;
    }
    assert_eq!(
        f.store
            .ingest_facts(TENANT, PROJECT, OBSERVER, facts)
            .await
            .unwrap()["data"]["observation_receipt"]["fact_source"],
        "adapter_observation"
    );
    let mut attempt = f.selection.clone();
    attempt.request_id = "observer-selection".into();
    attempt.expected_selected_digest = Some(attempt.candidate.binding.digest().unwrap());
    assert!(matches!(
        f.store
            .select_candidate(TENANT, PROJECT, OBSERVER, attempt)
            .await,
        Err(PgError::ClaimHeld) | Err(PgError::Forbidden)
    ));
    let view = f
        .store
        .inspect(TENANT, PROJECT, OBSERVER, "a")
        .await
        .unwrap();
    assert_eq!(view["acceptance_ready"], false);
    assert_eq!(
        f.admin
            .query_one("SELECT count(*) FROM awr_team.claims", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn selecting_the_same_candidate_again_cannot_resurrect_its_old_inspection() {
    let f = setup_sync().await;
    let old = f.reserve("before-replacement").await;
    let mut second = f.selection.clone();
    second.request_id = "second".into();
    second.expected_selected_digest = Some(second.candidate.binding.digest().unwrap());
    second.candidate.binding.candidate_version = "2".into();
    let second_digest = second.candidate.binding.digest().unwrap();
    f.store
        .select_candidate(TENANT, PROJECT, A, second)
        .await
        .unwrap();
    let mut again = f.selection.clone();
    again.request_id = "again".into();
    again.expected_selected_digest = Some(second_digest);
    f.store
        .select_candidate(TENANT, PROJECT, A, again)
        .await
        .unwrap();
    let old = f
        .store
        .ingest_facts(TENANT, PROJECT, A, f.ingest(&old, "old-response"))
        .await
        .unwrap();
    assert_eq!(old["data"]["observation_receipt"]["state"], "superseded");
    let view = f.store.inspect(TENANT, PROJECT, A, "a").await.unwrap();
    assert_eq!(view["selection_version"], "3");
    assert_eq!(view["facts"], json!([]));
}

#[tokio::test]
async fn changed_epoch_fences_reservations_and_preserves_historical_facts() {
    let mut f = setup_sync().await;
    let old = f.reserve("old-epoch").await;
    let request = f.ingest(&old, "old-fact");
    f.store
        .ingest_facts(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    f.admin.batch_execute("UPDATE awr_team.projects SET coordinator_epoch='next-epoch' WHERE tenant_id='reader-tenant' AND id='reader-project'").await.unwrap();
    assert!(matches!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::EpochChanged)
    ));
    f.set.coordinator_epoch = "next-epoch".into();
    let mut current = request;
    current.request_id = "late-epoch".into();
    current.read_set = f.set.clone();
    assert!(matches!(
        f.store.ingest_facts(TENANT, PROJECT, A, current).await,
        Err(PgError::EpochChanged)
    ));
    let view = f.store.inspect(TENANT, PROJECT, A, "a").await.unwrap();
    assert_eq!(view["facts"][0]["current"], false);
    assert_eq!(view["history"].as_array().unwrap().len(), 1);
}

impl Fixture {
    fn configure(&self, request: &str, version: &str, enabled: bool) -> ConfigureDeliveryConnector {
        ConfigureDeliveryConnector {
            request_id: request.into(),
            read_set: self.set.clone(),
            expected_connector_version: version.into(),
            mapping: DeliveryConnectorMapping {
                connector_id: "neutral".into(),
                provider: "reference".into(),
                resource: RESOURCE.into(),
                principal_actor_id: "agent".into(),
                principal_client_id: "cli-a".into(),
                fact_source: FactSource::CallerDeclared,
                enabled,
            },
        }
    }

    fn reserve_request(&self, id: &str) -> ReserveDeliveryInspection {
        ReserveDeliveryInspection {
            request_id: id.into(),
            read_set: self.set.clone(),
            connector_id: "neutral".into(),
            connector_version: "1".into(),
            candidate_digest: self.selection.candidate.binding.digest().unwrap(),
            lease_seconds: 60,
        }
    }

    async fn reserve(&self, id: &str) -> Value {
        self.store
            .reserve_inspection(TENANT, PROJECT, A, self.reserve_request(id))
            .await
            .unwrap()["data"]
            .clone()
    }

    fn ingest(&self, inspection: &Value, event: &str) -> IngestDeliveryFacts {
        let record = DeliveryEnvelope {
            protocol: DELIVERY_PROTOCOL.into(),
            protocol_version: DELIVERY_PROTOCOL_VERSION,
            record: DeliveryRecord::Verification(VerificationRun {
                binding: self.selection.candidate.binding.clone(),
                run_id: "run-a".into(),
                check: "report".into(),
                outcome: VerificationOutcome::Unknown,
                result_artifact: None,
                provenance: FactProvenance {
                    source: FactSource::CallerDeclared,
                    reference: "fixture://checks/run-a".into(),
                    observed_at_unix_ms: None,
                    recorded_at_unix_ms: 123,
                },
            }),
        };
        IngestDeliveryFacts {
            request_id: format!("ingest-{event}"),
            read_set: self.set.clone(),
            connector_id: "neutral".into(),
            inspection_id: inspection["inspection_id"].as_str().unwrap().into(),
            event_id: event.into(),
            records: vec![record],
        }
    }

    async fn snapshot(&self) -> Value {
        self.admin.query_one("SELECT jsonb_build_object(
            'connectors',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.delivery_connectors x),
            'candidates',(SELECT jsonb_agg(to_jsonb(x) ORDER BY binding_digest) FROM awr_team.delivery_candidates x),
            'selections',(SELECT jsonb_agg(to_jsonb(x) ORDER BY work_id) FROM awr_team.delivery_selections x),
            'inspections',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.delivery_inspections x),
            'inbox',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.delivery_inbox x),
            'facts',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.delivery_facts x),
            'heads',(SELECT jsonb_agg(to_jsonb(x) ORDER BY slot) FROM awr_team.delivery_fact_heads x),
            'notifications',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.delivery_notifications x),
            'requests',(SELECT jsonb_agg(to_jsonb(x) ORDER BY request_id) FROM awr_team.delivery_sync_requests x),
            'events',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.events x),
            'audit',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.ops_audit_records x),
            'project',(SELECT to_jsonb(x) FROM awr_team.projects x WHERE tenant_id=$1 AND id=$2))",
            &[&TENANT,&PROJECT]).await.unwrap().get(0)
    }
}

#[tokio::test]
async fn durable_batch_replay_and_restart_preserve_origin_without_acceptance() {
    let f = setup_sync().await;
    let reserved = f.reserve("poll-one").await;
    let request = f.ingest(&reserved, "event-one");
    let first = f
        .store
        .ingest_facts(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    let before = f.snapshot().await;
    let restarted = DeliverySyncStore::from_config(f.config.clone());
    let replay = restarted
        .ingest_facts(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], first["data"]);
    assert_eq!(f.snapshot().await, before);
    let view = restarted.inspect(TENANT, PROJECT, A, "a").await.unwrap();
    assert_eq!(view["selected_current"], true);
    assert_eq!(view["facts"][0]["current"], true);
    assert!(view["facts"][0]["observation"]["provenance"]["observed_at_unix_ms"].is_null());
    assert_eq!(
        view["facts"][0]["observation"]["provenance"]["source"],
        "caller_declared"
    );
    assert_ne!(
        view["facts"][0]["observation"]["provenance"]["recorded_at_unix_ms"],
        123
    );
    for k in [
        "acceptance_ready",
        "execution_authorized",
        "source_synchronized",
    ] {
        assert_eq!(view[k], false);
    }
    let mut same_event = request;
    same_event.request_id = "retry-new-request".into();
    let replay = restarted
        .ingest_facts(TENANT, PROJECT, A, same_event)
        .await
        .unwrap();
    assert_eq!(replay["data"]["event_replayed"], true);
    let counts=f.admin.query_one("SELECT (SELECT count(*) FROM awr_team.delivery_notifications),(SELECT count(*) FROM awr_team.delivery_facts),
        (SELECT count(*) FROM awr_team.completion_receipts),(SELECT count(*) FROM awr_team.review_rounds)",&[]).await.unwrap();
    assert_eq!(
        (
            counts.get::<_, i64>(0),
            counts.get::<_, i64>(1),
            counts.get::<_, i64>(2),
            counts.get::<_, i64>(3)
        ),
        (1, 1, 0, 0)
    );
}

#[tokio::test]
async fn changed_event_or_request_payload_conflicts_without_partial_writes() {
    let f = setup_sync().await;
    let reserved = f.reserve("inspection").await;
    let request = f.ingest(&reserved, "event");
    f.store
        .ingest_facts(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    for same_request in [true, false] {
        let before = f.snapshot().await;
        let mut changed = request.clone();
        if !same_request {
            changed.request_id = "new-request".into();
        }
        if let DeliveryRecord::Verification(v) = &mut changed.records[0].record {
            v.outcome = VerificationOutcome::Failed;
        }
        assert!(matches!(
            f.store.ingest_facts(TENANT, PROJECT, A, changed).await,
            Err(PgError::IdempotencyConflict)
        ));
        assert_eq!(f.snapshot().await, before);
    }
    let mut duplicate = request;
    duplicate.records.push(duplicate.records[0].clone());
    assert!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, duplicate)
            .await
            .unwrap_err()
            .to_string()
            .contains("bounds")
    );
}

#[tokio::test]
async fn concurrent_reservations_and_events_have_server_order_and_one_notification() {
    let f = setup_sync().await;
    let request = f.reserve_request("same-reservation");
    let (a, b) = tokio::join!(
        f.store
            .reserve_inspection(TENANT, PROJECT, A, request.clone()),
        f.store.reserve_inspection(TENANT, PROJECT, A, request)
    );
    assert_eq!(a.unwrap()["data"], b.unwrap()["data"]);
    let (a, b) = tokio::join!(
        f.store
            .reserve_inspection(TENANT, PROJECT, A, f.reserve_request("next-a")),
        f.store
            .reserve_inspection(TENANT, PROJECT, A, f.reserve_request("next-b"))
    );
    let a = a.unwrap()["data"].clone();
    let b = b.unwrap()["data"].clone();
    assert_ne!(a["generation"], b["generation"]);
    let (new, old) = if a["generation"].as_str().unwrap().parse::<u64>().unwrap()
        > b["generation"].as_str().unwrap().parse::<u64>().unwrap()
    {
        (a, b)
    } else {
        (b, a)
    };
    let first = f.ingest(&new, "new");
    let mut repeat = first.clone();
    repeat.request_id = "other-ingestion".into();
    let (a, b) = tokio::join!(
        f.store.ingest_facts(TENANT, PROJECT, A, first),
        f.store.ingest_facts(TENANT, PROJECT, A, repeat)
    );
    assert_eq!(
        a.unwrap()["data"]["observation_receipt"],
        b.unwrap()["data"]["observation_receipt"]
    );
    let late = f
        .store
        .ingest_facts(TENANT, PROJECT, A, f.ingest(&old, "late"))
        .await
        .unwrap();
    assert_eq!(late["data"]["observation_receipt"]["state"], "superseded");
    let view = f.store.inspect(TENANT, PROJECT, A, "a").await.unwrap();
    assert_eq!(view["facts"].as_array().unwrap().len(), 1);
    assert_eq!(view["facts"][0]["receipt"]["event_id"], "new");
    assert_eq!(view["history"].as_array().unwrap().len(), 2);
    let count: i64 = f
        .admin
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_notifications WHERE state='pending'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn observation_slot_conflicts_in_one_generation_and_rolls_back_entire_batch() {
    let f = setup_sync().await;
    let reserved = f.reserve("one").await;
    f.store
        .ingest_facts(TENANT, PROJECT, A, f.ingest(&reserved, "first"))
        .await
        .unwrap();
    let before = f.snapshot().await;
    let next = f.ingest(&reserved, "second");
    assert!(matches!(
        f.store.ingest_facts(TENANT, PROJECT, A, next).await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
async fn candidate_selection_needs_executor_lease_exact_checks_and_cas() {
    let f = setup_sync().await;
    f.admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'agent','cli-b',$3,1,true,true)",&[&TENANT,&PROJECT,&f.set.workstream_id.to_string()]).await.unwrap();
    let before = f.snapshot().await;
    let mut peer = f.selection.clone();
    peer.request_id = "peer-select".into();
    peer.expected_selected_digest = Some(peer.candidate.binding.digest().unwrap());
    assert!(matches!(
        f.store.select_candidate(TENANT, PROJECT, B, peer).await,
        Err(PgError::ClaimHeld | PgError::Forbidden)
    ));
    assert_eq!(f.snapshot().await, before);
    for case in ["cas", "checks", "immutable", "fence"] {
        let mut r = f.selection.clone();
        r.request_id = case.into();
        r.expected_selected_digest = Some(r.candidate.binding.digest().unwrap());
        match case {
            "cas" => r.expected_selected_digest = None,
            "checks" => r.candidate.binding.required_checks.clear(),
            "immutable" => {
                r.candidate.binding.source_revision.as_mut().unwrap().value = "b".repeat(64)
            }
            "fence" => r.fence = "999".into(),
            _ => unreachable!(),
        }
        assert!(
            f.store
                .select_candidate(TENANT, PROJECT, A, r)
                .await
                .is_err(),
            "{case}"
        );
        assert_eq!(f.snapshot().await, before);
    }
    f.admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let mut expired = f.selection.clone();
    expired.request_id = "expired-select".into();
    expired.expected_selected_digest = Some(expired.candidate.binding.digest().unwrap());
    assert!(matches!(
        f.store.select_candidate(TENANT, PROJECT, A, expired).await,
        Err(PgError::LeaseExpired)
    ));
}

#[tokio::test]
async fn replacement_and_source_contract_changes_keep_old_observations_historical() {
    let mut f = setup_sync().await;
    let old = f.reserve("old").await;
    let request = f.ingest(&old, "old-event");
    let mut next = f.selection.clone();
    next.request_id = "select-v2".into();
    next.expected_selected_digest = Some(next.candidate.binding.digest().unwrap());
    next.candidate.binding.candidate_version = "2".into();
    next.candidate
        .binding
        .source_revision
        .as_mut()
        .unwrap()
        .value = "b".repeat(64);
    f.store
        .select_candidate(TENANT, PROJECT, A, next)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request)
            .await
            .unwrap()["data"]["observation_receipt"]["state"],
        "superseded"
    );
    assert_eq!(
        f.store.inspect(TENANT, PROJECT, A, "a").await.unwrap()["facts"],
        json!([])
    );
    // Owner-controlled fault fixture: install an internally consistent new contract,
    // leaving the old reservation untouched. Never mutate real source snapshots.
    let row = f
        .admin
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='a'",
            &[],
        )
        .await
        .unwrap();
    let mut contract: awr_team::WorkContract = serde_json::from_value(row.get(0)).unwrap();
    contract.goals.push("additional scoped requirement".into());
    let hash = contract.hash().unwrap();
    f.admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='a'",&[&json!(contract),&hash]).await.unwrap();
    f.set.contract_hash = hash;
    let mut r = f.ingest(&old, "contract-late");
    r.read_set = f.set.clone();
    assert_eq!(
        f.store.ingest_facts(TENANT, PROJECT, A, r).await.unwrap()["data"]["observation_receipt"]["state"],
        "superseded"
    );
    assert_eq!(
        f.store.inspect(TENANT, PROJECT, A, "a").await.unwrap()["selected_current"],
        false
    );
}

#[tokio::test]
async fn revoked_connector_credentials_and_authority_cannot_reuse_inspection() {
    let f = setup_sync().await;
    let reserved = f.reserve("one").await;
    let request = f.ingest(&reserved, "event");
    f.store
        .configure_connector(TENANT, PROJECT, A, f.configure("revoke", "1", false))
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    f.store
        .configure_connector(TENANT, PROJECT, A, f.configure("enable", "2", true))
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let mut r = f.reserve_request("fresh");
    r.connector_version = "3".into();
    let fresh = f
        .store
        .reserve_inspection(TENANT, PROJECT, A, r)
        .await
        .unwrap()["data"]
        .clone();
    let request = f.ingest(&fresh, "fresh-event");
    f.admin.batch_execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    assert!(matches!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    f.admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store.ingest_facts(TENANT, PROJECT, A, request).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(
        f.admin
            .query_one("SELECT count(*) FROM awr_team.delivery_inbox", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn expired_inspection_is_history_and_event_time_cannot_order_snapshots() {
    let f = setup_sync().await;
    let reserved = f.reserve("expired").await;
    f.admin.batch_execute("UPDATE awr_team.delivery_inspections SET expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
    let mut r = f.ingest(&reserved, "late");
    if let DeliveryRecord::Verification(v) = &mut r.records[0].record {
        v.provenance.observed_at_unix_ms = Some(9_000_000_000_000_000);
    }
    let late = f.store.ingest_facts(TENANT, PROJECT, A, r).await.unwrap();
    assert_eq!(late["data"]["observation_receipt"]["state"], "superseded");
    let view = f.store.inspect(TENANT, PROJECT, A, "a").await.unwrap();
    assert_eq!(view["facts"], json!([]));
    assert_eq!(view["history"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn notification_failure_rolls_back_facts_heads_receipts_and_audit_then_recovers() {
    let f = setup_sync().await;
    let reserved = f.reserve("one").await;
    let request = f.ingest(&reserved, "event");
    f.admin.batch_execute("CREATE FUNCTION awr_team.delivery_test_abort() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic notification failure'; END $$;
        CREATE TRIGGER delivery_test_abort BEFORE INSERT ON awr_team.delivery_notifications FOR EACH ROW EXECUTE FUNCTION awr_team.delivery_test_abort()").await.unwrap();
    let before = f.snapshot().await;
    assert!(matches!(
        f.store
            .ingest_facts(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(f.snapshot().await, before);
    f.admin.batch_execute("DROP TRIGGER delivery_test_abort ON awr_team.delivery_notifications; DROP FUNCTION awr_team.delivery_test_abort()").await.unwrap();
    let restarted = DeliverySyncStore::from_config(f.config.clone());
    assert_eq!(
        restarted
            .ingest_facts(TENANT, PROJECT, A, request)
            .await
            .unwrap()["data"]["observation_receipt"]["state"],
        "applied"
    );
}

#[tokio::test]
async fn denied_scope_management_resource_provenance_and_large_payloads_leave_no_writes() {
    let f = setup_sync().await;
    let before = f.snapshot().await;
    assert!(matches!(
        f.store
            .configure_connector(TENANT, PROJECT, B, f.configure("no-manage", "1", true))
            .await,
        Err(PgError::Forbidden) | Err(PgError::Workstream(_))
    ));
    assert!(matches!(
        f.store.inspect("other-tenant", PROJECT, A, "a").await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        f.store.inspect(TENANT, PROJECT, NONE, "a").await,
        Err(PgError::Forbidden)
    ));
    let mut wrong = f.reserve_request("wrong-resource");
    wrong.candidate_digest = "f".repeat(64);
    assert!(matches!(
        f.store.reserve_inspection(TENANT, PROJECT, A, wrong).await,
        Err(PgError::PreconditionsChanged)
    ));
    let mut spoof = f.configure("spoof-adapter", "1", true);
    spoof.mapping.fact_source = FactSource::AdapterObservation;
    assert!(matches!(
        f.store.configure_connector(TENANT, PROJECT, A, spoof).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(f.snapshot().await, before);
    let reserved = f.reserve("valid").await;
    let before = f.snapshot().await;
    for case in ["source", "binding", "oversize"] {
        let mut r = f.ingest(&reserved, case);
        if let DeliveryRecord::Verification(v) = &mut r.records[0].record {
            match case {
                "source" => v.provenance.source = FactSource::AdapterObservation,
                "binding" => v.binding.target.reference = Some("changed-target".into()),
                "oversize" => v.provenance.reference = "x".repeat(66000),
                _ => unreachable!(),
            }
        }
        assert!(
            f.store.ingest_facts(TENANT, PROJECT, A, r).await.is_err(),
            "{case}"
        );
        assert_eq!(f.snapshot().await, before);
    }
    let app = common::app_client(f.config.get_dbname().unwrap()).await;
    app.batch_execute("SELECT set_config('awr.tenant_id','other-tenant',false); SELECT set_config('awr.project_id','reader-project',false)").await.unwrap();
    assert_eq!(
        app.query_one("SELECT count(*) FROM awr_team.delivery_candidates", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let error=app.execute("INSERT INTO awr_team.delivery_candidates(tenant_id,project_id,binding_digest,work_id,candidate_id,candidate_version,body_json)
        VALUES($1,$2,$3,'a','forbidden','1','{}')",&[&TENANT,&PROJECT,&"d".repeat(64)]).await.unwrap_err();
    assert_eq!(
        *error.as_db_error().unwrap().code(),
        tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE
    );
}
