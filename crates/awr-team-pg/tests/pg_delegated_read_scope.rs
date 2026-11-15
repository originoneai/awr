//! Actual authenticated PG reads must honor the selected WS-016 delegation.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::*;
use awr_team_pg::{AuthorizationStore, PgError, SourceFile};
use fixture::*;
use serde_json::json;
use std::collections::BTreeSet;

async fn agent_identity(admin: &tokio_postgres::Client) {
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES('reader-tenant','reader-project','alice','Alice','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','bind-agent','alice','agent','active');
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        SELECT tenant_id,project_id,actor_id,'cli-a',workstream_id,authority_version,can_read FROM awr_team.workstream_grants WHERE client_id='cli-b';").await.unwrap();
}
async fn grant(db: &str, scope: AuthorizationScope) {
    let a = AgentAuthorization {
        id: "read-authorization".into(),
        authorizer_person_id: PersonId::new("alice").unwrap(),
        responsible_person_id: PersonId::new("alice").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope,
        actions: BTreeSet::from([AuthorizedAction::Inspect, AuthorizedAction::StartWork]),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1000,
        binding_id: Some("bind-agent".into()),
    };
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "issue".into(),
                authorization: a,
            },
        )
        .await
        .unwrap();
}

async fn grant_stream(db: &str, id: &str, stream: u128, actions: &[AuthorizedAction]) {
    grant_scope(
        db,
        id,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(stream).to_string(),
        },
        actions,
    )
    .await;
}

async fn grant_task(db: &str, id: &str, work: &str, actions: &[AuthorizedAction]) {
    grant_scope(
        db,
        id,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: work.into(),
        },
        actions,
    )
    .await;
}

async fn grant_scope(db: &str, id: &str, scope: AuthorizationScope, actions: &[AuthorizedAction]) {
    let store = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db));
    let mut authorization = store
        .get(TENANT, PROJECT, "read-authorization")
        .await
        .unwrap()
        .unwrap();
    authorization.id = id.into();
    authorization.created_at_ms += 1;
    authorization.scope = scope;
    authorization.actions = actions.iter().copied().collect();
    store
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: id.into(),
                authorization,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn multiple_scopes_are_discoverable_without_sharing_actions_or_hidden_work() {
    let (_g, admin, db, store) = setup_with_three_streams().await;
    agent_identity(&admin).await;
    // Access grants are intentionally broader than the Agent's delegations.
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-a',$3,1,true)", &[&TENANT,&PROJECT,&Id::from(3).to_string()]).await.unwrap();
    enable_writes(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    grant_stream(
        &db,
        "alpha-claim",
        1,
        &[AuthorizedAction::ClaimCoordination],
    )
    .await;
    grant_stream(&db, "beta-read", 2, &[AuthorizedAction::Inspect]).await;
    admin
        .batch_execute("UPDATE awr_team.sessions SET client_id='cli-a' WHERE id='session-b'")
        .await
        .unwrap();

    let streams = store
        .query(TENANT, PROJECT, A, query("workstreams.list"))
        .await
        .unwrap();
    assert_eq!(streams["total"], 2);
    assert_eq!(streams["items"][1]["external_key"], "private-beta");
    assert!(!streams.to_string().contains("hidden-gamma"));
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    let items = next["data"]["items"].as_array().unwrap();
    assert_eq!(
        items.iter().find(|i| i["work_id"] == "a").unwrap()["navigation"],
        "prepare"
    );
    assert_eq!(
        items.iter().find(|i| i["work_id"] == "b-private").unwrap()["navigation"],
        "observe"
    );
    assert!(!next.to_string().contains("d-hidden"));
    let resume = next["data"]["resume"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["work_id"] == "b-private")
        .unwrap();
    let inspect = serde_json::from_value(resume["next_query"].clone()).unwrap();
    store.query(TENANT, PROJECT, A, inspect).await.unwrap();

    for op in ["work.list", "work.graph", "work.search", "events.list"] {
        let mut q = query(op);
        if op == "work.search" {
            q.search = Some("private".into());
        }
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q.clone()).await,
            Err(PgError::Workstream(WorkstreamError::ScopeRequired))
        ));
        q.workstream_id = Some(serde_json::from_value(streams["items"][1]["id"].clone()).unwrap());
        let found = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
        assert!(found.to_string().contains("b-private"));
        q.workstream_id = Some(Id::from(3));
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    let prepared = prepare(&store, A, "b-private").await;
    let denied = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "beta-no-start",
                "session.start",
                json!({"conversation_id":"beta"}),
            ),
        )
        .await;
    assert!(matches!(denied, Err(PgError::Forbidden)), "{denied:?}");
    let denied = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &prepared,
                    "beta-no-claim",
                    "claim.acquire",
                    json!({"session_id":"session-b","expected_session_version":"1",
                        "expected_work_version":prepared["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
                        "ttl_seconds":60})
                )
            )
            .await;
    assert!(matches!(denied, Err(PgError::Forbidden)), "{denied:?}");
}

