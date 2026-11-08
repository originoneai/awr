#![cfg(feature = "pg-tests")]
//! Synthetic route mechanisms; these tests do not establish native acceptance.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team::delivery::FactSource;
use awr_team_pg::*;
use fixture::*;
use serde_json::{Value, json};
use std::sync::MutexGuard;
use tokio_postgres::Client;

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    admin: Client,
    reads: WorkstreamReadStore,
    config: tokio_postgres::Config,
}

async fn setup_routes() -> Fixture {
    let (guard, admin, db, reads) = setup_with_assigned_tasks().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
        VALUES('reader-tenant','route-worker','system','Synthetic delivery worker','active')",
        )
        .await
        .unwrap();
    let config = common::with_app_role(&common::test_config(), &db);
    let p = prepare(&reads, A, "a").await;
    let set = serde_json::from_value(json!({
        "work_id":"a","workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],
        "authority_version":p["authority_version"],"ownership_version":p["data"]["ownership_version"],
        "contract_hash":p["data"]["contract_hash"]
    })).unwrap();
    DeliverySyncStore::from_config(config.clone())
        .configure_connector(
            TENANT,
            PROJECT,
            A,
            ConfigureDeliveryConnector {
                request_id: "seed-route-connector".into(),
                read_set: set,
                expected_connector_version: "0".into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "repo-source".into(),
                    provider: "neutral-git".into(),
                    resource: "fixture://shared-repository".into(),
                    principal_actor_id: "route-worker".into(),
                    principal_client_id: "route-worker-client".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled: true,
                    source_locator: Some(DeliverySourceLocatorUpdate::Set {
                        locator: DeliverySourceLocator {
                            contract: "awr-git-source-v1".into(),
                            url: "https://git.example.invalid/team/project.git".into(),
                        },
                    }),
                },
            },
        )
        .await
        .unwrap();
    Fixture {
        _guard: guard,
        admin,
        reads,
        config,
    }
}

fn configure(
    p: &Value,
    request: &str,
    id: &str,
    expected: &str,
    enabled: bool,
) -> WorkstreamCommand {
    command(
        p,
        request,
        "delivery.route.configure",
        json!({
            "source_snapshot_id":p["source_snapshot_id"],"template_id":id,
            "expected_template_version":expected,"source_connector_id":"repo-source",
            "source_connector_version":"1","enabled":enabled
        }),
    )
}

async fn routes(f: &Fixture, token: &str, work: &str) -> Result<Value, PgError> {
    let q =
        serde_json::from_value(json!({"protocol_version":1,"op":"delivery.routes","work_id":work}))
            .unwrap();
    f.reads.query(TENANT, PROJECT, token, q).await
}

async fn publish_route(f: &Fixture) {
    let p = prepare(&f.reads, A, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "publish-template", "repository", "0", true),
        )
        .await
        .unwrap();
}

fn reuse_route(p: &Value, request: &str, connector: &str) -> WorkstreamCommand {
    command(
        p,
        request,
        "delivery.route.reuse",
        json!({
            "source_snapshot_id":p["source_snapshot_id"],"template_id":"repository",
            "template_version":"1","source_connector_version":"1","connector_id":connector
        }),
    )
}

