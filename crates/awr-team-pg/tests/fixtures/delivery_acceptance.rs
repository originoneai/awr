#![allow(dead_code)]
//! Real source/execution/review/finalization fixture shared with configured workers.
use crate::fixture::*;
use crate::integration_fixture::{self, *};
use awr_team::ExecutionSettlementPolicy;
use awr_team::delivery::*;
use awr_team_pg::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub struct SourceRoot(pub PathBuf);
impl Drop for SourceRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct SourceFixture {
    pub f: Fixture,
    pub root: SourceRoot,
}

impl SourceFixture {
    pub async fn new(simulated: bool) -> Self {
        Self::new_with_extra(simulated, None).await
    }

    pub async fn new_with_extra(simulated: bool, extra: Option<&str>) -> Self {
        let path =
            std::env::temp_dir().join(format!("awr-delivery-acceptance-{}", awr_core::Id::new()));
        std::fs::create_dir_all(path.join("docs")).unwrap();
        let root = SourceRoot(std::fs::canonicalize(path).unwrap());
        std::fs::write(
            root.0.join("docs/spec.md"),
            "# Contract\nVerify the reviewed package.\n",
        )
        .unwrap();
        let policy = if simulated {
            ExecutionSettlementPolicy::SIMULATED_MEMBER_COMPLETION_POLICY
        } else {
            POLICY
        };
        std::fs::write(
            root.0.join("ledger.yaml"),
            format!(
                r#"# Preserve this comment and unrelated work.
workstreams:
  version: 1
  definitions:
    - id: 00000000000000000000000001
      external_key: alpha
      title: Alpha
      state: active
      authority_version: 2
      goal_keys: [alpha]
      acceptance_contracts: [docs/spec.md]
    - id: 00000000000000000000000002
      external_key: private-beta
      title: Private
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
    title: Reviewed package
    status: planned
    workstream: alpha
    goals: [alpha]
    acceptance: [verified]
    paths: [src/api]
    depends_on: []
    completion_policy: {policy}
    execution_settlement:
      mode: independent_workspace_v1
      workspace_id: synthetic-workspace-a
    verification_requirements: [report]
  - id: c
    title: Unrelated work
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
"#
            ),
        )
        .unwrap();
        let mut manifest = ArtifactManifest {
            entries: vec![ArtifactEntry {
                artifact_id: "package".into(),
                sha256: format!("{:x}", Sha256::digest(CONTENT)),
                byte_length: CONTENT.len().to_string(),
                locator: "git-path:src/api/result.json".into(),
            }],
        };
        if let Some(extra) = extra {
            manifest.entries.push(ArtifactEntry {
                artifact_id: "notes".into(),
                sha256: format!("{:x}", Sha256::digest(extra)),
                byte_length: extra.len().to_string(),
                locator: "artifact:notes.txt".into(),
            });
        }
        // Opaque neutral fixture identity; this test makes no repository-effect claim.
        let candidate: DeliveryCandidate = serde_json::from_value(json!({"binding":{
            "tenant_id":TENANT,"project_id":PROJECT,"scope_id":"main","workstream_id":awr_core::Id::from(1),
            "work_id":"a","candidate_id":"candidate-a","candidate_version":"1","contract_hash":"0".repeat(64),
            "manifest_digest":manifest.digest().unwrap(),"source_revision":{"resource":RESOURCE,"format":"git_sha256","value":"a".repeat(64)},
            "required_checks":["report"],"target":{"resource":RESOURCE,"reference":"main","precondition":{"kind":"missing"}}},"manifest":manifest})).unwrap();
        let checks = ["report".into()];
        let mut f = setup_source_integration_with_artifacts(
            candidate,
            &root.0,
            &checks,
            simulated,
            &extra.into_iter().collect::<Vec<_>>(),
        )
        .await;
        // Session provisioning is identity infrastructure; all execution,
        // evidence, business review and acceptance use their actual commands.
        f.admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
            VALUES($1,$2,'session-supervisor','main','a','supervisor','cli-supervisor','supervisor-conversation','active',$3,1)",
            &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
        f.approve_source_review().await;
        Self { f, root }
    }

    pub async fn finalize(&self, key: &str) -> Value {
        self.try_finalize(key).await.unwrap()["receipt"]["data"].clone()
    }