#[tokio::test]
async fn task_navigation_does_not_promote_its_owner_or_resume_siblings() {
    let (_g, admin, db, store) = setup_with_assigned_tasks().await;
    agent_identity(&admin).await;
    enable_writes(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(2).to_string(),
        },
    )
    .await;
    grant_task(
        &db,
        "assigned-a",
        "a",
        &[
            AuthorizedAction::Inspect,
            AuthorizedAction::ClaimCoordination,
        ],
    )
    .await;
    // Three independent assignments in a different owner stream, in a grant
    // order distinct from work ordering, mirror the native-client regression.
    for task in ["ab-assigned", "aa-assigned"] {
        grant_task(&db, task, task, &[AuthorizedAction::Inspect]).await;
    }
    // An own session on an unassigned sibling must not appear through the
    // readable owner retained for task a's per-row authorization checks.
    admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        VALUES($1,$2,'session-c','main','c','agent','cli-a','sibling','active',$3,1)", &[&TENANT,&PROJECT,&Id::from(1).to_string()]).await.unwrap();
    let streams = store
        .query(TENANT, PROJECT, A, query("workstreams.list"))
        .await
        .unwrap();
    assert_eq!(streams["total"], 1);
    assert_eq!(streams["items"][0]["external_key"], "private-beta");
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    let items = next["data"]["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|i| i["work_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["a", "aa-assigned", "ab-assigned", "b-private"]
    );
    assert_eq!(items[0]["navigation"], "prepare");
    assert!(
        items[1..]
            .iter()
            .all(|item| item["navigation"] == "observe")
    );
    for item in items {
        let prepared = store
            .query(
                TENANT,
                PROJECT,
                A,
                serde_json::from_value(item["next_query"].clone()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(prepared["data"]["work_id"], item["work_id"]);
    }
    assert_eq!(next["data"]["resume"].as_array().unwrap().len(), 1);
    assert_eq!(next["data"]["resume"][0]["session_id"], "session-a");
    assert!(!next.to_string().contains("session-c"));
    let mut q = query("work.prepare");
    q.work_id = Some("c".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    let mut q = query("work.list");
    q.workstream_id = Some(Id::from(1));
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    // Task a's claim action cannot borrow the other stream's StartWork action.
    let prepared = prepare(&store, A, "a").await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &prepared,
                    "task-no-start",
                    "session.start",
                    json!({"conversation_id":"denied"})
                )
            )
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn task_navigation_pages_in_global_order_and_invalidates_changed_scope() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    grant_task(&db, "assigned-b", "b-private", &[AuthorizedAction::Inspect]).await;
    let mut q = query("work.next");
    q.limit = Some(1);
    let mut seen = Vec::new();
    loop {
        let page = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
        let items = page["data"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        seen.push(items[0]["work_id"].as_str().unwrap().to_owned());
        q.cursor = page["data"]["next_cursor"].as_str().map(Into::into);
        if q.cursor.is_none() {
            break;
        }
        assert!(seen.len() < 4);
    }
    assert_eq!(seen, vec!["a", "b-private", "c"]);
    let original: serde_json::Value = admin
        .query_one(
            "SELECT body_json FROM awr_team.agent_authorizations WHERE id='assigned-b'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    for change in [
        "revoked",
        "expired",
        "no-inspect",
        "body",
        "access",
        "owner",
    ] {
        q.cursor = None;
        let page = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
        q.cursor = Some(page["data"]["next_cursor"].as_str().unwrap().into());
        let mut changed = original.clone();
        match change {
            "revoked" => changed["status"] = json!("revoked"),
            "expired" => changed["expires_at_ms"] = json!(1),
            "no-inspect" => changed["actions"] = json!(["start_work"]),
            "body" => changed["created_at_ms"] = json!(1002),
            "access" => {
                admin.execute("UPDATE awr_team.workstream_grants SET can_read=false,grant_version=grant_version+1 WHERE client_id='cli-a' AND workstream_id=$1", &[&Id::from(2).to_string()]).await.unwrap();
            }
            "owner" => {
                admin.execute("UPDATE awr_team.workstream_snapshot_ownership SET workstream_id=$1 WHERE work_id='b-private'", &[&Id::from(1).to_string()]).await.unwrap();
            }
            _ => unreachable!(),
        }
        admin
            .execute(
                "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='assigned-b'",
                &[&changed],
            )
            .await
            .unwrap();
        assert!(
            matches!(
                store.query(TENANT, PROJECT, A, q.clone()).await,
                Err(PgError::CursorExpired)
            ),
            "{change}"
        );
        admin
            .execute(
                "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='assigned-b'",
                &[&original],
            )
            .await
            .unwrap();
        admin.execute("UPDATE awr_team.workstream_grants SET can_read=true WHERE client_id='cli-a' AND workstream_id=$1", &[&Id::from(2).to_string()]).await.unwrap();
        admin.execute("UPDATE awr_team.workstream_snapshot_ownership SET workstream_id=$1 WHERE work_id='b-private'", &[&Id::from(2).to_string()]).await.unwrap();
    }
}

#[tokio::test]
async fn task_navigation_rechecks_live_delegation_access_and_exact_task_scope() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    grant_task(&db, "assigned-b", "b-private", &[AuthorizedAction::Inspect]).await;
    let original: serde_json::Value = admin
        .query_one(
            "SELECT body_json FROM awr_team.agent_authorizations WHERE id='assigned-b'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    for (field, value) in [
        ("expires_at_ms", json!(1)),
        ("client_id", json!("cli-b")),
        ("status", json!("revoked")),
        ("actions", json!(["start_work", "claim_coordination"])),
        (
            "scope",
            serde_json::to_value(AuthorizationScope::Task {
                project_id: PROJECT.into(),
                work_item_id: "missing".into(),
            })
            .unwrap(),
        ),
        (
            "scope",
            serde_json::to_value(AuthorizationScope::TaskPool {
                project_id: PROJECT.into(),
                pool_id: "b".into(),
            })
            .unwrap(),
        ),
    ] {
        let mut invalid = original.clone();
        invalid[field] = value;
        admin
            .execute(
                "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='assigned-b'",
                &[&invalid],
            )
            .await
            .unwrap();
        let next = store
            .query(TENANT, PROJECT, A, query("work.next"))
            .await
            .unwrap();
        assert!(!next.to_string().contains("b-private"), "{field}: {next}");
    }
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='assigned-b'",
            &[&original],
        )
        .await
        .unwrap();
    admin.execute("UPDATE awr_team.workstream_grants SET can_read=false WHERE client_id='cli-a' AND workstream_id=$1", &[&Id::from(2).to_string()]).await.unwrap();
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert!(!next.to_string().contains("b-private"));
}

