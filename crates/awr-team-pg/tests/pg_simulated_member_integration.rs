#![cfg(feature = "pg-tests")]
//! Isolated mechanism regressions; no native-client or repository-effect credit.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/delivery_integration.rs"]
mod integration_fixture;

use awr_team::delivery::*;
use awr_team_pg::*;
use fixture::*;
use integration_fixture::*;
use serde_json::{Value, json};

fn finalization_args(f: &Fixture) -> Value {
    json!({"session_id":"session-supervisor","expected_session_version":"1",
        "evidence_id":f.evidence["evidence_id"],"context_complete":true})
}

async fn receipt_count(f: &Fixture) -> i64 {
    f.admin
        .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn authorized_agent_finalizes_once_with_original_basis_and_exact_replay() {
    let f = setup_simulated_member_integration().await;
    let p = prepare(&f.reads, SUPERVISOR, "a").await;
    let first = command(
        &p,
        "finalize-one",
        "delivery.finalize",
        finalization_args(&f),
    );
    let second = command(&p, "finalize-two", "work.complete", finalization_args(&f));
    let commands = f.reads.commands();
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, SUPERVISOR, first.clone()),
        commands.execute(TENANT, PROJECT, SUPERVISOR, second.clone())
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (success, replay_command) = match (a, b) {
        (Ok(value), Err(PgError::CompletionRejected)) => (value, first),
        (Err(PgError::CompletionRejected), Ok(value)) => (value, second),
        other => panic!("Unexpected finalization results: {other:?}"),
    };
    assert_eq!(receipt_count(&f).await, 1);
    let data = &success["receipt"]["data"];
    assert_eq!(data["task_complete"], true);
    assert_eq!(
        data["approval_basis"],
        "simulated_member_independent_review"
    );
    assert_eq!(data["execution_basis"], "caller_asserted_workspace_settled");
    assert_eq!(data["human_approval"], false);
    assert_eq!(data["team_independent_acceptance"], false);
    assert_eq!(
        data["member_review_basis"]["reviewer_member_id"],
        "member-b"
    );
    assert!(data["member_review_basis"].get("origins").is_none());
    let stored: Value = f
        .admin
        .query_one(
            "SELECT approved_by_json FROM awr_team.completion_receipts",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let original: Value = f
        .admin
        .query_one(
            "SELECT member_review_basis_json FROM awr_team.review_decisions WHERE id=$1",
            &[&f.request.review_decision_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored["member_review_basis"], original);
    assert_eq!(stored["review_decision_id"], f.request.review_decision_id);
    for field in [
        "approved_by_person_id",
        "executor_actor_id",
        "final_submitter_actor_id",
    ] {
        assert!(
            f.admin
                .batch_execute(&format!(
                    "UPDATE awr_team.completion_receipts SET {field}='replacement'"
                ))
                .await
                .is_err(),
            "Original receipt attribution cannot change: {field}"
        );
    }
    let replay = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, SUPERVISOR, replay_command)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], success["receipt"]);
    assert_eq!(receipt_count(&f).await, 1);
}

#[tokio::test]
async fn review_permission_and_revoked_finalize_delegation_cannot_complete_work() {
    let f = setup_simulated_member_integration().await;
    let p = prepare(&f.reads, REVIEWER, "a").await;
    let mut args = finalization_args(&f);
    args["session_id"] = json!("session-reviewer");
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                REVIEWER,
                command(&p, "review-only-finalize", "delivery.finalize", args)
            )
            .await,
        Err(PgError::Forbidden)
    ));
    let p = prepare(&f.reads, SUPERVISOR, "a").await;
    f.admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='grant-supervisor'",
        )
        .await
        .unwrap();
    // A previous preparation cannot restore revoked authority.
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                SUPERVISOR,
                command(
                    &p,
                    "revoked-finalize",
                    "delivery.finalize",
                    finalization_args(&f)
                )
            )
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(receipt_count(&f).await, 0);
    f.admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='active' WHERE id='grant-supervisor'",
        )
        .await
        .unwrap();
    integration_fixture::run(
        &f.reads,
        SUPERVISOR,
        "fresh-finalize",
        "delivery.finalize",
        finalization_args(&f),
    )
    .await;
    assert_eq!(receipt_count(&f).await, 1);
}

#[tokio::test]
async fn changed_bytes_length_settlement_or_review_refuse_finalization_without_side_effects() {
    for change in [
        "UPDATE awr_team.artifacts SET content='different'::bytea",
        "UPDATE awr_team.artifacts SET byte_length=byte_length+1",
        "UPDATE awr_team.executions SET workspace_effects_settled=false",
        "UPDATE awr_team.review_rounds SET state='invalidated'",
    ] {
        let f = setup_simulated_member_integration().await;
        f.admin.batch_execute(change).await.unwrap();
        let p = prepare(&f.reads, SUPERVISOR, "a").await;
        let result = f
            .reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                SUPERVISOR,
                command(
                    &p,
                    "changed-finalize",
                    "delivery.finalize",
                    finalization_args(&f),
                ),
            )
            .await;
        assert!(
            matches!(
                result,
                Err(PgError::EvidenceInvalid) | Err(PgError::ReviewRequired)
            ),
            "{change}: {result:?}"
        );
        assert_eq!(receipt_count(&f).await, 0);
    }
}

