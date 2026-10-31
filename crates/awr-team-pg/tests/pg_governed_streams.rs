#![cfg(feature = "pg-tests")]
//! Admission belongs to the work and its stream: a decision, a read and an authority never cross
//! into another stream, and work that requires work in another stream is admitted on its own.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/governance.rs"]
mod governed;

use awr_team_pg::PgError;
use fixture::*;
use governed::*;
use serde_json::json;

#[tokio::test]
async fn work_that_requires_work_in_another_stream_is_admitted_on_its_own() {
    let mut governed = candidate("governed_enforce", complete());
    for key in ["c", "b-private"] {
        let mut work = governed.works[0].clone();
        work.external_key = key.into();
        governed.works.push(work);
    }
    // The declaration has one canonical order: its works are sorted by key.
    governed
        .works
        .sort_by(|a, b| a.external_key.cmp(&b.external_key));
    let (_guard, admin, _db, store) = setup_with_specs(files(&governed)).await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1
            WHERE client_id='cli-b'",
        )
        .await
        .unwrap();
    // The reader of one stream cannot read what is decided for another.
    let hidden = admission_read(&store, A, "b-private").await;
    assert!(matches!(hidden, Err(PgError::Forbidden)), "{hidden:?}");
    // What it reads of its own work, which requires that work, says nothing of the other stream.
    let own = admission_read(&store, A, "c").await.unwrap();
    assert_eq!(own["state"], "missing");
    assert!(!own.to_string().contains("b-private"), "{own}");
    let session = session_on(&store, A, "c", "dependent").await;
    let held = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            claim_by(&store, A, "c", &session, "take-before").await,
        )
        .await;
    refused_with(&held, "admission_missing");
    // The decision of c admits c and only c.
    let approved = approve_work(&store, A, "c", "approve-c").await.unwrap();
    assert!(!approved.to_string().contains("b-private"), "{approved}");
    assert_eq!(approved["receipt"]["data"]["state"], "approved");
    // A decider of one stream cannot decide for work of another, even with everything it needs
    // to decide: the scope and the history that the other stream's decider reads.
    let theirs = admission_read(&store, B, "b-private").await.unwrap();
    assert_eq!(theirs["state"], "missing");
    let mut stray = command(
        &prepare(&store, A, "c").await,
        "approve-stray",
        "admission.decide",
        json!({"admission_decision":"approved","admission_judgment":judgment(),
               "binding_fingerprint":theirs["binding_fingerprint"],
               "expected_admission_version":theirs["admission_version"]}),
    );
    stray.work_id = "b-private".into();
    stray.workstream_id = awr_core::Id::from(2);
    let refused = store.commands().execute(TENANT, PROJECT, A, stray).await;
    assert!(matches!(refused, Err(PgError::Forbidden)), "{refused:?}");
    let rows: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.work_admissions WHERE work_id='b-private'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 0);
    // Admitted, c still waits for the work it requires: the decision is not readiness.
    let waiting = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            claim_by(&store, A, "c", &session, "take-after").await,
        )
        .await;
    assert!(
        matches!(waiting, Err(PgError::MissingDependency)),
        "{waiting:?}"
    );
    // The other stream decides its own work, and the decision of c is untouched by it.
    approve_work(&store, B, "b-private", "approve-b")
        .await
        .unwrap();
    assert_eq!(
        admission_read(&store, B, "b-private").await.unwrap()["state"],
        "approved"
    );
    assert_eq!(
        admission_read(&store, A, "c").await.unwrap()["state"],
        "approved"
    );
    let rows: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.work_admissions", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 2);
}
