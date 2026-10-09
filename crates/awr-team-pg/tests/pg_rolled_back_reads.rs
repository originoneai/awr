#![cfg(feature = "pg-tests")]
//! A repeatable-read read that locks a row another transaction updated after the read's
//! snapshot began is rolled back by PostgreSQL (serialization failure). Every command updates the
//! project row, so concurrent workers hit this regularly; the delivery sync reads must run again
//! instead of reporting the store as unavailable.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{DeliverySyncStore, PgError, retry_rolled_back};
use common::{connect_config, test_config, with_app_role, with_db};
use fixture::{A, PROJECT, TENANT};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio_postgres::Client;

/// A real SQLSTATE 40001: a repeatable-read transaction updates a row that changed after its snapshot.
async fn serialization_failure(admin: &Client, db: &str) -> PgError {
    admin
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS public.rolled_back_probe(id int PRIMARY KEY, v int NOT NULL);
             INSERT INTO public.rolled_back_probe VALUES (1,0) ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let reader = connect_config(&with_db(&test_config(), db)).await;
    reader
        .batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await
        .unwrap();
    reader
        .query("SELECT v FROM public.rolled_back_probe WHERE id=1", &[])
        .await
        .unwrap();
    admin
        .execute("UPDATE public.rolled_back_probe SET v=v+1 WHERE id=1", &[])
        .await
        .unwrap();
    let error = reader
        .execute("UPDATE public.rolled_back_probe SET v=v+10 WHERE id=1", &[])
        .await
        .unwrap_err();
    PgError::from(error)
}

#[tokio::test]
async fn a_rolled_back_attempt_runs_again_and_other_errors_do_not() {
    let (_guard, admin, db, _reads) = fixture::setup().await;
    let attempts = AtomicU32::new(0);
    let first_fails = retry_rolled_back(|| async {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(serialization_failure(&admin, &db).await)
        } else {
            Ok(7)
        }
    })
    .await
    .unwrap();
    assert_eq!((first_fails, attempts.load(Ordering::SeqCst)), (7, 2));

    let attempts = AtomicU32::new(0);
    let refused = retry_rolled_back(|| async {
        attempts.fetch_add(1, Ordering::SeqCst);
        Err::<(), _>(PgError::Forbidden)
    })
    .await;
    assert!(matches!(refused, Err(PgError::Forbidden)));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "a domain error is not repeated"
    );

    let attempts = AtomicU32::new(0);
    let exhausted = retry_rolled_back(|| async {
        attempts.fetch_add(1, Ordering::SeqCst);
        Err::<(), _>(serialization_failure(&admin, &db).await)
    })
    .await;
    assert!(exhausted.unwrap_err().is_retryable());
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        8,
        "eight attempts, then the error is returned"
    );
}

#[tokio::test]
async fn a_delivery_read_survives_a_project_update_between_its_snapshot_and_its_locks() {
    let (_guard, admin, db, _reads) = fixture::setup().await;
    let store = DeliverySyncStore::from_config(with_app_role(&test_config(), &db));
    let writer = connect_config(&with_db(&test_config(), &db)).await;
    // Hold the workstream-mode row: the read starts, takes its snapshot and waits for this lock.
    admin.batch_execute("BEGIN").await.unwrap();
    admin
        .query_one(
            "SELECT enabled FROM awr_team.workstream_modes WHERE tenant_id=$1 AND project_id=$2 FOR UPDATE",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    let read = tokio::spawn(async move { store.inspect(TENANT, PROJECT, A, "a").await });
    for _ in 0..200 {
        let waiting: i64 = writer
            .query_one(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname=$1 AND pid<>pg_backend_pid() AND state='active' AND wait_event_type='Lock'",
                &[&db],
            )
            .await
            .unwrap()
            .get(0);
        if waiting > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A command commits a new project revision while the read waits, so the read's snapshot is
    // older than the project row it is about to lock.
    writer
        .execute(
            "UPDATE awr_team.projects SET project_revision=project_revision+1 WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    admin.batch_execute("COMMIT").await.unwrap();
    let view = read
        .await
        .unwrap()
        .expect("the rolled-back read runs again and succeeds");
    assert!(view.is_object());
}