#[tokio::test]
async fn neutral_integration_retains_simulated_basis_and_restores_only_original_unknown_effect() {
    let f = setup_simulated_member_integration().await;
    let prepared = f.prepared().await;
    let id = prepared["integration_id"].as_str().unwrap();
    let eligibility: Value = f
        .admin
        .query_one(
            "SELECT eligibility_json FROM awr_team.delivery_integration_intents WHERE id=$1",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    let original: Value = f
        .admin
        .query_one(
            "SELECT member_review_basis_json FROM awr_team.review_decisions WHERE id=$1",
            &[&f.request.review_decision_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(eligibility["member_review_basis"], original);
    let lease = f.leased(id).await;
    let dispatch = f.dispatched(id, lease["lease_id"].as_str().unwrap()).await;
    assert!(dispatch.permit.is_some());
    drop(dispatch.permit);
    let rebuilt = DeliverySyncStore::from_config(f.config.clone());
    let replay = rebuilt
        .dispatch_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.dispatch_request("dispatch", id, lease["lease_id"].as_str().unwrap()),
        )
        .await
        .unwrap();
    assert!(replay.permit.is_none());
    let unknown = f
        .ingest_record(
            "unknown-effect",
            f.observation(id, IntegrationOutcome::Unknown),
        )
        .await;
    rebuilt
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("confirm-unknown", id, &unknown),
        )
        .await
        .unwrap();
    assert_eq!(f.guards().await, 1);
    let fact = f
        .ingest_record(
            "known-effect",
            f.observation(id, IntegrationOutcome::Applied),
        )
        .await;
    let confirmed = rebuilt
        .confirm_integration(
            TENANT,
            PROJECT,
            WORKER,
            f.confirm_request("confirm-known", id, &fact),
        )
        .await
        .unwrap();
    assert_eq!(confirmed["data"]["state"], "confirmed");
    assert_eq!(f.guards().await, 0);
    assert_eq!(receipt_count(&f).await, 0);
    assert_eq!(confirmed["acceptance_ready"], false);
}

#[tokio::test]
async fn prepared_simulated_integration_rechecks_approval_and_current_permissions() {
    for change in [
        "UPDATE awr_team.review_rounds SET state='invalidated'",
        "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='grant-supervisor'",
        "UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-supervisor'",
    ] {
        let f = setup_simulated_member_integration().await;
        let prepared = f.prepared().await;
        let id = prepared["integration_id"].as_str().unwrap();
        let lease = f.leased(id).await;
        f.admin.batch_execute(change).await.unwrap();
        assert!(
            f.store
                .dispatch_integration(
                    TENANT,
                    PROJECT,
                    WORKER,
                    f.dispatch_request("changed-dispatch", id, lease["lease_id"].as_str().unwrap())
                )
                .await
                .is_err(),
            "{change}"
        );
        let dispatched: i64 = f.admin.query_one("SELECT count(*) FROM awr_team.delivery_integration_intents WHERE state='dispatched'", &[])
            .await.unwrap().get(0);
        assert_eq!(dispatched, 0);
        assert_eq!(receipt_count(&f).await, 0);
    }
}

/// Publish a real V4 source successor from the complete current task contracts.
/// Do not restore the fixture's older V1 declaration or mutate the active pointer.
async fn publish_source_successor(f: &Fixture, changed_work: &str) -> CurrentWorkstreamSource {
    integration_fixture::run(
        &f.reads,
        A,
        "release-before-source",
        "claim.release",
        json!({
            "session_id":"session-a","expected_session_version":"1",
            "claim_id":f.selection.claim_id,"expected_fence":f.selection.fence,
            "expected_lease_version":f.selection.lease_version,
        }),
    )
    .await;
    let prepared = prepare(&f.reads, SUPERVISOR, "a").await;
    let snapshot = prepared["source_snapshot_id"].as_str().unwrap();
    let catalog: awr_core::WorkstreamCatalog = serde_json::from_value(
        f.admin
            .query_one(
                "SELECT catalog_json FROM awr_team.workstream_catalogs
                 WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
                &[&TENANT, &PROJECT, &snapshot],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    let mut contracts = Vec::new();
    for row in f
        .admin
        .query(
            "SELECT c.contract_json,o.workstream_id FROM awr_team.work_contracts c
         JOIN awr_team.workstream_snapshot_ownership o
           ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.snapshot_id=c.snapshot_id
           AND o.scope_id=c.scope_id AND o.work_id=c.work_id
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3 AND c.scope_id='main'
         ORDER BY c.work_id",
            &[&TENANT, &PROJECT, &snapshot],
        )
        .await
        .unwrap()
    {
        let mut contract: awr_team::WorkContract = serde_json::from_value(row.get(0)).unwrap();
        if contract.work_id.as_str() == changed_work {
            contract
                .acceptance
                .push("Verify the revised task requirement".into());
        }
        contracts.push(awr_team::WorkstreamContract {
            workstream_id: serde_json::from_value(json!(row.get::<_, String>(1))).unwrap(),
            contract,
        });
    }
    let bundle = awr_team::WorkstreamBundle {
        codec: awr_team::WorkstreamBundle::CODEC_V4.into(),
        catalog,
        contracts,
    };
    bundle.validate(PROJECT).unwrap();
    let source = SourceStore::from_config(f.config.clone());
    let candidate = source
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "supervisor".into(),
            parser_version: "workstreams/4".into(),
            files: vec![SourceFile {
                path: "workstreams.json".into(),
                bytes: serde_json::to_vec(&bundle).unwrap(),
            }],
        })
        .await
        .unwrap();
    source
        .approve(
            TENANT,
            PROJECT,
            &candidate.proposal_id,
            "integrator",
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    source
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

#[tokio::test]
async fn unrelated_source_publication_preserves_the_exact_original_simulated_review() {
    let f = setup_simulated_member_integration().await;
    let original = prepare(&f.reads, SUPERVISOR, "a").await;
    let published = publish_source_successor(&f, "c").await;
    assert_ne!(published.snapshot_id, original["source_snapshot_id"]);
    let current = prepare(&f.reads, SUPERVISOR, "a").await;
    assert_eq!(
        current["data"]["contract_hash"],
        original["data"]["contract_hash"]
    );
    let result = f
        .reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            SUPERVISOR,
            command(
                &current,
                "finalize-after-unrelated-publication",
                "delivery.finalize",
                finalization_args(&f),
            ),
        )
        .await
        .unwrap();
    assert_eq!(result["receipt"]["data"]["task_complete"], true);
    let stored: Value = f
        .admin
        .query_one(
            "SELECT approved_by_json FROM awr_team.completion_receipts",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        stored["member_review_basis"]["authority"]["snapshot_id"],
        original["source_snapshot_id"]
    );
    assert_eq!(receipt_count(&f).await, 1);
}

#[tokio::test]
async fn relevant_source_changes_cannot_reuse_a_previous_simulated_approval() {
    let f = setup_simulated_member_integration().await;
    let original = prepare(&f.reads, SUPERVISOR, "a").await;
    publish_source_successor(&f, "a").await;
    let current = prepare(&f.reads, SUPERVISOR, "a").await;
    assert_ne!(
        current["data"]["contract_hash"],
        original["data"]["contract_hash"]
    );
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                SUPERVISOR,
                command(
                    &current,
                    "reject-previous-contract",
                    "delivery.finalize",
                    finalization_args(&f)
                )
            )
            .await,
        Err(PgError::EvidenceInvalid)
    ));
    assert_eq!(receipt_count(&f).await, 0);
}