async fn supervisor(f: &Fixture) {
    f.admin.batch_execute("UPDATE awr_team.project_memberships SET business_roles='[\"supervisor\"]',assignment_grant=true
        WHERE actor_id='agent'; UPDATE awr_team.workstream_grants SET can_manage=false WHERE client_id='cli-a'")
        .await.unwrap();
}

#[tokio::test]
async fn delegated_supervisor_can_reuse_but_a_revoked_delegation_cannot_replay() {
    let f = setup_routes().await;
    publish_route(&f).await;
    supervisor(&f).await;
    f.admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','reader-project','supervisor-person','Supervisor','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
        VALUES('reader-tenant','reader-project','supervisor-binding','supervisor-person','agent','active')")
        .await.unwrap();
    let grant = awr_core::AgentAuthorization {
        id: "route-delegation".into(),
        authorizer_person_id: awr_core::PersonId::new("supervisor-person").unwrap(),
        responsible_person_id: awr_core::PersonId::new("supervisor-person").unwrap(),
        subject_kind: awr_core::ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: awr_core::AuthorizationScope::Workstream {
            project_id: PROJECT.into(),
            workstream_id: awr_core::Id::from(1).to_string(),
        },
        actions: std::collections::BTreeSet::from([
            awr_core::AuthorizedAction::Inspect,
            awr_core::AuthorizedAction::AssignWork,
        ]),
        expires_at_ms: None,
        status: awr_core::AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1000,
        binding_id: Some("supervisor-binding".into()),
    };
    AuthorizationStore::from_config(f.config.clone())
        .issue(
            TENANT,
            PROJECT,
            &awr_core::IssueAuthorizationRequest {
                request_key: "issue-supervisor-route-delegation".into(),
                authorization: grant,
            },
        )
        .await
        .unwrap();
    let p = prepare(&f.reads, A, "aa-assigned").await;
    let original = reuse_route(&p, "delegated-reuse", "target");
    f.reads
        .commands()
        .execute(TENANT, PROJECT, A, original.clone())
        .await
        .unwrap();
    f.admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='route-delegation'",
        )
        .await
        .unwrap();
    assert!(matches!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, A, original)
            .await,
        Err(PgError::Forbidden)
    ));
    let count: i64 = f
        .admin
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_connectors WHERE work_id='aa-assigned'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn route_reuse_rechecks_assignment_authority_and_target_workstream() {
    let f = setup_routes().await;
    publish_route(&f).await;
    let p = prepare(&f.reads, A, "aa-assigned").await;
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                reuse_route(&p, "without-assignment", "target")
            )
            .await,
        Err(PgError::Forbidden)
    ));
    supervisor(&f).await;
    f.admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE client_id='cli-b'",
        )
        .await
        .unwrap();
    let other = prepare(&f.reads, B, "b-private").await;
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                B,
                reuse_route(&other, "wrong-stream", "target")
            )
            .await,
        Err(PgError::Forbidden)
    ));
    f.admin.batch_execute("UPDATE awr_team.project_memberships SET business_roles='[\"developer\"]' WHERE actor_id='agent'").await.unwrap();
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                reuse_route(&p, "developer-selection", "target")
            )
            .await,
        Err(PgError::Forbidden)
    ));
    let count: i64 = f
        .admin
        .query_one("SELECT count(*) FROM awr_team.delivery_connectors", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn reuse_replay_preserves_original_mapping_after_template_revocation() {
    let f = setup_routes().await;
    publish_route(&f).await;
    supervisor(&f).await;
    let p = prepare(&f.reads, A, "aa-assigned").await;
    let original = reuse_route(&p, "stable-reuse", "target");
    let first = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, A, original.clone())
        .await
        .unwrap();
    f.admin
        .batch_execute(
            "UPDATE awr_team.delivery_route_templates SET enabled=false,version=version+1",
        )
        .await
        .unwrap();
    let replay = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, A, original.clone())
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(first["receipt"]["data"], replay["receipt"]["data"]);
    let mut changed = original;
    changed.args["connector_id"] = json!("another");
    assert!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, A, changed)
            .await
            .is_err()
    );
    let q = serde_json::from_value(json!({"protocol_version":1,"op":"delivery.neutral.outcome",
        "work_id":"aa-assigned","request_id":"stable-reuse"}))
    .unwrap();
    let outcome = f.reads.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(outcome["data"]["outcome"], "committed");
    assert_eq!(outcome["data"]["receipt"]["data"], first["receipt"]["data"]);
    let p2 = prepare(&f.reads, A, "ab-assigned").await;
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                reuse_route(&p2, "revoked-template", "new-target")
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let count: i64 = f.admin.query_one("SELECT count(*) FROM awr_team.delivery_connectors WHERE work_id IN ('aa-assigned','ab-assigned')", &[])
        .await.unwrap().get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn competing_route_selections_create_exactly_one_independent_mapping() {
    let f = setup_routes().await;
    publish_route(&f).await;
    supervisor(&f).await;
    let p = prepare(&f.reads, A, "aa-assigned").await;
    let commands = f.reads.commands();
    let (left, right) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, reuse_route(&p, "reuse-left", "left")),
        commands.execute(TENANT, PROJECT, A, reuse_route(&p, "reuse-right", "right"))
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(matches!(
        if left.is_err() { left } else { right },
        Err(PgError::PreconditionsChanged)
    ));
    let count: i64 = f
        .admin
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_connectors WHERE work_id='aa-assigned'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn route_selection_refuses_changed_versions_and_current_source_authority() {
    let f = setup_routes().await;
    publish_route(&f).await;
    supervisor(&f).await;
    let p = prepare(&f.reads, A, "aa-assigned").await;
    let mut stale = reuse_route(&p, "stale-template", "target");
    stale.args["template_version"] = json!("2");
    assert!(matches!(
        f.reads.commands().execute(TENANT, PROJECT, A, stale).await,
        Err(PgError::PreconditionsChanged)
    ));
    for (index, mutation) in [
        "UPDATE awr_team.delivery_connectors SET version=2 WHERE id='repo-source'",
        "UPDATE awr_team.delivery_connectors SET enabled=false WHERE id='repo-source'",
        "UPDATE awr_team.delivery_connectors SET coordinator_epoch='old' WHERE id='repo-source'",
        "UPDATE awr_team.actors SET status='disabled' WHERE id='route-worker'",
    ]
    .into_iter()
    .enumerate()
    {
        f.admin.batch_execute(mutation).await.unwrap();
        assert!(matches!(
            f.reads
                .commands()
                .execute(
                    TENANT,
                    PROJECT,
                    A,
                    reuse_route(&p, &format!("stale-source-{index}"), "target")
                )
                .await,
            Err(PgError::PreconditionsChanged)
        ));
        f.admin.batch_execute("UPDATE awr_team.delivery_connectors SET version=1,enabled=true,coordinator_epoch='epoch-a'
            WHERE id='repo-source'; UPDATE awr_team.actors SET status='active' WHERE id='route-worker'").await.unwrap();
    }
    f.admin
        .batch_execute("DELETE FROM awr_team.workstream_snapshot_ownership WHERE work_id='a'")
        .await
        .unwrap();
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                reuse_route(&p, "retired-source", "target")
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let count: i64 = f
        .admin
        .query_one("SELECT count(*) FROM awr_team.delivery_connectors", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn route_selection_preserves_owned_claimed_and_preconfigured_work() {
    let f = setup_routes().await;
    publish_route(&f).await;
    supervisor(&f).await;
    let owned = prepare(&f.reads, A, "aa-assigned").await;
    f.admin.batch_execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','reader-project','owner','Owner','active');
        INSERT INTO awr_team.task_responsibilities(tenant_id,project_id,work_id,owner_person_id,version)
        VALUES('reader-tenant','reader-project','aa-assigned','owner',1)").await.unwrap();
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                reuse_route(&owned, "owned-task", "owned-mapping")
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let busy = prepare(&f.reads, A, "ab-assigned").await;
    f.admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        VALUES('reader-tenant','reader-project','busy-session','main','ab-assigned','agent','cli-a','busy','active',$1,1)",
        &[&busy["workstream_id"].as_str().unwrap()]).await.unwrap();
    f.admin.execute("INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,
        fence,lease_version,expires_at,state,workstream_id,ownership_version,coordinator_epoch)
        VALUES('reader-tenant','reader-project','busy-claim','main','ab-assigned','busy-session','agent',
            1,1,clock_timestamp()+interval '1 minute','active',$1,1,'epoch-a')",
        &[&busy["workstream_id"].as_str().unwrap()]).await.unwrap();
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                reuse_route(&busy, "claimed-task", "claimed-mapping")
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let configured = prepare(&f.reads, A, "a").await;
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                reuse_route(&configured, "existing-task", "replacement")
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let count: i64 = f
        .admin
        .query_one("SELECT count(*) FROM awr_team.delivery_connectors", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn reuse_does_not_rebind_a_retained_locator_to_a_new_connector_version() {
    let f = setup_routes().await;
    publish_route(&f).await;
    f.admin
        .batch_execute(
            "UPDATE awr_team.delivery_connectors SET version=2 WHERE id='repo-source';
        UPDATE awr_team.delivery_route_templates SET version=2,source_connector_version=2",
        )
        .await
        .unwrap();
    supervisor(&f).await;
    let p = prepare(&f.reads, A, "aa-assigned").await;
    let mut request = reuse_route(&p, "stale-locator-copy", "target");
    request.args["template_version"] = json!("2");
    request.args["source_connector_version"] = json!("2");
    f.reads
        .commands()
        .execute(TENANT, PROJECT, A, request)
        .await
        .unwrap();
    let locator = f
        .admin
        .query_one(
            "SELECT source_locator_contract,source_locator_url,source_locator_connector_version
        FROM awr_team.delivery_connectors WHERE id='target'",
            &[],
        )
        .await
        .unwrap();
    assert!(locator.get::<_, Option<String>>(0).is_none());
    assert!(locator.get::<_, Option<String>>(1).is_none());
    assert!(locator.get::<_, Option<i64>>(2).is_none());
}

#[tokio::test]
async fn supervisor_reuses_authorized_route_without_management_or_new_grants() {
    let f = setup_routes().await;
    publish_route(&f).await;
    f.admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET business_roles='[\"supervisor\"]',assignment_grant=true;
        UPDATE awr_team.workstream_grants SET can_manage=false WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let p = prepare(&f.reads, A, "aa-assigned").await;
    let grants: Value = f.admin.query_one("SELECT jsonb_agg(to_jsonb(g) ORDER BY client_id,workstream_id) FROM awr_team.workstream_grants g", &[])
        .await.unwrap().get(0);
    let result = f
        .reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            reuse_route(&p, "new-task-route", "task-repository"),
        )
        .await
        .expect("supervisors can select an administrator-authorized same-stream route");
    assert_eq!(result["receipt"]["data"]["connector_version"], "1");
    assert_eq!(result["receipt"]["data"]["grants_created"], false);
    let mapping = f.admin.query_one("SELECT work_id,provider,resource,principal_actor_id,principal_client_id,
        source_locator_connector_version,version FROM awr_team.delivery_connectors WHERE id='task-repository'", &[])
        .await.unwrap();
    assert_eq!(mapping.get::<_, String>(0), "aa-assigned");
    assert_eq!(mapping.get::<_, String>(1), "neutral-git");
    assert_eq!(mapping.get::<_, String>(2), "fixture://shared-repository");
    assert_eq!(mapping.get::<_, String>(3), "route-worker");
    assert_eq!(mapping.get::<_, String>(4), "route-worker-client");
    assert_eq!(mapping.get::<_, Option<i64>>(5), Some(1));
    assert_eq!(mapping.get::<_, i64>(6), 1);
    let after: Value = f.admin.query_one("SELECT jsonb_agg(to_jsonb(g) ORDER BY client_id,workstream_id) FROM awr_team.workstream_grants g", &[])
        .await.unwrap().get(0);
    assert_eq!(grants, after);
    assert!(!result.to_string().contains("route-worker-client"));
}

#[tokio::test]
async fn administrative_template_is_discoverable_only_in_its_workstream() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    let result = f
        .reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "publish-route", "team-repository", "0", true),
        )
        .await
        .expect("an administrator can publish a reusable version-bound route");
    assert_eq!(result["receipt"]["data"]["template_version"], "1");
    let listed = routes(&f, A, "a").await.unwrap();
    let items = listed["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["template_id"], "team-repository");
    assert_eq!(items[0]["template_version"], "1");
    assert_eq!(items[0]["resource"], "fixture://shared-repository");
    assert_eq!(
        items[0]["source_locator"]["url"],
        "https://git.example.invalid/team/project.git"
    );
    for private in [
        "route-worker-client",
        "principal_actor_id",
        "principal_client_id",
        A,
    ] {
        assert!(!listed.to_string().contains(private));
    }
    assert!(
        routes(&f, B, "b-private").await.unwrap()["data"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(routes(&f, B, "a").await.is_err());
    assert!(routes(&f, NONE, "a").await.is_err());
}

#[tokio::test]
async fn replay_does_not_restore_a_disabled_template_and_changed_intent_conflicts() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    let original = configure(&p, "route-original", "repo", "0", true);
    let first = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, A, original.clone())
        .await
        .unwrap();
    let disabled = f
        .reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "route-disable", "repo", "1", false),
        )
        .await
        .unwrap();
    assert_eq!(disabled["receipt"]["data"]["template_version"], "2");
    let replay = f
        .reads
        .commands()
        .execute(TENANT, PROJECT, A, original.clone())
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(first["receipt"]["data"], replay["receipt"]["data"]);
    assert!(
        routes(&f, A, "a").await.unwrap()["data"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut changed = original;
    changed.args["enabled"] = json!(false);
    assert!(matches!(
        f.reads
            .commands()
            .execute(TENANT, PROJECT, A, changed)
            .await,
        Err(PgError::IdempotencyConflict)
    ));
    let q = serde_json::from_value(json!({"protocol_version":1,"op":"delivery.neutral.outcome",
        "work_id":"a","request_id":"route-original"}))
    .unwrap();
    let outcome = f.reads.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(outcome["data"]["outcome"], "committed");
    assert_eq!(outcome["data"]["receipt"]["data"]["template_version"], "1");
}

#[tokio::test]
async fn retained_source_locator_is_not_disclosed_for_a_different_connector_version() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    // Connector updates may intentionally retain a locator bound to an older version.
    f.admin
        .execute(
            "UPDATE awr_team.delivery_connectors SET version=2 WHERE id='repo-source'",
            &[],
        )
        .await
        .unwrap();
    let mut publish = configure(&p, "publish-new-version", "repo", "0", true);
    publish.args["source_connector_version"] = json!("2");
    f.reads
        .commands()
        .execute(TENANT, PROJECT, A, publish)
        .await
        .unwrap();
    let listed = routes(&f, A, "a").await.unwrap();
    assert_eq!(listed["data"]["items"].as_array().unwrap().len(), 1);
    assert!(listed["data"]["items"][0]["source_locator"].is_null());
}

