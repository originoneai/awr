#![cfg(feature = "pg-tests")]
//! Authenticated PG mechanism tests, not native-client business acceptance.
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod access;
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "fixtures/github.rs"]
mod github;

use access::*;
use awr_server::delivery_adapter::{GitHubAdapter, GitHubError, GitHubPollRequest};
use awr_team::delivery::{DeliveryRecord, FactSource};
use awr_team_pg::*;
use serde_json::{Value, json};

const OBSERVER: &str =
    "awr1.github-observer.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
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
    github: github::Fixture,
    adapter: GitHubAdapter,
    select: SelectDeliveryCandidate,
    set: DeliveryReadSet,
}

// The shared PostgreSQL fixture lock deliberately spans async setup operations.
#[allow(clippy::await_holding_lock)]
async fn setup_github() -> Fixture {
    let (guard, admin, db, reads) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
        VALUES('reader-tenant','github-observer','system','Synthetic GitHub observer','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','reader-project','github-observer','developer')",
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
        VALUES($1,'github-observer','github-observer','cli-github-observer',$2)",
            &[&TENANT, &workstream_credential_hash(OBSERVER).unwrap()],
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'github-observer','cli-github-observer',$3,1,true,true)",
        &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let p = prepare(&reads, A, "a").await;
    let claim=reads.commands().execute(TENANT,PROJECT,A,command(&p,"github-claim","task.claim_available",json!({
        "session_id":"session-a","expected_session_version":"1","expected_work_version":"0",
        "expected_responsibility_version":p["data"]["responsibility"]["version"],"ttl_seconds":3600
    }))).await.unwrap()["receipt"]["data"].clone();
    let p = prepare(&reads, A, "a").await;
    let set = read_set(&p);
    let mut github = github::Fixture::new();
    github.config.tenant_id = TENANT.into();
    github.config.project_id = PROJECT.into();
    github.config.workstream_id = set.workstream_id.to_string();
    let mut candidate = github.candidate();
    candidate.binding.contract_hash = set.contract_hash.clone();
    candidate.binding.required_checks=serde_json::from_value(admin.query_one(
        "SELECT contract_json->'verification_requirements' FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id='a'",
        &[&TENANT,&PROJECT,&set.source_snapshot_id]).await.unwrap().get(0)).unwrap();
    let database = common::with_app_role(&common::test_config(), &db);
    let store = DeliverySyncStore::from_config(database.clone());
    let select = SelectDeliveryCandidate {
        request_id: "github-select".into(),
        read_set: set.clone(),
        expected_selected_digest: None,
        session_id: "session-a".into(),
        claim_id: claim["claim_id"].as_str().unwrap().into(),
        fence: claim["fence"].as_str().unwrap().into(),
        lease_version: claim["lease_version"].as_str().unwrap().into(),
        candidate,
    };
    store
        .select_candidate(TENANT, PROJECT, A, select.clone())
        .await
        .unwrap();
    store
        .configure_connector(
            TENANT,
            PROJECT,
            A,
            ConfigureDeliveryConnector {
                request_id: "github-configure".into(),
                read_set: set.clone(),
                expected_connector_version: "0".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: github.config.connector_id.clone(),
                    provider: "github".into(),
                    resource: github.config.resource.clone(),
                    principal_actor_id: "github-observer".into(),
                    principal_client_id: "cli-github-observer".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let set = read_set(&prepare(&reads, OBSERVER, "a").await);
    let adapter = github.adapter();
    Fixture {
        _guard: guard,
        admin,
        store,
        database,
        github,
        adapter,
        select,
        set,
    }
}

impl Fixture {
    fn request(&self, key: &str) -> GitHubPollRequest {
        GitHubPollRequest {
            request_id: key.into(),
            read_set: self.set.clone(),
            connector_version: "1".into(),
        }
    }

    async fn view(&self) -> Value {
        self.store
            .inspect(TENANT, PROJECT, OBSERVER, "a")
            .await
            .unwrap()
    }

    async fn durable_counts(&self) -> Value {
        self.admin
            .query_one(
                "SELECT jsonb_build_object(
            'inspections',(SELECT count(*) FROM awr_team.delivery_inspections),
            'inbox',(SELECT count(*) FROM awr_team.delivery_inbox),
            'facts',(SELECT count(*) FROM awr_team.delivery_facts),
            'notifications',(SELECT count(*) FROM awr_team.delivery_notifications),
            'completions',(SELECT count(*) FROM awr_team.completion_receipts))",
                &[],
            )
            .await
            .unwrap()
            .get(0)
    }
}

