#![cfg(feature = "pg-tests")]
mod common;
#[path = "cases/evidence_input.rs"]
mod evidence_input_tests;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "cases/review_submission.rs"]
mod review_submission_tests;

use awr_team::WorkContract;
use awr_team_pg::{PgError, WorkstreamReadStore, workstream_credential_hash};
use fixture::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_postgres::Client;

const RUNNER: &str =
    "awr1.runner-sys.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const REVIEWER_TOKEN: &str =
    "awr1.reviewer-h.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const INPUT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RESULT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn stored_evidence_digest(
    work_id: &str,
    contract_hash: &str,
    input_digest: Option<&str>,
    output_digest: Option<&str>,
    execution_result_digest: Option<&str>,
    payload: &Value,
) -> String {
    let canonical = awr_team::canonical_json(&json!({
        "work_id": work_id,
        "contract_hash": contract_hash,
        "input_digest": input_digest,
        "output_digest": output_digest,
        "execution_result_digest": execution_result_digest,
        "payload": payload,
    }))
    .unwrap();
    format!("{:x}", Sha256::digest(canonical))
}

async fn seed_review_actors(admin: &Client) {
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
                ('reader-tenant','runner','system','Runner','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
                ('reader-tenant','reader-project','runner','worker')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('reader-tenant','reader-project','person-author','Author','active'),
                ('reader-tenant','reader-project','reviewer','Reviewer','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
                ('reader-tenant','reader-project','bind-agent','person-author','agent','active')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    for (id, actor, client, token) in [
        ("runner-sys", "runner", "cli-runner", RUNNER),
        ("reviewer-h", "reviewer", "cli-reviewer", REVIEWER_TOKEN),
    ] {
        let hash = workstream_credential_hash(token).unwrap();
        admin
            .execute(
                "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
                 VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
                &[&TENANT, &id, &actor, &client, &hash],
            )
            .await
            .unwrap();
    }
    let stream: String = admin
        .query_one(
            "SELECT workstream_id FROM awr_team.sessions WHERE id='session-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    for (client, actor) in [("cli-runner", "runner"), ("cli-reviewer", "reviewer")] {
        admin
            .execute(
                "INSERT INTO awr_team.workstream_grants(
                    tenant_id,project_id,actor_id,client_id,workstream_id,
                    authority_version,can_read,can_write)
                 VALUES($1,$2,$3,$4,$5,1,true,true)
                 ON CONFLICT DO NOTHING",
                &[&TENANT, &PROJECT, &actor, &client, &stream],
            )
            .await
            .unwrap();
    }
    for (session, actor, client) in [
        ("session-runner", "runner", "cli-runner"),
        ("session-reviewer", "reviewer", "cli-reviewer"),
    ] {
        admin
            .execute(
                "INSERT INTO awr_team.sessions(
                    tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,
                    conversation_id,state,workstream_id,ownership_version)
                 VALUES($1,$2,$3,'main','a',$4,$5,$3,'active',$6,1)
                 ON CONFLICT DO NOTHING",
                &[&TENANT, &PROJECT, &session, &actor, &client, &stream],
            )
            .await
            .unwrap();
    }
    enable_writes(admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships
             SET role='developer', independent_review=true,
                 membership_version=membership_version+1
             WHERE tenant_id='reader-tenant' AND project_id='reader-project'
               AND actor_id='reviewer';
             UPDATE awr_team.workstream_grants
             SET can_write=true, grant_version=grant_version+1
             WHERE client_id IN ('cli-runner','cli-reviewer')",
        )
        .await
        .unwrap();
}

async fn insert_execution(admin: &Client, id: &str, contract_hash: &str, fence: i64, state: &str) {
    admin
        .execute(
            "INSERT INTO awr_team.executions(
                tenant_id,project_id,id,work_id,fence,contract_hash,input_digest,
                executor_actor_id,state,result_digest,scope_id)
             VALUES($1,$2,$3,'a',$4,$5,$6,'runner',$7,$8,'main')",
            &[
                &TENANT,
                &PROJECT,
                &id,
                &fence,
                &contract_hash,
                &INPUT,
                &state,
                &RESULT,
            ],
        )
        .await
        .unwrap();
}

async fn insert_succeeded_execution(admin: &Client, id: &str, contract_hash: &str, fence: i64) {
    insert_execution(admin, id, contract_hash, fence, "succeeded").await;
}

async fn insert_currently_bound_execution(
    admin: &Client,
    id: &str,
    contract_hash: &str,
    fence: i64,
    state: &str,
) {
    let claim_id = format!("{id}-claim");
    admin
        .execute(
            "INSERT INTO awr_team.work_runtime(
                tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
             VALUES($1,$2,'main','a','claimed',1,$3)",
            &[&TENANT, &PROJECT, &fence],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.claims(
                tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,
                fence,expires_at,state,workstream_id,ownership_version,coordinator_epoch)
             SELECT s.tenant_id,s.project_id,$1,'main','a',s.id,'runner',$2,
                clock_timestamp()+interval '1 hour','active',s.workstream_id,
                s.ownership_version,p.coordinator_epoch
             FROM awr_team.sessions s
             JOIN awr_team.projects p ON p.tenant_id=s.tenant_id AND p.id=s.project_id
             WHERE s.tenant_id=$3 AND s.project_id=$4 AND s.id='session-runner'",
            &[&claim_id, &fence, &TENANT, &PROJECT],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.executions(
                tenant_id,project_id,id,work_id,session_id,claim_id,fence,
                contract_hash,input_digest,executor_actor_id,state,result_digest,
                scope_id,workstream_id,ownership_version,executor_client_id,
                coordinator_epoch)
             SELECT s.tenant_id,s.project_id,$1,'a',s.id,$2,$3,$4,$5,
                'runner',$6,$7,'main',s.workstream_id,s.ownership_version,
                s.client_id,p.coordinator_epoch
             FROM awr_team.sessions s
             JOIN awr_team.projects p ON p.tenant_id=s.tenant_id AND p.id=s.project_id
             WHERE s.tenant_id=$8 AND s.project_id=$9 AND s.id='session-runner'",
            &[
                &id,
                &claim_id,
                &fence,
                &contract_hash,
                &INPUT,
                &state,
                &RESULT,
                &TENANT,
                &PROJECT,
            ],
        )
        .await
        .unwrap();
}

