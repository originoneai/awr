#![cfg(feature = "pg-tests")]
mod common;
use awr_core::*;
use awr_team_pg::{DeliveryAdoptionStore, PgError};
use common::{app_client, fresh_team_schema, test_config, with_app_role};
use serde_json::{Value, json};
use std::sync::MutexGuard;
use std::time::Duration;
use tokio_postgres::Client;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (
    MutexGuard<'static, ()>,
    DeliveryAdoptionStore,
    String,
    Client,
) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');",
        )
        .await
        .unwrap();
    let cfg = with_app_role(&test_config(), &db);
    (guard, DeliveryAdoptionStore::from_config(cfg), db, admin)
}

fn digest(n: u8) -> String {
    format!("{n:064x}")
}
fn source_sha(n: u8) -> String {
    format!("{n:040x}")
}

#[tokio::test]
async fn reused_registration_key_rejects_changed_payload() {
    let (_guard, store, _, _) = setup().await;
    let requirement = requirement();
    let request = RegisterHardDependencyRequest {
        request_key: "same-request".into(),
        dependency_id: "original-dependency".into(),
        provider: requirement.provider,
        consumer: requirement.consumer,
        selected: requirement.selected,
        policy: requirement.policy,
        minimum_level: requirement.minimum_level,
        now_ms: 10,
    };
    store
        .register_dependency(TENANT, PROJECT, &request)
        .await
        .unwrap();
    let changed = RegisterHardDependencyRequest {
        dependency_id: "different-dependency".into(),
        ..request
    };
    assert!(
        matches!(
            store.register_dependency(TENANT, PROJECT, &changed).await,
            Err(PgError::IdempotencyConflict)
        ),
        "A reused request key must reject a different dependency instead of returning the original success"
    );
}

fn requirement() -> DeliveryRequirement {
    DeliveryRequirement {
        provider: WorkstreamWorkBinding {
            project_id: PROJECT.into(),
            work_item_id: "upstream".into(),
            workstream_id: Id::from(1),
        },
        consumer: WorkstreamWorkBinding {
            project_id: PROJECT.into(),
            work_item_id: "downstream".into(),
            workstream_id: Id::from(2),
        },
        selected: DeliveryVersion {
            completion_receipt: Id::from(3),
            contract_sha256: digest(0xa),
            artifact_sha256: digest(0xb),
            source_sha: source_sha(0xc),
            environment: "candidate-v1".into(),
            acceptance_round: "round-1".into(),
            export_scope_sha256: digest(0xd),
        },
        policy: DeliveryVersionPolicy::FixedDelivery,
        minimum_level: EvidenceLevel::LocallyVerified,
    }
}

fn proof(req: &DeliveryRequirement) -> CompletionAcceptanceProof {
    CompletionAcceptanceProof {
        completion_receipt_id: req.selected.completion_receipt,
        work_item_id: req.provider.work_item_id.clone(),
        contract_sha256: req.selected.contract_sha256.clone(),
        artifact_sha256: req.selected.artifact_sha256.clone(),
        independence_kind: "team_independent".into(),
        team_independent_acceptance: true,
        author_person_id: "author".into(),
        reviewer_person_id: "reviewer".into(),
        evidence_id: Id::from(4),
        evidence_level: EvidenceLevel::LocallyVerified,
        verified_at_ms: 90,
    }
}

#[derive(Clone)]
enum WriteRequest {
    Register(RegisterHardDependencyRequest),
    Grant(GrantExportAuthorizationRequest),
    Adopt(AdoptDeliveryRequest),
    RevokeDependency(RevokeHardDependencyRequest),
    RevokeExport(RevokeExportAuthorizationRequest),
}

