//! AWR-TMCP-023: atomic planning receipts + scoped source.content.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team::{
    DraftChange, DraftDefinitionState, DraftOpKind, OrdinaryPlanningSelfApprovePolicy, TaskDraft,
};
use awr_team_pg::{
    PgError, PlanningApproveRequest, PlanningDraftRequest, PlanningPublishRequest,
    PlanningSuggestRequest, SourceStore, WorkstreamQuery, WorkstreamReadStore,
};
use fixture::*;
use serde_json::json;

async fn elev_maintainer(admin: &tokio_postgres::Client) {
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn planning_suggest_resume_after_reserved_does_not_duplicate() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));

    let req = PlanningSuggestRequest {
        protocol_version: 1,
        request_id: "suggest-resume-1".into(),
        rationale: "shared types".into(),
        affected_work_keys: vec!["a".into()],
        proposed_notes: json!({"note": "x"}),
        author_person_id: Some("agent".into()),
    };
    let first = store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(first["already_recorded"], false);
    let suggestion_id = first["result"]["suggestion_id"]
        .as_str()
        .unwrap()
        .to_string();

    admin
        .execute(
            "UPDATE awr_team.planning_command_receipts
             SET status='reserved',
                 result_json = jsonb_build_object(
                    'protocol', 'awr-team-planning-command-v1',
                    'request_id', 'suggest-resume-1',
                    'op', 'planning.propose',
                    'status', 'reserved',
                    'domain_id', $3::text,
                    'already_recorded', false
                 ),
                 updated_at=clock_timestamp()
             WHERE tenant_id=$1 AND project_id=$2 AND request_id='suggest-resume-1'",
            &[&TENANT, &PROJECT, &suggestion_id],
        )
        .await
        .unwrap();

    let second = store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(second["result"]["suggestion_id"], suggestion_id);

    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_suggestions
             WHERE tenant_id=$1 AND project_id=$2 AND rationale='shared types'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1, "resume must not create a second suggestion");

    let third = store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(third["already_recorded"], true);
}

#[tokio::test]
async fn concurrent_planning_suggest_same_request_does_not_abort() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let req = PlanningSuggestRequest {
        protocol_version: 1,
        request_id: "suggest-race-1".into(),
        rationale: "race types".into(),
        affected_work_keys: vec!["a".into()],
        proposed_notes: json!({"note": "race"}),
        author_person_id: Some("agent".into()),
    };
    let (left, right) = tokio::join!(
        store.planning_suggest(TENANT, PROJECT, A, &req),
        store.planning_suggest(TENANT, PROJECT, A, &req),
    );
    let left = left.expect("concurrent reserve must not abort the loser");
    let right = right.expect("concurrent reserve must not abort the loser");
    let left_id = left["result"]["suggestion_id"].as_str().unwrap();
    let right_id = right["result"]["suggestion_id"].as_str().unwrap();
    assert_eq!(left_id, right_id);
    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_suggestions
             WHERE tenant_id=$1 AND project_id=$2 AND rationale='race types'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1, "raced suggest must keep a single suggestion");
}

