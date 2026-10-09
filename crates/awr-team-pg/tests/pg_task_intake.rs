//! Authenticated intake regressions, not native collaboration acceptance.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::*;
use awr_team_pg::{AuthorizationStore, PgError, WorkstreamCommand, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio_postgres::Client;

const SUPERVISOR: &str =
    "awr1.supervisor.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const OTHER_AGENT: &str =
    "awr1.other-agent.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const OTHER_CLIENT: &str =
    "awr1.other-client.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

mod assignee_directory {
    use super::*;

    fn lookup(work: &str) -> awr_team_pg::WorkstreamQuery {
        let mut q = query("task.assignees");
        q.work_id = Some(work.into());
        q
    }

    async fn read(store: &WorkstreamReadStore, q: awr_team_pg::WorkstreamQuery) -> Value {
        store.query(TENANT, PROJECT, SUPERVISOR, q).await.unwrap()
    }

    async fn issue(admin: &Client, db: &str, id: &str, actions: &[AuthorizedAction], created: i64) {
        let grant = AgentAuthorization {
            id: id.into(),
            authorizer_person_id: PersonId::new("manager-person").unwrap(),
            responsible_person_id: PersonId::new("manager-person").unwrap(),
            subject_kind: ExecutionSubjectKind::Agent,
            subject_id: "reviewer".into(),
            client_id: "supervisor-client".into(),
            session_id: None,
            model_id: None,
            scope: AuthorizationScope::Task {
                project_id: PROJECT.into(),
                work_item_id: "a".into(),
            },
            actions: actions.iter().copied().collect(),
            expires_at_ms: None,
            status: AuthorizationStatus::Active,
            revoked_at_ms: None,
            revoked_by: None,
            verifiable_capabilities: vec![],
            self_reported_skill_hints: vec![],
            parent_authorization_id: None,
            maintainer_person_id: None,
            created_at_ms: created,
            binding_id: Some("manager-binding".into()),
        };
        let auths =
            AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db));
        auths
            .issue(
                TENANT,
                PROJECT,
                &IssueAuthorizationRequest {
                    request_key: format!("issue-{id}"),
                    authorization: grant,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            admin
                .query_one(
                    "SELECT count(*) FROM awr_team.agent_authorizations WHERE id=$1",
                    &[&id]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            1
        );
    }

    async fn agent_supervisor(admin: &Client) {
        admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='reviewer';
            INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity)
              VALUES('reader-tenant','reader-project','manager-person','Manager','active',
                '{\"kind\":\"simulated_member\",\"controller_ref\":\"shared-controller\"}'::jsonb);
            INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
              VALUES('reader-tenant','reader-project','manager-binding','manager-person','reviewer','active')").await.unwrap();
    }

    #[tokio::test]
    async fn names_are_scoped_literal_unambiguous_and_read_only() {
        let (_guard, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        members(&admin, &db).await;
        assert!(matches!(
            store
                .project_access()
                .inspect(TENANT, PROJECT, SUPERVISOR, "alice", "cli-a")
                .await,
            Err(PgError::Forbidden)
        ));
        let before = snapshot(&admin).await;
        let first = read(&store, lookup("a")).await;
        assert_eq!(
            first["data"]["items"],
            json!([
                {"name":"Alice","assignee_person_id":"alice"},
                {"name":"Bob","assignee_person_id":"bob"},
            ])
        );
        assert!(first["data"]["next_cursor"].is_null());
        assert_eq!(snapshot(&admin).await, before);
        for forbidden in [
            "secret_hash",
            "worker-b",
            "PRIVATE NEXT ACTION",
            "private-beta",
        ] {
            assert!(!first.to_string().contains(forbidden));
        }
        assert!(
            store
                .query(TENANT, PROJECT, SUPERVISOR, lookup("b-private"))
                .await
                .is_err()
        );
        assert!(matches!(
            store.query(TENANT, PROJECT, A, lookup("a")).await,
            Err(PgError::Forbidden)
        ));
        admin
            .batch_execute(
                "UPDATE awr_team.persons SET display_name='Mei%_ UI' WHERE id='alice';
            UPDATE awr_team.persons SET display_name='Mei plain' WHERE id='bob'",
            )
            .await
            .unwrap();
        let mut q = lookup("a");
        q.search = Some("%_".into());
        assert_eq!(
            read(&store, q).await["data"]["items"][0]["assignee_person_id"],
            "alice"
        );
        admin
            .batch_execute(
                "UPDATE awr_team.persons SET display_name='MEI' WHERE id IN ('alice','bob')",
            )
            .await
            .unwrap();
        let mut q = lookup("a");
        q.search = Some("mei".into());
        assert_eq!(
            read(&store, q).await["data"]["items"],
            json!([
                {"name":"MEI","assignee_person_id":"alice"},
                {"name":"MEI","assignee_person_id":"bob"},
            ])
        );
    }

    #[tokio::test]
    async fn one_combined_live_grant_is_required_and_action_only_grants_do_not_mask_it() {
        let (_guard, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        members(&admin, &db).await;
        agent_supervisor(&admin).await;
        issue(
            &admin,
            &db,
            "earlier-assignment",
            &[AuthorizedAction::AssignWork],
            500,
        )
        .await;
        issue(
            &admin,
            &db,
            "separate-reader",
            &[AuthorizedAction::Inspect],
            1000,
        )
        .await;
        let before = snapshot(&admin).await;
        assert!(matches!(
            store.query(TENANT, PROJECT, SUPERVISOR, lookup("a")).await,
            Err(PgError::Forbidden)
        ));
        assert_ne!(
            prepare(&store, SUPERVISOR, "a").await["data"]["guidance"]["action"]["op"],
            "task.assignees"
        );
        assert_eq!(snapshot(&admin).await, before);
        issue(
            &admin,
            &db,
            "combined-directory",
            &[AuthorizedAction::Inspect, AuthorizedAction::AssignWork],
            2000,
        )
        .await;
        let result = read(&store, lookup("a")).await;
        assert_eq!(result["data"]["items"].as_array().unwrap().len(), 2);
        assert_eq!(
            prepare(&store, SUPERVISOR, "a").await["data"]["guidance"]["action"]["op"],
            "task.assignees"
        );
        assert!(
            store
                .query(TENANT, PROJECT, SUPERVISOR, lookup("b-private"))
                .await
                .is_err()
        );
        let mut q = lookup("a");
        q.limit = Some(1);
        q.cursor = Some(
            read(&store, q.clone()).await["data"]["next_cursor"]
                .as_str()
                .unwrap()
                .into(),
        );
        AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db))
            .revoke(
                TENANT,
                PROJECT,
                &RevokeAuthorizationRequest {
                    request_key: "revoke-directory".into(),
                    authorization_id: "combined-directory".into(),
                    revoked_by: PersonId::new("manager-person").unwrap(),
                    revoked_at_ms: 3000,
                    reason: "assignment scope ended".into(),
                },
            )
            .await
            .unwrap();
        let before = snapshot(&admin).await;
        assert!(matches!(
            store.query(TENANT, PROJECT, SUPERVISOR, q).await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, before);
    }

    #[tokio::test]
    async fn bounded_scanning_can_return_an_empty_page_without_exposing_hidden_ids() {
        let (_guard, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        members(&admin, &db).await;
        admin.batch_execute("INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
            SELECT 'reader-tenant','reader-project','00-hidden-'||lpad(n::text,3,'0'),'Withheld name','active'
            FROM generate_series(1,100) n").await.unwrap();
        let before = snapshot(&admin).await;
        let first = read(&store, lookup("a")).await;
        assert_eq!(first["data"]["items"], json!([]));
        let cursor = first["data"]["next_cursor"].as_str().unwrap();
        assert!(!cursor.contains("00-hidden") && !first.to_string().contains("Withheld name"));
        let mut q = lookup("a");
        q.cursor = Some(cursor.into());
        q.limit = Some(1);
        let second = read(&store, q.clone()).await;
        assert_eq!(second["data"]["items"][0]["assignee_person_id"], "alice");
        q.cursor = Some(second["data"]["next_cursor"].as_str().unwrap().into());
        let third = read(&store, q).await;
        assert_eq!(third["data"]["items"][0]["assignee_person_id"], "bob");
        assert!(third["data"]["next_cursor"].is_null());
        assert_eq!(snapshot(&admin).await, before);
    }

    #[tokio::test]
    async fn current_target_policy_is_shared_and_assignment_rechecks_after_lookup() {
        let (_guard, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        members(&admin, &db).await;
        for (disable, restore) in [
            (
                "UPDATE awr_team.workstream_grants SET active=false WHERE actor_id='worker-b'",
                "UPDATE awr_team.workstream_grants SET active=true WHERE actor_id='worker-b'",
            ),
            (
                "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='bind-b'",
                "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='bind-b'",
            ),
            (
                "UPDATE awr_team.actors SET status='disabled' WHERE id='bob'",
                "UPDATE awr_team.actors SET status='active' WHERE id='bob'",
            ),
            (
                "UPDATE awr_team.persons SET status='disabled' WHERE id='bob'",
                "UPDATE awr_team.persons SET status='active' WHERE id='bob'",
            ),
            (
                "UPDATE awr_team.project_memberships SET business_roles='[\"observer\"]'::jsonb WHERE actor_id='bob'",
                "UPDATE awr_team.project_memberships SET business_roles='[\"developer\"]'::jsonb WHERE actor_id='bob'",
            ),
            (
                "UPDATE awr_team.workstream_grants SET authority_version=2 WHERE actor_id='worker-b'",
                "UPDATE awr_team.workstream_grants SET authority_version=1 WHERE actor_id='worker-b'",
            ),
        ] {
            assert_eq!(
                read(&store, lookup("a")).await["data"]["items"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            let command = assign(&store, "a", "lookup-then-assign", "bob").await;
            admin.batch_execute(disable).await.unwrap();
            let before = snapshot(&admin).await;
            assert_eq!(
                read(&store, lookup("a")).await["data"]["items"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert!(matches!(
                store
                    .commands()
                    .execute(TENANT, PROJECT, SUPERVISOR, command)
                    .await,
                Err(PgError::Forbidden)
            ));
            assert_eq!(snapshot(&admin).await, before);
            admin.batch_execute(restore).await.unwrap();
        }
    }

    #[tokio::test]
    async fn typed_access_change_invalidates_cursor_and_rejects_stale_assignment_atomically() {
        let (_guard, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        members(&admin, &db).await;
        let mut q = lookup("a");
        q.limit = Some(1);
        q.cursor = Some(
            read(&store, q.clone()).await["data"]["next_cursor"]
                .as_str()
                .unwrap()
                .into(),
        );
        let old_command = assign(&store, "a", "assign-before-access-change", "bob").await;
        let mut owner = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
        let plan = awr_team_pg::AccessPlan {
            protocol_version: 1,
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor: awr_team_pg::AccessActor {
                id: "worker-b".into(),
                kind: "agent".into(),
                display_name: "Worker B".into(),
            },
            client_id: "cli-b".into(),
            role: "developer".into(),
            business_roles: Some(BTreeSet::from([awr_team::BusinessRole::Developer])),
            assignment_grant: None,
            grants: vec![],
            credential: None,
            revoke_credentials: vec![],
            independent_review: false,
            agent_review: false,
        };
        let preview = awr_team_pg::OperatorAccess::preview(&mut owner, &plan)
            .await
            .unwrap();
        awr_team_pg::OperatorAccess::apply(
            &mut owner,
            &plan,
            "revoke-target-scope",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
        let before = snapshot(&admin).await;
        assert!(matches!(
            store.query(TENANT, PROJECT, SUPERVISOR, q).await,
            Err(PgError::CursorExpired)
        ));
        assert!(
            store
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, old_command)
                .await
                .is_err()
        );
        assert_eq!(
            read(&store, lookup("a")).await["data"]["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(snapshot(&admin).await, before);
    }

    #[tokio::test]
    async fn activated_task_contract_change_expires_directory_cursor_and_stale_assignment() {
        let (_guard, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        members(&admin, &db).await;
        let mut q = lookup("a");
        q.limit = Some(1);
        let first = read(&store, q.clone()).await;
        q.cursor = Some(first["data"]["next_cursor"].as_str().unwrap().into());
        let old_command = assign(&store, "a", "assign-before-source-change", "bob").await;
        let mut bundle: Value = admin.query_one("SELECT jsonb_build_object(
            'codec',$3::text,'catalog',c.catalog_json,'contracts',(
              SELECT jsonb_agg(jsonb_build_object('workstream_id',o.workstream_id,'contract',w.contract_json) ORDER BY w.work_id)
              FROM awr_team.work_contracts w JOIN awr_team.workstream_snapshot_ownership o
                ON o.tenant_id=w.tenant_id AND o.project_id=w.project_id
                AND o.snapshot_id=w.snapshot_id AND o.scope_id=w.scope_id AND o.work_id=w.work_id
              WHERE w.tenant_id=p.tenant_id AND w.project_id=p.id AND w.snapshot_id=p.active_snapshot_id))
            FROM awr_team.projects p JOIN awr_team.workstream_catalogs c
              ON c.tenant_id=p.tenant_id AND c.project_id=p.id AND c.snapshot_id=p.active_snapshot_id
            WHERE p.tenant_id=$1 AND p.id=$2",
            &[&TENANT,&PROJECT,&awr_team::WorkstreamBundle::CODEC]).await.unwrap().get(0);
        // Assignment fences the selected task contract, not unrelated project
        // changes. Change that contract through reviewed source activation.
        bundle["contracts"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry["contract"]["work_id"] == "a")
            .unwrap()["contract"]["acceptance"]
            .as_array_mut()
            .unwrap()
            .push(json!("record verified member dispatch"));
        admin
            .batch_execute(
                "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
            ('reader-tenant','source-author','agent','Source author','active'),
            ('reader-tenant','source-reviewer','human','Source reviewer','active');
            INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
            ('reader-tenant','reader-project','source-reviewer','reviewer')",
            )
            .await
            .unwrap();
        let source = awr_team_pg::SourceStore::from_config(common::with_app_role(
            &common::test_config(),
            &db,
        ));
        let candidate = source
            .ingest(awr_team_pg::IngestRequest {
                tenant_id: TENANT.into(),
                project_id: PROJECT.into(),
                actor_id: "source-author".into(),
                parser_version: "workstreams/1".into(),
                files: vec![
                    awr_team_pg::SourceFile {
                        path: "workstreams.json".into(),
                        bytes: serde_json::to_vec(&bundle).unwrap(),
                    },
                    awr_team_pg::SourceFile {
                        path: "docs/public-notes.md".into(),
                        bytes: b"# Public notes\nMember dispatch is part of acceptance.\n".to_vec(),
                    },
                ],
            })
            .await
            .unwrap();
        source
            .approve(
                TENANT,
                PROJECT,
                &candidate.proposal_id,
                "source-reviewer",
                &candidate.manifest_digest,
            )
            .await
            .unwrap();
        source
            .activate_workstreams(
                TENANT,
                PROJECT,
                "source-author",
                &candidate.proposal_id,
                &awr_team::SourceActivationPlan {
                    candidate_digest: candidate.manifest_digest.clone(),
                    parser_version: candidate.parser_version.clone(),
                    expected_authority_epoch: candidate.base_epoch.clone(),
                    approved_candidate_digest: candidate.manifest_digest.clone(),
                },
            )
            .await
            .unwrap();
        let before = snapshot(&admin).await;
        assert!(matches!(
            store.query(TENANT, PROJECT, SUPERVISOR, q).await,
            Err(PgError::CursorExpired)
        ));
        assert!(
            store
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, old_command)
                .await
                .is_err()
        );
        let fresh = read(&store, lookup("a")).await;
        assert_ne!(fresh["source_snapshot_id"], first["source_snapshot_id"]);
        assert_eq!(fresh["data"]["items"].as_array().unwrap().len(), 2);
        assert_eq!(snapshot(&admin).await, before);
        let refreshed_command = assign(&store, "a", "assign-after-source-change", "bob").await;
        assert!(
            store
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, refreshed_command)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn cursor_and_guidance_respect_identity_search_recovery_and_context_boundaries() {
        let (_guard, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        members(&admin, &db).await;
        let mut q = lookup("a");
        q.limit = Some(1);
        q.cursor = Some(
            read(&store, q.clone()).await["data"]["next_cursor"]
                .as_str()
                .unwrap()
                .into(),
        );
        let mut changed = q.clone();
        changed.search = Some("Bob".into());
        assert!(matches!(
            store.query(TENANT, PROJECT, SUPERVISOR, changed).await,
            Err(PgError::CursorExpired)
        ));
        let alternate =
            "awr1.directory-alt.1111111111111111111111111111111111111111111111111111111111111111";
        admin
            .execute(
                "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
            VALUES($1,'directory-alt','reviewer','supervisor-client',$2)",
                &[
                    &TENANT,
                    &awr_team_pg::workstream_credential_hash(alternate).unwrap(),
                ],
            )
            .await
            .unwrap();
        assert!(matches!(
            store.query(TENANT, PROJECT, alternate, q.clone()).await,
            Err(PgError::CursorExpired)
        ));
        let mut forged: Value = serde_json::from_str(q.cursor.as_ref().unwrap()).unwrap();
        forged["key"] = json!("hidden-member-selector");
        q.cursor = Some(forged.to_string());
        assert!(matches!(
            store.query(TENANT, PROJECT, SUPERVISOR, q).await,
            Err(PgError::CursorExpired)
        ));
        let p = prepare(&store, SUPERVISOR, "a").await;
        let hint = &p["data"]["guidance"];
        assert_eq!(hint["action"]["op"], "task.assignees");
        assert_eq!(hint["action"]["query"]["work_id"], "a");
        assert!(hint.to_string().len() < 900);
        admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,recovery_blocked)
            VALUES('reader-tenant','reader-project','main','a','in_progress',true)").await.unwrap();
        assert_eq!(
            prepare(&store, SUPERVISOR, "a").await["data"]["guidance"]["action"]["op"],
            "work.recovery"
        );
        let mut tiny = query("work.prepare");
        tiny.work_id = Some("a".into());
        tiny.max_context_bytes = Some(1);
        let incomplete = store.query(TENANT, PROJECT, SUPERVISOR, tiny).await;
        assert!(
            incomplete.is_err()
                || incomplete.unwrap()["data"]["guidance"]["action"]["op"] != "task.assignees"
        );
    }
}

async fn members(admin: &Client, db: &str) {
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
        UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]'::jsonb WHERE actor_id='agent';
        UPDATE awr_team.project_memberships SET role='maintainer',business_roles='[\"supervisor\"]'::jsonb,assignment_grant=true WHERE actor_id='reviewer';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
          ('reader-tenant','alice','agent','Member Alice','active'),
          ('reader-tenant','bob','agent','Member Bob','active'),
          ('reader-tenant','worker-b','agent','Worker B','active'),
          ('reader-tenant','worker-alt','agent','Alternate worker','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role,business_roles) VALUES
          ('reader-tenant','reader-project','alice','developer','[\"developer\"]'::jsonb),
          ('reader-tenant','reader-project','bob','developer','[\"developer\"]'::jsonb),
          ('reader-tenant','reader-project','worker-b','developer','[\"developer\"]'::jsonb),
          ('reader-tenant','reader-project','worker-alt','developer','[\"developer\"]'::jsonb);
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
          ('reader-tenant','reader-project','alice','Alice','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"shared-controller\"}'::jsonb),
          ('reader-tenant','reader-project','bob','Bob','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"shared-controller\"}'::jsonb);
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
          ('reader-tenant','reader-project','bind-a','alice','agent','active'),
          ('reader-tenant','reader-project','bind-b','bob','worker-b','active'),
          ('reader-tenant','reader-project','bind-alt','alice','worker-alt','active');
        UPDATE awr_team.credentials SET actor_id='worker-b' WHERE id='reader-b';").await.unwrap();
    for (actor, client) in [
        ("reviewer", "supervisor-client"),
        ("worker-b", "cli-b"),
        ("worker-alt", "cli-alt"),
        ("agent", "cli-other"),
    ] {
        admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
            VALUES($1,$2,$3,$4,$5,1,true,true)",&[&TENANT,&PROJECT,&actor,&client,&Id::from(1).to_string()]).await.unwrap();
    }
    for (id, actor, client, token) in [
        ("supervisor", "reviewer", "supervisor-client", SUPERVISOR),
        ("other-agent", "worker-alt", "cli-alt", OTHER_AGENT),
        ("other-client", "agent", "cli-other", OTHER_CLIENT),
    ] {
        admin.execute("INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash) VALUES($1,$2,$3,$4,$5)",
            &[&TENANT,&id,&actor,&client,&awr_team_pg::workstream_credential_hash(token).unwrap()]).await.unwrap();
    }
    let auths = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db));
    for (id, person, actor, client, binding) in [
        ("auth-a", "alice", "agent", "cli-a", "bind-a"),
        ("auth-b", "bob", "worker-b", "cli-b", "bind-b"),
        ("auth-alt", "alice", "worker-alt", "cli-alt", "bind-alt"),
        ("auth-other-client", "alice", "agent", "cli-other", "bind-a"),
    ] {
        let grant = AgentAuthorization {
            id: id.into(),
            authorizer_person_id: PersonId::new(person).unwrap(),
            responsible_person_id: PersonId::new(person).unwrap(),
            subject_kind: ExecutionSubjectKind::Agent,
            subject_id: actor.into(),
            client_id: client.into(),
            session_id: None,
            model_id: None,
            scope: AuthorizationScope::Workstream {
                project_id: PROJECT.into(),
                workstream_id: Id::from(1).to_string(),
            },
            actions: BTreeSet::from([
                AuthorizedAction::Inspect,
                AuthorizedAction::StartWork,
                AuthorizedAction::ClaimCoordination,
            ]),
            expires_at_ms: None,
            status: AuthorizationStatus::Active,
            revoked_at_ms: None,
            revoked_by: None,
            verifiable_capabilities: vec![],
            self_reported_skill_hints: vec![],
            parent_authorization_id: None,
            maintainer_person_id: None,
            created_at_ms: 1000,
            binding_id: Some(binding.into()),
        };
        auths
            .issue(
                TENANT,
                PROJECT,
                &IssueAuthorizationRequest {
                    request_key: format!("issue-{id}"),
                    authorization: grant,
                },
            )
            .await
            .unwrap();
    }
}

async fn snapshot(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'responsibilities',(SELECT jsonb_agg(to_jsonb(r) ORDER BY work_id) FROM awr_team.task_responsibilities r),
        'responsibility_events',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.responsibility_events r),
        'responsibility_receipts',(SELECT jsonb_agg(to_jsonb(r) ORDER BY request_key) FROM awr_team.responsibility_receipts r),
        'claims',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.claims r),
        'runtime',(SELECT jsonb_agg(to_jsonb(r) ORDER BY work_id) FROM awr_team.work_runtime r),
        'events',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.events r),
        'operations',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.operations r),
        'persons',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.persons r),
        'revision',(SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2))",
        &[&TENANT,&PROJECT]).await.unwrap().get(0)
}

async fn start(store: &WorkstreamReadStore, token: &str, work: &str, request: &str) -> String {
    let p = prepare(store, token, work).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(
                &p,
                request,
                "session.start",
                json!({"conversation_id":request,
                    "client_info":{"product":"Synthetic Agent","capabilities":{
                        "model":"unsupported","usage":"unsupported","progress":"supported"}}}),
            ),
        )
        .await
        .unwrap()["receipt"]["data"]["session_id"]
        .as_str()
        .unwrap()
        .into()
}

async fn assign(
    store: &WorkstreamReadStore,
    work: &str,
    request: &str,
    target: &str,
) -> WorkstreamCommand {
    let p = prepare(store, SUPERVISOR, work).await;
    command(
        &p,
        request,
        "task.assign",
        json!({"assignee_person_id":target,
        "expected_responsibility_version":p["data"]["responsibility"]["version"]}),
    )
}

async fn take(
    store: &WorkstreamReadStore,
    token: &str,
    work: &str,
    session: &str,
    request: &str,
    accept: bool,
) -> WorkstreamCommand {
    let p = prepare(store, token, work).await;
    let mut args = json!({"session_id":session,"expected_session_version":"1",
        "expected_responsibility_version":p["data"]["responsibility"]["version"],
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":60});
    if accept {
        args["assignment_request_key"] =
            p["data"]["responsibility"]["pending"]["transfer_request_key"].clone();
    }
    command(
        &p,
        request,
        if accept {
            "task.accept_assignment"
        } else {
            "task.claim_available"
        },
        args,
    )
}

#[tokio::test]
async fn supervisor_reserves_without_execution_session_and_target_accepts_atomically() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let before = snapshot(&admin).await;
    let reserved = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            SUPERVISOR,
            assign(&store, "a", "dispatch", "alice").await,
        )
        .await
        .unwrap();
    assert_eq!(
        reserved["receipt"]["data"]["coordination_claim_acquired"],
        false
    );
    assert_eq!(reserved["current_responsibility"]["owner"], "alice");
    assert_eq!(
        reserved["current_responsibility"]["pending"]["kind"],
        "no_acceptor"
    );
    assert_eq!(snapshot(&admin).await["claims"], before["claims"]);
    let accepted = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            take(&store, A, "a", "session-a", "accept", true).await,
        )
        .await
        .unwrap();
    assert!(accepted["current_responsibility"]["pending"].is_null());
    assert_eq!(
        accepted["current_responsibility"]["current_executor"]["agent_id"],
        "agent"
    );
    assert_eq!(accepted["receipt"]["data"]["state"], "active");
    assert_eq!(accepted["execution_authorized"], false);
    assert_eq!(
        snapshot(&admin).await["claims"].as_array().unwrap().len(),
        1
    );
}

