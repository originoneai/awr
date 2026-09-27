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
    let store = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db));
    let mut authorization = store
        .get(TENANT, PROJECT, "read-authorization")
        .await
        .unwrap()
        .unwrap();
    authorization.id = id.into();
    authorization.created_at_ms += 1;
    authorization.scope = AuthorizationScope::Workstream {
        project_id: PROJECT.into(),
        workstream_id: Id::from(stream).to_string(),
    };
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

    for op in ["work.list", "work.search", "events.list"] {
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
    for op in ["capabilities", "work.list", "work.search", "work.next"] {
        let mut q = query(op);
        if op == "work.search" {
            q.search = Some("private".into());
        }
        let got = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert!(!got.to_string().contains("b-private"), "{op}: {got}");
        assert!(!got.to_string().contains("private-beta"), "{op}: {got}");
    }
    for op in ["work.list", "work.search", "events.list"] {
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
async fn task_grants_require_an_explicit_covered_work_and_do_not_enable_discovery() {
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
        "work.next",
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
async fn task_child_prevents_selector_free_reads_and_parent_fallback_on_sibling() {
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
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.next")).await,
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