fn slot(fact: &Value) -> String {
    let observation = &fact["observation"];
    match observation["kind"].as_str().unwrap() {
        "verification" => format!("verification:{}", observation["check"].as_str().unwrap()),
        kind => kind.into(),
    }
}

fn assert_only_changed(before: &Value, after: &Value, changed: &str) {
    assert_eq!(before["facts"].as_array().unwrap().len(), 4);
    assert_eq!(after["facts"].as_array().unwrap().len(), 4);
    for previous in before["facts"].as_array().unwrap() {
        let current = after["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| slot(f) == slot(previous))
            .unwrap();
        assert_eq!(current["current"], true);
        if slot(previous) == changed {
            assert_ne!(previous["fact_id"], current["fact_id"]);
        } else {
            assert_eq!(
                previous,
                current,
                "unrelated slot changed: {}",
                slot(previous)
            );
        }
    }
}

#[tokio::test]
async fn current_poll_queries_freshly_without_churning_facts_after_restart() {
    let f = setup_github().await;
    let first = f
        .adapter
        .reconcile_current(&f.store, OBSERVER)
        .await
        .unwrap();
    assert_eq!(first["unchanged"], false);
    assert_eq!(first["changed_slots"].as_array().unwrap().len(), 4);
    let view = f.view().await;
    let counts = f.durable_counts().await;
    for _ in 0..3 {
        let calls = f.github.api.calls();
        let store = DeliverySyncStore::from_config(f.database.clone());
        let result = f
            .github
            .adapter()
            .reconcile_current(&store, OBSERVER)
            .await
            .unwrap();
        assert_eq!(result["unchanged"], true);
        assert_eq!(result["read_only"], true);
        assert_eq!(result["observation"], Value::Null);
        assert_eq!(result["acceptance_ready"], false);
        assert_eq!(result["execution_authorized"], false);
        assert_eq!(result["source_synchronized"], false);
        assert!(
            f.github.api.calls() > calls,
            "cached authority must not replace a fresh query"
        );
        assert_eq!(f.view().await["facts"], view["facts"]);
        assert_eq!(f.durable_counts().await, counts);
    }
}

#[tokio::test]
async fn concurrent_current_polls_converge_on_one_durable_observation() {
    let f = setup_github().await;
    let (a, b) = tokio::join!(
        f.adapter.reconcile_current(&f.store, OBSERVER),
        f.adapter.reconcile_current(&f.store, OBSERVER)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    if a["unchanged"] == false && b["unchanged"] == false {
        assert_eq!(a["observation"]["receipt"], b["observation"]["receipt"]);
    }
    let counts = f.durable_counts().await;
    assert_eq!(counts["inspections"], 1);
    assert_eq!(counts["inbox"], 1);
    assert_eq!(counts["facts"], 4);
    assert_eq!(counts["completions"], 0);
}

#[tokio::test]
async fn current_poll_changes_target_pr_and_check_slots_independently() {
    let f = setup_github().await;
    f.adapter
        .reconcile_current(&f.store, OBSERVER)
        .await
        .unwrap();
    let before = f.view().await;
    f.github
        .api
        .set("/git/ref/heads/main", github::reference(github::SOURCE));
    let result = f
        .adapter
        .reconcile_current(&f.store, OBSERVER)
        .await
        .unwrap();
    assert_eq!(result["changed_slots"], json!(["integration_observation"]));
    let target = f.view().await;
    assert_only_changed(&before, &target, "integration_observation");

    let mut pull = github::pull();
    pull["state"] = "closed".into();
    pull["merged"] = true.into();
    f.github.api.set("/pulls/11", pull);
    let result = f
        .adapter
        .reconcile_current(&f.store, OBSERVER)
        .await
        .unwrap();
    assert_eq!(result["changed_slots"], json!(["change_request"]));
    let pr = f.view().await;
    assert_only_changed(&target, &pr, "change_request");

    let mut run = github::run();
    run["id"] = 2.into();
    f.github.api.set(
        &format!(
            "/commits/{}/check-runs?filter=latest&per_page=100&page=1",
            github::SOURCE
        ),
        json!({"total_count":1,"check_runs":[run.clone()]}),
    );
    f.github.api.set("/check-runs/2", run);
    let result = f
        .adapter
        .reconcile_current(&f.store, OBSERVER)
        .await
        .unwrap();
    let check = format!("verification:{}", f.github.config.checks[0].check);
    assert_eq!(result["changed_slots"], json!([check]));
    assert_only_changed(&pr, &f.view().await, &check);
    assert_eq!(f.durable_counts().await["facts"], 7);
    assert_eq!(f.durable_counts().await["completions"], 0);
}

#[tokio::test]
async fn missing_or_corrupt_report_and_index_cannot_prove_unchanged_facts() {
    for damage in [
        "missing-report",
        "corrupt-report",
        "missing-index",
        "wrong-index",
    ] {
        let f = setup_github().await;
        let first = f
            .adapter
            .reconcile_current(&f.store, OBSERVER)
            .await
            .unwrap();
        let before = f.view().await;
        let receipt = &first["observation"]["data"]["observation_receipt"];
        let inspection = receipt["inspection_id"].as_str().unwrap();
        let hash = before["facts"][0]["observation"]["provenance"]["reference"]
            .as_str()
            .unwrap()
            .rsplit(':')
            .next()
            .unwrap();
        let report = f.github.root.join(format!("github-report-{hash}.json"));
        let key =
            github::hash(&serde_json::to_vec(&(&f.github.config.adapter_id, inspection)).unwrap());
        let index = f.github.root.join(format!("github-inspection-{key}.json"));
        match damage {
            "missing-report" => std::fs::remove_file(&report).unwrap(),
            "corrupt-report" => std::fs::write(&report, b"synthetic corrupt proof").unwrap(),
            "missing-index" => std::fs::remove_file(&index).unwrap(),
            "wrong-index" => {
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
                value["binding_digest"] = "f".repeat(64).into();
                std::fs::write(&index, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        let damaged = std::fs::read(if damage.contains("report") {
            &report
        } else {
            &index
        })
        .ok();
        let result = f
            .adapter
            .reconcile_current(&f.store, OBSERVER)
            .await
            .unwrap();
        assert_eq!(
            result["unchanged"], false,
            "damage incorrectly accepted: {damage}"
        );
        assert_eq!(result["changed_slots"].as_array().unwrap().len(), 4);
        let after = f.view().await;
        for previous in before["facts"].as_array().unwrap() {
            assert!(
                after["facts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|f| f["fact_id"] != previous["fact_id"])
            );
        }
        assert_eq!(
            std::fs::read(if damage.contains("report") {
                &report
            } else {
                &index
            })
            .ok(),
            damaged,
            "immutable damaged proof must not be overwritten"
        );
    }
}

#[tokio::test]
async fn inconsistent_exposed_fact_summary_requires_fresh_slot_proof() {
    for (path, value) in [
        (vec!["record", "data", "run_id"], json!("inconsistent-run")),
        (vec!["record", "data", "outcome"], json!("failed")),
        (
            vec!["record", "data", "provenance", "observed_at_unix_ms"],
            json!(1),
        ),
    ] {
        let f = setup_github().await;
        f.adapter
            .reconcile_current(&f.store, OBSERVER)
            .await
            .unwrap();
        let before = f.view().await;
        let check = format!("verification:{}", f.github.config.checks[0].check);
        let fact = before["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| slot(v) == check)
            .unwrap();
        f.admin
            .execute(
                "UPDATE awr_team.delivery_facts SET envelope_json=jsonb_set(envelope_json,$1,$2)
            WHERE id=$3",
                &[&path, &value, &fact["fact_id"].as_str().unwrap()],
            )
            .await
            .unwrap();
        let result = f
            .adapter
            .reconcile_current(&f.store, OBSERVER)
            .await
            .unwrap();
        assert_eq!(result["changed_slots"], json!([check]));
        assert_only_changed(&before, &f.view().await, &check);
    }
}

#[tokio::test]
async fn current_poll_rechecks_revocation_during_a_stable_provider_query() {
    let f = setup_github().await;
    f.adapter
        .reconcile_current(&f.store, OBSERVER)
        .await
        .unwrap();
    let counts = f.durable_counts().await;
    let calls = f.github.api.calls();
    f.github.api.0.lock().unwrap().delay_ms = 20;
    let revoke = async {
        while f.github.api.calls() == calls {
            tokio::task::yield_now().await;
        }
        f.admin.execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='github-observer'", &[]).await.unwrap();
    };
    let (result, ()) = tokio::join!(f.adapter.reconcile_current(&f.store, OBSERVER), revoke);
    assert_eq!(result.unwrap_err(), GitHubError::AuthorizationUnavailable);
    assert_eq!(f.durable_counts().await, counts);
    let calls = f.github.api.calls();
    assert_eq!(
        f.adapter
            .reconcile_current(&f.store, OBSERVER)
            .await
            .unwrap_err(),
        GitHubError::AuthorizationUnavailable
    );
    assert_eq!(f.github.api.calls(), calls);
}

#[tokio::test]
async fn current_poll_cannot_reuse_disabled_wrong_or_replaced_binding() {
    for change in ["disabled", "wrong-resource", "candidate", "source"] {
        let f = setup_github().await;
        f.adapter
            .reconcile_current(&f.store, OBSERVER)
            .await
            .unwrap();
        let calls = f.github.api.calls();
        match change {
            "disabled" => {
                f.admin
                    .batch_execute("UPDATE awr_team.delivery_connectors SET enabled=false")
                    .await
                    .unwrap();
            }
            "wrong-resource" => {
                f.admin
                    .batch_execute(
                        "UPDATE awr_team.delivery_connectors SET resource='other:repository'",
                    )
                    .await
                    .unwrap();
            }
            "source" => {
                f.admin.batch_execute("UPDATE awr_team.delivery_selections SET source_snapshot_id='obsolete-source'").await.unwrap();
            }
            "candidate" => {
                let mut selection = f.select.clone();
                selection.request_id = "new-current-candidate".into();
                selection.expected_selected_digest =
                    Some(selection.candidate.binding.digest().unwrap());
                selection.candidate.binding.candidate_version = "2".into();
                f.store
                    .select_candidate(TENANT, PROJECT, A, selection)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let counts = f.durable_counts().await;
        if change == "candidate" {
            let result = f
                .adapter
                .reconcile_current(&f.store, OBSERVER)
                .await
                .unwrap();
            assert_eq!(result["unchanged"], false);
            assert_eq!(result["changed_slots"].as_array().unwrap().len(), 4);
            assert_eq!(
                f.view().await["candidate"]["binding"]["candidate_version"],
                "2"
            );
        } else {
            assert!(
                f.adapter
                    .reconcile_current(&f.store, OBSERVER)
                    .await
                    .is_err()
            );
            assert_eq!(f.github.api.calls(), calls);
            assert_eq!(f.durable_counts().await, counts);
        }
    }
}

#[tokio::test]
async fn truncated_inventory_with_absent_expected_slots_refuses_before_query() {
    let f = setup_github().await;
    f.adapter
        .reconcile_current(&f.store, OBSERVER)
        .await
        .unwrap();
    f.store
        .configure_connector(
            TENANT,
            PROJECT,
            A,
            ConfigureDeliveryConnector {
                request_id: "extra-connector".into(),
                read_set: f.set.clone(),
                expected_connector_version: "0".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "aaa-extra".into(),
                    provider: "github".into(),
                    resource: f.github.config.resource.clone(),
                    principal_actor_id: "github-observer".into(),
                    principal_client_id: "cli-github-observer".into(),
                    fact_source: FactSource::AdapterObservation,
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
            OBSERVER,
            ReserveDeliveryInspection {
                request_id: "extra-reserve".into(),
                read_set: f.set.clone(),
                connector_id: "aaa-extra".into(),
                connector_version: "1".into(),
                candidate_digest: f.select.candidate.binding.digest().unwrap(),
                lease_seconds: 120,
            },
        )
        .await
        .unwrap();
    let inspection = reserved["data"]["inspection_id"].as_str().unwrap();
    let snapshot = f
        .adapter
        .inspect(&f.select.candidate, inspection)
        .await
        .unwrap();
    let records = (0..32)
        .map(|i| {
            let mut record = snapshot.records[0].clone();
            let DeliveryRecord::Verification(run) = &mut record.record else {
                unreachable!()
            };
            run.check = format!("extra-{i}");
            record
        })
        .collect();
    f.store
        .ingest_facts(
            TENANT,
            PROJECT,
            OBSERVER,
            IngestDeliveryFacts {
                request_id: "extra-ingest".into(),
                read_set: f.set.clone(),
                connector_id: "aaa-extra".into(),
                inspection_id: inspection.into(),
                event_id: "extra-event".into(),
                records,
            },
        )
        .await
        .unwrap();
    assert_eq!(f.view().await["facts_truncated"], true);
    let counts = f.durable_counts().await;
    let calls = f.github.api.calls();
    assert_eq!(
        f.adapter
            .reconcile_current(&f.store, OBSERVER)
            .await
            .unwrap_err(),
        GitHubError::InvalidResponse
    );
    assert_eq!(f.github.api.calls(), calls);
    assert_eq!(f.durable_counts().await, counts);
}

#[tokio::test]
async fn failed_reserved_query_renews_only_a_proven_expired_observation() {
    let f = setup_github().await;
    let repo = json!({"id":7,"full_name":"acme/demo","archived":false,"permissions":{"push":true}});
    f.github
        .api
        .sequence("", vec![repo.clone(), repo.clone(), json!({"id":8})]);
    assert_eq!(
        f.adapter
            .reconcile_current(&f.store, OBSERVER)
            .await
            .unwrap_err(),
        GitHubError::BindingMismatch
    );
    let counts = f.durable_counts().await;
    assert_eq!(counts["inspections"], 1);
    assert_eq!(counts["facts"], 0);
    f.admin.batch_execute("UPDATE awr_team.delivery_inspections SET expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
    f.github.api.set("", repo);
    let result = f
        .github
        .adapter()
        .reconcile_current(&f.store, OBSERVER)
        .await
        .unwrap();
    assert_eq!(result["unchanged"], false);
    let counts = f.durable_counts().await;
    assert_eq!(counts["inspections"], 2);
    assert_eq!(counts["inbox"], 1);
    assert_eq!(counts["facts"], 4);
    assert_eq!(counts["completions"], 0);
}

#[tokio::test]
async fn mapped_query_ingests_inspectable_facts_and_recovers_original_receipts_after_restart() {
    let f = setup_github().await;
    let first = f
        .adapter
        .reconcile(&f.store, OBSERVER, f.request("first"))
        .await
        .unwrap();
    assert_eq!(
        first["data"]["observation_receipt"]["fact_source"],
        "adapter_observation"
    );
    assert_eq!(first["data"]["observation_receipt"]["state"], "applied");
    let calls = f.github.api.calls();
    let store = DeliverySyncStore::from_config(f.database.clone());
    let replay = f
        .github
        .adapter()
        .reconcile(&store, OBSERVER, f.request("first"))
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(first["receipt"], replay["receipt"]);
    assert_eq!(calls, f.github.api.calls());
    let view = store.inspect(TENANT, PROJECT, OBSERVER, "a").await.unwrap();
    assert_eq!(view["facts"].as_array().unwrap().len(), 4);
    assert_eq!(view["acceptance_ready"], false);
    assert!(
        !view["facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["observation"]["kind"] == "review_decision")
    );
}

#[tokio::test]
async fn concurrent_same_request_has_one_receipt_and_no_duplicate_facts() {
    let f = setup_github().await;
    let (a, b) = tokio::join!(
        f.adapter
            .reconcile(&f.store, OBSERVER, f.request("concurrent")),
        f.adapter
            .reconcile(&f.store, OBSERVER, f.request("concurrent"))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a["receipt"], b["receipt"]);
    assert_eq!(
        f.store
            .inspect(TENANT, PROJECT, OBSERVER, "a")
            .await
            .unwrap()["facts"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[tokio::test]
async fn revoked_identity_and_old_connector_version_cannot_use_cached_proof() {
    let f = setup_github().await;
    f.adapter
        .reconcile(&f.store, OBSERVER, f.request("original"))
        .await
        .unwrap();
    let calls = f.github.api.calls();
    let mut old = f.request("old-connector");
    old.connector_version = "2".into();
    assert_eq!(
        f.adapter
            .reconcile(&f.store, OBSERVER, old)
            .await
            .unwrap_err(),
        GitHubError::PreconditionsChanged
    );
    f.admin.execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='github-observer'",&[]).await.unwrap();
    assert_eq!(
        f.adapter
            .reconcile(&f.store, OBSERVER, f.request("original"))
            .await
            .unwrap_err(),
        GitHubError::AuthorizationUnavailable
    );
    assert_eq!(calls, f.github.api.calls());
}

#[tokio::test]
async fn replaced_candidate_keeps_original_inspection_as_audit_without_current_facts() {
    let f = setup_github().await;
    let reservation = f
        .store
        .reserve_inspection(
            TENANT,
            PROJECT,
            OBSERVER,
            ReserveDeliveryInspection {
                request_id: "race-reserve".into(),
                read_set: f.set.clone(),
                connector_id: f.github.config.connector_id.clone(),
                connector_version: "1".into(),
                candidate_digest: f.select.candidate.binding.digest().unwrap(),
                lease_seconds: 120,
            },
        )
        .await
        .unwrap();
    let inspection = reservation["data"]["inspection_id"].as_str().unwrap();
    let snapshot = f
        .adapter
        .inspect(&f.select.candidate, inspection)
        .await
        .unwrap();
    let mut select = f.select.clone();
    select.request_id = "replace".into();
    select.expected_selected_digest = Some(select.candidate.binding.digest().unwrap());
    select.candidate.binding.candidate_version = "2".into();
    f.store
        .select_candidate(TENANT, PROJECT, A, select)
        .await
        .unwrap();
    let rejected = f
        .store
        .ingest_facts(
            TENANT,
            PROJECT,
            OBSERVER,
            IngestDeliveryFacts {
                request_id: "race-ingest".into(),
                read_set: f.set.clone(),
                connector_id: f.github.config.connector_id.clone(),
                inspection_id: inspection.into(),
                event_id: "race-event".into(),
                records: snapshot.records,
            },
        )
        .await;
    // The neutral inbox deliberately retains authentic late facts for audit.
    // They cannot become heads of the newly selected candidate.
    let superseded = rejected.unwrap();
    assert_eq!(
        superseded["data"]["observation_receipt"]["state"],
        "superseded"
    );
    assert_eq!(
        f.store
            .inspect(TENANT, PROJECT, OBSERVER, "a")
            .await
            .unwrap()["facts"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn failed_query_does_not_create_a_passing_fact_or_complete_source_work() {
    let f = setup_github().await;
    f.github
        .api
        .raw("", 500, b"synthetic provider error".to_vec());
    assert_eq!(
        f.adapter
            .reconcile(&f.store, OBSERVER, f.request("failed"))
            .await
            .unwrap_err(),
        GitHubError::ProviderUnavailable
    );
    let view = f
        .store
        .inspect(TENANT, PROJECT, OBSERVER, "a")
        .await
        .unwrap();
    assert_eq!(view["facts"].as_array().unwrap().len(), 0);
    assert_eq!(f.github.files(), 0);
    assert!(!view["acceptance_ready"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn provider_mapping_and_workstream_are_checked_before_network_access() {
    let f = setup_github().await;
    let mut config = f.github.config.clone();
    config.resource = "repository:other".into();
    let adapter = GitHubAdapter::from_transport(config, f.github.api.clone()).unwrap();
    assert_eq!(
        adapter
            .reconcile(&f.store, OBSERVER, f.request("mapping"))
            .await
            .unwrap_err(),
        GitHubError::BindingMismatch
    );
    let mut other = f.request("workstream");
    other.read_set.workstream_id = awr_core::Id::from(2);
    assert_eq!(
        f.adapter
            .reconcile(&f.store, OBSERVER, other)
            .await
            .unwrap_err(),
        GitHubError::BindingMismatch
    );
    assert_eq!(f.github.api.calls(), 0);
}
