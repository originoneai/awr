#![cfg(feature = "pg-tests")]
//! Synthetic authenticated transport mechanisms, not native business acceptance.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_source::{PublishPrepOptions, fingerprint, prepare_publish_from_server_directory};
use awr_team::{SourceActivationPlan, delivery::*};
use awr_team_pg::*;
use fixture::*;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::MutexGuard};
use tokio_postgres::Client;

const RESOURCE: &str = "fixture://publication-artifacts";

fn access_denied(result: &PgResult<Value>) -> bool {
    matches!(
        result,
        Err(PgError::Forbidden | PgError::Workstream(awr_core::WorkstreamError::AccessDenied))
    )
}

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    root: PathBuf,
    admin: Client,
    config: tokio_postgres::Config,
    reads: WorkstreamReadStore,
    set: DeliveryReadSet,
    selection: SelectDeliveryCandidate,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn set(p: &Value) -> DeliveryReadSet {
    serde_json::from_value(json!({"work_id":p["data"]["work_id"],"workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],
        "authority_version":p["authority_version"],"ownership_version":p["data"]["ownership_version"],
        "contract_hash":p["data"]["contract_hash"]})).unwrap()
}

async fn select(
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

async fn setup_commands() -> Fixture {
    let (guard, admin, db, reads) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let root = std::env::temp_dir().join(format!("awr-neutral-command-{}", awr_core::Id::new()));
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
        set: selection.read_set.clone(),
        selection,
    };
    f
}