#[tokio::test]
async fn two_simulated_members_self_claiming_have_one_atomic_winner() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let bob_session = start(&store, B, "a", "bob-session").await;
    let a = take(&store, A, "a", "session-a", "shared-public-id", false).await;
    let b = take(&store, B, "a", &bob_session, "shared-public-id", false).await;
    let commands = store.commands();
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, a),
        commands.execute(TENANT, PROJECT, B, b)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let winner = a.as_ref().ok().or(b.as_ref().ok()).unwrap();
    let state = snapshot(&admin).await;
    assert_eq!(state["claims"].as_array().unwrap().len(), 1);
    assert_eq!(state["responsibilities"].as_array().unwrap().len(), 1);
    assert_eq!(
        state["responsibilities"][0]["owner_person_id"],
        winner["current_responsibility"]["owner"]
    );
    assert_eq!(winner["execution_authorized"], false);
    assert!(matches!(
        a.err().or(b.err()),
        Some(PgError::ClaimHeld | PgError::PreconditionsChanged)
    ));
}

#[tokio::test]
async fn supervisor_dispatch_and_pool_claim_cannot_overwrite_each_other() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let dispatch = assign(&store, "a", "dispatch", "bob").await;
    let claim = take(&store, A, "a", "session-a", "claim", false).await;
    let commands = store.commands();
    let (dispatch, claim) = tokio::join!(
        commands.execute(TENANT, PROJECT, SUPERVISOR, dispatch),
        commands.execute(TENANT, PROJECT, A, claim)
    );
    assert_ne!(dispatch.is_ok(), claim.is_ok());
    let state = snapshot(&admin).await;
    if dispatch.is_ok() {
        assert_eq!(state["responsibilities"][0]["owner_person_id"], "bob");
        assert!(state["claims"].is_null());
    } else {
        assert_eq!(state["responsibilities"][0]["owner_person_id"], "alice");
        assert_eq!(state["claims"].as_array().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn reserved_dependencies_and_wrong_acceptance_leave_all_facts_unchanged() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            SUPERVISOR,
            assign(&store, "c", "reserve-dependency", "alice").await,
        )
        .await
        .unwrap();
    let session = start(&store, A, "c", "downstream-session").await;
    let before = snapshot(&admin).await;
    let request = take(&store, A, "c", &session, "blocked-accept", true).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, request).await,
        Err(PgError::MissingDependency)
    ));
    assert_eq!(snapshot(&admin).await, before);
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            SUPERVISOR,
            assign(&store, "a", "reserve-ready", "alice").await,
        )
        .await
        .unwrap();
    let bob = start(&store, B, "a", "bob-session").await;
    let wrong_member = take(&store, B, "a", &bob, "wrong-member", true).await;
    let mut wrong_reference = take(&store, A, "a", "session-a", "wrong-reference", true).await;
    wrong_reference.args["assignment_request_key"] = json!("unrelated-transfer-reference");
    for (token, request) in [(B, wrong_member), (A, wrong_reference)] {
        let before = snapshot(&admin).await;
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, token, request)
                .await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, before);
    }
    let steal = take(&store, B, "a", &bob, "steal-reservation", false).await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, steal).await,
        Err(PgError::ClaimHeld)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn exact_replay_preserves_receipt_and_exposes_current_state_without_renewal() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let request = take(&store, A, "a", "session-a", "once", false).await;
    let original = store
        .commands()
        .execute(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    admin.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second';
        UPDATE awr_team.task_responsibilities SET owner_person_id='bob',version=version+1 WHERE work_id='a'").await.unwrap();
    let before = snapshot(&admin).await;
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    assert_eq!(replay["receipt"], original["receipt"]);
    assert_eq!(replay["current_responsibility"]["owner"], "bob");
    assert_eq!(replay["execution_authorized"], false);
    assert_eq!(snapshot(&admin).await, before);
    let mut changed = request;
    changed.args["ttl_seconds"] = json!(120);
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false WHERE actor_id='agent' AND client_id='cli-a'").await.unwrap();
    let mut q = query("command.inspect");
    q.work_id = Some("a".into());
    q.request_id = Some("once".into());
    assert!(store.query(TENANT, PROJECT, A, q).await.is_ok());
    let replay = take(&store, A, "a", "session-a", "denied-new-request", false).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, replay).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn target_policy_and_untrusted_actor_fields_cannot_create_responsibility() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let request = assign(&store, "a", "target-policy", "bob").await;
    for (disable, restore) in [
        (
            "UPDATE awr_team.actors SET status='disabled' WHERE id='bob'",
            "UPDATE awr_team.actors SET status='active' WHERE id='bob'",
        ),
        (
            "UPDATE awr_team.persons SET status='disabled' WHERE id='bob'",
            "UPDATE awr_team.persons SET status='active' WHERE id='bob'",
        ),
        (
            "UPDATE awr_team.project_memberships SET role='reader' WHERE actor_id='bob'",
            "UPDATE awr_team.project_memberships SET role='developer' WHERE actor_id='bob'",
        ),
        (
            "UPDATE awr_team.workstream_grants SET can_write=false WHERE actor_id='worker-b'",
            "UPDATE awr_team.workstream_grants SET can_write=true WHERE actor_id='worker-b'",
        ),
    ] {
        admin.batch_execute(disable).await.unwrap();
        let before = snapshot(&admin).await;
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, SUPERVISOR, request.clone())
                .await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, before);
        admin.batch_execute(restore).await.unwrap();
    }
    let missing = assign(&store, "a", "missing-target", "not-a-project-member").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, SUPERVISOR, missing)
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let mut forged = take(&store, A, "a", "session-a", "forged-claimant", false).await;
    forged.args["claimant"] = json!("bob");
    assert!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, forged)
            .await
            .is_err()
    );
    assert_eq!(snapshot(&admin).await, before);
    let mut session_pair = request;
    session_pair.args["session_id"] = json!("session-a");
    assert!(
        store
            .commands()
            .execute(TENANT, PROJECT, SUPERVISOR, session_pair)
            .await
            .is_err()
    );
    assert_eq!(snapshot(&admin).await, before);
}

