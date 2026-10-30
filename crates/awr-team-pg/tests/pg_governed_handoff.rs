#![cfg(feature = "pg-tests")]
//! A transfer hands the claim to another actor, so accepting it needs the decision to be current.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/governance.rs"]
mod governed;

use awr_team_pg::{PgError, WorkstreamCommand, WorkstreamReadStore};
use fixture::*;
use governed::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

async fn members(admin: &Client) {
    admin.batch_execute(r#"UPDATE awr_team.project_memberships SET role='worker' WHERE actor_id='agent';
        UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1 WHERE client_id='cli-a';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
          VALUES('reader-tenant','receiver','human','Successor member','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
          VALUES('reader-tenant','reader-project','receiver','worker');
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
          ('reader-tenant','reader-project','agent','Predecessor','active','{"kind":"simulated_member","controller_ref":"shared-controller"}'::jsonb),
          ('reader-tenant','reader-project','receiver','Successor','active','{"kind":"simulated_member","controller_ref":"shared-controller"}'::jsonb);
        UPDATE awr_team.credentials SET actor_id='receiver' WHERE id='reader-b';
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
          VALUES('reader-tenant','reader-project','receiver','cli-b','00000000000000000000000001',1,true,true);"#).await.unwrap();
}

async fn scoped(store: &WorkstreamReadStore, token: &str, session: &str) -> Value {
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    q.session_id = Some(session.into());
    q.max_context_bytes = Some(262144);
    store.query(TENANT, PROJECT, token, q).await.unwrap()
}

async fn start_session(store: &WorkstreamReadStore, token: &str, name: &str) -> String {
    let p = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&p, name, "session.start", json!({"conversation_id":name})),
        )
        .await
        .unwrap()["receipt"]["data"]["session_id"]
        .as_str()
        .unwrap()
        .into()
}

struct Trial {
    sender: String,
    receiver: String,
    claim: Value,
    handoff: Value,
    now: i64,
}

/// The predecessor claims the work and proposes it to the successor.
async fn trial(admin: &Client, store: &WorkstreamReadStore) -> Trial {
    members(admin).await;
    let sender = start_session(store, A, "predecessor-conversation").await;
    let receiver = start_session(store, B, "successor-conversation").await;
    let p = scoped(store, A, &sender).await;
    let claimed = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "take",
                "task.claim_available",
                json!({"session_id":sender,"expected_session_version":"1",
            "expected_responsibility_version":p["data"]["responsibility"]["version"],
            "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
            "ttl_seconds":600}),
            ),
        )
        .await
        .unwrap();
    let claim = claimed["receipt"]["data"].clone();
    let p = scoped(store, A, &sender).await;
    let checkpoint = store.commands().execute(TENANT, PROJECT, A, command(&p, "save-checkpoint", "session.checkpoint",
        json!({"session_id":sender,"expected_session_version":"1","context_hash":p["data"]["context_hash"],
            "next_action":"Review the unfinished API implementation","open_loops":["Independent review pending"]})))
        .await.unwrap();
    let now: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let p = scoped(store, A, &sender).await;
    let args = json!({"session_id":sender,"expected_session_version":"2","handoff_id":"handoff",
            "kind":"execution","to_person_id":"receiver",
            "package":{"task_id":"a","contract_version":p["data"]["contract_hash"],"contract_hash":p["data"]["contract_hash"],
                "current_person_id":"agent","current_execution":{"kind":"person","person_id":"agent"},
                "consumed_context_digest":checkpoint["receipt"]["data"]["context_hash"],
                "checkpoint_ids":[checkpoint["receipt"]["data"]["checkpoint_id"]],"artifact_versions":[],"dependency_ids":[],
                "todos":["Review the unfinished API implementation","Independent review pending"],
                "awaiting_replies":[],"unknown_side_effects":[]},
            "proposed_successor":{"kind":"person","person_id":"receiver"},
            "proposer_execution_id":null,
            "proposer_fence":claim["fence"],"now_ms":now,"expires_at_ms":now+60000});
    let handoff = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(&p, "propose", "handoff.propose", args),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    Trial {
        sender,
        receiver,
        claim,
        handoff,
        now,
    }
}

async fn inspect(store: &WorkstreamReadStore, trial: &mut Trial) {
    let p = scoped(store, B, &trial.receiver).await;
    trial.handoff = store.commands().execute(TENANT, PROJECT, B, command(&p, "inspect", "handoff.inspect",
        json!({"session_id":trial.receiver,"expected_session_version":"1","handoff_id":"handoff",
            "expected_handoff_version":trial.handoff["version"],"inspector_person_id":"receiver","now_ms":trial.now})))
        .await.unwrap()["receipt"]["data"].clone();
}

async fn accept(store: &WorkstreamReadStore, trial: &Trial) -> WorkstreamCommand {
    let p = scoped(store, B, &trial.receiver).await;
    let mut args = json!({"session_id":trial.receiver,"expected_session_version":"1","handoff_id":"handoff",
        "expected_handoff_version":trial.handoff["version"],"acceptor_person_id":"receiver",
        "successor_execution":{"kind":"person","person_id":"receiver"},"prior_execution_stopped":true,
        "prior_reconciled":true,"context_reprepared":true,"expected_current_fence":trial.claim["fence"],"now_ms":trial.now});
    // New clients select the actual inspection; legacy responses have none.
    if !trial.handoff["consumption"].is_null() {
        args["inspection_request_id"] = trial.handoff["inspection_request_id"].clone();
    }
    command(&p, "accept", "handoff.accept", args)
}

async fn decide_for_a(store: &WorkstreamReadStore) {
    let mut q = query("work.admission");
    q.work_id = Some("a".into());
    let read = store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepare(store, A, "a").await,
                "decide-transfer",
                "admission.decide",
                json!({"admission_decision":"approved","binding_fingerprint":read["binding_fingerprint"],
                       "expected_admission_version":read["admission_version"],
                       "admission_judgment":judgment()}),
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_transfer_needs_the_decision_that_admitted_the_claim_to_be_current_when_it_is_accepted() {
    let governed = candidate("governed_enforce", complete());
    let (_guard, admin, db, store) = setup_with_specs(files(&governed)).await;
    // The predecessor claims under a current decision (the decider is still the project admin).
    enable_writes(&admin).await;
    decide_for_a(&store).await;
    let mut t = trial(&admin, &store).await;
    inspect(&store, &mut t).await;
    // The definition of the work changes while the handoff waits to be accepted.
    let mut changed = governed.clone();
    changed.works[0].definition = Some(
        serde_json::from_value(json!({
            "version": 1, "purpose": "research", "tier": "required",
            "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": PIN}],
            "contributes": [{"goal": "ship", "outcome": "O", "claim": "Maps the failure modes", "by": "agent-a"}],
            "granularity": {"judgment": "independent_result", "reason": "One answer", "by": "agent-a"},
            "deliverable": "A different list of failure modes"
        }))
        .unwrap(),
    );
    publish(&db, Some(&changed)).await.unwrap();
    let refused = store
        .commands()
        .execute(TENANT, PROJECT, B, accept(&store, &t).await)
        .await;
    match refused {
        Err(PgError::AdmissionRequired { code, .. }) => assert_eq!(code, "admission_stale"),
        other => panic!("expected the stale decision to hold the transfer, got {other:?}"),
    }
    // Nothing moved: the predecessor still holds the claim.
    let holder: String = admin
        .query_one(
            "SELECT session_id FROM awr_team.claims WHERE work_id='a' AND state='active'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(holder, t.sender);
}