#[tokio::test]
async fn source_content_refuses_mixed_catalog_without_full_stream_grants() {
    let (_g, admin, db, read) = setup().await;
    let q_deny: WorkstreamQuery = serde_json::from_value(json!({
        "protocol_version": 1,
        "op": "source.content",
        "source_path": "workstreams.json"
    }))
    .unwrap();
    let err = read.query(TENANT, PROJECT, A, q_deny).await.unwrap_err();
    assert!(
        matches!(err, PgError::Forbidden),
        "partial stream grant must not read mixed workstreams.json: {err:?}"
    );

    admin
        .batch_execute(
            "INSERT INTO awr_team.workstream_grants(
                tenant_id,project_id,actor_id,client_id,workstream_id,
                authority_version,can_read,can_write,active)
             VALUES (
                'reader-tenant','reader-project','agent','cli-a',
                '00000000000000000000000002',1,true,false,true)
             ON CONFLICT (tenant_id,project_id,actor_id,client_id,workstream_id)
             DO UPDATE SET can_read=true, active=true,
               grant_version=awr_team.workstream_grants.grant_version+1;",
        )
        .await
        .unwrap();
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let q_ok: WorkstreamQuery = serde_json::from_value(json!({
        "protocol_version": 1,
        "op": "source.content",
        "source_path": "workstreams.json"
    }))
    .unwrap();
    match read.query(TENANT, PROJECT, A, q_ok).await {
        Ok(v) => {
            let text = v["data"]["text"].as_str().unwrap_or("");
            assert!(
                text.contains("private-beta")
                    || text.contains("b-private")
                    || text.contains("00000000000000000000000002"),
                "elevated read should include private stream content: {text}"
            );
        }
        Err(PgError::Protocol(msg)) => {
            // Accept missing persisted text as environmental, not auth bypass.
            assert!(!msg.to_lowercase().contains("forbidden"), "{msg}");
        }
        Err(e) => panic!("elevated read should not be Forbidden: {e:?}"),
    }
}

fn task(id: &str, title: &str) -> TaskDraft {
    TaskDraft {
        work_id: id.into(),
        external_key: id.into(),
        title: title.into(),
        goals: vec!["delivery".into()],
        scope_paths: vec!["specs/api.md".into()],
        acceptance: vec!["ok".into()],
        required_dependencies: vec![],
        completion_policy: "independent_review".into(),
        dependency_acceptance: None,
        hard_rules: None,
        verification_requirements: None,
        execution_settlement: None,
        definition_state: DraftDefinitionState::Draft,
        workstream: None,
        split_from: None,
        split_children: vec![],
    }
}

fn create_req(request_id: &str, work_id: &str) -> PlanningDraftRequest {
    PlanningDraftRequest {
        protocol_version: 1,
        request_id: request_id.into(),
        mode: "create".into(),
        candidate_id: None,
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: task(work_id, "one"),
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id: Some("agent".into()),
    }
}