#[tokio::test]
async fn discovery_cursors_track_every_contributing_authorization_and_access_grant() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    grant_stream(
        &db,
        "beta-read",
        2,
        &[AuthorizedAction::Inspect, AuthorizedAction::StartWork],
    )
    .await;
    grant_stream(
        &db,
        "alpha-claim",
        1,
        &[AuthorizedAction::ClaimCoordination],
    )
    .await;
    for op in ["workstreams.list", "work.next"] {
        for change in ["authorization", "action-only", "access"] {
            let mut q = query(op);
            q.limit = Some(1);
            let page = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
            q.cursor = Some(
                if op == "work.next" {
                    &page["data"]["next_cursor"]
                } else {
                    &page["next_cursor"]
                }
                .as_str()
                .unwrap()
                .into(),
            );
            if change == "authorization" {
                admin.execute("UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{created_at_ms}',to_jsonb((body_json->>'created_at_ms')::bigint+1)) WHERE id='beta-read'", &[]).await.unwrap();
            } else if change == "action-only" {
                admin.execute("UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{actions}', CASE WHEN body_json->'actions'='[]'::jsonb THEN '[\"claim_coordination\"]'::jsonb ELSE '[]'::jsonb END) WHERE id='alpha-claim'", &[]).await.unwrap();
            } else {
                admin.execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a' AND workstream_id=$1", &[&Id::from(2).to_string()]).await.unwrap();
            }
            assert!(
                matches!(
                    store.query(TENANT, PROJECT, A, q).await,
                    Err(PgError::CursorExpired)
                ),
                "{op}/{change}"
            );
        }
    }
}

#[tokio::test]
async fn unusable_additional_delegations_do_not_reveal_their_scope() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    grant_stream(&db, "beta-read", 2, &[AuthorizedAction::Inspect]).await;
    let original: serde_json::Value = admin
        .query_one(
            "SELECT body_json FROM awr_team.agent_authorizations WHERE id='beta-read'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    for (field, value) in [
        ("expires_at_ms", json!(1)),
        ("client_id", json!("cli-b")),
        ("status", json!("revoked")),
    ] {
        let mut invalid = original.clone();
        invalid[field] = value;
        admin
            .execute(
                "UPDATE awr_team.agent_authorizations SET body_json=$1 WHERE id='beta-read'",
                &[&invalid],
            )
            .await
            .unwrap();
        let streams = store
            .query(TENANT, PROJECT, A, query("workstreams.list"))
            .await
            .unwrap();
        assert_eq!(streams["total"], 1, "{field}");
        let next = store
            .query(TENANT, PROJECT, A, query("work.next"))
            .await
            .unwrap();
        assert!(!next.to_string().contains("b-private"), "{field}");
    }
}