#[tokio::test]
async fn stale_or_disabled_connectors_are_hidden_but_their_template_can_be_disabled() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "seed-template", "repo", "0", true),
        )
        .await
        .unwrap();
    f.admin
        .batch_execute(
            "UPDATE awr_team.delivery_connectors SET version=2,enabled=false
        WHERE id='repo-source'",
        )
        .await
        .unwrap();
    assert!(
        routes(&f, A, "a").await.unwrap()["data"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                configure(&p, "stale-enable", "second", "0", true)
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let disabled = f
        .reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "disable-stale", "repo", "1", false),
        )
        .await
        .unwrap();
    assert_eq!(disabled["receipt"]["data"]["enabled"], false);
    let count: i64 = f
        .admin
        .query_one("SELECT count(*) FROM awr_team.delivery_connectors", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        count, 1,
        "template maintenance must not create task connectors"
    );
}

#[tokio::test]
async fn non_administrators_cannot_maintain_templates_but_retain_scoped_discovery() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "seed-template", "repo", "0", true),
        )
        .await
        .unwrap();
    f.admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer' WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    assert_eq!(
        routes(&f, A, "a").await.unwrap()["data"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                configure(&p, "developer-disable", "repo", "1", false)
            )
            .await,
        Err(PgError::Forbidden)
    ));
    let unchanged: bool = f
        .admin
        .query_one(
            "SELECT enabled FROM awr_team.delivery_route_templates WHERE id='repo'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(unchanged);
}

