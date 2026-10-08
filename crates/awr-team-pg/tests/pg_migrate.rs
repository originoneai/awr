#![cfg(feature = "pg-tests")]
mod common;
use awr_team_pg::{EXPECTED_SCHEMA_VERSION, check_schema, migrate};
use serde_json::{Value, json};

#[tokio::test]
async fn schema51_artifact_adoptions_upgrade_atomically_preserve_exports_and_enforce_rls() {
    let (_g, admin, db) = common::historical_team_schema(51).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
            VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
            dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,independence_kind)
            VALUES('upgrade-tenant','upgrade-project','legacy-receipt','legacy-work','main','old-contract','old-result',
                'old-dependencies','old-bundle','ordinary_confirm','{}','unspecified');
        INSERT INTO awr_team.artifacts(tenant_id,project_id,id,object_key,sha256,byte_length,media_type,state,created_by)
            VALUES('upgrade-tenant','upgrade-project','legacy-artifact','fixture',repeat('a',64),0,'text/plain','finalized','fixture-author');
        INSERT INTO awr_team.workstream_artifact_exports(tenant_id,project_id,id,provider_work_id,consumer_work_id,receipt_id,artifact_id,
            disclosure_sha256,manifest_json,proof_json,published_by_actor_id,published_by_client_id)
            VALUES('upgrade-tenant','upgrade-project','legacy-export','legacy-work','consumer','legacy-receipt','legacy-artifact',
                repeat('b',64),'{\"historical\":true}','{\"original\":true}','publisher','client')").await.unwrap();
    let original: Value = admin
        .query_one(
            "SELECT to_jsonb(e) FROM awr_team.workstream_artifact_exports e",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let ddl = include_str!("../migrations/20261008000052_workstream_artifact_adoptions.sql");
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
    let rollback = admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),to_regclass('awr_team.workstream_artifact_adoptions')::text", &[]).await.unwrap();
    assert_eq!(rollback.get::<_, i32>(0), 51);
    assert!(rollback.get::<_, Option<String>>(1).is_none());
    migrate(&admin).await.unwrap();
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    assert_eq!(
        admin
            .query_one(
                "SELECT to_jsonb(e) FROM awr_team.workstream_artifact_exports e",
                &[]
            )
            .await
            .unwrap()
            .get::<_, Value>(0),
        original
    );
    assert_eq!(
        admin
            .query_one(
                "SELECT count(*) FROM awr_team.workstream_artifact_adoptions",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0,
        "Upgrade must not adopt an old export or fabricate consumer consent"
    );
    let rls = admin.query_one("SELECT relrowsecurity,relforcerowsecurity FROM pg_class WHERE oid='awr_team.workstream_artifact_adoptions'::regclass", &[]).await.unwrap();
    assert!(rls.get::<_, bool>(0) && rls.get::<_, bool>(1));
    let insert = "INSERT INTO awr_team.workstream_artifact_adoptions(tenant_id,project_id,id,consumer_work_id,provider_work_id,export_id,
        export_version,disclosure_sha256,consumer_contract_hash,consumer_ownership_version,version,adopted_by_actor_id,adopted_by_client_id)
        VALUES('upgrade-tenant','upgrade-project',$1,'consumer','legacy-work','legacy-export',1,repeat('b',64),repeat('c',64),1,1,'consumer','client')";
    admin.execute(insert, &[&"fixture-adoption"]).await.unwrap();
    awr_team_pg::Bootstrap::grant_app(&admin, "awr_app")
        .await
        .unwrap();
    let app = common::app_client(&db).await;
    for settings in [
        "SELECT set_config('awr.tenant_id','other-tenant',false),set_config('awr.project_id','upgrade-project',false)",
        "SELECT set_config('awr.tenant_id','upgrade-tenant',false),set_config('awr.project_id','other-project',false)",
    ] {
        app.batch_execute(settings).await.unwrap();
        assert_eq!(
            app.query_one(
                "SELECT count(*) FROM awr_team.workstream_artifact_adoptions",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
            0
        );
        assert_eq!(
            app.execute(insert, &[&"forbidden-adoption"])
                .await
                .unwrap_err()
                .code(),
            Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE)
        );
    }
    app.batch_execute("SELECT set_config('awr.tenant_id','upgrade-tenant',false),set_config('awr.project_id','upgrade-project',false)").await.unwrap();
    assert_eq!(
        app.query_one(
            "SELECT count(*) FROM awr_team.workstream_artifact_adoptions",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    assert!(
        admin
            .execute(insert, &[&"duplicate-generation"])
            .await
            .is_err()
    );
    admin
        .batch_execute("UPDATE awr_team.workstream_artifact_adoptions SET selected=false")
        .await
        .unwrap();
    admin
        .execute(
            &insert.replace(",1,'consumer','client')", ",2,'consumer','client')"),
            &[&"next-generation"],
        )
        .await
        .unwrap();
    assert_eq!(admin.query_one("SELECT count(*),count(*) FILTER(WHERE selected) FROM awr_team.workstream_artifact_adoptions", &[]).await.unwrap().get::<_, i64>(0), 2);
}

#[tokio::test]
async fn schema50_approved_exports_upgrade_atomically_preserve_history_and_enforce_rls() {
    let (_g, admin, db) = common::historical_team_schema(50).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
            VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
            dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,independence_kind)
            VALUES('upgrade-tenant','upgrade-project','legacy-receipt','legacy-work','main','old-contract','old-result',
                'old-dependencies','old-bundle','ordinary_confirm','{}','unspecified');
        INSERT INTO awr_team.artifacts(tenant_id,project_id,id,object_key,sha256,byte_length,media_type,state,created_by)
            VALUES('upgrade-tenant','upgrade-project','legacy-artifact','fixture',repeat('a',64),0,'text/plain','finalized','fixture-author')").await.unwrap();
    let original: Value = admin
        .query_one(
            "SELECT to_jsonb(r) FROM awr_team.completion_receipts r",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let ddl = include_str!("../migrations/20261008000051_workstream_artifact_exports.sql");
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
    let rolled_back=admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),to_regclass('awr_team.workstream_artifact_exports')::text",&[]).await.unwrap();
    assert_eq!(rolled_back.get::<_, i32>(0), 50);
    assert!(rolled_back.get::<_, Option<String>>(1).is_none());
    migrate(&admin).await.unwrap();
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    assert_eq!(
        admin
            .query_one(
                "SELECT to_jsonb(r) FROM awr_team.completion_receipts r",
                &[]
            )
            .await
            .unwrap()
            .get::<_, Value>(0),
        original,
        "Migration must not invent original review references or alter historical receipts"
    );
    let isolation=admin.query_one("SELECT relrowsecurity,relforcerowsecurity FROM pg_class WHERE oid='awr_team.workstream_artifact_exports'::regclass",&[]).await.unwrap();
    assert!(isolation.get::<_, bool>(0) && isolation.get::<_, bool>(1));
    let insert="INSERT INTO awr_team.workstream_artifact_exports(tenant_id,project_id,id,provider_work_id,consumer_work_id,receipt_id,artifact_id,
        disclosure_sha256,manifest_json,proof_json,published_by_actor_id,published_by_client_id)
        VALUES('upgrade-tenant','upgrade-project',$1,'legacy-work','consumer','legacy-receipt','legacy-artifact',repeat('b',64),'{}','{}','publisher','client')";
    admin.execute(insert, &[&"fixture-export"]).await.unwrap();
    awr_team_pg::Bootstrap::grant_app(&admin, "awr_app")
        .await
        .unwrap();
    let app = common::app_client(&db).await;
    app.batch_execute("SELECT set_config('awr.tenant_id','other-tenant',false),set_config('awr.project_id','upgrade-project',false)").await.unwrap();
    assert_eq!(
        app.query_one(
            "SELECT count(*) FROM awr_team.workstream_artifact_exports",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    assert_eq!(
        app.execute(insert, &[&"forbidden-export"])
            .await
            .unwrap_err()
            .code(),
        Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE)
    );
    app.batch_execute("SELECT set_config('awr.tenant_id','upgrade-tenant',false),set_config('awr.project_id','other-project',false)").await.unwrap();
    assert_eq!(
        app.query_one(
            "SELECT count(*) FROM awr_team.workstream_artifact_exports",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    app.batch_execute("SELECT set_config('awr.tenant_id','upgrade-tenant',false),set_config('awr.project_id','upgrade-project',false)").await.unwrap();
    assert_eq!(
        app.query_one(
            "SELECT count(*) FROM awr_team.workstream_artifact_exports",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn schema49_delivery_requests_upgrade_atomically_without_fabricating_legacy_bindings() {
    let (_guard, admin, db) = common::historical_team_schema(49).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
            VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.hard_delivery_dependencies(tenant_id,project_id,id,provider_work_id,provider_workstream_id,
            consumer_work_id,consumer_workstream_id,policy,status,completion_receipt,contract_sha256,artifact_sha256,created_at_ms,body_json)
            VALUES('upgrade-tenant','upgrade-project','legacy-dep','upstream','up-stream','downstream','down-stream',
                'fixed_delivery','active','legacy-completion','old-contract','old-artifact',10,'{\"legacy_fixture\":true}');
        INSERT INTO awr_team.export_authorizations(tenant_id,project_id,id,provider_work_id,status,completion_receipt,
            contract_sha256,artifact_sha256,export_scope_sha256,granted_by,created_at_ms,body_json)
            VALUES('upgrade-tenant','upgrade-project','legacy-export','upstream','granted','legacy-completion',
                'old-contract','old-artifact','old-scope','owner',20,'{\"legacy_fixture\":true}');
        INSERT INTO awr_team.adoption_credentials(tenant_id,project_id,id,dependency_id,status,completion_receipt,
            export_authorization_id,adopted_at_ms,body_json)
            VALUES('upgrade-tenant','upgrade-project','legacy-credential','legacy-dep','active','legacy-completion',
                'legacy-export',100,'{\"legacy_fixture\":true}');
        INSERT INTO awr_team.delivery_credential_receipts(tenant_id,project_id,request_key,subject_id,op,event_id,created_at_ms)
            SELECT 'upgrade-tenant','upgrade-project',op,'legacy-subject',op,'legacy-event-'||op,100
            FROM unnest(ARRAY['register_dependency','grant_export','adopt','revoke_dependency','revoke_export']) AS op").await.unwrap();
    let tables = [
        "hard_delivery_dependencies",
        "export_authorizations",
        "adoption_credentials",
        "delivery_credential_receipts",
    ];
    let mut originals = Vec::new();
    for table in tables {
        originals.push(admin.query_one(&format!("SELECT jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text) FROM awr_team.{table} t"), &[])
            .await.unwrap().get::<_,Value>(0));
    }
    let ddl = include_str!("../migrations/20261008000050_delivery_credential_requests.sql");
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
    let rolled_back = admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),
        (SELECT count(*) FROM information_schema.columns WHERE table_schema='awr_team'
            AND table_name='delivery_credential_receipts' AND column_name IN ('request_hash','result_json'))", &[]).await.unwrap();
    assert_eq!(rolled_back.get::<_, i32>(0), 49);
    assert_eq!(rolled_back.get::<_, i64>(1), 0);
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    migrate(&admin).await.unwrap();
    assert_eq!(EXPECTED_SCHEMA_VERSION, 52);
    for (table, original) in tables.into_iter().zip(originals) {
        let mut rows = admin.query_one(&format!("SELECT jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text) FROM awr_team.{table} t"), &[])
            .await.unwrap().get::<_,Value>(0);
        if table == "delivery_credential_receipts" {
            for row in rows.as_array_mut().unwrap() {
                assert_eq!(
                    row.as_object_mut().unwrap().remove("request_hash"),
                    Some(Value::Null)
                );
                assert_eq!(
                    row.as_object_mut().unwrap().remove("result_json"),
                    Some(Value::Null)
                );
            }
        }
        assert_eq!(rows, original);
        let policy = admin
            .query_one(
                "SELECT relrowsecurity,relforcerowsecurity FROM pg_class
            WHERE oid=to_regclass($1)",
                &[&format!("awr_team.{table}")],
            )
            .await
            .unwrap();
        assert!(policy.get::<_, bool>(0) && policy.get::<_, bool>(1));
    }
    for assignment in [
        "request_hash=repeat('a',64)",
        "result_json='{\"id\":\"legacy-subject\"}'",
        "request_hash=repeat('a',63),result_json='{\"id\":\"legacy-subject\"}'",
        "request_hash=repeat('A',64),result_json='{\"id\":\"legacy-subject\"}'",
        "request_hash=repeat('a',64),result_json='[]'",
        "request_hash=repeat('a',64),result_json='{}'",
        "request_hash=repeat('a',64),result_json='{\"id\":null}'",
        "request_hash=repeat('a',64),result_json='{\"id\":7}'",
        "request_hash=repeat('a',64),result_json='{\"id\":\"other-subject\"}'",
    ] {
        assert!(
            admin
                .batch_execute(&format!(
                    "UPDATE awr_team.delivery_credential_receipts SET {assignment}"
                ))
                .await
                .is_err(),
            "malformed request/outcome binding must be rejected: {assignment}"
        );
    }
    admin.batch_execute("UPDATE awr_team.delivery_credential_receipts SET request_hash=repeat('a',64),result_json='{\"id\":\"legacy-subject\"}';
        UPDATE awr_team.delivery_credential_receipts SET request_hash=NULL,result_json=NULL").await.unwrap();
    awr_team_pg::Bootstrap::grant_app(&admin, "awr_app")
        .await
        .unwrap();
    let app = common::app_client(&db).await;
    assert_eq!(
        app.query_one(
            "SELECT count(*) FROM awr_team.delivery_credential_receipts",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    app.batch_execute("SELECT set_config('awr.tenant_id','other-tenant',false),set_config('awr.project_id','upgrade-project',false)").await.unwrap();
    assert_eq!(
        app.query_one(
            "SELECT count(*) FROM awr_team.delivery_credential_receipts",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    app.batch_execute("SELECT set_config('awr.tenant_id','upgrade-tenant',false),set_config('awr.project_id','upgrade-project',false)").await.unwrap();
    assert_eq!(app.query_one("SELECT count(*) FROM awr_team.delivery_credential_receipts WHERE request_hash IS NULL AND result_json IS NULL", &[]).await.unwrap().get::<_,i64>(0),5);
}

#[tokio::test]
async fn schema48_acceptance_origins_upgrade_atomically_and_preserve_observer_and_legacy_records() {
    let (_guard, admin, db) = common::historical_team_schema(48).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
            VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES('upgrade-tenant','legacy-observer','system','Legacy observer','active');
        INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
            dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,independence_kind)
            VALUES('upgrade-tenant','upgrade-project','legacy-receipt','legacy-work','main','old-contract','old-result',
                'old-dependencies','old-bundle','ordinary_confirm','{}','unspecified');
        INSERT INTO awr_team.delivery_candidates(tenant_id,project_id,binding_digest,work_id,candidate_id,candidate_version,body_json)
            VALUES('upgrade-tenant','upgrade-project',repeat('a',64),'legacy-work','legacy-candidate','1','{\"legacy_fixture\":true}');
        INSERT INTO awr_team.delivery_connectors(tenant_id,project_id,id,scope_id,workstream_id,work_id,provider,resource,
            principal_actor_id,principal_client_id,fact_source,version,coordinator_epoch,enabled,configured_by_actor_id)
            VALUES('upgrade-tenant','upgrade-project','legacy-connector','main','legacy-stream','legacy-work','reference',
                'fixture://legacy','legacy-observer','legacy-client','adapter_observation',1,'old-epoch',true,'legacy-observer');
        INSERT INTO awr_team.delivery_inspections(tenant_id,project_id,id,connector_id,connector_version,generation,binding_digest,
            selection_version,source_snapshot_id,ownership_version,coordinator_epoch,actor_id,client_id,authority_binding,expires_at)
            VALUES('upgrade-tenant','upgrade-project','legacy-inspection','legacy-connector',1,1,repeat('a',64),1,
                'legacy-snapshot',1,'old-epoch','legacy-observer','legacy-client','legacy-authority',clock_timestamp()+interval '1 hour');
        INSERT INTO awr_team.delivery_inbox(tenant_id,project_id,id,connector_id,event_id,inspection_id,input_digest,state,receipt_json)
            VALUES('upgrade-tenant','upgrade-project','legacy-inbox','legacy-connector','legacy-event','legacy-inspection',repeat('b',64),'applied','{}');
        INSERT INTO awr_team.delivery_notifications(tenant_id,project_id,id,inbox_id,work_id)
            VALUES('upgrade-tenant','upgrade-project','legacy-notification','legacy-inbox','legacy-work');
        INSERT INTO awr_team.delivery_sync_intents(tenant_id,project_id,id,notification_id,kind,work_id,connector_id,
            connector_version,generation,candidate_digest,selection_version,read_set_json)
            VALUES('upgrade-tenant','upgrade-project','legacy-intent','legacy-notification','source','legacy-work',
                'legacy-connector',1,1,repeat('a',64),1,'{\"legacy_fixture\":true}')").await.unwrap();
    let mut originals = Vec::new();
    for table in [
        "delivery_notifications",
        "delivery_sync_intents",
        "completion_receipts",
    ] {
        let value: Value = admin
            .query_one(&format!("SELECT to_jsonb(t) FROM awr_team.{table} t"), &[])
            .await
            .unwrap()
            .get(0);
        originals.push(value);
    }
    let ddl = include_str!("../migrations/20261007000049_acceptance_source_sync.sql");
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
    let rolled_back = admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),
        (SELECT count(*) FROM information_schema.columns WHERE table_schema='awr_team' AND column_name='origin')", &[])
        .await.unwrap();
    assert_eq!(rolled_back.get::<_, i32>(0), 48);
    assert_eq!(rolled_back.get::<_, i64>(1), 0);
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    migrate(&admin).await.unwrap();
    for (table, original) in [
        "delivery_notifications",
        "delivery_sync_intents",
        "completion_receipts",
    ]
    .into_iter()
    .zip(originals)
    {
        let mut value: Value = admin
            .query_one(&format!("SELECT to_jsonb(t) FROM awr_team.{table} t"), &[])
            .await
            .unwrap()
            .get(0);
        if table != "completion_receipts" {
            assert_eq!(
                value.as_object_mut().unwrap().remove("origin"),
                Some(json!("adapter_observation"))
            );
            assert_eq!(
                value
                    .as_object_mut()
                    .unwrap()
                    .remove("completion_receipt_id"),
                Some(Value::Null)
            );
        }
        assert_eq!(value, original);
    }
    for sql in [
        "UPDATE awr_team.delivery_notifications SET origin='unrecognized'",
        "UPDATE awr_team.delivery_notifications SET origin='domain_acceptance',completion_receipt_id='legacy-receipt'",
        "UPDATE awr_team.delivery_sync_intents SET origin='domain_acceptance',completion_receipt_id='legacy-receipt'",
        "UPDATE awr_team.delivery_sync_intents SET connector_id=NULL",
    ] {
        assert!(
            admin.batch_execute(sql).await.is_err(),
            "Malformed or mixed origins must be rejected"
        );
    }
    awr_team_pg::Bootstrap::grant_app(&admin, "awr_app")
        .await
        .unwrap();
    let app = common::app_client(&db).await;
    assert_eq!(
        app.query_one("SELECT count(*) FROM awr_team.delivery_sync_intents", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0,
        "Existing forced RLS must remain effective"
    );
    assert_eq!(admin.query_one("SELECT count(*) FROM awr_team.delivery_notifications WHERE origin='domain_acceptance'", &[])
        .await.unwrap().get::<_,i64>(0), 0, "Migration must not infer acceptance from historical completions");
}

#[tokio::test]
async fn schema47_simulated_completion_upgrade_is_atomic_and_keeps_legacy_receipts() {
    let (_g, admin, _) = common::historical_team_schema(47).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
            dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,independence_kind)
        VALUES('upgrade-tenant','upgrade-project','legacy-receipt','legacy-work','main','old-contract','old-result',
            'old-dependencies','old-bundle','ordinary_confirm','{}','unspecified');").await.unwrap();
    let original: Value = admin
        .query_one(
            "SELECT to_jsonb(c) FROM awr_team.completion_receipts c",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let ddl = include_str!("../migrations/20261007000048_simulated_member_completion.sql");
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
    let row = admin
        .query_one(
            "SELECT (SELECT version FROM awr_team.schema_state),
        to_regprocedure('awr_team.keep_simulated_completion_basis()')::text,
        to_regclass('awr_team.completion_simulated_decision_once')::text",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, i32>(0), 47);
    assert!(row.get::<_, Option<String>>(1).is_none());
    assert!(row.get::<_, Option<String>>(2).is_none());
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    migrate(&admin).await.unwrap();
    let preserved: Value = admin
        .query_one(
            "SELECT to_jsonb(c) FROM awr_team.completion_receipts c",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(preserved, original);
    assert!(admin.batch_execute("UPDATE awr_team.completion_receipts SET independence_kind='simulated_member_independent',
        policy='caller_managed_execution_and_simulated_member_review'").await.is_err());
}

#[tokio::test]
async fn schema46_member_origins_upgrade_is_atomic_and_never_backfills_legacy_records() {
    let (_g, admin, _) = common::historical_team_schema(46).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
        VALUES('upgrade-tenant','upgrade-project','legacy-run','legacy-work',1,'old-contract','old-executor','unknown');
        INSERT INTO awr_team.evidence(tenant_id,project_id,id,work_id,contract_hash,evidence_kind,trust_basis,digest,payload_json,created_by)
        VALUES('upgrade-tenant','upgrade-project','legacy-evidence','legacy-work','old-contract','report','caller_asserted','old-digest','{}','old-executor');
        INSERT INTO awr_team.review_rounds(tenant_id,project_id,id,work_id,round_index,bundle_hash,contract_hash,author_actor_id,state,evidence_id)
        VALUES('upgrade-tenant','upgrade-project','legacy-round','legacy-work',1,'old-digest','old-contract','old-executor','approved','legacy-evidence');
        INSERT INTO awr_team.review_decisions(tenant_id,project_id,id,review_round_id,work_id,bundle_hash,reviewer_actor_id,decision,reason)
        VALUES('upgrade-tenant','upgrade-project','legacy-decision','legacy-round','legacy-work','old-digest','old-reviewer','approve','Legacy review');").await.unwrap();
    let tables = [
        ("executions", "executor_origin_json"),
        ("evidence", "member_origins_json"),
        ("review_rounds", "member_origins_json"),
        ("review_decisions", "member_review_basis_json"),
    ];
    let mut originals = vec![];
    for (table, _) in tables {
        let row: Value = admin
            .query_one(&format!("SELECT to_jsonb(t) FROM awr_team.{table} t"), &[])
            .await
            .unwrap()
            .get(0);
        originals.push(row);
    }
    let ddl = include_str!("../migrations/20261007000047_simulated_member_review.sql");
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
        (SELECT count(*) FROM information_schema.columns WHERE table_schema='awr_team' AND column_name='executor_origin_json'),
        to_regprocedure('awr_team.valid_member_origin(jsonb)')::text",&[]).await.unwrap();
    assert_eq!(row.get::<_, i32>(0), 46);
    assert_eq!(row.get::<_, i64>(1), 0);
    assert!(row.get::<_, Option<String>>(2).is_none());
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    migrate(&admin).await.unwrap();
    for ((table, column), original) in tables.into_iter().zip(originals) {
        let mut row: Value = admin
            .query_one(&format!("SELECT to_jsonb(t) FROM awr_team.{table} t"), &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            row.as_object_mut().unwrap().remove(column),
            Some(Value::Null)
        );
        assert_eq!(row, original);
        assert!(
            admin
                .batch_execute(&format!("UPDATE awr_team.{table} SET {column}='{{}}'"))
                .await
                .is_err()
        );
    }
    assert!(admin.batch_execute("UPDATE awr_team.review_decisions SET independence_kind='simulated_member_independent',
        approval_basis='simulated_member_independent_review',reviewer_client_id='legacy-client'").await.is_err());
    for value in [
        json!({}),
        json!({"codec":"awr-member-origin-v1"}),
        json!({"codec":"awr-member-origin-v1","caller_identity":"forged"}),
    ] {
        let accepted: bool = admin
            .query_one("SELECT awr_team.valid_member_origin($1)", &[&value])
            .await
            .unwrap()
            .get(0);
        assert!(!accepted);
    }
}

#[tokio::test]
async fn schema45_integration_upgrade_is_atomic_and_preserves_delivery_records() {
    let (_g, admin, _) = common::historical_team_schema(45).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.delivery_candidates(tenant_id,project_id,binding_digest,work_id,candidate_id,candidate_version,body_json)
        VALUES('upgrade-tenant','upgrade-project',repeat('a',64),'legacy-work','legacy-candidate','1','{\"legacy_fixture\":true}')").await.unwrap();
    let original: Value = admin
        .query_one(
            "SELECT to_jsonb(c) FROM awr_team.delivery_candidates c",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let ddl = include_str!("../migrations/20261006000046_delivery_integration.sql");
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
    let before=admin.query_one("SELECT (SELECT version FROM awr_team.schema_state),to_regclass('awr_team.delivery_integration_intents')::text,
        to_regclass('awr_team.delivery_integration_target_guards')::text",&[]).await.unwrap();
    assert_eq!(before.get::<_, i32>(0), 45);
    assert!(before.get::<_, Option<String>>(1).is_none());
    assert!(before.get::<_, Option<String>>(2).is_none());
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    migrate(&admin).await.unwrap();
    let after = admin
        .query_one(
            "SELECT (SELECT version FROM awr_team.schema_state),
        (SELECT count(*) FROM awr_team.delivery_integration_intents),
        (SELECT count(*) FROM awr_team.delivery_integration_target_guards)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(after.get::<_, i32>(0), EXPECTED_SCHEMA_VERSION);
    assert_eq!(after.get::<_, i64>(1), 0);
    assert_eq!(after.get::<_, i64>(2), 0);
    let preserved: Value = admin
        .query_one(
            "SELECT to_jsonb(c) FROM awr_team.delivery_candidates c",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(preserved, original);
}

#[tokio::test]
async fn schema43_source_publication_upgrade_is_atomic_and_leaves_legacy_receipts_unbound() {
    let (_g, admin, _) = common::historical_team_schema(43).await;
    admin.batch_execute("INSERT INTO awr_team.tenants(id,name,status) VALUES('upgrade-tenant','Upgrade','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('upgrade-tenant','upgrade-project','upgrade','team','old-epoch','active');
        INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
        VALUES('upgrade-tenant','upgrade-project','legacy-work','legacy-work');
        INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
            dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json)
        VALUES('upgrade-tenant','upgrade-project','legacy-completion','legacy-work','main','old-contract','old-result',
            'old-dependencies','old-evidence','review','[]')").await.unwrap();
    let before: Value = admin
        .query_one(
            "SELECT to_jsonb(c) FROM awr_team.completion_receipts c",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let ddl = include_str!("../migrations/20261006000044_delivery_source_publisher.sql");
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
    let rolled_back = admin
        .query_one(
            "SELECT (SELECT version FROM awr_team.schema_state),
        to_regclass('awr_team.delivery_source_publications')::text,
        (SELECT count(*) FROM information_schema.columns WHERE table_schema='awr_team'
          AND table_name='completion_receipts' AND column_name='delivery_candidate_digest')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(rolled_back.get::<_, i32>(0), 43);
    assert!(rolled_back.get::<_, Option<String>>(1).is_none());
    assert_eq!(rolled_back.get::<_, i64>(2), 0);
    let unchanged: Value = admin
        .query_one(
            "SELECT to_jsonb(c) FROM awr_team.completion_receipts c",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(unchanged, before);
    migrate(&admin).await.unwrap();
    check_schema(&admin).await.unwrap();
    let after: Value = admin
        .query_one(
            "SELECT to_jsonb(c) FROM awr_team.completion_receipts c",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let mut preserved = after.clone();
    assert!(preserved["delivery_candidate_digest"].is_null());
    preserved
        .as_object_mut()
        .unwrap()
        .remove("delivery_candidate_digest");
    assert_eq!(preserved, before);
    let empty = admin
        .query_one(
            "SELECT (SELECT count(*) FROM awr_team.delivery_source_cursors),
        (SELECT count(*) FROM awr_team.delivery_source_publications)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(empty.get::<_, i64>(0), 0);
    assert_eq!(empty.get::<_, i64>(1), 0);
    migrate(&admin).await.unwrap();
    let repeated: Value = admin
        .query_one(
            "SELECT to_jsonb(c) FROM awr_team.completion_receipts c",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(repeated, after);
}

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
    assert_eq!(EXPECTED_SCHEMA_VERSION, 52);
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
        "executor_origin_json",
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
