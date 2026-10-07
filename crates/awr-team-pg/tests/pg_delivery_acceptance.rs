#![cfg(feature = "pg-tests")]
//! Actual source/command/publication regressions; no native or repository-effect credit.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/delivery_integration.rs"]
mod integration_fixture;

use awr_team::ExecutionSettlementPolicy;
use awr_team::delivery::*;
use awr_team_pg::*;
use fixture::*;
use integration_fixture::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

struct SourceRoot(PathBuf);
impl Drop for SourceRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct SourceFixture {
    f: Fixture,
    root: SourceRoot,
}

impl SourceFixture {
    async fn new(simulated: bool) -> Self {
        Self::new_with_extra(simulated, None).await
    }

    async fn new_with_extra(simulated: bool, extra: Option<&str>) -> Self {
        let path =
            std::env::temp_dir().join(format!("awr-delivery-acceptance-{}", ulid::Ulid::new()));
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
        let mut f = if simulated {
            setup_source_simulated_member_integration(candidate, &root.0, &checks).await
        } else {
            setup_source_integration_with_checks(candidate, &root.0, &checks).await
        };
        // Session provisioning is identity infrastructure; all execution,
        // evidence, business review and acceptance use their actual commands.
        f.admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
            VALUES($1,$2,'session-supervisor','main','a','supervisor','cli-supervisor','supervisor-conversation','active',$3,1)",
            &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
        f.approve_source_review().await;
        Self { f, root }
    }

    async fn finalize(&self, key: &str) -> Value {
        self.try_finalize(key).await.unwrap()["receipt"]["data"].clone()
    }

    async fn try_finalize(&self, key: &str) -> PgResult<Value> {
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

    async fn resubmit_reviewed_evidence(&mut self, mut payload: Value) {
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
        let round = integration_fixture::run(
            &self.f.reads,
            A,
            "reopen",
            "review.open",
            json!({"session_id":"session-a","expected_session_version":"1",
                "evidence_id":self.f.evidence["evidence_id"]}),
        )
        .await;
        self.f.request.evidence_id = self.f.evidence["evidence_id"].as_str().unwrap().into();
        self.f.request.review_round_id = round["round_id"].as_str().unwrap().into();
        self.f.request.review_decision_id.clear();
        self.f
            .approve_source_review_with_key("approve-resubmission")
            .await;
    }

    async fn assert_no_completion(&self) {
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
    }

    async fn request(&self, key: &str, receipt: &str) -> PrepareDeliverySourcePublication {
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

    fn step(&self, key: &str, publication: &Value) -> DeliveryPublicationStep {
        DeliveryPublicationStep {
            request_id: key.into(),
            read_set: self.f.set.clone(),
            publication_id: publication["publication_id"].as_str().unwrap().into(),
            fence: publication["fence"].as_str().unwrap().into(),
        }
    }
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
}

#[tokio::test]
async fn publication_rechecks_actual_receipt_artifact_execution_and_current_binding() {
    let f = SourceFixture::new(false).await;
    let before = std::fs::read(f.root.0.join("ledger.yaml")).unwrap();
    let completed = f.finalize("finalize").await;
    let receipt = completed["receipt_id"].as_str().unwrap();
    let request = f.request("prepare", receipt).await;
    let artifact = f.f.evidence["artifact_id"].as_str().unwrap();
    let mutations = [
        "UPDATE awr_team.completion_receipts SET delivery_candidate_digest=NULL",
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
