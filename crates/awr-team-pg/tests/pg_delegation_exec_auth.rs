//! AWR-TMCP-030: agent delegation ∩ TMCP product permissions on real PG paths.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_core::*;
use awr_team_pg::{
    AuthorizationStore, PgError, PlanningSuggestRequest, SourceStore, SuggestionSubmit,
};
use fixture::*;
use serde_json::json;
use std::collections::BTreeSet;

async fn flip_actor_to_agent(admin: &tokio_postgres::Client) {
    admin
        .batch_execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
               VALUES('reader-tenant','reader-project','alice','Alice','active')
               ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
               VALUES('reader-tenant','reader-project','bind-agent','alice','agent','active')
               ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
}

fn work_grant(person: &PersonId) -> AgentAuthorization {
    AgentAuthorization {
        id: "auth-agent-work".into(),
        authorizer_person_id: person.clone(),
        responsible_person_id: person.clone(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        actions: BTreeSet::from([
            AuthorizedAction::StartWork,
            AuthorizedAction::ClaimCoordination,
            AuthorizedAction::Inspect,
        ]),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1_000,
        binding_id: Some("bind-agent".into()),
    }
}

#[tokio::test]
async fn admin_membership_agent_without_delegation_is_forbidden() {
    let (_guard, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    flip_actor_to_agent(&admin).await;
    let commands = store.commands();
    let err = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-no-deleg",
                "session.start",
                json!({"conversation_id": "c-denied"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
}

#[tokio::test]
async fn explicit_start_work_delegation_allows_session() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    flip_actor_to_agent(&admin).await;
    let authz = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let alice = PersonId::new("alice").unwrap();
    authz
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-agent-work".into(),
                authorization: work_grant(&alice),
            },
        )
        .await
        .unwrap();

    let commands = store.commands();
    let started = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-with-deleg",
                "session.start",
                json!({"conversation_id": "c-allowed"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(started["replayed"], false);
}

#[tokio::test]
async fn disabled_binding_or_person_blocks_delegated_session() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    flip_actor_to_agent(&admin).await;
    let authz = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let alice = PersonId::new("alice").unwrap();
    authz
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-agent-disabled-bind".into(),
                authorization: work_grant(&alice),
            },
        )
        .await
        .unwrap();

    // Disable the binding while authorization JSON remains active.
    admin
        .batch_execute(
            "UPDATE awr_team.person_agent_bindings SET status='disabled'
             WHERE id='bind-agent';",
        )
        .await
        .unwrap();
    let commands = store.commands();
    let err = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-disabled-bind",
                "session.start",
                json!({"conversation_id": "c-disabled-bind"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");

    // Re-enable binding but disable the person.
    admin
        .batch_execute(
            "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='bind-agent';
             UPDATE awr_team.persons SET status='disabled' WHERE id='alice';",
        )
        .await
        .unwrap();
    let err = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-disabled-person",
                "session.start",
                json!({"conversation_id": "c-disabled-person"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");

    // Valid binding+person still works (control).
    admin
        .batch_execute("UPDATE awr_team.persons SET status='active' WHERE id='alice';")
        .await
        .unwrap();
    let started = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-live-ok",
                "session.start",
                json!({"conversation_id": "c-live-ok"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(started["replayed"], false);
}

fn suggestion(request: &str, work: &[&str]) -> PlanningSuggestRequest {
    PlanningSuggestRequest {
        protocol_version: 1,
        request_id: request.into(),
        rationale: "A missing prerequisite should be considered by the planner.".into(),
        affected_work_keys: work.iter().map(|s| (*s).into()).collect(),
        proposed_notes: json!({"reason": "interface dependency"}),
        author_person_id: None,
    }
}

fn planning_grant(id: &str, scope: AuthorizationScope) -> AgentAuthorization {
    let mut grant = work_grant(&PersonId::new("alice").unwrap());
    grant.id = id.into();
    grant.scope = scope;
    grant.actions = BTreeSet::from([AuthorizedAction::ProposePlanning]);
    grant
}

async fn issue_planning_grant(db: &str, grant: AgentAuthorization) {
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: format!("issue-{}", grant.id),
                authorization: grant,
            },
        )
        .await
        .unwrap();
}

async fn deny_both_suggestion_entries(store: &SourceStore, request: &PlanningSuggestRequest) {
    assert!(matches!(
        store.planning_suggest(TENANT, PROJECT, A, request).await,
        Err(PgError::Forbidden)
    ));
    let submit = SuggestionSubmit {
        rationale: request.rationale.clone(),
        affected_work_keys: request.affected_work_keys.clone(),
        proposed_notes: request.proposed_notes.clone(),
        author_person_id: None,
        predetermined_suggestion_id: None,
    };
    assert!(matches!(
        store
            .submit_planning_suggestion(TENANT, PROJECT, A, &submit)
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn delegated_suggestion_covers_all_tasks_is_inert_and_replays_once() {
    let (_guard, admin, db, _) = setup().await;
    enable_writes(&admin).await;
    flip_actor_to_agent(&admin).await;
    issue_planning_grant(
        &db,
        planning_grant(
            "planning-stream",
            AuthorizationScope::Workstream {
                project_id: PROJECT.into(),
                workstream_id: Id::from(1).to_string(),
            },
        ),
    )
    .await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let before: String = admin
        .query_one(
            "SELECT active_snapshot_id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let req = suggestion("planning-bounded-success", &["a", "c", "a"]);
    let result = store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    for flag in [
        "claimable",
        "adds_formal_work",
        "mutates_live_deps",
        "mutates_live_acceptance",
    ] {
        assert_eq!(result["result"][flag], false, "{flag}");
    }
    let replay = store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(replay["already_recorded"], true);
    assert_eq!(
        result["result"]["suggestion_id"],
        replay["result"]["suggestion_id"]
    );
    let state = admin.query_one(
        "SELECT active_snapshot_id,
            (SELECT count(*) FROM awr_team.planning_suggestions WHERE tenant_id=$1 AND project_id=$2),
            (SELECT count(*) FROM awr_team.planning_candidates WHERE tenant_id=$1 AND project_id=$2)
         FROM awr_team.projects WHERE tenant_id=$1 AND id=$2", &[&TENANT, &PROJECT]
    ).await.unwrap();
    assert_eq!(state.get::<_, String>(0), before);
    assert_eq!(state.get::<_, i64>(1), 1);
    assert_eq!(state.get::<_, i64>(2), 0);
}

#[tokio::test]
async fn start_work_never_implies_planning_and_denials_leave_no_reservation() {
    let (_guard, admin, db, _) = setup().await;
    enable_writes(&admin).await;
    flip_actor_to_agent(&admin).await;
    issue_planning_grant(&db, work_grant(&PersonId::new("alice").unwrap())).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    deny_both_suggestion_entries(&store, &suggestion("planning-not-delegated", &["a"])).await;
    for table in ["planning_command_receipts", "planning_suggestions"] {
        let count: i64 = admin
            .query_one(&format!("SELECT count(*) FROM awr_team.{table}"), &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 0, "denial must not leave {table}");
    }
}

#[tokio::test]
async fn task_suggestions_refuse_empty_unknown_siblings_and_synthetic_grant_union() {
    let (_guard, admin, db, _) = setup().await;
    enable_writes(&admin).await;
    flip_actor_to_agent(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    for work in ["a", "c"] {
        issue_planning_grant(
            &db,
            planning_grant(
                &format!("planning-task-{work}"),
                AuthorizationScope::Task {
                    project_id: PROJECT.into(),
                    work_item_id: work.into(),
                },
            ),
        )
        .await;
        store
            .planning_suggest(
                TENANT,
                PROJECT,
                A,
                &suggestion(&format!("one-{work}"), &[work]),
            )
            .await
            .unwrap();
    }
    for (id, work) in [
        ("empty", vec![]),
        ("unknown", vec!["unknown"]),
        ("other-stream", vec!["b-private"]),
        ("union", vec!["a", "c"]),
    ] {
        deny_both_suggestion_entries(&store, &suggestion(id, &work)).await;
    }
}

#[tokio::test]
async fn planning_suggestions_recheck_membership_grants_and_binding_on_replay() {
    let (_guard, admin, db, _) = setup().await;
    enable_writes(&admin).await;
    flip_actor_to_agent(&admin).await;
    issue_planning_grant(
        &db,
        planning_grant(
            "planning-live",
            AuthorizationScope::Project {
                project_id: PROJECT.into(),
            },
        ),
    )
    .await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let req = suggestion("planning-live-replay", &["a"]);
    store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    for (disable, restore) in [
        (
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE client_id='cli-a'",
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE client_id='cli-a'",
        ),
        (
            "UPDATE awr_team.project_memberships SET role='reader' WHERE actor_id='agent'",
            "UPDATE awr_team.project_memberships SET role='admin' WHERE actor_id='agent'",
        ),
        (
            "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='bind-agent'",
            "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='bind-agent'",
        ),
        (
            "UPDATE awr_team.persons SET status='disabled' WHERE id='alice'",
            "UPDATE awr_team.persons SET status='active' WHERE id='alice'",
        ),
    ] {
        admin.batch_execute(disable).await.unwrap();
        deny_both_suggestion_entries(&store, &req).await;
        admin.batch_execute(restore).await.unwrap();
        assert_eq!(
            store
                .planning_suggest(TENANT, PROJECT, A, &req)
                .await
                .unwrap()["already_recorded"],
            true
        );
    }
    assert!(
        matches!(
            store.planning_suggest(TENANT, PROJECT, B, &req).await,
            Err(PgError::Forbidden)
        ),
        "another client cannot reuse delegation"
    );
    // A project delegation still cannot exceed the client's workstream grants.
    deny_both_suggestion_entries(&store, &suggestion("grant-ceiling", &["a", "b-private"])).await;
}

#[tokio::test]
async fn planning_suggestions_refuse_expired_revoked_and_removed_child_action() {
    let (_guard, admin, db, _) = setup().await;
    enable_writes(&admin).await;
    flip_actor_to_agent(&admin).await;
    let mut parent = planning_grant(
        "planning-parent",
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
    );
    parent.actions.insert(AuthorizedAction::StartWork);
    issue_planning_grant(&db, parent.clone()).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let req = suggestion("planning-revocable", &["a"]);
    store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations
        SET body_json=jsonb_set(body_json,'{expires_at_ms}','1001'::jsonb)
        WHERE id='planning-parent'",
        )
        .await
        .unwrap();
    deny_both_suggestion_entries(&store, &req).await;
    admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations
        SET body_json=jsonb_set(body_json,'{expires_at_ms}','null'::jsonb),status='revoked'
        WHERE id='planning-parent'",
        )
        .await
        .unwrap();
    deny_both_suggestion_entries(&store, &req).await;
    admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='active'
        WHERE id='planning-parent'",
        )
        .await
        .unwrap();
    let mut child = parent;
    child.id = "planning-child".into();
    child.parent_authorization_id = Some("planning-parent".into());
    child.actions = BTreeSet::from([AuthorizedAction::StartWork]);
    issue_planning_grant(&db, child).await;
    deny_both_suggestion_entries(&store, &req).await;
}

#[tokio::test]
async fn suggestion_only_agent_cannot_draft_approve_publish_or_start_execution() {
    let (_guard, admin, db, read) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&read, A, "a").await;
    flip_actor_to_agent(&admin).await;
    issue_planning_grant(
        &db,
        planning_grant(
            "planning-only",
            AuthorizationScope::Project {
                project_id: PROJECT.into(),
            },
        ),
    )
    .await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let draft =
        serde_json::from_value(json!({"request_id":"draft-denied", "mode":"create"})).unwrap();
    assert!(matches!(
        store.planning_draft(TENANT, PROJECT, A, &draft).await,
        Err(PgError::Forbidden)
    ));
    let approval = serde_json::from_value(json!({"request_id":"approve-denied",
        "candidate_id":"absent", "candidate_digest":"absent"}))
    .unwrap();
    assert!(matches!(
        store.planning_approve(TENANT, PROJECT, A, &approval).await,
        Err(PgError::Forbidden)
    ));
    let publish = serde_json::from_value(json!({"request_id":"publish-denied",
        "candidate_id":"absent", "candidate_digest":"absent"}))
    .unwrap();
    assert!(matches!(
        store.planning_publish(TENANT, PROJECT, A, &publish).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        read.commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &prepared,
                    "session-denied",
                    "session.start",
                    json!({"conversation_id":"planning-only"})
                )
            )
            .await,
        Err(PgError::Forbidden)
    ));
}