async fn run(
    store: &WorkstreamReadStore,
    token: &str,
    request: &str,
    op: &str,
    args: Value,
) -> Value {
    let prepared = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&prepared, request, op, args),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone()
}

async fn run_err(
    store: &WorkstreamReadStore,
    token: &str,
    request: &str,
    op: &str,
    args: Value,
) -> PgError {
    let prepared = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&prepared, request, op, args),
        )
        .await
        .unwrap_err()
}

fn submit_args(session: &str, execution_id: &str, artifact: &str) -> Value {
    json!({
        "session_id": session,
        "expected_session_version": "1",
        "payload": {"passed": true, "output_digest": RESULT},
        "artifact_hex": artifact,
        "input_digest": INPUT,
        "dirty_tree": false,
        "execution_id": execution_id
    })
}

async fn current_contract(admin: &Client) -> WorkContract {
    let row = admin
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id='a'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    serde_json::from_value(row.get(0)).unwrap()
}

async fn put_contract(admin: &Client, contract: &WorkContract) {
    let hash = contract.hash().unwrap();
    let json = serde_json::to_value(contract).unwrap();
    admin
        .execute(
            "UPDATE awr_team.work_contracts
             SET contract_json=$3, contract_hash=$4
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id='a'",
            &[&TENANT, &PROJECT, &json, &hash],
        )
        .await
        .unwrap();
}

async fn rework_business_snapshot(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'project',(SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM awr_team.projects p),
        'work',(SELECT jsonb_agg(to_jsonb(w) ORDER BY work_id) FROM awr_team.work_runtime w),
        'reviews',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.review_rounds r),
        'decisions',(SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM awr_team.review_decisions d),
        'evidence',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.evidence e),
        'operations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM awr_team.operations o),
        'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.events e),
        'completion',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM awr_team.completion_receipts c))", &[])
        .await.unwrap().get(0)
}

#[tokio::test]
async fn rework_distinguishes_review_state_from_input_without_mutation_or_history_loss() {
    for state in ["open", "approved", "invalidated", "unavailable", "rejected"] {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        let evidence = run(&store, A, "state-evidence", "evidence.submit",
            json!({"session_id":"session-a","expected_session_version":"1","payload":{},"dirty_tree":false})).await;
        let opened = run(&store, A, "state-open", "review.open",
            json!({"session_id":"session-a","expected_session_version":"1","evidence_id":evidence["evidence_id"]})).await;
        let round_id = opened["round_id"].as_str().unwrap();
        // These are isolated historical-state fixtures, never client approval authority.
        if state != "unavailable" {
            admin
                .execute(
                    "UPDATE awr_team.review_rounds SET state=$1 WHERE id=$2",
                    &[&state, &round_id],
                )
                .await
                .unwrap();
        }
        let selected = if state == "unavailable" {
            "unavailable-review-sentinel"
        } else {
            round_id
        };
        let args = json!({"session_id":"session-a","expected_session_version":"1",
            "round_id":selected,"note":"private-rework-value-sentinel"});
        let p = prepare(&store, A, "a").await;
        let command = command(&p, "state-rework", "work.rework", args);
        let before = rework_business_snapshot(&admin).await;
        let result = store
            .commands()
            .execute(TENANT, PROJECT, A, command.clone())
            .await;
        if state == "rejected" {
            let result = result.unwrap();
            assert_eq!(result["receipt"]["data"]["rework_acknowledged"], true);
            assert_eq!(result["receipt"]["data"]["history_retained"], true);
            assert_eq!(result["receipt"]["data"]["task_complete"], false);
            let after = rework_business_snapshot(&admin).await;
            assert_eq!(before["reviews"], after["reviews"]);
            assert_eq!(before["decisions"], after["decisions"]);
            let replay = store
                .commands()
                .execute(TENANT, PROJECT, A, command)
                .await
                .unwrap();
            assert_eq!(replay["receipt"], result["receipt"]);
            assert_eq!(rework_business_snapshot(&admin).await, after);
        } else {
            let error = result.unwrap_err();
            if state == "unavailable" {
                assert!(matches!(error, PgError::ReviewUnavailable));
            } else {
                assert!(
                    matches!(error, PgError::ReworkRequiresReturnedReview),
                    "{state}"
                );
            }
            assert!(!error.to_string().contains("private-rework-value-sentinel"));
            assert!(!error.is_invalid_command_fields());
            assert_eq!(rework_business_snapshot(&admin).await, before, "{state}");
        }
    }
}

#[tokio::test]
async fn malformed_rework_fields_stay_input_errors_and_cannot_change_review_history() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let before = rework_business_snapshot(&admin).await;
    for case in [
        "missing-note",
        "reason",
        "extra",
        "numeric-version",
        "null-round",
        "missing-version",
    ] {
        let mut args = json!({"session_id":"session-a","expected_session_version":"1",
            "round_id":"private-round-value-sentinel","note":"private-note-value-sentinel"});
        match case {
            "missing-note" => {
                args.as_object_mut().unwrap().remove("note");
            }
            "reason" => {
                args.as_object_mut().unwrap().remove("note");
                args["reason"] = json!("private-reason-value-sentinel");
            }
            "extra" => {
                args["private-key-sentinel"] = json!("private-value-sentinel");
            }
            "numeric-version" => {
                args["expected_session_version"] = json!(1);
            }
            "null-round" => {
                args["round_id"] = Value::Null;
            }
            _ => {
                args.as_object_mut()
                    .unwrap()
                    .remove("expected_session_version");
            }
        }
        let error = run_err(&store, A, case, "work.rework", args).await;
        if case == "missing-version" {
            assert!(error.is_missing_review_session_version());
        } else {
            assert!(error.is_invalid_command_fields(), "{case}");
        }
        assert!(!error.to_string().contains("private-"));
        assert_eq!(rework_business_snapshot(&admin).await, before, "{case}");
    }
}