fn wire<T: serde::Serialize>(op: &str, request: &T) -> WorkstreamCommand {
    let mut args = serde_json::to_value(request)
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    let set: DeliveryReadSet = serde_json::from_value(args.remove("read_set").unwrap()).unwrap();
    let request_id = args
        .remove("request_id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    args.insert("source_snapshot_id".into(), json!(set.source_snapshot_id));
    WorkstreamCommand {
        protocol_version: 1,
        request_id,
        op: op.into(),
        workstream_id: set.workstream_id,
        work_id: set.work_id,
        coordinator_epoch: set.coordinator_epoch,
        expected_project_revision: "0".into(),
        expected_authority_version: set.authority_version,
        expected_ownership_version: set.ownership_version,
        expected_contract_hash: set.contract_hash,
        args: Value::Object(args),
    }
}

impl Fixture {
    async fn execute<T: serde::Serialize>(&self, op: &str, request: &T) -> Value {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.reads
                .commands()
                .execute(TENANT, PROJECT, A, wire(op, request)),
        )
        .await
        .expect("neutral dispatch must not nest project admission transactions")
        .unwrap()
    }
    async fn read(&self, token: &str, op: &str, request: Option<&str>) -> PgResult<Value> {
        let mut q = query(op);
        q.work_id = Some("a".into());
        q.request_id = request.map(str::to_owned);
        self.reads
            .query(TENANT, PROJECT, token, q)
            .await
            .map(|v| v["data"].clone())
    }
    fn configure(&self, key: &str) -> ConfigureDeliveryConnector {
        ConfigureDeliveryConnector {
            request_id: key.into(),
            read_set: self.set.clone(),
            expected_connector_version: "0".into(),
            mapping: DeliveryConnectorMapping {
                connector_id: "wire-connector".into(),
                provider: "reference".into(),
                resource: RESOURCE.into(),
                principal_actor_id: "agent".into(),
                principal_client_id: "cli-a".into(),
                fact_source: FactSource::CallerDeclared,
                enabled: true,
            },
        }
    }
    fn reserve_request(&self, key: &str) -> ReserveDeliveryInspection {
        ReserveDeliveryInspection {
            request_id: key.into(),
            read_set: self.set.clone(),
            connector_id: "wire-connector".into(),
            connector_version: "1".into(),
            candidate_digest: self.selection.candidate.binding.digest().unwrap(),
            lease_seconds: 60,
        }
    }
    fn ingest_request(&self, reserved: &Value, key: &str) -> IngestDeliveryFacts {
        IngestDeliveryFacts {
            request_id: key.into(),
            read_set: self.set.clone(),
            connector_id: "wire-connector".into(),
            inspection_id: reserved["receipt"]["data"]["inspection_id"]
                .as_str()
                .unwrap()
                .into(),
            event_id: key.into(),
            records: vec![DeliveryEnvelope {
                protocol: DELIVERY_PROTOCOL.into(),
                protocol_version: DELIVERY_PROTOCOL_VERSION,
                record: DeliveryRecord::Verification(VerificationRun {
                    binding: self.selection.candidate.binding.clone(),
                    run_id: key.into(),
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
    async fn observe(&self) -> Value {
        self.execute(
            "delivery.connector.configure",
            &self.configure("wire-configure"),
        )
        .await;
        let reserved = self
            .execute(
                "delivery.inspection.reserve",
                &self.reserve_request("wire-reserve"),
            )
            .await;
        self.execute(
            "delivery.facts.ingest",
            &self.ingest_request(&reserved, "wire-ingest"),
        )
        .await
    }
    async fn publication(&self, key: &str) -> Value {
        let status = self.read(A, "delivery.source.status", None).await.unwrap();
        let facts = self
            .read(A, "delivery.neutral.inspect", None)
            .await
            .unwrap();
        self.execute(
            "delivery.source.prepare",
            &PrepareDeliverySourcePublication {
                request_id: key.into(),
                read_set: self.set.clone(),
                candidate_digest: self.selection.candidate.binding.digest().unwrap(),
                expected_selection_version: facts["selection_version"].as_str().unwrap().into(),
                expected_metadata_revision: status["metadata_revision"].as_str().unwrap().into(),
                expected_source_fingerprint: status["confirmed_fingerprint"]
                    .as_str()
                    .unwrap()
                    .into(),
                completion_receipt_id: None,
                lease_seconds: 60,
            },
        )
        .await["receipt"]["data"]
            .clone()
    }
    fn step(&self, p: &Value, key: &str) -> DeliveryPublicationStep {
        DeliveryPublicationStep {
            request_id: key.into(),
            read_set: self.set.clone(),
            publication_id: p["publication_id"].as_str().unwrap().into(),
            fence: p["fence"].as_str().unwrap().into(),
        }
    }
    async fn counts(&self) -> Value {
        let row = self
            .admin
            .query_one(
                "SELECT
            (SELECT count(*) FROM awr_team.delivery_sync_requests),
            (SELECT count(*) FROM awr_team.operations),
            (SELECT count(*) FROM awr_team.delivery_inbox),
            (SELECT count(*) FROM awr_team.delivery_source_publications),
            (SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2)",
                &[&TENANT, &PROJECT],
            )
            .await
            .unwrap();
        json!([
            row.get::<_, i64>(0),
            row.get::<_, i64>(1),
            row.get::<_, i64>(2),
            row.get::<_, i64>(3),
            row.get::<_, i64>(4)
        ])
    }
}

#[tokio::test]
async fn neutral_commands_publish_and_query_without_duplicate_generic_journals() {
    let f = setup_commands().await;
    let before = f.counts().await;
    let mut selection = f.selection.clone();
    selection.request_id = "wire-selection".into();
    selection.expected_selected_digest = Some(selection.candidate.binding.digest().unwrap());
    let selected = f.execute("delivery.candidate.select", &selection).await;
    assert_eq!(selected["receipt"]["protocol"], "awr-delivery-sync-v1");
    assert_eq!(selected["execution_authorized"], false);
    let ingested = f.observe().await;
    assert_eq!(
        ingested["receipt"]["data"]["observation_receipt"]["state"],
        "applied"
    );
    let facts = f.read(A, "delivery.neutral.inspect", None).await.unwrap();
    assert_eq!(facts["selected_current"], true);
    assert_eq!(facts["facts"][0]["current"], true);
    assert_eq!(facts["acceptance_ready"], false);
    let publication = f.publication("wire-prepare").await;
    let step = f.step(&publication, "wire-renew");
    let mut renewal = wire("delivery.source.renew", &step);
    renewal.args["lease_seconds"] = json!(60);
    let renewed = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, A, renewal)
        .await
        .unwrap();
    let mut new_step = f.step(&publication, "wire-write");
    new_step.fence = renewed["receipt"]["data"]["fence"].as_str().unwrap().into();
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                wire("delivery.source.write", &f.step(&publication, "old-fence"))
            )
            .await,
        Err(PgError::StaleFence)
    ));
    let written = f.execute("delivery.source.write", &new_step).await;
    new_step.request_id = "wire-confirm".into();
    let confirmed = f.execute("delivery.source.confirm", &new_step).await;
    assert_eq!(confirmed["receipt"]["data"]["phase"], "confirmed");
    assert_eq!(
        f.read(A, "delivery.source.status", None).await.unwrap()["source_synchronized"],
        true
    );
    let outcome = f
        .read(A, "delivery.neutral.outcome", Some("wire-write"))
        .await
        .unwrap();
    assert_eq!(outcome["receipt"], written["receipt"]);
    assert_eq!(outcome["state_basis"], "at_commit");
    assert_eq!(outcome["execution_authorized"], false);
    let after = f.counts().await;
    assert_eq!(
        after[1], before[1],
        "neutral mutations do not create generic operation rows"
    );
    let bytes = std::fs::read(f.root.join("ledger.yaml")).unwrap();
    let ledger = String::from_utf8(bytes).unwrap();
    assert!(ledger.contains("  - id: a\n    title: Work A\n    status: planned\n"));
    let note: awr_source::DeliverySourceNote = serde_json::from_str(
        ledger
            .lines()
            .find_map(|line| line.trim_start().strip_prefix("\"delivery_sync\": "))
            .expect("publication writes the typed reference note"),
    )
    .unwrap();
    assert_eq!(
        note.candidate_digest,
        selection.candidate.binding.digest().unwrap()
    );
    assert_eq!(note.work_external_key, "a");
    assert_eq!(note.fact_ids.len(), 1);
    assert!(
        note.completion_reference.is_none(),
        "observations never fabricate completion"
    );
}

#[tokio::test]
async fn simultaneous_exact_retry_commits_one_domain_receipt_event_and_audit() {
    let f = setup_commands().await;
    let request = f.configure("concurrent-configure");
    let c = wire("delivery.connector.configure", &request);
    let a = f.reads.commands();
    let b = WorkstreamReadStore::from_config(f.config.clone()).commands();
    let (first, second) = tokio::join!(
        a.execute(TENANT, PROJECT, A, c.clone()),
        b.execute(TENANT, PROJECT, A, c.clone())
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first["replayed"], second["replayed"]);
    assert_eq!(first["receipt"]["data"], second["receipt"]["data"]);
    let row=f.admin.query_one("SELECT
        (SELECT count(*) FROM awr_team.delivery_sync_requests WHERE request_id='concurrent-configure'),
        (SELECT count(*) FROM awr_team.events WHERE event_type='delivery.connector.configure'),
        (SELECT count(*) FROM awr_team.ops_audit_records WHERE request_id='concurrent-configure')",&[]).await.unwrap();
    for i in 0..3 {
        assert_eq!(row.get::<_, i64>(i), 1);
    }
    let mut changed = c.clone();
    changed.args["mapping"]["enabled"] = json!(false);
    assert!(matches!(
        a.execute(TENANT, PROJECT, A, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    let mut cursor = c;
    cursor.expected_project_revision = "12345".into();
    assert_eq!(
        a.execute(TENANT, PROJECT, A, cursor).await.unwrap()["replayed"],
        true
    );
}

#[tokio::test]
async fn header_shadowing_invalid_versions_and_bounds_have_no_effect() {
    let f = setup_commands().await;
    let original = wire("delivery.connector.configure", &f.configure("malformed"));
    let before = f.counts().await;
    for field in [
        "read_set",
        "request_id",
        "step",
        "work_id",
        "actor_id",
        "client_id",
        "source_path",
    ] {
        let mut c = original.clone();
        c.args[field] = json!("injected");
        assert!(
            f.reads
                .commands()
                .execute(TENANT, PROJECT, A, c)
                .await
                .is_err(),
            "{field}"
        );
    }
    for value in ["", "01", "-1", "9223372036854775808"] {
        for field in 0..3 {
            let mut c = original.clone();
            match field {
                0 => c.expected_project_revision = value.into(),
                1 => c.expected_authority_version = value.into(),
                _ => c.expected_ownership_version = value.into(),
            }
            assert!(
                f.reads
                    .commands()
                    .execute(TENANT, PROJECT, A, c)
                    .await
                    .is_err()
            );
        }
    }
    let mut c = original.clone();
    c.protocol_version = 2;
    assert!(matches!(
        f.reads.commands().execute(TENANT, PROJECT, A, c).await,
        Err(PgError::Unsupported(_))
    ));
    let mut c = original.clone();
    c.args["source_snapshot_id"] = json!("wrong-source");
    assert!(matches!(
        f.reads.commands().execute(TENANT, PROJECT, A, c).await,
        Err(PgError::PreconditionsChanged)
    ));
    let mut c = original.clone();
    c.args["mapping"]["resource"] = json!("x".repeat(66000));
    assert!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, A, c)
            .await
            .is_err()
    );
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn outcomes_require_current_actual_work_actor_client_and_permission() {
    let f = setup_commands().await;
    assert_eq!(
        f.read(A, "delivery.neutral.outcome", Some("not-submitted"))
            .await
            .unwrap()["outcome"],
        "unknown"
    );
    f.execute(
        "delivery.connector.configure",
        &f.configure("not-submitted"),
    )
    .await;
    assert_eq!(
        f.read(A, "delivery.neutral.outcome", Some("not-submitted"))
            .await
            .unwrap()["outcome"],
        "committed"
    );
    let mut configured = f.configure("outcome");
    configured.expected_connector_version = "1".into();
    let original = f.execute("delivery.connector.configure", &configured).await;
    let mut q = query("delivery.neutral.outcome");
    q.work_id = Some("c".into());
    q.request_id = Some("outcome".into());
    assert!(matches!(
        f.reads.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    f.admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-b',$3,2,true)",&[&TENANT,&PROJECT,&f.set.workstream_id.to_string()]).await.unwrap();
    assert_eq!(
        f.read(B, "delivery.neutral.outcome", Some("outcome"))
            .await
            .unwrap()["outcome"],
        "unknown"
    );
    let other =
        "awr1.other-reader.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    f.admin.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,'other-reader','reviewer','cli-other',$2)",
        &[&TENANT,&workstream_credential_hash(other).unwrap()]).await.unwrap();
    f.admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'reviewer','cli-other',$3,2,true)",&[&TENANT,&PROJECT,&f.set.workstream_id.to_string()]).await.unwrap();
    assert_eq!(
        f.read(other, "delivery.neutral.outcome", Some("outcome"))
            .await
            .unwrap()["outcome"],
        "unknown"
    );
    let mut hidden = query("delivery.neutral.inspect");
    hidden.work_id = Some("b-private".into());
    assert!(access_denied(
        &f.reads.query(TENANT, PROJECT, A, hidden).await
    ));
    assert!(access_denied(
        &f.read(NONE, "delivery.neutral.outcome", Some("outcome"))
            .await
    ));
    let mut q = query("delivery.neutral.outcome");
    q.work_id = Some("a".into());
    q.request_id = Some("outcome".into());
    assert!(access_denied(
        &f.reads.query("other-tenant", PROJECT, A, q.clone()).await
    ));
    assert!(access_denied(
        &f.reads.query(TENANT, "missing-project", A, q.clone()).await
    ));
    let outcome = f
        .read(A, "delivery.neutral.outcome", Some("outcome"))
        .await
        .unwrap();
    assert_eq!(outcome["receipt"], original["receipt"]);
    f.admin.batch_execute("UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='reader-a'").await.unwrap();
    assert!(access_denied(
        &f.read(A, "delivery.neutral.outcome", Some("outcome")).await
    ));
    assert!(access_denied(
        &f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                wire("delivery.connector.configure", &f.configure("outcome"))
            )
            .await
    ));
    f.admin.batch_execute("UPDATE awr_team.credentials SET expires_at=NULL WHERE id='reader-a'; UPDATE awr_team.workstream_grants SET active=false WHERE client_id='cli-a'").await.unwrap();
    assert!(access_denied(
        &f.read(A, "delivery.neutral.outcome", Some("outcome")).await
    ));
    f.admin
        .batch_execute("UPDATE awr_team.workstream_grants SET active=true WHERE client_id='cli-a'")
        .await
        .unwrap();
    f.admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(access_denied(
        &f.read(A, "delivery.neutral.outcome", Some("outcome")).await
    ));
    assert!(access_denied(
        &f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                wire("delivery.connector.configure", &f.configure("outcome"))
            )
            .await
    ));
}

#[tokio::test]
async fn abandoned_source_intent_and_read_only_currentness_remain_distinct() {
    let f = setup_commands().await;
    f.observe().await;
    let before = std::fs::read(f.root.join("ledger.yaml")).unwrap();
    let publication = f.publication("abandon-prepare").await;
    let abandoned = f
        .execute("delivery.source.abandon", &f.step(&publication, "abandon"))
        .await;
    assert_eq!(abandoned["receipt"]["data"]["phase"], "conflict");
    assert_eq!(abandoned["receipt"]["data"]["failure_code"], "withdrawn");
    assert_eq!(std::fs::read(f.root.join("ledger.yaml")).unwrap(), before);
    let count = f.counts().await;
    let listing: Vec<_> = std::fs::read_dir(&f.root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let status = f.read(A, "delivery.source.status", None).await.unwrap();
    assert_eq!(status["source_synchronized"], false);
    assert_eq!(f.counts().await, count);
    let listing_after: Vec<_> = std::fs::read_dir(&f.root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(listing_after, listing);
    let mut q = query("delivery.neutral.outcome");
    q.session_id = Some("session-a".into());
    q.request_id = Some("abandon".into());
    assert!(
        q.validate().is_err(),
        "neutral reads require actual work, not a session selector"
    );
}

#[tokio::test]
async fn delegated_developer_transport_cannot_promote_itself_to_source_manager() {
    let f = setup_commands().await;
    f.observe().await;
    f.admin.batch_execute("UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]'::jsonb WHERE actor_id='agent'").await.unwrap();
    let mut selection = f.selection.clone();
    selection.request_id = "developer-selection".into();
    selection.expected_selected_digest = Some(selection.candidate.binding.digest().unwrap());
    assert_eq!(
        f.execute("delivery.candidate.select", &selection).await["execution_authorized"],
        false
    );
    assert!(access_denied(
        &f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                wire(
                    "delivery.connector.configure",
                    &f.configure("developer-manager")
                )
            )
            .await
    ));
    f.admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity)
        VALUES('reader-tenant','reader-project','wire-member','Simulated member','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"synthetic-controller\"}'::jsonb);
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
        VALUES('reader-tenant','reader-project','wire-binding','wire-member','agent','active')").await.unwrap();
    assert!(access_denied(
        &f.read(A, "delivery.neutral.inspect", None).await
    ));
    let grant = awr_core::AgentAuthorization {
        id: "wire-delegation".into(),
        authorizer_person_id: awr_core::PersonId::new("wire-member").unwrap(),
        responsible_person_id: awr_core::PersonId::new("wire-member").unwrap(),
        subject_kind: awr_core::ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: awr_core::AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: f.set.workstream_id.to_string(),
        },
        actions: std::collections::BTreeSet::from([
            awr_core::AuthorizedAction::Inspect,
            awr_core::AuthorizedAction::StartWork,
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
        binding_id: Some("wire-binding".into()),
    };
    AuthorizationStore::from_config(f.config.clone())
        .issue(
            TENANT,
            PROJECT,
            &awr_core::IssueAuthorizationRequest {
                request_key: "wire-delegation-issue".into(),
                authorization: grant,
            },
        )
        .await
        .unwrap();
    let reserved = f
        .execute(
            "delivery.inspection.reserve",
            &f.reserve_request("delegated-reserve"),
        )
        .await;
    let request = f.ingest_request(&reserved, "delegated-facts");
    assert_eq!(
        f.execute("delivery.facts.ingest", &request).await["receipt"]["data"]["observation_receipt"]
            ["state"],
        "applied"
    );
    assert_eq!(
        f.read(A, "delivery.neutral.outcome", Some("delegated-facts"))
            .await
            .unwrap()["outcome"],
        "committed"
    );
    let before = f.counts().await;
    let status = f.read(A, "delivery.source.status", None).await.unwrap();
    let facts = f.read(A, "delivery.neutral.inspect", None).await.unwrap();
    let prepare = PrepareDeliverySourcePublication {
        request_id: "agent-source-prepare".into(),
        read_set: f.set.clone(),
        candidate_digest: selection.candidate.binding.digest().unwrap(),
        expected_selection_version: facts["selection_version"].as_str().unwrap().into(),
        expected_metadata_revision: status["metadata_revision"].as_str().unwrap().into(),
        expected_source_fingerprint: status["confirmed_fingerprint"].as_str().unwrap().into(),
        completion_receipt_id: None,
        lease_seconds: 60,
    };
    assert!(access_denied(
        &f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                wire("delivery.source.prepare", &prepare)
            )
            .await
    ));
    assert!(access_denied(
        &f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                wire(
                    "delivery.connector.configure",
                    &f.configure("wire-configure")
                )
            )
            .await
    ));
    for op in [
        "delivery.source.write",
        "delivery.source.confirm",
        "delivery.source.abandon",
        "delivery.source.renew",
    ] {
        let step = DeliveryPublicationStep {
            request_id: op.into(),
            read_set: f.set.clone(),
            publication_id: "synthetic-unwritten-intent".into(),
            fence: "1".into(),
        };
        let mut c = wire(op, &step);
        if op == "delivery.source.renew" {
            c.args["lease_seconds"] = json!(60);
        }
        assert!(
            access_denied(&f.reads.commands().execute(TENANT, PROJECT, A, c).await),
            "{op}"
        );
    }
    assert_eq!(f.counts().await, before);
    f.admin.batch_execute("UPDATE awr_team.agent_authorizations SET expires_at_ms=1,body_json=jsonb_set(body_json,'{expires_at_ms}','1') WHERE id='wire-delegation'").await.unwrap();
    assert!(access_denied(
        &f.read(A, "delivery.neutral.outcome", Some("delegated-facts"))
            .await
    ));
    assert!(access_denied(
        &f.reads
            .commands()
            .execute(TENANT, PROJECT, A, wire("delivery.facts.ingest", &request))
            .await
    ));
    f.admin.batch_execute("UPDATE awr_team.agent_authorizations SET expires_at_ms=NULL,body_json=jsonb_set(body_json,'{expires_at_ms}','null') WHERE id='wire-delegation'").await.unwrap();
    assert_eq!(
        f.read(A, "delivery.neutral.inspect", None).await.unwrap()["selected_current"],
        true
    );
    f.admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='wire-delegation'",
        )
        .await
        .unwrap();
    assert!(access_denied(
        &f.read(A, "delivery.neutral.outcome", Some("delegated-facts"))
            .await
    ));
    assert!(access_denied(
        &f.reads
            .commands()
            .execute(TENANT, PROJECT, A, wire("delivery.facts.ingest", &request))
            .await
    ));
}

#[tokio::test]
async fn entire_command_envelope_and_sensitive_values_are_refused_before_effects() {
    let f = setup_commands().await;
    let before = f.counts().await;
    let reserved = json!({"receipt":{"data":{"inspection_id":"not-yet-reserved"}}});
    let mut request = f.ingest_request(&reserved, "envelope-limit");
    request.records = vec![request.records[0].clone(); 16];
    for (i, record) in request.records.iter_mut().enumerate() {
        if let DeliveryRecord::Verification(v) = &mut record.record {
            // All nested envelopes remain valid; only the outer transport is too large.
            v.run_id = format!("bound-run-{i:02}");
        }
    }
    let mut remaining = 65537
        - serde_json::to_vec(&wire("delivery.facts.ingest", &request))
            .unwrap()
            .len();
    for record in &mut request.records {
        if let DeliveryRecord::Verification(v) = &mut record.record {
            let added = remaining.min(4096 - v.provenance.reference.len());
            v.provenance.reference.push_str(&"r".repeat(added));
            remaining -= added;
        }
        record.validate().unwrap();
    }
    assert_eq!(remaining, 0);
    let command = wire("delivery.facts.ingest", &request);
    assert!(serde_json::to_vec(&request).unwrap().len() <= 65536);
    assert_eq!(serde_json::to_vec(&command).unwrap().len(), 65537);
    assert!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, A, command)
            .await
            .unwrap_err()
            .is_invalid_command_fields()
    );
    let secret = format!("ghp_{}", "a".repeat(36));
    let mut sensitive = f.configure("sensitive");
    sensitive.mapping.resource = format!("fixture://resource/{secret}");
    let error = f
        .reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            wire("delivery.connector.configure", &sensitive),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains(&secret));
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn history_is_bounded_and_oversized_outcomes_fail_without_mutation() {
    let f = setup_commands().await;
    f.execute(
        "delivery.connector.configure",
        &f.configure("wire-configure"),
    )
    .await;
    for i in 0..34 {
        let reserved = f
            .execute(
                "delivery.inspection.reserve",
                &f.reserve_request(&format!("history-reserve-{i}")),
            )
            .await;
        f.execute(
            "delivery.facts.ingest",
            &f.ingest_request(&reserved, &format!("history-facts-{i}")),
        )
        .await;
    }
    let before = f.counts().await;
    let view = f.read(A, "delivery.neutral.inspect", None).await.unwrap();
    assert_eq!(view["history"].as_array().unwrap().len(), 32);
    assert_eq!(view["history_truncated"], true);
    assert_eq!(view["facts"].as_array().unwrap().len(), 1);
    assert_eq!(view["facts"][0]["current"], true);
    f.admin.execute("UPDATE awr_team.delivery_sync_requests SET result_json=result_json||jsonb_build_object('diagnostic',$1::text) WHERE request_id='wire-configure'",
        &[&"x".repeat(262144)]).await.unwrap();
    assert!(matches!(
        f.read(A, "delivery.neutral.outcome", Some("wire-configure"))
            .await,
        Err(PgError::ResponseTooLarge)
    ));
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn historical_outcomes_survive_only_as_receipts_after_relevant_changes() {
    let f = setup_commands().await;
    let original = f.observe().await;
    let publication = f.publication("current-prepare").await;
    f.execute(
        "delivery.source.write",
        &f.step(&publication, "current-write"),
    )
    .await;
    f.execute(
        "delivery.source.confirm",
        &f.step(&publication, "current-confirm"),
    )
    .await;
    let mut config = f.configure("connector-change");
    config.expected_connector_version = "1".into();
    config.mapping.enabled = false;
    f.execute("delivery.connector.configure", &config).await;
    assert_eq!(
        f.read(A, "delivery.neutral.inspect", None).await.unwrap()["facts"][0]["current"],
        false
    );
    assert_eq!(
        f.read(A, "delivery.source.status", None).await.unwrap()["source_synchronized"],
        false
    );
    let mut selection = f.selection.clone();
    selection.request_id = "candidate-change".into();
    selection.expected_selected_digest = Some(selection.candidate.binding.digest().unwrap());
    selection.candidate.binding.candidate_version = "2".into();
    f.execute("delivery.candidate.select", &selection).await;
    let prepared = prepare(&f.reads, A, "a").await;
    f.reads.commands().execute(TENANT,PROJECT,A,command(&prepared,"release-source-transition","claim.release",json!({
        "session_id":"session-a","expected_session_version":"1","claim_id":selection.claim_id,
        "expected_fence":selection.fence,"expected_lease_version":selection.lease_version,
    }))).await.unwrap();
    let prepared = prepare(&f.reads, A, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "quiesce-source-transition",
                "session.end",
                json!({"session_id":"session-a","expected_session_version":"1"}),
            ),
        )
        .await
        .unwrap();
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
    let view = f.read(A, "delivery.neutral.inspect", None).await.unwrap();
    assert_ne!(view["source_snapshot_id"], f.set.source_snapshot_id);
    assert_eq!(view["selected_current"], false);
    let outcome = f
        .read(A, "delivery.neutral.outcome", Some("wire-ingest"))
        .await
        .unwrap();
    assert_eq!(outcome["receipt"], original["receipt"]);
    assert_eq!(outcome["state_basis"], "at_commit");
    assert_eq!(outcome["execution_authorized"], false);
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                wire(
                    "delivery.connector.configure",
                    &f.configure("wire-configure")
                )
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
}

