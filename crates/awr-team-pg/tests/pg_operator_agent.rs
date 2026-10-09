#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction, Id,
    IssueAuthorizationRequest, MemberIdentityKind, MemberIdentityMetadata, PersonId,
    RevokeAuthorizationRequest,
};
use awr_team_pg::{
    AccessPlan, AgentAuthorizationIssuePlan, AgentProvisionPlan, AgentRenewPlan,
    AuthorizationStore, OperatorAccess, OperatorAgent, PgError, workstream_credential_hash,
};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

const TOKEN: &str =
    "awr1.native-agent.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
fn access_plan() -> AccessPlan {
    serde_json::from_value(json!({"protocol_version":1,"tenant_id":TENANT,"project_id":PROJECT,
        "actor":{"id":"native-agent","kind":"agent","display_name":"Native developer"},
        "client_id":"native-client","role":"developer","agent_review":false,
        "grants":[{"workstream_id":awr_core::Id::from(1),"authority_version":"1","read":true,"write":true,
            "manage":false,"attest_execution":false,"reconcile_execution":false}],
        "credential":{"id":"native-agent","secret_hash":workstream_credential_hash(TOKEN).unwrap(),"expires_at_unix_ms":null},
        "revoke_credentials":[]})).unwrap()
}
async fn plan(admin: &Client) -> AgentProvisionPlan {
    let now: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    serde_json::from_value(
        json!({"protocol_version":1,"tenant_id":TENANT,"project_id":PROJECT,"authorization":{
        "id":"native-authorization","authorizer_person_id":"agent","responsible_person_id":"agent",
        "subject_kind":"agent","subject_id":"native-agent","client_id":"native-client",
        "scope":{"kind":"task","project_id":PROJECT,"work_item_id":"a"},
        "actions":["inspect","start_work","claim_coordination"],"status":"active",
        "verifiable_capabilities":[],"self_reported_skill_hints":[],"created_at_ms":now,
        "expires_at_ms":now+3600000,"binding_id":"native-binding"}}),
    )
    .unwrap()
}
async fn stage(admin: &mut Client) {
    let p = access_plan();
    let preview = OperatorAccess::preview(admin, &p).await.unwrap();
    OperatorAccess::apply(
        admin,
        &p,
        "access-stage",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
}
async fn apply(admin: &mut Client, p: &AgentProvisionPlan, request: &str) -> Value {
    let v = OperatorAgent::preview(admin, p).await.unwrap();
    OperatorAgent::apply(
        admin,
        p,
        request,
        v["state_digest"].as_str().unwrap(),
        v["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap()
}

async fn apply_access(admin: &mut Client, plan: &AccessPlan, request: &str) {
    let preview = OperatorAccess::preview(admin, plan).await.unwrap();
    OperatorAccess::apply(
        admin,
        plan,
        request,
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn pure_business_duties_can_start_checkpoint_and_end_without_development_authority() {
    use awr_team::BusinessRole;
    use std::collections::BTreeSet;

    for (duty, action, template) in [
        (
            BusinessRole::Reviewer,
            AuthorizedAction::Review,
            "developer",
        ),
        (BusinessRole::Reviewer, AuthorizedAction::Review, "reviewer"),
        (
            BusinessRole::Supervisor,
            AuthorizedAction::AssignWork,
            "maintainer",
        ),
        (
            BusinessRole::Deliverer,
            AuthorizedAction::FinalizeDelivery,
            "maintainer",
        ),
    ] {
        let (_guard, mut admin, _, store) = setup().await;
        let mut access = access_plan();
        access.role = template.into();
        access.business_roles = Some(BTreeSet::from([duty]));
        access.agent_review = duty == BusinessRole::Reviewer;
        access.assignment_grant = Some(duty == BusinessRole::Supervisor);
        apply_access(&mut admin, &access, "pure-duty-access").await;
        let mut provision = plan(&admin).await;
        provision.authorization.actions = BTreeSet::from([AuthorizedAction::Inspect, action]);
        apply(&mut admin, &provision, "pure-duty-provision").await;
        let count: i64 = admin
            .query_one(
                "SELECT count(*) FROM awr_team.sessions WHERE actor_id='native-agent'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 0, "the participant must have no preseeded session");
        let current = prepare(&store, TOKEN, "a").await;
        let started = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                TOKEN,
                command(
                    &current,
                    "pure-duty-start",
                    "session.start",
                    json!({"conversation_id":"pure-business-duty"}),
                ),
            )
            .await
            .expect("the duty must support its own first-use session");
        assert_eq!(started["receipt"]["execution_authorized"], false);
        let session = started["receipt"]["data"]["session_id"].as_str().unwrap();
        let current = prepare(&store, TOKEN, "a").await;
        for (request, op, args) in [
            (
                "pure-duty-no-claim",
                "claim.acquire",
                json!({"session_id":session,"expected_session_version":"1",
                    "expected_work_version":current["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
                    "ttl_seconds":60}),
            ),
            (
                "pure-duty-no-execution",
                "execution.prepare",
                json!({"session_id":session,"expected_session_version":"1","claim_id":"missing",
                    "expected_fence":"1","expected_lease_version":"1","expected_work_version":"1",
                    "input_digest":"a".repeat(64),"declared_scope":["src/example.rs"]}),
            ),
            (
                "pure-duty-no-other-session",
                "session.end",
                json!({"session_id":"session-a","expected_session_version":"1"}),
            ),
        ] {
            let before = snapshot(&admin).await;
            assert!(
                matches!(
                    store
                        .commands()
                        .execute(TENANT, PROJECT, TOKEN, command(&current, request, op, args))
                        .await,
                    Err(PgError::Forbidden)
                ),
                "{duty:?}: {op} must not gain unrelated authority"
            );
            assert_eq!(snapshot(&admin).await, before);
        }
        let mut outside = query("work.prepare");
        outside.work_id = Some("c".into());
        assert!(matches!(
            store.query(TENANT, PROJECT, TOKEN, outside).await,
            Err(PgError::Forbidden)
        ));
        let saved = store.commands().execute(TENANT, PROJECT, TOKEN,
            command(&current, "pure-duty-checkpoint", "session.checkpoint",
                json!({"session_id":session,"expected_session_version":"1",
                    "context_hash":current["data"]["context_hash"],
                    "next_action":"Inspect the pending business result","open_loops":[],
                    "progress":{"phase":"reviewing","summary":"Read the current material; waiting for a submitted result."}}))).await.unwrap();
        assert_eq!(saved["receipt"]["data"]["session_version"], "2");
        let current = prepare(&store, TOKEN, "a").await;
        let ended = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                TOKEN,
                command(
                    &current,
                    "pure-duty-end",
                    "session.end",
                    json!({"session_id":session,"expected_session_version":"2"}),
                ),
            )
            .await
            .unwrap();
        assert_eq!(ended["receipt"]["data"]["state"], "ended");
        let row = admin
            .query_one(
                "SELECT actor_id,client_id,(SELECT count(*) FROM awr_team.claims),
                (SELECT count(*) FROM awr_team.executions),
                (SELECT count(*) FROM awr_team.evidence)
             FROM awr_team.sessions WHERE id=$1",
                &[&session],
            )
            .await
            .unwrap();
        assert_eq!(row.get::<_, String>(0), "native-agent");
        assert_eq!(row.get::<_, String>(1), "native-client");
        for column in 2..5 {
            assert_eq!(row.get::<_, i64>(column), 0);
        }
    }
}

#[tokio::test]
async fn pure_reviewer_requires_both_review_membership_and_session_delegation() {
    use awr_team::BusinessRole;
    use std::collections::BTreeSet;

    for missing in ["membership", "delegation"] {
        let (_guard, mut admin, _, store) = setup().await;
        let mut access = access_plan();
        access.role = "reviewer".into();
        access.business_roles = Some(BTreeSet::from([BusinessRole::Reviewer]));
        access.agent_review = missing != "membership";
        apply_access(&mut admin, &access, "reviewer-access").await;
        let mut provision = plan(&admin).await;
        provision.authorization.actions = BTreeSet::from([AuthorizedAction::Inspect]);
        if missing == "membership" {
            provision
                .authorization
                .actions
                .insert(AuthorizedAction::Review);
            let before = snapshot(&admin).await;
            assert!(matches!(
                OperatorAgent::preview(&mut admin, &provision).await,
                Err(PgError::Forbidden)
            ));
            assert_eq!(snapshot(&admin).await, before);
        } else {
            apply(&mut admin, &provision, "inspect-only-reviewer").await;
            let current = prepare(&store, TOKEN, "a").await;
            let before = snapshot(&admin).await;
            assert!(matches!(
                store
                    .commands()
                    .execute(
                        TENANT,
                        PROJECT,
                        TOKEN,
                        command(
                            &current,
                            "inspect-only-no-session",
                            "session.start",
                            json!({"conversation_id":"review-without-delegation"})
                        )
                    )
                    .await,
                Err(PgError::Forbidden)
            ));
            assert_eq!(snapshot(&admin).await, before);
        }
    }
}

#[tokio::test]
async fn pure_review_session_ownership_stays_bound_to_the_credential_client() {
    use awr_team::BusinessRole;
    use std::collections::BTreeSet;

    let (_guard, mut admin, db, store) = setup().await;
    let mut access = access_plan();
    access.role = "reviewer".into();
    access.business_roles = Some(BTreeSet::from([BusinessRole::Reviewer]));
    access.agent_review = true;
    apply_access(&mut admin, &access, "reviewer-first-client").await;
    let mut provision = plan(&admin).await;
    provision.authorization.actions =
        BTreeSet::from([AuthorizedAction::Inspect, AuthorizedAction::Review]);
    apply(&mut admin, &provision, "reviewer-first-delegation").await;
    let started = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            TOKEN,
            command(
                &prepare(&store, TOKEN, "a").await,
                "reviewer-first-session",
                "session.start",
                json!({"conversation_id":"first-client"}),
            ),
        )
        .await
        .unwrap();
    let session = started["receipt"]["data"]["session_id"].as_str().unwrap();

    let other_token = format!("awr1.native-other.{}", "e".repeat(64));
    access.client_id = "native-other-client".into();
    let credential = access.credential.as_mut().unwrap();
    credential.id = "native-other".into();
    credential.secret_hash = workstream_credential_hash(&other_token).unwrap();
    apply_access(&mut admin, &access, "reviewer-second-client").await;
    let mut other_grant = provision.authorization.clone();
    other_grant.id = "native-other-authorization".into();
    other_grant.client_id = access.client_id.clone();
    let authorizations =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    authorizations
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "reviewer-second-delegation".into(),
                authorization: other_grant,
            },
        )
        .await
        .unwrap();
    let current = prepare(&store, &other_token, "a").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                &other_token,
                command(
                    &current,
                    "reviewer-other-client-end",
                    "session.end",
                    json!({"session_id":session,"expected_session_version":"1"})
                )
            )
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
    // The second client really has session authority, but only over its own session.
    let other = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            &other_token,
            command(
                &current,
                "reviewer-other-client-start",
                "session.start",
                json!({"conversation_id":"second-client"}),
            ),
        )
        .await
        .unwrap();
    assert_ne!(other["receipt"]["data"]["session_id"], json!(session));
}

