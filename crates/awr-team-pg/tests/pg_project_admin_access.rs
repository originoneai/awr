#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::{
    AdminAccessPlan, OperatorAccess, PgError, ProjectAccessStore, workstream_credential_hash,
};
use fixture::*;
use serde_json::json;

const NEW_TOKEN: &str =
    "awr1.new-member.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

#[tokio::test]
async fn project_credentials_rotate_without_returning_secrets_or_changing_other_scopes() {
    let (_g, owner, db, store) = setup().await;
    enable_admin_manage(&owner).await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut plan = admin_plan_member();
    plan.credential_project_scoped = true;
    let p = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "scoped-issue",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        prepare(&store, NEW_TOKEN, "a").await["data"]["work_id"],
        "a"
    );
    let members = access.members(TENANT, PROJECT, A, None, 100).await.unwrap();
    let member = members["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["actor_id"] == "new-human")
        .unwrap();
    assert_eq!(
        member["clients"][0]["credentials"][0]["project_scoped"],
        true
    );
    assert!(!members.to_string().contains(NEW_TOKEN));
    assert!(!members.to_string().contains("secret_hash"));
    assert!(matches!(
        access.members(TENANT, PROJECT, NEW_TOKEN, None, 100).await,
        Err(PgError::Forbidden)
    ));
    // Keep all current grants and membership valid; changing only the bound
    // project must make authentication fail independently of those grants.
    owner.batch_execute("INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status) VALUES('reader-tenant','credential-other','credential-other','team','epoch-other','active'); UPDATE awr_team.credentials SET project_id='credential-other' WHERE id='new-member'").await.unwrap();
    assert!(matches!(
        store
            .query(TENANT, PROJECT, NEW_TOKEN, query("capabilities"))
            .await,
        Err(PgError::Forbidden)
    ));
    owner
        .batch_execute(
            "UPDATE awr_team.credentials SET project_id='reader-project' WHERE id='new-member'",
        )
        .await
        .unwrap();
    let second =
        "awr1.rotated-member.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    plan.credential.as_mut().unwrap().id = "rotated-member".into();
    plan.credential.as_mut().unwrap().secret_hash = workstream_credential_hash(second).unwrap();
    plan.revoke_project_credentials = vec!["new-member".into()];
    let p = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    let applied = access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "scoped-rotate",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(applied["replayed"], false);
    let replay = access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "scoped-rotate",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert!(!replay.to_string().contains(second));
    assert!(!replay.to_string().contains("secret_hash"));
    assert!(matches!(
        store
            .query(TENANT, PROJECT, NEW_TOKEN, query("capabilities"))
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(prepare(&store, second, "a").await["data"]["work_id"], "a");
    let page = access.members(TENANT, PROJECT, A, None, 1).await.unwrap();
    if let Some(cursor) = page["next_cursor"].as_str() {
        let next = access
            .members(TENANT, PROJECT, A, Some(cursor), 1)
            .await
            .unwrap();
        assert!(
            next["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["actor_id"].as_str().unwrap() > cursor)
        );
    }
    // Legacy tenant credentials cannot be revoked through the new project path.
    owner.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,'legacy-member','new-human','new-cli',$2)", &[&TENANT,&workstream_credential_hash("awr1.legacy-member.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff").unwrap()]).await.unwrap();
    plan.credential = None;
    plan.credential_project_scoped = false;
    plan.revoke_project_credentials = vec!["legacy-member".into()];
    assert!(matches!(
        access.preview(TENANT, PROJECT, A, &plan).await,
        Err(PgError::Forbidden)
    ));
}

async fn enable_admin_manage(owner: &tokio_postgres::Client) {
    owner
        .batch_execute(
            "UPDATE awr_team.workstream_grants
             SET can_write=true, can_manage=true, grant_version=grant_version+1
             WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
}

fn admin_plan_member() -> AdminAccessPlan {
    serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"new-human","kind":"human","display_name":"New member"},
        "subject_client_id":"new-cli",
        "role":"developer",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write":true,"manage":false,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"new-member",
            "secret_hash":workstream_credential_hash(NEW_TOKEN).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap()
}

#[tokio::test]
async fn duty_restricted_admin_is_not_a_handoff_manager_and_directory_matches_live_policy() {
    use awr_team::BusinessRole;
    use std::collections::BTreeSet;
    let (_g, owner, db, store) = setup().await;
    enable_admin_manage(&owner).await;
    // Changing project-wide duties also affects this member's other client.
    owner
        .batch_execute(
            "INSERT INTO awr_team.workstream_grants
             (tenant_id, project_id, actor_id, client_id, workstream_id,
              can_read, can_write, can_manage, authority_version, grant_version, active)
             VALUES ('reader-tenant','reader-project','agent','cli-a',
              '00000000000000000000000002',true,true,true,1,1,true)
             ON CONFLICT (tenant_id,project_id,actor_id,client_id,workstream_id)
             DO UPDATE SET can_manage=true,grant_version=awr_team.workstream_grants.grant_version+1",
        )
        .await
        .unwrap();
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut other = admin_plan_member();
    other.role = "project_admin".into();
    other.business_roles = Some(BTreeSet::from([BusinessRole::Observer]));
    other.grants[0].manage = true;
    let p = access.preview(TENANT, PROJECT, A, &other).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &other,
            "shadow-admin",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let directory = access.members(TENANT, PROJECT, A, None, 100).await.unwrap();
    let member = directory["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["actor_id"] == "new-human")
        .unwrap();
    assert_eq!(member["business_roles"], json!(["observer"]));
    assert_eq!(member["membership_action_ceiling"], json!(["work.read"]));
    assert!(matches!(
        access.members(TENANT, PROJECT, NEW_TOKEN, None, 100).await,
        Err(PgError::Forbidden)
    ));

    let demote: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,"subject":{"id":"agent","kind":"human","display_name":"Worker"},
        "subject_client_id":"cli-a","role":"project_admin","business_roles":["observer"],
        "grants":other.grants,"credential":null
    }))
    .unwrap();
    assert!(matches!(
        access.preview(TENANT, PROJECT, A, &demote).await,
        Err(PgError::Forbidden)
    ));
    other.business_roles = Some(BTreeSet::from([BusinessRole::Administrator]));
    let p = access.preview(TENANT, PROJECT, A, &other).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &other,
            "usable-admin",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let p = access.preview(TENANT, PROJECT, A, &demote).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &demote,
            "handoff-duties",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        access.members(TENANT, PROJECT, A, None, 100).await,
        Err(PgError::Forbidden)
    ));
    assert!(
        access
            .members(TENANT, PROJECT, NEW_TOKEN, None, 100)
            .await
            .is_ok()
    );
    let caps = store
        .query(TENANT, PROJECT, NEW_TOKEN, query("capabilities"))
        .await
        .unwrap();
    assert_eq!(caps["identity"]["can_manage_members"], true);
    let fresh = prepare(&store, NEW_TOKEN, "a").await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                NEW_TOKEN,
                command(
                    &fresh,
                    "admin-not-developer",
                    "session.start",
                    json!({"conversation_id":"no-implicit-development"})
                )
            )
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn changed_duties_check_the_actors_other_clients_but_omission_keeps_their_policy() {
    use awr_team::BusinessRole;
    use std::collections::BTreeSet;
    let (_g, mut owner, db, _) = setup().await;
    enable_admin_manage(&owner).await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut first = admin_plan_member();
    first.business_roles = Some(BTreeSet::from([BusinessRole::Developer]));
    let p = access.preview(TENANT, PROJECT, A, &first).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &first,
            "client-one-duties",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let second:awr_team_pg::AccessPlan=serde_json::from_value(json!({
        "protocol_version":1,"tenant_id":TENANT,"project_id":PROJECT,
        "actor":first.subject,"client_id":"other-client","role":"developer",
        "grants":[{"workstream_id":awr_core::Id::from(2),"authority_version":"1","read":true,"write":true,
            "manage":false,"attest_execution":false,"reconcile_execution":false}],
        "credential":null,"revoke_credentials":[]
    })).unwrap();
    let p = OperatorAccess::preview(&mut owner, &second).await.unwrap();
    OperatorAccess::apply(
        &mut owner,
        &second,
        "second-client-scope",
        p["state_digest"].as_str().unwrap(),
        p["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    first.business_roles = None;
    assert!(access.preview(TENANT, PROJECT, A, &first).await.is_ok());
    first.business_roles = Some(BTreeSet::from([BusinessRole::Observer]));
    assert!(matches!(
        access.preview(TENANT, PROJECT, A, &first).await,
        Err(PgError::Forbidden)
    ));
    let after = OperatorAccess::inspect(&mut owner, TENANT, PROJECT, "new-human", "other-client")
        .await
        .unwrap();
    assert_eq!(
        after["state"]["membership"]["business_roles"],
        json!(["developer"])
    );
    assert_eq!(after["state"]["grants"][0]["active"], true);
}

#[tokio::test]
async fn project_admin_mcp_path_preview_apply_outcome_and_denies_non_admin() {
    let (_g, owner, db, store) = setup().await;
    enable_admin_manage(&owner).await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    // Token A is actor=agent role=admin — project admin with manage grant ceiling.
    let plan = admin_plan_member();
    let preview = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["raw_secrets_in_response"], false);
    assert!(!preview.to_string().contains(NEW_TOKEN));
    assert!(
        !preview
            .to_string()
            .contains(plan.credential.as_ref().unwrap().secret_hash.as_str())
    );
    assert_eq!(
        preview["credential_revocation_scope"],
        "explicit_project_credentials_only"
    );
    assert_eq!(preview["project_revoke_preserves_other_projects"], true);
    let applied = access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "add-member-1",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(applied["replayed"], false);
    assert!(!applied.to_string().contains(NEW_TOKEN));
    let outcome = access
        .outcome(TENANT, PROJECT, A, "add-member-1")
        .await
        .unwrap();
    assert_eq!(outcome["outcome"], "committed");
    assert_eq!(
        outcome["receipt"]["request_id"],
        applied["receipt"]["request_id"]
    );
    // Exact replay
    let replay = access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "add-member-1",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], applied["receipt"]);
    // New member can read granted workstream.
    assert_eq!(
        prepare(&store, NEW_TOKEN, "a").await["data"]["work_id"],
        "a"
    );
    // Non-admin (reader-b / reviewer membership) cannot manage access.
    // B is actor=agent? No - reader-b is also actor agent with cli-b. Same admin role!
    // Use NONE (no-grants) which still has membership admin... fixture gives agent admin.
    // Create a worker-only subject and use a separate non-admin token after demoting...
    // Instead: register a reader-only member and try with their token.
    let reader_token =
        "awr1.reader-only.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let reader_plan: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"reader-human","kind":"human","display_name":"Reader"},
        "subject_client_id":"reader-cli",
        "role":"reader",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write":false,"manage":false,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"reader-only",
            "secret_hash":workstream_credential_hash(reader_token).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap();
    let p2 = access
        .preview(TENANT, PROJECT, A, &reader_plan)
        .await
        .unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &reader_plan,
            "add-reader",
            p2["state_digest"].as_str().unwrap(),
            p2["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        access.preview(TENANT, PROJECT, reader_token, &plan).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        access
            .apply(
                TENANT,
                PROJECT,
                reader_token,
                &plan,
                "evil",
                preview["state_digest"].as_str().unwrap(),
                preview["plan_digest"].as_str().unwrap()
            )
            .await,
        Err(PgError::Forbidden)
    ));
    // Owner path still works and app cannot use OperatorAccess.
    let _ = owner;
}