#[tokio::test]
async fn source_status_never_recreates_missing_locks_or_source_files() {
    let f = setup_commands().await;
    f.observe().await;
    let publication = f.publication("readonly-prepare").await;
    f.execute(
        "delivery.source.write",
        &f.step(&publication, "readonly-write"),
    )
    .await;
    f.execute(
        "delivery.source.confirm",
        &f.step(&publication, "readonly-confirm"),
    )
    .await;
    let lock = std::fs::read_dir(&f.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".lock")
        })
        .unwrap();
    std::fs::rename(&lock, lock.with_extension("retained")).unwrap();
    let before = f.counts().await;
    assert_eq!(
        f.read(A, "delivery.source.status", None).await.unwrap()["source_synchronized"],
        false
    );
    assert!(!lock.exists());
    std::fs::write(&lock, []).unwrap();
    assert_eq!(
        f.read(A, "delivery.source.status", None).await.unwrap()["source_synchronized"],
        false
    );
    std::fs::remove_file(f.root.join("ledger.yaml")).unwrap();
    let status = f.read(A, "delivery.source.status", None).await.unwrap();
    assert_eq!(status["source_observation"], "unavailable");
    assert_eq!(status["source_synchronized"], false);
    assert!(!f.root.join("ledger.yaml").exists());
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn changed_ownership_invalidates_current_facts_without_rewriting_outcomes() {
    let f = setup_commands().await;
    let original = f.observe().await;
    f.admin.execute("UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=ownership_version+1
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND work_id='a'",
        &[&TENANT,&PROJECT,&f.set.source_snapshot_id]).await.unwrap();
    let view = f.read(A, "delivery.neutral.inspect", None).await.unwrap();
    assert_eq!(view["selected_current"], false);
    assert_eq!(view["facts"][0]["current"], false);
    assert_eq!(
        f.read(A, "delivery.neutral.outcome", Some("wire-ingest"))
            .await
            .unwrap()["receipt"],
        original["receipt"]
    );
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                wire(
                    "delivery.inspection.reserve",
                    &f.reserve_request("old-owner")
                )
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
}

