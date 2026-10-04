#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::*;
use awr_team_pg::{HandoffStore, PgError, ResponsibilityStore, WorkstreamCommandStore};
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
    let (_g, _admin, _db, store) = setup().await;
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
                expires_at_ms: Some(10_000),
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
    let (_g, _admin, _db, store) = setup().await;
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
                expires_at_ms: Some(5_000),
                now_ms: 1_000,
            },
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
                expires_at_ms: Some(10_000),
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
                expected_current_fence: None,
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
                expires_at_ms: Some(10_000),
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

#[tokio::test]
async fn authenticated_accept_requires_receiver_credential() {
    let (_guard, admin, db, store) = fixture::setup().await;
    fixture::enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1
             WHERE client_id='cli-b';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('reader-tenant','reader-project','alice','Alice','active'),
                ('reader-tenant','reader-project','bob','Bob','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
                VALUES ('reader-tenant','reader-project','bind-alice','alice','agent','active');
             INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
                VALUES ('reader-tenant','bob','human','Bob','active')
                ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
                VALUES ('reader-tenant','reader-project','bob','admin')
                ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    // Point credential B at human bob so acceptor identity is bob.
    admin
        .execute(
            "UPDATE awr_team.credentials SET actor_id='bob' WHERE id='reader-b'",
            &[],
        )
        .await
        .unwrap();

    let commands = WorkstreamCommandStore::from_config(with_app_role(&test_config(), &db));
    let prepared = fixture::prepare(&store, fixture::A, "a").await;
    let start = fixture::command(
        &prepared,
        "sess-start-a",
        "session.start",
        serde_json::json!({"conversation_id":"handoff-alice"}),
    );
    let started = commands
        .execute(fixture::TENANT, fixture::PROJECT, fixture::A, start)
        .await
        .unwrap();
    let sess_a = started["receipt"]["data"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Bob needs a write grant on stream 1 (work a). Grant may be stream 2 only for cli-b.
    admin
        .batch_execute(
            "INSERT INTO awr_team.workstream_grants(
                tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
             VALUES ('reader-tenant','reader-project','bob','cli-b',
                     '00000000000000000000000001',1,true,true)
             ON CONFLICT DO NOTHING;",
        )
        .await
        .ok();
    let prepared_b = fixture::prepare(&store, fixture::B, "a").await;
    let start_b = fixture::command(
        &prepared_b,
        "sess-start-b",
        "session.start",
        serde_json::json!({"conversation_id":"handoff-bob"}),
    );
    let started_b = commands
        .execute(fixture::TENANT, fixture::PROJECT, fixture::B, start_b)
        .await
        .unwrap();
    let sess_b = started_b["receipt"]["data"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let current = fixture::prepare(&store, fixture::A, "a").await;
    let digest = "0000000000000000000000000000000000000000000000000000000000000001";
    let propose = fixture::command(
        &current,
        "ho-propose",
        "handoff.propose",
        serde_json::json!({
            "session_id": sess_a,
            "expected_session_version": "1",
            "handoff_id": "ho-auth",
            "kind": "responsibility",
            "to_person_id": "bob",
            "package": {
                "task_id": "a",
                "contract_version": "3",
                "contract_hash": digest,
                "current_person_id": "alice",
                "current_execution": {"kind":"person","person_id":"alice"},
                "consumed_context_digest": digest,
                "checkpoint_ids": ["cp-1"],
                "artifact_versions": [],
                "dependency_ids": [],
                "todos": [],
                "awaiting_replies": [],
                "unknown_side_effects": []
            },
            "proposed_successor": {"kind":"person","person_id":"bob"},
            "now_ms": 1000,
            "expires_at_ms": 100000
        }),
    );
    let proposed = commands
        .execute(fixture::TENANT, fixture::PROJECT, fixture::A, propose)
        .await
        .unwrap();
    assert_eq!(proposed["receipt"]["data"]["status"], "proposed");
    let version = proposed["receipt"]["data"]["version"]
        .as_str()
        .unwrap()
        .to_string();

    // Same alice credential forges acceptor_person_id=bob.
    let current_a = fixture::prepare(&store, fixture::A, "a").await;
    let missing = fixture::command(
        &current_a,
        "inspect-missing-handoff",
        "handoff.inspect",
        serde_json::json!({
            "session_id": sess_a,
            "expected_session_version": "1",
            "handoff_id": "not-a-handoff-record",
            "expected_handoff_version": "1",
            "inspector_person_id": "alice",
            "now_ms": 1500
        }),
    );
    let missing_error = commands
        .execute(fixture::TENANT, fixture::PROJECT, fixture::A, missing)
        .await
        .unwrap_err();
    assert!(missing_error.is_handoff_unavailable());
    let forged = fixture::command(
        &current_a,
        "ho-forged",
        "handoff.accept",
        serde_json::json!({
            "session_id": sess_a,
            "expected_session_version": "1",
            "handoff_id": "ho-auth",
            "expected_handoff_version": version,
            "acceptor_person_id": "bob",
            "successor_execution": {"kind":"person","person_id":"bob"},
            "prior_execution_stopped": true,
            "prior_reconciled": false,
            "context_reprepared": true,
            "now_ms": 2000
        }),
    );
    let err = commands
        .execute(fixture::TENANT, fixture::PROJECT, fixture::A, forged)
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Forbidden),
        "proposer must not accept as receiver: {err:?}"
    );

    // A settled or expired original claim still leaves runtime fencing in place.
    admin.execute(
        "INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence) VALUES ($1,$2,'main','a','unclaimed',1,7)",
        &[&fixture::TENANT, &fixture::PROJECT],
    ).await.unwrap();
    let current_b = fixture::prepare(&store, fixture::B, "a").await;
    assert_eq!(current_b["data"]["runtime"]["last_fence"], "7");
    let missing_fence = fixture::command(
        &current_b,
        "ho-missing-fence",
        "handoff.accept",
        serde_json::json!({
            "session_id":sess_b, "expected_session_version":"1", "handoff_id":"ho-auth",
            "expected_handoff_version":version, "acceptor_person_id":"bob",
            "successor_execution":{"kind":"person","person_id":"bob"},
            "prior_execution_stopped":true, "prior_reconciled":false, "context_reprepared":true, "now_ms":2000
        }),
    );
    let err = commands
        .execute(fixture::TENANT, fixture::PROJECT, fixture::B, missing_fence)
        .await
        .unwrap_err();
    assert!(
        err.is_missing_handoff_fence(),
        "A required runtime fence must have actionable diagnostics: {err:?}"
    );
    let handoff = admin.query_one(
        "SELECT status,version FROM awr_team.team_handoffs WHERE tenant_id=$1 AND project_id=$2 AND id='ho-auth'",
        &[&fixture::TENANT,&fixture::PROJECT],
    ).await.unwrap();
    assert_eq!(handoff.get::<_, String>(0), "proposed");
    assert_eq!(handoff.get::<_, i64>(1).to_string(), version);

    let current_b = fixture::prepare(&store, fixture::B, "a").await;
    let accept = fixture::command(
        &current_b,
        "ho-accept-bob",
        "handoff.accept",
        serde_json::json!({
            "session_id": sess_b,
            "expected_session_version": "1",
            "handoff_id": "ho-auth",
            "expected_handoff_version": version,
            "acceptor_person_id": "bob",
            "successor_execution": {"kind":"person","person_id":"bob"},
            "prior_execution_stopped": true,
            "prior_reconciled": false,
            "context_reprepared": true,
            "expected_current_fence": "7",
            "now_ms": 2000
        }),
    );
    let ok = commands
        .execute(fixture::TENANT, fixture::PROJECT, fixture::B, accept)
        .await
        .unwrap();
    assert_eq!(ok["receipt"]["data"]["status"], "accepted");
}
