//! Shared isolated filesystem/PG fixture; no native business acceptance.
#![allow(dead_code)]
use crate::{common, fixture::*};
use awr_source::{PublishPrepOptions, fingerprint, prepare_publish_from_server_directory};
use awr_team::{SourceActivationPlan, delivery::*};
use awr_team_pg::*;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::MutexGuard};
use tokio_postgres::Client;

pub(crate) const RESOURCE: &str = "fixture://publication-artifacts";

pub(crate) fn access_denied(result: &PgResult<Value>) -> bool {
    matches!(
        result,
        Err(PgError::Forbidden | PgError::Workstream(awr_core::WorkstreamError::AccessDenied))
    )
}

pub(crate) struct Fixture {
    _guard: MutexGuard<'static, ()>,
    pub(crate) root: PathBuf,
    pub(crate) admin: Client,
    pub(crate) config: tokio_postgres::Config,
    pub(crate) reads: WorkstreamReadStore,
    pub(crate) store: DeliverySyncStore,
    pub(crate) set: DeliveryReadSet,
    pub(crate) selection: SelectDeliveryCandidate,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(crate) fn set(p: &Value) -> DeliveryReadSet {
    serde_json::from_value(json!({"work_id":p["data"]["work_id"],"workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],
        "authority_version":p["authority_version"],"ownership_version":p["data"]["ownership_version"],
        "contract_hash":p["data"]["contract_hash"]})).unwrap()
}

pub(crate) async fn select(
    reads: &WorkstreamReadStore,
    store: &DeliverySyncStore,
    work: &str,
    session: &str,
) -> SelectDeliveryCandidate {
    let p = prepare(reads, A, work).await;
    let claim=reads.commands().execute(TENANT,PROJECT,A,command(&p,&format!("claim-{work}"),"task.claim_available",
        json!({"session_id":session,"expected_session_version":"1",
            "expected_responsibility_version":p["data"]["responsibility"]["version"],
            "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":3600})))
        .await.unwrap()["receipt"]["data"].clone();
    let p = prepare(reads, A, work).await;
    let set = set(&p);
    let content = b"synthetic output\n";
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: "package".into(),
            sha256: fingerprint(content)[7..].into(),
            byte_length: content.len().to_string(),
            locator: "fixture://artifacts/package".into(),
        }],
    };
    let candidate=serde_json::from_value(json!({"binding":{
        "tenant_id":TENANT,"project_id":PROJECT,"scope_id":"main","workstream_id":set.workstream_id,
        "work_id":work,"candidate_id":format!("candidate-{work}"),"candidate_version":"1","contract_hash":set.contract_hash,
        "manifest_digest":manifest.digest().unwrap(),"source_revision":null,"required_checks":["report"],
        "target":{"resource":RESOURCE,"reference":"main","precondition":{"kind":"missing"}}
    },"manifest":manifest})).unwrap();
    let request = SelectDeliveryCandidate {
        request_id: format!("select-{work}"),
        read_set: set,
        expected_selected_digest: None,
        session_id: session.into(),
        claim_id: claim["claim_id"].as_str().unwrap().into(),
        fence: claim["fence"].as_str().unwrap().into(),
        lease_version: claim["lease_version"].as_str().unwrap().into(),
        candidate,
    };
    store
        .select_candidate(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    request
}

pub(crate) async fn setup_publisher() -> Fixture {
    let (guard, admin, db, reads) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let root = std::env::temp_dir().join(format!("awr-delivery-publisher-{}", awr_core::Id::new()));
    std::fs::create_dir_all(root.join("docs")).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        root.join("docs/spec.md"),
        "# Synthetic acceptance\nPreserve compatibility.\n",
    )
    .unwrap();
    let ledger = r#"# Preserve this comment and unrelated source metadata.
