#![allow(dead_code)]
//! Separate simulated members share a controller, not an identity or credential.
use super::*;
use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
};
use awr_team::{ExecutionSettlementMode, ExecutionSettlementPolicy, WorkContract};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::MutexGuard};
use tokio_postgres::Client;

pub const SUPERVISOR: &str =
    "awr1.integration-supervisor.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
pub const REVIEWER: &str =
    "awr1.integration-reviewer.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
pub const WORKER: &str =
    "awr1.integration-worker.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
pub const RESOURCE: &str = "fixture://integration-repository";
pub const INPUT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const CONTENT: &str = "Reviewed package bytes";
pub const POLICY: &str = "caller_managed_execution_and_agent_review";

pub struct Fixture {
    pub _guard: MutexGuard<'static, ()>,
    pub admin: Client,
    pub config: tokio_postgres::Config,
    pub reads: WorkstreamReadStore,
    pub store: DeliverySyncStore,
    pub set: DeliveryReadSet,
    pub selection: SelectDeliveryCandidate,
    pub request: PrepareDeliveryIntegration,
    pub evidence: Value,
}

pub async fn run(
    store: &WorkstreamReadStore,
    token: &str,
    key: &str,
    op: &str,
    args: Value,
) -> Value {
    let p = prepare(store, token, "a").await;
    store
        .commands()
        .execute(TENANT, PROJECT, token, command(&p, key, op, args))
        .await
        .unwrap()["receipt"]["data"]
        .clone()
}

pub fn set(p: &Value) -> DeliveryReadSet {
    serde_json::from_value(json!({"work_id":"a","workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],
        "authority_version":p["authority_version"],"ownership_version":p["data"]["ownership_version"],
        "contract_hash":p["data"]["contract_hash"]})).unwrap()
}

pub async fn setup_integration() -> Fixture {
    setup_integration_with_candidate(None).await
}