#[tokio::test]
async fn pure_review_session_rechecks_live_access_and_delegation_on_commands_and_replays() {
    use awr_team::BusinessRole;
    use std::collections::BTreeSet;

    let (_guard, mut admin, db, store) = setup().await;
    let mut access = access_plan();
    access.role = "reviewer".into();
    access.agent_review = true;
    access.business_roles = Some(BTreeSet::from([BusinessRole::Reviewer]));
    apply_access(&mut admin, &access, "reviewer-access").await;
    let mut provision = plan(&admin).await;
    provision.authorization.actions =
        BTreeSet::from([AuthorizedAction::Inspect, AuthorizedAction::Review]);
    apply(&mut admin, &provision, "reviewer-delegation").await;
    let start = command(
        &prepare(&store, TOKEN, "a").await,
        "reviewer-session",
        "session.start",
        json!({"conversation_id":"reviewer"}),
    );
    let started = store
        .commands()
        .execute(TENANT, PROJECT, TOKEN, start.clone())
        .await
        .unwrap();
    let session = started["receipt"]["data"]["session_id"].as_str().unwrap();
    let end = command(
        &prepare(&store, TOKEN, "a").await,
        "reviewer-end",
        "session.end",
        json!({"session_id":session,"expected_session_version":"1"}),
    );

    for (change, undo) in [
        (
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE actor_id='native-agent'",
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE actor_id='native-agent'",
        ),
        (
            "UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{expires_at_ms}','0'),expires_at_ms=0 WHERE id='native-authorization'",
            "UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{expires_at_ms}',to_jsonb($1::bigint)),expires_at_ms=$1 WHERE id='native-authorization'",
        ),
        (
            "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='native-binding'",
            "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='native-binding'",
        ),
    ] {
        admin.batch_execute(change).await.unwrap();
        let before = snapshot(&admin).await;
        for request in [&end, &start] {
            assert!(
                matches!(
                    store
                        .commands()
                        .execute(TENANT, PROJECT, TOKEN, request.clone())
                        .await,
                    Err(PgError::Forbidden)
                ),
                "{change}"
            );
        }
        assert_eq!(snapshot(&admin).await, before);
        if undo.contains("$1") {
            admin
                .execute(undo, &[&provision.authorization.expires_at_ms.unwrap()])
                .await
                .unwrap();
        } else {
            admin.batch_execute(undo).await.unwrap();
        }
    }
    access.credential = None;
    access.business_roles = Some(BTreeSet::from([BusinessRole::Observer]));
    apply_access(&mut admin, &access, "reviewer-duty-narrowed").await;
    for request in [&end, &start] {
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, TOKEN, request.clone())
                .await,
            Err(PgError::Forbidden)
        ));
    }
    access.business_roles = Some(BTreeSet::from([BusinessRole::Reviewer]));
    apply_access(&mut admin, &access, "reviewer-duty-restored").await;
    let authorizations =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    authorizations
        .revoke(
            TENANT,
            PROJECT,
            &RevokeAuthorizationRequest {
                request_key: "reviewer-revoked".into(),
                authorization_id: provision.authorization.id.clone(),
                revoked_by: provision.authorization.responsible_person_id.clone(),
                revoked_at_ms: provision.authorization.created_at_ms + 1,
                reason: "review duty ended".into(),
            },
        )
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for request in [&end, &start] {
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, TOKEN, request.clone())
                .await,
            Err(PgError::Forbidden)
        ));
    }
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn supervisor_project_provisioning_requires_explicit_membership_ceiling_and_all_streams() {
    let (_guard, mut admin, _, _) = setup().await;
    let mut access = access_plan();
    access.role = "maintainer".into();
    access.business_roles = Some(std::collections::BTreeSet::from([
        awr_team::BusinessRole::Supervisor,
        awr_team::BusinessRole::Deliverer,
    ]));
    let mut second = access.grants[0].clone();
    second.workstream_id = Id::from(2);
    access.grants.push(second);
    let preview = OperatorAccess::preview(&mut admin, &access).await.unwrap();
    OperatorAccess::apply(
        &mut admin,
        &access,
        "supervisor-access",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    let mut p = plan(&admin).await;
    p.authorization.scope = AuthorizationScope::Project {
        project_id: PROJECT.into(),
    };
    p.authorization.actions = std::collections::BTreeSet::from([
        AuthorizedAction::Inspect,
        AuthorizedAction::AssignWork,
        AuthorizedAction::EditPlanning,
        AuthorizedAction::ApprovePlanning,
        AuthorizedAction::PublishPlanning,
        AuthorizedAction::FinalizeDelivery,
    ]);
    assert!(OperatorAgent::preview(&mut admin, &p).await.is_err());
    access.assignment_grant = Some(true);
    let preview = OperatorAccess::preview(&mut admin, &access).await.unwrap();
    OperatorAccess::apply(
        &mut admin,
        &access,
        "supervisor-assignment",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert!(OperatorAgent::preview(&mut admin, &p).await.is_ok());
    access.grants[1].write = false;
    let preview = OperatorAccess::preview(&mut admin, &access).await.unwrap();
    OperatorAccess::apply(
        &mut admin,
        &access,
        "supervisor-stream-narrow",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert!(OperatorAgent::preview(&mut admin, &p).await.is_err());
    access.grants[1].write = true;
    access.business_roles = Some(std::collections::BTreeSet::from([
        awr_team::BusinessRole::Observer,
    ]));
    let preview = OperatorAccess::preview(&mut admin, &access).await.unwrap();
    OperatorAccess::apply(
        &mut admin,
        &access,
        "supervisor-duty-narrow",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert!(OperatorAgent::preview(&mut admin, &p).await.is_err());
    access.business_roles = Some(std::collections::BTreeSet::from([
        awr_team::BusinessRole::Supervisor,
        awr_team::BusinessRole::Deliverer,
    ]));
    let preview = OperatorAccess::preview(&mut admin, &access).await.unwrap();
    OperatorAccess::apply(
        &mut admin,
        &access,
        "supervisor-duty-approved",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    apply(&mut admin, &p, "supervisor-provision").await;
    assert_eq!(
        OperatorAgent::inspect(&mut admin, &p).await.unwrap()["configuration_matches_plan"],
        true
    );
    assert!(!p.authorization.actions.contains(&AuthorizedAction::Review));
}

async fn expired_renewal_plan(admin: &Client, provision: &AgentProvisionPlan) -> AgentRenewPlan {
    let now: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let mut previous = provision.authorization.clone();
    previous.created_at_ms = now - 7_200_000;
    previous.expires_at_ms = Some(now - 3_600_000);
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET created_at_ms=$1,expires_at_ms=$2,body_json=$3 WHERE tenant_id=$4 AND project_id=$5 AND id=$6",
            &[
                &previous.created_at_ms,
                &previous.expires_at_ms,
                &json!(previous),
                &TENANT,
                &PROJECT,
                &previous.id,
            ],
        )
        .await
        .unwrap();
    let mut successor = previous.clone();
    successor.id = "native-authorization-renewed".into();
    successor.created_at_ms = now;
    successor.expires_at_ms = Some(now + 3_600_000);
    AgentRenewPlan {
        protocol_version: 1,
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        previous_authorization_id: previous.id,
        authorization: successor,
        member_identity: provision.member_identity.clone(),
    }
}

async fn grant_second_stream(admin: &Client) {
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write) VALUES($1,$2,'native-agent','native-client',$3,1,true,true)",
            &[&TENANT, &PROJECT, &Id::from(2).to_string()],
        )
        .await
        .unwrap();
}

async fn authorization_issue_plan(
    admin: &Client,
    id: &str,
    scope: AuthorizationScope,
) -> AgentAuthorizationIssuePlan {
    let mut authorization = plan(admin).await.authorization;
    authorization.id = id.into();
    authorization.scope = scope;
    AgentAuthorizationIssuePlan {
        protocol_version: 1,
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        authorization,
        member_identity: None,
    }
}

async fn preserved_authority(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'person',(SELECT to_jsonb(t) FROM awr_team.persons t WHERE id='agent'),
        'bindings',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM awr_team.person_agent_bindings t WHERE agent_id='native-agent'),
        'authorization',(SELECT to_jsonb(t) FROM awr_team.agent_authorizations t WHERE id='native-authorization'),
        'actor',(SELECT to_jsonb(t) FROM awr_team.actors t WHERE id='native-agent'),
        'credential',(SELECT to_jsonb(t) FROM awr_team.credentials t WHERE id='native-agent'),
        'membership',(SELECT to_jsonb(t) FROM awr_team.project_memberships t WHERE actor_id='native-agent'),
        'grants',(SELECT jsonb_agg(to_jsonb(t) ORDER BY workstream_id) FROM awr_team.workstream_grants t WHERE actor_id='native-agent' AND client_id='native-client'))",&[]).await.unwrap().get(0)
}
async fn snapshot(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'people',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM awr_team.persons t),
        'bindings',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM awr_team.person_agent_bindings t),
        'authorizations',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM awr_team.agent_authorizations t),
        'authorization_receipts',(SELECT jsonb_agg(to_jsonb(t) ORDER BY request_key) FROM awr_team.agent_authorization_receipts t),
        'access_receipts',(SELECT jsonb_agg(to_jsonb(t) ORDER BY request_id) FROM awr_team.access_changes t),
        'events',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM awr_team.events t),
        'audit',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM awr_team.ops_audit_records t),
        'projects',(SELECT jsonb_agg(to_jsonb(t) ORDER BY tenant_id,id) FROM awr_team.projects t),
        'actors',(SELECT jsonb_agg(to_jsonb(t) ORDER BY tenant_id,id) FROM awr_team.actors t))",&[]).await.unwrap().get(0)
}

