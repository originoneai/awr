//! An authenticated but elapsed Agent delegation must not trigger a login loop.
#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/oauth_http.rs"]
mod web;

use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
};
use awr_team_pg::{AuthorizationStore, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[tokio::test]
async fn expired_delegation_is_not_an_oauth_login_failure() {
    let (_guard, admin, db, _store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
               VALUES('reader-tenant','reader-project','alice','Alice','active');
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
               VALUES('reader-tenant','reader-project','bind-agent','alice','agent','active');",
        )
        .await
        .unwrap();
    let grants =
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    grants
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "expired-discovery-grant".into(),
                authorization: AgentAuthorization {
                    id: "elapsed-agent-read".into(),
                    authorizer_person_id: PersonId::new("alice").unwrap(),
                    responsible_person_id: PersonId::new("alice").unwrap(),
                    subject_kind: ExecutionSubjectKind::Agent,
                    subject_id: "agent".into(),
                    client_id: "cli-a".into(),
                    session_id: None,
                    model_id: None,
                    scope: AuthorizationScope::Project {
                        project_id: PROJECT.into(),
                    },
                    actions: BTreeSet::from([AuthorizedAction::Inspect]),
                    expires_at_ms: Some(2_000),
                    status: AuthorizationStatus::Active,
                    revoked_at_ms: None,
                    revoked_by: None,
                    verifiable_capabilities: vec![],
                    self_reported_skill_hints: vec![],
                    parent_authorization_id: None,
                    maintainer_person_id: None,
                    created_at_ms: 1_000,
                    binding_id: Some("bind-agent".into()),
                },
            },
        )
        .await
        .unwrap();

    for oauth_enabled in [false, true] {
        let server = web::start(
            WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db)),
            oauth_enabled,
        )
        .await;
        for (path, body) in [
            (
                "/v1/projects/one/query",
                json!({"protocol_version":1,"op":"work.next"}),
            ),
            (
                "/v1/projects/one/mcp",
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                    "protocolVersion":"2025-03-26","capabilities":{},
                    "clientInfo":{"name":"ordinary-agent","version":"1"}}}),
            ),
        ] {
            let response = server
                .post(path)
                .bearer_auth(A)
                .header("accept", "application/json, text/event-stream")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 403, "{path}, oauth={oauth_enabled}");
            assert!(response.headers().get("www-authenticate").is_none());
            let data: Value = response.json().await.unwrap();
            assert_eq!(data["code"], "AgentAuthorizationExpired");
            assert!(data["next_step"].as_str().unwrap().contains("renew"));
            assert!(data["recheck_when"].as_str().unwrap().contains("renewal"));
            let text = serde_json::to_string(&data).unwrap();
            assert!(text.len() < 1_024);
            for private in [A, "alice", "elapsed-agent-read", "bind-agent"] {
                assert!(!text.contains(private));
            }
        }
        admin
            .batch_execute(
                "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            )
            .await
            .unwrap();
        let refused = server
            .post("/v1/projects/one/mcp")
            .bearer_auth(A)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), if oauth_enabled { 401 } else { 403 });
        assert_eq!(
            refused.headers().get("www-authenticate").is_some(),
            oauth_enabled
        );
        let refused: Value = refused.json().await.unwrap();
        assert_ne!(refused["code"], "AgentAuthorizationExpired");
        assert!(refused.get("evidence").is_none());
        admin
            .batch_execute("UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'")
            .await
            .unwrap();
        server.shutdown().await;
    }
}