fn lease_change(p: &Value, lease: &Value, request: &str, op: &str) -> WorkstreamCommand {
    let mut args = json!({"session_id":lease["session_id"],"expected_session_version":"1",
        "claim_id":lease["claim_id"],"expected_fence":lease["fence"],"expected_lease_version":lease["lease_version"]});
    if op == "claim.renew" {
        args["ttl_seconds"] = json!(60);
    }
    command(p, request, op, args)
}

#[tokio::test]
async fn lease_expiry_and_release_retain_owner_and_require_handoff_for_other_executors() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let lease = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            take(&store, A, "a", "session-a", "take", false).await,
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let bob = start(&store, B, "a", "bob").await;
    let alt = start(&store, OTHER_AGENT, "a", "alt").await;
    let client = start(&store, OTHER_CLIENT, "a", "other-client").await;
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    for (token, session) in [(B, bob), (OTHER_AGENT, alt), (OTHER_CLIENT, client)] {
        let p = prepare(&store, token, "a").await;
        let request = command(
            &p,
            "legacy-steal",
            "claim.acquire",
            json!({"session_id":session,"expected_session_version":"1",
            "expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":60}),
        );
        let before = snapshot(&admin).await;
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, token, request)
                .await,
            Err(PgError::ClaimHeld)
        ));
        assert_eq!(snapshot(&admin).await, before);
    }
    let released = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            lease_change(
                &prepare(&store, A, "a").await,
                &lease,
                "release",
                "claim.release",
            ),
        )
        .await
        .unwrap();
    assert_eq!(released["current_responsibility"]["owner"], "alice");
    assert_eq!(
        released["current_responsibility"]["current_executor"]["agent_id"],
        "agent"
    );
    let p = prepare(&store, A, "a").await;
    let renewed = store.commands().execute(TENANT,PROJECT,A,command(&p,"continue-own","claim.acquire",json!({
        "session_id":"session-a","expected_session_version":"1","expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":60})))
        .await.unwrap();
    assert_eq!(renewed["receipt"]["data"]["fence"], "2");
    assert_eq!(renewed["current_responsibility"]["owner"], "alice");
}