#[tokio::test]
async fn execution_consistency_rejects_success_claim_for_failed_execution_before_storage() {
    let (_guard, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"].as_str().unwrap();
    insert_execution(&admin, "exec-failed-claim", contract_hash, 1, "failed").await;
    let counts = "SELECT (SELECT count(*) FROM awr_team.evidence),
                         (SELECT count(*) FROM awr_team.artifacts)";
    let before = admin.query_one(counts, &[]).await.unwrap();

    let error = run_err(
        &store,
        RUNNER,
        "failed-claim",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-failed-claim",
            &hex_encode(b"must-not-persist"),
        ),
    )
    .await;
    assert!(matches!(error, PgError::EvidenceInvalid));

    let after = admin.query_one(counts, &[]).await.unwrap();
    assert_eq!(after.get::<_, i64>(0), before.get::<_, i64>(0));
    assert_eq!(after.get::<_, i64>(1), before.get::<_, i64>(1));
}

#[tokio::test]
async fn execution_consistency_rejects_opposite_terminal_claim_without_result_digests() {
    let (_guard, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"].as_str().unwrap();
    let counts = "SELECT (SELECT count(*) FROM awr_team.evidence),
                         (SELECT count(*) FROM awr_team.artifacts)";
    let before = admin.query_one(counts, &[]).await.unwrap();

    for (index, state, claimed_success) in [
        (0, "failed", true),
        (1, "cancelled", true),
        (2, "succeeded", false),
    ] {
        let execution_id = format!("exec-terminal-null-{index}");
        insert_execution(&admin, &execution_id, contract_hash, 1, state).await;
        admin
            .execute(
                "UPDATE awr_team.executions SET result_digest=NULL WHERE id=$1",
                &[&execution_id],
            )
            .await
            .unwrap();
        let error = run_err(
            &store,
            RUNNER,
            &format!("terminal-null-{index}"),
            "evidence.submit",
            json!({
                "session_id":"session-runner",
                "expected_session_version":"1",
                "payload":{"passed":claimed_success},
                "artifact_hex":hex_encode(format!("terminal-{index}").as_bytes()),
                "input_digest":INPUT,
                "dirty_tree":false,
                "execution_id":execution_id,
            }),
        )
        .await;
        assert!(matches!(error, PgError::EvidenceInvalid), "{state}");
        let after = admin.query_one(counts, &[]).await.unwrap();
        assert_eq!(after.get::<_, i64>(0), before.get::<_, i64>(0), "{state}");
        assert_eq!(after.get::<_, i64>(1), before.get::<_, i64>(1), "{state}");
    }
}

#[tokio::test]
async fn execution_consistency_keeps_nonterminal_execution_unverifiable_without_success() {
    let (_guard, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"].as_str().unwrap();
    insert_execution(&admin, "exec-unknown-claim", contract_hash, 1, "unknown").await;

    let evidence = run(
        &store,
        RUNNER,
        "unknown-claim",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-unknown-claim",
            &hex_encode(b"unknown-report"),
        ),
    )
    .await;
    assert_eq!(evidence["claimed_success"], true);
    assert!(evidence["execution_success"].is_null());
    assert_eq!(evidence["execution_consistency"]["status"], "unverifiable");
    assert_eq!(
        evidence["execution_consistency"]["recorded_state"],
        "unknown"
    );
    assert_eq!(
        evidence["execution_consistency"]["independent_verification"],
        false
    );
}

#[tokio::test]
async fn execution_consistency_inspects_legacy_contradiction_without_receipt_upgrade_or_mutation() {
    let (_guard, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"].as_str().unwrap();
    insert_currently_bound_execution(&admin, "exec-legacy-failed", contract_hash, 1, "failed")
        .await;
    let payload = json!({"passed":true,"output_digest":RESULT});
    let evidence_digest = stored_evidence_digest(
        "a",
        contract_hash,
        Some(INPUT),
        None,
        Some(RESULT),
        &payload,
    );
    admin
        .execute(
            "INSERT INTO awr_team.execution_receipts(
                tenant_id,project_id,id,execution_id,reporter_actor_id,
                receipt_kind,digest,payload_json)
             VALUES($1,$2,'receipt-legacy-trusted','exec-legacy-failed','agent',
                'trusted_executor',$3,$4)",
            &[
                &TENANT,
                &PROJECT,
                &"e".repeat(64),
                &json!({"outcome":"failed"}),
            ],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.evidence(
                tenant_id,project_id,id,work_id,execution_id,contract_hash,
                input_digest,execution_result_digest,evidence_kind,trust_basis,
                digest,payload_json,created_by)
             VALUES($1,$2,'legacy-contradiction','a','exec-legacy-failed',$3,
                $4,$5,'report','caller_asserted',$6,$7,'agent')",
            &[
                &TENANT,
                &PROJECT,
                &contract_hash,
                &INPUT,
                &RESULT,
                &evidence_digest,
                &payload,
            ],
        )
        .await
        .unwrap();
    let stored_before: Value = admin
        .query_one(
            "SELECT to_jsonb(e) FROM awr_team.evidence e WHERE id='legacy-contradiction'",
            &[],
        )
        .await
        .unwrap()
        .get(0);

    let mut inspect = query("evidence.inspect");
    inspect.work_id = Some("a".into());
    inspect.evidence_id = Some("legacy-contradiction".into());
    let evidence =
        store.query(TENANT, PROJECT, A, inspect).await.unwrap()["data"]["evidence"].clone();
    let consistency = &evidence["execution_consistency"];
    assert_eq!(consistency["status"], "contradicted", "{consistency}");
    assert_eq!(consistency["claimed_success"], true);
    assert_eq!(consistency["execution_success"], false);
    assert_eq!(consistency["recorded_state"], "failed");
    assert_eq!(consistency["fact_source"], "team_execution_record");
    assert_eq!(consistency["independent_verification"], false);
    let stored_after: Value = admin
        .query_one(
            "SELECT to_jsonb(e) FROM awr_team.evidence e WHERE id='legacy-contradiction'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored_after, stored_before);

    admin
        .execute(
            "UPDATE awr_team.evidence SET payload_json=$1 WHERE id='legacy-contradiction'",
            &[&json!({"passed":false,"output_digest":RESULT})],
        )
        .await
        .unwrap();
    let mut inspect = query("evidence.inspect");
    inspect.work_id = Some("a".into());
    inspect.evidence_id = Some("legacy-contradiction".into());
    let tampered = store.query(TENANT, PROJECT, A, inspect).await.unwrap()["data"]["evidence"]
        ["execution_consistency"]
        .clone();
    assert_eq!(tampered["status"], "unverifiable");
    assert_eq!(tampered["reason"], "evidence_digest_mismatch");
    assert!(tampered["claimed_success"].is_null());
    assert!(tampered["execution_success"].is_null());
    assert!(tampered["recorded_state"].is_null());
    assert!(tampered["fact_source"].is_null());
    assert_eq!(tampered["independent_verification"], false);
}

#[tokio::test]
async fn execution_consistency_sanitizes_legacy_execution_without_current_binding() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"].as_str().unwrap();
    insert_execution(&admin, "exec-legacy-unbound", contract_hash, 1, "failed").await;
    let payload = json!({"passed":true,"output_digest":RESULT});
    let evidence_digest = stored_evidence_digest(
        "a",
        contract_hash,
        Some(INPUT),
        None,
        Some(RESULT),
        &payload,
    );
    admin
        .execute(
            "INSERT INTO awr_team.evidence(
                tenant_id,project_id,id,work_id,execution_id,contract_hash,
                input_digest,execution_result_digest,evidence_kind,trust_basis,
                digest,payload_json,created_by)
             VALUES($1,$2,'legacy-unbound-evidence','a','exec-legacy-unbound',$3,
                $4,$5,'report','caller_asserted',$6,$7,'agent')",
            &[
                &TENANT,
                &PROJECT,
                &contract_hash,
                &INPUT,
                &RESULT,
                &evidence_digest,
                &payload,
            ],
        )
        .await
        .unwrap();

    let mut inspect = query("evidence.inspect");
    inspect.work_id = Some("a".into());
    inspect.evidence_id = Some("legacy-unbound-evidence".into());
    let consistency = store.query(TENANT, PROJECT, A, inspect).await.unwrap()["data"]["evidence"]
        ["execution_consistency"]
        .clone();
    assert_eq!(consistency["status"], "unverifiable");
    assert_eq!(consistency["reason"], "execution_not_inspectable");
    assert!(consistency["claimed_success"].is_null());
    assert!(consistency["execution_success"].is_null());
    assert!(consistency["recorded_state"].is_null());
    assert!(consistency["fact_source"].is_null());
    assert_eq!(consistency["independent_verification"], false);
}

