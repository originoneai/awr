#![cfg(feature = "pg-tests")]
//! A decision is made by a verified person, answers one scope and, when the policy asks for it,
//! is reviewed by another person than its proposer.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/governance.rs"]
mod governed;

use awr_core::WorkDefinition;
use awr_team_pg::{PgError, workstream_credential_hash};
use fixture::*;
use governed::*;
use serde_json::json;
use tokio_postgres::Client;

const APPROVER: &str =
    "awr1.approver-h.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

/// A second person who may decide: a maintainer with a credential and a grant on the stream.
async fn seed_approver(admin: &Client) {
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
                ('reader-tenant','approver','human','Approver','active') ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
                ('reader-tenant','reader-project','approver','maintainer') ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('reader-tenant','reader-project','approver','Approver','active') ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let hash = workstream_credential_hash(APPROVER).unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
             VALUES($1,'approver-h','approver','cli-approver',$2) ON CONFLICT DO NOTHING",
            &[&TENANT, &hash],
        )
        .await
        .unwrap();
    let stream: String = admin
        .query_one(
            "SELECT workstream_id FROM awr_team.sessions WHERE id='session-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,
                authority_version,can_read,can_write)
             VALUES($1,$2,'approver','cli-approver',$3,1,true,true) ON CONFLICT DO NOTHING",
            &[&TENANT, &PROJECT, &stream],
        )
        .await
        .unwrap();
}

fn other_definition() -> WorkDefinition {
    serde_json::from_value(json!({
        "version": 1, "purpose": "research", "tier": "required",
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": PIN}],
        "contributes": [{"goal": "ship", "outcome": "O", "claim": "Maps the failure modes", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One answer", "by": "agent-a"},
        "deliverable": "A different list of failure modes"
    }))
    .unwrap()
}

#[tokio::test]
async fn a_decision_is_recorded_under_the_verified_actor_and_its_person() {
    let governed = candidate("governed_enforce", complete());
    let (_guard, admin, _db, store) = setup_with_specs(files(&governed)).await;
    enable_writes(&admin).await;
    let decided = approve_work(&store, A, "a", "approve").await.unwrap();
    assert_eq!(decided["receipt"]["data"]["decider"]["actor_id"], "agent");
    let row = admin
        .query_one(
            "SELECT actor_id, person_id, decision, request_id FROM awr_team.work_admissions WHERE work_id='a'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "agent");
    assert_eq!(
        row.get::<_, Option<String>>(1).as_deref(),
        decided["receipt"]["data"]["decider"]["person_id"].as_str()
    );
    assert_eq!(row.get::<_, String>(2), "approved");
    assert_eq!(row.get::<_, String>(3), "approve");
}

#[tokio::test]
async fn independent_review_of_a_decision_needs_another_person_than_the_proposer() {
    let mut governed = candidate("governed_enforce", complete());
    governed.policy = serde_json::from_value(json!({
        "version": 1, "mode": "governed_enforce", "scope": "all", "independent_review_tags": ["security"]
    }))
    .unwrap();
    governed.works[0].tags = vec!["security".into()];
    let (_guard, admin, _db, store) = setup_with_specs(files(&governed)).await;
    enable_writes(&admin).await;
    seed_approver(&admin).await;
    // Under independent review an approval answers a proposal; stating a judgment alone is not enough.
    let alone = approve_work(&store, A, "a", "approve-alone").await;
    assert!(alone.is_err(), "{alone:?}");
    let proposed = decide_work(
        &store,
        A,
        "a",
        "propose",
        json!({"admission_decision":"proposed","admission_judgment":judgment()}),
    )
    .await
    .unwrap();
    let proposal = proposed["receipt"]["data"]["admission_id"].clone();
    // The proposer cannot approve their own proposal.
    let own = decide_work(
        &store,
        A,
        "a",
        "approve-own",
        json!({"admission_decision":"approved","admission_proposal_id":proposal}),
    )
    .await;
    assert!(matches!(own, Err(PgError::AuthorCannotReview)), "{own:?}");
    // Another person can, and the work is admitted.
    let answered = decide_work(
        &store,
        APPROVER,
        "a",
        "approve-other",
        json!({"admission_decision":"approved","admission_proposal_id":proposal}),
    )
    .await
    .unwrap();
    assert_eq!(answered["receipt"]["data"]["state"], "approved");
    assert_eq!(answered["receipt"]["data"]["admitted"], true);
    assert_eq!(
        answered["receipt"]["data"]["decider"]["actor_id"],
        "approver"
    );
}

#[tokio::test]
async fn a_proposal_answers_only_for_the_scope_it_was_made_for() {
    let governed = candidate("governed_enforce", complete());
    let (_guard, admin, db, store) = setup_with_specs(files(&governed)).await;
    enable_writes(&admin).await;
    let propose = |request: &'static str| {
        decide_work(
            &store,
            A,
            "a",
            request,
            json!({"admission_decision":"proposed","admission_judgment":judgment()}),
        )
    };
    let first = propose("propose-1").await.unwrap();
    let old_proposal = first["receipt"]["data"]["admission_id"].clone();
    // The definition of the work changes in a new snapshot, so the scope is another one.
    let mut changed = governed.clone();
    changed.works[0].definition = Some(other_definition());
    publish(&db, Some(&changed)).await.unwrap();
    let late = decide_work(
        &store,
        A,
        "a",
        "approve-late",
        json!({"admission_decision":"approved","admission_proposal_id":old_proposal}),
    )
    .await;
    assert!(
        matches!(late, Err(PgError::PreconditionsChanged)),
        "{late:?}"
    );
    // A proposal made for the new scope is answered for it.
    let second = propose("propose-2").await.unwrap();
    let proposal = second["receipt"]["data"]["admission_id"].clone();
    let answered = decide_work(
        &store,
        A,
        "a",
        "approve-new",
        json!({"admission_decision":"approved","admission_proposal_id":proposal}),
    )
    .await
    .unwrap();
    assert_eq!(answered["receipt"]["data"]["state"], "approved");
}
