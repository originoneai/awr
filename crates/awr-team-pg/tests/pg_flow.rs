#![cfg(feature = "pg-tests")]

mod common;

use awr_team_pg::{ExecutionStore, LeaseStore, PgError, ReviewStore};
use common::{fresh_team_schema, test_config, with_app_role};
use serde_json::json;
use sha2::Digest as _;
use std::sync::MutexGuard;
use tokio_postgres::Client;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (
    MutexGuard<'static, ()>,
    Client,
    LeaseStore,
    LeaseStore,
    ExecutionStore,
    ReviewStore,
) {
    let (guard, admin, db) = fresh_team_schema().await;
    let config = with_app_role(&test_config(), &db);
    admin
        .batch_execute(
            r#"
INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
   ('tenant-a','actor-a','agent','A','active'),
   ('tenant-a','actor-b','agent','B','active'),
   ('tenant-a','reviewer-a','human','R','active'),
   ('tenant-a','runner-a','system','Runner','active');
INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
   VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');
INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role,independent_review)
   VALUES ('tenant-a','project-a','reviewer-a','developer',true);
INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
   VALUES ('tenant-a','project-a','main','main','active');
INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
   VALUES ('tenant-a','project-a','work-a','W');
INSERT INTO awr_team.source_snapshots(
   tenant_id, project_id, id, manifest_digest, source_ref_json, parser_version, created_by)
   VALUES ('tenant-a','project-a','snap-1','digest','{}','p1','actor-a');
UPDATE awr_team.projects SET active_snapshot_id='snap-1' WHERE id='project-a';
INSERT INTO awr_team.work_contracts(
   tenant_id, project_id, snapshot_id, scope_id, work_id, contract_hash,
   definition_state, title, contract_json)
   VALUES ('tenant-a','project-a','snap-1','main','work-a','hash-a','enabled','W',
           '{"completion_policy":"trusted_execution_and_review","acceptance":["done"]}');
"#,
        )
        .await
        .unwrap();
    (
        guard,
        admin,
        LeaseStore::from_config(config.clone()),
        LeaseStore::from_config(config.clone()),
        ExecutionStore::from_config(config.clone()),
        ReviewStore::from_config(config),
    )
}