#[tokio::test]
async fn mainline_submit_open_accept_complete_is_team_independent() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-main", &contract_hash, 1).await;

    let evidence = run(
        &store,
        RUNNER,
        "ev-1",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-main",
            &hex_encode(b"ws018-artifact"),
        ),
    )
    .await;
    assert_eq!(evidence["trust_basis"], "trusted_executor");
    assert_eq!(evidence["claimed_success"], true);
    assert_eq!(evidence["execution_success"], true);
    assert_eq!(evidence["execution_consistency"]["status"], "consistent");
    assert_eq!(
        evidence["execution_consistency"]["independent_verification"],
        false
    );
    assert_eq!(evidence["author_self_report"], false);
    assert_eq!(evidence["task_complete"], false);
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();

    let opened = run(
        &store,
        A,
        "open-1",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    assert_eq!(opened["state"], "open");
    assert_eq!(opened["author_person_id"], "person-author");
    assert_eq!(
        opened["binds_exact_contract_artifact_execution_round"],
        true
    );
    let round_id = opened["round_id"].as_str().unwrap().to_string();

    let accepted = run(
        &store,
        REVIEWER_TOKEN,
        "acc-1",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"looks good"
        }),
    )
    .await;
    assert_eq!(accepted["state"], "approved");
    assert_eq!(accepted["independence_kind"], "team_independent");
    assert_eq!(accepted["team_independent_acceptance"], true);
    assert_eq!(accepted["human_approval"], true);
    assert_eq!(accepted["task_complete"], false);

    let completed = run(
        &store,
        A,
        "done-1",
        "work.complete",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id,
            "context_complete": true
        }),
    )
    .await;
    assert_eq!(completed["task_complete"], true);
    assert_eq!(completed["execution_success"], true);
    assert_eq!(completed["human_approval"], true);
    assert_eq!(completed["independence_kind"], "team_independent");
    assert_eq!(completed["team_independent_acceptance"], true);
    assert!(completed["provider_private_session"].is_null());
    assert_eq!(completed["selected_completion_id"], completed["receipt_id"]);

    let mut q = query("completion.inspect");
    q.work_id = Some("a".into());
    let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    assert_eq!(inspected["runtime_state"], "completed");
    assert_eq!(
        inspected["completion"]["independence_kind"],
        "team_independent"
    );
    assert!(inspected["completion"]["provider_private_session"].is_null());
}