#[tokio::test]
async fn competing_updates_keep_one_version_and_reject_the_other() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "seed-template", "repo", "0", true),
        )
        .await
        .unwrap();
    let left = configure(&p, "left-update", "repo", "1", false);
    let right = configure(&p, "right-update", "repo", "1", true);
    let commands = f.reads.commands();
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, left),
        commands.execute(TENANT, PROJECT, A, right),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let refused = if a.is_err() { a } else { b };
    assert!(matches!(refused, Err(PgError::PreconditionsChanged)));
    let version: i64 = f
        .admin
        .query_one(
            "SELECT version FROM awr_team.delivery_route_templates WHERE id='repo'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(version, 2);
}

#[tokio::test]
async fn competing_first_publications_serialize_before_the_template_row_exists() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    let commands = f.reads.commands();
    let (left, right) = tokio::join!(
        commands.execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "first-left", "new-template", "0", true)
        ),
        commands.execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "first-right", "new-template", "0", true)
        ),
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let refused = if left.is_err() { left } else { right };
    assert!(matches!(refused, Err(PgError::PreconditionsChanged)));
    commands
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "disable-created", "new-template", "1", false),
        )
        .await
        .unwrap();
    assert!(matches!(
        commands
            .execute(
                TENANT,
                PROJECT,
                A,
                configure(&p, "late-first", "new-template", "0", true)
            )
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    let stored = f
        .admin
        .query_one(
            "SELECT version,enabled FROM awr_team.delivery_route_templates WHERE id='new-template'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(stored.get::<_, i64>(0), 2);
    assert!(!stored.get::<_, bool>(1));
}

#[tokio::test]
async fn discovery_pages_are_bound_to_the_callers_work_scope() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    for id in ["first", "second"] {
        f.reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                configure(&p, &format!("seed-{id}"), id, "0", true),
            )
            .await
            .unwrap();
    }
    let mut q: WorkstreamQuery = serde_json::from_value(json!({"protocol_version":1,
        "op":"delivery.routes","work_id":"a","limit":1}))
    .unwrap();
    let page = f.reads.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(page["data"]["items"][0]["template_id"], "first");
    q.cursor = Some(page["data"]["next_cursor"].as_str().unwrap().into());
    let next = f.reads.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(next["data"]["items"][0]["template_id"], "second");
    assert!(next["data"]["next_cursor"].is_null());
    q.work_id = Some("b".into());
    assert!(f.reads.query(TENANT, PROJECT, B, q).await.is_err());
}

