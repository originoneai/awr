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

fn blocking_finding(key: &str) -> Value {
    json!({"finding_key": key, "summary": "Saving offline loses the draft",
           "criterion": "verified",
           "counterexample": {"given": "the storage is read-only", "when": "the user saves",
                              "then": "the draft is gone"}})
}

async fn submitted(
    admin: &Client,
    store: &WorkstreamReadStore,
    execution: &str,
    request: &str,
    artifact: &[u8],
) -> String {
    let contract_hash = prepare(store, A, "a").await["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    if admin
        .query_opt(
            "SELECT 1 FROM awr_team.executions WHERE id=$1",
            &[&execution],
        )
        .await
        .unwrap()
        .is_none()
    {
        insert_succeeded_execution(admin, execution, &contract_hash, 1).await;
    }
    run(
        store,
        RUNNER,
        request,
        "evidence.submit",
        submit_args("session-runner", execution, &hex_encode(artifact)),
    )
    .await["evidence_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn open_round(store: &WorkstreamReadStore, request: &str, evidence_id: &str) -> Value {
    run(
        store,
        A,
        request,
        "review.open",
        json!({"session_id":"session-a","expected_session_version":"1","evidence_id": evidence_id}),
    )
    .await
}

fn decision_args(round_id: &Value, extra: Value) -> Value {
    let mut args = json!({"session_id":"session-reviewer","expected_session_version":"1",
                          "round_id": round_id, "reason":"reviewed"});
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    args
}

#[tokio::test]
async fn a_return_of_governed_work_needs_a_blocking_finding_and_everything_else_is_deferred() {
    let governed = candidate("governed_enforce", complete());
    let (_g, admin, _, store) = setup_with_specs(files(&governed)).await;
    seed_review_actors(&admin).await;
    let evidence_id = submitted(&admin, &store, "exec-find-1", "ev-find-1", b"findings-1").await;
    let opened = open_round(&store, "open-find-1", &evidence_id).await;
    assert!(
        opened.get("re_review").is_none(),
        "the first round has nothing to re-review"
    );
    let round = opened["round_id"].clone();
    // A return without any finding, and a return with only a deferred one, return nothing.
    let bare = run_err(
        &store,
        REVIEWER_TOKEN,
        "ret-bare",
        "review.return",
        decision_args(&round, json!({})),
    )
    .await;
    assert!(matches!(bare, PgError::FindingsBlock { .. }), "{bare:?}");
    let style = run_err(
        &store,
        REVIEWER_TOKEN,
        "ret-style",
        "review.return",
        decision_args(
            &round,
            json!({"findings": [{"finding_key": "S1", "summary": "The name could say more"}]}),
        ),
    )
    .await;
    assert!(matches!(style, PgError::FindingsBlock { .. }), "{style:?}");
    let recorded: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.review_findings", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(recorded, 0, "a refused decision leaves nothing behind");
    // A blocking finding returns the work; the one next to it is a deferred record.
    let returned = run(
        &store,
        REVIEWER_TOKEN,
        "ret-block",
        "review.return",
        decision_args(
            &round,
            json!({"findings": [blocking_finding("F1"),
                                {"finding_key": "S1", "summary": "The name could say more"}]}),
        ),
    )
    .await;
    assert_eq!(returned["state"], "rejected");
    assert_eq!(returned["findings"]["open_blocking"], json!(["F1"]));
    assert_eq!(returned["findings"]["recorded"][0]["severity"], "blocking");
    assert_eq!(
        returned["findings"]["recorded"][0]["disposition"],
        "returns_work"
    );
    assert_eq!(
        returned["findings"]["recorded"][1]["severity"],
        "non_blocking"
    );
    assert_eq!(
        returned["findings"]["recorded"][1]["disposition"],
        "deferred"
    );
    // The findings are part of what the review shows, attributed to the verified reviewer.
    let mut q = query("review.inspect");
    q.work_id = Some("a".into());
    q.review_round_id = Some(round.as_str().unwrap().into());
    let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    assert_eq!(
        inspected["review"]["findings"]["open_blocking"],
        json!(["F1"])
    );
    assert_eq!(inspected["review"]["findings"]["deferred"], json!(["S1"]));
    assert_eq!(
        inspected["review"]["findings"]["findings"][0]["recorded_by"],
        "reviewer"
    );
    // A decided round is not decided again, and its findings stay as they were recorded.
    let again = run_err(
        &store,
        REVIEWER_TOKEN,
        "ret-again",
        "review.return",
        decision_args(&round, json!({"findings": [blocking_finding("F1")]})),
    )
    .await;
    assert!(matches!(again, PgError::ReviewRequired), "{again:?}");
}

#[tokio::test]
async fn a_second_round_covers_the_open_findings_and_the_delta_and_defers_what_is_outside_them() {
    let governed = candidate("governed_enforce", complete());
    let (_g, admin, _, store) = setup_with_specs(files(&governed)).await;
    seed_review_actors(&admin).await;
    let first = submitted(&admin, &store, "exec-find-2", "ev-find-2", b"findings-2").await;
    let round1 = open_round(&store, "open-find-2", &first).await["round_id"].clone();
    run(
        &store,
        REVIEWER_TOKEN,
        "ret-find-2",
        "review.return",
        decision_args(&round1, json!({"findings": [blocking_finding("F1")]})),
    )
    .await;
    run(
        &store,
        A,
        "rework-find-2",
        "work.rework",
        json!({"session_id":"session-a","expected_session_version":"1",
               "round_id": round1, "note":"addressing the return"}),
    )
    .await;
    let second = submitted(
        &admin,
        &store,
        "exec-find-2",
        "ev-find-2b",
        b"findings-2-fixed",
    )
    .await;
    let opened = open_round(&store, "open-find-2b", &second).await;
    // The re-review says what it covers.
    assert_eq!(opened["re_review"]["unresolved_blocking"], json!(["F1"]));
    assert_eq!(opened["re_review"]["bundle_changed"], true);
    assert_eq!(opened["re_review"]["previous_round_id"], round1);
    let round2 = opened["round_id"].clone();
    // The earlier blocking finding is still open, so approving without dealing with it is refused.
    let unresolved = run_err(
        &store,
        REVIEWER_TOKEN,
        "acc-unresolved",
        "review.accept",
        decision_args(&round2, json!({})),
    )
    .await;
    assert!(
        matches!(unresolved, PgError::FindingsBlock { .. }),
        "{unresolved:?}"
    );
    // A new finding that looks blocking but belongs neither to the open ones nor to the delta is
    // deferred instead of reopening the round, and the approval resolves what was open.
    let approved = run(
        &store,
        REVIEWER_TOKEN,
        "acc-resolved",
        "review.accept",
        decision_args(
            &round2,
            json!({"resolutions": [{"finding_key": "F1", "resolution": "Fixed; the counterexample passes"}],
                   "findings": [blocking_finding("F2")]}),
        ),
    )
    .await;
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["findings"]["resolved"], json!(["F1"]));
    assert_eq!(approved["findings"]["open_blocking"], json!([]));
    assert_eq!(approved["findings"]["recorded"][0]["finding_key"], "F2");
    assert_eq!(
        approved["findings"]["recorded"][0]["severity"],
        "non_blocking"
    );
    assert!(
        approved["findings"]["recorded"][0]["basis"]
            .as_str()
            .unwrap()
            .contains("re-review")
    );
    // With nothing blocking left the work completes, and the deferred record stays on file.
    let completed = run(
        &store,
        A,
        "done-find-2",
        "work.complete",
        json!({"session_id":"session-a","expected_session_version":"1",
               "evidence_id": second, "context_complete": true}),
    )
    .await;
    assert_eq!(completed["task_complete"], true);
    let deferred: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.review_findings WHERE action='record' AND disposition='deferred'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(deferred, 1);
}

#[tokio::test]
async fn only_an_approval_resolves_a_finding_and_only_one_that_is_open() {
    let governed = candidate("governed_enforce", complete());
    let (_g, admin, _, store) = setup_with_specs(files(&governed)).await;
    seed_review_actors(&admin).await;
    let first = submitted(&admin, &store, "exec-find-4", "ev-find-4", b"findings-4").await;
    let round1 = open_round(&store, "open-find-4", &first).await["round_id"].clone();
    run(
        &store,
        REVIEWER_TOKEN,
        "ret-find-4",
        "review.return",
        decision_args(&round1, json!({"findings": [blocking_finding("F1")]})),
    )
    .await;
    run(
        &store,
        A,
        "rework-find-4",
        "work.rework",
        json!({"session_id":"session-a","expected_session_version":"1",
               "round_id": round1, "note":"addressing the return"}),
    )
    .await;
    let second = submitted(
        &admin,
        &store,
        "exec-find-4",
        "ev-find-4b",
        b"findings-4-fixed",
    )
    .await;
    let round2 = open_round(&store, "open-find-4b", &second).await["round_id"].clone();
    // A return cannot say that a finding was dealt with, even beside a finding that returns the work.
    let on_return = run_err(
        &store,
        REVIEWER_TOKEN,
        "ret-with-resolution",
        "review.return",
        decision_args(
            &round2,
            json!({"resolutions": [{"finding_key": "F1", "resolution": "Fixed"}],
                   "findings": [blocking_finding("F3")]}),
        ),
    )
    .await;
    assert!(matches!(on_return, PgError::Protocol(_)), "{on_return:?}");
    // An approval cannot resolve a finding that is not open.
    let unknown = run_err(
        &store,
        REVIEWER_TOKEN,
        "acc-unknown",
        "review.accept",
        decision_args(
            &round2,
            json!({"resolutions": [{"finding_key": "F1", "resolution": "Fixed"},
                                   {"finding_key": "NOPE", "resolution": "Never was open"}]}),
        ),
    )
    .await;
    assert!(matches!(unknown, PgError::Protocol(_)), "{unknown:?}");
    let resolved: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.review_findings WHERE action='resolve'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(resolved, 0, "a refused decision leaves nothing behind");
    // Dealing with exactly the open one approves the round.
    let approved = run(
        &store,
        REVIEWER_TOKEN,
        "acc-resolved-4",
        "review.accept",
        decision_args(
            &round2,
            json!({"resolutions": [{"finding_key": "F1", "resolution": "Fixed; the counterexample passes"}]}),
        ),
    )
    .await;
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["findings"]["resolved"], json!(["F1"]));
}