#[tokio::test]
async fn same_person_agent_cannot_fake_team_independence() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    admin
        .batch_execute(
            "DELETE FROM awr_team.person_agent_bindings
             WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND agent_id='agent';
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES ('reader-tenant','reader-project','bind-same','reviewer','agent','active');",
        )
        .await
        .unwrap();
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-same", &contract_hash, 1).await;
    let evidence = run(
        &store,
        RUNNER,
        "ev-same",
        "evidence.submit",
        submit_args("session-runner", "exec-same", &hex_encode(b"same-person")),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-same",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    assert_eq!(opened["author_person_id"], "reviewer");
    let round_id = opened["round_id"].as_str().unwrap().to_string();
    let err = run_err(
        &store,
        REVIEWER_TOKEN,
        "acc-same",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"self approve blocked"
        }),
    )
    .await;
    assert!(matches!(err, PgError::AuthorCannotReview));

    let mut contract = current_contract(&admin).await;
    contract.completion_policy = "trusted_execution_and_author_self_review".into();
    put_contract(&admin, &contract).await;
    let hash = contract.hash().unwrap();
    insert_succeeded_execution(&admin, "exec-self", &hash, 2).await;
    let evidence = run(
        &store,
        RUNNER,
        "ev-self",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-self",
            &hex_encode(b"self-review-bytes"),
        ),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-self",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    let round_id = opened["round_id"].as_str().unwrap().to_string();
    let accepted = run(
        &store,
        REVIEWER_TOKEN,
        "acc-self",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"explicit self review policy"
        }),
    )
    .await;
    assert_eq!(accepted["independence_kind"], "personal_self_review");
    assert_eq!(accepted["team_independent_acceptance"], false);
}

#[tokio::test]
async fn return_rework_keeps_history_and_contract_change_blocks_stale_approval() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-ret", &contract_hash, 1).await;
    let evidence = run(
        &store,
        RUNNER,
        "ev-ret",
        "evidence.submit",
        submit_args("session-runner", "exec-ret", &hex_encode(b"return-bytes")),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-ret",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    let round_id = opened["round_id"].as_str().unwrap().to_string();
    let returned = run(
        &store,
        REVIEWER_TOKEN,
        "ret-1",
        "review.return",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"needs fixes"
        }),
    )
    .await;
    assert_eq!(returned["state"], "rejected");
    let rework = run(
        &store,
        A,
        "rew-1",
        "work.rework",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "round_id": round_id,
            "note":"addressing return"
        }),
    )
    .await;
    assert_eq!(rework["history_retained"], true);
    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND state='rejected'",
            &[&TENANT, &PROJECT, &round_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);

    let evidence2 = run(
        &store,
        RUNNER,
        "ev-ret2",
        "evidence.submit",
        submit_args("session-runner", "exec-ret", &hex_encode(b"return-bytes-2")),
    )
    .await;
    let evidence_id2 = evidence2["evidence_id"].as_str().unwrap().to_string();
    let opened2 = run(
        &store,
        A,
        "open-ret2",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id2
        }),
    )
    .await;
    let round2 = opened2["round_id"].as_str().unwrap().to_string();
    run(
        &store,
        REVIEWER_TOKEN,
        "acc-ret2",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round2,
            "reason":"ok for now"
        }),
    )
    .await;
    let mut contract = current_contract(&admin).await;
    contract.acceptance.push("extra-gate".into());
    put_contract(&admin, &contract).await;
    let err = run_err(
        &store,
        A,
        "done-stale",
        "work.complete",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id2,
            "context_complete": true
        }),
    )
    .await;
    assert!(matches!(
        err,
        PgError::EvidenceInvalid | PgError::ReviewRequired | PgError::CompletionRejected
    ));
}

#[tokio::test]
async fn completion_keeps_author_executor_distinct_from_finalizer_without_pr() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-attr", &contract_hash, 1).await;

    let evidence = run(
        &store,
        RUNNER,
        "ev-attr",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-attr",
            &hex_encode(b"ws031-attr-artifact"),
        ),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-attr",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    let round_id = opened["round_id"].as_str().unwrap().to_string();
    run(
        &store,
        REVIEWER_TOKEN,
        "acc-attr",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"independent approve"
        }),
    )
    .await;

    // Finalizer is agent (A), distinct from runner author/executor and reviewer.
    let completed = run(
        &store,
        A,
        "done-attr",
        "work.complete",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id,
            "context_complete": true
        }),
    )
    .await;
    assert_eq!(completed["author_actor_id"], "runner");
    assert_eq!(completed["executor_actor_id"], "runner");
    assert_eq!(completed["reviewer_actor_id"], "reviewer");
    assert_eq!(completed["final_submitter_actor_id"], "agent");
    assert_ne!(
        completed["author_actor_id"],
        completed["final_submitter_actor_id"]
    );
    assert_ne!(completed["executor_actor_id"], completed["execution_id"]);

    let mut q = query("completion.inspect");
    q.work_id = Some("a".into());
    let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    let completion = &inspected["completion"];
    assert_eq!(completion["author_actor_id"], "runner");
    assert_eq!(completion["executor_actor_id"], "runner");
    assert_eq!(completion["final_submitter_actor_id"], "agent");
    assert_eq!(completion["approved_by"]["author_actor_id"], "runner");
    assert_eq!(completion["approved_by"]["executor_actor_id"], "runner");
    assert_eq!(completion["approved_by"]["reviewer_actor_id"], "reviewer");
    assert_eq!(
        completion["approved_by"]["final_submitter_actor_id"],
        "agent"
    );
    assert!(completion["approved_by"]["pr_delivery_id"].is_null());

    let row = admin
        .query_one(
            "SELECT author_actor_id, executor_actor_id, final_submitter_actor_id, execution_id
             FROM awr_team.completion_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &TENANT,
                &PROJECT,
                &completed["receipt_id"].as_str().unwrap(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, Option<String>>(0).as_deref(), Some("runner"));
    assert_eq!(row.get::<_, Option<String>>(1).as_deref(), Some("runner"));
    assert_eq!(row.get::<_, Option<String>>(2).as_deref(), Some("agent"));
    assert_eq!(
        row.get::<_, Option<String>>(3).as_deref(),
        Some("exec-attr")
    );
    assert_ne!(
        row.get::<_, Option<String>>(1).as_deref(),
        row.get::<_, Option<String>>(3).as_deref()
    );
}

