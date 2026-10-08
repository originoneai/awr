#![cfg(feature = "pg-tests")]
//! The direct store APIs follow the same lock order as the authenticated command
//! path (see `lock_order.rs`): the project barrier first. A store call that took
//! the task lock first and reached the barrier later, through the foreign key of
//! the task row, deadlocked against a command that already held the barrier.
mod common;
use awr_core::*;
use awr_team_pg::{HandoffStore, PgError, ResponsibilityStore};
use common::{connect_config, fresh_team_schema, test_config, with_app_role, with_db};
use std::sync::MutexGuard;
use std::time::Duration;
use tokio_postgres::Client;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    /// Plays the command path: it holds the project barrier.
    holder: Client,
    /// Looks at the other backends; never takes part.
    observer: Client,
    db: String,
}

async fn setup() -> Fixture {
    let (guard, holder, db) = fresh_team_schema().await;
    holder
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');",
        )
        .await
        .unwrap();
    let observer = connect_config(&with_db(&test_config(), &db)).await;
    Fixture {
        _guard: guard,
        holder,
        observer,
        db,
    }
}

impl Fixture {
    async fn hold_the_barrier(&self) {
        self.holder.batch_execute("BEGIN").await.unwrap();
        self.holder
            .query_one(
                "SELECT status FROM awr_team.projects WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
                &[&TENANT, &PROJECT],
            )
            .await
            .unwrap();
    }

    async fn release_the_barrier(&self) {
        self.holder.batch_execute("ROLLBACK").await.unwrap();
    }

