#![cfg(feature = "pg-tests")]
//! A review covers the governed scope it opened under: when the definition, the examples or the
//! policy of the work move after the round opened, the round no longer covers the work and a new
//! review is needed.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/governance.rs"]
mod governed;

use awr_team_pg::{PgError, WorkstreamReadStore, workstream_credential_hash};
use fixture::*;
use governed::*;
use serde_json::{Value, json};
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

async fn insert_succeeded_execution(admin: &Client, id: &str, contract_hash: &str, fence: i64) {
    admin
        .execute(
            "INSERT INTO awr_team.executions(
                tenant_id,project_id,id,work_id,fence,contract_hash,input_digest,
                executor_actor_id,state,result_digest,scope_id)
             VALUES($1,$2,$3,'a',$4,$5,$6,'runner','succeeded',$7,'main')",
            &[
                &TENANT,
                &PROJECT,
                &id,
                &fence,
                &contract_hash,
                &INPUT,
                &RESULT,
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

async fn fingerprint(store: &WorkstreamReadStore) -> String {
    let mut q = query("work.admission");
    q.work_id = Some("a".into());
    store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["binding_fingerprint"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_review_covers_the_governed_scope_it_opened_under() {
    let governed = candidate("governed_enforce", complete());
    let (_g, admin, db, store) = setup_with_specs(files(&governed)).await;
    seed_review_actors(&admin).await;
    let contract_hash = prepare(&store, A, "a").await["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-gov", &contract_hash, 1).await;
    let evidence = run(
        &store,
        RUNNER,
        "ev-gov",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-gov",
            &hex_encode(b"governed-artifact"),
        ),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    // The round opens under the governed scope as it stands and records it.
    let scope = fingerprint(&store).await;
    let open = |request: &str| json!({"request": request, "args": {"session_id":"session-a","expected_session_version":"1","evidence_id": evidence_id}});
    let first = run(
        &store,
        A,
        "open-gov-1",
        "review.open",
        open("open-gov-1")["args"].clone(),
    )
    .await;
    assert_eq!(first["governance_binding"], scope);
    let stored: Option<String> = admin
        .query_one(
            "SELECT governance_binding FROM awr_team.review_rounds WHERE id=$1",
            &[&first["round_id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored.as_deref(), Some(scope.as_str()));
    // The definition of the work changes in a new snapshot; its contract does not.
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
    assert_ne!(fingerprint(&store).await, scope);
    // The reviewer's decision on the old round no longer covers the work.
    let refused = run_err(
        &store,
        REVIEWER_TOKEN,
        "acc-gov-old",
        "review.accept",
        json!({"session_id":"session-reviewer","expected_session_version":"1",
               "round_id": first["round_id"], "reason":"looks good"}),
    )
    .await;
    assert!(matches!(refused, PgError::ReviewRequired), "{refused:?}");
    // A new round opens under the new scope, can be accepted and completes the work.
    let second = run(
        &store,
        A,
        "open-gov-2",
        "review.open",
        open("open-gov-2")["args"].clone(),
    )
    .await;
    assert_eq!(second["governance_binding"], fingerprint(&store).await);
    assert_ne!(second["governance_binding"], first["governance_binding"]);
    let accepted = run(
        &store,
        REVIEWER_TOKEN,
        "acc-gov-new",
        "review.accept",
        json!({"session_id":"session-reviewer","expected_session_version":"1",
               "round_id": second["round_id"], "reason":"looks good"}),
    )
    .await;
    assert_eq!(accepted["state"], "approved");
    let completed = run(
        &store,
        A,
        "done-gov",
        "work.complete",
        json!({"session_id":"session-a","expected_session_version":"1",
               "evidence_id": evidence_id, "context_complete": true}),
    )
    .await;
    assert_eq!(completed["task_complete"], true);
}

#[tokio::test]
async fn an_approval_given_before_the_scope_moved_cannot_complete_the_work() {
    let governed = candidate("governed_enforce", complete());
    let (_g, admin, db, store) = setup_with_specs(files(&governed)).await;
    seed_review_actors(&admin).await;
    let contract_hash = prepare(&store, A, "a").await["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-gov-2", &contract_hash, 1).await;
    let evidence = run(
        &store,
        RUNNER,
        "ev-gov-2",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-gov-2",
            &hex_encode(b"governed-artifact-2"),
        ),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-gov-3",
        "review.open",
        json!({"session_id":"session-a","expected_session_version":"1","evidence_id": evidence_id}),
    )
    .await;
    run(
        &store,
        REVIEWER_TOKEN,
        "acc-gov-3",
        "review.accept",
        json!({"session_id":"session-reviewer","expected_session_version":"1",
               "round_id": opened["round_id"], "reason":"looks good"}),
    )
    .await;
    // The policy of the source changes after the approval, so the approved round no longer covers the work.
    let mut changed = governed.clone();
    changed.policy = serde_json::from_value(
        json!({"version": 1, "mode": "governed_enforce", "scope": "all", "independent_review_tags": ["security"]}),
    )
    .unwrap();
    publish(&db, Some(&changed)).await.unwrap();
    let refused = run_err(
        &store,
        A,
        "done-gov-3",
        "work.complete",
        json!({"session_id":"session-a","expected_session_version":"1",
               "evidence_id": evidence_id, "context_complete": true}),
    )
    .await;
    assert!(matches!(refused, PgError::ReviewRequired), "{refused:?}");
}