impl WriteRequest {
    async fn run(
        &self,
        store: &DeliveryAdoptionStore,
    ) -> std::result::Result<(Value, DeliveryCredentialReceipt), PgError> {
        let (result, receipt) = match self {
            Self::Register(r) => {
                let (v, receipt) = store.register_dependency(TENANT, PROJECT, r).await?;
                (serde_json::to_value(v).unwrap(), receipt)
            }
            Self::Grant(r) => {
                let (v, receipt) = store.grant_export(TENANT, PROJECT, r).await?;
                (serde_json::to_value(v).unwrap(), receipt)
            }
            Self::Adopt(r) => {
                let (v, receipt) = store.adopt(TENANT, PROJECT, r).await?;
                (serde_json::to_value(v).unwrap(), receipt)
            }
            Self::RevokeDependency(r) => {
                let (v, receipt) = store.revoke_dependency(TENANT, PROJECT, r).await?;
                (serde_json::to_value(v).unwrap(), receipt)
            }
            Self::RevokeExport(r) => {
                let (v, receipt) = store.revoke_export(TENANT, PROJECT, r).await?;
                (serde_json::to_value(v).unwrap(), receipt)
            }
        };
        Ok((result, receipt))
    }

    fn body(&self) -> Value {
        match self {
            Self::Register(r) => serde_json::to_value(r),
            Self::Grant(r) => serde_json::to_value(r),
            Self::Adopt(r) => serde_json::to_value(r),
            Self::RevokeDependency(r) => serde_json::to_value(r),
            Self::RevokeExport(r) => serde_json::to_value(r),
        }
        .unwrap()
    }

    fn changed(&self, path: &str, replacement: Value) -> Self {
        let mut body = self.body();
        *body.pointer_mut(path).expect("existing request field") = replacement;
        match self {
            Self::Register(_) => Self::Register(serde_json::from_value(body).unwrap()),
            Self::Grant(_) => Self::Grant(serde_json::from_value(body).unwrap()),
            Self::Adopt(_) => Self::Adopt(serde_json::from_value(body).unwrap()),
            Self::RevokeDependency(_) => {
                Self::RevokeDependency(serde_json::from_value(body).unwrap())
            }
            Self::RevokeExport(_) => Self::RevokeExport(serde_json::from_value(body).unwrap()),
        }
    }
}

// Each mutation is initially unused. Separate base dependency/export records
// support adoption and revocation without pre-creating those request receipts.
async fn write_requests(store: &DeliveryAdoptionStore, admin: &Client) -> Vec<WriteRequest> {
    let r = requirement();
    seed_ws018_completion(admin, &r).await;
    let register = RegisterHardDependencyRequest {
        request_key: "base-register".into(),
        dependency_id: "base-dependency".into(),
        provider: r.provider.clone(),
        consumer: r.consumer.clone(),
        selected: r.selected.clone(),
        policy: r.policy,
        minimum_level: r.minimum_level,
        now_ms: 10,
    };
    let (dep, _) = store
        .register_dependency(TENANT, PROJECT, &register)
        .await
        .unwrap();
    let grant = GrantExportAuthorizationRequest {
        request_key: "base-grant".into(),
        authorization_id: "base-export".into(),
        project_id: PROJECT.into(),
        provider_work_item_id: r.provider.work_item_id.clone(),
        delivery: r.selected.clone(),
        granted_by: "owner".into(),
        now_ms: 20,
    };
    let (export, _) = store.grant_export(TENANT, PROJECT, &grant).await.unwrap();
    vec![
        WriteRequest::Register(RegisterHardDependencyRequest {
            request_key: "new-register".into(),
            dependency_id: "new-dependency".into(),
            ..register
        }),
        WriteRequest::Grant(GrantExportAuthorizationRequest {
            request_key: "new-grant".into(),
            authorization_id: "new-export".into(),
            ..grant
        }),
        WriteRequest::Adopt(AdoptDeliveryRequest {
            request_key: "new-adopt".into(),
            credential_id: "new-credential".into(),
            dependency: dep,
            export_authorization: export,
            completion: proof(&r),
            availability: DeliveryAvailability::Available,
            current_selection: Some(r.selected),
            now_ms: 100,
        }),
        WriteRequest::RevokeDependency(RevokeHardDependencyRequest {
            request_key: "new-revoke-dependency".into(),
            dependency_id: "base-dependency".into(),
            expected_status: HardDependencyStatus::Active,
            now_ms: 200,
        }),
        WriteRequest::RevokeExport(RevokeExportAuthorizationRequest {
            request_key: "new-revoke-export".into(),
            authorization_id: "base-export".into(),
            reason: "disclosure withdrawn".into(),
            now_ms: 200,
        }),
    ]
}