#[tokio::test]
async fn missing_or_corrupt_archived_contract_cannot_reconstruct_review_authority() {
    for missing in [true, false] {
        let f = setup_simulated_member_integration().await;
        let original = prepare(&f.reads, SUPERVISOR, "a").await;
        publish_source_successor(&f, "c").await;
        let snapshot = original["source_snapshot_id"].as_str().unwrap();
        if missing {
            // Simulate unavailable archived history through the test administrator;
            // normal application writers cannot delete source projections.
            assert_eq!(f.admin.execute(
                "DELETE FROM awr_team.workstream_snapshot_ownership
                 WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id='a'",
                &[&TENANT, &PROJECT, &snapshot],
            ).await.unwrap(), 1);
        }
        let sql = if missing {
            "DELETE FROM awr_team.work_contracts
             WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id='a'"
        } else {
            "UPDATE awr_team.work_contracts
             SET contract_json=jsonb_set(contract_json,'{acceptance}','[\"Altered archived requirement\"]'::jsonb)
             WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3 AND scope_id='main' AND work_id='a'"
        };
        assert_eq!(
            f.admin
                .execute(sql, &[&TENANT, &PROJECT, &snapshot])
                .await
                .unwrap(),
            1
        );
        let current = prepare(&f.reads, SUPERVISOR, "a").await;
        assert!(matches!(
            f.reads
                .commands()
                .execute(
                    TENANT,
                    PROJECT,
                    SUPERVISOR,
                    command(
                        &current,
                        "reject-unproven-history",
                        "delivery.finalize",
                        finalization_args(&f)
                    )
                )
                .await,
            Err(PgError::ReviewRequired)
        ));
        assert_eq!(receipt_count(&f).await, 0);
    }
}