/// Bind an actual repository candidate before submitting evidence or approval.
pub async fn setup_integration_with_candidate(actual: Option<DeliveryCandidate>) -> Fixture {
    let (guard, admin, db, reads) = setup().await;
    let config = common::with_app_role(&common::test_config(), &db);
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id IN ('agent','reviewer');
        UPDATE awr_team.project_memberships SET role='developer' WHERE actor_id IN ('agent','reviewer');
        UPDATE awr_team.project_memberships SET agent_review=true WHERE actor_id='reviewer';
        UPDATE awr_team.workstream_grants SET can_write=true WHERE client_id='cli-a';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
          ('reader-tenant','supervisor','agent','Supervisor','active'),('reader-tenant','integrator','system','Integrator','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role,business_roles) VALUES
          ('reader-tenant','reader-project','supervisor','maintainer','[\"deliverer\"]'),
          ('reader-tenant','reader-project','integrator','admin',NULL);").await.unwrap();
    for (id, actor, client, token) in [
        (
            "integration-supervisor",
            "supervisor",
            "cli-supervisor",
            SUPERVISOR,
        ),
        ("integration-reviewer", "reviewer", "cli-reviewer", REVIEWER),
        ("integration-worker", "integrator", "cli-worker", WORKER),
    ] {
        admin.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,$2,$3,$4,$5)",
            &[&TENANT,&id,&actor,&client,&workstream_credential_hash(token).unwrap()]).await.unwrap();
        admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage)
            VALUES($1,$2,$3,$4,$5,1,true,true,$6)",
            &[&TENANT,&PROJECT,&actor,&client,&awr_core::Id::from(1).to_string(),&(actor=="integrator")]).await.unwrap();
    }
    for (actor, client, member, actions) in [
        (
            "agent",
            "cli-a",
            "member-a",
            vec![
                AuthorizedAction::Inspect,
                AuthorizedAction::StartWork,
                AuthorizedAction::ClaimCoordination,
            ],
        ),
        (
            "reviewer",
            "cli-reviewer",
            "member-b",
            vec![
                AuthorizedAction::Inspect,
                AuthorizedAction::StartWork,
                AuthorizedAction::Review,
            ],
        ),
        (
            "supervisor",
            "cli-supervisor",
            "member-supervisor",
            vec![
                AuthorizedAction::Inspect,
                AuthorizedAction::FinalizeDelivery,
            ],
        ),
    ] {
        let binding = format!("bind-{actor}");
        admin.execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity)
            VALUES($1,$2,$3,$3,'active','{\"kind\":\"simulated_member\",\"controller_ref\":\"shared-test-controller\"}')",
            &[&TENANT,&PROJECT,&member]).await.unwrap();
        admin.execute("INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
            VALUES($1,$2,$3,$4,$5,'active')",&[&TENANT,&PROJECT,&binding,&member,&actor]).await.unwrap();
        AuthorizationStore::from_config(config.clone())
            .issue(
                TENANT,
                PROJECT,
                &IssueAuthorizationRequest {
                    request_key: format!("grant-{actor}"),
                    authorization: AgentAuthorization {
                        id: format!("grant-{actor}"),
                        authorizer_person_id: PersonId::new(member).unwrap(),
                        responsible_person_id: PersonId::new(member).unwrap(),
                        subject_kind: ExecutionSubjectKind::Agent,
                        subject_id: actor.into(),
                        client_id: client.into(),
                        session_id: None,
                        model_id: None,
                        scope: AuthorizationScope::Workstream {
                            project_id: PROJECT.into(),
                            workstream_id: awr_core::Id::from(1).to_string(),
                        },
                        actions: actions.into_iter().collect::<BTreeSet<_>>(),
                        expires_at_ms: None,
                        status: AuthorizationStatus::Active,
                        revoked_at_ms: None,
                        revoked_by: None,
                        verifiable_capabilities: vec![],
                        self_reported_skill_hints: vec![],
                        parent_authorization_id: None,
                        maintainer_person_id: None,
                        created_at_ms: 1000,
                        binding_id: Some(binding),
                    },
                },
            )
            .await
            .unwrap();
    }
    admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        VALUES($1,$2,'session-reviewer','main','a','reviewer','cli-reviewer','reviewer-conversation','active',$3,1)",
        &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let value: Value = admin
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let mut contract: WorkContract = serde_json::from_value(value).unwrap();
    contract.codec = WorkContract::CODEC_V3.into();
    contract.completion_policy = POLICY.into();
    contract.execution_settlement = Some(ExecutionSettlementPolicy {
        mode: ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "synthetic-workspace-a".into(),
    });
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='a'",
        &[&json!(contract),&contract.hash().unwrap()]).await.unwrap();
    let claim = run(
        &reads,
        A,
        "take",
        "task.claim_available",
        json!({"session_id":"session-a","expected_session_version":"1",
        "expected_responsibility_version":"0","expected_work_version":"0","ttl_seconds":3600}),
    )
    .await;
    let p = prepare(&reads, A, "a").await;
    let set = set(&p);
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: "package".into(),
            sha256: format!("{:x}", Sha256::digest(CONTENT)),
            byte_length: CONTENT.len().to_string(),
            locator: "git-path:src/api/result.json".into(),
        }],
    };
    let mut candidate: DeliveryCandidate = serde_json::from_value(json!({"binding":{
        "tenant_id":TENANT,"project_id":PROJECT,"scope_id":"main","workstream_id":set.workstream_id,"work_id":"a",
        "candidate_id":"candidate-a","candidate_version":"1","contract_hash":set.contract_hash,
        "manifest_digest":manifest.digest().unwrap(),"source_revision":{"resource":RESOURCE,"format":"git_sha256","value":"a".repeat(64)},
        "required_checks":["report"],"target":{"resource":RESOURCE,"reference":"main","precondition":{"kind":"missing"}}},"manifest":manifest})).unwrap();
    if let Some(mut supplied) = actual {
        supplied.binding.tenant_id = awr_team::TenantId::new(TENANT).unwrap();
        supplied.binding.project_id = awr_team::ProjectId::new(PROJECT).unwrap();
        supplied.binding.scope_id = awr_team::ScopeId::new("main").unwrap();
        supplied.binding.work_id = awr_team::WorkId::new("a").unwrap();
        supplied.binding.workstream_id = set.workstream_id.to_string();
        supplied.binding.contract_hash = set.contract_hash.clone();
        supplied.binding.required_checks = contract.verification_requirements.clone();
        candidate = supplied;
    }
    let selection = SelectDeliveryCandidate {
        request_id: "select".into(),
        read_set: set.clone(),
        expected_selected_digest: None,
        session_id: "session-a".into(),
        claim_id: claim["claim_id"].as_str().unwrap().into(),
        fence: claim["fence"].as_str().unwrap().into(),
        lease_version: claim["lease_version"].as_str().unwrap().into(),
        candidate,
    };
    let store = DeliverySyncStore::from_config(config.clone());
    store
        .select_candidate(TENANT, PROJECT, A, selection.clone())
        .await
        .unwrap();
    let candidate_digest = selection.candidate.binding.digest().unwrap();
    store
        .configure_connector(
            TENANT,
            PROJECT,
            WORKER,
            ConfigureDeliveryConnector {
                request_id: "configure".into(),
                read_set: set.clone(),
                expected_connector_version: "0".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "git".into(),
                    provider: "reference".into(),
                    resource: selection.candidate.binding.target.resource.clone(),
                    principal_actor_id: "integrator".into(),
                    principal_client_id: "cli-worker".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let execution=run(&reads,A,"prepare-execution","execution.prepare",json!({"session_id":"session-a","expected_session_version":"1",
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":prepare(&reads,A,"a").await["data"]["runtime"]["work_version"],"input_digest":INPUT,"declared_scope":["src/api"]})).await;
    let started=run(&reads,A,"start-execution","execution.start",json!({"session_id":"session-a","expected_session_version":"1",
        "execution_id":execution["execution_id"],"expected_execution_version":execution["execution_version"],"claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":prepare(&reads,A,"a").await["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"})).await;
    let result = &selection.candidate.manifest.entries[0].sha256;
    run(&reads,A,"report-execution","execution.report",json!({"session_id":"session-a","expected_session_version":"1",
        "execution_id":execution["execution_id"],"expected_execution_version":started["execution_version"],"outcome":"succeeded",
        "output_digest":result,"observed_paths":["src/api/result.json"],"note":"Completed independent fixture workspace.",
        "workspace_settlement":{"workspace_id":"synthetic-workspace-a","input_digest":INPUT,"environment_digest":"c".repeat(64),
            "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],"executor_stopped":true,"no_external_effects":true}})).await;
    let evidence=run(&reads,A,"submit-evidence","evidence.submit",json!({"session_id":"session-a","expected_session_version":"1",
        "artifact_text":CONTENT,"input_digest":INPUT,"execution_id":execution["execution_id"],"dirty_tree":false,
        "payload":{"passed":true,"output_digest":result,"delivery_candidate_digest":candidate_digest}})).await;
    let round=run(&reads,A,"open-review","review.open",json!({"session_id":"session-a","expected_session_version":"1","evidence_id":evidence["evidence_id"]})).await;
    run(&reads,REVIEWER,"approve-review","review.decide",json!({"session_id":"session-reviewer","expected_session_version":"1",
        "round_id":round["round_id"],"decision":"approve","reason":"Verified the exact package and candidate binding."})).await;
    let decision: String = admin
        .query_one(
            "SELECT id FROM awr_team.review_decisions WHERE review_round_id=$1",
            &[&round["round_id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    let request = PrepareDeliveryIntegration {
        request_id: "integration-prepare".into(),
        read_set: set.clone(),
        connector_id: "git".into(),
        connector_version: "1".into(),
        candidate_digest,
        selection_version: "1".into(),
        evidence_id: evidence["evidence_id"].as_str().unwrap().into(),
        review_round_id: round["round_id"].as_str().unwrap().into(),
        review_decision_id: decision,
        operation: IntegrationOperation::FastForward,
    };
    let f = Fixture {
        _guard: guard,
        admin,
        config,
        reads,
        store,
        set,
        selection,
        request,
        evidence,
    };
    f.check("initial-check", VerificationOutcome::Passed).await;
    f
}

impl Fixture {
    pub async fn ingest_record(&self, key: &str, record: DeliveryRecord) -> String {
        let inspection = self
            .store
            .reserve_inspection(
                TENANT,
                PROJECT,
                WORKER,
                ReserveDeliveryInspection {
                    request_id: format!("reserve-{key}"),
                    read_set: self.set.clone(),
                    connector_id: "git".into(),
                    connector_version: "1".into(),
                    candidate_digest: self.request.candidate_digest.clone(),
                    lease_seconds: 60,
                },
            )
            .await
            .unwrap();
        let receipt = self
            .store
            .ingest_facts(
                TENANT,
                PROJECT,
                WORKER,
                IngestDeliveryFacts {
                    request_id: format!("ingest-{key}"),
                    read_set: self.set.clone(),
                    connector_id: "git".into(),
                    inspection_id: inspection["data"]["inspection_id"].as_str().unwrap().into(),
                    event_id: key.into(),
                    records: vec![DeliveryEnvelope {
                        protocol: DELIVERY_PROTOCOL.into(),
                        protocol_version: DELIVERY_PROTOCOL_VERSION,
                        record,
                    }],
                },
            )
            .await
            .unwrap();
        receipt["data"]["observation_receipt"]["fact_ids"][0]
            .as_str()
            .unwrap()
            .into()
    }
    pub async fn check(&self, key: &str, outcome: VerificationOutcome) -> String {
        self.ingest_record(
            key,
            DeliveryRecord::Verification(VerificationRun {
                binding: self.selection.candidate.binding.clone(),
                run_id: key.into(),
                check: "report".into(),
                result_artifact: (outcome == VerificationOutcome::Passed)
                    .then(|| self.selection.candidate.manifest.entries[0].clone()),
                outcome,
                provenance: provenance(key),
            }),
        )
        .await
    }
    pub async fn prepared(&self) -> Value {
        self.store
            .prepare_integration(TENANT, PROJECT, SUPERVISOR, self.request.clone())
            .await
            .unwrap()["data"]
            .clone()
    }
    pub fn lease_request(&self, key: &str, id: &str) -> LeaseDeliveryIntegration {
        LeaseDeliveryIntegration {
            request_id: key.into(),
            read_set: self.set.clone(),
            integration_id: id.into(),
            lease_seconds: 60,
        }
    }
    pub async fn leased(&self, id: &str) -> Value {
        self.store
            .lease_integration(TENANT, PROJECT, WORKER, self.lease_request("lease", id))
            .await
            .unwrap()["data"]
            .clone()
    }
    pub fn dispatch_request(
        &self,
        key: &str,
        id: &str,
        lease: &str,
    ) -> DispatchDeliveryIntegration {
        DispatchDeliveryIntegration {
            request_id: key.into(),
            read_set: self.set.clone(),
            integration_id: id.into(),
            lease_id: lease.into(),
        }
    }
    pub async fn dispatched(&self, id: &str, lease: &str) -> DeliveryIntegrationDispatch {
        self.store
            .dispatch_integration(
                TENANT,
                PROJECT,
                WORKER,
                self.dispatch_request("dispatch", id, lease),
            )
            .await
            .unwrap()
    }
    pub fn observation(&self, id: &str, outcome: IntegrationOutcome) -> DeliveryRecord {
        let applied = outcome == IntegrationOutcome::Applied;
        DeliveryRecord::IntegrationObservation(IntegrationObservation {
            binding: self.selection.candidate.binding.clone(),
            request_id: Some(awr_team::RequestId::new(id).unwrap()),
            external_reference: format!("fixture://attempt/{id}"),
            outcome,
            result_revision: applied.then(|| {
                self.selection
                    .candidate
                    .binding
                    .source_revision
                    .clone()
                    .unwrap()
            }),
            contains_manifest_digest: applied
                .then(|| self.selection.candidate.binding.manifest_digest.clone()),
            provenance: provenance("effect"),
        })
    }
    pub fn confirm_request(&self, key: &str, id: &str, fact: &str) -> ConfirmDeliveryIntegration {
        ConfirmDeliveryIntegration {
            request_id: key.into(),
            read_set: self.set.clone(),
            integration_id: id.into(),
            fact_id: fact.into(),
        }
    }
    pub async fn guards(&self) -> i64 {
        self.admin
            .query_one(
                "SELECT count(*) FROM awr_team.delivery_integration_target_guards",
                &[],
            )
            .await
            .unwrap()
            .get(0)
    }
}

fn provenance(reference: &str) -> FactProvenance {
    FactProvenance {
        source: FactSource::AdapterObservation,
        reference: format!("fixture://{reference}"),
        observed_at_unix_ms: None,
        recorded_at_unix_ms: 1,
    }
}