#[tokio::test]
async fn two_actors_handoff_review_and_complete_with_independent_oracle() {
    let (_lock, admin, left, right, exec, review) = setup().await;
    let session = left
        .start_session(TENANT, PROJECT, "actor-a", "c1", "conv-a", "main", "work-a")
        .await
        .unwrap();
    let claim = left
        .claim(TENANT, PROJECT, &session.id, "actor-a", "c1", "claim-a", 60)
        .await
        .unwrap();
    let prepared = exec
        .prepare(
            TENANT,
            PROJECT,
            "actor-a",
            "c1",
            "prep-1",
            &claim.id,
            "runner-a",
            "hash-a",
            "in-1",
            "hard_fence",
            &["src/foo".into()],
            &json!([{"path":"src/foo/a.rs","content":"ok"}]),
        )
        .await
        .unwrap();
    exec.accept(TENANT, PROJECT, &prepared.id, claim.fence)
        .await
        .unwrap();
    exec.start(TENANT, PROJECT, &prepared.id, claim.fence)
        .await
        .unwrap();
    exec.report(
        TENANT,
        PROJECT,
        "runner-a",
        "trusted_executor",
        &prepared.id,
        "succeeded",
        // The execution RESULT digest is a different contract from the
        // artifact bytes digest below (CR #59 r3 P2-2).
        json!({"output_digest": format!("{:x}", sha2::Sha256::digest(b"exec-result-flow")), "environment_digest": "env"}),
        &["src/foo/a.rs".into()],
    )
    .await
    .unwrap();
    let handed = left
        .handoff(
            TENANT, PROJECT, &claim.id, "actor-a", "actor-b", "c2", "conv-b",
        )
        .await
        .unwrap();
    let err = left
        .require_fence(TENANT, PROJECT, "main", "work-a", "actor-a", claim.fence)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        PgError::StaleFence | PgError::LeaseExpired | PgError::Forbidden
    ));
    let stale = right
        .require_fence(TENANT, PROJECT, "main", "work-a", "actor-b", claim.fence)
        .await
        .unwrap_err();
    assert!(matches!(stale, PgError::StaleFence));
    right
        .require_fence(TENANT, PROJECT, "main", "work-a", "actor-b", handed.fence)
        .await
        .unwrap();
    let evidence = review
        .record_evidence(
            TENANT,
            PROJECT,
            "runner-a",
            "work-a",
            "hash-a",
            None,
            &json!({"log": "tested", "output_digest": format!("{:x}", sha2::Sha256::digest(b"exec-result-flow"))}),
            Some(b"oracle-bytes"),
            Some("in-1"),
            false,
            Some(&prepared.id),
        )
        .await
        .unwrap();
    let round = review
        .open_review(TENANT, PROJECT, "actor-b", "work-a", &evidence.id)
        .await
        .unwrap();
    let err = review
        .decide_review(TENANT, PROJECT, "actor-b", &round.id, "approve", "self")
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::AuthorCannotReview));
    review
        .decide_review(TENANT, PROJECT, "reviewer-a", &round.id, "approve", "ok")
        .await
        .unwrap();
    let receipt = review
        .complete(
            TENANT,
            PROJECT,
            "actor-b",
            "c2",
            "flow-complete-1",
            "work-a",
            "main",
            &evidence.id,
            None,
            true,
        )
        .await
        .unwrap();

    // Read persisted relationships through the independent admin connection,
    // rather than asking the Store that performed the completion to verify it.
    assert_eq!(receipt.work_id, "work-a");
    assert_eq!(receipt.contract_hash, "hash-a");
    assert_eq!(receipt.policy, "trusted_execution_and_review");
    let runtime = admin
        .query_one(
            "SELECT state, selected_completion_id FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id='work-a'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    assert_eq!(runtime.get::<_, String>("state"), "completed");
    assert_eq!(
        runtime
            .get::<_, Option<String>>("selected_completion_id")
            .as_deref(),
        Some(receipt.id.as_str())
    );
    let saved = admin
        .query_one(
            "SELECT id, contract_hash, policy, evidence_bundle_hash, approved_by_json
             FROM awr_team.completion_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id='work-a'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    assert_eq!(saved.get::<_, String>("id"), receipt.id);
    assert_eq!(
        saved.get::<_, String>("contract_hash"),
        receipt.contract_hash
    );
    assert_eq!(saved.get::<_, String>("policy"), receipt.policy);
    assert_eq!(
        saved.get::<_, String>("evidence_bundle_hash"),
        evidence.digest
    );
    assert_eq!(
        saved.get::<_, serde_json::Value>("approved_by_json"),
        // Derived from the scenario's actors, not from the code under test:
        // runner-a recorded the evidence and reported the execution, actor-b
        // opened the review and completed the work, reviewer-a approved it.
        json!({
            "approved_by": "reviewer-a",
            "reviewer_actor_id": "reviewer-a",
            "submitted_by": "actor-b",
            "final_submitter_actor_id": "actor-b",
            "author_actor_id": "runner-a",
            "executor_actor_id": "runner-a",
            "owner_person_id": null,
            "pr_delivery_id": null,
            "github_merged_does_not_complete": true
        })
    );
    let linked = admin
        .query_one(
            "SELECT e.id, e.execution_id, e.work_id, e.contract_hash, e.digest, ce.criterion_id
             FROM awr_team.completion_evidence ce
             JOIN awr_team.evidence e
               ON (e.tenant_id, e.project_id, e.id) = (ce.tenant_id, ce.project_id, ce.evidence_id)
             WHERE ce.tenant_id=$1 AND ce.project_id=$2 AND ce.completion_id=$3",
            &[&TENANT, &PROJECT, &receipt.id],
        )
        .await
        .unwrap();
    assert_eq!(linked.get::<_, String>("id"), evidence.id);
    assert_eq!(
        linked.get::<_, Option<String>>("execution_id").as_deref(),
        Some(prepared.id.as_str())
    );
    assert_eq!(linked.get::<_, String>("work_id"), "work-a");
    assert_eq!(linked.get::<_, String>("contract_hash"), "hash-a");
    assert_eq!(linked.get::<_, String>("digest"), evidence.digest);
    assert_eq!(linked.get::<_, String>("criterion_id"), "contract");
    let approval = admin
        .query_one(
            "SELECT r.author_actor_id, r.state, r.bundle_hash, d.reviewer_actor_id, d.decision
             FROM awr_team.review_rounds r
             JOIN awr_team.review_decisions d
               ON (d.tenant_id, d.project_id, d.review_round_id) = (r.tenant_id, r.project_id, r.id)
             WHERE r.tenant_id=$1 AND r.project_id=$2 AND r.id=$3 AND r.work_id='work-a'",
            &[&TENANT, &PROJECT, &round.id],
        )
        .await
        .unwrap();
    assert_eq!(approval.get::<_, String>("author_actor_id"), "actor-b");
    assert_eq!(approval.get::<_, String>("state"), "approved");
    assert_eq!(approval.get::<_, String>("bundle_hash"), evidence.digest);
    assert_eq!(approval.get::<_, String>("reviewer_actor_id"), "reviewer-a");
    assert_eq!(approval.get::<_, String>("decision"), "approve");

    let receipts: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.completion_receipts WHERE work_id='work-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let active: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.claims WHERE work_id='work-a' AND state='active'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let executions: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.executions WHERE work_id='work-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let holder: String = admin
        .query_one(
            "SELECT actor_id FROM awr_team.claims WHERE work_id='work-a' AND state='active'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(receipts, 1);
    assert_eq!(active, 1);
    assert_eq!(executions, 1);
    assert_eq!(holder, "actor-b");
}