async fn simulated_plan(admin: &mut Client, name: &str) -> (AgentProvisionPlan, String) {
    let member = format!("{name}-member");
    let actor = format!("{name}-agent");
    let client = format!("{name}-client");
    let token = format!(
        "awr1.{actor}.{}",
        awr_team::request_hash(&json!({"synthetic_credential_seed":name})).unwrap()
    );
    for is_member in [true, false] {
        let mut access = access_plan();
        access.actor.id = if is_member { &member } else { &actor }.clone();
        access.actor.display_name = access.actor.id.clone();
        access.client_id = if is_member {
            format!("{name}-member-client")
        } else {
            client.clone()
        };
        access.role = if is_member { "reader" } else { "developer" }.into();
        access.grants[0].write = !is_member;
        access.credential = if is_member {
            None
        } else {
            let mut credential = access.credential.unwrap();
            credential.id = actor.clone();
            credential.secret_hash = workstream_credential_hash(&token).unwrap();
            Some(credential)
        };
        let preview = OperatorAccess::preview(admin, &access).await.unwrap();
        OperatorAccess::apply(
            admin,
            &access,
            &format!("{name}-access-{is_member}"),
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    }
    let mut provision = plan(admin).await;
    let authorization = &mut provision.authorization;
    authorization.id = format!("{name}-authorization");
    authorization.authorizer_person_id = PersonId::new(&member).unwrap();
    authorization.responsible_person_id = PersonId::new(&member).unwrap();
    authorization.subject_id = actor;
    authorization.client_id = client;
    authorization.scope = AuthorizationScope::Workstream {
        project_id: PROJECT.into(),
        workstream_id: Id::from(1).to_string(),
    };
    authorization.binding_id = Some(format!("{name}-binding"));
    provision.member_identity = Some(MemberIdentityMetadata {
        kind: MemberIdentityKind::SimulatedMember,
        controller_ref: Some("shared-experiment-controller".into()),
    });
    (provision, token)
}

#[tokio::test]
async fn simulated_members_share_a_controller_without_sharing_identity_or_credentials() {
    let (_guard, mut admin, _, store) = setup().await;
    let (first, first_token) = simulated_plan(&mut admin, "alpha").await;
    let (second, second_token) = simulated_plan(&mut admin, "beta").await;
    let mut receipts = Vec::new();
    for (plan, token) in [(&first, &first_token), (&second, &second_token)] {
        let receipt = apply(&mut admin, plan, &plan.authorization.id).await;
        assert_eq!(
            receipt["receipt"]["member_identity"],
            json!(plan.member_identity)
        );
        assert_eq!(receipt["receipt"]["human_approval"], false);
        assert_eq!(receipt["receipt"]["team_independent_acceptance"], false);
        let current = OperatorAgent::inspect(&mut admin, plan).await.unwrap();
        assert_eq!(current["configuration_matches_plan"], true);
        assert_eq!(current["state"]["human_actor"]["kind"], "agent");
        let caps = store
            .query(TENANT, PROJECT, token, query("capabilities"))
            .await
            .unwrap();
        assert_eq!(caps["identity"]["actor_id"], plan.authorization.subject_id);
        assert_eq!(caps["identity"]["client_id"], plan.authorization.client_id);
        assert_eq!(caps["identity"]["can_manage_members"], false);
        let prepared = prepare(&store, token, "a").await;
        assert_eq!(prepared["data"]["work_id"], "a");
        let mut sibling = query("work.prepare");
        sibling.work_id = Some("b-private".into());
        assert!(matches!(
            store.query(TENANT, PROJECT, token, sibling).await,
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            store
                .query("other-tenant", PROJECT, token, query("capabilities"))
                .await,
            Err(PgError::Forbidden)
        ));
        receipts.push(receipt);
    }
    assert_ne!(
        first.authorization.responsible_person_id,
        second.authorization.responsible_person_id
    );
    assert_ne!(
        first.authorization.subject_id,
        second.authorization.subject_id
    );
    assert_ne!(
        first.authorization.client_id,
        second.authorization.client_id
    );
    assert_ne!(
        first.authorization.binding_id,
        second.authorization.binding_id
    );
    assert_ne!(first_token, second_token);
    let rows = admin
        .query(
            "SELECT id,member_identity FROM awr_team.persons WHERE id=ANY($1) ORDER BY id",
            &[&vec![
                first.authorization.responsible_person_id.to_string(),
                second.authorization.responsible_person_id.to_string(),
            ]],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<_, Value>(1), rows[1].get::<_, Value>(1));
    let before = snapshot(&admin).await;
    let original = &receipts[0]["receipt"];
    let replay = OperatorAgent::apply(
        &mut admin,
        &first,
        &first.authorization.id,
        original["before_digest"].as_str().unwrap(),
        original["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(snapshot(&admin).await, before);
    let mut changed = first.clone();
    changed.member_identity.as_mut().unwrap().controller_ref = Some("different-controller".into());
    assert!(matches!(
        OperatorAgent::apply(
            &mut admin,
            &changed,
            &first.authorization.id,
            original["before_digest"].as_str().unwrap(),
            original["plan_digest"].as_str().unwrap()
        )
        .await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn schema_upgrade_keeps_legacy_members_unspecified_and_validates_metadata_shape() {
    // A real version-36 database built from the migration files themselves. The
    // previous version of this test reverted the DDL of migrations 37-40 by hand,
    // so every later migration broke it (migration 41 did, and it stayed red).
    let (_guard, admin, _) = common::historical_team_schema(36).await;
    admin
        .execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES($1,'Readers','active')",
            &[&TENANT],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
             VALUES($1,$2,'p','team','epoch-a','active')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES($1,$2,'legacy-member','Legacy member','active')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    let before: Value = admin
        .query_one(
            "SELECT to_jsonb(p) FROM awr_team.persons p WHERE id='legacy-member'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::check_schema(&admin).await.unwrap();
    let row = admin
        .query_one(
            "SELECT to_jsonb(p)-'member_identity',member_identity FROM awr_team.persons p WHERE id='legacy-member'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, Value>(0), before);
    assert_eq!(row.get::<_, Option<Value>>(1), None);
    for metadata in [
        json!({"kind":"simulated_member","controller_ref":"shared-controller"}),
        json!({"kind":"human"}),
    ] {
        admin
            .execute(
                "UPDATE awr_team.persons SET member_identity=$1 WHERE id='legacy-member'",
                &[&metadata],
            )
            .await
            .unwrap();
    }
    for metadata in [
        json!({}),
        json!({"kind":null}),
        json!({"kind":"model"}),
        json!({"kind":"human","permission":"admin"}),
        json!({"kind":"simulated_member","controller_ref":""}),
        json!({"kind":"simulated_member","controller_ref":"界".repeat(43)}),
        json!({"kind":"simulated_member","controller_ref":"control\ncharacter"}),
    ] {
        assert!(
            admin
                .execute(
                    "UPDATE awr_team.persons SET member_identity=$1 WHERE id='legacy-member'",
                    &[&metadata]
                )
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn declared_human_metadata_and_unspecified_legacy_metadata_remain_distinct() {
    for declared in [false, true] {
        let (_guard, mut admin, _, _) = setup().await;
        stage(&mut admin).await;
        let mut provision = plan(&admin).await;
        if declared {
            provision.member_identity = Some(MemberIdentityMetadata {
                kind: MemberIdentityKind::Human,
                controller_ref: None,
            });
        }
        apply(&mut admin, &provision, "explicit-or-legacy").await;
        let current = OperatorAgent::inspect(&mut admin, &provision)
            .await
            .unwrap();
        assert_eq!(
            current["state"]["person"]["member_identity"],
            json!(provision.member_identity)
        );
        assert_eq!(current["human_approval"], false);
        let serialized = serde_json::to_value(&provision).unwrap();
        assert_eq!(serialized.get("member_identity").is_some(), declared);
        let mut renewal = expired_renewal_plan(&admin, &provision).await;
        if !declared {
            renewal.member_identity = Some(MemberIdentityMetadata {
                kind: MemberIdentityKind::Human,
                controller_ref: None,
            });
            assert!(matches!(
                OperatorAgent::renew_preview(&mut admin, &renewal).await,
                Err(PgError::Forbidden)
            ));
        }
    }
}

#[tokio::test]
async fn simulated_registration_cannot_relabel_a_human_system_or_existing_binding() {
    let (_guard, mut admin, _, _) = setup().await;
    stage(&mut admin).await;
    let mut invalid_human = plan(&admin).await;
    invalid_human.member_identity = Some(MemberIdentityMetadata {
        kind: MemberIdentityKind::SimulatedMember,
        controller_ref: None,
    });
    let before = snapshot(&admin).await;
    assert!(matches!(
        OperatorAgent::preview(&mut admin, &invalid_human).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let (provision, _) = simulated_plan(&mut admin, "alpha").await;
    let before = snapshot(&admin).await;
    let mut declared_human = provision.clone();
    declared_human.member_identity.as_mut().unwrap().kind = MemberIdentityKind::Human;
    assert!(matches!(
        OperatorAgent::preview(&mut admin, &declared_human).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .execute(
            "UPDATE awr_team.actors SET kind='system' WHERE id=$1",
            &[&provision.authorization.responsible_person_id.as_str()],
        )
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        OperatorAgent::preview(&mut admin, &provision).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE id=$1",
            &[&provision.authorization.responsible_person_id.as_str()],
        )
        .await
        .unwrap();
    apply(&mut admin, &provision, "simulated-provision").await;
    let before = snapshot(&admin).await;
    let mut moved = provision.clone();
    moved.authorization.authorizer_person_id = PersonId::new("agent").unwrap();
    moved.authorization.responsible_person_id = PersonId::new("agent").unwrap();
    moved.member_identity = None;
    assert!(matches!(
        OperatorAgent::preview(&mut admin, &moved).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let mut inspection = provision.clone();
    inspection.member_identity = None;
    let current = OperatorAgent::inspect(&mut admin, &inspection)
        .await
        .unwrap();
    assert_eq!(current["configuration_matches_plan"], true);
    assert_eq!(
        current["state"]["person"]["member_identity"]["kind"],
        "simulated_member"
    );
}

#[tokio::test]
async fn simulated_member_credentials_and_delegations_recheck_expiry_and_revocation() {
    let (_guard, mut admin, _, store) = setup().await;
    let (first, first_token) = simulated_plan(&mut admin, "alpha").await;
    let (second, second_token) = simulated_plan(&mut admin, "beta").await;
    apply(&mut admin, &first, "first-member").await;
    apply(&mut admin, &second, "second-member").await;
    for (deny, restore) in [
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 second' WHERE id='alpha-agent'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='alpha-agent'",
        ),
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='alpha-agent'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='alpha-agent'",
        ),
        (
            "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='alpha-binding'",
            "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='alpha-binding'",
        ),
    ] {
        admin.batch_execute(deny).await.unwrap();
        let mut own = query("work.prepare");
        own.work_id = Some("a".into());
        assert!(matches!(
            store.query(TENANT, PROJECT, &first_token, own).await,
            Err(PgError::Forbidden)
        ));
        prepare(&store, &second_token, "a").await;
        assert_eq!(
            OperatorAgent::inspect(&mut admin, &first).await.unwrap()["configuration_matches_plan"],
            false
        );
        admin.batch_execute(restore).await.unwrap();
        prepare(&store, &first_token, "a").await;
    }
    let mut expired = first.authorization.clone();
    expired.expires_at_ms = Some(expired.created_at_ms);
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET expires_at_ms=$1,body_json=$2 WHERE id=$3",
            &[&expired.expires_at_ms, &json!(expired), &expired.id],
        )
        .await
        .unwrap();
    let mut own = query("work.prepare");
    own.work_id = Some("a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, &first_token, own).await,
        Err(PgError::Forbidden)
    ));
    prepare(&store, &second_token, "a").await;
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET expires_at_ms=$1,body_json=$2 WHERE id=$3",
            &[
                &first.authorization.expires_at_ms,
                &json!(first.authorization),
                &first.authorization.id,
            ],
        )
        .await
        .unwrap();
    let renewal = expired_renewal_plan(&admin, &first).await;
    let before = snapshot(&admin).await;
    let mut changed = renewal.clone();
    changed.member_identity.as_mut().unwrap().controller_ref = Some("changed-controller".into());
    assert!(matches!(
        OperatorAgent::renew_preview(&mut admin, &changed).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let preview = OperatorAgent::renew_preview(&mut admin, &renewal)
        .await
        .unwrap();
    OperatorAgent::renew_apply(
        &mut admin,
        &renewal,
        "simulated-renewal",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    prepare(&store, &first_token, "a").await;
    let mut revoked = renewal.authorization.clone();
    revoked.status = AuthorizationStatus::Revoked;
    revoked.revoked_at_ms = Some(revoked.created_at_ms + 1);
    revoked.revoked_by = Some(revoked.authorizer_person_id.clone());
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked',body_json=$1 WHERE id=$2",
            &[&json!(revoked), &revoked.id],
        )
        .await
        .unwrap();
    let mut own = query("work.prepare");
    own.work_id = Some("a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, &first_token, own).await,
        Err(PgError::Forbidden)
    ));
    prepare(&store, &second_token, "a").await;
}

#[tokio::test]
async fn explicit_suggestion_only_operator_plan_admits_bounded_native_suggestion() {
    let (_guard, mut admin, db, _) = setup().await;
    stage(&mut admin).await;
    let mut provision = plan(&admin).await;
    provision.authorization.actions =
        std::collections::BTreeSet::from([AuthorizedAction::ProposePlanning]);
    let receipt = apply(&mut admin, &provision, "suggestion-only-provision").await;
    assert_eq!(receipt["execution_authorized"], false);
    let store =
        awr_team_pg::SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let request = awr_team_pg::PlanningSuggestRequest {
        protocol_version: 1,
        request_id: "native-bounded-suggestion".into(),
        rationale: "A prerequisite needs planner review.".into(),
        affected_work_keys: vec!["a".into()],
        proposed_notes: json!({"reason":"dependency"}),
        author_person_id: None,
    };
    let result = store
        .planning_suggest(TENANT, PROJECT, TOKEN, &request)
        .await
        .unwrap();
    assert_eq!(result["result"]["claimable"], false);
    assert_eq!(result["result"]["adds_formal_work"], false);
    let sibling = awr_team_pg::PlanningSuggestRequest {
        request_id: "native-uncovered-suggestion".into(),
        affected_work_keys: vec!["c".into()],
        ..request
    };
    assert!(matches!(
        store
            .planning_suggest(TENANT, PROJECT, TOKEN, &sibling)
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn suggestion_overlay_preserves_development_authority_and_rejects_action_overlap() {
    let (_guard, mut admin, db, store) = setup().await;
    stage(&mut admin).await;
    let initial = plan(&admin).await;
    apply(&mut admin, &initial, "initial-development").await;
    let mut overlay = authorization_issue_plan(
        &admin,
        "suggestion-overlay",
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    overlay.authorization.actions =
        std::collections::BTreeSet::from([AuthorizedAction::ProposePlanning]);
    overlay.authorization.expires_at_ms = initial.authorization.expires_at_ms;
    for action in [
        AuthorizedAction::Inspect,
        AuthorizedAction::ClaimCoordination,
        AuthorizedAction::StartWork,
    ] {
        let mut partially_overlapping = overlay.clone();
        partially_overlapping.authorization.actions.insert(action);
        let unchanged = snapshot(&admin).await;
        assert!(matches!(
            OperatorAgent::authorize_preview(&mut admin, &partially_overlapping).await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, unchanged);
    }
    let preserved = preserved_authority(&admin).await;
    let before = snapshot(&admin).await;
    let preview = OperatorAgent::authorize_preview(&mut admin, &overlay)
        .await
        .unwrap();
    assert_eq!(snapshot(&admin).await, before);
    let applied = OperatorAgent::authorize_apply(
        &mut admin,
        &overlay,
        "add-suggestion-overlay",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(applied["execution_authorized"], false);
    assert_eq!(preserved_authority(&admin).await, preserved);
    let after = snapshot(&admin).await;
    let replay = OperatorAgent::authorize_apply(
        &mut admin,
        &overlay,
        "add-suggestion-overlay",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], applied["receipt"]);
    assert_eq!(snapshot(&admin).await, after);
    assert_eq!(prepare(&store, TOKEN, "a").await["data"]["work_id"], "a");
    let mut sibling_read = query("work.prepare");
    sibling_read.work_id = Some("c".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, TOKEN, sibling_read).await,
        Err(PgError::Forbidden)
    ));

    let source =
        awr_team_pg::SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let request = awr_team_pg::PlanningSuggestRequest {
        protocol_version: 1,
        request_id: "overlay-bounded-suggestion".into(),
        rationale: "The assigned work needs a prerequisite reviewed by a planner.".into(),
        affected_work_keys: vec!["a".into(), "c".into()],
        proposed_notes: json!({"reason":"dependency"}),
        author_person_id: None,
    };
    let suggestion = source
        .planning_suggest(TENANT, PROJECT, TOKEN, &request)
        .await
        .unwrap();
    assert_eq!(suggestion["result"]["claimable"], false);
    assert_eq!(suggestion["result"]["adds_formal_work"], false);
    let outside = awr_team_pg::PlanningSuggestRequest {
        request_id: "overlay-uncovered-suggestion".into(),
        affected_work_keys: vec!["b-private".into()],
        ..request
    };
    assert!(matches!(
        source
            .planning_suggest(TENANT, PROJECT, TOKEN, &outside)
            .await,
        Err(PgError::Forbidden)
    ));

    let mut duplicate = overlay.clone();
    duplicate.authorization.id = "duplicate-overlay".into();
    let unchanged = snapshot(&admin).await;
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &duplicate).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, unchanged);
    assert_eq!(preserved_authority(&admin).await, preserved);
}

#[tokio::test]
async fn renewal_preserves_disjoint_suggestion_overlay_in_overlapping_scope() {
    for revoke_overlay in [false, true] {
        let (_guard, mut admin, db, store) = setup().await;
        stage(&mut admin).await;
        let initial = plan(&admin).await;
        apply(&mut admin, &initial, "initial-development").await;
        let renewal = expired_renewal_plan(&admin, &initial).await;
        let mut overlay = authorization_issue_plan(
            &admin,
            "suggestion-overlay",
            AuthorizationScope::Workstream {
                project_id: PROJECT.into(),
                workstream_id: Id::from(1).to_string(),
            },
        )
        .await;
        overlay.authorization.actions =
            std::collections::BTreeSet::from([AuthorizedAction::ProposePlanning]);
        let preview = OperatorAgent::authorize_preview(&mut admin, &overlay)
            .await
            .unwrap();
        OperatorAgent::authorize_apply(
            &mut admin,
            &overlay,
            "add-suggestion-overlay",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
        if revoke_overlay {
            let auth =
                AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
            auth.revoke(
                TENANT,
                PROJECT,
                &RevokeAuthorizationRequest {
                    request_key: "revoke-suggestion-overlay".into(),
                    authorization_id: overlay.authorization.id.clone(),
                    revoked_by: overlay.authorization.responsible_person_id.clone(),
                    revoked_at_ms: overlay.authorization.created_at_ms + 1,
                    reason: "Stop suggestions without replacing development authority".into(),
                },
            )
            .await
            .unwrap();
        }
        let overlay_before: Value = admin
            .query_one(
                "SELECT body_json FROM awr_team.agent_authorizations WHERE id=$1",
                &[&overlay.authorization.id],
            )
            .await
            .unwrap()
            .get(0);
        let preserved = preserved_authority(&admin).await;
        let before = snapshot(&admin).await;
        let preview = OperatorAgent::renew_preview(&mut admin, &renewal)
            .await
            .unwrap();
        assert_eq!(snapshot(&admin).await, before);
        OperatorAgent::renew_apply(
            &mut admin,
            &renewal,
            "renew-development",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(preserved_authority(&admin).await, preserved);
        let overlay_after: Value = admin
            .query_one(
                "SELECT body_json FROM awr_team.agent_authorizations WHERE id=$1",
                &[&overlay.authorization.id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(overlay_after, overlay_before);
        assert_eq!(prepare(&store, TOKEN, "a").await["data"]["work_id"], "a");
    }
}

#[tokio::test]
async fn additional_workstream_authorization_preserves_history_and_is_discoverable() {
    let (_g, mut admin, _, store) = setup().await;
    stage(&mut admin).await;
    grant_second_stream(&admin).await;
    let mut initial = plan(&admin).await;
    initial.authorization.scope = AuthorizationScope::Workstream {
        project_id: PROJECT.into(),
        workstream_id: Id::from(1).to_string(),
    };
    apply(&mut admin, &initial, "agent-issue").await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES('reader-tenant','reader-project','reviewer','Reviewer','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','native-binding-disabled','reviewer','native-agent','disabled')",
        )
        .await
        .unwrap();
    let proposed = authorization_issue_plan(
        &admin,
        "native-authorization-second",
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(2).to_string(),
        },
    )
    .await;
    let preserved = preserved_authority(&admin).await;
    let before = snapshot(&admin).await;
    let preview = OperatorAgent::authorize_preview(&mut admin, &proposed)
        .await
        .unwrap();
    assert_eq!(snapshot(&admin).await, before);
    assert_eq!(preview["current"]["bindings"].as_array().unwrap().len(), 2);
    assert_eq!(
        preview["current"]["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|binding| binding["status"] == "active")
            .count(),
        1
    );
    assert_eq!(preview["binding_reused"], true);
    assert_eq!(preview["historical_authorizations_rewritten"], false);
    assert_eq!(
        OperatorAgent::authorize_outcome(&mut admin, TENANT, PROJECT, "authorize-second")
            .await
            .unwrap()["outcome"],
        "unknown"
    );
    let applied = OperatorAgent::authorize_apply(
        &mut admin,
        &proposed,
        "authorize-second",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        applied["receipt"]["scope"],
        json!(proposed.authorization.scope)
    );
    assert_eq!(
        applied["receipt"]["actions"],
        json!(proposed.authorization.actions)
    );
    assert_eq!(applied["receipt"]["human_approval"], false);
    assert_eq!(applied["receipt"]["team_independent_acceptance"], false);
    assert_eq!(preserved_authority(&admin).await, preserved);
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.agent_authorizations", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        2
    );

    let explicit = prepare(&store, TOKEN, "b-private").await;
    assert_eq!(explicit["data"]["work_id"], "b-private");
    let selector_free = store
        .query(TENANT, PROJECT, TOKEN, query("workstreams.list"))
        .await
        .unwrap();
    assert_eq!(selector_free["total"], 2);
    assert_eq!(selector_free["items"][0]["external_key"], "alpha");
    assert_eq!(selector_free["items"][1]["external_key"], "private-beta");
    let mut search = query("work.search");
    search.workstream_id =
        Some(serde_json::from_value(selector_free["items"][1]["id"].clone()).unwrap());
    search.search = Some("private".into());
    let found = store.query(TENANT, PROJECT, TOKEN, search).await.unwrap();
    assert_eq!(found["data"]["items"][0]["work_id"], "b-private");

    let after = snapshot(&admin).await;
    let replayed = OperatorAgent::authorize_apply(
        &mut admin,
        &proposed,
        "authorize-second",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["receipt"], applied["receipt"]);
    assert_eq!(snapshot(&admin).await, after);
    assert_eq!(
        OperatorAgent::authorize_outcome(&mut admin, TENANT, PROJECT, "authorize-second")
            .await
            .unwrap()["receipt"],
        applied["receipt"]
    );
    let mut conflicting = proposed.clone();
    conflicting.authorization.id = "native-authorization-conflict".into();
    assert!(matches!(
        OperatorAgent::authorize_apply(
            &mut admin,
            &conflicting,
            "authorize-second",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await,
        Err(PgError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn additional_authorization_rejects_invalid_overlap_elevation_and_stale_state() {
    let (_g, mut admin, db, _) = setup().await;
    stage(&mut admin).await;
    grant_second_stream(&admin).await;
    let mut initial = plan(&admin).await;
    initial.authorization.scope = AuthorizationScope::Workstream {
        project_id: PROJECT.into(),
        workstream_id: Id::from(1).to_string(),
    };
    apply(&mut admin, &initial, "agent-issue").await;
    let proposed = authorization_issue_plan(
        &admin,
        "native-authorization-second",
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(2).to_string(),
        },
    )
    .await;

    for invalid_case in [
        "authorizer",
        "session",
        "scope",
        "lifetime",
        "inactive",
        "action",
        "review",
    ] {
        let mut invalid = proposed.clone();
        match invalid_case {
            "authorizer" => {
                invalid.authorization.authorizer_person_id =
                    awr_core::PersonId::new("reviewer").unwrap()
            }
            "session" => invalid.authorization.session_id = Some("session-b".into()),
            "scope" => {
                invalid.authorization.scope = AuthorizationScope::Project {
                    project_id: PROJECT.into(),
                }
            }
            "lifetime" => invalid.authorization.expires_at_ms = None,
            "inactive" => {
                invalid.authorization.expires_at_ms = Some(invalid.authorization.created_at_ms)
            }
            "action" => {
                invalid
                    .authorization
                    .actions
                    .insert(AuthorizedAction::ManageAuthorization);
            }
            "review" => {
                invalid
                    .authorization
                    .actions
                    .insert(AuthorizedAction::Review);
            }
            _ => unreachable!(),
        }
        assert!(
            OperatorAgent::authorize_preview(&mut admin, &invalid)
                .await
                .is_err(),
            "{invalid_case}"
        );
    }

    let mut same_stream = proposed.clone();
    same_stream.authorization.scope = initial.authorization.scope.clone();
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &same_stream).await,
        Err(PgError::Forbidden)
    ));
    let mut covered_task = proposed.clone();
    covered_task.authorization.scope = AuthorizationScope::Task {
        project_id: PROJECT.into(),
        work_item_id: "a".into(),
    };
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &covered_task).await,
        Err(PgError::Forbidden)
    ));

    admin
        .execute(
            "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='native-binding'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &proposed).await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='native-binding'",
            &[],
        )
        .await
        .unwrap();
    admin
        .execute(
            "UPDATE awr_team.persons SET status='disabled' WHERE id='agent'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &proposed).await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.persons SET status='active' WHERE id='agent'",
            &[],
        )
        .await
        .unwrap();
    admin
        .execute(
            "UPDATE awr_team.actors SET status='disabled' WHERE id='native-agent'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &proposed).await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.actors SET status='active' WHERE id='native-agent'",
            &[],
        )
        .await
        .unwrap();
    admin
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='native-agent'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &proposed).await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='native-agent'",
            &[],
        )
        .await
        .unwrap();
    for elevated in [
        "can_manage",
        "can_attest_execution",
        "can_reconcile_execution",
    ] {
        admin
            .execute(
                &format!(
                    "UPDATE awr_team.workstream_grants SET {elevated}=true WHERE actor_id='native-agent' AND client_id='native-client' AND workstream_id=$1"
                ),
                &[&Id::from(2).to_string()],
            )
            .await
            .unwrap();
        assert!(
            matches!(
                OperatorAgent::authorize_preview(&mut admin, &proposed).await,
                Err(PgError::Forbidden)
            ),
            "{elevated}"
        );
        admin
            .execute(
                &format!(
                    "UPDATE awr_team.workstream_grants SET {elevated}=false WHERE actor_id='native-agent' AND client_id='native-client' AND workstream_id=$1"
                ),
                &[&Id::from(2).to_string()],
            )
            .await
            .unwrap();
    }

    let preview = OperatorAgent::authorize_preview(&mut admin, &proposed)
        .await
        .unwrap();
    let mut app = common::app_client(&db).await;
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut app, &proposed).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorAgent::authorize_apply(
            &mut app,
            &proposed,
            "app-authorize",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorAgent::authorize_outcome(&mut app, TENANT, PROJECT, "app-authorize").await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE actor_id='native-agent' AND client_id='native-client' AND workstream_id=$1",
            &[&Id::from(2).to_string()],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::authorize_apply(
            &mut admin,
            &proposed,
            "stale-grant",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE actor_id='native-agent' AND client_id='native-client' AND workstream_id=$1",
            &[&Id::from(2).to_string()],
        )
        .await
        .unwrap();

    let task_plan = authorization_issue_plan(
        &admin,
        "native-authorization-task-b",
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "b-private".into(),
        },
    )
    .await;
    let task_preview = OperatorAgent::authorize_preview(&mut admin, &task_plan)
        .await
        .unwrap();
    admin
        .execute(
            "UPDATE awr_team.work_contracts SET definition_state='archived' WHERE work_id='b-private'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::authorize_apply(
            &mut admin,
            &task_plan,
            "stale-source",
            task_preview["state_digest"].as_str().unwrap(),
            task_preview["plan_digest"].as_str().unwrap(),
        )
        .await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .execute(
            "UPDATE awr_team.work_contracts SET definition_state='enabled' WHERE work_id='b-private'",
            &[],
        )
        .await
        .unwrap();
    let authorization_store =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut existing_task = proposed.authorization.clone();
    existing_task.id = "native-authorization-existing-task-b".into();
    existing_task.scope = AuthorizationScope::Task {
        project_id: PROJECT.into(),
        work_item_id: "b-private".into(),
    };
    authorization_store
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "issue-existing-task-b".into(),
                authorization: existing_task.clone(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &proposed).await,
        Err(PgError::Forbidden)
    ));
    let mut same_task = proposed.clone();
    same_task.authorization.id = "native-authorization-same-task-b".into();
    same_task.authorization.scope = AuthorizationScope::Task {
        project_id: PROJECT.into(),
        work_item_id: "b-private".into(),
    };
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &same_task).await,
        Err(PgError::Forbidden)
    ));

    admin
        .execute(
            "UPDATE awr_team.workstream_ownership SET workstream_id=$1 WHERE work_id='b-private'",
            &[&Id::from(1).to_string()],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::authorize_preview(&mut admin, &proposed).await,
        Err(PgError::Forbidden)
    ));

    existing_task.status = AuthorizationStatus::Expired;
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET status='expired',body_json=$1 WHERE id=$2",
            &[&json!(existing_task), &existing_task.id],
        )
        .await
        .unwrap();
    OperatorAgent::authorize_preview(&mut admin, &proposed)
        .await
        .unwrap();

    existing_task.status = AuthorizationStatus::Active;
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET status='active',body_json=$1 WHERE id=$2",
            &[&json!(existing_task), &existing_task.id],
        )
        .await
        .unwrap();
    authorization_store
        .revoke(
            TENANT,
            PROJECT,
            &RevokeAuthorizationRequest {
                request_key: "revoke-existing-task-b".into(),
                authorization_id: existing_task.id.clone(),
                revoked_by: existing_task.responsible_person_id.clone(),
                revoked_at_ms: existing_task.created_at_ms + 1,
                reason: "retire task authorization".into(),
            },
        )
        .await
        .unwrap();
    OperatorAgent::authorize_preview(&mut admin, &proposed)
        .await
        .unwrap();
}

#[tokio::test]
async fn concurrent_overlapping_authorization_previews_issue_once() {
    let (_g, mut admin, db, _) = setup().await;
    stage(&mut admin).await;
    grant_second_stream(&admin).await;
    let mut initial = plan(&admin).await;
    initial.authorization.scope = AuthorizationScope::Workstream {
        project_id: PROJECT.into(),
        workstream_id: Id::from(1).to_string(),
    };
    apply(&mut admin, &initial, "agent-issue").await;
    let first_plan = authorization_issue_plan(
        &admin,
        "native-authorization-second-a",
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(2).to_string(),
        },
    )
    .await;
    let mut second_plan = first_plan.clone();
    second_plan.authorization.id = "native-authorization-second-b".into();
    let first_preview = OperatorAgent::authorize_preview(&mut admin, &first_plan)
        .await
        .unwrap();
    let second_preview = OperatorAgent::authorize_preview(&mut admin, &second_plan)
        .await
        .unwrap();
    assert_eq!(
        first_preview["state_digest"],
        second_preview["state_digest"]
    );
    let mut other = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let (first, second) = tokio::join!(
        OperatorAgent::authorize_apply(
            &mut admin,
            &first_plan,
            "authorize-race-a",
            first_preview["state_digest"].as_str().unwrap(),
            first_preview["plan_digest"].as_str().unwrap(),
        ),
        OperatorAgent::authorize_apply(
            &mut other,
            &second_plan,
            "authorize-race-b",
            second_preview["state_digest"].as_str().unwrap(),
            second_preview["plan_digest"].as_str().unwrap(),
        ),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert!(matches!(
        first.as_ref().err().or(second.as_ref().err()),
        Some(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.agent_authorizations", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        2
    );
}

#[tokio::test]
async fn expired_authorization_gets_immutable_digest_gated_successor_and_replays() {
    let (_g, mut admin, db, store) = setup().await;
    stage(&mut admin).await;
    let initial = plan(&admin).await;
    apply(&mut admin, &initial, "agent-issue").await;
    let prepared_before_expiry = prepare(&store, TOKEN, "a").await;
    let renewal = expired_renewal_plan(&admin, &initial).await;
    let previous_before: Value = admin
        .query_one(
            "SELECT body_json FROM awr_team.agent_authorizations WHERE id=$1",
            &[&renewal.previous_authorization_id],
        )
        .await
        .unwrap()
        .get(0);
    let denied = command(
        &prepared_before_expiry,
        "expired-before-renewal",
        "session.start",
        json!({"conversation_id":"expired"}),
    );
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, TOKEN, denied)
            .await,
        Err(PgError::Forbidden)
    ));
    let before = snapshot(&admin).await;
    let preview = OperatorAgent::renew_preview(&mut admin, &renewal)
        .await
        .unwrap();
    assert_eq!(snapshot(&admin).await, before);
    assert_eq!(preview["binding_reused"], true);
    assert_eq!(preview["predecessor_rewritten"], false);
    assert_eq!(preview["previous_lifetime_ms"], "3600000");
    let applied = OperatorAgent::renew_apply(
        &mut admin,
        &renewal,
        "agent-renew",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(applied["receipt"]["binding_reused"], true);
    assert_eq!(applied["receipt"]["predecessor_rewritten"], false);
    assert_eq!(applied["receipt"]["previous_lifetime_ms"], "3600000");
    let successor_plan = AgentProvisionPlan {
        protocol_version: renewal.protocol_version,
        tenant_id: renewal.tenant_id.clone(),
        project_id: renewal.project_id.clone(),
        authorization: renewal.authorization.clone(),
        member_identity: renewal.member_identity.clone(),
    };
    assert_eq!(
        OperatorAgent::inspect(&mut admin, &successor_plan)
            .await
            .unwrap()["configuration_matches_plan"],
        true
    );
    let previous_after: Value = admin
        .query_one(
            "SELECT body_json FROM awr_team.agent_authorizations WHERE id=$1",
            &[&renewal.previous_authorization_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(previous_after, previous_before);
    let counts = admin
        .query_one(
            "SELECT (SELECT count(*) FROM awr_team.persons),(SELECT count(*) FROM awr_team.person_agent_bindings),(SELECT count(*) FROM awr_team.agent_authorizations)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(counts.get::<_, i64>(0), 1);
    assert_eq!(counts.get::<_, i64>(1), 1);
    assert_eq!(counts.get::<_, i64>(2), 2);
    let started = command(
        &prepare(&store, TOKEN, "a").await,
        "start-after-renewal",
        "session.start",
        json!({"conversation_id":"renewed"}),
    );
    store
        .commands()
        .execute(TENANT, PROJECT, TOKEN, started)
        .await
        .unwrap();
    let after = snapshot(&admin).await;
    let replayed = OperatorAgent::renew_apply(
        &mut admin,
        &renewal,
        "agent-renew",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["receipt"], applied["receipt"]);
    assert_eq!(snapshot(&admin).await, after);
    assert_eq!(
        OperatorAgent::renew_outcome(&mut admin, TENANT, PROJECT, "agent-renew")
            .await
            .unwrap()["receipt"],
        applied["receipt"]
    );
    assert!(matches!(
        OperatorAgent::outcome(&mut admin, TENANT, PROJECT, "agent-renew").await,
        Err(PgError::IdempotencyConflict)
    ));

    let auth = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    auth.revoke(
        TENANT,
        PROJECT,
        &RevokeAuthorizationRequest {
            request_key: "revoke-successor".into(),
            authorization_id: renewal.authorization.id.clone(),
            revoked_by: renewal.authorization.responsible_person_id.clone(),
            revoked_at_ms: renewal.authorization.created_at_ms + 1,
            reason: "stop renewed access".into(),
        },
    )
    .await
    .unwrap();
    let now: i64 = admin
        .query_one(
            "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let mut stale_predecessor = renewal.clone();
    stale_predecessor.authorization.id = "native-authorization-stale-retry".into();
    stale_predecessor.authorization.created_at_ms = now;
    stale_predecessor.authorization.expires_at_ms = Some(now + 3_600_000);
    assert!(matches!(
        OperatorAgent::renew_preview(&mut admin, &stale_predecessor).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn expired_renewal_ignores_newer_authorizations_in_disjoint_scopes() {
    for renew_stream in [false, true] {
        for other_stream in [false, true] {
            let (_g, mut admin, _, store) = setup().await;
            stage(&mut admin).await;
            grant_second_stream(&admin).await;
            let mut initial = plan(&admin).await;
            if renew_stream {
                initial.authorization.scope = AuthorizationScope::Workstream {
                    project_id: PROJECT.into(),
                    workstream_id: Id::from(1).to_string(),
                };
            }
            apply(&mut admin, &initial, "agent-issue").await;
            let renewal = expired_renewal_plan(&admin, &initial).await;
            let other_scope = if other_stream {
                AuthorizationScope::Workstream {
                    project_id: PROJECT.into(),
                    workstream_id: Id::from(2).to_string(),
                }
            } else {
                AuthorizationScope::Task {
                    project_id: PROJECT.into(),
                    work_item_id: "b-private".into(),
                }
            };
            let other = authorization_issue_plan(&admin, "other-scope", other_scope).await;
            let other_preview = OperatorAgent::authorize_preview(&mut admin, &other)
                .await
                .unwrap();
            OperatorAgent::authorize_apply(
                &mut admin,
                &other,
                "authorize-other-scope",
                other_preview["state_digest"].as_str().unwrap(),
                other_preview["plan_digest"].as_str().unwrap(),
            )
            .await
            .unwrap();
            let preserved = preserved_authority(&admin).await;
            let before = snapshot(&admin).await;
            let preview = OperatorAgent::renew_preview(&mut admin, &renewal)
                .await
                .unwrap();
            assert_eq!(snapshot(&admin).await, before);
            assert_eq!(preview["previous_lifetime_ms"], "3600000");
            let applied = OperatorAgent::renew_apply(
                &mut admin,
                &renewal,
                "renew-independent-scope",
                preview["state_digest"].as_str().unwrap(),
                preview["plan_digest"].as_str().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(preserved_authority(&admin).await, preserved);
            let other_after: Value = admin
                .query_one(
                    "SELECT body_json FROM awr_team.agent_authorizations WHERE id=$1",
                    &[&other.authorization.id],
                )
                .await
                .unwrap()
                .get(0);
            assert_eq!(other_after, json!(other.authorization));
            assert_eq!(prepare(&store, TOKEN, "a").await["data"]["work_id"], "a");
            assert_eq!(
                prepare(&store, TOKEN, "b-private").await["data"]["work_id"],
                "b-private"
            );
            let after = snapshot(&admin).await;
            let replay = OperatorAgent::renew_apply(
                &mut admin,
                &renewal,
                "renew-independent-scope",
                preview["state_digest"].as_str().unwrap(),
                preview["plan_digest"].as_str().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(replay["replayed"], true);
            assert_eq!(replay["receipt"], applied["receipt"]);
            assert_eq!(snapshot(&admin).await, after);
        }
    }
}

#[tokio::test]
async fn renewal_rejects_newer_overlapping_authorization_even_after_revocation() {
    for overlap_stream in [false, true] {
        let (_g, mut admin, db, _) = setup().await;
        stage(&mut admin).await;
        let initial = plan(&admin).await;
        apply(&mut admin, &initial, "agent-issue").await;
        let renewal = expired_renewal_plan(&admin, &initial).await;
        let scope = if overlap_stream {
            AuthorizationScope::Workstream {
                project_id: PROJECT.into(),
                workstream_id: Id::from(1).to_string(),
            }
        } else {
            initial.authorization.scope.clone()
        };
        let newer = authorization_issue_plan(&admin, "newer-overlapping", scope).await;
        let preview = OperatorAgent::authorize_preview(&mut admin, &newer)
            .await
            .unwrap();
        OperatorAgent::authorize_apply(
            &mut admin,
            &newer,
            "authorize-newer-overlapping",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
        let before = snapshot(&admin).await;
        assert!(matches!(
            OperatorAgent::renew_preview(&mut admin, &renewal).await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, before);
        let auth =
            AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
        auth.revoke(
            TENANT,
            PROJECT,
            &RevokeAuthorizationRequest {
                request_key: "revoke-newer-overlapping".into(),
                authorization_id: newer.authorization.id,
                revoked_by: newer.authorization.responsible_person_id,
                revoked_at_ms: newer.authorization.created_at_ms + 1,
                reason: "Stop overlapping authority".into(),
            },
        )
        .await
        .unwrap();
        let revoked = snapshot(&admin).await;
        assert!(matches!(
            OperatorAgent::renew_preview(&mut admin, &renewal).await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, revoked);
    }
}

#[tokio::test]
async fn renewal_rejects_revoked_or_changed_authority_and_stale_preview() {
    let (_g, mut admin, db, _) = setup().await;
    stage(&mut admin).await;
    let initial = plan(&admin).await;
    apply(&mut admin, &initial, "agent-issue").await;
    let renewal = expired_renewal_plan(&admin, &initial).await;

    for changed in ["client", "scope", "actions", "binding", "lifetime"] {
        let mut drifted = renewal.clone();
        match changed {
            "client" => drifted.authorization.client_id = "other-client".into(),
            "scope" => {
                drifted.authorization.scope = awr_core::AuthorizationScope::Task {
                    project_id: PROJECT.into(),
                    work_item_id: "b".into(),
                };
            }
            "actions" => {
                drifted
                    .authorization
                    .actions
                    .remove(&AuthorizedAction::ClaimCoordination);
            }
            "binding" => drifted.authorization.binding_id = Some("other-binding".into()),
            "lifetime" => {
                *drifted.authorization.expires_at_ms.as_mut().unwrap() += 1;
            }
            _ => unreachable!(),
        }
        assert!(
            OperatorAgent::renew_preview(&mut admin, &drifted)
                .await
                .is_err(),
            "{changed}"
        );
    }

    let inactive_previous_json: Value = admin
        .query_one(
            "SELECT body_json FROM awr_team.agent_authorizations WHERE id=$1",
            &[&renewal.previous_authorization_id],
        )
        .await
        .unwrap()
        .get(0);
    let mut inactive_previous: AgentAuthorization =
        serde_json::from_value(inactive_previous_json).unwrap();
    inactive_previous.status = AuthorizationStatus::Expired;
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET status='expired',body_json=$1 WHERE id=$2",
            &[
                &json!(inactive_previous),
                &renewal.previous_authorization_id,
            ],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::renew_preview(&mut admin, &renewal).await,
        Err(PgError::Forbidden)
    ));
    inactive_previous.status = AuthorizationStatus::Active;
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET status='active',body_json=$1 WHERE id=$2",
            &[
                &json!(inactive_previous),
                &renewal.previous_authorization_id,
            ],
        )
        .await
        .unwrap();

    admin
        .execute(
            "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='native-binding'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::renew_preview(&mut admin, &renewal).await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='native-binding'",
            &[],
        )
        .await
        .unwrap();

    let preview = OperatorAgent::renew_preview(&mut admin, &renewal)
        .await
        .unwrap();
    let mut app = common::app_client(&db).await;
    assert!(matches!(
        OperatorAgent::renew_preview(&mut app, &renewal).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorAgent::renew_apply(
            &mut app,
            &renewal,
            "app-renewal",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorAgent::renew_outcome(&mut app, TENANT, PROJECT, "app-renewal").await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE actor_id='native-agent'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorAgent::renew_apply(
            &mut admin,
            &renewal,
            "stale-renewal",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE actor_id='native-agent'",
            &[],
        )
        .await
        .unwrap();
    let auth = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    auth.revoke(
        TENANT,
        PROJECT,
        &RevokeAuthorizationRequest {
            request_key: "revoke-expired".into(),
            authorization_id: renewal.previous_authorization_id.clone(),
            revoked_by: renewal.authorization.responsible_person_id.clone(),
            revoked_at_ms: renewal.authorization.created_at_ms,
            reason: "do not renew".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        OperatorAgent::renew_preview(&mut admin, &renewal).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn concurrent_renewal_previews_have_one_successor() {
    let (_g, mut admin, db, _) = setup().await;
    stage(&mut admin).await;
    let initial = plan(&admin).await;
    apply(&mut admin, &initial, "agent-issue").await;
    let first_plan = expired_renewal_plan(&admin, &initial).await;
    let mut second_plan = first_plan.clone();
    second_plan.authorization.id = "native-authorization-renewed-b".into();
    let first_preview = OperatorAgent::renew_preview(&mut admin, &first_plan)
        .await
        .unwrap();
    let second_preview = OperatorAgent::renew_preview(&mut admin, &second_plan)
        .await
        .unwrap();
    assert_eq!(
        first_preview["state_digest"],
        second_preview["state_digest"]
    );
    let mut other = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let (first, second) = tokio::join!(
        OperatorAgent::renew_apply(
            &mut admin,
            &first_plan,
            "renew-race-a",
            first_preview["state_digest"].as_str().unwrap(),
            first_preview["plan_digest"].as_str().unwrap(),
        ),
        OperatorAgent::renew_apply(
            &mut other,
            &second_plan,
            "renew-race-b",
            second_preview["state_digest"].as_str().unwrap(),
            second_preview["plan_digest"].as_str().unwrap(),
        ),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert!(matches!(
        first.as_ref().err().or(second.as_ref().err()),
        Some(PgError::PreconditionsChanged)
    ));
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.agent_authorizations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 2);
}

#[tokio::test]
async fn staged_identity_cannot_work_until_atomic_provision_and_revocation_denies_next_command() {
    let (_g, mut admin, db, store) = setup().await;
    stage(&mut admin).await;
    let p = plan(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let c = command(
        &prepared,
        "start-before",
        "session.start",
        json!({"conversation_id":"native"}),
    );
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, TOKEN, c).await,
        Err(PgError::Forbidden)
    ));
    let before = snapshot(&admin).await;
    let preview = OperatorAgent::preview(&mut admin, &p).await.unwrap();
    assert_eq!(snapshot(&admin).await, before);
    assert_eq!(
        preview,
        OperatorAgent::preview(&mut admin, &p).await.unwrap()
    );
    assert!(!preview.to_string().contains(TOKEN));
    assert!(!preview.to_string().contains("secret_hash"));
    let receipt = OperatorAgent::apply(
        &mut admin,
        &p,
        "agent-issue",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(receipt["receipt"]["human_approval"], false);
    assert_eq!(receipt["receipt"]["team_independent_acceptance"], false);
    assert_eq!(
        OperatorAgent::inspect(&mut admin, &p).await.unwrap()["configuration_matches_plan"],
        true
    );
    let current_inspect = OperatorAgent::inspect(&mut admin, &p).await.unwrap();
    assert_eq!(
        current_inspect["state_digest"],
        receipt["receipt"]["after_digest"]
    );
    let event = admin
        .query_one(
            "SELECT payload_json FROM awr_team.events WHERE id=$1",
            &[&receipt["receipt"]["event_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(
        event.get::<_, Value>(0)["authorization_id"],
        p.authorization.id
    );
    let current = prepare(&store, TOKEN, "a").await;
    let c = command(
        &current,
        "start-after",
        "session.start",
        json!({"conversation_id":"native"}),
    );
    let started = store
        .commands()
        .execute(TENANT, PROJECT, TOKEN, c)
        .await
        .unwrap();
    let id = started["receipt"]["data"]["session_id"].as_str().unwrap();
    let actual = admin
        .query_one(
            "SELECT actor_id,client_id FROM awr_team.sessions WHERE id=$1",
            &[&id],
        )
        .await
        .unwrap();
    assert_eq!(actual.get::<_, String>(0), "native-agent");
    assert_eq!(actual.get::<_, String>(1), "native-client");
    let elsewhere = command(
        &prepare(&store, A, "c").await,
        "out-of-task",
        "session.start",
        json!({"conversation_id":"denied"}),
    );
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, TOKEN, elsewhere)
            .await,
        Err(PgError::Forbidden)
    ));
    let auth = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    auth.revoke(
        TENANT,
        PROJECT,
        &RevokeAuthorizationRequest {
            request_key: "revoke-native".into(),
            authorization_id: p.authorization.id.clone(),
            revoked_by: p.authorization.responsible_person_id.clone(),
            revoked_at_ms: p.authorization.created_at_ms + 1,
            reason: "finished".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        OperatorAgent::inspect(&mut admin, &p).await.unwrap()["configuration_matches_plan"],
        false
    );
    let c = command(
        &current,
        "after-revoke",
        "session.start",
        json!({"conversation_id":"denied"}),
    );
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, TOKEN, c).await,
        Err(PgError::Forbidden)
    ));
    let after = snapshot(&admin).await;
    let replay = OperatorAgent::apply(
        &mut admin,
        &p,
        "agent-issue",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(replay["receipt"], receipt["receipt"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(snapshot(&admin).await, after);
    assert_eq!(
        OperatorAgent::outcome(&mut admin, TENANT, PROJECT, "agent-issue")
            .await
            .unwrap()["receipt"],
        receipt["receipt"]
    );
    let row=admin.query_one("SELECT person_id,actor_id,client_id,summary_json FROM awr_team.ops_audit_records WHERE request_id='agent-issue'",&[]).await.unwrap();
    assert!(row.get::<_, Option<String>>(0).is_none());
    assert_eq!(
        row.get::<_, String>(1),
        receipt["receipt"]["operator_role"].as_str().unwrap()
    );
    assert_eq!(row.get::<_, String>(2), "awr-server-owner-cli");
    assert_eq!(row.get::<_, Value>(3)["subject_actor_id"], "native-agent");
    let kind: String = admin
        .query_one(
            "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id='agent'",
            &[&TENANT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(kind, "human");
}

#[tokio::test]
async fn owner_only_and_request_namespaces_are_checked() {
    let (_g, mut admin, db, _) = setup().await;
    stage(&mut admin).await;
    let p = plan(&admin).await;
    let v = OperatorAgent::preview(&mut admin, &p).await.unwrap();
    let d = v["state_digest"].as_str().unwrap();
    let h = v["plan_digest"].as_str().unwrap();
    let mut app = common::app_client(&db).await;
    assert!(matches!(
        OperatorAgent::preview(&mut app, &p).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorAgent::inspect(&mut app, &p).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorAgent::apply(&mut app, &p, "issue", d, h).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorAgent::outcome(&mut app, TENANT, PROJECT, "issue").await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorAgent::outcome(&mut admin, TENANT, PROJECT, "access-stage").await,
        Err(PgError::IdempotencyConflict)
    ));
    assert!(matches!(
        OperatorAgent::apply(&mut admin, &p, "access-stage", d, h).await,
        Err(PgError::IdempotencyConflict)
    ));
    OperatorAgent::apply(&mut admin, &p, "issue", d, h)
        .await
        .unwrap();
    let mut edited = p.clone();
    edited
        .authorization
        .actions
        .remove(&AuthorizedAction::ClaimCoordination);
    assert!(matches!(
        OperatorAgent::apply(&mut admin, &edited, "issue", d, h).await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(
        OperatorAgent::outcome(&mut admin, TENANT, PROJECT, "missing")
            .await
            .unwrap()["outcome"],
        "unknown"
    );
}

#[tokio::test]
async fn changed_access_and_scope_facts_invalidate_preview() {
    let (_g, mut admin, _, _) = setup().await;
    stage(&mut admin).await;
    let p = plan(&admin).await;
    for (change, undo) in [
        (
            "UPDATE awr_team.project_memberships SET independent_review=true WHERE actor_id='native-agent'",
            "UPDATE awr_team.project_memberships SET independent_review=false WHERE actor_id='native-agent'",
        ),
        (
            "UPDATE awr_team.actors SET kind='human' WHERE id='native-agent'",
            "UPDATE awr_team.actors SET kind='agent' WHERE id='native-agent'",
        ),
        (
            "UPDATE awr_team.actors SET status='disabled' WHERE id='native-agent'",
            "UPDATE awr_team.actors SET status='active' WHERE id='native-agent'",
        ),
        (
            "UPDATE awr_team.actors SET status='disabled' WHERE id='agent'",
            "UPDATE awr_team.actors SET status='active' WHERE id='agent'",
        ),
        (
            "UPDATE awr_team.project_memberships SET role='reader' WHERE actor_id='native-agent'",
            "UPDATE awr_team.project_memberships SET role='developer' WHERE actor_id='native-agent'",
        ),
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='native-agent'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='native-agent'",
        ),
        (
            "UPDATE awr_team.credentials SET expires_at=clock_timestamp()-interval '1 day' WHERE id='native-agent'",
            "UPDATE awr_team.credentials SET expires_at=NULL WHERE id='native-agent'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE actor_id='native-agent'",
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE actor_id='native-agent'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET authority_version=2 WHERE actor_id='native-agent'",
            "UPDATE awr_team.workstream_grants SET authority_version=1 WHERE actor_id='native-agent'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE actor_id='native-agent'",
            "UPDATE awr_team.workstream_grants SET can_manage=false WHERE actor_id='native-agent'",
        ),
        (
            "UPDATE awr_team.workstream_ownership SET ownership_version=2 WHERE work_id='a'",
            "UPDATE awr_team.workstream_ownership SET ownership_version=1 WHERE work_id='a'",
        ),
        (
            "UPDATE awr_team.work_contracts SET definition_state='archived' WHERE work_id='a'",
            "UPDATE awr_team.work_contracts SET definition_state='enabled' WHERE work_id='a'",
        ),
    ] {
        let v = OperatorAgent::preview(&mut admin, &p).await.unwrap();
        admin.batch_execute(change).await.unwrap();
        assert!(
            matches!(
                OperatorAgent::apply(
                    &mut admin,
                    &p,
                    "stale",
                    v["state_digest"].as_str().unwrap(),
                    v["plan_digest"].as_str().unwrap()
                )
                .await,
                Err(PgError::PreconditionsChanged)
            ),
            "{change}"
        );
        assert!(
            OperatorAgent::preview(&mut admin, &p).await.is_err(),
            "{change}"
        );
        assert_eq!(
            OperatorAgent::inspect(&mut admin, &p).await.unwrap()["configuration_matches_plan"],
            false
        );
        admin.batch_execute(undo).await.unwrap();
    }
}

#[tokio::test]
async fn malformed_or_overbroad_initial_plans_and_existing_bindings_are_refused() {
    let (_g, mut admin, _, _) = setup().await;
    stage(&mut admin).await;
    let p = plan(&admin).await;
    for (field, value) in [
        ("actions", json!(["accept_responsibility"])),
        ("actions", json!(["occupy_collaboratively"])),
        ("subject_kind", json!("person")),
        ("client_id", json!("other-client")),
        ("authorizer_person_id", json!("reviewer")),
        ("scope", json!({"kind":"project","project_id":PROJECT})),
        ("session_id", json!("one-session")),
        ("parent_authorization_id", json!("parent")),
        ("actions", json!(["inspect", "manage_authorization"])),
        (
            "created_at_ms",
            json!(p.authorization.created_at_ms + 3600000),
        ),
        ("created_at_ms", json!(0)),
        ("expires_at_ms", json!(p.authorization.created_at_ms - 1)),
    ] {
        let mut value_plan = json!(p);
        value_plan["authorization"][field] = value;
        let edited: AgentProvisionPlan = serde_json::from_value(value_plan).unwrap();
        assert!(
            OperatorAgent::preview(&mut admin, &edited).await.is_err(),
            "{field}"
        );
    }
    let mut unknown = json!(p);
    unknown["secret"] = json!("not-accepted");
    assert!(serde_json::from_value::<AgentProvisionPlan>(unknown).is_err());
    admin.batch_execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES('reader-tenant','reader-project','agent','Different name','active')").await.unwrap();
    assert!(OperatorAgent::preview(&mut admin, &p).await.is_err());
    admin.batch_execute("UPDATE awr_team.persons SET display_name='Worker'; INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','native-binding','agent','native-agent','active')").await.unwrap();
    assert!(OperatorAgent::preview(&mut admin, &p).await.is_err());
    let before = snapshot(&admin).await;
    let inspected = OperatorAgent::inspect(&mut admin, &p).await.unwrap();
    assert_eq!(inspected["configuration_matches_plan"], false);
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn each_late_write_failure_rolls_back_identity_delegation_receipts_and_revision() {
    let (_g, mut admin, _, _) = setup().await;
    stage(&mut admin).await;
    let p = plan(&admin).await;
    let v = OperatorAgent::preview(&mut admin, &p).await.unwrap();
    let before = snapshot(&admin).await;
    admin.batch_execute("CREATE FUNCTION awr_team.fail_agent_provision() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic provisioning failure'; END $$").await.unwrap();
    for table in [
        "persons",
        "person_agent_bindings",
        "agent_authorizations",
        "agent_authorization_receipts",
        "access_changes",
        "events",
        "ops_audit_records",
    ] {
        admin.batch_execute(&format!("CREATE TRIGGER fail_agent_write BEFORE INSERT ON awr_team.{table} FOR EACH ROW EXECUTE FUNCTION awr_team.fail_agent_provision()" )).await.unwrap();
        assert!(
            OperatorAgent::apply(
                &mut admin,
                &p,
                "fault",
                v["state_digest"].as_str().unwrap(),
                v["plan_digest"].as_str().unwrap()
            )
            .await
            .is_err(),
            "{table}"
        );
        assert_eq!(snapshot(&admin).await, before, "{table}");
        admin
            .batch_execute(&format!(
                "DROP TRIGGER fail_agent_write ON awr_team.{table}"
            ))
            .await
            .unwrap();
    }
    apply(&mut admin, &p, "after-fault").await;
}

#[tokio::test]
async fn competing_provisions_have_one_winner_and_audited_identity() {
    let (_g, mut admin, db, _) = setup().await;
    stage(&mut admin).await;
    let p = plan(&admin).await;
    let v = OperatorAgent::preview(&mut admin, &p).await.unwrap();
    let mut other = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    let (a, b) = tokio::join!(
        OperatorAgent::apply(
            &mut admin,
            &p,
            "race-a",
            v["state_digest"].as_str().unwrap(),
            v["plan_digest"].as_str().unwrap()
        ),
        OperatorAgent::apply(
            &mut other,
            &p,
            "race-b",
            v["state_digest"].as_str().unwrap(),
            v["plan_digest"].as_str().unwrap()
        )
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        OperatorAgent::inspect(&mut admin, &p).await.unwrap()["configuration_matches_plan"],
        true
    );
    let n: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.agent_authorizations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(n, 1);
}

#[tokio::test]
async fn agent_review_requires_both_explicit_grant_and_action() {
    let (_g, mut admin, _, _) = setup().await;
    stage(&mut admin).await;
    let mut p = plan(&admin).await;
    p.authorization
        .actions
        .remove(&AuthorizedAction::ClaimCoordination);
    p.authorization.actions.insert(AuthorizedAction::Review);
    assert!(OperatorAgent::preview(&mut admin, &p).await.is_err());
    let mut access = access_plan();
    access.credential = None;
    access.agent_review = true;
    let v = OperatorAccess::preview(&mut admin, &access).await.unwrap();
    OperatorAccess::apply(
        &mut admin,
        &access,
        "allow-agent-review",
        v["state_digest"].as_str().unwrap(),
        v["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    let result = apply(&mut admin, &p, "reviewer").await;
    assert_eq!(result["receipt"]["human_approval"], false);
    assert_eq!(
        OperatorAgent::inspect(&mut admin, &p).await.unwrap()["configuration_matches_plan"],
        true
    );
    admin.batch_execute("UPDATE awr_team.project_memberships SET agent_review=false WHERE actor_id='native-agent'").await.unwrap();
    assert_eq!(
        OperatorAgent::inspect(&mut admin, &p).await.unwrap()["mismatch_reason"],
        "action_not_effective"
    );
}