#[tokio::test]
async fn workstream_delegation_filters_discovery_navigation_sources_and_denies_project_receipts() {
    let (_g, admin, db, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"# Public alpha contract".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"# Private beta contract".to_vec(),
        },
    ])
    .await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    let streams = store
        .query(TENANT, PROJECT, A, query("workstreams.list"))
        .await
        .unwrap();
    assert_eq!(streams["total"], 1);
    assert_eq!(streams["items"][0]["external_key"], "alpha");
    for op in [
        "capabilities",
        "work.list",
        "work.graph",
        "planning.candidates",
        "sources.inspect",
        "work.search",
        "work.next",
    ] {
        let mut q = query(op);
        if op == "work.search" {
            q.search = Some("private".into());
        }
        let got = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert!(!got.to_string().contains("b-private"), "{op}: {got}");
        assert!(!got.to_string().contains("private-beta"), "{op}: {got}");
    }
    for op in [
        "work.list",
        "work.graph",
        "work.search",
        "events.list",
        "planning.candidates",
        "sources.inspect",
    ] {
        let mut q = query(op);
        q.workstream_id = Some(Id::from(2));
        if op == "work.search" {
            q.search = Some("private".into());
        }
        assert!(store.query(TENANT, PROJECT, A, q).await.is_err(), "{op}");
    }
    let mut q = query("source.content");
    q.source_path = Some("docs/alpha.md".into());
    let own = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert!(own.to_string().contains("Public alpha"));
    let mut q = query("source.content");
    q.source_path = Some("docs/private-beta.md".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    let mut q = query("planning.outcome");
    q.request_id = Some("any-project-receipt".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    let mut q = query("session.inspect");
    q.session_id = Some("session-b".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(prepare(&store, A, "a").await["data"]["work_id"], "a");
}

#[tokio::test]
async fn task_grants_discover_only_exact_work_without_enabling_other_discovery() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
    )
    .await;
    for op in [
        "capabilities",
        "workstreams.list",
        "work.list",
        "planning.candidates",
        "sources.inspect",
        "events.list",
        "audit.requests",
    ] {
        assert!(
            matches!(
                store.query(TENANT, PROJECT, A, query(op)).await,
                Err(PgError::Forbidden)
            ),
            "{op}"
        );
    }
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(next["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["data"]["items"][0]["work_id"], "a");
    assert_eq!(next["data"]["resume"].as_array().unwrap().len(), 1);
    assert_eq!(next["data"]["resume"][0]["session_id"], "session-a");
    let mut search = query("work.search");
    search.search = Some("a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, search).await,
        Err(PgError::Forbidden)
    ));
    let mut list = query("work.list");
    list.workstream_id = Some(Id::from(1));
    assert!(matches!(
        store.query(TENANT, PROJECT, A, list).await,
        Err(PgError::Forbidden)
    ));
    let mut q = query("session.inspect");
    q.session_id = Some("session-a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::Forbidden)
    ));
    q.work_id = Some("a".into());
    store.query(TENANT, PROJECT, A, q).await.unwrap();
    let own = prepare(&store, A, "a").await;
    assert_eq!(own["data"]["work_id"], "a");
    for work in ["c", "b-private"] {
        let mut q = query("work.prepare");
        q.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_read=false WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.next")).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn review_history_requires_current_exact_work_delegation_even_with_broader_stream_access() {
    let (_guard, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
    )
    .await;
    let mut selected = query("review.rounds");
    selected.work_id = Some("a".into());
    let own = store
        .query(TENANT, PROJECT, A, selected.clone())
        .await
        .unwrap();
    assert_eq!(own["data"]["work_id"], "a");
    assert_eq!(own["data"]["items"], json!([]));
    let mut session = query("review.rounds");
    session.session_id = Some("session-a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, session.clone()).await,
        Err(PgError::Forbidden)
    ));
    session.work_id = Some("a".into());
    session.workstream_id = Some(Id::from(1));
    assert_eq!(
        store.query(TENANT, PROJECT, A, session).await.unwrap()["data"]["work_id"],
        "a"
    );
    for work in ["c", "b-private"] {
        let mut q = selected.clone();
        q.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    admin.execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='read-authorization'", &[]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, selected).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn handoff_discovery_unions_only_readable_exact_tasks_and_rechecks_revocation() {
    let (_guard, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
    )
    .await;
    for (id, work) in [
        ("handoff-a", "a"),
        ("handoff-c", "c"),
        ("PRIVATE HANDOFF", "b-private"),
    ] {
        admin.execute("INSERT INTO awr_team.team_handoffs(tenant_id,project_id,id,work_id,kind,status,version,
            from_person_id,to_person_id,package_json,body_json,created_at_ms,updated_at_ms)
            VALUES($1,$2,$3,$4,'execution','accepted',1,'alice','alice','{}','{}',100,100)",
            &[&TENANT,&PROJECT,&id,&work]).await.unwrap();
    }
    let mut q = query("handoff.list");
    q.handoff_state = Some("all".into());
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 1);
    assert_eq!(first["data"]["items"][0]["work_id"], "a");
    assert_eq!(first["data"]["count_limited"], true);
    assert!(!first.to_string().contains("PRIVATE"));
    let mut sibling = q.clone();
    sibling.work_id = Some("c".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, sibling).await,
        Err(PgError::Forbidden)
    ));
    let mut stream = q.clone();
    stream.workstream_id = Some(Id::from(1));
    assert!(matches!(
        store.query(TENANT, PROJECT, A, stream).await,
        Err(PgError::Forbidden)
    ));
    grant_task(&db, "other-task-read", "c", &[AuthorizedAction::Inspect]).await;
    q.limit = Some(1);
    let both = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(both["data"]["total"], 2);
    q.cursor = both["data"]["next_cursor"].as_str().map(Into::into);
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='other-task-read'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    q.cursor = None;
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["total"],
        1
    );
    admin.execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='read-authorization'",&[]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn ordinary_question_discovery_filters_exact_tasks_before_pagination_and_rechecks_revocation()
{
    let (_guard, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
    )
    .await;
    for (id, work, session) in [
        ("question-a", "a", "session-a"),
        ("question-c", "c", "session-a"),
        ("PRIVATE QUESTION", "b-private", "session-b"),
    ] {
        let metadata = json!({"basis":{"contract_hash":"a".repeat(64)},"impact":"The team is waiting for a scope decision.",
            "requester":{"person_id":"alice","actor_id":"agent","client_id":"cli-a"},
            "allowed_responder_person_ids":["alice"],"source_snapshot_id":"fixture"});
        admin.execute("INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state,
            decision_key,decision_baseline_digest,decision_json,decision_version)
            VALUES($1,$2,$3,$4,$5,$3,'open',$3,$6,$7,1)",
            &[&TENANT,&PROJECT,&id,&session,&work,&"a".repeat(64),&metadata]).await.unwrap();
    }
    let mut q = query("decisions.list");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["data"]["items"][0]["work_id"], "a");
    assert!(first["data"]["next_cursor"].is_null());
    assert!(!first.to_string().contains("PRIVATE"));
    assert!(!first.to_string().contains("question-c"));
    q.work_id = Some("c".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::Forbidden)
    ));
    q.work_id = None;
    admin.execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='read-authorization'",&[]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn dependency_discovery_keeps_task_scope_and_reauthorizes_upstream_metadata() {
    let (_guard, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "c".into(),
        },
    )
    .await;
    let q = query("dependencies.list");
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 1);
    assert_eq!(first["data"]["items"][0]["work_id"], "c");
    assert_eq!(first["data"]["items"][0]["provider_work_id"], "b-private");
    assert!(first["data"]["items"][0]["provider_title"].is_null());
    let mut sibling = q.clone();
    sibling.work_id = Some("a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, sibling).await,
        Err(PgError::Forbidden)
    ));
    grant_task(
        &db,
        "upstream-read",
        "b-private",
        &[AuthorizedAction::Inspect],
    )
    .await;
    let readable = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(readable["data"]["total"], 1);
    assert_eq!(readable["data"]["items"][0]["provider_title"], "b-private");
    admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='upstream-read'",
        )
        .await
        .unwrap();
    let restricted = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert!(restricted["data"]["items"][0]["provider_title"].is_null());
    assert_eq!(
        restricted["data"]["items"][0]["missing"]["provider_title"],
        "permission_restricted"
    );
    admin.batch_execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='read-authorization'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
}

