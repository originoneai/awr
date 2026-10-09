#![cfg(feature = "pg-tests")]

mod common;

use awr_team_pg::{CommandRequest, PgError, TeamStore, check_schema, migrate};
use serde_json::json;
use std::sync::MutexGuard;
use tokio_postgres::{Client, Config};

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";
const OTHER_PROJECT: &str = "project-b";
const ACTOR: &str = "actor-a";
const CLIENT: &str = "client-a";
const SCOPE: &str = "main";
const WORK: &str = "work-a";

/// Uses the shared hardened fixture: a process-private database on a loopback
/// target named by `AWR_TEAM_TEST_DATABASE_URL`. This suite used to read the
/// runtime `AWR_TEAM_DATABASE_URL` and drop `awr_team` in whatever database it
/// named, so a shell that had the runtime URL exported lost that schema.
async fn setup() -> (MutexGuard<'static, ()>, Client, Config) {
    let (guard, admin, db) = common::fresh_team_schema().await;
    seed(&admin).await;
    (
        guard,
        admin,
        common::with_app_role(&common::test_config(), &db),
    )
}

async fn seed(admin: &Client) {
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES ('tenant-a','actor-a','agent','A','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status) VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status) VALUES ('tenant-a','project-b','beta','team','epoch-1','active');
             INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status) VALUES ('tenant-a','project-a','main','main','active');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES ('tenant-a','project-a','work-a','W');",
        )
        .await
        .unwrap();
}

fn touch(request_id: &str) -> CommandRequest {
    CommandRequest {
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        actor_id: ACTOR.into(),
        client_id: CLIENT.into(),
        request_id: request_id.into(),
        op: "work.touch".into(),
        args: json!({"work_id": WORK, "scope_id": SCOPE}),
    }
}

#[tokio::test]
async fn migrate_is_idempotent_when_schema_already_matches() {
    let (_lock, admin, _) = setup().await;
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
}

#[tokio::test]
async fn clean_migration_and_incompatible_schema_refuses_start() {
    let (_lock, admin, _) = setup().await;
    check_schema(&admin).await.unwrap();
    admin
        .execute(
            "UPDATE awr_team.schema_state SET version=99 WHERE component='awr_team'",
            &[],
        )
        .await
        .unwrap();
    let err = check_schema(&admin).await.unwrap_err();
    assert!(err.to_string().contains("schema version 99"));
}

#[tokio::test]
async fn app_role_is_not_owner_and_has_no_bypassrls() {
    let (_lock, admin, _) = setup().await;
    let bypass: bool = admin
        .query_one(
            "SELECT rolbypassrls FROM pg_roles WHERE rolname='awr_app'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!bypass);
    let superuser: bool = admin
        .query_one("SELECT rolsuper FROM pg_roles WHERE rolname='awr_app'", &[])
        .await
        .unwrap()
        .get(0);
    assert!(!superuser);
    let owner: String = admin
        .query_one(
            "SELECT pg_get_userbyid(relowner) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='awr_team' AND c.relname='events'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_ne!(owner, "awr_app");
}

#[tokio::test]
async fn domain_transaction_rolls_back_partial_writes() {
    let (_lock, admin, app) = setup().await;
    let store = TeamStore::from_config(app);
    store
        .abort_after_partial_write(touch("partial"))
        .await
        .unwrap();
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.events", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
}

#[tokio::test]
async fn identical_request_replays_without_a_second_event() {
    let (_lock, admin, app) = setup().await;
    let store = TeamStore::from_config(app);
    let first = store.execute(touch("r1")).await.unwrap();
    assert!(!first.replayed);
    let second = store.execute(touch("r1")).await.unwrap();
    assert!(second.replayed);
    assert_eq!(
        first.committed_project_revision,
        second.committed_project_revision
    );
    let events: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.events", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(events, 1);
}

#[tokio::test]
async fn same_request_id_with_new_args_conflicts() {
    let (_lock, _, app) = setup().await;
    let store = TeamStore::from_config(app);
    store.execute(touch("r2")).await.unwrap();
    let mut changed = touch("r2");
    changed.args = json!({"work_id": WORK, "scope_id": SCOPE, "extra": true});
    let err = store.execute(changed).await.unwrap_err();
    assert!(matches!(err, PgError::IdempotencyConflict));
}

#[tokio::test]
async fn rls_hides_other_projects_and_local_config_does_not_leak() {
    let (_lock, _, config) = setup().await;
    let mut app = common::connect_config(&config).await;
    let tx = app.transaction().await.unwrap();
    tx.execute("SELECT set_config('awr.tenant_id', $1, true)", &[&TENANT])
        .await
        .unwrap();
    tx.execute("SELECT set_config('awr.project_id', $1, true)", &[&PROJECT])
        .await
        .unwrap();
    let visible: i64 = tx
        .query_one("SELECT count(*) FROM awr_team.projects", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(visible, 1);
    tx.commit().await.unwrap();
    let leaked: i64 = app
        .query_one("SELECT count(*) FROM awr_team.projects", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(leaked, 0);
    let other = app
        .execute(
            "INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES ($1,$2,'x','X')",
            &[&TENANT, &OTHER_PROJECT],
        )
        .await;
    assert!(other.is_err());
}

#[tokio::test]
async fn events_are_append_only_for_the_app_role() {
    let (_lock, _, config) = setup().await;
    let store = TeamStore::from_config(config.clone());
    store.execute(touch("r3")).await.unwrap();
    let app = common::connect_config(&config).await;
    let update = app
        .execute("UPDATE awr_team.events SET event_type='tamper'", &[])
        .await;
    assert!(update.is_err());
}

#[tokio::test]
async fn concurrent_commands_serialize_project_revisions() {
    let (_lock, _, app) = setup().await;
    let store = TeamStore::from_config(app);
    let a = store.execute(touch("c1"));
    let b = store.execute(touch("c2"));
    let (ra, rb) = tokio::join!(a, b);
    let mut revs = [
        ra.unwrap()
            .committed_project_revision
            .parse::<i64>()
            .unwrap(),
        rb.unwrap()
            .committed_project_revision
            .parse::<i64>()
            .unwrap(),
    ];
    revs.sort();
    assert_eq!(revs, [1, 2]);
}
