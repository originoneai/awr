#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team::{DirectoryMember, DirectoryUnit, ProjectDirectoryPlan};
use awr_team_pg::{DirectoryApplyRequest, PgError, WorkstreamReadStore};
use fixture::*;
use serde_json::json;
use tokio_postgres::Client;

fn plan() -> ProjectDirectoryPlan {
    ProjectDirectoryPlan {
        organization: Some(DirectoryUnit {
            id: "example".into(),
            name: "Example company".into(),
        }),
        departments: vec![DirectoryUnit {
            id: "engineering".into(),
            name: "Engineering".into(),
        }],
        project_department_ids: vec!["engineering".into()],
        members: vec![
            DirectoryMember {
                person_id: "person-one".into(),
                department_ids: vec!["engineering".into()],
            },
            DirectoryMember {
                person_id: "person-two".into(),
                department_ids: vec!["engineering".into()],
            },
        ],
    }
}

fn request(id: &str, version: &str, plan: ProjectDirectoryPlan) -> DirectoryApplyRequest {
    DirectoryApplyRequest {
        protocol_version: 1,
        request_id: id.into(),
        expected_version: version.into(),
        expected_plan_digest: plan.digest().unwrap(),
        plan,
    }
}

async fn prepare_directory(admin: &Client) {
    admin.batch_execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','reader-project','person-one','Alex','active'),
              ('reader-tenant','reader-project','person-two','Alex','active');
        INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage)
        SELECT tenant_id,project_id,actor_id,'cli-a',workstream_id,authority_version,true,true,true
        FROM awr_team.workstream_grants WHERE client_id='cli-b';
        UPDATE awr_team.workstream_grants SET can_read=true,can_write=true,can_manage=true WHERE client_id='cli-a';").await.unwrap();
}