async fn dependency_selector_parity(store: &awr_team_pg::WorkstreamReadStore) {
    let mut expected = None;
    for (work, stream) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut q = query("dependencies.list");
        if work {
            q.work_id = Some("c".into());
        }
        if stream {
            q.workstream_id = Some(Id::from(1));
        }
        let response = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert_eq!(response["data"]["total"], 1);
        let mut item = response["data"]["items"][0].clone();
        assert_eq!(
            item["provider_title"], "b-private",
            "work={work} stream={stream}"
        );
        item.as_object_mut().unwrap().remove("dependency_position");
        item.as_object_mut().unwrap().remove("work_title");
        if let Some(expected) = &expected {
            assert_eq!(&item, expected);
        } else {
            expected = Some(item);
        }
    }
    for stream in [false, true] {
        let mut q = query("delivery.inspect");
        q.work_id = Some("c".into());
        if stream {
            q.workstream_id = Some(Id::from(1));
        }
        let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert_eq!(
            inspected["data"]["delivery"]["dependencies"],
            json!([expected.as_ref().unwrap()])
        );
    }
}

#[tokio::test]
async fn dependency_narrowed_selectors_preserve_separate_task_grants_without_promoting_consumer_access()
 {
    let (_guard, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "c".into(),
        },
    )
    .await;
    grant_task(
        &db,
        "upstream-read",
        "b-private",
        &[AuthorizedAction::Inspect],
    )
    .await;
    dependency_selector_parity(&store).await;
    for op in ["dependencies.list", "delivery.inspect"] {
        let mut q = query(op);
        q.work_id = Some("a".into());
        q.workstream_id = Some(Id::from(1));
        assert!(
            matches!(
                store.query(TENANT, PROJECT, A, q).await,
                Err(PgError::Forbidden)
            ),
            "{op}"
        );
    }
    admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='upstream-read'",
        )
        .await
        .unwrap();
    let mut q = query("dependencies.list");
    q.work_id = Some("c".into());
    q.workstream_id = Some(Id::from(1));
    let restricted = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert!(restricted["data"]["items"][0]["provider_title"].is_null());
}