#[tokio::test]
async fn invalidated_prior_approval_cannot_complete() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-inv", &contract_hash, 1).await;

    let evidence1 = run(
        &store,
        RUNNER,
        "ev-inv-1",
        "evidence.submit",
        submit_args("session-runner", "exec-inv", &hex_encode(b"invalidated-e1")),
    )
    .await;
    let evidence_id1 = evidence1["evidence_id"].as_str().unwrap().to_string();
    let opened1 = run(
        &store,
        A,
        "open-inv-1",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id1
        }),
    )
    .await;
    let round1 = opened1["round_id"].as_str().unwrap().to_string();
    run(
        &store,
        REVIEWER_TOKEN,
        "acc-inv-1",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round1,
            "reason":"approve e1"
        }),
    )
    .await;

    let evidence2 = run(
        &store,
        RUNNER,
        "ev-inv-2",
        "evidence.submit",
        submit_args("session-runner", "exec-inv", &hex_encode(b"invalidated-e2")),
    )
    .await;
    let evidence_id2 = evidence2["evidence_id"].as_str().unwrap().to_string();
    run(
        &store,
        A,
        "open-inv-2",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id2
        }),
    )
    .await;

    let r1_state: String = admin
        .query_one(
            "SELECT state FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &round1],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(r1_state, "invalidated");

    let err = run_err(
        &store,
        A,
        "done-inv-stale",
        "work.complete",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id1,
            "context_complete": true
        }),
    )
    .await;
    assert!(matches!(err, PgError::ReviewRequired));

    let completed: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND work_id='a' AND state='completed'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(completed, 0);
}

mod agent_review_tests {
    use super::*;
    use awr_core::{
        AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
        ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
    };
    use awr_team_pg::AuthorizationStore;
    use std::collections::BTreeSet;

    const POLICY: &str = "caller_managed_execution_and_agent_review";
    include!("cases/agent_dependencies.rs");