    /// The backend of this fixture's database that is waiting for a lock, with
    /// the statement it is blocked on and the advisory locks it already holds.
    async fn blocked_backend(&self) -> (String, i64) {
        for _ in 0..200 {
            let rows = self
                .observer
                .query(
                    "SELECT a.pid, a.query FROM pg_stat_activity a
                     WHERE a.datname=$1 AND a.pid<>pg_backend_pid()
                       AND a.state='active' AND a.wait_event_type='Lock'",
                    &[&self.db],
                )
                .await
                .unwrap();
            if let Some(row) = rows.first() {
                let pid: i32 = row.get(0);
                let query: String = row.get(1);
                let advisory: i64 = self
                    .observer
                    .query_one(
                        "SELECT count(*) FROM pg_locks
                         WHERE pid=$1 AND locktype='advisory' AND granted",
                        &[&pid],
                    )
                    .await
                    .unwrap()
                    .get(0);
                return (query, advisory);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the store call never started waiting for a lock");
    }
}

fn blocked_on_the_barrier(query: &str) -> bool {
    query.contains("FROM awr_team.projects") && query.contains("FOR UPDATE")
}

#[tokio::test]
async fn a_responsibility_call_waits_for_the_barrier_before_it_takes_any_task_lock() {
    let f = setup().await;
    let store = ResponsibilityStore::from_config(with_app_role(&test_config(), &f.db));
    let alice = PersonId::new("alice").unwrap();
    store
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    f.hold_the_barrier().await;
    let call = tokio::spawn(async move {
        store
            .assign(
                TENANT,
                PROJECT,
                "work-a",
                &AssignResponsibilityRequest {
                    request_key: "asg-order".into(),
                    expected_version: 0,
                    owner: Some(alice.clone()),
                    collaborators: vec![],
                    independent_reviewer: None,
                    allow_unassigned: false,
                    authorized_by: alice,
                },
            )
            .await
    });
    let (query, advisory_locks) = f.blocked_backend().await;
    assert!(
        blocked_on_the_barrier(&query),
        "the call must wait at the project barrier, not further in: {query}"
    );
    assert_eq!(
        advisory_locks, 0,
        "a call waiting for the barrier must not already hold a task lock the barrier holder needs"
    );
    f.release_the_barrier().await;
    call.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_handoff_call_waits_for_the_barrier_before_it_writes_anything() {
    let f = setup().await;
    let store = HandoffStore::from_config(with_app_role(&test_config(), &f.db));
    let digest = |n: u8| format!("{n:064x}");
    let package = HandoffPackage {
        task_id: "work-a".into(),
        contract_version: "3".into(),
        contract_hash: digest(1),
        current_person_id: PersonId::new("sender").unwrap(),
        current_execution: ExecutionInstance::Person {
            person_id: PersonId::new("sender").unwrap(),
        },
        consumed_context_digest: digest(2),
        checkpoint_ids: vec!["cp-1".into()],
        artifact_versions: vec![],
        branch_id: None,
        working_directory: None,
        dependency_ids: vec![],
        todos: vec!["finish".into()],
        awaiting_replies: vec![],
        unknown_side_effects: vec![],
    };
    f.hold_the_barrier().await;
    let call = tokio::spawn(async move {
        store
            .propose(
                TENANT,
                PROJECT,
                "work-a",
                &PersonId::new("sender").unwrap(),
                &ProposeHandoffRequest {
                    request_key: "propose-order".into(),
                    handoff_id: "handoff-order".into(),
                    kind: HandoffKind::Execution,
                    package,
                    to_person_id: PersonId::new("receiver").unwrap(),
                    proposed_successor: None,
                    proposer_execution_id: None,
                    proposer_fence: None,
                    expires_at_ms: None,
                    now_ms: 1_000,
                },
            )
            .await
    });
    let (query, _) = f.blocked_backend().await;
    assert!(
        blocked_on_the_barrier(&query),
        "the call must wait at the project barrier before it writes any row: {query}"
    );
    f.release_the_barrier().await;
    call.await.unwrap().unwrap();
}

/// Two sessions that lock two rows in opposite order: PostgreSQL rolls one back
/// with SQLSTATE 40P01 after `deadlock_timeout`.
async fn provoke_deadlock(f: &Fixture) -> tokio_postgres::Error {
    f.observer
        .batch_execute(
            "CREATE TABLE public.lock_probe(id int PRIMARY KEY, v int NOT NULL);
             INSERT INTO public.lock_probe VALUES (1,0),(2,0);",
        )
        .await
        .unwrap();
    let first = connect_config(&with_db(&test_config(), &f.db)).await;
    let second = connect_config(&with_db(&test_config(), &f.db)).await;
    first.batch_execute("BEGIN").await.unwrap();
    second.batch_execute("BEGIN").await.unwrap();
    first
        .execute("SELECT 1 FROM public.lock_probe WHERE id=1 FOR UPDATE", &[])
        .await
        .unwrap();
    second
        .execute("SELECT 1 FROM public.lock_probe WHERE id=2 FOR UPDATE", &[])
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        first.execute("SELECT 1 FROM public.lock_probe WHERE id=2 FOR UPDATE", &[]),
        second.execute("SELECT 1 FROM public.lock_probe WHERE id=1 FOR UPDATE", &[]),
    );
    let _ = first.batch_execute("ROLLBACK").await;
    let _ = second.batch_execute("ROLLBACK").await;
    match (a, b) {
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => error,
        other => panic!("exactly one session must be chosen as the deadlock victim: {other:?}"),
    }
}

#[tokio::test]
async fn deadlocks_and_serialization_failures_are_retryable_and_nothing_else_is() {
    let f = setup().await;
    let deadlock = provoke_deadlock(&f).await;
    assert_eq!(
        deadlock.code(),
        Some(&tokio_postgres::error::SqlState::T_R_DEADLOCK_DETECTED)
    );
    assert!(PgError::from(deadlock).is_retryable());

    // REPEATABLE READ: the second writer of a row changed since the snapshot fails with 40001.
    let reader = connect_config(&with_db(&test_config(), &f.db)).await;
    reader
        .batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await
        .unwrap();
    reader
        .query("SELECT v FROM public.lock_probe WHERE id=1", &[])
        .await
        .unwrap();
    f.observer
        .execute("UPDATE public.lock_probe SET v=v+1 WHERE id=1", &[])
        .await
        .unwrap();
    let conflict = reader
        .execute("UPDATE public.lock_probe SET v=v+10 WHERE id=1", &[])
        .await
        .unwrap_err();
    assert_eq!(
        conflict.code(),
        Some(&tokio_postgres::error::SqlState::T_R_SERIALIZATION_FAILURE)
    );
    assert!(PgError::from(conflict).is_retryable());

    // Other database errors and domain errors leave the outcome to the caller.
    let unique = f
        .observer
        .execute("INSERT INTO public.lock_probe VALUES (1,0)", &[])
        .await
        .unwrap_err();
    assert!(!PgError::from(unique).is_retryable());
    let syntax = f.observer.batch_execute("SELEKT 1").await.unwrap_err();
    assert!(!PgError::from(syntax).is_retryable());
    assert!(!PgError::Forbidden.is_retryable());
    assert!(!PgError::IdempotencyConflict.is_retryable());
}