#[tokio::test]
async fn dependency_narrowed_workstream_reads_preserve_upstream_task_grants() {
    let (_guard, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    grant_task(
        &db,
        "upstream-read",
        "b-private",
        &[AuthorizedAction::Inspect],
    )
    .await;
    dependency_selector_parity(&store).await;
}

#[tokio::test]
async fn agent_authentication_defaults_to_no_product_actions_even_on_separate_audit_surface() {
    let (_g, admin, db, store) = setup().await;
    // Human behavior stays unchanged.
    assert_eq!(
        store
            .query(TENANT, PROJECT, A, query("workstreams.list"))
            .await
            .unwrap()["total"],
        1
    );
    agent_identity(&admin).await;
    for op in [
        "audit.history",
        "audit.export",
        "audit.count",
        "capabilities",
    ] {
        assert!(
            matches!(
                store.query(TENANT, PROJECT, A, query(op)).await,
                Err(PgError::Forbidden)
            ),
            "{op}"
        );
    }
    grant(
        &db,
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
    )
    .await;
    assert_eq!(
        store
            .query(TENANT, PROJECT, A, query("workstreams.list"))
            .await
            .unwrap()["total"],
        2
    );
    for op in ["audit.history", "audit.export", "audit.count"] {
        assert!(
            matches!(
                store.query(TENANT, PROJECT, A, query(op)).await,
                Err(PgError::Forbidden)
            ),
            "{op}"
        );
    }
}

#[tokio::test]
async fn delegated_cursor_is_bound_to_authorization_body() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    let mut q = query("work.list");
    q.limit = Some(1);
    let page = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    q.cursor = Some(page["data"]["next_cursor"].as_str().unwrap().into());
    admin.execute("UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{actions}',$1) WHERE id='read-authorization'",&[&json!(["inspect"])]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
}

#[tokio::test]
async fn same_action_scope_narrowing_cannot_fall_back_to_broader_parent() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
    )
    .await;
    let auth = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let parent = auth
        .get(TENANT, PROJECT, "read-authorization")
        .await
        .unwrap()
        .unwrap();
    let mut child = parent.clone();
    child.id = "narrow-stream".into();
    child.scope = AuthorizationScope::Workstream {
        project_id: PROJECT.into(),
        workstream_id: Id::from(1).to_string(),
    };
    auth.delegate(
        TENANT,
        PROJECT,
        &DelegateAuthorizationRequest {
            request_key: "narrow-stream".into(),
            parent_authorization_id: parent.id.clone(),
            child,
        },
        2000,
    )
    .await
    .unwrap();
    assert_eq!(
        store
            .query(TENANT, PROJECT, A, query("workstreams.list"))
            .await
            .unwrap()["total"],
        1
    );
    let mut q = query("work.prepare");
    q.work_id = Some("b-private".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    // A separate grant, without a parent link, remains independent.
    let mut independent = parent.clone();
    independent.id = "independent-other".into();
    independent.created_at_ms = 3000;
    independent.scope = AuthorizationScope::Task {
        project_id: PROJECT.into(),
        work_item_id: "b-private".into(),
    };
    auth.issue(
        TENANT,
        PROJECT,
        &IssueAuthorizationRequest {
            request_key: "independent".into(),
            authorization: independent,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        prepare(&store, A, "b-private").await["data"]["work_id"],
        "b-private"
    );
}

#[tokio::test]
async fn task_child_limits_navigation_and_prevents_parent_fallback_on_sibling() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
    )
    .await;
    let auth = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let parent = auth
        .get(TENANT, PROJECT, "read-authorization")
        .await
        .unwrap()
        .unwrap();
    let mut child = parent.clone();
    child.id = "narrow-task".into();
    child.scope = AuthorizationScope::Task {
        project_id: PROJECT.into(),
        work_item_id: "a".into(),
    };
    auth.delegate(
        TENANT,
        PROJECT,
        &DelegateAuthorizationRequest {
            request_key: "narrow-task".into(),
            parent_authorization_id: parent.id.clone(),
            child,
        },
        2000,
    )
    .await
    .unwrap();
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(next["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["data"]["items"][0]["work_id"], "a");
    assert!(matches!(
        store
            .query(TENANT, PROJECT, A, query("workstreams.list"))
            .await,
        Err(PgError::Forbidden)
    ));
    let mut q = query("work.prepare");
    q.work_id = Some("c".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(prepare(&store, A, "a").await["data"]["work_id"], "a");
}

#[tokio::test]
async fn scoped_agent_can_save_the_context_returned_by_native_prepare() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    enable_writes(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    let prepared = prepare(&store, A, "a").await;
    let started = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-start",
                "session.start",
                json!({"conversation_id":"scoped-checkpoint"}),
            ),
        )
        .await
        .unwrap();
    let id = started["receipt"]["data"]["session_id"].as_str().unwrap();
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    q.session_id = Some(id.into());
    let context = store.query(TENANT, PROJECT, A, q).await.unwrap();
    let saved=store.commands().execute(TENANT,PROJECT,A,command(&context,"agent-checkpoint","session.checkpoint",json!({
        "session_id":id,"expected_session_version":"1","context_hash":context["data"]["context_hash"],"next_action":"Continue integration","open_loops":["Review pending"]}))).await.unwrap();
    assert_eq!(saved["receipt"]["data"]["session_version"], "2");
}

#[tokio::test]
async fn checkpoint_uses_the_same_scoped_shared_sources_as_prepare() {
    let (_g, admin, db, store) = setup_with_specs(vec![SourceFile {
        path: "docs/alpha.md".into(),
        bytes: b"Shared contract requires both streams".to_vec(),
    }])
    .await;
    // Both credential-readable streams reference this document, while the
    // explicit Agent delegation only permits the first stream.
    admin.batch_execute("UPDATE awr_team.workstream_catalogs SET catalog_json=jsonb_set(catalog_json,'{workstreams,1,acceptance_contracts}','[\"docs/alpha.md\"]')").await.unwrap();
    agent_identity(&admin).await;
    enable_writes(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    q.session_id = Some("session-a".into());
    let context = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(context["data"]["required_specs"], json!([]));
    assert_eq!(
        context["data"]["completeness_reasons"],
        json!(["required_spec_forbidden"])
    );
    let saved = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &context,
                "shared-source-checkpoint",
                "session.checkpoint",
                json!({
                    "session_id":"session-a", "expected_session_version":"1",
                    "context_hash":context["data"]["context_hash"],
                    "next_action":"Request an authorized copy of the shared contract",
                    "open_loops":["Required shared contract is not readable"]
                }),
            ),
        )
        .await
        .unwrap();
    assert_eq!(saved["receipt"]["data"]["session_version"], "2");
}

#[tokio::test]
async fn checkpoint_resolves_read_and_command_grants_independently() {
    let (_g, admin, db, store) = setup().await;
    agent_identity(&admin).await;
    enable_writes(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    let authorizations =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut start_only = authorizations
        .get(TENANT, PROJECT, "read-authorization")
        .await
        .unwrap()
        .unwrap();
    start_only.id = "separate-start-authorization".into();
    start_only.created_at_ms = 2000;
    start_only.actions = BTreeSet::from([AuthorizedAction::StartWork]);
    authorizations
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "separate-start".into(),
                authorization: start_only,
            },
        )
        .await
        .unwrap();
    admin.execute("UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{actions}',$1) WHERE id='read-authorization'",&[&json!(["inspect"])]).await.unwrap();
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    q.session_id = Some("session-a".into());
    let context = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    let checkpoint = command(
        &context,
        "separate-grant-checkpoint",
        "session.checkpoint",
        json!({
            "session_id":"session-a", "expected_session_version":"1",
            "context_hash":context["data"]["context_hash"],
            "next_action":"Continue with separate read and work authority", "open_loops":[]
        }),
    );
    let saved = store
        .commands()
        .execute(TENANT, PROJECT, A, checkpoint)
        .await
        .unwrap();
    assert_eq!(saved["receipt"]["data"]["session_version"], "2");

    // Losing WorkRead cannot be bypassed by retaining StartWork and replaying
    // a previously consumed hash in a fresh checkpoint command.
    let context = store.query(TENANT, PROJECT, A, q).await.unwrap();
    admin.execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='read-authorization'",&[]).await.unwrap();
    let denied = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &context,
                "read-revoked-checkpoint",
                "session.checkpoint",
                json!({
                    "session_id":"session-a", "expected_session_version":"2",
                    "context_hash":context["data"]["context_hash"],
                    "next_action":"Must not save without live read authority", "open_loops":[]
                }),
            ),
        )
        .await;
    assert!(matches!(denied, Err(PgError::Forbidden)));
}

