#![cfg(feature = "pg-tests")]
//! WS-015 responsibility / identity / assignment on real PostgreSQL.
mod common;
use awr_core::*;
use awr_team_pg::{PgError, ResponsibilityStore};
use common::{app_client, fresh_team_schema, test_config, with_app_role};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::sync::MutexGuard;
use tokio_postgres::Client;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (MutexGuard<'static, ()>, Client, ResponsibilityStore) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');",
        )
        .await
        .unwrap();
    let store = ResponsibilityStore::from_config(with_app_role(&test_config(), &db));
    (guard, admin, store)
}

#[tokio::test]
async fn claim_does_not_steal_ownership_and_receipts_replay() {
    let (_g, _admin, store) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    store
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    store
        .ensure_person(TENANT, PROJECT, bob.as_str(), "Bob")
        .await
        .unwrap();
    let (assigned, receipt) = store
        .assign(
            TENANT,
            PROJECT,
            "work-a",
            &AssignResponsibilityRequest {
                request_key: "asg-1".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![bob.clone()],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(assigned.owner, Some(alice.clone()));
    let (_, replay) = store
        .assign(
            TENANT,
            PROJECT,
            "work-a",
            &AssignResponsibilityRequest {
                request_key: "asg-1".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![bob.clone()],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.event_id, receipt.event_id);

    let (claimed, _) = store
        .claim_execution(
            TENANT,
            PROJECT,
            "work-a",
            &ClaimExecutionRequest {
                request_key: "exec-1".into(),
                expected_version: assigned.version,
                executor: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                coordination_claim_id: Some("coord-1".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(claimed.owner, Some(alice));
    assert_eq!(claimed.current_executor.unwrap().person_id(), &bob);
}

#[tokio::test]
async fn agent_swap_requires_explicit_binding_not_actor_kind() {
    let (_g, admin, store) = setup().await;
    // Seed an actor.kind=agent that must NOT be treated as a person↔agent binding.
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
             VALUES ('tenant-a','agent-ghost','agent','Ghost','active');",
        )
        .await
        .unwrap();
    let alice = PersonId::new("alice").unwrap();
    store
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    let err = store
        .claim_execution(
            TENANT,
            PROJECT,
            "work-b",
            &ClaimExecutionRequest {
                request_key: "bad".into(),
                expected_version: 0,
                executor: ExecutionInstance::AgentRun {
                    person_id: alice.clone(),
                    agent_id: "agent-ghost".into(),
                    binding_id: "missing".into(),
                },
                coordination_claim_id: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, awr_team_pg::PgError::Forbidden));

    store
        .bind_person_agent(
            TENANT,
            PROJECT,
            &PersonAgentBinding {
                id: "bind-1".into(),
                person_id: alice.clone(),
                agent_id: "agent-ghost".into(),
                status: BindingStatus::Active,
                created_at_ms: 1,
            },
        )
        .await
        .unwrap();
    let (assigned, _) = store
        .assign(
            TENANT,
            PROJECT,
            "work-b",
            &AssignResponsibilityRequest {
                request_key: "own".into(),
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
    let (claimed, _) = store
        .claim_execution(
            TENANT,
            PROJECT,
            "work-b",
            &ClaimExecutionRequest {
                request_key: "ok".into(),
                expected_version: assigned.version,
                executor: ExecutionInstance::AgentRun {
                    person_id: alice.clone(),
                    agent_id: "agent-ghost".into(),
                    binding_id: "bind-1".into(),
                },
                coordination_claim_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(claimed.owner, Some(alice));
}

#[tokio::test]
async fn responsibility_rls_blocks_unscoped_and_cross_tenant_app_reads() {
    let (_g, admin, store) = setup().await;
    // Seed a second tenant/project as superuser (bypasses FORCE RLS).
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-b','B','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-b','project-b','beta','team','epoch-1','active');
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
                VALUES ('tenant-b','project-b','eve','Eve','active');
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
                VALUES ('tenant-a','project-a','alice','Alice','active');",
        )
        .await
        .unwrap();

    // Store path with correct tenant scope can read its own person via get after assign.
    let alice = PersonId::new("alice").unwrap();
    store
        .assign(
            TENANT,
            PROJECT,
            "work-rls",
            &AssignResponsibilityRequest {
                request_key: "rls-asg".into(),
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
    let mine = store.get(TENANT, PROJECT, "work-rls").await.unwrap();
    assert_eq!(mine.owner, Some(alice));

    // Unscoped app role must not see any persons (including other tenants).
    let mut app = {
        // Reconnect as awr_app against the same DB the store uses.
        let url = std::env::var("AWR_TEAM_TEST_DATABASE_URL").ok();
        let _ = url;
        // Pull db name from admin connection via current_database.
        let db: String = admin
            .query_one("SELECT current_database()", &[])
            .await
            .unwrap()
            .get(0);
        app_client(&db).await
    };
    let leaked: i64 = app
        .query_one("SELECT count(*) FROM awr_team.persons", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        leaked, 0,
        "unscoped app must not read persons across tenants"
    );

    // Wrong-tenant scope must not reveal tenant-b rows.
    let tx = app.transaction().await.unwrap();
    tx.execute("SELECT set_config('awr.tenant_id', $1, true)", &[&TENANT])
        .await
        .unwrap();
    tx.execute("SELECT set_config('awr.project_id', $1, true)", &[&PROJECT])
        .await
        .unwrap();
    let cross: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.persons WHERE tenant_id='tenant-b'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(cross, 0);
    let visible: i64 = tx
        .query_one("SELECT count(*) FROM awr_team.persons", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(visible, 1);
    tx.commit().await.unwrap();

    // Append-only: app cannot UPDATE/DELETE responsibility_events / receipts.
    let scoped = app.transaction().await.unwrap();
    scoped
        .execute("SELECT set_config('awr.tenant_id', $1, true)", &[&TENANT])
        .await
        .unwrap();
    scoped
        .execute("SELECT set_config('awr.project_id', $1, true)", &[&PROJECT])
        .await
        .unwrap();
    assert!(
        scoped
            .execute(
                "UPDATE awr_team.responsibility_events SET event_type='tamper'",
                &[]
            )
            .await
            .is_err()
    );
    assert!(
        scoped
            .execute("DELETE FROM awr_team.responsibility_receipts", &[])
            .await
            .is_err()
    );
    scoped.rollback().await.unwrap();
}

#[tokio::test]
async fn request_key_cannot_replay_onto_a_different_work_item() {
    let (_g, _admin, store) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    store
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    store
        .assign(
            TENANT,
            PROJECT,
            "work-a",
            &AssignResponsibilityRequest {
                request_key: "same-key".into(),
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
    let err = store
        .assign(
            TENANT,
            PROJECT,
            "work-b",
            &AssignResponsibilityRequest {
                request_key: "same-key".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, awr_team_pg::PgError::IdempotencyConflict));
    let other = store.get(TENANT, PROJECT, "work-b").await.unwrap();
    assert!(other.owner.is_none());
    assert_eq!(other.version, 0);
}

#[tokio::test]
async fn concurrent_first_assigns_do_not_overwrite_each_other() {
    let (_g, _admin, store) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    store
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    store
        .ensure_person(TENANT, PROJECT, bob.as_str(), "Bob")
        .await
        .unwrap();
    let alice_req = AssignResponsibilityRequest {
        request_key: "race-alice".into(),
        expected_version: 0,
        owner: Some(alice.clone()),
        collaborators: vec![],
        independent_reviewer: None,
        allow_unassigned: false,
        authorized_by: alice.clone(),
    };
    let bob_req = AssignResponsibilityRequest {
        request_key: "race-bob".into(),
        expected_version: 0,
        owner: Some(bob.clone()),
        collaborators: vec![],
        independent_reviewer: None,
        allow_unassigned: false,
        authorized_by: bob.clone(),
    };
    let left = store.assign(TENANT, PROJECT, "work-race", &alice_req);
    let right = store.assign(TENANT, PROJECT, "work-race", &bob_req);
    let (left, right) = tokio::join!(left, right);
    let wins = [&left, &right].iter().filter(|r| r.is_ok()).count();
    assert_eq!(wins, 1, "left={left:?} right={right:?}");
    let task = store.get(TENANT, PROJECT, "work-race").await.unwrap();
    assert_eq!(task.version, 1);
    assert!(task.owner == Some(alice) || task.owner == Some(bob));
}

fn person(id: &str) -> PersonId {
    PersonId::new(id).unwrap()
}

fn assignment(key: &str, version: u64, owner: &str) -> AssignResponsibilityRequest {
    AssignResponsibilityRequest {
        request_key: key.into(),
        expected_version: version,
        owner: Some(person(owner)),
        collaborators: vec![],
        independent_reviewer: Some(person("reviewer")),
        allow_unassigned: false,
        authorized_by: person("supervisor"),
    }
}

fn available(key: &str, version: u64, claimant: &str) -> ClaimAvailableRequest {
    ClaimAvailableRequest {
        request_key: key.into(),
        expected_version: version,
        claimant: person(claimant),
    }
}

fn changed<T: Serialize + DeserializeOwned>(request: &T, field: &str, value: Value) -> T {
    let mut input = serde_json::to_value(request).unwrap();
    input[field] = value;
    serde_json::from_value(input).unwrap()
}

fn conflict<T: std::fmt::Debug>(result: std::result::Result<T, PgError>) {
    assert!(
        matches!(result, Err(PgError::IdempotencyConflict)),
        "{result:?}"
    );
}

async fn snapshot(admin: &Client) -> Value {
    admin.query_one(
        "SELECT jsonb_build_object(
            'persons', COALESCE((SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM awr_team.persons p), '[]'::jsonb),
            'tasks', COALESCE((SELECT jsonb_agg(to_jsonb(t) ORDER BY work_id) FROM awr_team.task_responsibilities t), '[]'::jsonb),
            'collaborators', COALESCE((SELECT jsonb_agg(to_jsonb(c) ORDER BY work_id,person_id) FROM awr_team.task_collaborators c), '[]'::jsonb),
            'events', COALESCE((SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.responsibility_events e), '[]'::jsonb),
            'receipts', COALESCE((SELECT jsonb_agg(to_jsonb(r) ORDER BY request_key) FROM awr_team.responsibility_receipts r), '[]'::jsonb))",
        &[],
    ).await.unwrap().get(0)
}

#[tokio::test]
async fn assignment_binds_every_field_and_replay_returns_current_projection() {
    let (_g, admin, store) = setup().await;
    let request = assignment("assign-complete", 0, "alice");
    let (_, original) = store
        .assign(TENANT, PROJECT, "task", &request)
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for (field, value) in [
        ("expected_version", json!(1)),
        ("owner", json!("bob")),
        ("collaborators", json!(["bob"])),
        ("independent_reviewer", Value::Null),
        ("allow_unassigned", json!(true)),
        ("authorized_by", json!("other-supervisor")),
    ] {
        conflict(
            store
                .assign(TENANT, PROJECT, "task", &changed(&request, field, value))
                .await,
        );
        assert_eq!(
            snapshot(&admin).await,
            before,
            "changed {field} mutated data"
        );
    }
    let (latest, _) = store
        .assign(
            TENANT,
            PROJECT,
            "task",
            &assignment("assign-next", 1, "bob"),
        )
        .await
        .unwrap();
    let before_replay = snapshot(&admin).await;
    let (current, replay) = store
        .assign(TENANT, PROJECT, "task", &request)
        .await
        .unwrap();
    assert_eq!(current, latest);
    assert_eq!(current.owner, Some(person("bob")));
    assert_eq!(replay.event_id, original.event_id);
    assert_eq!((replay.version_before, replay.version_after), (0, 1));
    assert!(replay.replayed);
    assert_eq!(snapshot(&admin).await, before_replay);
}

#[tokio::test]
async fn responsibility_operations_bind_full_requests_and_distinct_discriminators() {
    let (_g, admin, store) = setup().await;
    store
        .assign(TENANT, PROJECT, "task", &assignment("assign", 0, "alice"))
        .await
        .unwrap();
    let accept = AcceptResponsibilityRequest {
        request_key: "accept".into(),
        expected_version: 1,
        acceptor: person("alice"),
        as_owner: true,
    };
    let (_, accepted) = store
        .accept(TENANT, PROJECT, "task", &accept)
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for (field, value) in [
        ("expected_version", json!(2)),
        ("acceptor", json!("bob")),
        ("as_owner", json!(false)),
    ] {
        conflict(
            store
                .accept(TENANT, PROJECT, "task", &changed(&accept, field, value))
                .await,
        );
        assert_eq!(snapshot(&admin).await, before);
    }
    let claim = ClaimExecutionRequest {
        request_key: "execute".into(),
        expected_version: 2,
        executor: ExecutionInstance::Person {
            person_id: person("alice"),
        },
        coordination_claim_id: Some("coordination".into()),
    };
    let (_, claimed) = store
        .claim_execution(TENANT, PROJECT, "task", &claim)
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for (field, value) in [
        ("expected_version", json!(3)),
        ("executor", json!({"kind":"person","person_id":"bob"})),
        ("coordination_claim_id", json!("other-claim")),
    ] {
        conflict(
            store
                .claim_execution(TENANT, PROJECT, "task", &changed(&claim, field, value))
                .await,
        );
        assert_eq!(snapshot(&admin).await, before);
    }
    conflict(
        store
            .swap_agent(
                TENANT,
                PROJECT,
                "task",
                "execute",
                2,
                &person("alice"),
                claim.executor.clone(),
            )
            .await,
    );
    assert_eq!(snapshot(&admin).await, before);
    let (_, replay) = store
        .claim_execution(TENANT, PROJECT, "task", &claim)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.event_id, claimed.event_id);

    let (_, released) = store
        .release_execution(TENANT, PROJECT, "task", "release", 3, &person("alice"))
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    conflict(
        store
            .release_execution(TENANT, PROJECT, "task", "release", 4, &person("alice"))
            .await,
    );
    conflict(
        store
            .release_execution(TENANT, PROJECT, "task", "release", 3, &person("bob"))
            .await,
    );
    assert_eq!(snapshot(&admin).await, before);
    let (_, replay) = store
        .release_execution(TENANT, PROJECT, "task", "release", 3, &person("alice"))
        .await
        .unwrap();
    assert_eq!(replay.event_id, released.event_id);
    assert!(replay.replayed);

    let executor = ExecutionInstance::Person {
        person_id: person("alice"),
    };
    let (_, swapped) = store
        .swap_agent(
            TENANT,
            PROJECT,
            "task",
            "swap",
            4,
            &person("alice"),
            executor.clone(),
        )
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for (version, actor, next) in [
        (5, person("alice"), executor.clone()),
        (4, person("bob"), executor.clone()),
        (
            4,
            person("alice"),
            ExecutionInstance::Person {
                person_id: person("bob"),
            },
        ),
    ] {
        conflict(
            store
                .swap_agent(TENANT, PROJECT, "task", "swap", version, &actor, next)
                .await,
        );
        assert_eq!(snapshot(&admin).await, before);
    }
    conflict(
        store
            .claim_execution(
                TENANT,
                PROJECT,
                "task",
                &ClaimExecutionRequest {
                    request_key: "swap".into(),
                    expected_version: 4,
                    executor: executor.clone(),
                    coordination_claim_id: None,
                },
            )
            .await,
    );
    let (_, replay) = store
        .swap_agent(
            TENANT,
            PROJECT,
            "task",
            "swap",
            4,
            &person("alice"),
            executor,
        )
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.event_id, swapped.event_id);

    let transfer = TransferOwnerRequest {
        request_key: "transfer".into(),
        expected_version: 5,
        from_owner: person("alice"),
        to_owner: person("bob"),
        authorized_by: person("alice"),
    };
    let (_, transferred) = store
        .transfer_owner(TENANT, PROJECT, "task", &transfer)
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for (field, value) in [
        ("expected_version", json!(6)),
        ("from_owner", json!("bob")),
        ("to_owner", json!("carol")),
        ("authorized_by", json!("supervisor")),
    ] {
        conflict(
            store
                .transfer_owner(TENANT, PROJECT, "task", &changed(&transfer, field, value))
                .await,
        );
        assert_eq!(snapshot(&admin).await, before);
    }
    let (_, replay) = store
        .transfer_owner(TENANT, PROJECT, "task", &transfer)
        .await
        .unwrap();
    assert_eq!(replay.event_id, transferred.event_id);
    assert!(replay.replayed);

    let pending = ResponsibilityPending {
        kind: ResponsibilityPendingKind::Disabled,
        person_id: Some(person("bob")),
        legacy_ref: Some("legacy".into()),
        transfer_request_key: Some("transfer".into()),
        detail: "awaiting member reactivation".into(),
    };
    let (latest, marked) = store
        .mark_pending(TENANT, PROJECT, "task", "pending", 6, pending.clone())
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for (field, value) in [
        ("kind", json!("departure")),
        ("person_id", json!("carol")),
        ("legacy_ref", json!("different-legacy")),
        ("transfer_request_key", json!("different-transfer")),
        ("detail", json!("different reason")),
    ] {
        conflict(
            store
                .mark_pending(
                    TENANT,
                    PROJECT,
                    "task",
                    "pending",
                    6,
                    changed(&pending, field, value),
                )
                .await,
        );
        assert_eq!(snapshot(&admin).await, before);
    }
    conflict(
        store
            .mark_pending(TENANT, PROJECT, "task", "pending", 7, pending.clone())
            .await,
    );
    let (_, replay) = store
        .mark_pending(TENANT, PROJECT, "task", "pending", 6, pending)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.event_id, marked.event_id);
    let (current, replay) = store
        .accept(TENANT, PROJECT, "task", &accept)
        .await
        .unwrap();
    assert_eq!(current, latest);
    assert_eq!(replay.event_id, accepted.event_id);
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn available_claim_preserves_roles_and_receipt_binding() {
    let (_g, admin, store) = setup().await;
    let mut pool = assignment("pool", 0, "alice");
    pool.owner = None;
    pool.allow_unassigned = true;
    pool.collaborators = vec![person("alice"), person("bob")];
    store.assign(TENANT, PROJECT, "task", &pool).await.unwrap();
    let request = available("available", 1, "alice");
    let (task, receipt) = store
        .claim_available(TENANT, PROJECT, "task", &request)
        .await
        .unwrap();
    assert_eq!(task.owner, Some(person("alice")));
    assert_eq!(task.collaborators, vec![person("bob")]);
    assert_eq!(task.independent_reviewer, Some(person("reviewer")));
    assert!(task.current_executor.is_none());
    assert_eq!(receipt.op, ResponsibilityEventType::AvailableClaimed);
    let before = snapshot(&admin).await;
    for (field, value) in [("claimant", json!("bob")), ("expected_version", json!(2))] {
        conflict(
            store
                .claim_available(TENANT, PROJECT, "task", &changed(&request, field, value))
                .await,
        );
        assert_eq!(snapshot(&admin).await, before);
    }
    let (current, replay) = store
        .claim_available(TENANT, PROJECT, "task", &request)
        .await
        .unwrap();
    assert_eq!(current, task);
    assert_eq!(replay.event_id, receipt.event_id);
    assert!(replay.replayed);
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn reserved_executing_and_reviewer_tasks_are_not_available() {
    let (_g, admin, store) = setup().await;
    store
        .mark_pending(
            TENANT,
            PROJECT,
            "reserved",
            "reservation",
            0,
            ResponsibilityPending {
                kind: ResponsibilityPendingKind::NoAcceptor,
                person_id: Some(person("alice")),
                legacy_ref: None,
                transfer_request_key: Some("dispatch".into()),
                detail: "awaiting acceptance".into(),
            },
        )
        .await
        .unwrap();
    store
        .claim_execution(
            TENANT,
            PROJECT,
            "executing",
            &ClaimExecutionRequest {
                request_key: "execution".into(),
                expected_version: 0,
                executor: ExecutionInstance::Person {
                    person_id: person("alice"),
                },
                coordination_claim_id: None,
            },
        )
        .await
        .unwrap();
    let mut pool = assignment("reviewer-pool", 0, "alice");
    pool.owner = None;
    pool.allow_unassigned = true;
    store
        .assign(TENANT, PROJECT, "reviewing", &pool)
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for task in ["reserved", "executing"] {
        assert!(matches!(
            store
                .claim_available(TENANT, PROJECT, task, &available(task, 1, "bob"))
                .await,
            Err(PgError::ClaimHeld)
        ));
    }
    assert!(matches!(
        store
            .claim_available(
                TENANT,
                PROJECT,
                "reviewing",
                &available("reviewer-claim", 1, "reviewer")
            )
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn competing_available_claims_and_assignment_have_one_winner() {
    let (_g, admin, store) = setup().await;
    let first = available("first", 0, "alice");
    let second = available("second", 0, "bob");
    let (left, right) = tokio::join!(
        store.claim_available(TENANT, PROJECT, "race", &first),
        store.claim_available(TENANT, PROJECT, "race", &second),
    );
    assert_eq!(
        [&left, &right].iter().filter(|r| r.is_ok()).count(),
        1,
        "{left:?} {right:?}"
    );
    assert!(
        matches!(left, Err(PgError::PreconditionsChanged))
            || matches!(right, Err(PgError::PreconditionsChanged))
    );
    let task = store.get(TENANT, PROJECT, "race").await.unwrap();
    assert_eq!(task.version, 1);
    assert!(task.owner == Some(person("alice")) || task.owner == Some(person("bob")));
    let dispatched = assignment("dispatch", 0, "assigned-member");
    let self_claim = available("self-claim", 0, "pool-member");
    let (left, right) = tokio::join!(
        store.assign(TENANT, PROJECT, "dispatch-race", &dispatched),
        store.claim_available(TENANT, PROJECT, "dispatch-race", &self_claim),
    );
    assert_eq!(
        [&left, &right].iter().filter(|r| r.is_ok()).count(),
        1,
        "{left:?} {right:?}"
    );
    let task = store.get(TENANT, PROJECT, "dispatch-race").await.unwrap();
    assert_eq!(task.version, 1);
    let data = snapshot(&admin).await;
    assert_eq!(data["events"].as_array().unwrap().len(), 2);
    assert_eq!(data["receipts"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn concurrent_exact_replay_commits_one_event_and_receipt() {
    let (_g, admin, store) = setup().await;
    let request = available("same-request", 0, "alice");
    let (left, right) = tokio::join!(
        store.claim_available(TENANT, PROJECT, "task", &request),
        store.claim_available(TENANT, PROJECT, "task", &request),
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.0, right.0);
    assert_eq!(left.1.event_id, right.1.event_id);
    assert_ne!(left.1.replayed, right.1.replayed);
    let data = snapshot(&admin).await;
    assert_eq!(data["events"].as_array().unwrap().len(), 1);
    assert_eq!(data["receipts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_cross_task_request_key_reuse_returns_domain_conflict() {
    let (_g, admin, store) = setup().await;
    let request = available("shared-key", 0, "alice");
    let (left, right) = tokio::join!(
        store.claim_available(TENANT, PROJECT, "first-task", &request),
        store.claim_available(TENANT, PROJECT, "second-task", &request),
    );
    assert_eq!(
        [&left, &right].iter().filter(|r| r.is_ok()).count(),
        1,
        "{left:?} {right:?}"
    );
    assert!(
        matches!(left, Err(PgError::IdempotencyConflict))
            || matches!(right, Err(PgError::IdempotencyConflict))
    );
    let data = snapshot(&admin).await;
    assert_eq!(data["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(data["events"].as_array().unwrap().len(), 1);
    assert_eq!(data["receipts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn late_receipt_failure_rolls_back_owner_person_and_event_then_retry_succeeds() {
    let (_g, admin, store) = setup().await;
    let before = snapshot(&admin).await;
    admin
        .batch_execute("REVOKE INSERT ON awr_team.responsibility_receipts FROM awr_app")
        .await
        .unwrap();
    let request = available("rollback", 0, "new-person");
    assert!(matches!(
        store
            .claim_available(TENANT, PROJECT, "task", &request)
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute("GRANT INSERT ON awr_team.responsibility_receipts TO awr_app")
        .await
        .unwrap();
    let (task, receipt) = store
        .claim_available(TENANT, PROJECT, "task", &request)
        .await
        .unwrap();
    assert_eq!(task.version, 1);
    assert!(!receipt.replayed);
    let data = snapshot(&admin).await;
    assert_eq!(data["persons"].as_array().unwrap().len(), 1);
    assert_eq!(data["events"].as_array().unwrap().len(), 1);
    assert_eq!(data["receipts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn schema39_upgrade_retains_history_without_guessing_request_hashes() {
    let (_g, admin, store) = setup().await;
    let request = assignment("legacy", 0, "alice");
    store
        .assign(TENANT, PROJECT, "task", &request)
        .await
        .unwrap();
    admin
        .batch_execute(
            "ALTER TABLE awr_team.responsibility_receipts DROP COLUMN request_hash;
         UPDATE awr_team.schema_state SET version=39 WHERE component='awr_team';",
        )
        .await
        .unwrap();
    let historical = snapshot(&admin).await;
    assert!(matches!(
        awr_team_pg::check_schema(&admin).await,
        Err(PgError::SchemaIncompatible(_))
    ));
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::check_schema(&admin).await.unwrap();
    let after = snapshot(&admin).await;
    let mut preserved = after.clone();
    for receipt in preserved["receipts"].as_array_mut().unwrap() {
        assert!(receipt["request_hash"].is_null());
        receipt.as_object_mut().unwrap().remove("request_hash");
    }
    assert_eq!(preserved, historical);
    conflict(store.assign(TENANT, PROJECT, "task", &request).await);
    assert_eq!(snapshot(&admin).await, after);
    awr_team_pg::migrate(&admin).await.unwrap();
    assert_eq!(snapshot(&admin).await, after);
    let (task, _) = store
        .assign(
            TENANT,
            PROJECT,
            "task",
            &assignment("verified-new", 1, "bob"),
        )
        .await
        .unwrap();
    assert_eq!(task.version, 2);
    let hashes = admin.query("SELECT request_key,request_hash FROM awr_team.responsibility_receipts ORDER BY request_key", &[]).await.unwrap();
    assert_eq!(hashes[0].get::<_, String>(0), "legacy");
    assert_eq!(hashes[0].get::<_, Option<String>>(1), None);
    assert_eq!(hashes[1].get::<_, String>(1).len(), 64);
}