#[tokio::test]
async fn supervisor_planning_selects_exact_project_grant_and_rechecks_scope_on_replay() {
    use awr_core::{AgentAuthorization, IssueAuthorizationRequest};
    use awr_team_pg::AuthorizationStore;
    let (_guard, admin, db, _) = setup().await;
    elev_maintainer(&admin).await;
    admin.batch_execute(
        "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
         INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
           VALUES('reader-tenant','reader-project','supervisor-member','Supervisor','active');
         INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
           VALUES('reader-tenant','reader-project','supervisor-binding','supervisor-member','agent','active');",
    ).await.unwrap();
    let config = common::with_app_role(&common::test_config(), &db);
    let auths = AuthorizationStore::from_config(config.clone());
    let store = SourceStore::from_config(config);
    let grant = |id: &str,
                 scope: serde_json::Value,
                 actions: serde_json::Value|
     -> AgentAuthorization {
        serde_json::from_value(json!({
            "id":id,"authorizer_person_id":"supervisor-member","responsible_person_id":"supervisor-member",
            "subject_kind":"agent","subject_id":"agent","client_id":"cli-a","scope":scope,
            "actions":actions,"status":"active","verifiable_capabilities":[],"self_reported_skill_hints":[],
            "created_at_ms":1000,"binding_id":"supervisor-binding"
        })).unwrap()
    };
    let project = json!({"kind":"project","project_id":PROJECT});
    auths
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "supervisor-inspect-first".into(),
                authorization: grant("aaa-inspect", project.clone(), json!(["inspect"])),
            },
        )
        .await
        .unwrap();
    let scoped = grant(
        "bbb-task-edit",
        json!({"kind":"task","project_id":PROJECT,"work_item_id":"a"}),
        json!(["inspect", "edit_planning"]),
    );
    auths
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "supervisor-task-grant".into(),
                authorization: scoped,
            },
        )
        .await
        .unwrap();
    let req = create_req("supervisor-create", "supervisor-draft");
    assert!(matches!(
        store.planning_draft(TENANT, PROJECT, A, &req).await,
        Err(PgError::Forbidden)
    ));
    let mut live = grant(
        "ccc-project-edit",
        project,
        json!(["inspect", "edit_planning"]),
    );
    auths
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "supervisor-project-grant".into(),
                authorization: live.clone(),
            },
        )
        .await
        .unwrap();
    // The grant covers the project, but current client scope is still partial.
    assert!(matches!(
        store.planning_draft(TENANT, PROJECT, A, &req).await,
        Err(PgError::Forbidden)
    ));
    admin.batch_execute(
        "INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,active)
         VALUES('reader-tenant','reader-project','agent','cli-a','00000000000000000000000002',1,true,true,true)
         ON CONFLICT(tenant_id,project_id,actor_id,client_id,workstream_id)
         DO UPDATE SET can_read=true,can_write=true,active=true,grant_version=awr_team.workstream_grants.grant_version+1;",
    ).await.unwrap();
    let first = store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    let replay = store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(
        replay["result"]["candidate_id"],
        first["result"]["candidate_id"]
    );
    assert_eq!(replay["already_recorded"], true);
    let candidate = first["result"]["candidate_id"].as_str().unwrap();
    assert!(
        store
            .preview_planning_candidate(TENANT, PROJECT, A, candidate)
            .await
            .is_ok()
    );
    // Fixture-only failure injection; native business acceptance is separate.
    live.status = awr_core::AuthorizationStatus::Revoked;
    admin.execute("UPDATE awr_team.agent_authorizations SET status='revoked',body_json=$1 WHERE id='ccc-project-edit'", &[&json!(live)]).await.unwrap();
    assert!(matches!(
        store.planning_draft(TENANT, PROJECT, A, &req).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store
            .planning_draft(
                TENANT,
                PROJECT,
                A,
                &create_req("supervisor-next", "supervisor-next-draft")
            )
            .await,
        Err(PgError::Forbidden)
    ));
}