#[tokio::test]
async fn document_content_selects_a_covering_grant_after_an_earlier_task_grant() {
    for scope in [
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
    ] {
        let (_guard, admin, db, store) = setup_with_specs(vec![
            SourceFile {
                path: "docs/alpha.md".into(),
                bytes: b"# Delivery guide\n\nReview delivery.".to_vec(),
            },
            SourceFile {
                path: "docs/private-beta.md".into(),
                bytes: b"# Peer guide\n\nReview peer delivery.".to_vec(),
            },
        ])
        .await;
        agent_identity(&admin).await;
        grant(
            &db,
            AuthorizationScope::Task {
                project_id: PROJECT.into(),
                work_item_id: "a".into(),
            },
        )
        .await;
        grant_scope(&db, "later-broad-read", scope, &[AuthorizedAction::Inspect]).await;
        for stream in [None, Some(Id::from(1))] {
            let mut q = query("documents.list");
            q.workstream_id = stream;
            let result = store.query(TENANT, PROJECT, A, q).await.unwrap();
            let items = result["data"]["items"].as_array().unwrap();
            assert!(!items.is_empty());
            for item in items {
                assert_eq!(item["readable"], true, "{item}");
                assert!(item["content_work_id"].is_null());
                let mut content = query("source.content");
                content.source_path = item["path"].as_str().map(Into::into);
                content.expected_sha256 = item["sha256"].as_str().map(Into::into);
                let body = store
                    .query(TENANT, PROJECT, A, content.clone())
                    .await
                    .unwrap();
                assert_eq!(body["data"]["byte_length"], item["byte_length"]);
                content.work_id = Some(
                    if item["path"] == "docs/alpha.md" {
                        "a"
                    } else {
                        "b-private"
                    }
                    .into(),
                );
                store.query(TENANT, PROJECT, A, content).await.unwrap();
            }
        }
        // Grant order can change which candidate is first, never availability.
        admin.execute("UPDATE awr_team.agent_authorizations SET created_at_ms=0 WHERE id='later-broad-read'", &[]).await.unwrap();
        let result = store
            .query(TENANT, PROJECT, A, query("documents.list"))
            .await
            .unwrap();
        assert!(
            result["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["readable"] == true)
        );
    }
}

#[tokio::test]
async fn independent_stream_document_reads_do_not_union_shared_source_authority() {
    let (_guard, admin, db, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"# Delivery guide".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"# Peer guide".to_vec(),
        },
        SourceFile {
            path: "docs/shared.md".into(),
            bytes: b"# Shared guide".to_vec(),
        },
    ])
    .await;
    let mut catalog: serde_json::Value = admin
        .query_one("SELECT catalog_json FROM awr_team.workstream_catalogs", &[])
        .await
        .unwrap()
        .get(0);
    for stream in catalog["workstreams"].as_array_mut().unwrap() {
        stream["acceptance_contracts"]
            .as_array_mut()
            .unwrap()
            .push(json!("docs/shared.md"));
    }
    admin
        .execute(
            "UPDATE awr_team.workstream_catalogs SET catalog_json=$1",
            &[&catalog],
        )
        .await
        .unwrap();
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: Id::from(1).to_string(),
        },
    )
    .await;
    grant_stream(&db, "peer-stream-read", 2, &[AuthorizedAction::Inspect]).await;
    for selected in [None, Some(Id::from(1)), Some(Id::from(2))] {
        let mut q = query("documents.list");
        q.workstream_id = selected;
        let result = store.query(TENANT, PROJECT, A, q).await.unwrap();
        let items = result["data"]["items"].as_array().unwrap();
        assert_eq!(
            items.iter().filter(|item| item["readable"] == true).count(),
            if selected.is_none() { 2 } else { 1 }
        );
        assert_eq!(
            items
                .iter()
                .filter(|item| item["status"] == "restricted")
                .count(),
            1
        );
        for item in items.iter().filter(|item| item["readable"] == true) {
            let mut content = query("source.content");
            content.source_path = item["path"].as_str().map(Into::into);
            content.expected_sha256 = item["sha256"].as_str().map(Into::into);
            let body = store.query(TENANT, PROJECT, A, content).await.unwrap();
            assert_eq!(body["data"]["sha256"], item["sha256"]);
        }
    }
    let mut shared = query("source.content");
    shared.source_path = Some("docs/shared.md".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, shared.clone()).await,
        Err(PgError::Forbidden)
    ));
    grant_scope(
        &db,
        "project-read",
        AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        &[AuthorizedAction::Inspect],
    )
    .await;
    let result = store
        .query(TENANT, PROJECT, A, query("documents.list"))
        .await
        .unwrap();
    assert_eq!(result["data"]["total"], 3);
    assert!(
        result["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["readable"] == true)
    );
    store
        .query(TENANT, PROJECT, A, shared.clone())
        .await
        .unwrap();
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='project-read'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, shared).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn task_document_discovery_and_content_use_the_same_exact_live_read_scope() {
    let body = "# Delivery guide\n\nCheck the outcome before sharing it.";
    let (_guard, admin, db, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: body.as_bytes().to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"# PRIVATE peer guide\n\nPeer-only instructions.".to_vec(),
        },
    ])
    .await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
    )
    .await;
    let mut q = query("documents.list");
    let project = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(project["data"]["scope"], "authorized_works");
    assert_eq!(project["data"]["total"], 1);
    let item = &project["data"]["items"][0];
    assert_eq!(item["name"], "Delivery guide");
    assert_eq!(item["content_work_id"], "a");
    assert_eq!(item["readable"], true);
    assert!(!project.to_string().contains("PRIVATE"));
    assert!(!project.to_string().contains("private-beta"));
    let mut content = query("source.content");
    content.source_path = item["path"].as_str().map(Into::into);
    content.expected_sha256 = item["sha256"].as_str().map(Into::into);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, content.clone()).await,
        Err(PgError::Forbidden)
    ));
    content.work_id = item["content_work_id"].as_str().map(Into::into);
    assert_eq!(
        store
            .query(TENANT, PROJECT, A, content.clone())
            .await
            .unwrap()["data"]["text"],
        body
    );
    for work in ["c", "b-private"] {
        content.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, content.clone()).await,
            Err(PgError::Forbidden)
        ));
    }
    q.search = Some("PRIVATE".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["total"],
        0
    );
    q.search = None;
    q.workstream_id = Some(Id::from(1));
    assert_eq!(
        store.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["items"][0]["content_work_id"],
        "a"
    );
    q.workstream_id = Some(Id::from(2));
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::Forbidden)
    ));
    admin.execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='read-authorization'", &[]).await.unwrap();
    content.work_id = Some("a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, content).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store
            .query(TENANT, PROJECT, A, query("documents.list"))
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn document_task_paging_rechecks_current_grants_without_promoting_task_owners_to_streams() {
    let (_guard, admin, db, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"# Delivery guide\n\nReview delivery.".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"# Peer guide\n\nReview peer delivery.".to_vec(),
        },
    ])
    .await;
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
    )
    .await;
    grant_task(
        &db,
        "peer-task-read",
        "b-private",
        &[AuthorizedAction::Inspect],
    )
    .await;
    let mut q = query("documents.list");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(first["data"]["total"], 2);
    assert_eq!(first["data"]["items"][0]["content_work_id"], "a");
    q.cursor = first["data"]["next_cursor"].as_str().map(Into::into);
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(second["data"]["items"][0]["name"], "Peer guide");
    assert_eq!(second["data"]["items"][0]["content_work_id"], "b-private");
    let mut content = query("source.content");
    content.work_id = Some("b-private".into());
    content.source_path = second["data"]["items"][0]["path"].as_str().map(Into::into);
    content.expected_sha256 = second["data"]["items"][0]["sha256"]
        .as_str()
        .map(Into::into);
    assert!(
        store.query(TENANT, PROJECT, A, content).await.unwrap()["data"]["text"]
            .as_str()
            .unwrap()
            .contains("Review peer delivery")
    );
    admin.execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='read-authorization'", &[]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    q.cursor = None;
    let narrowed = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(narrowed["data"]["total"], 1);
    assert_eq!(narrowed["data"]["items"][0]["content_work_id"], "b-private");
    assert!(!narrowed.to_string().contains("Delivery guide"));
}