    pub async fn try_finalize(&self, key: &str) -> PgResult<Value> {
        let p = prepare(&self.f.reads, SUPERVISOR, "a").await;
        self.f
            .reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                SUPERVISOR,
                command(
                    &p,
                    key,
                    "delivery.finalize",
                    json!({"session_id":"session-supervisor","expected_session_version":"1",
                "evidence_id":self.f.evidence["evidence_id"],"context_complete":true}),
                ),
            )
            .await
    }

    pub async fn resubmit_reviewed_evidence(&mut self, mut payload: Value) -> PgResult<()> {
        payload["output_digest"] = json!(self.f.selection.candidate.manifest.entries[0].sha256);
        let execution: String = self
            .f
            .admin
            .query_one(
                "SELECT execution_id FROM awr_team.evidence WHERE id=$1",
                &[&self.f.evidence["evidence_id"].as_str().unwrap()],
            )
            .await
            .unwrap()
            .get(0);
        self.f.evidence = integration_fixture::run(&self.f.reads, A, "resubmit", "evidence.submit",
            json!({"session_id":"session-a","expected_session_version":"1","input_digest":INPUT,
                "execution_id":execution,"artifact_text":CONTENT,"dirty_tree":false,"payload":payload})).await;
        let p = prepare(&self.f.reads, A, "a").await;
        let response = self
            .f
            .reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &p,
                    "reopen",
                    "review.open",
                    json!({
                        "session_id":"session-a","expected_session_version":"1",
                        "evidence_id":self.f.evidence["evidence_id"]
                    }),
                ),
            )
            .await?;
        let round = &response["receipt"]["data"];
        self.f.request.evidence_id = self.f.evidence["evidence_id"].as_str().unwrap().into();
        self.f.request.review_round_id = round["round_id"].as_str().unwrap().into();
        self.f.request.review_decision_id.clear();
        self.f
            .approve_source_review_with_key("approve-resubmission")
            .await;
        Ok(())
    }

    pub async fn assert_no_completion(&self) {
        assert_eq!(
            self.f
                .admin
                .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[],)
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
        assert_ne!(
            self.f
                .admin
                .query_one(
                    "SELECT state FROM awr_team.work_runtime WHERE work_id='a'",
                    &[],
                )
                .await
                .unwrap()
                .get::<_, String>(0),
            "completed"
        );
        for table in ["delivery_notifications", "delivery_sync_intents"] {
            assert_eq!(
                self.f
                    .admin
                    .query_one(&format!("SELECT count(*) FROM awr_team.{table}"), &[])
                    .await
                    .unwrap()
                    .get::<_, i64>(0),
                0,
                "Refused finalization must leave no queue records"
            );
        }
    }

    pub async fn publish_successor(&self, changed_work: Option<&str>) -> CurrentWorkstreamSource {
        integration_fixture::run(
            &self.f.reads,
            A,
            "release-before-successor",
            "claim.release",
            json!({"session_id":"session-a","expected_session_version":"1",
                "claim_id":self.f.selection.claim_id,"expected_fence":self.f.selection.fence,
                "expected_lease_version":self.f.selection.lease_version}),
        )
        .await;
        let path = self.root.0.join("ledger.yaml");
        let text = std::fs::read_to_string(&path).unwrap();
        let changed = match changed_work {
            Some("a") => text.replace("paths: [src/api]", "paths: [src/api, changed]"),
            Some("c") => text.replace("paths: [other]", "paths: [other, changed]"),
            None => format!("{text}\n# Unrelated source annotation.\n"),
            _ => panic!("unsupported fixture successor"),
        };
        assert_ne!(changed, text);
        std::fs::write(path, changed).unwrap();
        let package = awr_source::prepare_publish_from_server_directory(
            &self.root.0,
            "ledger.yaml",
            PROJECT,
            &awr_source::PublishPrepOptions::default(),
        )
        .unwrap();
        let store = SourceStore::from_config(self.f.config.clone());
        let (candidate, _) = store
            .ingest_publish_candidate(IngestRequest {
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
                actor_id: "supervisor".into(),
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
        store
            .approve(
                TENANT,
                PROJECT,
                &candidate.proposal_id,
                "integrator",
                &candidate.manifest_digest,
            )
            .await
            .unwrap();
        store
            .activate_workstreams(
                TENANT,
                PROJECT,
                "supervisor",
                &candidate.proposal_id,
                &awr_team::SourceActivationPlan {
                    candidate_digest: candidate.manifest_digest.clone(),
                    parser_version: candidate.parser_version,
                    expected_authority_epoch: candidate.base_epoch,
                    approved_candidate_digest: candidate.manifest_digest,
                },
            )
            .await
            .unwrap()
    }

    pub async fn queue(&self) -> Value {
        self.f
            .store
            .sync_intents(TENANT, PROJECT, WORKER, awr_core::Id::from(1), 64, None)
            .await
            .unwrap()
    }

    pub async fn take(&self, key: &str, kind: &str) -> DeliverySyncLease {
        let queue = self.queue().await;
        let row = queue["intents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["kind"] == kind && i["origin"] == "domain_acceptance")
            .unwrap();
        self.f
            .store
            .claim_sync_intent(
                TENANT,
                PROJECT,
                WORKER,
                ClaimDeliverySyncIntent {
                    request_id: key.into(),
                    read_set: serde_json::from_value(row["read_set"].clone()).unwrap(),
                    intent_id: row["intent_id"].as_str().unwrap().into(),
                    worker_id: "source-worker".into(),
                    expected_fence: row["fence"].as_str().unwrap().into(),
                    lease_seconds: 60,
                },
            )
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn request(&self, key: &str, receipt: &str) -> PrepareDeliverySourcePublication {
        let status = self
            .f
            .store
            .source_publication_status(TENANT, PROJECT, WORKER, "a")
            .await
            .unwrap();
        PrepareDeliverySourcePublication {
            request_id: key.into(),
            read_set: self.f.set.clone(),
            candidate_digest: self.f.request.candidate_digest.clone(),
            expected_selection_version: "1".into(),
            expected_metadata_revision: status["metadata_revision"].as_str().unwrap().into(),
            expected_source_fingerprint: status["confirmed_fingerprint"].as_str().unwrap().into(),
            completion_receipt_id: Some(receipt.into()),
            lease_seconds: 60,
        }
    }

    pub fn step(&self, key: &str, publication: &Value) -> DeliveryPublicationStep {
        DeliveryPublicationStep {
            request_id: key.into(),
            read_set: self.f.set.clone(),
            publication_id: publication["publication_id"].as_str().unwrap().into(),
            fence: publication["fence"].as_str().unwrap().into(),
        }
    }
}