#[tokio::test]
async fn a_work_with_an_open_blocking_finding_cannot_complete_until_it_is_resolved() {
    let governed = candidate("governed_enforce", complete());
    let (_g, admin, _, store) = setup_with_specs(files(&governed)).await;
    seed_review_actors(&admin).await;
    let evidence_id = submitted(&admin, &store, "exec-find-5", "ev-find-5", b"findings-5").await;
    let opened = open_round(&store, "open-find-5", &evidence_id).await;
    let round = opened["round_id"].clone();
    run(
        &store,
        REVIEWER_TOKEN,
        "acc-find-5",
        "review.accept",
        decision_args(&round, json!({})),
    )
    .await;
    // A blocking finding recorded after the approval, by a later review, keeps the work open.
    admin
        .execute(
            "INSERT INTO awr_team.review_findings(tenant_id,project_id,id,work_id,review_round_id,round_index,
                finding_key,action,severity,disposition,basis,input_json,reviewer_actor_id,reviewer_person_id)
             VALUES($1,$2,'late-finding','a',$3,1,'LATE','record','blocking','returns_work',
                'it states a security risk with its reasoning',$4,'reviewer','reviewer')",
            &[
                &TENANT,
                &PROJECT,
                &round.as_str().unwrap(),
                &json!({"finding_key":"LATE","summary":"A later review found it"}),
            ],
        )
        .await
        .unwrap();
    let complete_args = || {
        json!({"session_id":"session-a","expected_session_version":"1",
               "evidence_id": evidence_id, "context_complete": true})
    };
    let held = run_err(
        &store,
        A,
        "done-find-open",
        "work.complete",
        complete_args(),
    )
    .await;
    assert!(matches!(held, PgError::FindingsBlock { .. }), "{held:?}");
    admin
        .execute(
            "INSERT INTO awr_team.review_findings(tenant_id,project_id,id,work_id,review_round_id,round_index,
                finding_key,action,input_json,resolution,reviewer_actor_id,reviewer_person_id)
             VALUES($1,$2,'late-resolution','a',$3,1,'LATE','resolve',$4,'Dealt with',
                'reviewer','reviewer')",
            &[
                &TENANT,
                &PROJECT,
                &round.as_str().unwrap(),
                &json!({"finding_key":"LATE"}),
            ],
        )
        .await
        .unwrap();
    let completed = run(
        &store,
        A,
        "done-find-closed",
        "work.complete",
        complete_args(),
    )
    .await;
    assert_eq!(completed["task_complete"], true);
}

#[tokio::test]
async fn a_definition_that_names_no_tier_judges_its_criteria_as_required() {
    let mut governed = candidate("governed_enforce", complete());
    let mut definition =
        serde_json::to_value(governed.works[0].definition.as_ref().unwrap()).unwrap();
    definition.as_object_mut().unwrap().remove("tier");
    governed.works[0].definition = Some(serde_json::from_value(definition).unwrap());
    let (_g, admin, _, store) = setup_with_specs(files(&governed)).await;
    seed_review_actors(&admin).await;
    let evidence_id = submitted(&admin, &store, "exec-find-6", "ev-find-6", b"findings-6").await;
    let round = open_round(&store, "open-find-6", &evidence_id).await["round_id"].clone();
    // Without a tier the criteria are required, so a counterexample against one returns the work.
    let returned = run(
        &store,
        REVIEWER_TOKEN,
        "ret-find-6",
        "review.return",
        decision_args(&round, json!({"findings": [blocking_finding("F1")]})),
    )
    .await;
    assert_eq!(returned["state"], "rejected");
    assert_eq!(returned["findings"]["recorded"][0]["severity"], "blocking");
}