#[tokio::test]
async fn concurrent_selection_and_reads_preserve_one_authenticated_generation() {
    let f = setup_commands().await;
    f.observe().await;
    let writer = async {
        let mut selection = f.selection.clone();
        for i in 2..10 {
            selection.request_id = format!("concurrent-selection-{i}");
            selection.expected_selected_digest =
                Some(selection.candidate.binding.digest().unwrap());
            selection.candidate.binding.candidate_version = i.to_string();
            f.execute("delivery.candidate.select", &selection).await;
            tokio::task::yield_now().await;
        }
    };
    let reader = async {
        for _ in 0..30 {
            let mut q = query("delivery.neutral.inspect");
            q.work_id = Some("a".into());
            let view = f.reads.query(TENANT, PROJECT, A, q).await.unwrap();
            assert_eq!(view["project_revision"], view["data"]["project_revision"]);
            assert_eq!(
                view["source_snapshot_id"],
                view["data"]["source_snapshot_id"]
            );
            assert_eq!(view["data"]["selected_current"], true);
            assert_eq!(
                view["data"]["facts"][0]["current"],
                view["data"]["candidate"]["binding"]["candidate_version"] == "1"
            );
            tokio::task::yield_now().await;
        }
    };
    tokio::join!(writer, reader);
}
