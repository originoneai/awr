#![cfg(feature = "pg-tests")]
//! Governed work is claimed, held and run under one current decision, however the callers race.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/governance.rs"]
mod governed;

use awr_team_pg::{PgError, WorkstreamReadStore};
use fixture::*;
use governed::*;
use serde_json::{Value, json};

/// `execution.prepare` under `claim`, from a context read inside the session.
async fn prepare_run(
    store: &WorkstreamReadStore,
    session: &str,
    claim: &Value,
    request: &str,
) -> Result<Value, PgError> {
    let p = prepare_in(store, A, "a", session).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                request,
                "execution.prepare",
                json!({"session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],
                       "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
                       "expected_work_version":p["data"]["runtime"]["work_version"],
                       "input_digest":"a".repeat(64),"declared_scope":["src/api"]}),
            ),
        )
        .await
}

#[tokio::test]
async fn a_denial_after_the_claim_holds_the_run_and_leaves_recovery_open() {
    let governed = candidate("governed_enforce", complete());
    let (_guard, admin, _db, store) = setup_with_specs(files(&governed)).await;
    enable_writes(&admin).await;
    approve_work(&store, A, "a", "approve").await.unwrap();
    let session = session_on(&store, A, "a", "holder").await;
    let claimed = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            claim_by(&store, A, "a", &session, "take").await,
        )
        .await
        .unwrap();
    let claim = claimed["receipt"]["data"].clone();
    assert_eq!(claim["fence"], "1");
    // The decision is withdrawn after the claim: the run is held, the claim is not taken away.
    deny_work(
        &store,
        A,
        "a",
        "withdraw",
        "The examples restate the acceptance",
    )
    .await
    .unwrap();
    let held = prepare_run(&store, &session, &claim, "prepare-denied").await;
    refused_with(&held, "admission_denied");
    // Recovery is never held: the session records where it stopped and lets the claim go.
    let p = prepare_in(&store, A, "a", &session).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "save",
                "session.checkpoint",
                json!({"session_id":session,"expected_session_version":"1","context_hash":p["data"]["context_hash"],
                       "next_action":"Wait for a new decision","open_loops":["Admission was denied after the claim"]}),
            ),
        )
        .await
        .unwrap();
    let p = prepare_in(&store, A, "a", &session).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "let-go",
                "claim.release",
                json!({"session_id":session,"expected_session_version":"2","claim_id":claim["claim_id"],
                       "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"]}),
            ),
        )
        .await
        .unwrap();
    // Nobody takes it again while the denial stands.
    let next = session_on(&store, A, "a", "next").await;
    let again = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            claim_by(&store, A, "a", &next, "take-again").await,
        )
        .await;
    refused_with(&again, "admission_denied");
}

#[tokio::test]
async fn rival_claims_under_one_decision_give_one_claim() {
    let governed = candidate("governed_enforce", complete());
    let (_guard, admin, _db, store) = setup_with_specs(files(&governed)).await;
    enable_writes(&admin).await;
    approve_work(&store, A, "a", "approve").await.unwrap();
    let first = session_on(&store, A, "a", "first").await;
    let second = session_on(&store, A, "a", "second").await;
    let one = claim_by(&store, A, "a", &first, "rival-1").await;
    let other = claim_by(&store, A, "a", &second, "rival-2").await;
    let commands = store.commands();
    let (one, other) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, one),
        commands.execute(TENANT, PROJECT, A, other)
    );
    let results = [one, other];
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "{results:?}"
    );
    // The loser is told about the claim that exists, never about a decision, which stands.
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| !matches!(error, PgError::AdmissionRequired { .. })),
        "{results:?}"
    );
    let active: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.claims WHERE work_id='a' AND state='active'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(active, 1);
}

#[tokio::test]
async fn a_claim_racing_a_denial_never_runs_under_the_denial() {
    let governed = candidate("governed_enforce", complete());
    let (_guard, admin, _db, store) = setup_with_specs(files(&governed)).await;
    enable_writes(&admin).await;
    approve_work(&store, A, "a", "approve").await.unwrap();
    let session = session_on(&store, A, "a", "racer").await;
    let take = claim_by(&store, A, "a", &session, "race-take").await;
    // The decider reads before the race and states what it read.
    let read = admission_read(&store, A, "a").await.unwrap();
    let deny = command(
        &prepare(&store, A, "a").await,
        "race-deny",
        "admission.decide",
        json!({"admission_decision":"denied","admission_reason":"The failure modes are already listed",
               "binding_fingerprint":read["binding_fingerprint"],
               "expected_admission_version":read["admission_version"]}),
    );
    let commands = store.commands();
    let (took, denied) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, take),
        commands.execute(TENANT, PROJECT, A, deny)
    );
    if denied.is_err() {
        // A claim that came first changed what the decider read; reading again, it decides.
        deny_work(
            &store,
            A,
            "a",
            "race-deny-again",
            "The failure modes are already listed",
        )
        .await
        .unwrap();
    }
    assert_eq!(
        admission_read(&store, A, "a").await.unwrap()["state"],
        "denied"
    );
    match took {
        // The denial came first: the work was not claimed.
        Err(error @ PgError::AdmissionRequired { .. }) => {
            refused_with(&Err::<Value, PgError>(error), "admission_denied")
        }
        // The claim came first: it stands, and the run it would start is held.
        Ok(claimed) => {
            let claim = claimed["receipt"]["data"].clone();
            let held = prepare_run(&store, &session, &claim, "race-run").await;
            refused_with(&held, "admission_denied");
        }
        other => panic!("expected the claim or the denial to win cleanly, got {other:?}"),
    }
}
