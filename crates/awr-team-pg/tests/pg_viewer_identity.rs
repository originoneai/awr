#![cfg(feature = "pg-tests")]
//! AWR-PPD-057: capabilities and work.next disclose only the authenticated
//! viewer's unambiguous, active, recorded person relationship.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
};
use awr_team_pg::{AuthorizationStore, PgError, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

async fn identity(store: &WorkstreamReadStore) -> Value {
    let capabilities = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    let capabilities_identity = capabilities["identity"].clone();
    assert_eq!(
        capabilities_identity, next["data"]["identity"],
        "capabilities and work.next must share one identity"
    );
    capabilities_identity
}

fn assert_person(identity: &Value, person_id: &str, display_name: &str) {
    assert_eq!(identity["person_id"], json!(person_id));
    assert_eq!(identity["person_display_name"], json!(display_name));
}

fn assert_unknown(identity: &Value) {
    assert_eq!(identity["person_id"], Value::Null, "{identity}");
    assert_eq!(identity["person_display_name"], Value::Null, "{identity}");
}

async fn agent_actor(admin: &tokio_postgres::Client) {
    admin
        .execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id=$1 AND id='agent'",
            &[&TENANT],
        )
        .await
        .unwrap();
}

async fn grant_agent_read(db: &str) {
    let authorization = AgentAuthorization {
        id: "viewer-project-read".into(),
        authorizer_person_id: PersonId::new("person-one").unwrap(),
        responsible_person_id: PersonId::new("person-one").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        actions: BTreeSet::from([AuthorizedAction::Inspect]),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1000,
        binding_id: Some("binding-one".into()),
    };
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "viewer-project-read".into(),
                authorization,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn agent_binding_and_human_directory_row_resolve_to_the_same_person_in_both_reads() {
    let (_guard, admin, db, store) = setup().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
             ('reader-tenant','reader-project','agent','Human Viewer','active'),
             ('reader-tenant','reader-project','person-one','Agent Owner','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('reader-tenant','reader-project','binding-one','person-one','agent','active')",
        )
        .await
        .unwrap();

    // A human uses the existing person-id rule. A recorded binding takes
    // precedence for either actor kind, exactly as in review attribution.
    assert_person(&identity(&store).await, "person-one", "Agent Owner");
    admin
        .execute(
            "DELETE FROM awr_team.person_agent_bindings WHERE tenant_id=$1 AND project_id=$2 AND agent_id='agent'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    assert_person(&identity(&store).await, "agent", "Human Viewer");

    agent_actor(&admin).await;
    admin
        .execute(
            "INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES($1,$2,'binding-one','person-one','agent','active')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    grant_agent_read(&db).await;
    assert_person(&identity(&store).await, "person-one", "Agent Owner");
}

#[tokio::test]
async fn conflicting_disabled_and_inactive_human_relationships_stay_unknown() {
    let (_guard, admin, _, store) = setup().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
             ('reader-tenant','reader-project','person-one','First Owner','active'),
             ('reader-tenant','reader-project','person-two','Second Owner','active')",
        )
        .await
        .unwrap();

    // A disabled binding does not become a human fallback or leak another person.
    admin
        .execute(
            "INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES($1,$2,'binding-disabled','person-one','agent','disabled')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    assert_unknown(&identity(&store).await);

    admin
        .execute(
            "UPDATE awr_team.person_agent_bindings SET status='active'
             WHERE tenant_id=$1 AND project_id=$2 AND id='binding-disabled'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    assert_person(&identity(&store).await, "person-one", "First Owner");

    admin
        .execute(
            "INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES($1,$2,'binding-conflict','person-two','agent','active')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    assert_unknown(&identity(&store).await);

    admin
        .batch_execute(
            "DELETE FROM awr_team.person_agent_bindings WHERE id='binding-conflict';
             UPDATE awr_team.persons SET status='departed' WHERE id='person-one'",
        )
        .await
        .unwrap();
    assert_unknown(&identity(&store).await);
}

#[tokio::test]
async fn missing_human_people_stay_unknown_without_creating_directory_rows() {
    let (_guard, admin, _, store) = setup().await;
    let persons_before: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.persons", &[])
        .await
        .unwrap()
        .get(0);

    // The read-only counterpart of the human resolver does not create a person.
    assert_unknown(&identity(&store).await);
    let persons_after: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.persons", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(persons_after, persons_before);
}

#[tokio::test]
async fn inactive_human_person_stays_unknown_in_both_reads() {
    let (_guard, admin, _, store) = setup().await;
    admin
        .execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES($1,$2,'agent','Departed Viewer','departed')",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    assert_unknown(&identity(&store).await);
}

#[tokio::test]
async fn bindings_from_another_project_are_not_mixed_and_existing_authorization_stays_closed() {
    let (_guard, admin, db, store) = setup().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
             VALUES('reader-tenant','other-project','other','team','epoch-other','active');
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
             VALUES('reader-tenant','other-project','other-person','Other Project Owner','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES('reader-tenant','other-project','other-binding','other-person','agent','active')",
        )
        .await
        .unwrap();

    // The actor id is tenant-wide, but its person relationship is project-local.
    assert_unknown(&identity(&store).await);
    // Also pin the explicit project predicates independently of RLS: the owner
    // connection bypasses row policies, so dropping either predicate is visible.
    let owner_store =
        WorkstreamReadStore::from_config(common::with_db(&common::test_config(), &db));
    assert_unknown(&identity(&owner_store).await);
    for (tenant, project, token) in [
        (TENANT, "other-project", A),
        ("other-tenant", PROJECT, A),
        (TENANT, PROJECT, NONE),
    ] {
        assert!(matches!(
            store
                .query(tenant, project, token, query("capabilities"))
                .await,
            Err(PgError::Forbidden)
        ));
    }
}