#[tokio::test]
async fn route_table_forces_tenant_and_project_isolation() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "seed-template", "repo", "0", true),
        )
        .await
        .unwrap();
    let mut app = common::connect_config(&f.config).await;
    let insert_allowed: bool = app
        .query_one(
            "SELECT has_table_privilege(current_user,'awr_team.delivery_route_templates','INSERT')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(
        insert_allowed,
        "the RLS write test must not fail at the table privilege gate"
    );
    let count: i64 = app
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_route_templates",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
    let tx = app.transaction().await.unwrap();
    tx.batch_execute(
        "SET LOCAL awr.tenant_id='reader-tenant'; SET LOCAL awr.project_id='other-project'",
    )
    .await
    .unwrap();
    let count: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_route_templates",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
    let denied = tx.execute("INSERT INTO awr_team.delivery_route_templates
        (tenant_id,project_id,id,workstream_id,source_connector_id,source_connector_version,
         version,coordinator_epoch,enabled,configured_by_actor_id)
        VALUES('reader-tenant','reader-project','injected',$1,'repo-source',1,1,'epoch-a',true,'agent')",
        &[&p["workstream_id"].as_str().unwrap()]).await.unwrap_err();
    assert_eq!(
        denied.as_db_error().unwrap().code(),
        &tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE
    );
    tx.rollback().await.unwrap();
    let count: i64 = f
        .admin
        .query_one(
            "SELECT count(*) FROM awr_team.delivery_route_templates",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn discovery_rechecks_source_epoch_actor_and_observation_kind() {
    let f = setup_routes().await;
    let p = prepare(&f.reads, A, "a").await;
    f.reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            configure(&p, "seed-template", "repo", "0", true),
        )
        .await
        .unwrap();
    for (index, mutation) in [
        "UPDATE awr_team.delivery_connectors SET coordinator_epoch='old-epoch' WHERE id='repo-source'",
        "UPDATE awr_team.actors SET status='disabled' WHERE id='route-worker'",
        "UPDATE awr_team.delivery_connectors SET fact_source='operator_recorded' WHERE id='repo-source'",
    ].into_iter().enumerate() {
        f.admin.batch_execute(mutation).await.unwrap();
        assert!(routes(&f, A, "a").await.unwrap()["data"]["items"].as_array().unwrap().is_empty());
        assert!(matches!(f.reads.commands().execute(TENANT, PROJECT, A,
            configure(&p, &format!("invalid-source-{index}"), "other", "0", true)).await,
            Err(PgError::PreconditionsChanged)));
        f.admin.batch_execute("UPDATE awr_team.delivery_connectors SET coordinator_epoch='epoch-a',fact_source='adapter_observation'
            WHERE id='repo-source'; UPDATE awr_team.actors SET status='active' WHERE id='route-worker'").await.unwrap();
    }
    assert_eq!(
        routes(&f, A, "a").await.unwrap()["data"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