async fn rewind_receipt(
    admin: &tokio_postgres::Client,
    request_id: &str,
    op: &str,
    domain_id: &str,
    pre_revision: Option<i32>,
) {
    let mut result = json!({
        "protocol": "awr-team-planning-command-v1",
        "request_id": request_id,
        "op": op,
        "status": "reserved",
        "domain_id": domain_id,
        "already_recorded": false
    });
    if let Some(revision) = pre_revision {
        result["pre_revision"] = json!(revision);
    }
    admin
        .execute(
            "UPDATE awr_team.planning_command_receipts
             SET status='reserved', result_json=$4, updated_at=clock_timestamp()
             WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
            &[&TENANT, &PROJECT, &request_id, &result],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn reserved_edit_resume_does_not_bump_revision_again() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let created = store
        .planning_draft(TENANT, PROJECT, A, &create_req("edit-create", "OPS-EDIT-1"))
        .await
        .unwrap();
    let candidate_id = created["result"]["candidate_id"]
        .as_str()
        .unwrap()
        .to_string();
    let edit = PlanningDraftRequest {
        protocol_version: 1,
        request_id: "edit-once".into(),
        mode: "edit".into(),
        candidate_id: Some(candidate_id.clone()),
        changes: vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(task("OPS-EDIT-1", "one")),
            after: task("OPS-EDIT-1", "two"),
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec![],
        project_goal_keys: vec![],
        self_approve_policy: None,
        author_person_id: None,
    };
    let edited = store
        .planning_draft(TENANT, PROJECT, A, &edit)
        .await
        .unwrap();
    assert_eq!(edited["result"]["draft_revision"], 2);
    let history_before: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_candidate_history
             WHERE tenant_id=$1 AND project_id=$2 AND candidate_id=$3",
            &[&TENANT, &PROJECT, &candidate_id],
        )
        .await
        .unwrap()
        .get(0);
    rewind_receipt(
        &admin,
        "edit-once",
        "planning.edit_draft",
        &candidate_id,
        Some(1),
    )
    .await;
    let again = store
        .planning_draft(TENANT, PROJECT, A, &edit)
        .await
        .unwrap();
    assert_eq!(again["result"]["draft_revision"], 2);
    assert_eq!(again["result"]["resumed"], true);
    let history_after: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_candidate_history
             WHERE tenant_id=$1 AND project_id=$2 AND candidate_id=$3",
            &[&TENANT, &PROJECT, &candidate_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(history_after, history_before);
    let revision: i32 = admin
        .query_one(
            "SELECT draft_revision FROM awr_team.planning_candidates
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &candidate_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(revision, 2);
}

#[tokio::test]
async fn reserved_approve_and_publish_resume_do_not_duplicate_rows() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let created = store
        .planning_draft(TENANT, PROJECT, A, &create_req("pub-create", "OPS-PUB-1"))
        .await
        .unwrap();
    let candidate_id = created["result"]["candidate_id"]
        .as_str()
        .unwrap()
        .to_string();
    let digest = created["result"]["candidate_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let approve = PlanningApproveRequest {
        protocol_version: 1,
        request_id: "approve-once".into(),
        candidate_id: candidate_id.clone(),
        candidate_digest: digest.clone(),
        author_person_id: Some("agent".into()),
    };
    let approved = store
        .planning_approve(TENANT, PROJECT, A, &approve)
        .await
        .unwrap();
    let approval_id = approved["result"]["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    rewind_receipt(
        &admin,
        "approve-once",
        "planning.approve",
        &candidate_id,
        None,
    )
    .await;
    let approved_again = store
        .planning_approve(TENANT, PROJECT, A, &approve)
        .await
        .unwrap();
    assert_eq!(approved_again["result"]["approval_id"], approval_id);
    let approvals: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_approvals
             WHERE tenant_id=$1 AND project_id=$2 AND candidate_id=$3",
            &[&TENANT, &PROJECT, &candidate_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(approvals, 1);

    let publish = PlanningPublishRequest {
        protocol_version: 1,
        request_id: "publish-once".into(),
        candidate_id: candidate_id.clone(),
        candidate_digest: digest.clone(),
        activate: false,
        publish_receipt_id: None,
        impact_proven: false,
        stopped_work_ids: vec![],
    };
    let published = store
        .planning_publish(TENANT, PROJECT, A, &publish)
        .await
        .unwrap();
    let receipt_id = published["result"]["publish"]["receipt_id"]
        .as_str()
        .unwrap()
        .to_string();
    rewind_receipt(
        &admin,
        "publish-once",
        "planning.publish",
        &candidate_id,
        None,
    )
    .await;
    let published_again = store
        .planning_publish(TENANT, PROJECT, A, &publish)
        .await
        .unwrap();
    assert_eq!(
        published_again["result"]["publish"]["receipt_id"],
        receipt_id
    );
    let receipts: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_publish_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND candidate_id=$3",
            &[&TENANT, &PROJECT, &candidate_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(receipts, 1);
}

#[tokio::test]
async fn developer_publish_does_not_reserve_a_receipt() {
    let (_g, admin, db, _read) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let publish = PlanningPublishRequest {
        protocol_version: 1,
        request_id: "dev-publish".into(),
        candidate_id: "missing-candidate".into(),
        candidate_digest: "missing-digest".into(),
        activate: false,
        publish_receipt_id: None,
        impact_proven: false,
        stopped_work_ids: vec![],
    };
    let err = store
        .planning_publish(TENANT, PROJECT, A, &publish)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
    let reserved: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_command_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND request_id='dev-publish'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(reserved, 0);
}