#[tokio::test]
async fn navigation_is_current_read_only_and_never_offers_another_members_task() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            SUPERVISOR,
            assign(&store, "a", "dispatch", "alice").await,
        )
        .await
        .unwrap();
    let before = snapshot(&admin).await;
    for (token, relation) in [(A, "assigned_to_me"), (B, "assigned_to_other")] {
        let next = store
            .query(TENANT, PROJECT, token, query("work.next"))
            .await
            .unwrap();
        let task = next["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["work_id"] == "a")
            .unwrap();
        assert_eq!(task["responsibility"]["relation"], relation);
        if token == B {
            assert_eq!(task["navigation"], "held");
        }
        let p = prepare(&store, token, "a").await;
        assert_eq!(p["data"]["responsibility"]["version"], "1");
        assert_eq!(p["data"]["responsibility"]["relation"], relation);
        let mut observe = query("work.observe");
        observe.work_id = Some("a".into());
        let hint =
            store.query(TENANT, PROJECT, token, observe).await.unwrap()["data"]["guidance"].clone();
        assert!(hint.to_string().len() < 900);
        for key in ["when", "because", "action", "recheck_on"] {
            assert!(!hint[key].is_null());
        }
        if token == B {
            assert_eq!(hint["action"]["op"], "work.next");
        }
    }
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn late_failure_rolls_back_responsibility_lease_events_and_receipts_then_exact_retry_succeeds()
 {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let request = take(&store, A, "a", "session-a", "late-failure", false).await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_intake_operation() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'synthetic late intake failure'; END; $$;
        CREATE TRIGGER reject_intake_operation BEFORE INSERT ON awr_team.operations
        FOR EACH ROW EXECUTE FUNCTION awr_team.reject_intake_operation()").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, request.clone())
            .await
            .is_err()
    );
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute(
            "DROP TRIGGER reject_intake_operation ON awr_team.operations;
        DROP FUNCTION awr_team.reject_intake_operation()",
        )
        .await
        .unwrap();
    let result = store
        .commands()
        .execute(TENANT, PROJECT, A, request)
        .await
        .unwrap();
    assert_eq!(result["current_responsibility"]["owner"], "alice");
    assert_eq!(result["receipt"]["data"]["fence"], "1");
}

