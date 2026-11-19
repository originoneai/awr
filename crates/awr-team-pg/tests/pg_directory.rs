#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team::{DirectoryDepartment, DirectoryMember, DirectoryUnit, ProjectDirectoryPlan};
use awr_team_pg::{DirectoryApplyRequest, PgError, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

fn plan() -> ProjectDirectoryPlan {
    ProjectDirectoryPlan {
        organization: Some(DirectoryUnit {
            id: "example".into(),
            name: "Example company".into(),
        }),
        departments: vec![DirectoryDepartment {
            id: "engineering".into(),
            name: "Engineering".into(),
            parent_id: None,
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

/// A four-level chain whose leaf ("scheduling") is the group the project and
/// its members attach under.
fn hierarchical_plan() -> ProjectDirectoryPlan {
    let chain = [
        ("engineering", "Engineering", None),
        ("platform", "Platform", Some("engineering")),
        ("runtime", "Runtime", Some("platform")),
        ("scheduling", "Scheduling", Some("runtime")),
    ];
    ProjectDirectoryPlan {
        organization: Some(DirectoryUnit {
            id: "example".into(),
            name: "Example company".into(),
        }),
        departments: chain
            .into_iter()
            .map(|(id, name, parent)| DirectoryDepartment {
                id: id.into(),
                name: name.into(),
                parent_id: parent.map(str::to_owned),
            })
            .collect(),
        project_department_ids: vec!["scheduling".into()],
        members: vec![DirectoryMember {
            person_id: "person-one".into(),
            department_ids: vec!["scheduling".into()],
        }],
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

#[tokio::test]
async fn hierarchical_plans_round_trip_and_report_depth_and_leaves_in_the_summary() {
    let (_guard, admin, _, store) = setup().await;
    prepare_directory(&admin).await;
    let directory = store.directory();
    let preview = directory
        .preview(TENANT, PROJECT, A, &hierarchical_plan())
        .await
        .unwrap();
    assert_eq!(preview["expected_version"], "0");
    assert_eq!(
        preview["plan_digest"],
        hierarchical_plan().digest().unwrap()
    );
    assert_eq!(
        preview["after"]["departments"][3]["parent_id"], "runtime",
        "the previewed plan keeps the proposed hierarchy"
    );
    let input = request("hierarchy-v1", "0", hierarchical_plan());
    let applied = directory.apply(TENANT, PROJECT, A, &input).await.unwrap();
    assert_eq!(applied["receipt"]["version"], "1");
    let observed = directory.read(TENANT, PROJECT, A).await.unwrap();
    assert_eq!(observed["version"], "1");
    let departments = observed["plan"]["departments"].as_array().unwrap();
    assert_eq!(departments.len(), 4);
    assert_eq!(departments[0]["parent_id"], json!(null));
    assert_eq!(departments[1]["parent_id"], "engineering");
    assert_eq!(departments[2]["parent_id"], "platform");
    assert_eq!(departments[3]["parent_id"], "runtime");
    assert_eq!(
        observed["plan"]["project_department_ids"],
        json!(["scheduling"]),
        "the project is attached under the leaf group"
    );
    assert_eq!(
        observed["members"][0]["department_ids"],
        json!(["scheduling"])
    );
    assert_eq!(observed["members"][0]["display_name"], "Alex");
    // The audit summary of the apply carries the hierarchy fields.
    let summary: Value = admin
        .query_one(
            "SELECT summary_json FROM awr_team.ops_audit_records
             WHERE tenant_id=$1 AND project_id=$2 AND action='directory.manage_project'
             ORDER BY created_at DESC LIMIT 1",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(summary["department_count"], 4);
    assert_eq!(summary["member_count"], 1);
    assert_eq!(summary["department_depth"], 4);
    assert_eq!(summary["leaf_department_count"], 1);
    // A flat apply summarizes depth 1 with every department itself a leaf.
    directory
        .apply(TENANT, PROJECT, A, &request("flatten-v2", "1", plan()))
        .await
        .unwrap();
    let summary: Value = admin
        .query_one(
            "SELECT summary_json FROM awr_team.ops_audit_records
             WHERE tenant_id=$1 AND project_id=$2 AND action='directory.manage_project'
             ORDER BY created_at DESC LIMIT 1",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(summary["department_depth"], 1);
    assert_eq!(summary["leaf_department_count"], 1);
}

#[tokio::test]
async fn hierarchy_faults_are_rejected_without_touching_the_directory() {
    let (_guard, admin, _, store) = setup().await;
    prepare_directory(&admin).await;
    let directory = store.directory();
    let mut cycle = hierarchical_plan();
    cycle.departments[0].parent_id = Some("scheduling".into());
    let mut too_deep = hierarchical_plan();
    too_deep.departments.push(DirectoryDepartment {
        id: "fair-share".into(),
        name: "Fair share".into(),
        parent_id: Some("scheduling".into()),
    });
    let mut sibling_rename = hierarchical_plan();
    sibling_rename.departments.push(DirectoryDepartment {
        id: "observability".into(),
        name: " Scheduling ".into(),
        parent_id: Some("runtime".into()),
    });
    // Two top-level departments whose names differ only by surrounding
    // whitespace collide in the one top-level sibling group.
    let mut top_level_rename = hierarchical_plan();
    top_level_rename.departments.push(DirectoryDepartment {
        id: "product".into(),
        name: " Engineering ".into(),
        parent_id: None,
    });
    let mut unknown_parent = hierarchical_plan();
    unknown_parent.departments[1].parent_id = Some("unregistered".into());
    for (label, broken) in [
        ("cycle", cycle),
        ("too deep", too_deep),
        ("sibling rename", sibling_rename),
        ("top-level rename", top_level_rename),
        ("unknown parent", unknown_parent),
    ] {
        assert!(
            matches!(
                directory.preview(TENANT, PROJECT, A, &broken).await,
                Err(PgError::Protocol(_))
            ),
            "{label} must fail preview with the existing invalid-input category"
        );
        // A placeholder digest: an invalid plan is refused while normalizing,
        // before any digest comparison, so the value never takes effect.
        let refused = DirectoryApplyRequest {
            protocol_version: 1,
            request_id: format!("fault-{label}"),
            expected_version: "0".into(),
            expected_plan_digest: "0".repeat(64),
            plan: broken,
        };
        assert!(
            matches!(
                directory.apply(TENANT, PROJECT, A, &refused).await,
                Err(PgError::Protocol(_))
            ),
            "{label} must fail apply with the existing invalid-input category"
        );
    }
    let observed = directory.read(TENANT, PROJECT, A).await.unwrap();
    assert_eq!(observed["version"], "0");
    assert_eq!(observed["configured"], false);
    let written: i64 = admin
        .query_one(
            "SELECT COUNT(*) FROM awr_team.ops_audit_records
             WHERE tenant_id=$1 AND project_id=$2 AND action='directory.manage_project'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(written, 0, "rejected plans record no directory audit row");
    let stored: i64 = admin
        .query_one(
            "SELECT COUNT(*) FROM awr_team.project_directories WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn directories_recorded_without_the_hierarchy_field_stay_readable_and_upgradable() {
    let (_guard, admin, _, store) = setup().await;
    prepare_directory(&admin).await;
    // A directory exactly as the pre-hierarchy service stored it.
    admin
        .execute(
            "INSERT INTO awr_team.project_directories(tenant_id,project_id,version,plan_json,provenance)
             VALUES($1,$2,1,$3,'{}'::jsonb)",
            &[
                &TENANT,
                &PROJECT,
                &json!({"organization":{"id":"example","name":"Example company"},
                    "departments":[{"id":"engineering","name":"Engineering"}],
                    "project_department_ids":["engineering"],
                    "members":[{"person_id":"person-one","department_ids":["engineering"]}]}),
            ],
        )
        .await
        .unwrap();
    let directory = store.directory();
    let legacy = directory.read(TENANT, PROJECT, A).await.unwrap();
    assert_eq!(legacy["configured"], true);
    assert_eq!(legacy["version"], "1");
    assert_eq!(
        legacy["plan"]["departments"][0].get("parent_id"),
        None,
        "an absent parent reference means top level, not an error"
    );
    assert_eq!(legacy["members"][0]["display_name"], "Alex");
    assert_eq!(legacy["members"][0]["available"], true);
    // A directory the older rules allowed: two same-named top-level
    // departments. Reading keeps working; previewing the plan unchanged is
    // refused until one of the two is renamed.
    admin
        .execute(
            "UPDATE awr_team.project_directories SET plan_json=$3
             WHERE tenant_id=$1 AND project_id=$2",
            &[
                &TENANT,
                &PROJECT,
                &json!({"organization":{"id":"example","name":"Example company"},
                    "departments":[{"id":"engineering","name":"Engineering"},
                        {"id":"product","name":"Engineering"}],
                    "project_department_ids":["engineering","product"],
                    "members":[{"person_id":"person-one","department_ids":["engineering"]}]}),
            ],
        )
        .await
        .unwrap();
    let duplicated = directory.read(TENANT, PROJECT, A).await.unwrap();
    assert_eq!(
        duplicated["plan"]["departments"].as_array().unwrap().len(),
        2
    );
    let unchanged: ProjectDirectoryPlan =
        serde_json::from_value(duplicated["plan"].clone()).unwrap();
    assert!(matches!(
        directory.preview(TENANT, PROJECT, A, &unchanged).await,
        Err(PgError::Protocol(_))
    ));
    // The old directory upgrades to a hierarchy through the normal flow.
    let preview = directory
        .preview(TENANT, PROJECT, A, &hierarchical_plan())
        .await
        .unwrap();
    assert_eq!(preview["expected_version"], "1");
    directory
        .apply(
            TENANT,
            PROJECT,
            A,
            &request("upgrade-to-hierarchy", "1", hierarchical_plan()),
        )
        .await
        .unwrap();
    let upgraded = directory.read(TENANT, PROJECT, A).await.unwrap();
    assert_eq!(upgraded["version"], "2");
    assert_eq!(upgraded["plan"]["departments"][3]["parent_id"], "runtime");
    // A flat apply keeps storing plans without parent references at all.
    directory
        .apply(TENANT, PROJECT, A, &request("back-to-flat", "2", plan()))
        .await
        .unwrap();
    let flat: bool = admin
        .query_one(
            "SELECT NOT (plan_json->'departments'->0 ? 'parent_id')
             FROM awr_team.project_directories WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert!(flat);
}