    async fn seed_agent_reviewer(admin: &Client, db: &str) {
        seed_review_actors(admin).await;
        admin.batch_execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE id='reviewer';
             UPDATE awr_team.project_memberships SET agent_review=true WHERE actor_id='reviewer';
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('reader-tenant','reader-project','bind-reviewer','person-author','reviewer','active');"
        ).await.unwrap();
        let person = PersonId::new("person-author").unwrap();
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
            .issue(
                TENANT,
                PROJECT,
                &IssueAuthorizationRequest {
                    request_key: "issue-agent-review".into(),
                    authorization: AgentAuthorization {
                        id: "review-grant".into(),
                        authorizer_person_id: person.clone(),
                        responsible_person_id: person,
                        subject_kind: ExecutionSubjectKind::Agent,
                        subject_id: "reviewer".into(),
                        client_id: "cli-reviewer".into(),
                        session_id: None,
                        model_id: None,
                        scope: AuthorizationScope::Project {
                            project_id: PROJECT.into(),
                        },
                        actions: BTreeSet::from([
                            AuthorizedAction::Inspect,
                            AuthorizedAction::StartWork,
                            AuthorizedAction::Review,
                        ]),
                        expires_at_ms: None,
                        status: AuthorizationStatus::Active,
                        revoked_at_ms: None,
                        revoked_by: None,
                        verifiable_capabilities: vec![],
                        self_reported_skill_hints: vec![],
                        parent_authorization_id: None,
                        maintainer_person_id: None,
                        created_at_ms: 1000,
                        binding_id: Some("bind-reviewer".into()),
                    },
                },
            )
            .await
            .unwrap();
    }

    async fn open_round(
        admin: &Client,
        store: &WorkstreamReadStore,
        policy: &str,
        key: &str,
    ) -> Value {
        let mut contract = current_contract(admin).await;
        contract.completion_policy = policy.into();
        put_contract(admin, &contract).await;
        let ev = run(
            store,
            A,
            &format!("{key}-evidence"),
            "evidence.submit",
            json!({
                "session_id":"session-a", "expected_session_version":"1",
                "payload":{"passed":true}, "artifact_hex":hex_encode(b"reviewed artifact"),
                "input_digest":INPUT, "dirty_tree":false
            }),
        )
        .await;
        run(store, A, &format!("{key}-open"), "review.open", json!({
            "session_id":"session-a", "expected_session_version":"1", "evidence_id":ev["evidence_id"]
        })).await
    }

    fn args(round: &Value) -> Value {
        json!({"session_id":"session-reviewer", "expected_session_version":"1",
            "round_id":round["round_id"], "decision":"approve", "reason":"Reviewed the exact artifact"})
    }

    async fn denied(store: &WorkstreamReadStore, key: &str, op: &str, args: Value) -> PgError {
        // Obtain independent preconditions so a denied query cannot mask the
        // command authorization being exercised.
        let p = prepare(store, A, "a").await;
        store
            .commands()
            .execute(TENANT, PROJECT, REVIEWER_TOKEN, command(&p, key, op, args))
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn same_person_agent_reviewer_records_approval_without_human_acceptance() {
        let (_g, admin, db, store) = setup().await;
        seed_agent_reviewer(&admin, &db).await;
        let round = open_round(&admin, &store, POLICY, "agent-positive").await;
        let result = run(
            &store,
            REVIEWER_TOKEN,
            "agent-decide",
            "review.decide",
            args(&round),
        )
        .await;
        assert_eq!(result["approval_basis"], "agent_review");
        assert_eq!(result["independence_kind"], "agent_review");
        assert_eq!(result["human_approval"], false);
        assert_eq!(result["team_independent_acceptance"], false);
        assert_eq!(result["task_complete"], false);
        assert_eq!(result["author_person_id"], result["reviewer_person_id"]);
        assert_eq!(result["author_client_id"], "cli-a");
        assert_eq!(result["reviewer_client_id"], "cli-reviewer");
        let mut q = query("review.inspect");
        q.work_id = Some("a".into());
        q.review_round_id = Some(round["round_id"].as_str().unwrap().into());
        let inspected = store
            .query(TENANT, PROJECT, REVIEWER_TOKEN, q)
            .await
            .unwrap();
        assert_eq!(inspected["data"]["review"]["author_client_id"], "cli-a");
        let decision = &inspected["data"]["review"]["decisions"][0];
        assert_eq!(decision["approval_basis"], "agent_review");
        assert_eq!(decision["human_approval"], false);
        assert_eq!(decision["team_independent_acceptance"], false);
        let stored = admin.query_one("SELECT approval_basis,reviewer_client_id FROM awr_team.review_decisions WHERE review_round_id=$1",
            &[&round["round_id"].as_str().unwrap()]).await.unwrap();
        assert_eq!(stored.get::<_, String>(0), "agent_review");
        assert_eq!(stored.get::<_, String>(1), "cli-reviewer");
        assert!(matches!(
            run_err(
                &store,
                A,
                "agent-no-completion",
                "work.complete",
                json!({
                    "session_id":"session-a","expected_session_version":"1",
                    "evidence_id":round["evidence_id"],"context_complete":true
                })
            )
            .await,
            PgError::EvidenceInvalid
        ));
    }

    include!("cases/agent_completion.rs");
    include!("cases/review_provenance.rs");
    include!("cases/workspace_completion.rs");
    mod simulated_member_tests {
        use super::*;
        include!("cases/simulated_member_review.rs");
        include!("cases/simulated_member_dependencies.rs");
        include!("cases/cross_workstream_exports.rs");
        include!("cases/cross_workstream_adoption.rs");
        include!("cases/cross_workstream_history.rs");
    }

    #[tokio::test]
    async fn independent_delegations_cover_each_action_without_restoring_narrowed_parent() {
        let (_g, admin, db, store) = setup().await;
        seed_agent_reviewer(&admin, &db).await;
        let original: Value = admin
            .query_one(
                "SELECT body_json FROM awr_team.agent_authorizations WHERE id='review-grant'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        let mut review_only = original.clone();
        review_only["actions"] = json!(["review"]);
        admin
            .execute(
                "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='review-grant'",
                &[&review_only],
            )
            .await
            .unwrap();
        let authz =
            AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
        let mut work_grant: AgentAuthorization = serde_json::from_value(original).unwrap();
        work_grant.id = "separate-work-grant".into();
        work_grant.actions =
            BTreeSet::from([AuthorizedAction::Inspect, AuthorizedAction::StartWork]);
        work_grant.created_at_ms = 500;
        authz
            .issue(
                TENANT,
                PROJECT,
                &IssueAuthorizationRequest {
                    request_key: "separate-work".into(),
                    authorization: work_grant.clone(),
                },
            )
            .await
            .unwrap();
        let round = open_round(&admin, &store, POLICY, "separate-grants").await;
        // An older work grant must not hide a separately issued review grant.
        let prepared = prepare(&store, REVIEWER_TOKEN, "a").await;
        let approved = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                REVIEWER_TOKEN,
                command(&prepared, "separate-review", "review.decide", args(&round)),
            )
            .await
            .unwrap();
        assert_eq!(
            approved["receipt"]["data"]["approval_basis"],
            "agent_review"
        );
        // Reverse issue order: an older review-only grant must not hide reads.
        admin.batch_execute("UPDATE awr_team.agent_authorizations SET created_at_ms=2000 WHERE id='separate-work-grant'").await.unwrap();
        prepare(&store, REVIEWER_TOKEN, "a").await;
        // A child removing StartWork must not fall back to its broader parent.
        let mut child = work_grant.clone();
        child.id = "read-only-child".into();
        child.created_at_ms = 3000;
        child.actions = BTreeSet::from([AuthorizedAction::Inspect]);
        authz
            .delegate(
                TENANT,
                PROJECT,
                &awr_core::DelegateAuthorizationRequest {
                    request_key: "narrow-work".into(),
                    parent_authorization_id: work_grant.id,
                    child,
                },
                4000,
            )
            .await
            .unwrap();
        let prepared = prepare(&store, REVIEWER_TOKEN, "a").await;
        // The independent live Review grant still supports its own session.
        // That lifecycle support must not restore the parent's StartWork.
        let started = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                REVIEWER_TOKEN,
                command(
                    &prepared,
                    "narrowed-start",
                    "session.start",
                    json!({"conversation_id":"review-support"}),
                ),
            )
            .await
            .unwrap();
        assert_eq!(started["receipt"]["execution_authorized"], false);
        let session = started["receipt"]["data"]["session_id"].as_str().unwrap();
        let prepared = prepare(&store, REVIEWER_TOKEN, "a").await;
        let denied = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                REVIEWER_TOKEN,
                command(
                    &prepared,
                    "narrowed-no-claim",
                    "claim.acquire",
                    json!({"session_id":session,"expected_session_version":"1",
                        "expected_work_version":prepared["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
                        "ttl_seconds":60}),
                ),
            )
            .await
            .unwrap_err();
        assert!(matches!(denied, PgError::Forbidden), "{denied:?}");
        // Revoking just the review grant leaves reads available, never review.
        admin
            .batch_execute(
                "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='review-grant'",
            )
            .await
            .unwrap();
        let prepared = prepare(&store, REVIEWER_TOKEN, "a").await;
        let denied = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                REVIEWER_TOKEN,
                command(
                    &prepared,
                    "revoked-no-support-session",
                    "session.start",
                    json!({"conversation_id":"revoked-review"}),
                ),
            )
            .await
            .unwrap_err();
        assert!(matches!(denied, PgError::Forbidden), "{denied:?}");
        let round = open_round(&admin, &store, POLICY, "separate-revoked").await;
        let denied = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                REVIEWER_TOKEN,
                command(&prepared, "revoked-review", "review.decide", args(&round)),
            )
            .await
            .unwrap_err();
        assert!(matches!(denied, PgError::Forbidden), "{denied:?}");
    }

    #[tokio::test]
    async fn agent_review_requires_both_live_grants_and_matching_identity() {
        let (_g, admin, db, store) = setup().await;
        seed_agent_reviewer(&admin, &db).await;
        let round = open_round(&admin, &store, POLICY, "agent-grants").await;
        let original: Value = admin
            .query_one(
                "SELECT body_json FROM awr_team.agent_authorizations WHERE id='review-grant'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        let mutations = [
            (
                "no-member-grant",
                "UPDATE awr_team.project_memberships SET agent_review=false WHERE actor_id='reviewer'",
                "UPDATE awr_team.project_memberships SET agent_review=true WHERE actor_id='reviewer'",
            ),
            (
                "revoked",
                "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='review-grant'",
                "UPDATE awr_team.agent_authorizations SET status='active' WHERE id='review-grant'",
            ),
            (
                "disabled-binding",
                "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='bind-reviewer'",
                "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='bind-reviewer'",
            ),
            (
                "no-write",
                "UPDATE awr_team.workstream_grants SET can_write=false WHERE actor_id='reviewer'",
                "UPDATE awr_team.workstream_grants SET can_write=true WHERE actor_id='reviewer'",
            ),
        ];
        for (key, mutate, restore) in mutations {
            admin.batch_execute(mutate).await.unwrap();
            assert!(
                matches!(
                    denied(&store, key, "review.decide", args(&round)).await,
                    PgError::Forbidden
                ),
                "{key}"
            );
            admin.batch_execute(restore).await.unwrap();
        }
        for (key, field, value) in [
            ("wrong-client", "client_id", json!("other-client")),
            ("wrong-session", "session_id", json!("other-session")),
            ("expired", "expires_at_ms", json!(1)),
            (
                "no-review-delegation",
                "actions",
                json!(["inspect", "start_work"]),
            ),
            (
                "wrong-project",
                "scope",
                json!({"kind":"project","project_id":"other-project"}),
            ),
        ] {
            let mut body = original.clone();
            body[field] = value;
            admin
                .execute(
                    "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='review-grant'",
                    &[&body],
                )
                .await
                .unwrap();
            assert!(
                matches!(
                    denied(&store, key, "review.decide", args(&round)).await,
                    PgError::Forbidden
                ),
                "{key}"
            );
        }
        admin
            .execute(
                "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='review-grant'",
                &[&original],
            )
            .await
            .unwrap();
        // A matching session-bound grant must succeed; mismatched sessions
        // above must fail. Model labels remain informational metadata.
        let mut bound = original.clone();
        bound["session_id"] = json!("session-reviewer");
        bound["model_id"] = json!("declared-model");
        admin
            .execute(
                "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='review-grant'",
                &[&bound],
            )
            .await
            .unwrap();
        let mut q = query("work.prepare");
        q.work_id = Some("a".into());
        q.session_id = Some("session-reviewer".into());
        let prepared = store
            .query(TENANT, PROJECT, REVIEWER_TOKEN, q)
            .await
            .unwrap();
        let ok = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                REVIEWER_TOKEN,
                command(&prepared, "grants-restored", "review.decide", args(&round)),
            )
            .await
            .unwrap()["receipt"]["data"]
            .clone();
        assert_eq!(ok["state"], "approved");
    }

    #[tokio::test]
    async fn agent_review_cannot_satisfy_human_policies_or_review_its_own_work() {
        let (_g, admin, db, store) = setup().await;
        seed_agent_reviewer(&admin, &db).await;
        for (i, policy) in [
            "independent_review",
            "trusted_execution_and_review",
            "trusted_execution_and_author_self_review",
        ]
        .iter()
        .enumerate()
        {
            let round = open_round(&admin, &store, policy, &format!("human-{i}")).await;
            assert!(matches!(
                denied(
                    &store,
                    &format!("human-deny-{i}"),
                    "review.decide",
                    args(&round)
                )
                .await,
                PgError::AuthorCannotReview
            ));
        }
        let round = open_round(&admin, &store, POLICY, "agent-self").await;
        for op in ["review.accept", "review.return"] {
            let mut a = args(&round);
            a.as_object_mut().unwrap().remove("decision");
            assert!(matches!(
                denied(&store, op, op, a).await,
                PgError::Forbidden
            ));
        }
        let rid = round["round_id"].as_str().unwrap();
        for (key, column, value) in [
            ("same-actor", "author_actor_id", "reviewer"),
            ("same-client", "author_client_id", "cli-reviewer"),
        ] {
            let old: String = admin
                .query_one(
                    &format!("SELECT {column} FROM awr_team.review_rounds WHERE id=$1"),
                    &[&rid],
                )
                .await
                .unwrap()
                .get(0);
            admin
                .execute(
                    &format!("UPDATE awr_team.review_rounds SET {column}=$2 WHERE id=$1"),
                    &[&rid, &value],
                )
                .await
                .unwrap();
            assert!(matches!(
                denied(&store, key, "review.decide", args(&round)).await,
                PgError::AuthorCannotReview
            ));
            admin
                .execute(
                    &format!("UPDATE awr_team.review_rounds SET {column}=$2 WHERE id=$1"),
                    &[&rid, &old],
                )
                .await
                .unwrap();
        }
        admin
            .execute(
                "UPDATE awr_team.evidence SET created_by='reviewer' WHERE id=$1",
                &[&round["evidence_id"].as_str().unwrap()],
            )
            .await
            .unwrap();
        assert!(matches!(
            denied(&store, "hidden-author", "review.decide", args(&round)).await,
            PgError::AuthorCannotReview
        ));
        admin
            .execute(
                "UPDATE awr_team.evidence SET created_by='agent' WHERE id=$1",
                &[&round["evidence_id"].as_str().unwrap()],
            )
            .await
            .unwrap();
        let mut contract = current_contract(&admin).await;
        contract.acceptance.push("changed acceptance".into());
        put_contract(&admin, &contract).await;
        assert!(matches!(
            denied(&store, "changed-contract", "review.decide", args(&round)).await,
            PgError::ReviewRequired
        ));
    }
}