async fn delivery_rows(admin: &Client) -> Vec<Value> {
    let mut result = Vec::new();
    for table in [
        "hard_delivery_dependencies",
        "export_authorizations",
        "adoption_credentials",
        "delivery_credential_receipts",
    ] {
        result.push(admin.query_one(&format!("SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb) FROM awr_team.{table} t"), &[])
            .await.unwrap().get(0));
    }
    result
}

// Replace every typed business leaf individually, including caller proof fields
// that are ignored when obtaining authoritative completion evidence.
fn changed_fields(path: &str, value: &Value, output: &mut Vec<(String, Value)>) {
    if let Value::Object(fields) = value {
        for (key, value) in fields {
            let nested = format!("{path}/{key}");
            if nested != "/request_key" {
                changed_fields(&nested, value, output);
            }
        }
        if path == "/current_selection" {
            output.push((path.into(), Value::Null));
        }
        return;
    }
    let key = path.rsplit('/').next().unwrap();
    let changed = match value {
        Value::Bool(v) => json!(!v),
        Value::Number(v) => json!(v.as_i64().unwrap() + 1),
        Value::Null if key == "revoked_at_ms" => json!(99),
        Value::Null => json!("changed"),
        Value::String(_) if key == "policy" => json!("current_contract"),
        Value::String(_) if key == "minimum_level" || key == "evidence_level" => {
            json!("real_environment_validated")
        }
        Value::String(_) if key == "status" || key == "expected_status" => json!("revoked"),
        Value::String(_) if key == "availability" => json!("unknown"),
        Value::String(v) if v.len() == 26 => serde_json::to_value(Id::from(999)).unwrap(),
        Value::String(v) => json!(format!("{v}-changed")),
        _ => panic!("unexpected request field {path}"),
    };
    assert_ne!(value, &changed);
    output.push((path.into(), changed));
}