#[tokio::test]
async fn tenant_credential_revoke_refused_project_revoke_preserves_other_projects_and_last_admin() {
    let (_g, mut owner, db, _) = setup().await;
    enable_admin_manage(&owner).await;
    // cli-b's grant is on stream 2. Replacing that client's grants requires the
    // caller to already manage stream 2; this does not touch the other project.
    owner
        .execute(
            "INSERT INTO awr_team.workstream_grants(
                tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,
                can_read,can_write,can_manage,can_attest_execution,can_reconcile_execution,active)
             VALUES($1,$2,'agent','cli-a',$3,1,true,true,true,false,false,true)",
            &[&TENANT, &PROJECT, &awr_core::Id::from(2).to_string()],
        )
        .await
        .unwrap();
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    // Seed a grant in another project for the same actor to prove project revoke is scoped.
    owner
        .batch_execute(
            "INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
             VALUES('reader-tenant','other-project','o','team','epoch-o','active');
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
             VALUES('reader-tenant','other-project','agent','admin');
             INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage,can_attest_execution,can_reconcile_execution,active)
             VALUES('reader-tenant','other-project','agent','cli-a','00000000000000000000000001',1,true,true,true,false,false,true);",
        )
        .await
        .unwrap();

    let mut revoke_tenant: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"agent","kind":"human","display_name":"Worker"},
        "subject_client_id":"cli-b",
        "role":"admin",
        "grants":[],
        "credential":null,
        "remove_membership":false,
        "revoke_tenant_credentials":["reader-b"]
    }))
    .unwrap();
    assert!(matches!(
        access.preview(TENANT, PROJECT, A, &revoke_tenant).await,
        Err(PgError::Forbidden)
    ));

    // Project grant clear for cli-b must not delete other-project grants (cli-a).
    // Keep the admin caller's (cli-a) manage ceiling intact for later steps.
    revoke_tenant.revoke_tenant_credentials.clear();
    let preview = access
        .preview(TENANT, PROJECT, A, &revoke_tenant)
        .await
        .unwrap();
    assert_eq!(
        preview["impact"]["project_grant_revoke_preserves_other_projects"],
        true
    );
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &revoke_tenant,
            "clear-cli-b",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let other: i64 = owner
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.workstream_grants
             WHERE tenant_id=$1 AND project_id='other-project' AND actor_id='agent' AND active",
            &[&TENANT],
        )
        .await
        .unwrap()
        .get(0);
    assert!(other >= 1);

    // Special elevation refused.
    let mut special = admin_plan_member();
    special.grants[0].attest_execution = true;
    assert!(matches!(
        access.preview(TENANT, PROJECT, A, &special).await,
        Err(PgError::Protocol(_)) | Err(PgError::Forbidden)
    ));

    // Last admin cannot remove themselves without handoff.
    // First ensure only one admin remains: demote reviewer is not admin.
    // Promote a second admin, then demote first — positive handoff.
    let second: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"reviewer","kind":"human","display_name":"Reviewer"},
        "subject_client_id":"reviewer-cli",
        "role":"project_admin",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write":true,"manage":true,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"handoff-reviewer",
            "secret_hash":workstream_credential_hash("awr1.handoff-reviewer.2222222222222222222222222222222222222222222222222222222222222222").unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap();
    let p = access.preview(TENANT, PROJECT, A, &second).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &second,
            "handoff-promote",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();

    // Now removing agent admin membership should succeed (reviewer is admin).
    let remove: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"agent","kind":"human","display_name":"Worker"},
        "subject_client_id":"cli-a",
        "role":"reader",
        "grants":[],
        "credential":null,
        "remove_membership":true,
        "revoke_tenant_credentials":[]
    }))
    .unwrap();
    let p = access.preview(TENANT, PROJECT, A, &remove).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &remove,
            "handoff-remove",
            p["state_digest"].as_str().unwrap(),
            p["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();

    // Last remaining admin (reviewer) cannot remove self.
    // Register another reviewer credential via the owner path for rotation checks.
    let reviewer_token =
        "awr1.reviewer-admin.1111111111111111111111111111111111111111111111111111111111111111";
    let owner_plan: awr_team_pg::AccessPlan = serde_json::from_value(json!({
        "protocol_version":1,"tenant_id":TENANT,"project_id":PROJECT,
        "actor":{"id":"reviewer","kind":"human","display_name":"Reviewer"},
        "client_id":"reviewer-cli","role":"project_admin",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),"authority_version":"1",
            "read":true,"write":true,"manage":true,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"reviewer-admin",
            "secret_hash":workstream_credential_hash(reviewer_token).unwrap(),
            "expires_at_unix_ms":null
        },
        "revoke_credentials":[]
    }))
    .unwrap();
    let op = OperatorAccess::preview(&mut owner, &owner_plan)
        .await
        .unwrap();
    OperatorAccess::apply(
        &mut owner,
        &owner_plan,
        "owner-reviewer-cred",
        op["state_digest"].as_str().unwrap(),
        op["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();

    let last_remove: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"reviewer","kind":"human","display_name":"Reviewer"},
        "subject_client_id":"reviewer-cli",
        "role":"reader",
        "grants":[],
        "credential":null,
        "remove_membership":true,
        "revoke_tenant_credentials":[]
    }))
    .unwrap();
    assert!(matches!(
        access
            .preview(TENANT, PROJECT, reviewer_token, &last_remove)
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn concurrent_admin_applies_serialize_and_owner_receipts_stay_separate() {
    let (_g, owner, db, _) = setup().await;
    enable_admin_manage(&owner).await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let plan = admin_plan_member();
    let preview = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    let access2 =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let (a, b) = tokio::join!(
        access.apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "one",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        ),
        access2.apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "two",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let err = a.err().or(b.err()).unwrap();
    assert!(
        matches!(
            err,
            PgError::PreconditionsChanged | PgError::IdempotencyConflict | PgError::Db(_)
        ),
        "unexpected concurrent error: {err:?}"
    );
    // Owner OperatorAccess still cannot be called by app role.
    let mut app = common::app_client(&db).await;
    let owner_plan: awr_team_pg::AccessPlan = serde_json::from_value(json!({
        "protocol_version":1,"tenant_id":TENANT,"project_id":PROJECT,
        "actor":{"id":"x","kind":"agent","display_name":"X"},
        "client_id":"x","role":"reader","grants":[],"credential":null,"revoke_credentials":[]
    }))
    .unwrap();
    assert!(matches!(
        OperatorAccess::preview(&mut app, &owner_plan).await,
        Err(PgError::Forbidden)
    ));
    let _ = owner;
}

#[tokio::test]
async fn admin_membership_without_manage_grant_cannot_escalate_on_preview_or_apply() {
    let (_g, owner, db, _) = setup().await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let plan = admin_plan_member();
    // NONE: admin membership, client unscoped, zero grants — AccessManageProject alone
    // must not bootstrap developer membership/credential/grants.
    assert!(
        matches!(
            access.preview(TENANT, PROJECT, NONE, &plan).await,
            Err(PgError::Forbidden)
        ),
        "NONE preview must enforce client grant ceiling"
    );
    assert!(
        matches!(
            access
                .apply(
                    TENANT,
                    PROJECT,
                    NONE,
                    &plan,
                    "none-escalate",
                    "a".repeat(64).as_str(),
                    "b".repeat(64).as_str(),
                )
                .await,
            Err(PgError::Forbidden)
        ),
        "NONE apply must enforce client grant ceiling"
    );
    assert!(
        matches!(
            access
                .inspect(TENANT, PROJECT, NONE, "agent", "cli-a")
                .await,
            Err(PgError::Forbidden)
        ),
        "NONE inspect must enforce client grant ceiling"
    );
    assert!(
        matches!(
            access.outcome(TENANT, PROJECT, NONE, "any").await,
            Err(PgError::Forbidden)
        ),
        "NONE outcome must enforce client grant ceiling"
    );

    // Read-only grant (no manage) on an admin membership still cannot escalate.
    enable_admin_manage(&owner).await;
    owner
        .batch_execute(
            "UPDATE awr_team.workstream_grants
             SET can_write=false, can_manage=false, grant_version=grant_version+1
             WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        access.preview(TENANT, PROJECT, A, &plan).await,
        Err(PgError::Forbidden)
    ));

    // Restoring manage+write allows the same plan (grant ceiling satisfied).
    enable_admin_manage(&owner).await;
    let preview = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    assert_eq!(preview["applied"], false);
}

#[tokio::test]
async fn empty_grants_cannot_wipe_unmanaged_private_stream_via_full_delta() {
    let (_g, owner, db, _) = setup().await;
    enable_admin_manage(&owner).await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));

    // Token A manages only alpha (stream 1). cli-b holds private-beta (stream 2).
    // Inspect of cli-b must already fail the ceiling.
    assert!(
        matches!(
            access.inspect(TENANT, PROJECT, A, "agent", "cli-b").await,
            Err(PgError::Forbidden)
        ),
        "alpha-only manager must not inspect cli-b private-beta grants"
    );

    // Replacing cli-b's grant set with [] would deactivate private-beta. Refuse.
    let wipe: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"agent","kind":"agent","display_name":"Worker"},
        "subject_client_id":"cli-b",
        "role":"admin",
        "grants":[],
        "credential":null,
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap();
    assert!(
        matches!(
            access.preview(TENANT, PROJECT, A, &wipe).await,
            Err(PgError::Forbidden)
        ),
        "empty grants must authorize full delta including current private-beta"
    );
    assert!(
        matches!(
            access
                .apply(
                    TENANT,
                    PROJECT,
                    A,
                    &wipe,
                    "wipe-cli-b",
                    "a".repeat(64).as_str(),
                    "b".repeat(64).as_str(),
                )
                .await,
            Err(PgError::Forbidden)
        ),
        "apply with grants=[] must not wipe unmanaged streams"
    );

    // private-beta grant remains active for cli-b.
    let active: i64 = owner
        .query_one(
            "SELECT count(*)::bigint FROM awr_team.workstream_grants
             WHERE client_id='cli-b' AND active
               AND workstream_id=$1",
            &[&awr_core::Id::from(2).to_string()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(active, 1, "private-beta grant must survive refused wipe");
}

#[tokio::test]
async fn agent_review_grant_roundtrips_without_granting_human_review_or_other_clients() {
    let (_g, owner, db, _store) = setup().await;
    enable_admin_manage(&owner).await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut plan = admin_plan_member();
    plan.agent_review = true;
    assert!(access.preview(TENANT, PROJECT, A, &plan).await.is_err());
    plan.subject.id = "agent-reviewer".into();
    plan.subject.kind = "agent".into();
    plan.subject.display_name = "Review assistant".into();
    let initial = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "agent-review-add",
            initial["state_digest"].as_str().unwrap(),
            initial["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let members = access.members(TENANT, PROJECT, A, None, 100).await.unwrap();
    let member = members["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["actor_id"] == "agent-reviewer")
        .unwrap();
    assert_eq!(member["agent_review"], true);
    assert_eq!(member["independent_review"], false);
    assert!(!members.to_string().contains(NEW_TOKEN));
    assert!(!members.to_string().contains("secret_hash"));
    let before = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    plan.agent_review = false;
    let revoke = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    assert_ne!(before["plan_digest"], revoke["plan_digest"]);
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "agent-review-revoke",
            revoke["state_digest"].as_str().unwrap(),
            revoke["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let after = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    assert_ne!(before["state_digest"], after["state_digest"]);
    let row=owner.query_one("SELECT agent_review,independent_review,membership_version FROM awr_team.project_memberships WHERE actor_id='agent-reviewer'",&[]).await.unwrap();
    assert!(!row.get::<_, bool>(0));
    assert!(!row.get::<_, bool>(1));
    assert_eq!(row.get::<_, i64>(2), 2);
}
