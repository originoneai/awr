#![cfg(feature = "pg-tests")]
mod common;
use awr_team_pg::{EXPECTED_SCHEMA_VERSION, check_schema, migrate};
use serde_json::{Value, json};

#[tokio::test]
async fn schema42_delivery_upgrade_is_atomic_preserves_legacy_rows_and_rejects_other_versions() {
    let (_g, admin, _) = common::historical_team_schema(42).await;
    assert!(matches!(
        check_schema(&admin).await,
        Err(awr_team_pg::PgError::SchemaIncompatible(_))
    ));
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
        VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work')").await.unwrap();
    let before: Value = admin
        .query_one("SELECT to_jsonb(w) FROM awr_team.work_items w", &[])
        .await
        .unwrap()
        .get(0);
    let ddl = include_str!("../migrations/20261006000043_delivery_sync.sql");
    assert!(
        admin
            .batch_execute(&ddl.replace(
                "UPDATE awr_team.schema_state",
                "SELECT 1/0; UPDATE awr_team.schema_state"
            ))
            .await
            .is_err()
    );
    admin.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(
        admin
            .query_one("SELECT version FROM awr_team.schema_state", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        42
    );
    assert!(
        admin
            .query_one(
                "SELECT to_regclass('awr_team.delivery_connectors')::text",
                &[]
            )
            .await
            .unwrap()
            .get::<_, Option<String>>(0)
            .is_none()
    );
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    let after: Value = admin
        .query_one("SELECT to_jsonb(w) FROM awr_team.work_items w", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(after, before);
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.delivery_candidates", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.delivery_inbox", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    migrate(&admin).await.unwrap();
    admin
        .execute(
            "UPDATE awr_team.schema_state SET version=$1",
            &[&(EXPECTED_SCHEMA_VERSION + 1)],
        )
        .await
        .unwrap();
    assert!(matches!(
        check_schema(&admin).await,
        Err(awr_team_pg::PgError::SchemaIncompatible(_))
    ));
    assert!(matches!(
        migrate(&admin).await,
        Err(awr_team_pg::PgError::SchemaIncompatible(_))
    ));
}

#[tokio::test]
async fn schema41_recovery_upgrade_does_not_infer_old_causes_and_rolls_back_atomically() {
    let (_g, admin, _) = common::historical_team_schema(41).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
        VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
        VALUES('upgrade-tenant','upgrade-project','main','Main','active');
        INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,recovery_blocked)
        VALUES('upgrade-tenant','upgrade-project','main','legacy-work','in_progress',true)").await.unwrap();
    let before: Value = admin
        .query_one("SELECT to_jsonb(w) FROM awr_team.work_runtime w", &[])
        .await
        .unwrap()
        .get(0);
    let ddl = include_str!("../migrations/20261005000042_execution_recovery_cause.sql");
    assert!(
        admin
            .batch_execute(&ddl.replace(
                "UPDATE awr_team.schema_state",
                "SELECT 1/0; UPDATE awr_team.schema_state"
            ))
            .await
            .is_err()
    );
    admin.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(
        admin
            .query_one("SELECT version FROM awr_team.schema_state", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        41
    );
    let unchanged: Value = admin
        .query_one("SELECT to_jsonb(w) FROM awr_team.work_runtime w", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(unchanged, before);
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    let after: Value = admin
        .query_one("SELECT to_jsonb(w) FROM awr_team.work_runtime w", &[])
        .await
        .unwrap()
        .get(0);
    let mut preserved = after.clone();
    for field in ["recovery_execution_id", "recovery_receipt_id"] {
        assert!(preserved[field].is_null());
        preserved.as_object_mut().unwrap().remove(field);
    }
    assert_eq!(preserved, before);
    migrate(&admin).await.unwrap();
    let repeated: Value = admin
        .query_one("SELECT to_jsonb(w) FROM awr_team.work_runtime w", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(repeated, after);
    assert!(admin.batch_execute("UPDATE awr_team.work_runtime SET recovery_execution_id='unbound' WHERE work_id='legacy-work'").await.is_err());
}

#[tokio::test]
async fn schema40_upgrade_preserves_legacy_provenance_and_is_atomic_and_repeatable() {
    let (_g, admin, _) = common::historical_team_schema(40).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
        VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
        VALUES('upgrade-tenant','upgrade-project','legacy-run','legacy-work',9,'old-contract','old-executor','unknown')").await.unwrap();
    let before: Value = admin
        .query_one(
            "SELECT to_jsonb(e) FROM awr_team.executions e WHERE id='legacy-run'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let ddl = include_str!("../migrations/20261005000041_execution_settlement.sql");
    assert!(
        admin
            .batch_execute(&ddl.replace(
                "UPDATE awr_team.schema_state",
                "SELECT 1/0; UPDATE awr_team.schema_state"
            ))
            .await
            .is_err()
    );
    admin.batch_execute("ROLLBACK").await.unwrap();
    let row=admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),
        (SELECT count(*) FROM information_schema.columns WHERE table_schema='awr_team' AND table_name='executions' AND column_name='settlement_policy_json')",&[]).await.unwrap();
    assert_eq!(row.get::<_, i32>(0), 40);
    assert_eq!(row.get::<_, i64>(1), 0);
    let unchanged: Value = admin
        .query_one(
            "SELECT to_jsonb(e) FROM awr_team.executions e WHERE id='legacy-run'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, unchanged);
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    assert_eq!(EXPECTED_SCHEMA_VERSION, 43);
    let after: Value = admin
        .query_one(
            "SELECT to_jsonb(e) FROM awr_team.executions e WHERE id='legacy-run'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let mut preserved = after.clone();
    for field in [
        "settlement_policy_json",
        "admission_mode",
        "admission_lease_version",
    ] {
        assert!(preserved[field].is_null());
        preserved.as_object_mut().unwrap().remove(field);
    }
    for field in ["terminal_reported", "workspace_effects_settled"] {
        assert_eq!(preserved[field], false);
        preserved.as_object_mut().unwrap().remove(field);
    }
    assert_eq!(preserved, before);
    migrate(&admin).await.unwrap();
    let repeated: Value = admin
        .query_one(
            "SELECT to_jsonb(e) FROM awr_team.executions e WHERE id='legacy-run'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(repeated, after);
    for policy in [
        json!({}),
        json!({"mode":"independent_workspace_v1"}),
        json!({"mode":"independent_workspace_v1","workspace_id":"../unsafe"}),
        json!({"mode":"independent_workspace_v1","workspace_id":"clone-a","trusted_executor":true}),
        Value::Null,
    ] {
        assert!(admin.execute("UPDATE awr_team.executions SET settlement_policy_json=$1 WHERE id='legacy-run'",&[&policy]).await.is_err());
    }
    assert!(admin.batch_execute("UPDATE awr_team.executions SET workspace_effects_settled=true WHERE id='legacy-run'").await.is_err());
    assert!(admin.batch_execute("UPDATE awr_team.executions SET admission_mode='caller_managed' WHERE id='legacy-run'").await.is_err());
}