description: Synthetic publication fixture
workstreams:
  version: 1
  definitions:
    - id: 00000000000000000000000001
      external_key: alpha
      title: alpha
      state: active
      authority_version: 2
      goal_keys: [alpha]
      acceptance_contracts: [docs/spec.md]
    - id: 00000000000000000000000002
      external_key: private-beta
      title: private-beta
      state: active
      authority_version: 1
      goal_keys: [private-beta]
      acceptance_contracts: []
goals:
  - id: alpha
    title: Alpha
    status: active
  - id: private-beta
    title: Private
    status: active
work_items:
  - id: a
    title: Work A
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [verified]
    paths: [src]
    depends_on: []
    completion_policy: ordinary_confirm
    verification_requirements: [report]
  - id: c
    title: Work C
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [verified]
    paths: [other]
    depends_on: []
    completion_policy: ordinary_confirm
    verification_requirements: [report]
  - id: b-private
    title: Private work
    status: planned
    workstream: private-beta
    goals: [private-beta]
    acceptance: [verified]
    paths: [private]
    depends_on: []
    completion_policy: ordinary_confirm
    verification_requirements: [report]
  - id: d
    title: Independent work
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [verified]
    paths: [src]
    depends_on: []
    completion_policy: ordinary_confirm
    verification_requirements: [report]
"#;
    std::fs::write(root.join("ledger.yaml"), ledger).unwrap();
    let package = prepare_publish_from_server_directory(
        &root,
        "ledger.yaml",
        PROJECT,
        &PublishPrepOptions::default(),
    )
    .unwrap();
    let config = common::with_app_role(&common::test_config(), &db);
    let source = SourceStore::from_config(config.clone());
    let (candidate, _) = source
        .ingest_publish_candidate(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: package.parser_version,
            files: package
                .files
                .into_iter()
                .map(|f| SourceFile {
                    path: f.path,
                    bytes: f.bytes,
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
    source
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &candidate.proposal_id,
            &SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                parser_version: candidate.parser_version,
                expected_authority_epoch: candidate.base_epoch,
                approved_candidate_digest: candidate.manifest_digest,
            },
        )
        .await
        .unwrap();
    // Changing the stream's source spec requires a new authority version and
    // an explicit matching grant; source publication never grants access.
    admin.batch_execute("UPDATE awr_team.workstream_grants SET authority_version=2,grant_version=grant_version+1 WHERE client_id='cli-a'")
        .await.unwrap();
    let store = DeliverySyncStore::from_config(config.clone());
    let selection = select(&reads, &store, "a", "session-a").await;
    let f = Fixture {
        _guard: guard,
        root,
        admin,
        config,
        reads,
        store,
        set: selection.read_set.clone(),
        selection,
    };
    f.observe(&f.selection, "initial").await;
    f
}