#[tokio::test]
async fn task_document_metadata_matches_the_content_scope_for_shared_sources() {
    let (_guard, admin, db, store) = setup_with_specs(vec![
        SourceFile {
            path: "docs/alpha.md".into(),
            bytes: b"# PRIVATE shared guidance\n\nShared guidance body.".to_vec(),
        },
        SourceFile {
            path: "docs/private-beta.md".into(),
            bytes: b"# Peer guide\n\nReview peer delivery.".to_vec(),
        },
    ])
    .await;
    let mut catalog: serde_json::Value = admin
        .query_one("SELECT catalog_json FROM awr_team.workstream_catalogs", &[])
        .await
        .unwrap()
        .get(0);
    catalog["workstreams"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|stream| stream["id"] == Id::from(2).to_string())
        .unwrap()["acceptance_contracts"]
        .as_array_mut()
        .unwrap()
        .push(json!("docs/alpha.md"));
    admin
        .execute(
            "UPDATE awr_team.workstream_catalogs SET catalog_json=$1",
            &[&catalog],
        )
        .await
        .unwrap();
    agent_identity(&admin).await;
    grant(
        &db,
        AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
        },
    )
    .await;
    grant_task(
        &db,
        "peer-task-read",
        "b-private",
        &[AuthorizedAction::Inspect],
    )
    .await;
    let result = store
        .query(TENANT, PROJECT, A, query("documents.list"))
        .await
        .unwrap();
    assert_eq!(result["data"]["total"], 2);
    let restricted = result["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["status"] == "restricted")
        .unwrap();
    for field in [
        "path",
        "name",
        "summary",
        "sha256",
        "byte_length",
        "content_work_id",
    ] {
        assert!(
            restricted[field].is_null(),
            "restricted {field} must remain hidden"
        );
    }
    assert!(!result.to_string().contains("PRIVATE shared guidance"));
    let mut content = query("source.content");
    content.source_path = Some("docs/alpha.md".into());
    for work in ["a", "b-private"] {
        content.work_id = Some(work.into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, content.clone()).await,
            Err(PgError::Forbidden)
        ));
    }
}
