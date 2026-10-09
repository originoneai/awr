#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::*;
use awr_team_pg::{HandoffStore, PgError, ResponsibilityStore};
use common::{fresh_team_schema, test_config, with_app_role};
use std::sync::MutexGuard;
use tokio_postgres::Client;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (MutexGuard<'static, ()>, Client, String, HandoffStore) {
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
    (guard, admin, db, HandoffStore::from_config(cfg))
}

fn digest(n: u8) -> String {
    format!("{n:064x}")
}

fn package(from: &str) -> HandoffPackage {
    HandoffPackage {
        task_id: "work-a".into(),
        contract_version: "3".into(),
        contract_hash: digest(1),
        current_person_id: PersonId::new(from).unwrap(),
        current_execution: ExecutionInstance::Person {
            person_id: PersonId::new(from).unwrap(),
        },
        consumed_context_digest: digest(2),
        checkpoint_ids: vec!["cp-1".into()],
        artifact_versions: vec![],
        branch_id: None,
        working_directory: Some("crates/awr-core".into()),
        dependency_ids: vec![],
        todos: vec!["finish".into()],
        awaiting_replies: vec![],
        unknown_side_effects: vec![],
    }
}

#[tokio::test]
async fn scoped_lookup_masks_missing_and_other_work_handoffs_after_authorization() {
    let (_guard, admin, db, reader) = fixture::setup().await;
    let store = HandoffStore::from_config(with_app_role(&test_config(), &db));
    for (id, work) in [("public-handoff", "a"), ("private-handoff", "b-private")] {
        let mut pkg = package("sender");
        pkg.task_id = work.into();
        store
            .propose(
                fixture::TENANT,
                fixture::PROJECT,
                work,
                &PersonId::new("sender").unwrap(),
                &ProposeHandoffRequest {
                    request_key: format!("propose-{id}"),
                    handoff_id: id.into(),
                    kind: HandoffKind::Execution,
                    package: pkg,
                    to_person_id: PersonId::new("receiver").unwrap(),
                    proposed_successor: None,
                    proposer_execution_id: None,
                    proposer_fence: None,
                    expires_at_ms: None,
                    now_ms: 1_000,
                },
            )
            .await
            .unwrap();
    }
    let mut q = fixture::query("handoff.inspect");
    q.work_id = Some("a".into());
    q.handoff_id = Some("public-handoff".into());
    let result = reader
        .query(fixture::TENANT, fixture::PROJECT, fixture::A, q.clone())
        .await
        .unwrap();
    assert_eq!(result["data"]["handoff"]["id"], "public-handoff");

    for id in ["missing-handoff", "ev-session-a", "private-handoff"] {
        q.handoff_id = Some(id.into());
        let error = reader
            .query(fixture::TENANT, fixture::PROJECT, fixture::A, q.clone())
            .await
            .unwrap_err();
        assert!(error.is_handoff_unavailable(), "{error}");
        assert!(!error.to_string().contains(id));
    }
    // An unauthorized selected work is rejected before looking up any record.
    q.work_id = Some("b-private".into());
    for id in ["missing-handoff", "private-handoff"] {
        q.handoff_id = Some(id.into());
        let error = reader
            .query(fixture::TENANT, fixture::PROJECT, fixture::A, q.clone())
            .await
            .unwrap_err();
        assert!(matches!(error, PgError::Forbidden));
    }
    q.work_id = Some("a".into());
    let error = reader
        .query(fixture::TENANT, fixture::PROJECT, fixture::NONE, q)
        .await
        .unwrap_err();
    assert!(matches!(error, PgError::Forbidden));

    // Retain the body-level work check even if stored metadata is inconsistent.
    admin
        .batch_execute(
            "UPDATE awr_team.team_handoffs SET body_json=jsonb_set(body_json, '{work_item_id}', '\"b-private\"')
             WHERE id='public-handoff'",
        )
        .await
        .unwrap();
    let mut q = fixture::query("handoff.inspect");
    q.work_id = Some("a".into());
    q.handoff_id = Some("public-handoff".into());
    let error = reader
        .query(fixture::TENANT, fixture::PROJECT, fixture::A, q)
        .await
        .unwrap_err();
    assert!(error.is_handoff_unavailable());
}