impl Fixture {
    pub(crate) fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.root.join("ledger.yaml")).unwrap()
    }
    pub(crate) fn restarted(&self) -> DeliverySyncStore {
        DeliverySyncStore::from_config(self.config.clone())
    }
    pub(crate) async fn status(&self) -> Value {
        self.store
            .source_publication_status(TENANT, PROJECT, A, "a")
            .await
            .unwrap()
    }
    pub(crate) async fn observe(&self, selection: &SelectDeliveryCandidate, event: &str) {
        let work = &selection.read_set.work_id;
        let connector = format!("connector-{work}");
        if event == "initial" || work != "a" {
            self.store
                .configure_connector(
                    TENANT,
                    PROJECT,
                    A,
                    ConfigureDeliveryConnector {
                        request_id: format!("configure-{work}"),
                        read_set: selection.read_set.clone(),
                        expected_connector_version: "0".into(),
                        mapping: DeliveryConnectorMapping {
                            connector_id: connector.clone(),
                            provider: "reference".into(),
                            resource: RESOURCE.into(),
                            principal_actor_id: "agent".into(),
                            principal_client_id: "cli-a".into(),
                            fact_source: FactSource::CallerDeclared,
                            enabled: true,
                        },
                    },
                )
                .await
                .unwrap();
        }
        let reserved = self
            .store
            .reserve_inspection(
                TENANT,
                PROJECT,
                A,
                ReserveDeliveryInspection {
                    request_id: format!("inspect-{work}-{event}"),
                    read_set: selection.read_set.clone(),
                    connector_id: connector.clone(),
                    connector_version: "1".into(),
                    candidate_digest: selection.candidate.binding.digest().unwrap(),
                    lease_seconds: 60,
                },
            )
            .await
            .unwrap();
        let record = DeliveryEnvelope {
            protocol: DELIVERY_PROTOCOL.into(),
            protocol_version: DELIVERY_PROTOCOL_VERSION,
            record: DeliveryRecord::Verification(VerificationRun {
                binding: selection.candidate.binding.clone(),
                run_id: format!("run-{work}"),
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
        };
        self.store
            .ingest_facts(
                TENANT,
                PROJECT,
                A,
                IngestDeliveryFacts {
                    request_id: format!("ingest-{work}-{event}"),
                    read_set: selection.read_set.clone(),
                    connector_id: connector,
                    inspection_id: reserved["data"]["inspection_id"].as_str().unwrap().into(),
                    event_id: event.into(),
                    records: vec![record],
                },
            )
            .await
            .unwrap();
    }
    pub(crate) async fn request(&self, id: &str) -> PrepareDeliverySourcePublication {
        self.request_for(id, &self.selection).await
    }
    pub(crate) async fn request_for(
        &self,
        id: &str,
        selection: &SelectDeliveryCandidate,
    ) -> PrepareDeliverySourcePublication {
        let status = self
            .store
            .source_publication_status(TENANT, PROJECT, A, &selection.read_set.work_id)
            .await
            .unwrap();
        PrepareDeliverySourcePublication {
            request_id: id.into(),
            read_set: selection.read_set.clone(),
            candidate_digest: selection.candidate.binding.digest().unwrap(),
            expected_selection_version: "1".into(),
            expected_metadata_revision: status["metadata_revision"].as_str().unwrap().into(),
            expected_source_fingerprint: status["confirmed_fingerprint"].as_str().unwrap().into(),
            completion_receipt_id: None,
            lease_seconds: 60,
        }
    }
    pub(crate) async fn prepare(&self, id: &str) -> Value {
        self.store
            .prepare_source_publication(TENANT, PROJECT, A, self.request(id).await)
            .await
            .unwrap()["data"]
            .clone()
    }
    pub(crate) fn step(&self, p: &Value, id: &str) -> DeliveryPublicationStep {
        DeliveryPublicationStep {
            request_id: id.into(),
            read_set: self.set.clone(),
            publication_id: p["publication_id"].as_str().unwrap().into(),
            fence: p["fence"].as_str().unwrap().into(),
        }
    }
    pub(crate) async fn after_bytes(&self, p: &Value) -> Vec<u8> {
        self.admin
            .query_one(
                "SELECT after_bytes FROM awr_team.delivery_source_publications WHERE id=$1",
                &[&p["publication_id"].as_str().unwrap()],
            )
            .await
            .unwrap()
            .get(0)
    }
    pub(crate) async fn publish(&self, id: &str, selection: &SelectDeliveryCandidate) -> Value {
        let p = self
            .store
            .prepare_source_publication(TENANT, PROJECT, A, self.request_for(id, selection).await)
            .await
            .unwrap()["data"]
            .clone();
        let mut step = self.step(&p, &format!("write-{id}"));
        step.read_set = selection.read_set.clone();
        self.store
            .write_source_publication(TENANT, PROJECT, A, step.clone())
            .await
            .unwrap();
        step.request_id = format!("confirm-{id}");
        let result = self
            .store
            .confirm_source_publication(TENANT, PROJECT, A, step)
            .await
            .unwrap();
        assert_eq!(result["data"]["phase"], "confirmed");
        result
    }
}