#[tokio::test]
async fn selected_delegation_binding_controls_intake_and_read_guidance_with_multiple_bindings() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    admin.batch_execute("INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
        VALUES('reader-tenant','reader-project','bind-read-bob','bob','agent','active')").await.unwrap();
    let mut grant: AgentAuthorization = serde_json::from_value(
        admin
            .query_one(
                "SELECT body_json FROM awr_team.agent_authorizations WHERE id='auth-a'",
                &[],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    grant.id = "earlier-read-only".into();
    grant.authorizer_person_id = PersonId::new("bob").unwrap();
    grant.responsible_person_id = PersonId::new("bob").unwrap();
    grant.binding_id = Some("bind-read-bob".into());
    grant.actions = BTreeSet::from([AuthorizedAction::Inspect]);
    grant.created_at_ms = 500;
    let auths = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    auths
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "read-only-bob".into(),
                authorization: grant,
            },
        )
        .await
        .unwrap();
    let p = prepare(&store, A, "a").await;
    assert_eq!(p["data"]["responsibility"]["member_person_id"], "alice");
    assert_eq!(
        p["data"]["responsibility"]["coordination_delegation_id"],
        "auth-a"
    );
    let result = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            take(&store, A, "a", "session-a", "claim-selected", false).await,
        )
        .await
        .unwrap();
    assert_eq!(result["current_responsibility"]["owner"], "alice");
    assert_eq!(
        result["current_responsibility"]["current_executor"]["binding_id"],
        "bind-a"
    );
    let before = snapshot(&admin).await;
    assert_eq!(
        prepare(&store, A, "a").await["data"]["responsibility"]["relation"],
        "owned_by_me"
    );
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    let item = next["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["work_id"] == "a")
        .unwrap();
    assert_eq!(item["responsibility"]["member_person_id"], "alice");
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn source_wait_recovery_and_live_authority_gates_leave_intake_atomic() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let request = take(&store, A, "a", "session-a", "guarded-take", false).await;
    for (disable, restore) in [
        (
            "UPDATE awr_team.work_contracts SET definition_state='archived' WHERE work_id='a'",
            "UPDATE awr_team.work_contracts SET definition_state='enabled' WHERE work_id='a'",
        ),
        (
            "UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='bind-a'",
            "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='bind-a'",
        ),
        (
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
            "UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='reader-a'",
        ),
        (
            "UPDATE awr_team.project_memberships SET business_roles='[\"observer\"]'::jsonb WHERE actor_id='agent'",
            "UPDATE awr_team.project_memberships SET business_roles='[\"developer\"]'::jsonb WHERE actor_id='agent'",
        ),
    ] {
        admin.batch_execute(disable).await.unwrap();
        let before = snapshot(&admin).await;
        assert!(
            store
                .commands()
                .execute(TENANT, PROJECT, A, request.clone())
                .await
                .is_err()
        );
        assert_eq!(snapshot(&admin).await, before);
        admin.batch_execute(restore).await.unwrap();
    }
    admin.execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,recovery_blocked)
        VALUES($1,$2,'main','a','unclaimed',0,0,true)",&[&TENANT,&PROJECT]).await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=false")
        .await
        .unwrap();
    admin.batch_execute("INSERT INTO awr_team.wait_items(tenant_id,project_id,id,session_id,work_id,question,state)
        VALUES('reader-tenant','reader-project','wait','session-a','a','Confirm the acceptance criteria','open')").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, request.clone())
            .await,
        Err(PgError::WaitOpen)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute("UPDATE awr_team.wait_items SET state='replied',reply='Criteria confirmed'")
        .await
        .unwrap();
    store
        .commands()
        .execute(TENANT, PROJECT, A, request)
        .await
        .unwrap();
}