#[tokio::test]
async fn propose_inspect_accept_reject_timeout_roundtrip() {
    let (_g, admin, _db, store) = setup().await;
    let now: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    let (h, receipt) = store
        .propose(
            TENANT,
            PROJECT,
            "work-a",
            &alice,
            &ProposeHandoffRequest {
                request_key: "prop-1".into(),
                handoff_id: "ho-1".into(),
                kind: HandoffKind::Execution,
                package: package("alice"),
                to_person_id: bob.clone(),
                proposed_successor: Some(ExecutionInstance::Person {
                    person_id: bob.clone(),
                }),
                proposer_execution_id: None,
                proposer_fence: Some(1),
                expires_at_ms: Some(now + 60_000),
                now_ms: 1_000,
            },
        )
        .await
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(h.status, HandoffStatus::Proposed);

    let (h, _) = store
        .inspect(
            TENANT,
            PROJECT,
            &InspectHandoffRequest {
                request_key: "ins-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 1,
                inspector_person_id: bob.clone(),
                now_ms: 1_500,
            },
        )
        .await
        .unwrap();
    assert_eq!(h.status, HandoffStatus::Inspected);

    let (h, _) = store
        .accept(
            TENANT,
            PROJECT,
            &AcceptHandoffRequest {
                request_key: "acc-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 2,
                acceptor_person_id: bob.clone(),
                successor_execution: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 2_000,
            },
        )
        .await
        .unwrap();
    assert_eq!(h.status, HandoffStatus::Accepted);
    let duty = store.duty(TENANT, PROJECT, "ho-1", 2_000).await.unwrap();
    assert!(duty.successor_may_execute);
    assert_eq!(duty.responsible_person_id, alice);

    // Idempotent replay
    let (_, replay) = store
        .accept(
            TENANT,
            PROJECT,
            &AcceptHandoffRequest {
                request_key: "acc-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 2,
                acceptor_person_id: bob.clone(),
                successor_execution: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 3_000,
            },
        )
        .await
        .unwrap();
    assert!(replay.replayed);
}

#[tokio::test]
async fn timeout_preserves_original_without_stop() {
    let (_g, admin, _db, store) = setup().await;
    let now: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    store
        .propose(
            TENANT,
            PROJECT,
            "work-a",
            &alice,
            &ProposeHandoffRequest {
                request_key: "prop-to".into(),
                handoff_id: "ho-to".into(),
                kind: HandoffKind::Execution,
                package: package("alice"),
                to_person_id: bob,
                proposed_successor: None,
                proposer_execution_id: None,
                proposer_fence: None,
                expires_at_ms: Some(now + 60_000),
                now_ms: 1_000,
            },
        )
        .await
        .unwrap();
    admin
        .execute(
            "UPDATE awr_team.team_handoffs SET expires_at_ms=$1,
        body_json=jsonb_set(body_json,'{expires_at_ms}',to_jsonb($1::bigint)) WHERE id='ho-to'",
            &[&(now - 1)],
        )
        .await
        .unwrap();
    let (h, _) = store
        .timeout(
            TENANT,
            PROJECT,
            &TimeoutHandoffRequest {
                request_key: "to-1".into(),
                handoff_id: "ho-to".into(),
                expected_version: 1,
                now_ms: 5_000,
            },
        )
        .await
        .unwrap();
    assert_eq!(h.status, HandoffStatus::TimedOut);
    let duty = h.duty_at(5_000).unwrap();
    assert_eq!(duty.responsible_person_id, alice);
    assert!(!duty.successor_may_execute);
    assert!(duty.note.contains("not stop"));
}

