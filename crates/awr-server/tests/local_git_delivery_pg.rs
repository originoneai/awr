#![cfg(feature = "pg-tests")]
//! Local repository/PG mechanism conformance; no native client or human approval credit.
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod access;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "fixtures/local_git.rs"]
mod git;

use access::*;
use awr_server::delivery_adapter::{LocalGitAdapter, LocalGitError, LocalGitPollRequest};
use awr_team::delivery::FactSource;
use awr_team_pg::*;
use serde_json::{Value, json};

const OBSERVER: &str =
    "awr1.local-observer.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

fn read_set(p: &Value) -> DeliveryReadSet {
    serde_json::from_value(json!({"work_id":"a","workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],
        "authority_version":p["authority_version"],"ownership_version":p["data"]["ownership_version"],
        "contract_hash":p["data"]["contract_hash"]})).unwrap()
}

struct Fixture {
    _guard: std::sync::MutexGuard<'static, ()>,
    admin: tokio_postgres::Client,
    store: DeliverySyncStore,
    database: tokio_postgres::Config,
    git: git::GitFixture,
    adapter: LocalGitAdapter,
    candidate: awr_team::delivery::DeliveryCandidate,
    set: DeliveryReadSet,
}

async fn setup_local() -> Fixture {
    let (guard, admin, db, reads) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
        VALUES('reader-tenant','local-observer','system','Synthetic Git observer','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','reader-project','local-observer','developer')",
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
        VALUES($1,'local-observer','local-observer','cli-local-observer',$2)",
            &[&TENANT, &workstream_credential_hash(OBSERVER).unwrap()],
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'local-observer','cli-local-observer',$3,1,true,true)", &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let p = prepare(&reads, A, "a").await;
    let claim = reads.commands().execute(TENANT, PROJECT, A, command(&p, "git-claim", "task.claim_available", json!({
        "session_id":"session-a","expected_session_version":"1","expected_work_version":"0",
        "expected_responsibility_version":p["data"]["responsibility"]["version"],"ttl_seconds":3600
    }))).await.unwrap()["receipt"]["data"].clone();
    let p = prepare(&reads, A, "a").await;
    let set = read_set(&p);
    let mut git = git::GitFixture::new(false);
    git.config.tenant_id = TENANT.into();
    git.config.project_id = PROJECT.into();
    git.config.workstream_id = set.workstream_id.to_string();
    let mut candidate = git.candidate();
    candidate.binding.contract_hash = set.contract_hash.clone();
    // The source fixture's required checks remain authoritative. A manifest
    // observation must not silently substitute for its required report check.
    candidate.binding.required_checks = serde_json::from_value(admin.query_one(
        "SELECT contract_json->'verification_requirements' FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id='a'",
        &[&TENANT, &PROJECT, &set.source_snapshot_id],
    ).await.unwrap().get(0)).unwrap();
    let database = common::with_app_role(&common::test_config(), &db);
    let store = DeliverySyncStore::from_config(database.clone());
    store
        .select_candidate(
            TENANT,
            PROJECT,
            A,
            SelectDeliveryCandidate {
                request_id: "local-select".into(),
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
    store
        .configure_connector(
            TENANT,
            PROJECT,
            A,
            ConfigureDeliveryConnector {
                request_id: "local-configure".into(),
                read_set: set.clone(),
                expected_connector_version: "0".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: git.config.connector_id.clone(),
                    provider: "local_git".into(),
                    resource: git.config.resource.clone(),
                    principal_actor_id: "local-observer".into(),
                    principal_client_id: "cli-local-observer".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let observer = prepare(&reads, OBSERVER, "a").await;
    let set = read_set(&observer);
    let adapter = git.adapter().await;
    Fixture {
        _guard: guard,
        admin,
        store,
        database,
        git,
        adapter,
        candidate,
        set,
    }
}

impl Fixture {
    fn poll(&self, key: &str) -> LocalGitPollRequest {
        LocalGitPollRequest {
            request_id: key.into(),
            read_set: self.set.clone(),
            connector_version: "1".into(),
        }
    }
    async fn reserve(&self, key: &str) -> Value {
        self.store
            .reserve_inspection(
                TENANT,
                PROJECT,
                OBSERVER,
                ReserveDeliveryInspection {
                    request_id: key.into(),
                    read_set: self.set.clone(),
                    connector_id: self.git.config.connector_id.clone(),
                    connector_version: "1".into(),
                    candidate_digest: self.candidate.binding.digest().unwrap(),
                    lease_seconds: 120,
                },
            )
            .await
            .unwrap()["data"]
            .clone()
    }
}

#[tokio::test]
async fn local_query_and_real_push_use_same_durable_neutral_facts_and_restart_receipts() {
    let f = setup_local().await;
    let first = f
        .adapter
        .reconcile(&f.store, OBSERVER, f.poll("local-first"))
        .await
        .unwrap();
    assert_eq!(
        first["data"]["observation_receipt"]["fact_source"],
        "adapter_observation"
    );
    assert_eq!(first["data"]["observation_receipt"]["state"], "applied");
    let store = DeliverySyncStore::from_config(f.database.clone());
    let restarted = f.git.adapter().await;
    let recovered = restarted
        .reconcile(&store, OBSERVER, f.poll("local-first"))
        .await
        .unwrap();
    assert_eq!(recovered["replayed"], true);
    assert_eq!(first["receipt"], recovered["receipt"]);
    f.git.push_main(); // Fixture-owned push; adapter capability is still read-only.
    f.adapter
        .reconcile(&store, OBSERVER, f.poll("local-after-push"))
        .await
        .unwrap();
    let view = store.inspect(TENANT, PROJECT, OBSERVER, "a").await.unwrap();
    let facts = view["facts"].as_array().unwrap();
    assert_eq!(facts.len(), 2);
    assert!(facts.iter().all(|fact| fact["current"] == true));
    let integration = facts
        .iter()
        .find(|fact| fact["observation"]["kind"] == "integration_observation")
        .unwrap();
    assert_eq!(integration["observation"]["outcome"], "applied");
    assert_eq!(
        integration["observation"]["result_revision"]["value"],
        f.git.source
    );
    assert_eq!(view["history"].as_array().unwrap().len(), 2);
    for key in [
        "acceptance_ready",
        "execution_authorized",
        "source_synchronized",
    ] {
        assert_eq!(view[key], false);
    }
    let counts = f
        .admin
        .query_one(
            "SELECT (SELECT count(*) FROM awr_team.delivery_inbox),
        (SELECT count(*) FROM awr_team.delivery_fact_heads),(SELECT count(*) FROM awr_team.claims),
        (SELECT count(*) FROM awr_team.delivery_sync_intents)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(counts.get::<_, i64>(0), 2);
    assert_eq!(counts.get::<_, i64>(1), 2);
    assert_eq!(counts.get::<_, i64>(2), 1);
    assert_eq!(counts.get::<_, i64>(3), 4); // One refresh and one source intent per observation.
}

#[tokio::test]
async fn provenance_forgery_and_revoked_worker_cannot_create_adapter_facts() {
    let f = setup_local().await;
    let mut set = f.set.clone();
    set.authority_version = "1".into();
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            A,
            ConfigureDeliveryConnector {
                request_id: "caller-configure".into(),
                read_set: set.clone(),
                expected_connector_version: "0".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "caller".into(),
                    provider: "local_git".into(),
                    resource: f.git.config.resource.clone(),
                    principal_actor_id: "agent".into(),
                    principal_client_id: "cli-a".into(),
                    fact_source: FactSource::CallerDeclared,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let reserved = f
        .store
        .reserve_inspection(
            TENANT,
            PROJECT,
            A,
            ReserveDeliveryInspection {
                request_id: "caller-reserve".into(),
                read_set: set.clone(),
                connector_id: "caller".into(),
                connector_version: "1".into(),
                candidate_digest: f.candidate.binding.digest().unwrap(),
                lease_seconds: 120,
            },
        )
        .await
        .unwrap();
    let inspection = reserved["data"]["inspection_id"].as_str().unwrap();
    let snapshot = f.adapter.inspect(&f.candidate, inspection).await.unwrap();
    assert!(matches!(
        f.store
            .ingest_facts(
                TENANT,
                PROJECT,
                A,
                IngestDeliveryFacts {
                    request_id: "forged-observation".into(),
                    read_set: set,
                    connector_id: "caller".into(),
                    inspection_id: inspection.into(),
                    event_id: "caller-forged-event".into(),
                    records: snapshot.records,
                }
            )
            .await,
        Err(PgError::Forbidden)
    ));
    f.admin
        .batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='local-observer'")
        .await
        .unwrap();
    let before = std::fs::read_dir(&f.git.config.report_directory)
        .unwrap()
        .count();
    assert!(matches!(
        f.adapter
            .reconcile(&f.store, OBSERVER, f.poll("revoked"))
            .await,
        Err(LocalGitError::AuthorizationUnavailable)
    ));
    assert_eq!(
        std::fs::read_dir(&f.git.config.report_directory)
            .unwrap()
            .count(),
        before
    );
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
async fn old_repository_query_is_superseded_by_new_inspection_generation() {
    let f = setup_local().await;
    let old = f.reserve("older-query").await;
    let old_id = old["inspection_id"].as_str().unwrap();
    let old_report = f.adapter.inspect(&f.candidate, old_id).await.unwrap();
    f.git.push_main();
    f.adapter
        .reconcile(&f.store, OBSERVER, f.poll("new-current-query"))
        .await
        .unwrap();
    let late = f
        .store
        .ingest_facts(
            TENANT,
            PROJECT,
            OBSERVER,
            IngestDeliveryFacts {
                request_id: "late-old-query".into(),
                read_set: f.set.clone(),
                connector_id: f.git.config.connector_id.clone(),
                inspection_id: old_id.into(),
                event_id: "late-old-event".into(),
                records: old_report.records,
            },
        )
        .await
        .unwrap();
    assert_eq!(late["data"]["observation_receipt"]["state"], "superseded");
    let view = f
        .store
        .inspect(TENANT, PROJECT, OBSERVER, "a")
        .await
        .unwrap();
    assert_eq!(
        view["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["observation"]["kind"] == "integration_observation")
            .unwrap()["observation"]["outcome"],
        "applied"
    );
    assert_eq!(view["acceptance_ready"], false);
}