#[tokio::test]
async fn legacy_acquisition_and_persistence_kernel_share_the_task_lock() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    let p = prepare(&store, A, "a").await;
    let request = command(
        &p,
        "legacy-take",
        "claim.acquire",
        json!({"session_id":"session-a",
        "expected_session_version":"1","expected_work_version":"0","ttl_seconds":60}),
    );
    let kernel = awr_team_pg::ResponsibilityStore::from_config(common::with_app_role(
        &common::test_config(),
        &db,
    ));
    let owner = ClaimAvailableRequest {
        request_key: "kernel-owner".into(),
        expected_version: 0,
        claimant: PersonId::new("bob").unwrap(),
    };
    let commands = store.commands();
    let (legacy, kernel) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, request),
        kernel.claim_available(TENANT, PROJECT, "a", &owner)
    );
    assert_ne!(legacy.is_ok(), kernel.is_ok());
    let state = snapshot(&admin).await;
    if legacy.is_ok() {
        assert_eq!(state["responsibilities"][0]["owner_person_id"], "alice");
        assert_eq!(state["claims"].as_array().unwrap().len(), 1);
    } else {
        assert_eq!(state["responsibilities"][0]["owner_person_id"], "bob");
        assert!(state["claims"].is_null());
    }
}

#[tokio::test]
async fn controlled_accepted_handoff_allows_a_successor_agent_to_continue_ownership() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    members(&admin, &db).await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            take(&store, A, "a", "session-a", "take", false).await,
        )
        .await
        .unwrap();
    let alt = start(&store, OTHER_AGENT, "a", "alternate-session").await;
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    q.session_id = Some("session-a".into());
    q.max_context_bytes = Some(262144);
    let p = store.query(TENANT, PROJECT, A, q).await.unwrap();
    let checkpoint = store.commands().execute(TENANT,PROJECT,A,command(&p,"checkpoint","session.checkpoint",json!({
        "session_id":"session-a","expected_session_version":"1","context_hash":p["data"]["context_hash"],
        "next_action":"Hand over implementation to the alternate Agent","open_loops":[]}))).await.unwrap();
    let successor = json!({"kind":"agent_run","person_id":"alice","agent_id":"worker-alt","binding_id":"bind-alt"});
    let proposed = store.commands().execute(TENANT,PROJECT,A,command(&prepare(&store,A,"a").await,"propose","handoff.propose",json!({
        "session_id":"session-a","expected_session_version":checkpoint["receipt"]["data"]["session_version"],
        "handoff_id":"agent-transfer","kind":"execution","to_person_id":"alice",
        "proposed_successor":successor}))).await.unwrap();
    let inspected = store.commands().execute(TENANT,PROJECT,OTHER_AGENT,command(&prepare(&store,OTHER_AGENT,"a").await,"inspect","handoff.inspect",json!({
        "session_id":alt,"expected_session_version":"1","handoff_id":"agent-transfer","expected_handoff_version":proposed["receipt"]["data"]["version"],
        "inspector_person_id":"alice"}))).await.unwrap();
    assert_eq!(
        inspected["receipt"]["data"]["prepared_context"]["data"]["context_complete"],
        true
    );
    let p = prepare(&store, OTHER_AGENT, "a").await;
    let accepted = store.commands().execute(TENANT,PROJECT,OTHER_AGENT,command(&p,"accept-handoff","handoff.accept",json!({
        "session_id":alt,"expected_session_version":"1","handoff_id":"agent-transfer","expected_handoff_version":inspected["receipt"]["data"]["version"],
        "acceptor_person_id":"alice","successor_execution":successor,
        "inspection_request_id":inspected["receipt"]["data"]["inspection_request_id"],
        "expected_current_fence":p["data"]["runtime"]["last_fence"]}))).await.unwrap();
    assert_eq!(accepted["receipt"]["data"]["status"], "accepted");
    let p = prepare(&store, OTHER_AGENT, "a").await;
    let claimed = store.commands().execute(TENANT,PROJECT,OTHER_AGENT,command(&p,"successor-claim","claim.acquire",json!({
        "session_id":alt,"expected_session_version":"1","expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":60}))).await.unwrap();
    assert_eq!(claimed["current_responsibility"]["owner"], "alice");
    assert_eq!(
        claimed["current_responsibility"]["current_executor"]["agent_id"],
        "worker-alt"
    );
    assert_eq!(claimed["execution_authorized"], false);
}