#[tokio::test]
async fn accept_transfers_responsibility_owner_and_execution_claim() {
    let (_g, admin, db, store) = setup().await;
    let now: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    let resp = ResponsibilityStore::from_config(with_app_role(&test_config(), &db));
    resp.ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    resp.ensure_person(TENANT, PROJECT, bob.as_str(), "Bob")
        .await
        .unwrap();
    let (assigned, _) = resp
        .assign(
            TENANT,
            PROJECT,
            "work-a",
            &AssignResponsibilityRequest {
                request_key: "own-1".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .await
        .unwrap();
    resp.claim_execution(
        TENANT,
        PROJECT,
        "work-a",
        &ClaimExecutionRequest {
            request_key: "exec-1".into(),
            expected_version: assigned.version,
            executor: ExecutionInstance::Person {
                person_id: alice.clone(),
            },
            coordination_claim_id: None,
        },
    )
    .await
    .unwrap();

    // Active coordination claim for alice.
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
                VALUES ('tenant-a','alice','human','Alice','active');
             INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                VALUES ('tenant-a','project-a','main','Main','active');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                VALUES ('tenant-a','project-a','work-a','work-a'),
                       ('tenant-a','project-a','work-b','work-b');
             INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
                VALUES ('tenant-a','project-a','main','work-a','active',1,1);
             INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
                VALUES ('tenant-a','project-a','sess-a','main','work-a','alice','cli-a','conv-a','active');
             INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,expires_at,state)
                VALUES ('tenant-a','project-a','claim-a','main','work-a','sess-a','alice',1,clock_timestamp()+interval '1 hour','active');",
        )
        .await
        .unwrap();

    store
        .propose(
            TENANT,
            PROJECT,
            "work-a",
            &alice,
            &ProposeHandoffRequest {
                request_key: "prop-resp".into(),
                handoff_id: "ho-resp".into(),
                kind: HandoffKind::Responsibility,
                package: package("alice"),
                to_person_id: bob.clone(),
                proposed_successor: Some(ExecutionInstance::Person {
                    person_id: bob.clone(),
                }),
                proposer_execution_id: None,
                proposer_fence: None,
                expires_at_ms: Some(now + 60_000),
                now_ms: 1_000,
            },
        )
        .await
        .unwrap();
    store
        .accept(
            TENANT,
            PROJECT,
            &AcceptHandoffRequest {
                request_key: "acc-resp".into(),
                handoff_id: "ho-resp".into(),
                expected_version: 1,
                acceptor_person_id: bob.clone(),
                successor_execution: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: Some(1),
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 2_000,
            },
        )
        .await
        .unwrap();
    let after = resp.get(TENANT, PROJECT, "work-a").await.unwrap();
    assert_eq!(
        after.owner,
        Some(bob.clone()),
        "responsibility owner must move"
    );

    // Fresh execution handoff moves executor + closes claim.
    let (assigned2, _) = resp
        .assign(
            TENANT,
            PROJECT,
            "work-b",
            &AssignResponsibilityRequest {
                request_key: "own-b".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .await
        .unwrap();
    resp.claim_execution(
        TENANT,
        PROJECT,
        "work-b",
        &ClaimExecutionRequest {
            request_key: "exec-b".into(),
            expected_version: assigned2.version,
            executor: ExecutionInstance::Person {
                person_id: alice.clone(),
            },
            coordination_claim_id: None,
        },
    )
    .await
    .unwrap();
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
                VALUES ('tenant-a','project-a','main','work-b','active',1,3);
             INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
                VALUES ('tenant-a','project-a','sess-b','main','work-b','alice','cli-a','conv-b','active');
             INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,expires_at,state)
                VALUES ('tenant-a','project-a','claim-b','main','work-b','sess-b','alice',3,clock_timestamp()+interval '1 hour','active');",
        )
        .await
        .unwrap();
    let mut pkg = package("alice");
    pkg.task_id = "work-b".into();
    store
        .propose(
            TENANT,
            PROJECT,
            "work-b",
            &alice,
            &ProposeHandoffRequest {
                request_key: "prop-ex".into(),
                handoff_id: "ho-ex".into(),
                kind: HandoffKind::Execution,
                package: pkg,
                to_person_id: bob.clone(),
                proposed_successor: Some(ExecutionInstance::Person {
                    person_id: bob.clone(),
                }),
                proposer_execution_id: None,
                proposer_fence: Some(3),
                expires_at_ms: Some(now + 60_000),
                now_ms: 1_000,
            },
        )
        .await
        .unwrap();
    store
        .accept(
            TENANT,
            PROJECT,
            &AcceptHandoffRequest {
                request_key: "acc-ex".into(),
                handoff_id: "ho-ex".into(),
                expected_version: 1,
                acceptor_person_id: bob.clone(),
                successor_execution: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: Some(3),
                live_fence: Some(3),
                unknown_executions_open: false,
                now_ms: 2_000,
            },
        )
        .await
        .unwrap();
    let exec_after = resp.get(TENANT, PROJECT, "work-b").await.unwrap();
    assert_eq!(
        exec_after.owner,
        Some(alice),
        "execution handoff keeps ownership"
    );
    assert_eq!(
        exec_after.current_executor.as_ref().map(|e| e.person_id()),
        Some(&bob),
        "executor must transfer"
    );
    let claim_state: String = admin
        .query_one("SELECT state FROM awr_team.claims WHERE id='claim-b'", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(claim_state, "handed_off");
    let fence: i64 = admin
        .query_one(
            "SELECT last_fence FROM awr_team.work_runtime WHERE work_id='work-b'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(fence > 3, "fence must bump so stale writes fail");
}