#[tokio::test]
async fn preview_apply_restart_and_lost_response_use_one_persistent_receipt() {
    let (_guard, admin, db, store) = setup().await;
    prepare_directory(&admin).await;
    let directory = store.directory();
    let empty = directory.read(TENANT, PROJECT, A).await.unwrap();
    assert_eq!(empty["configured"], false);
    assert_eq!(empty["version"], "0");
    let preview = directory
        .preview(TENANT, PROJECT, A, &plan())
        .await
        .unwrap();
    assert_eq!(preview["expected_version"], "0");
    assert_eq!(preview["plan_digest"], plan().digest().unwrap());
    assert_eq!(
        directory.read(TENANT, PROJECT, A).await.unwrap()["version"],
        "0"
    );
    let input = request("register-directory", "0", plan());
    let first = directory.apply(TENANT, PROJECT, A, &input).await.unwrap();
    assert_eq!(first["replayed"], false);
    assert_eq!(first["receipt"]["version"], "1");
    assert_eq!(first["receipt"]["provenance"]["actor_id"], "agent");
    assert_eq!(first["receipt"]["provenance"]["client_id"], "cli-a");
    assert_eq!(first["receipt"]["access_grants_changed"], false);
    let reopened =
        WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let outcome = reopened
        .directory()
        .outcome(TENANT, PROJECT, A, &input.request_id)
        .await
        .unwrap();
    assert_eq!(outcome["outcome"], "recorded");
    assert_eq!(outcome["receipt"], first["receipt"]);
    let replay = reopened
        .directory()
        .apply(TENANT, PROJECT, A, &input)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], first["receipt"]);
    let observed = reopened.directory().read(TENANT, PROJECT, A).await.unwrap();
    let members = observed["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["display_name"], members[1]["display_name"]);
    assert_ne!(members[0]["member_ref"], members[1]["member_ref"]);
    assert_eq!(observed["version"], "1");
    assert_eq!(observed["cross_project_person_merging"], false);
    assert!(
        admin
            .batch_execute("DELETE FROM awr_team.project_directory_changes")
            .await
            .is_err()
    );
    assert!(
        admin
            .batch_execute("UPDATE awr_team.project_directory_changes SET result_json='{}'")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn two_previewed_versions_cannot_overwrite_each_other() {
    let (_guard, admin, _, store) = setup().await;
    prepare_directory(&admin).await;
    let directory = store.directory();
    let left = request("first-manager", "0", plan());
    let mut changed = plan();
    changed.departments[0].name = "Product engineering".into();
    let right = request("second-manager", "0", changed);
    let (left_result, right_result) = tokio::join!(
        directory.apply(TENANT, PROJECT, A, &left),
        directory.apply(TENANT, PROJECT, A, &right),
    );
    assert_ne!(left_result.is_ok(), right_result.is_ok());
    let failure = if left_result.is_err() {
        left_result.unwrap_err()
    } else {
        right_result.unwrap_err()
    };
    assert!(matches!(failure, PgError::PreconditionsChanged));
    assert_eq!(
        directory.read(TENANT, PROJECT, A).await.unwrap()["version"],
        "1"
    );
}

#[tokio::test]
async fn concurrent_replay_and_changed_intent_preserve_the_original_change() {
    let (_guard, admin, _, store) = setup().await;
    prepare_directory(&admin).await;
    let directory = store.directory();
    let input = request("same-request", "0", plan());
    let (first, second) = tokio::join!(
        directory.apply(TENANT, PROJECT, A, &input),
        directory.apply(TENANT, PROJECT, A, &input),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first["receipt"], second["receipt"]);
    assert_ne!(first["replayed"], second["replayed"]);
    let mut changed = plan();
    changed.organization.as_mut().unwrap().name = "Changed company".into();
    assert!(matches!(
        directory
            .apply(TENANT, PROJECT, A, &request("same-request", "0", changed))
            .await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(
        directory.read(TENANT, PROJECT, A).await.unwrap()["plan"]["organization"]["name"],
        "Example company"
    );
}

#[tokio::test]
async fn partial_scope_and_revoked_authority_cannot_read_write_or_replay() {
    let (_guard, admin, _, store) = setup().await;
    let directory = store.directory();
    assert!(matches!(
        directory.read(TENANT, PROJECT, A).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        directory.preview(TENANT, PROJECT, A, &plan()).await,
        Err(PgError::Forbidden)
    ));
    prepare_directory(&admin).await;
    let input = request("before-revocation", "0", plan());
    directory.apply(TENANT, PROJECT, A, &input).await.unwrap();
    assert!(matches!(
        directory.read(TENANT, PROJECT, B).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        directory.read("other-tenant", PROJECT, A).await,
        Err(PgError::Forbidden)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        directory.read(TENANT, PROJECT, A).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        directory.apply(TENANT, PROJECT, A, &input).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        directory
            .outcome(TENANT, PROJECT, A, &input.request_id)
            .await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn absent_or_departed_people_reject_the_entire_plan_and_stale_names_remain_explicit() {
    let (_guard, admin, _, store) = setup().await;
    prepare_directory(&admin).await;
    let directory = store.directory();
    let mut invalid = plan();
    invalid.members[1].person_id = "unregistered-person".into();
    assert!(matches!(
        directory.preview(TENANT, PROJECT, A, &invalid).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert!(matches!(
        directory
            .apply(TENANT, PROJECT, A, &request("missing-person", "0", invalid))
            .await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(
        directory.read(TENANT, PROJECT, A).await.unwrap()["version"],
        "0"
    );
    directory
        .apply(TENANT, PROJECT, A, &request("registered", "0", plan()))
        .await
        .unwrap();
    admin
        .batch_execute("UPDATE awr_team.persons SET status='departed' WHERE id='person-two'")
        .await
        .unwrap();
    let observed = directory.read(TENANT, PROJECT, A).await.unwrap();
    assert_eq!(observed["members"].as_array().unwrap().len(), 2);
    assert_eq!(observed["members"][1]["available"], false);
    assert_eq!(observed["members"][1]["display_name"], json!(null));
    assert!(matches!(
        directory
            .apply(TENANT, PROJECT, A, &request("after-departure", "1", plan()))
            .await,
        Err(PgError::PreconditionsChanged)
    ));
}

#[tokio::test]
async fn the_same_actor_and_member_names_keep_project_qualified_identity() {
    use awr_core::{Id, Workstream, WorkstreamCatalog, WorkstreamState};
    use awr_team::{SourceActivationPlan, WorkContract, WorkstreamBundle, WorkstreamContract};
    use awr_team_pg::{IngestRequest, SourceFile, SourceStore};
    let (_guard, admin, db, store) = setup().await;
    prepare_directory(&admin).await;
    store
        .directory()
        .apply(TENANT, PROJECT, A, &request("first-project", "0", plan()))
        .await
        .unwrap();
    let other = "second-project";
    admin.batch_execute("INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
        VALUES('reader-tenant','second-project','second','team','second-epoch','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','second-project','agent','admin'),('reader-tenant','second-project','reviewer','reviewer');
        INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
        VALUES('reader-tenant','second-project','main','Main','active');
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','second-project','person-one','Alex','active'),
              ('reader-tenant','second-project','person-two','Alex','active');").await.unwrap();
    let contract: WorkContract =
        serde_json::from_value(prepare(&store, A, "a").await["data"]["published_contract"].clone())
            .unwrap();
    let bundle = WorkstreamBundle {
        codec: WorkstreamBundle::CODEC.into(),
        catalog: WorkstreamCatalog {
            version: 1,
            project_id: other.into(),
            legacy_default: None,
            workstreams: vec![Workstream {
                id: Id::from(1),
                project_id: other.into(),
                external_key: "delivery".into(),
                title: "Delivery".into(),
                state: WorkstreamState::Active,
                authority_version: 1,
                goal_keys: vec![],
                acceptance_contracts: vec![],
            }],
        },
        contracts: vec![WorkstreamContract {
            workstream_id: Id::from(1),
            contract,
        }],
    };
    let source = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let candidate = source
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: other.into(),
            actor_id: "agent".into(),
            parser_version: "workstreams/1".into(),
            files: vec![SourceFile {
                path: "workstreams.json".into(),
                bytes: serde_json::to_vec(&bundle).unwrap(),
            }],
        })
        .await
        .unwrap();
    source
        .approve(
            TENANT,
            other,
            &candidate.proposal_id,
            "reviewer",
            &candidate.manifest_digest,
        )
        .await
        .unwrap();
    source
        .activate_workstreams(
            TENANT,
            other,
            "agent",
            &candidate.proposal_id,
            &SourceActivationPlan {
                candidate_digest: candidate.manifest_digest.clone(),
                parser_version: candidate.parser_version.clone(),
                expected_authority_epoch: candidate.base_epoch.clone(),
                approved_candidate_digest: candidate.manifest_digest.clone(),
            },
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_manage)
        VALUES($1,$2,'agent','cli-a',$3,1,true,true,true)", &[&TENANT,&other,&Id::from(1).to_string()]).await.unwrap();
    let mut second_plan = plan();
    second_plan.organization.as_mut().unwrap().id = "different-company".into();
    store
        .directory()
        .apply(
            TENANT,
            other,
            A,
            &request("second-project", "0", second_plan),
        )
        .await
        .unwrap();
    let first = store.directory().read(TENANT, PROJECT, A).await.unwrap();
    let second = store.directory().read(TENANT, other, A).await.unwrap();
    assert_eq!(
        first["plan"]["organization"]["name"],
        second["plan"]["organization"]["name"]
    );
    assert_ne!(
        first["plan"]["organization"]["id"],
        second["plan"]["organization"]["id"]
    );
    assert_eq!(
        first["members"][0]["display_name"],
        second["members"][0]["display_name"]
    );
    assert_eq!(
        first["members"][0]["member_ref"]["person_id"],
        second["members"][0]["member_ref"]["person_id"]
    );
    assert_ne!(
        first["members"][0]["member_ref"],
        second["members"][0]["member_ref"]
    );
    assert_eq!(
        first["provenance"]["actor_id"],
        second["provenance"]["actor_id"]
    );
}

#[tokio::test]
async fn a_human_supervisor_manages_grouping_with_live_scope_without_member_administration() {
    let (_guard, admin, _, store) = setup().await;
    prepare_directory(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET business_roles='[\"supervisor\"]'
        WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND actor_id='agent'",
        )
        .await
        .unwrap();
    let directory = store.directory();
    let preview = directory
        .preview(TENANT, PROJECT, A, &plan())
        .await
        .unwrap();
    assert_eq!(preview["expected_version"], "0");
    directory
        .apply(
            TENANT,
            PROJECT,
            A,
            &request("supervisor-grouping", "0", plan()),
        )
        .await
        .unwrap();
    assert_eq!(
        directory
            .outcome(TENANT, PROJECT, A, "supervisor-grouping")
            .await
            .unwrap()["outcome"],
        "recorded"
    );
    assert!(matches!(
        store
            .project_access()
            .members(TENANT, PROJECT, A, None, 50)
            .await,
        Err(PgError::Forbidden)
    ));
    admin
        .execute(
            "UPDATE awr_team.workstream_grants SET can_manage=false,grant_version=grant_version+1
        WHERE tenant_id=$1 AND project_id=$2 AND client_id='cli-a' AND workstream_id=$3",
            &[&TENANT, &PROJECT, &awr_core::Id::from(1).to_string()],
        )
        .await
        .unwrap();
    assert!(matches!(
        directory.preview(TENANT, PROJECT, A, &plan()).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        directory
            .outcome(TENANT, PROJECT, A, "supervisor-grouping")
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(
        directory.read(TENANT, PROJECT, A).await.unwrap()["version"],
        "1"
    );
    admin.execute("UPDATE awr_team.workstream_grants SET can_manage=true,authority_version=2,grant_version=grant_version+1
        WHERE tenant_id=$1 AND project_id=$2 AND client_id='cli-a' AND workstream_id=$3",
        &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    assert!(matches!(
        directory.preview(TENANT, PROJECT, A, &plan()).await,
        Err(PgError::Forbidden)
    ));
}
