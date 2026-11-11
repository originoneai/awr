//! AWR-TMCP-023: atomic planning receipts + scoped source.content.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team::{
    DraftChange, DraftDefinitionState, DraftOpKind, OrdinaryPlanningSelfApprovePolicy, TaskDraft,
};
use awr_team_pg::{
    DraftCandidateCreate, PgError, PlanningApproveRequest, PlanningDraftRequest,
    PlanningPublishRequest, PlanningSuggestRequest, SourceStore, WorkstreamQuery,
    WorkstreamReadStore,
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
async fn stale_approval_records_known_rejection_and_recovers_on_fresh_draft() {
    let (_guard, admin, db, read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let created = store
        .planning_draft(
            TENANT,
            PROJECT,
            A,
            &create_req("before-source-change", "RECOVER-PLAN"),
        )
        .await
        .unwrap();
    let candidate_id = created["result"]["candidate_id"].as_str().unwrap();
    let candidate_digest = created["result"]["candidate_digest"].as_str().unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.source_snapshots
             SET manifest_digest='sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
             WHERE tenant_id='reader-tenant' AND project_id='reader-project'
               AND id=(SELECT active_snapshot_id FROM awr_team.projects
                       WHERE tenant_id='reader-tenant' AND id='reader-project')",
        )
        .await
        .unwrap();
    let request = PlanningApproveRequest {
        protocol_version: 1,
        request_id: "approve-stale-plan".into(),
        candidate_id: candidate_id.into(),
        candidate_digest: candidate_digest.into(),
        author_person_id: Some("agent".into()),
    };
    let (left, right) = tokio::join!(
        store.planning_approve(TENANT, PROJECT, A, &request),
        store.planning_approve(TENANT, PROJECT, A, &request),
    );
    let first = left.expect("a verified pre-write refusal must have a durable outcome");
    let replay = right.unwrap();
    assert_eq!(first["result"]["outcome"], "known_rejection");
    assert_eq!(first["result"]["code"], "PlanningBaselineChanged");
    assert_eq!(first["result"]["approval_written"], false);
    assert_eq!(
        first["result"]["guidance"]["action"]["op"],
        "planning.preview"
    );
    assert_eq!(
        first["result"]["guidance"]["action"]["candidate_id"],
        candidate_id
    );
    assert!(first["result"].to_string().len() < 2000);
    assert_eq!(first["result"], replay["result"]);
    assert_eq!(first["request_hash"], replay["request_hash"]);
    let state = admin
        .query_one(
            "SELECT state,(SELECT count(*) FROM awr_team.planning_approvals WHERE candidate_id=$1)
             FROM awr_team.planning_candidates WHERE id=$1",
            &[&candidate_id],
        )
        .await
        .unwrap();
    assert_eq!(state.get::<_, String>(0), "drafting");
    assert_eq!(state.get::<_, i64>(1), 0);
    let query: WorkstreamQuery = serde_json::from_value(json!({
        "protocol_version":1,"op":"planning.outcome","request_id":request.request_id
    }))
    .unwrap();
    let observed = read.query(TENANT, PROJECT, A, query).await.unwrap();
    assert_eq!(observed["data"]["already_recorded"], true);
    assert_eq!(observed["data"]["result"]["result"], first["result"]);
    let mut changed = request.clone();
    changed.candidate_digest = "b".repeat(64);
    assert!(matches!(
        store.planning_approve(TENANT, PROJECT, A, &changed).await,
        Err(PgError::IdempotencyConflict)
    ));

    // A new draft binds the current source; the rejected attempt cannot approve it.
    let fresh = store
        .planning_draft(
            TENANT,
            PROJECT,
            A,
            &create_req("after-source-change", "RECOVER-PLAN"),
        )
        .await
        .unwrap();
    let fresh_id = fresh["result"]["candidate_id"].as_str().unwrap();
    let fresh_request = PlanningApproveRequest {
        request_id: "approve-current-plan".into(),
        candidate_id: fresh_id.into(),
        candidate_digest: fresh["result"]["candidate_digest"].as_str().unwrap().into(),
        ..request.clone()
    };
    let approved = store
        .planning_approve(TENANT, PROJECT, A, &fresh_request)
        .await
        .unwrap();
    assert_eq!(approved["result"]["state"], "approved");
    assert!(approved["result"]["approval_id"].is_string());

    // Lost response after an actual approval still recovers that proof, even
    // when a later source change would prevent another approval now.
    rewind_receipt(
        &admin,
        &fresh_request.request_id,
        "planning.approve",
        fresh_id,
        None,
    )
    .await;
    admin
        .batch_execute(
            "UPDATE awr_team.source_snapshots
             SET manifest_digest='sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
             WHERE tenant_id='reader-tenant' AND project_id='reader-project'
               AND id=(SELECT active_snapshot_id FROM awr_team.projects
                       WHERE tenant_id='reader-tenant' AND id='reader-project')",
        )
        .await
        .unwrap();
    let recovered = store
        .planning_approve(TENANT, PROJECT, A, &fresh_request)
        .await
        .unwrap();
    assert_eq!(
        recovered["result"]["approval_id"],
        approved["result"]["approval_id"]
    );
    assert_eq!(recovered["result"]["state"], "approved");
    let old = store
        .planning_approve(TENANT, PROJECT, A, &request)
        .await
        .unwrap();
    assert_eq!(old["result"], first["result"]);
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

async fn receipt_count(admin: &tokio_postgres::Client, request: &str) -> i64 {
    admin.query_one(
        "SELECT count(*) FROM awr_team.planning_command_receipts WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
        &[&TENANT, &PROJECT, &request],
    ).await.unwrap().get(0)
}

#[tokio::test]
async fn invalid_author_does_not_consume_suggest_draft_or_approve_request_ids() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut suggest = PlanningSuggestRequest {
        protocol_version: 1,
        request_id: "correct-suggest".into(),
        rationale: "Share an API contract".into(),
        affected_work_keys: vec!["a".into()],
        proposed_notes: json!({}),
        author_person_id: Some("responsible-member".into()),
    };
    assert!(
        store
            .planning_suggest(TENANT, PROJECT, A, &suggest)
            .await
            .is_err()
    );
    assert_eq!(receipt_count(&admin, &suggest.request_id).await, 0);
    suggest.author_person_id = None;
    store
        .planning_suggest(TENANT, PROJECT, A, &suggest)
        .await
        .unwrap();
    let mut req = create_req("correct-draft", "API-CONTRACT");
    req.author_person_id = Some("responsible-member".into());
    assert!(
        store
            .planning_draft(TENANT, PROJECT, A, &req)
            .await
            .is_err()
    );
    assert_eq!(receipt_count(&admin, &req.request_id).await, 0);
    let candidates: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.planning_candidates", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(candidates, 0);
    req.author_person_id = None;
    let candidate = store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    let mut approval = PlanningApproveRequest {
        protocol_version: 1,
        request_id: "correct-approve".into(),
        candidate_id: candidate["result"]["candidate_id"].as_str().unwrap().into(),
        candidate_digest: candidate["result"]["candidate_digest"]
            .as_str()
            .unwrap()
            .into(),
        author_person_id: Some("responsible-member".into()),
    };
    assert!(
        store
            .planning_approve(TENANT, PROJECT, A, &approval)
            .await
            .is_err()
    );
    assert_eq!(receipt_count(&admin, &approval.request_id).await, 0);
    let approvals: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.planning_approvals", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(approvals, 0);
    approval.author_person_id = None;
    store
        .planning_approve(TENANT, PROJECT, A, &approval)
        .await
        .unwrap();
}

#[tokio::test]
async fn invalid_draft_definition_is_correctable_before_reservation() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut req = create_req("correct-definition", "SUPPORT-FILTER");
    req.changes[0].after.goals = vec!["support-queue".into()];
    assert!(
        store
            .planning_draft(TENANT, PROJECT, A, &req)
            .await
            .is_err()
    );
    assert_eq!(receipt_count(&admin, &req.request_id).await, 0);
    req.project_goal_keys = vec!["support-queue".into()];
    let created = store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    let mut edit = req.clone();
    edit.request_id = "correct-edit".into();
    edit.mode = "edit".into();
    edit.candidate_id = Some(created["result"]["candidate_id"].as_str().unwrap().into());
    edit.changes[0].op = DraftOpKind::EditFields;
    edit.changes[0].before = Some(req.changes[0].after.clone());
    edit.changes[0].after.scope_paths = vec!["../outside.md".into()];
    assert!(
        store
            .planning_draft(TENANT, PROJECT, A, &edit)
            .await
            .is_err()
    );
    assert_eq!(receipt_count(&admin, &edit.request_id).await, 0);
    edit.changes[0].after.scope_paths = vec!["specs/filter.md".into()];
    let edited = store
        .planning_draft(TENANT, PROJECT, A, &edit)
        .await
        .unwrap();
    assert_eq!(edited["result"]["draft_revision"], 2);
    let mut changed = edit.clone();
    changed.changes[0].after.title = "different intent".into();
    assert!(matches!(
        store.planning_draft(TENANT, PROJECT, A, &changed).await,
        Err(PgError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn reserved_outcome_exposes_pending_state_without_claiming_an_effect() {
    let (_g, admin, db, read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let req = create_req("reserved-visible", "VISIBLE-DRAFT");
    let created = store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    rewind_receipt(
        &admin,
        &req.request_id,
        "planning.edit_draft",
        created["result"]["candidate_id"].as_str().unwrap(),
        None,
    )
    .await;
    let pending = store
        .get_planning_command_receipt(TENANT, PROJECT, A, &req.request_id)
        .await
        .unwrap()
        .expect("a reservation is distinct from an absent receipt");
    assert_eq!(pending["status"], "reserved");
    assert_eq!(pending["already_recorded"], false);
    assert!(pending["result"].is_null());
    assert!(pending.get("domain_id").is_none());
    let query: WorkstreamQuery = serde_json::from_value(
        json!({"protocol_version":1,"op":"planning.outcome","request_id":req.request_id}),
    )
    .unwrap();
    assert_eq!(
        read.query(TENANT, PROJECT, A, query).await.unwrap()["data"],
        pending
    );
    assert!(
        store
            .get_planning_command_receipt(TENANT, PROJECT, A, "never-submitted")
            .await
            .unwrap()
            .is_none()
    );
    let mut changed = req.clone();
    changed.author_person_id = None;
    assert!(matches!(
        store.planning_draft(TENANT, PROJECT, A, &changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(
        store
            .get_planning_command_receipt(TENANT, PROJECT, A, &req.request_id)
            .await
            .unwrap()
            .unwrap()["already_recorded"],
        true
    );
}

// AWR-TFQ-023: a create that names an existing candidate_id resumes only the same creation.

/// Stored facts of a candidate that a refused create must leave untouched.
async fn candidate_row(admin: &tokio_postgres::Client, candidate_id: &str) -> serde_json::Value {
    let row = admin
        .query_one(
            "SELECT changes_json, draft_revision, candidate_digest, state, updated_at::text,
                    suggestion_ids, allowed_spec_roots, project_goal_keys, self_approve_policy_json
             FROM awr_team.planning_candidates
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &candidate_id],
        )
        .await
        .unwrap();
    json!({
        "changes_json": row.get::<_, serde_json::Value>(0),
        "draft_revision": row.get::<_, i32>(1),
        "candidate_digest": row.get::<_, String>(2),
        "state": row.get::<_, String>(3),
        "updated_at": row.get::<_, String>(4),
        "suggestion_ids": row.get::<_, serde_json::Value>(5),
        "allowed_spec_roots": row.get::<_, serde_json::Value>(6),
        "project_goal_keys": row.get::<_, serde_json::Value>(7),
        "self_approve_policy_json": row.get::<_, serde_json::Value>(8),
    })
}

/// Row counts of every table a planning create writes.
async fn planning_write_counts(admin: &tokio_postgres::Client) -> Vec<i64> {
    let mut counts = Vec::new();
    for table in [
        "planning_candidates",
        "planning_candidate_history",
        "planning_command_receipts",
        "ops_audit_records",
        "events",
    ] {
        let count: i64 = admin
            .query_one(&format!("SELECT count(*) FROM awr_team.{table}"), &[])
            .await
            .unwrap()
            .get(0);
        counts.push(count);
    }
    counts
}

fn create_with_candidate(
    request_id: &str,
    candidate_id: &str,
    work_id: &str,
) -> PlanningDraftRequest {
    let mut req = create_req(request_id, work_id);
    req.candidate_id = Some(candidate_id.into());
    req
}

fn assert_candidate_exists_refusal(
    err: &PgError,
    existing: &serde_json::Value,
    candidate_id: &str,
    differs_in: serde_json::Value,
) {
    let diagnostic = err
        .planning_input_diagnostic()
        .unwrap_or_else(|| panic!("expected the candidate-exists diagnostic, got {err:?}"));
    assert_eq!(diagnostic["code"], "InvalidInput");
    assert_eq!(
        diagnostic["reason"],
        "candidate_exists_with_different_request"
    );
    assert_eq!(diagnostic["invalid_field"], "/candidate_id");
    assert_eq!(diagnostic["request_reserved"], false);
    assert_eq!(diagnostic["candidate_changed"], false);
    assert_eq!(diagnostic["differs_in"], differs_in);
    assert_eq!(diagnostic["candidate_id"], candidate_id);
    assert_eq!(diagnostic["draft_revision"], existing["draft_revision"]);
    assert_eq!(diagnostic["candidate_digest"], existing["candidate_digest"]);
    assert_eq!(diagnostic["state"], existing["state"]);
    let next = diagnostic["next_step"].as_str().unwrap();
    assert!(
        next.contains("mode edit") && next.contains(candidate_id),
        "{next}"
    );
    assert!(
        next.contains("new candidate_id") && next.contains("was not changed"),
        "{next}"
    );
}

#[tokio::test]
async fn create_reusing_a_candidate_id_with_different_changes_is_refused_before_any_write() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let first = store
        .planning_draft(
            TENANT,
            PROJECT,
            A,
            &create_with_candidate("reuse-create-1", "cand-reuse", "OPS-REUSE-1"),
        )
        .await
        .unwrap();
    assert_eq!(first["result"]["candidate_id"], "cand-reuse");
    let before = candidate_row(&admin, "cand-reuse").await;
    let counts = planning_write_counts(&admin).await;

    // A supervisor reuses the candidate id with different changes under a new request id.
    let reused = create_with_candidate("reuse-create-2", "cand-reuse", "OPS-REUSE-2");
    let outcome = store.planning_draft(TENANT, PROJECT, A, &reused).await;
    assert_eq!(
        candidate_row(&admin, "cand-reuse").await,
        before,
        "the stored candidate keeps its original changes"
    );
    assert_eq!(
        planning_write_counts(&admin).await,
        counts,
        "a refused create writes no candidate, history, receipt, audit or event row"
    );
    let err = outcome.expect_err("a different create on an existing candidate id must be refused");
    assert_candidate_exists_refusal(&err, &before, "cand-reuse", json!(["changes"]));

    // The refused request id stays unused: the same id with a new candidate id creates it.
    let mut fresh = reused.clone();
    fresh.candidate_id = None;
    let created = store
        .planning_draft(TENANT, PROJECT, A, &fresh)
        .await
        .unwrap();
    assert_ne!(created["result"]["candidate_id"], "cand-reuse");
    assert!(created["result"].get("resumed").is_none());

    // The domain entry refuses the same way for an unbound caller.
    let direct = reused.clone();
    let direct = DraftCandidateCreate {
        changes: direct.changes,
        suggestion_ids: direct.suggestion_ids,
        allowed_spec_roots: direct.allowed_spec_roots,
        project_goal_keys: direct.project_goal_keys,
        self_approve_policy: direct.self_approve_policy,
        author_person_id: direct.author_person_id,
        predetermined_candidate_id: Some("cand-reuse".into()),
    };
    let counts = planning_write_counts(&admin).await;
    let err = store
        .create_planning_candidate(TENANT, PROJECT, A, &direct)
        .await
        .expect_err("the domain create refuses a different creation too");
    assert_candidate_exists_refusal(&err, &before, "cand-reuse", json!(["changes"]));
    assert_eq!(candidate_row(&admin, "cand-reuse").await, before);
    assert_eq!(planning_write_counts(&admin).await, counts);
}

#[tokio::test]
async fn create_reusing_a_candidate_id_with_other_creation_parameters_is_refused() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    store
        .planning_draft(
            TENANT,
            PROJECT,
            A,
            &create_with_candidate("params-create-1", "cand-params", "OPS-PARAMS-1"),
        )
        .await
        .unwrap();
    let before = candidate_row(&admin, "cand-params").await;
    let counts = planning_write_counts(&admin).await;

    let mut roots = create_with_candidate("params-create-2", "cand-params", "OPS-PARAMS-1");
    roots.allowed_spec_roots = vec!["specs".into(), "docs".into()];
    let err = store
        .planning_draft(TENANT, PROJECT, A, &roots)
        .await
        .expect_err("different spec roots are a different creation");
    assert_candidate_exists_refusal(&err, &before, "cand-params", json!(["allowed_spec_roots"]));

    let mut policy = create_with_candidate("params-create-3", "cand-params", "OPS-PARAMS-2");
    let mut other_policy = OrdinaryPlanningSelfApprovePolicy::ordinary_default();
    other_policy.allow_self_approve_ordinary = !other_policy.allow_self_approve_ordinary;
    policy.self_approve_policy = Some(other_policy);
    let err = store
        .planning_draft(TENANT, PROJECT, A, &policy)
        .await
        .expect_err("a different policy and changes are a different creation");
    assert_candidate_exists_refusal(
        &err,
        &before,
        "cand-params",
        json!(["changes", "self_approve_policy"]),
    );
    assert_eq!(candidate_row(&admin, "cand-params").await, before);
    assert_eq!(planning_write_counts(&admin).await, counts);

    // Omitted goals and policy resolve to the stored defaults, so this is the same creation.
    let mut defaults = create_with_candidate("params-create-4", "cand-params", "OPS-PARAMS-1");
    defaults.project_goal_keys = vec![];
    defaults.self_approve_policy = None;
    defaults.author_person_id = None;
    let resumed = store
        .planning_draft(TENANT, PROJECT, A, &defaults)
        .await
        .unwrap();
    assert_eq!(resumed["result"]["resumed"], true);
    assert_eq!(
        resumed["result"]["candidate_digest"],
        before["candidate_digest"]
    );
}

#[tokio::test]
async fn create_retry_of_the_same_creation_still_resumes() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let req = create_with_candidate("same-create-1", "cand-same", "OPS-SAME-1");
    let first = store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    let before = candidate_row(&admin, "cand-same").await;

    // The first response was lost and the client retries under a new request id.
    let mut retry = req.clone();
    retry.request_id = "same-create-2".into();
    let again = store
        .planning_draft(TENANT, PROJECT, A, &retry)
        .await
        .unwrap();
    assert_eq!(
        again["result"],
        json!({
            "candidate_id": "cand-same",
            "draft_revision": 1,
            "candidate_digest": first["result"]["candidate_digest"],
            "state": "drafting",
            "resumed": true,
        })
    );
    assert_eq!(candidate_row(&admin, "cand-same").await, before);

    // The domain entry resumes the same creation for an unbound caller too.
    let direct = DraftCandidateCreate {
        changes: req.changes.clone(),
        suggestion_ids: req.suggestion_ids.clone(),
        allowed_spec_roots: req.allowed_spec_roots.clone(),
        project_goal_keys: req.project_goal_keys.clone(),
        self_approve_policy: req.self_approve_policy.clone(),
        author_person_id: req.author_person_id.clone(),
        predetermined_candidate_id: Some("cand-same".into()),
    };
    let direct = store
        .create_planning_candidate(TENANT, PROJECT, A, &direct)
        .await
        .unwrap();
    assert_eq!(direct, again["result"]);

    // An edit moves the candidate to revision 2. A crashed original create (receipt still
    // reserved) resumes against the creation it recorded and reports the current revision.
    let edit = PlanningDraftRequest {
        protocol_version: 1,
        request_id: "same-edit".into(),
        mode: "edit".into(),
        candidate_id: Some("cand-same".into()),
        changes: vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(task("OPS-SAME-1", "one")),
            after: task("OPS-SAME-1", "edited"),
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
    rewind_receipt(
        &admin,
        "same-create-1",
        "planning.edit_draft",
        "cand-same",
        None,
    )
    .await;
    let history: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_candidate_history WHERE candidate_id='cand-same'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let resumed = store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(resumed["result"]["resumed"], true);
    assert_eq!(resumed["result"]["draft_revision"], 2);
    assert_eq!(
        resumed["result"]["candidate_digest"],
        edited["result"]["candidate_digest"]
    );
    let history_after: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_candidate_history WHERE candidate_id='cand-same'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(history_after, history);

    // The edited content is not the recorded creation: a new create with it is refused.
    let mut as_edited = create_with_candidate("same-create-3", "cand-same", "OPS-SAME-1");
    as_edited.changes = edit.changes.clone();
    let current = candidate_row(&admin, "cand-same").await;
    let err = store
        .planning_draft(TENANT, PROJECT, A, &as_edited)
        .await
        .expect_err("only the recorded creation resumes");
    assert_candidate_exists_refusal(&err, &current, "cand-same", json!(["changes"]));
}

#[tokio::test]
async fn create_reusing_a_candidate_id_through_another_client_is_refused_as_author() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    // Token B is the same actor through a second client (cli-b); let it write too.
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-b';",
        )
        .await
        .unwrap();
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let req = create_with_candidate("author-create-1", "cand-author", "OPS-AUTHOR-1");
    store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    let before = candidate_row(&admin, "cand-author").await;
    let counts = planning_write_counts(&admin).await;

    // The same body through another client is not the creation this candidate recorded.
    let mut other_client = req.clone();
    other_client.request_id = "author-create-2".into();
    let err = store
        .planning_draft(TENANT, PROJECT, B, &other_client)
        .await
        .expect_err("a create through another client is a different creation");
    assert_candidate_exists_refusal(&err, &before, "cand-author", json!(["author"]));
    assert_eq!(candidate_row(&admin, "cand-author").await, before);
    assert_eq!(
        planning_write_counts(&admin).await,
        counts,
        "the refused create writes nothing"
    );
    let unused: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_command_receipts WHERE request_id='author-create-2'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(unused, 0, "the refused request id stays unused");

    // The original author retrying the same body still resumes.
    let mut same_author = req.clone();
    same_author.request_id = "author-create-3".into();
    let resumed = store
        .planning_draft(TENANT, PROJECT, A, &same_author)
        .await
        .unwrap();
    assert_eq!(resumed["result"]["resumed"], true);
    assert_eq!(
        resumed["result"]["candidate_digest"],
        before["candidate_digest"]
    );
}

#[tokio::test]
async fn create_reusing_a_candidate_id_compares_suggestion_ids() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let mut suggestion_ids = Vec::new();
    for n in 1..=2 {
        let suggested = store
            .planning_suggest(
                TENANT,
                PROJECT,
                A,
                &PlanningSuggestRequest {
                    protocol_version: 1,
                    request_id: format!("sugg-ids-{n}"),
                    rationale: format!("suggestion {n}"),
                    affected_work_keys: vec!["a".into()],
                    proposed_notes: json!({"note": n}),
                    author_person_id: Some("agent".into()),
                },
            )
            .await
            .unwrap();
        suggestion_ids.push(
            suggested["result"]["suggestion_id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let mut req = create_with_candidate("sugg-create-1", "cand-sugg", "OPS-SUGG-1");
    req.suggestion_ids = vec![suggestion_ids[0].clone()];
    store
        .planning_draft(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    let before = candidate_row(&admin, "cand-sugg").await;
    assert_eq!(before["suggestion_ids"], json!([suggestion_ids[0]]));

    // (a) The same suggestion ids: an idempotent retry.
    let mut same = req.clone();
    same.request_id = "sugg-create-2".into();
    let resumed = store
        .planning_draft(TENANT, PROJECT, A, &same)
        .await
        .unwrap();
    assert_eq!(resumed["result"]["resumed"], true);
    assert_eq!(
        resumed["result"]["candidate_digest"],
        before["candidate_digest"]
    );

    // (b) Different suggestion ids with the same changes: refused, nothing written.
    let counts = planning_write_counts(&admin).await;
    let mut other = req.clone();
    other.request_id = "sugg-create-3".into();
    other.suggestion_ids = vec![suggestion_ids[1].clone()];
    let err = store
        .planning_draft(TENANT, PROJECT, A, &other)
        .await
        .expect_err("different suggestion ids are a different creation");
    assert_candidate_exists_refusal(&err, &before, "cand-sugg", json!(["suggestion_ids"]));
    assert_eq!(candidate_row(&admin, "cand-sugg").await, before);
    assert_eq!(planning_write_counts(&admin).await, counts);
    let state: String = admin
        .query_one(
            "SELECT state FROM awr_team.planning_suggestions WHERE id=$1",
            &[&suggestion_ids[1]],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(state, "open", "the refused create accepts no suggestion");
}
