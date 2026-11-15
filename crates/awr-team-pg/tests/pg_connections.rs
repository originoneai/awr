#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::{ConnectionSubject, PgError};
use fixture::*;
use serde_json::json;

#[tokio::test]
async fn own_binding_filters_secrets_and_rechecks_current_authority() {
    let (_guard, admin, _, store) = setup().await;
    let connections = store.connections();
    let result = connections
        .operate(TENANT, PROJECT, A, None, None, |scope| {
            assert!(scope.is_self);
            assert!(scope.allows(A));
            assert!(!scope.allows(B));
            Ok(json!({"items":scope.credentials()}))
        })
        .await
        .unwrap();
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    for secret in [A, B, "sha256:", "reader-a", "cli-a"] {
        assert!(!result.to_string().contains(secret));
    }
    for token in [
        NONE,
        "awr1.reader-a.0000000000000000000000000000000000000000000000000000000000000000",
    ] {
        assert!(matches!(
            connections
                .operate(TENANT, PROJECT, token, None, None, |_| panic!(
                    "denied effects must not run"
                ))
                .await,
            Err(PgError::Forbidden)
        ));
    }
    admin
        .batch_execute("UPDATE awr_team.workstream_grants SET active=false WHERE client_id='cli-a'")
        .await
        .unwrap();
    assert!(matches!(
        connections
            .operate(TENANT, PROJECT, A, None, None, |_| panic!(
                "revoked effects must not run"
            ))
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn administration_requires_covering_manage_grants_and_stays_project_scoped() {
    let (_guard, admin, _, store) = setup().await;
    let connections = store.connections();
    let subject = ConnectionSubject {
        actor_id: "agent".into(),
        client_id: "cli-b".into(),
    };
    assert!(matches!(
        connections
            .operate(TENANT, PROJECT, A, Some(&subject), None, |_| panic!(
                "label-only admin"
            ))
            .await,
        Err(PgError::Forbidden)
    ));
    admin.batch_execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage) SELECT tenant_id,project_id,actor_id,'cli-a',workstream_id,authority_version,true,true,true FROM awr_team.workstream_grants WHERE client_id='cli-b'; UPDATE awr_team.workstream_grants SET can_read=true,can_write=true,can_manage=true WHERE client_id='cli-a';").await.unwrap();
    let global = connections
        .operate(TENANT, PROJECT, A, Some(&subject), None, |scope| {
            Ok(json!(scope.credentials()))
        })
        .await
        .unwrap();
    assert!(
        global.as_array().unwrap().is_empty(),
        "project admin cannot list account-wide credentials"
    );
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET project_id='reader-project' WHERE id='reader-b'",
        )
        .await
        .unwrap();
    let result = connections
        .operate(TENANT, PROJECT, A, Some(&subject), None, |scope| {
            assert!(!scope.is_self);
            assert!(scope.allows(B));
            assert!(!scope.allows(A));
            Ok(json!(scope.credentials()))
        })
        .await
        .unwrap();
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["can_revoke"], false);
    let reference = result[0]["connection_id"].as_str().unwrap();
    assert!(matches!(
        connections
            .operate(
                TENANT,
                PROJECT,
                A,
                Some(&subject),
                Some(reference),
                |_| panic!("shared access uses member-admin preview/apply")
            )
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn shared_credential_revoke_is_idempotent_and_cuts_off_all_uses() {
    let (_guard, admin, _, store) = setup().await;
    let second =
        "awr1.reader-second.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let hash = awr_team_pg::workstream_credential_hash(second).unwrap();
    admin.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash,project_id) VALUES($1,'reader-second','agent','cli-a',$2,$3)",&[&TENANT,&hash,&PROJECT]).await.unwrap();
    let connections = store.connections();
    let reference = connections
        .operate(TENANT, PROJECT, A, None, None, |scope| {
            Ok(json!(scope.credentials()))
        })
        .await
        .unwrap()[0]["connection_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut cutoff = None;
    for _ in 0..2 {
        let result = connections
            .operate(TENANT, PROJECT, second, None, Some(&reference), |_| {
                panic!("credential effect belongs to PG")
            })
            .await
            .unwrap();
        assert_eq!(result["state"], "revoked");
        if let Some(first) = &cutoff {
            assert_eq!(&result["revoked_at_ms"], first);
        }
        cutoff = Some(result["revoked_at_ms"].clone());
    }
    assert!(matches!(
        connections
            .operate(TENANT, PROJECT, A, None, None, |_| panic!(
                "revoked credential"
            ))
            .await,
        Err(PgError::Forbidden)
    ));
    assert!(
        connections
            .operate(TENANT, PROJECT, second, None, None, |_| Ok(json!({})))
            .await
            .is_ok()
    );
}