#[tokio::test]
async fn every_business_field_and_operation_is_bound_to_the_original_request() {
    let (_guard, store, _, admin) = setup().await;
    let requests = write_requests(&store, &admin).await;
    for request in &requests {
        let (original, receipt) = request.run(&store).await.unwrap();
        assert!(!receipt.replayed);
        let before = delivery_rows(&admin).await;
        let mut fields = Vec::new();
        changed_fields("", &request.body(), &mut fields);
        for (path, value) in fields {
            assert!(
                matches!(
                    request.changed(&path, value).run(&store).await,
                    Err(PgError::IdempotencyConflict)
                ),
                "changed business field {path} must not replay another intent"
            );
        }
        let (recovered, replay) = request.run(&store).await.unwrap();
        assert_eq!(original, recovered);
        assert_eq!(receipt.event_id, replay.event_id);
        assert!(replay.replayed);
        assert_eq!(before, delivery_rows(&admin).await);
    }
    let changed_op = requests[1].changed("/request_key", json!("new-register"));
    assert!(matches!(
        changed_op.run(&store).await,
        Err(PgError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn exact_replay_recovers_original_snapshots_after_live_records_change() {
    let (_guard, store, _, admin) = setup().await;
    let requests = write_requests(&store, &admin).await;
    let mut originals = Vec::new();
    for request in &requests[..3] {
        originals.push(request.run(&store).await.unwrap());
    }
    // Revoke the newly registered objects and change the credential's live
    // status. Recovery must neither fabricate a new history nor undo changes.
    let revoke_dep = requests[3].changed("/dependency_id", json!("new-dependency"));
    let revoke_export = requests[4].changed("/authorization_id", json!("new-export"));
    revoke_dep.run(&store).await.unwrap();
    revoke_export.run(&store).await.unwrap();
    admin.batch_execute("UPDATE awr_team.adoption_credentials SET status='revoked',body_json=jsonb_set(body_json,'{status}','\"revoked\"')").await.unwrap();
    let before = delivery_rows(&admin).await;
    for (request, (original, receipt)) in requests[..3].iter().zip(originals) {
        let (recovered, replay) = request.run(&store).await.unwrap();
        assert_eq!(original, recovered);
        assert_eq!(receipt.event_id, replay.event_id);
        assert!(replay.replayed);
    }
    assert_eq!(
        store
            .get_dependency(TENANT, PROJECT, "new-dependency")
            .await
            .unwrap()
            .unwrap()
            .status,
        HardDependencyStatus::Revoked
    );
    assert_eq!(
        store
            .get_export(TENANT, PROJECT, "new-export")
            .await
            .unwrap()
            .unwrap()
            .status,
        ExportAuthorizationStatus::Revoked
    );
    assert_eq!(
        store
            .get_credential(TENANT, PROJECT, "new-credential")
            .await
            .unwrap()
            .unwrap()
            .status,
        AdoptionCredentialStatus::Revoked
    );
    assert_eq!(before, delivery_rows(&admin).await);
}

#[tokio::test]
async fn concurrent_identical_requests_return_one_original_and_one_replay_for_each_write() {
    let (_guard, store, _, admin) = setup().await;
    for request in write_requests(&store, &admin).await {
        let (left, right) = tokio::join!(request.run(&store), request.run(&store));
        let (a, ar) = left.unwrap();
        let (b, br) = right.unwrap();
        assert_eq!(a, b);
        assert_eq!(ar.event_id, br.event_id);
        assert_ne!(ar.replayed, br.replayed);
    }
    assert_eq!(delivery_rows(&admin).await[3].as_array().unwrap().len(), 7);
}

#[tokio::test]
async fn concurrent_changed_requests_return_one_winner_and_typed_conflict_for_each_write() {
    let (_guard, store, _, admin) = setup().await;
    for request in write_requests(&store, &admin).await {
        let changed = request.changed(
            "/now_ms",
            json!(request.body()["now_ms"].as_i64().unwrap() + 1),
        );
        let (left, right) = tokio::join!(request.run(&store), changed.run(&store));
        let (winner, loser, winning_request) = if left.is_ok() {
            (left.unwrap(), right, &request)
        } else {
            (right.unwrap(), left, &changed)
        };
        assert!(!winner.1.replayed);
        assert!(matches!(loser, Err(PgError::IdempotencyConflict)));
        let recovered = winning_request.run(&store).await.unwrap();
        assert_eq!(winner.0, recovered.0);
        assert_eq!(winner.1.event_id, recovered.1.event_id);
    }
    assert_eq!(delivery_rows(&admin).await[3].as_array().unwrap().len(), 7);
}

async fn wait_for_project_lock(tx: &tokio_postgres::Transaction<'_>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            tx.batch_execute("SELECT pg_stat_clear_snapshot()")
                .await
                .unwrap();
            let count: i64 = tx
                .query_one(
                    "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND query LIKE 'SELECT status FROM awr_team.projects%'",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            if count > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("mutation waits on the common project row lock");
}

#[tokio::test]
async fn every_write_obeys_project_freeze_including_replay_after_waiting_on_the_common_lock() {
    let (_guard, store, _, mut admin) = setup().await;
    for request in write_requests(&store, &admin).await {
        let before = delivery_rows(&admin).await;
        let tx = admin.transaction().await.unwrap();
        tx.query_one(
            "SELECT status FROM awr_team.projects WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
        let mut pending = Box::pin(request.run(&store));
        tokio::select! {
            result = &mut pending => panic!("write bypassed held project lock: {result:?}"),
            () = wait_for_project_lock(&tx) => (),
        }
        tx.execute(
            "UPDATE awr_team.projects SET status='frozen' WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(matches!(pending.await, Err(PgError::ProjectNotAvailable)));
        assert_eq!(before, delivery_rows(&admin).await);
        admin
            .batch_execute("UPDATE awr_team.projects SET status='active'")
            .await
            .unwrap();
        request.run(&store).await.unwrap();
        admin
            .batch_execute("UPDATE awr_team.projects SET status='frozen'")
            .await
            .unwrap();
        assert!(matches!(
            request.run(&store).await,
            Err(PgError::ProjectNotAvailable)
        ));
        admin
            .batch_execute("UPDATE awr_team.projects SET status='active'")
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn receipt_failure_rolls_back_every_subject_mutation() {
    let (_guard, store, _, admin) = setup().await;
    let requests = write_requests(&store, &admin).await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_test_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture receipt failure'; END $$;
        CREATE TRIGGER reject_test_receipt BEFORE INSERT ON awr_team.delivery_credential_receipts FOR EACH ROW EXECUTE FUNCTION awr_team.reject_test_receipt()").await.unwrap();
    let before = delivery_rows(&admin).await;
    for request in &requests {
        assert!(request.run(&store).await.is_err());
        assert_eq!(before, delivery_rows(&admin).await);
    }
    admin
        .batch_execute("DROP TRIGGER reject_test_receipt ON awr_team.delivery_credential_receipts")
        .await
        .unwrap();
    for request in &requests {
        assert!(!request.run(&store).await.unwrap().1.replayed);
    }
}

#[tokio::test]
async fn unbound_legacy_receipts_fail_closed_without_backfill_for_each_write() {
    let (_guard, store, _, admin) = setup().await;
    for request in write_requests(&store, &admin).await {
        let (_, receipt) = request.run(&store).await.unwrap();
        admin.execute("UPDATE awr_team.delivery_credential_receipts SET request_hash=NULL,result_json=NULL WHERE request_key=$1", &[&receipt.request_key]).await.unwrap();
        let before = delivery_rows(&admin).await;
        let error = request.run(&store).await.unwrap_err();
        assert!(
            matches!(error, PgError::Protocol(ref message) if message.contains("legacy delivery receipt") && message.contains("inspect current"))
        );
        assert_eq!(before, delivery_rows(&admin).await);
    }
}

#[tokio::test]
async fn dependency_revoke_and_adopt_race_has_one_serialized_outcome() {
    revoke_and_adopt_race(3).await;
}

#[tokio::test]
async fn export_revoke_and_adopt_race_has_one_serialized_outcome() {
    revoke_and_adopt_race(4).await;
}

async fn revoke_and_adopt_race(revoke_index: usize) {
    let (_guard, store, _, admin) = setup().await;
    let requests = write_requests(&store, &admin).await;
    let (adoption, revoked) =
        tokio::join!(requests[2].run(&store), requests[revoke_index].run(&store));
    let (revoked_subject, receipt) = revoked.unwrap();
    assert!(!receipt.replayed);
    assert_eq!(revoked_subject["status"], "revoked");
    let before = delivery_rows(&admin).await;
    match adoption {
        Ok((original, receipt)) => {
            let (recovered, replay) = requests[2].run(&store).await.unwrap();
            assert_eq!(original, recovered);
            assert_eq!(receipt.event_id, replay.event_id);
            assert!(replay.replayed);
        }
        Err(PgError::Protocol(message)) => {
            assert!(message.contains("snapshot"));
            assert!(
                store
                    .get_credential(TENANT, PROJECT, "new-credential")
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        Err(error) => panic!("unexpected concurrent result: {error}"),
    }
    let fresh_adoption = requests[2].changed("/request_key", json!("after-revoke"));
    assert!(fresh_adoption.run(&store).await.is_err());
    assert_eq!(before, delivery_rows(&admin).await);
}

/// Seed a currently-selected WS-018 completion with evidence + approved review.
async fn seed_ws018_completion(admin: &Client, req: &DeliveryRequirement) {
    let receipt_id = req.selected.completion_receipt.to_string();
    let evidence_id = Id::from(4).to_string();
    let contract = &req.selected.contract_sha256;
    let artifact = &req.selected.artifact_sha256;
    let bundle = digest(0xe);
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                VALUES ('tenant-a','project-a','main','Main','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
                ('tenant-a','project-a','upstream','UP'),
                ('tenant-a','project-a','downstream','DOWN')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('tenant-a','project-a','author','Author','active'),
                ('tenant-a','project-a','reviewer','Reviewer','active')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.evidence(
                tenant_id,project_id,id,work_id,contract_hash,output_digest,
                evidence_kind,trust_basis,digest,payload_json,created_by)
             VALUES ($1,$2,$3,'upstream',$4,$5,'artifact','trusted_executor',$6,'{}'::jsonb,'runner')",
            &[
                &TENANT,
                &PROJECT,
                &evidence_id,
                contract,
                artifact,
                &bundle,
            ],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.review_rounds(
                tenant_id,project_id,id,work_id,round_index,bundle_hash,contract_hash,
                author_actor_id,state,evidence_id)
             VALUES ($1,$2,'round-1','upstream',1,$3,$4,'author','approved',$5)",
            &[&TENANT, &PROJECT, &bundle, contract, &evidence_id],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.review_decisions(
                tenant_id,project_id,id,review_round_id,work_id,bundle_hash,
                reviewer_actor_id,decision,reason,reviewer_person_id,independence_kind)
             VALUES ($1,$2,'dec-1','round-1','upstream',$3,'reviewer','approve','ok',
                     'reviewer','team_independent')",
            &[&TENANT, &PROJECT, &bundle],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.completion_receipts(
                tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
                dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,
                independence_kind,evidence_id,approved_by_person_id,submitted_by_person_id,
                accepted_at)
             VALUES ($1,$2,$3,'upstream','main',$4,$5,'deps',$5,'review','{}'::jsonb,
                     'team_independent',$6,'reviewer','author',
                     to_timestamp(0.09))",
            &[
                &TENANT,
                &PROJECT,
                &receipt_id,
                contract,
                &bundle,
                &evidence_id,
            ],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.work_runtime(
                tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,
                selected_completion_id)
             VALUES ($1,$2,'main','upstream','completed',1,0,$3)",
            &[&TENANT, &PROJECT, &receipt_id],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn pg_adopt_refuses_without_ws018_completion() {
    let (_guard, store, _, _) = setup().await;
    let req = requirement();
    let (dep, _) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-empty".into(),
                dependency_id: "dep-empty".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();
    let (export, _) = store
        .grant_export(
            TENANT,
            PROJECT,
            &GrantExportAuthorizationRequest {
                request_key: "ex-empty".into(),
                authorization_id: "ea-empty".into(),
                project_id: PROJECT.into(),
                provider_work_item_id: req.provider.work_item_id.clone(),
                delivery: req.selected.clone(),
                granted_by: "owner".into(),
                now_ms: 20,
            },
        )
        .await
        .unwrap();

    let err = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-empty".into(),
                credential_id: "ac-empty".into(),
                dependency: dep,
                completion: proof(&req),
                export_authorization: export,
                availability: DeliveryAvailability::Available,
                current_selection: Some(req.selected.clone()),
                now_ms: 100,
            },
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("trusted WS-018 completion proof missing"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn pg_register_grant_adopt_and_replay() {
    let (_guard, store, _, admin) = setup().await;
    let req = requirement();
    seed_ws018_completion(&admin, &req).await;

    let (dep, receipt) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();
    assert!(!receipt.replayed);
    let (_, replay) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();
    assert!(replay.replayed);

    let (export, _) = store
        .grant_export(
            TENANT,
            PROJECT,
            &GrantExportAuthorizationRequest {
                request_key: "ex-1".into(),
                authorization_id: "ea-1".into(),
                project_id: PROJECT.into(),
                provider_work_item_id: req.provider.work_item_id.clone(),
                delivery: req.selected.clone(),
                granted_by: "owner".into(),
                now_ms: 20,
            },
        )
        .await
        .unwrap();

    // Request-supplied self-report fields must not matter: storage proof wins.
    let mut bad = proof(&req);
    bad.team_independent_acceptance = false;
    bad.independence_kind = "author_self_report".into();

    let original = AdoptDeliveryRequest {
        request_key: "ad-1".into(),
        credential_id: "ac-1".into(),
        dependency: dep,
        completion: bad,
        export_authorization: export,
        availability: DeliveryAvailability::Available,
        current_selection: Some(req.selected.clone()),
        now_ms: 100,
    };
    let (cred, receipt) = store.adopt(TENANT, PROJECT, &original).await.unwrap();
    assert_eq!(cred.status, AdoptionCredentialStatus::Active);
    assert_eq!(cred.acceptance_author, "author");
    assert_eq!(cred.acceptance_reviewer, "reviewer");
    let loaded = store
        .get_credential(TENANT, PROJECT, "ac-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.completion_receipt_id, cred.completion_receipt_id);

    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.completion_receipts
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);

    let (recovered, adopt_replay) = store.adopt(TENANT, PROJECT, &original).await.unwrap();
    assert!(adopt_replay.replayed);
    assert_eq!(recovered, cred);
    assert_eq!(adopt_replay.event_id, receipt.event_id);
    // Stored proof still wins on first execution, but a changed caller proof is
    // a different request and cannot masquerade as recovery of that result.
    let changed = AdoptDeliveryRequest {
        completion: proof(&req),
        ..original
    };
    assert!(matches!(
        store.adopt(TENANT, PROJECT, &changed).await,
        Err(PgError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn pg_rls_blocks_unscoped_and_cross_tenant_app_reads() {
    let (_guard, store, db, _) = setup().await;
    let req = requirement();
    store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();

    let app = app_client(&db).await;
    // No tenant context: FORCE RLS hides rows.
    let count: i64 = app
        .query_one(
            "SELECT count(*) FROM awr_team.hard_delivery_dependencies",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);

    // Wrong tenant still empty even if settings are forged for another tenant.
    app.batch_execute(
        "SELECT set_config('awr.tenant_id','tenant-b', false),
                set_config('awr.project_id','project-a', false)",
    )
    .await
    .unwrap();
    let count: i64 = app
        .query_one(
            "SELECT count(*) FROM awr_team.hard_delivery_dependencies",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);

    // Correct scope via store binding still returns the row.
    assert!(
        store
            .get_dependency(TENANT, PROJECT, "dep-1")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn pg_refuses_cross_project_consumer() {
    let (_guard, store, _, _) = setup().await;
    let mut req = requirement();
    req.consumer.project_id = "other".into();
    let err = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-x".into(),
                dependency_id: "dep-x".into(),
                provider: req.provider,
                consumer: req.consumer,
                selected: req.selected,
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("project") || msg.contains("binding"),
        "unexpected error: {msg}"
    );
}

async fn point_runtime_at_other_receipt(admin: &Client, req: &DeliveryRequirement) {
    let other = Id::from(9).to_string();
    let contract = &req.selected.contract_sha256;
    let bundle = digest(0x11);
    admin
        .execute(
            "INSERT INTO awr_team.completion_receipts(
                tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
                dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json)
             VALUES ($1,$2,$3,'upstream','main',$4,$5,'deps',$5,'review','{}'::jsonb)",
            &[&TENANT, &PROJECT, &other, contract, &bundle],
        )
        .await
        .unwrap();
    admin
        .execute(
            "UPDATE awr_team.work_runtime SET selected_completion_id=$3
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id='upstream'",
            &[&TENANT, &PROJECT, &other],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn pg_current_contract_refuses_drift_and_fixed_delivery_survives() {
    let (_guard, store, _, admin) = setup().await;
    let mut req = requirement();
    req.policy = DeliveryVersionPolicy::CurrentContract;
    seed_ws018_completion(&admin, &req).await;

    let (dep, _) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-cur".into(),
                dependency_id: "dep-cur".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();
    let (export, _) = store
        .grant_export(
            TENANT,
            PROJECT,
            &GrantExportAuthorizationRequest {
                request_key: "ex-cur".into(),
                authorization_id: "ea-cur".into(),
                project_id: PROJECT.into(),
                provider_work_item_id: req.provider.work_item_id.clone(),
                delivery: req.selected.clone(),
                granted_by: "owner".into(),
                now_ms: 20,
            },
        )
        .await
        .unwrap();
    let (cred, _) = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-cur".into(),
                credential_id: "ac-cur".into(),
                dependency: dep,
                completion: proof(&req),
                export_authorization: export,
                availability: DeliveryAvailability::Available,
                current_selection: None,
                now_ms: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(cred.status, AdoptionCredentialStatus::Active);

    point_runtime_at_other_receipt(&admin, &req).await;
    let drifted = store
        .get_dependency(TENANT, PROJECT, "dep-cur")
        .await
        .unwrap()
        .unwrap();
    let export = store
        .get_export(TENANT, PROJECT, "ea-cur")
        .await
        .unwrap()
        .unwrap();
    let err = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-cur-2".into(),
                credential_id: "ac-cur-2".into(),
                dependency: drifted,
                completion: proof(&req),
                export_authorization: export,
                availability: DeliveryAvailability::Available,
                current_selection: Some(req.selected.clone()),
                now_ms: 110,
            },
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("CurrentSelectionChanged"),
        "unexpected error: {msg}"
    );

    let fixed_req = requirement();
    let (fixed, _) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-fix".into(),
                dependency_id: "dep-fix".into(),
                provider: fixed_req.provider.clone(),
                consumer: fixed_req.consumer.clone(),
                selected: fixed_req.selected.clone(),
                policy: DeliveryVersionPolicy::FixedDelivery,
                minimum_level: fixed_req.minimum_level,
                now_ms: 12,
            },
        )
        .await
        .unwrap();
    let (fixed_export, _) = store
        .grant_export(
            TENANT,
            PROJECT,
            &GrantExportAuthorizationRequest {
                request_key: "ex-fix".into(),
                authorization_id: "ea-fix".into(),
                project_id: PROJECT.into(),
                provider_work_item_id: fixed_req.provider.work_item_id.clone(),
                delivery: fixed_req.selected.clone(),
                granted_by: "owner".into(),
                now_ms: 22,
            },
        )
        .await
        .unwrap();
    let (fixed_cred, _) = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-fix".into(),
                credential_id: "ac-fix".into(),
                dependency: fixed,
                completion: proof(&fixed_req),
                export_authorization: fixed_export,
                availability: DeliveryAvailability::Unavailable,
                current_selection: None,
                now_ms: 120,
            },
        )
        .await
        .unwrap();
    assert_eq!(fixed_cred.status, AdoptionCredentialStatus::Active);
    assert_eq!(
        fixed_cred.completion_receipt_id,
        fixed_req.selected.completion_receipt
    );
}

#[tokio::test]
async fn pg_adopt_refuses_approved_round_without_independent_decision() {
    let (_guard, store, _, admin) = setup().await;
    let req = requirement();
    seed_ws018_completion(&admin, &req).await;
    admin
        .execute(
            "DELETE FROM awr_team.review_decisions
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    let (dep, _) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-nd".into(),
                dependency_id: "dep-nd".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();
    let (export, _) = store
        .grant_export(
            TENANT,
            PROJECT,
            &GrantExportAuthorizationRequest {
                request_key: "ex-nd".into(),
                authorization_id: "ea-nd".into(),
                project_id: PROJECT.into(),
                provider_work_item_id: req.provider.work_item_id.clone(),
                delivery: req.selected.clone(),
                granted_by: "owner".into(),
                now_ms: 20,
            },
        )
        .await
        .unwrap();
    let err = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-nd".into(),
                credential_id: "ac-nd".into(),
                dependency: dep,
                completion: proof(&req),
                export_authorization: export,
                availability: DeliveryAvailability::Available,
                current_selection: Some(req.selected.clone()),
                now_ms: 100,
            },
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("independent-review binding missing"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn pg_refuses_reused_dependency_id() {
    let (_guard, store, _, _) = setup().await;
    let req = requirement();
    store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();
    let err = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-2".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider,
                consumer: req.consumer,
                selected: req.selected,
                policy: DeliveryVersionPolicy::CurrentContract,
                minimum_level: req.minimum_level,
                now_ms: 11,
            },
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("already registered"),
        "unexpected error: {msg}"
    );
    let kept = store
        .get_dependency(TENANT, PROJECT, "dep-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.policy, DeliveryVersionPolicy::FixedDelivery);
}
