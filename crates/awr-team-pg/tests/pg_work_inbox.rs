#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_core::*;
use awr_team::delivery::*;
use awr_team_pg::*;
use fixture::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio_postgres::Client;

#[test]
fn inbox_protocol_is_discoverable_and_accepts_bounded_pagination() {
    assert!(
        WorkstreamQuery::OPERATIONS.contains(&"work.inbox"),
        "supervisors need a repository-neutral current-fact inbox entry"
    );
    let mut request = query("work.inbox");
    request.limit = Some(5);
    request.max_context_bytes = Some(16_384);
    request.validate().unwrap();
    for invalid in [
        json!({"work_id":"a"}),
        json!({"session_id":"session-a"}),
        json!({"workstream_id":Id::from(1)}),
        json!({"limit":0}),
        json!({"max_context_bytes":0}),
        json!({"max_context_bytes":262145}),
    ] {
        let mut value = json!({"protocol_version":1,"op":"work.inbox"});
        value
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert!(
            serde_json::from_value::<WorkstreamQuery>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

async fn inbox(store: &WorkstreamReadStore, token: &str) -> Value {
    store
        .query(TENANT, PROJECT, token, query("work.inbox"))
        .await
        .unwrap()
}

fn item(page: &Value, work: &str) -> Value {
    page["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["work_id"] == work)
        .cloned()
        .unwrap_or(Value::Null)
}

async fn supervisor(admin: &Client) {
    enable_writes(admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET assignment_grant=true WHERE actor_id='agent'",
        )
        .await
        .unwrap();
}

async fn state(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'project',(SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2),
        'runtime',(SELECT jsonb_agg(to_jsonb(x) ORDER BY work_id) FROM awr_team.work_runtime x),
        'responsibilities',(SELECT jsonb_agg(to_jsonb(x) ORDER BY work_id) FROM awr_team.task_responsibilities x),
        'claims',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.claims x),
        'reviews',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.review_rounds x),
        'completions',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.completion_receipts x),
        'operations',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.operations x),
        'events',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM awr_team.events x))",
        &[&TENANT,&PROJECT]).await.unwrap().get(0)
}

async fn claim(store: &WorkstreamReadStore, accept: bool) -> Value {
    let p = prepare(store, A, "a").await;
    let mut args = json!({"session_id":"session-a","expected_session_version":"1",
        "expected_responsibility_version":p["data"]["responsibility"]["version"],
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":3600});
    if accept {
        args["assignment_request_key"] =
            p["data"]["responsibility"]["pending"]["transfer_request_key"].clone();
    }
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "take-inbox",
                if accept {
                    "task.accept_assignment"
                } else {
                    "task.claim_available"
                },
                args,
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone()
}

#[tokio::test]
async fn assignment_and_self_claim_remove_handled_items_without_automatic_effects() {
    let (_g, admin, _, store) = setup().await;
    let before = state(&admin).await;
    assert!(
        inbox(&store, A).await["data"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(state(&admin).await, before);
    supervisor(&admin).await;
    let page = inbox(&store, A).await;
    assert_eq!(item(&page, "a")["guidance"]["code"], "assignment");
    assert_eq!(item(&page, "c")["guidance"]["code"], "dependency");
    assert!(!page.to_string().contains("b-private"));
    assert!(!page.to_string().contains("PRIVATE NEXT ACTION"));
    let q: WorkstreamQuery =
        serde_json::from_value(item(&page, "a")["next_query"].clone()).unwrap();
    let p = store.query(TENANT, PROJECT, A, q).await.unwrap();
    store.commands().execute(TENANT,PROJECT,A,command(&p,"assign-inbox","task.assign",
        json!({"assignee_person_id":"agent","expected_responsibility_version":p["data"]["responsibility"]["version"]}))).await.unwrap();
    let assigned = inbox(&store, A).await;
    assert_eq!(item(&assigned, "a")["guidance"]["code"], "intake");
    assert_ne!(
        item(&assigned, "a")["item_key"],
        item(&page, "a")["item_key"]
    );
    claim(&store, true).await;
    let before = state(&admin).await;
    assert!(item(&inbox(&store, A).await, "a").is_null());
    assert_eq!(state(&admin).await, before);
    assert!(item(&inbox(&store, B).await, "a").is_null());
    assert_eq!(
        inbox(&store, A).await["data"]["execution_authorized"],
        false
    );
}

#[tokio::test]
async fn paging_survives_empty_pages_and_duplicate_observations_with_stable_keys() {
    let (_g, admin, _, store) = setup().await;
    supervisor(&admin).await;
    let page = inbox(&store, A).await;
    let before = state(&admin).await;
    let (first, second) = tokio::join!(inbox(&store, A), inbox(&store, A));
    assert_eq!(first["data"]["items"], second["data"]["items"]);
    assert_eq!(state(&admin).await, before);
    admin.batch_execute("INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json,workstream_id)
        SELECT tenant_id,project_id,'duplicate-observation',4,0,'delivery.observed','agent','a','{}',workstream_id
        FROM awr_team.workstream_snapshot_ownership WHERE work_id='a';
        UPDATE awr_team.projects SET project_revision=4 WHERE tenant_id='reader-tenant' AND id='reader-project'").await.unwrap();
    assert_eq!(
        page["data"]["items"],
        inbox(&store, A).await["data"]["items"]
    );
    claim(&store, false).await;
    let mut q = query("work.inbox");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert!(first["data"]["items"].as_array().unwrap().is_empty());
    q.cursor = first["data"]["next_cursor"].as_str().map(str::to_owned);
    assert!(q.cursor.is_some());
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    let second = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(second["data"]["items"][0]["work_id"], "c");
    assert!(second["data"]["next_cursor"].is_null());
    admin.batch_execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
}

async fn agent_binding(admin: &Client) {
    enable_writes(admin).await;
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
        UPDATE awr_team.project_memberships SET role='developer',business_roles='[\"developer\"]'::jsonb WHERE actor_id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
          ('reader-tenant','reader-project','member-a','Simulated Member A','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"fixture-controller\"}'::jsonb);
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
          ('reader-tenant','reader-project','binding-a','member-a','agent','active')").await.unwrap();
}

async fn issue(db: &str, id: &str, scope: AuthorizationScope, actions: &[AuthorizedAction]) {
    let grant = AgentAuthorization {
        id: id.into(),
        authorizer_person_id: PersonId::new("member-a").unwrap(),
        responsible_person_id: PersonId::new("member-a").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope,
        actions: actions.iter().copied().collect::<BTreeSet<_>>(),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1000,
        binding_id: Some("binding-a".into()),
    };
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
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

fn task(work: &str) -> AuthorizationScope {
    AuthorizationScope::Task {
        project_id: PROJECT.into(),
        work_item_id: work.into(),
    }
}

#[tokio::test]
async fn task_only_and_multiple_delegations_never_borrow_another_tasks_write_authority() {
    let (_g, admin, db, store) = setup().await;
    agent_binding(&admin).await;
    issue(&db, "read-a", task("a"), &[AuthorizedAction::Inspect]).await;
    issue(&db, "read-c", task("c"), &[AuthorizedAction::Inspect]).await;
    issue(
        &db,
        "claim-c",
        task("c"),
        &[AuthorizedAction::ClaimCoordination],
    )
    .await;
    let page = inbox(&store, A).await;
    assert!(item(&page, "a").is_null());
    assert_eq!(item(&page, "c")["guidance"]["code"], "dependency");
    assert!(!page.to_string().contains("b-private"));
    let mut q = query("work.inbox");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    q.cursor = first["data"]["next_cursor"].as_str().map(str::to_owned);
    issue(
        &db,
        "claim-a",
        task("a"),
        &[AuthorizedAction::ClaimCoordination],
    )
    .await;
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    let page = inbox(&store, A).await;
    assert_eq!(item(&page, "a")["guidance"]["code"], "intake");
    let before = state(&admin).await;
    let mut selected = query("work.prepare");
    selected.work_id = Some("b-private".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, selected).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(state(&admin).await, before);
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-a',$3,1,true)", &[&TENANT,&PROJECT,&Id::from(2).to_string()]).await.unwrap();
    issue(
        &db,
        "read-b",
        task("b-private"),
        &[AuthorizedAction::Inspect],
    )
    .await;
    issue(
        &db,
        "claim-b",
        task("b-private"),
        &[AuthorizedAction::ClaimCoordination],
    )
    .await;
    assert!(
        item(&inbox(&store, A).await, "b-private").is_null(),
        "read delegation cannot replace a missing write grant"
    );
    admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,recovery_blocked)
        VALUES('reader-tenant','reader-project','main','b-private','pending',true)").await.unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "b-private")["guidance"]["code"],
        "recovery"
    );
    admin
        .batch_execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='claim-a'",
        )
        .await
        .unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
    admin
        .batch_execute("UPDATE awr_team.agent_authorizations SET status='revoked'")
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, query("work.inbox")).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn source_activation_expires_inbox_cursors_and_refreshes_the_exact_contract() {
    let (_g, admin, db, store) = setup().await;
    supervisor(&admin).await;
    let mut q = query("work.inbox");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    q.cursor = first["data"]["next_cursor"].as_str().map(str::to_owned);
    let catalog: WorkstreamCatalog = serde_json::from_value(
        admin
            .query_one("SELECT catalog_json FROM awr_team.workstream_catalogs", &[])
            .await
            .unwrap()
            .get(0),
    )
    .unwrap();
    let rows=admin.query("SELECT c.contract_json,o.workstream_id FROM awr_team.work_contracts c
        JOIN awr_team.workstream_snapshot_ownership o USING(tenant_id,project_id,snapshot_id,scope_id,work_id)
        ORDER BY c.work_id",&[]).await.unwrap();
    let contracts = rows
        .into_iter()
        .map(|r| {
            let mut contract: awr_team::WorkContract = serde_json::from_value(r.get(0)).unwrap();
            if contract.work_id.as_str() == "a" {
                contract
                    .hard_rules
                    .push("preserve the additional reviewed constraint".into());
            }
            awr_team::WorkstreamContract {
                workstream_id: r.get::<_, String>(1).parse().unwrap(),
                contract,
            }
        })
        .collect();
    let bundle = awr_team::WorkstreamBundle {
        codec: awr_team::WorkstreamBundle::CODEC.into(),
        catalog,
        contracts,
    };
    let sources = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let c = sources
        .ingest(IngestRequest {
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            actor_id: "agent".into(),
            parser_version: "workstreams/1".into(),
            files: vec![SourceFile {
                path: "workstreams.json".into(),
                bytes: serde_json::to_vec(&bundle).unwrap(),
            }],
        })
        .await
        .unwrap();
    sources
        .approve(
            TENANT,
            PROJECT,
            &c.proposal_id,
            "reviewer",
            &c.manifest_digest,
        )
        .await
        .unwrap();
    sources
        .activate_workstreams(
            TENANT,
            PROJECT,
            "agent",
            &c.proposal_id,
            &awr_team::SourceActivationPlan {
                candidate_digest: c.manifest_digest.clone(),
                parser_version: c.parser_version.clone(),
                expected_authority_epoch: c.base_epoch.clone(),
                approved_candidate_digest: c.manifest_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    let refreshed = inbox(&store, A).await;
    assert_ne!(refreshed["source_snapshot_id"], first["source_snapshot_id"]);
    assert_ne!(
        item(&refreshed, "a")["contract_hash"],
        item(&first, "a")["contract_hash"]
    );
}

async fn round(admin: &Client, id: &str, index: i32, review_state: &str) {
    admin.execute("INSERT INTO awr_team.review_rounds(tenant_id,project_id,id,work_id,round_index,bundle_hash,contract_hash,author_actor_id,state)
        SELECT tenant_id,project_id,$1,work_id,$2,$1,contract_hash,'agent',$3 FROM awr_team.work_contracts WHERE work_id='a'",
        &[&id,&index,&review_state]).await.unwrap();
}

#[tokio::test]
async fn role_separation_current_review_and_rework_never_become_inferred_approval() {
    let (_g, admin, _, store) = setup().await;
    supervisor(&admin).await;
    round(&admin, "round-one", 1, "open").await;
    let open = inbox(&store, A).await;
    assert_eq!(item(&open, "a")["guidance"]["code"], "review");
    let selected: WorkstreamQuery =
        serde_json::from_value(item(&open, "a")["next_query"].clone()).unwrap();
    assert_eq!(
        store.query(TENANT, PROJECT, A, selected).await.unwrap()["data"]["review"]["round_id"],
        "round-one"
    );
    admin.batch_execute("UPDATE awr_team.review_rounds SET state='approved' WHERE id='round-one';
        UPDATE awr_team.project_memberships SET role='maintainer',business_roles='[\"deliverer\"]'::jsonb WHERE actor_id='agent'").await.unwrap();
    let approved = inbox(&store, A).await;
    assert_eq!(item(&approved, "a")["guidance"]["code"], "finalization");
    assert_ne!(
        item(&approved, "a")["item_key"],
        item(&open, "a")["item_key"]
    );
    let before = state(&admin).await;
    inbox(&store, A).await;
    assert_eq!(state(&admin).await, before);
    assert!(before["completions"].is_null());
    admin.batch_execute("UPDATE awr_team.project_memberships SET role='reader',business_roles='[\"observer\"]'::jsonb WHERE actor_id='agent'").await.unwrap();
    assert!(
        inbox(&store, A).await["data"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    admin.batch_execute("UPDATE awr_team.project_memberships SET role='reviewer',business_roles='[\"reviewer\"]'::jsonb WHERE actor_id='agent'").await.unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
    round(&admin, "round-two", 2, "open").await;
    assert!(
        item(&inbox(&store, A).await, "a").is_null(),
        "the author cannot decide its own review"
    );
    admin
        .batch_execute(
            "UPDATE awr_team.review_rounds SET author_actor_id='reviewer' WHERE id='round-two'",
        )
        .await
        .unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "review"
    );
    admin
        .batch_execute("UPDATE awr_team.review_rounds SET state='rejected' WHERE id='round-two'")
        .await
        .unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "rework"
    );
    admin
        .batch_execute(
            "UPDATE awr_team.work_contracts SET contract_hash=repeat('e',64) WHERE work_id='a'",
        )
        .await
        .unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
}

#[tokio::test]
async fn elapsed_claims_are_observations_but_unknown_effects_still_require_recovery() {
    let (_g, admin, _, store) = setup().await;
    supervisor(&admin).await;
    claim(&store, false).await;
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 minute'",
        )
        .await
        .unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
    round(&admin, "elapsed-review", 1, "open").await;
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "review"
    );
    for change in [
        "coordinator_epoch='previous'",
        "ownership_version=ownership_version+1",
        "fence=fence+1",
    ] {
        let original: Value = admin
            .query_one(
                "SELECT to_jsonb(c) FROM awr_team.claims c WHERE state='active'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        admin
            .batch_execute(&format!("UPDATE awr_team.claims SET {change}"))
            .await
            .unwrap();
        assert_eq!(
            item(&inbox(&store, A).await, "a")["guidance"]["code"],
            "recovery"
        );
        admin
            .execute(
                "UPDATE awr_team.claims SET coordinator_epoch=$1,ownership_version=$2,fence=$3",
                &[
                    &original["coordinator_epoch"].as_str().unwrap(),
                    &original["ownership_version"].as_i64().unwrap(),
                    &original["fence"].as_i64().unwrap(),
                ],
            )
            .await
            .unwrap();
    }
    admin
        .batch_execute("DELETE FROM awr_team.review_rounds WHERE id='elapsed-review'")
        .await
        .unwrap();
    admin.batch_execute("UPDATE awr_team.claims SET state='released';
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','unresolved-resource','a','named','fixture-effect','unknown')").await.unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "recovery"
    );
    admin.batch_execute("UPDATE awr_team.resource_reservations SET state='released';
        INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
        SELECT tenant_id,project_id,'unbound-pending-execution',work_id,1,contract_hash,'agent','unknown' FROM awr_team.work_contracts WHERE work_id='a'").await.unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "a")["next_query"]["op"],
        "work.recovery"
    );
    admin
        .batch_execute(
            "UPDATE awr_team.executions SET state='failed' WHERE id='unbound-pending-execution'",
        )
        .await
        .unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
}

#[tokio::test]
async fn paused_and_read_only_work_still_exposes_recovery_without_effect_authority() {
    let (_g, admin, _, store) = setup().await;
    supervisor(&admin).await;
    claim(&store, false).await;
    admin.batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=true WHERE work_id='a';
        UPDATE awr_team.project_memberships SET role='reader',business_roles='[\"observer\"]'::jsonb WHERE actor_id='agent';
        UPDATE awr_team.workstream_catalogs SET catalog_json=jsonb_set(catalog_json,'{workstreams,0,state}','\"paused\"')").await.unwrap();
    let before = state(&admin).await;
    let entry = item(&inbox(&store, A).await, "a");
    assert_eq!(entry["guidance"]["code"], "recovery");
    let next: WorkstreamQuery = serde_json::from_value(entry["next_query"].clone()).unwrap();
    assert!(store.query(TENANT, PROJECT, A, next).await.is_ok());
    assert_eq!(state(&admin).await, before);
}

#[tokio::test]
async fn feedback_blockers_follow_current_report_and_do_not_repeat_private_narrative() {
    let (_g, admin, _, store) = setup().await;
    supervisor(&admin).await;
    claim(&store, false).await;
    let p = prepare(&store, A, "a").await;
    store.commands().execute(TENANT,PROJECT,A,command(&p,"blocked-feedback","session.checkpoint",
        json!({"session_id":"session-a","expected_session_version":"1","context_hash":p["data"]["context_hash"],
            "next_action":"Await dependency clarification","open_loops":[],
            "progress":{"phase":"blocked","summary":"SYNTHETIC PRIVATE NARRATIVE","blockers":["Missing decision"]}}))).await.unwrap();
    let blocked = inbox(&store, A).await;
    assert_eq!(item(&blocked, "a")["guidance"]["code"], "blocked");
    assert!(!blocked.to_string().contains("SYNTHETIC PRIVATE NARRATIVE"));
    let p = prepare(&store, A, "a").await;
    store.commands().execute(TENANT,PROJECT,A,command(&p,"resolved-feedback","session.checkpoint",
        json!({"session_id":"session-a","expected_session_version":"2","context_hash":p["data"]["context_hash"],
            "next_action":"Continue implementation","open_loops":[],"progress":{"phase":"implementing","summary":"Clarification received"}}))).await.unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
}

async fn select_candidate(
    store: &WorkstreamReadStore,
    admin: &Client,
    db: &str,
) -> (DeliverySyncStore, DeliveryReadSet, DeliveryCandidate) {
    supervisor(admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let claim = claim(store, false).await;
    let p = prepare(store, A, "a").await;
    let set:DeliveryReadSet=serde_json::from_value(json!({"work_id":"a","workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],"authority_version":p["authority_version"],
        "ownership_version":p["data"]["ownership_version"],"contract_hash":p["data"]["contract_hash"]})).unwrap();
    let manifest = ArtifactManifest {
        entries: vec![ArtifactEntry {
            artifact_id: "package".into(),
            sha256: "e".repeat(64),
            byte_length: "12".into(),
            locator: "fixture://artifact/package".into(),
        }],
    };
    let candidate:DeliveryCandidate=serde_json::from_value(json!({"binding":{"tenant_id":TENANT,"project_id":PROJECT,
        "scope_id":"main","workstream_id":set.workstream_id,"work_id":"a","candidate_id":"candidate-a","candidate_version":"1",
        "contract_hash":set.contract_hash,"manifest_digest":manifest.digest().unwrap(),"source_revision":{"resource":"fixture://neutral-repository","format":"git_sha256","value":"a".repeat(64)},
        "required_checks":["report"],"target":{"resource":"fixture://neutral-repository","reference":"main","precondition":{"kind":"missing"}}},"manifest":manifest})).unwrap();
    let sync = DeliverySyncStore::from_config(common::with_app_role(&common::test_config(), db));
    sync.select_candidate(
        TENANT,
        PROJECT,
        A,
        SelectDeliveryCandidate {
            request_id: "select-inbox".into(),
            read_set: set.clone(),
            expected_selected_digest: None,
            session_id: "session-a".into(),
            claim_id: claim["claim_id"].as_str().unwrap().into(),
            fence: claim["fence"].as_str().unwrap().into(),
            lease_version: claim["lease_version"].as_str().unwrap().into(),
            candidate: candidate.clone(),
        },
    )
    .await
    .unwrap();
    sync.configure_connector(
        TENANT,
        PROJECT,
        A,
        ConfigureDeliveryConnector {
            request_id: "configure-inbox".into(),
            read_set: set.clone(),
            expected_connector_version: "0".into(),
            mapping: DeliveryConnectorMapping {
                connector_id: "neutral".into(),
                provider: "reference".into(),
                resource: "fixture://neutral-repository".into(),
                principal_actor_id: "agent".into(),
                principal_client_id: "cli-a".into(),
                fact_source: FactSource::CallerDeclared,
                enabled: true,
            },
        },
    )
    .await
    .unwrap();
    (sync, set, candidate)
}

#[tokio::test]
async fn current_neutral_check_changes_are_deduplicated_without_becoming_awr_acceptance() {
    let (_g, admin, db, store) = setup().await;
    let (sync, set, candidate) = select_candidate(&store, &admin, &db).await;
    let initial = inbox(&store, A).await;
    assert_eq!(item(&initial, "a")["guidance"]["code"], "verification");
    let mut keys = vec![item(&initial, "a")["item_key"].clone()];
    for (index, outcome) in [VerificationOutcome::Failed, VerificationOutcome::Passed]
        .into_iter()
        .enumerate()
    {
        let reserved = sync
            .reserve_inspection(
                TENANT,
                PROJECT,
                A,
                ReserveDeliveryInspection {
                    request_id: format!("inspect-{index}"),
                    read_set: set.clone(),
                    connector_id: "neutral".into(),
                    connector_version: "1".into(),
                    candidate_digest: candidate.binding.digest().unwrap(),
                    lease_seconds: 60,
                },
            )
            .await
            .unwrap();
        let request = IngestDeliveryFacts {
            request_id: format!("ingest-{index}"),
            read_set: set.clone(),
            connector_id: "neutral".into(),
            inspection_id: reserved["data"]["inspection_id"].as_str().unwrap().into(),
            event_id: format!("event-{index}"),
            records: vec![DeliveryEnvelope {
                protocol: DELIVERY_PROTOCOL.into(),
                protocol_version: DELIVERY_PROTOCOL_VERSION,
                record: DeliveryRecord::Verification(VerificationRun {
                    binding: candidate.binding.clone(),
                    run_id: "same-run".into(),
                    check: "report".into(),
                    outcome,
                    result_artifact: Some(candidate.manifest.entries[0].clone()),
                    provenance: FactProvenance {
                        source: FactSource::CallerDeclared,
                        reference: "fixture://check/report".into(),
                        observed_at_unix_ms: None,
                        recorded_at_unix_ms: 123,
                    },
                }),
            }],
        };
        sync.ingest_facts(TENANT, PROJECT, A, request.clone())
            .await
            .unwrap();
        let page = inbox(&store, A).await;
        assert_eq!(item(&page, "a")["guidance"]["code"], "verification");
        keys.push(item(&page, "a")["item_key"].clone());
        sync.ingest_facts(TENANT, PROJECT, A, request)
            .await
            .unwrap();
        assert_eq!(
            page["data"]["items"],
            inbox(&store, A).await["data"]["items"]
        );
    }
    assert_ne!(keys[0], keys[1]);
    assert_ne!(keys[1], keys[2]);
    assert!(state(&admin).await["completions"].is_null());
    round(&admin, "approved-before-change", 1, "approved").await;
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "finalization"
    );
    admin
        .batch_execute("UPDATE awr_team.work_runtime SET last_fence=last_fence+1 WHERE work_id='a'")
        .await
        .unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "delivery_changed"
    );
}

#[tokio::test]
async fn unsettled_integration_survives_reselection_but_confirmed_history_does_not_repeat() {
    let (_g, admin, db, store) = setup().await;
    select_candidate(&store, &admin, &db).await;
    admin.batch_execute("INSERT INTO awr_team.delivery_integration_intents(tenant_id,project_id,id,work_id,connector_id,request_json,eligibility_json,eligibility_digest,
        issuer_credential_id,issuer_secret_hash,issuer_authority_binding,state,dispatched_at)
        VALUES('reader-tenant','reader-project','original-intent','a','neutral','{}','{}',repeat('a',64),'reader-a','sha256:'||repeat('b',64),repeat('c',64),'unknown',clock_timestamp())").await.unwrap();
    let page = inbox(&store, A).await;
    assert_eq!(item(&page, "a")["guidance"]["code"], "integration_unknown");
    assert_eq!(
        item(&page, "a")["next_query"]["request_id"],
        "original-intent"
    );
    assert!(!page.to_string().contains("issuer_secret_hash"));
    admin
        .batch_execute(
            "UPDATE awr_team.delivery_selections SET selection_version=selection_version+1",
        )
        .await
        .unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "a")["next_query"]["request_id"],
        "original-intent"
    );
    admin
        .batch_execute("UPDATE awr_team.delivery_integration_intents SET state='rejected'")
        .await
        .unwrap();
    assert_ne!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "integration_unknown"
    );
}

#[tokio::test]
async fn response_budget_and_unicode_titles_are_bounded_without_truncating_required_context() {
    let (_g, admin, _, store) = setup().await;
    supervisor(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.work_contracts SET title=repeat('界',5000) WHERE work_id='a'",
        )
        .await
        .unwrap();
    let page = inbox(&store, A).await;
    assert_eq!(
        item(&page, "a")["title"].as_str().unwrap().chars().count(),
        160
    );
    assert_eq!(item(&page, "a")["title_truncated"], true);
    for entry in page["data"]["items"].as_array().unwrap() {
        assert!(entry["guidance"].to_string().len() < 1000);
        assert!(entry["guidance"]["because"].as_array().unwrap().len() <= 3);
        for key in ["when", "action", "recheck_on"] {
            assert!(!entry["guidance"][key].is_null());
        }
    }
    let bytes = serde_json::to_vec(&page).unwrap().len();
    assert!(bytes < 65536);
    let mut q = query("work.inbox");
    q.max_context_bytes = Some(bytes);
    assert!(store.query(TENANT, PROJECT, A, q.clone()).await.is_ok());
    q.max_context_bytes = Some(bytes - 1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ResponseTooLarge)
    ));
    let mut q = query("work.prepare");
    q.work_id = Some("a".into());
    q.max_context_bytes = Some(1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ContextIncomplete)
    ));
}

#[tokio::test]
async fn supervisor_and_reviewer_agents_require_exact_opt_in_action_and_write_grants() {
    let (_g, admin, db, store) = setup().await;
    agent_binding(&admin).await;
    admin.batch_execute("UPDATE awr_team.project_memberships SET role='maintainer',business_roles='[\"supervisor\"]'::jsonb,assignment_grant=true WHERE actor_id='agent'").await.unwrap();
    issue(&db, "read-a", task("a"), &[AuthorizedAction::Inspect]).await;
    issue(&db, "read-c", task("c"), &[AuthorizedAction::Inspect]).await;
    issue(&db, "assign-c", task("c"), &[AuthorizedAction::AssignWork]).await;
    let page = inbox(&store, A).await;
    assert!(item(&page, "a").is_null());
    assert_eq!(item(&page, "c")["guidance"]["code"], "dependency");
    issue(&db, "assign-a", task("a"), &[AuthorizedAction::AssignWork]).await;
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "assignment"
    );
    admin.batch_execute("UPDATE awr_team.project_memberships SET assignment_grant=false,business_roles='[\"reviewer\"]'::jsonb,membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
    round(&admin, "agent-review", 1, "open").await;
    admin
        .batch_execute("UPDATE awr_team.review_rounds SET author_actor_id='reviewer'")
        .await
        .unwrap();
    issue(&db, "review-a", task("a"), &[AuthorizedAction::Review]).await;
    assert!(item(&inbox(&store, A).await, "a").is_null());
    admin.batch_execute("UPDATE awr_team.project_memberships SET agent_review=true,membership_version=membership_version+1 WHERE actor_id='agent'").await.unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "review"
    );
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
}

#[tokio::test]
async fn confirmed_source_publication_and_closed_work_remove_completed_followups() {
    let (_g, admin, db, store) = setup().await;
    let (_, _, candidate) = select_candidate(&store, &admin, &db).await;
    let digest = candidate.binding.digest().unwrap();
    admin.execute("INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,delivery_candidate_digest)
        SELECT tenant_id,project_id,'fixture-completion',work_id,'main',contract_hash,'result','deps','evidence','review','[]',$1 FROM awr_team.work_contracts WHERE work_id='a'",
        &[&digest]).await.unwrap();
    admin.batch_execute("UPDATE awr_team.work_runtime SET state='completed',selected_completion_id='fixture-completion' WHERE work_id='a'").await.unwrap();
    assert_eq!(
        item(&inbox(&store, A).await, "a")["guidance"]["code"],
        "source_publication"
    );
    admin.batch_execute("INSERT INTO awr_team.delivery_source_cursors(tenant_id,project_id,source_snapshot_id,confirmed_fingerprint)
        SELECT tenant_id,project_id,source_snapshot_id,'sha256:'||repeat('a',64) FROM awr_team.delivery_selections WHERE work_id='a';
        INSERT INTO awr_team.delivery_source_publications(tenant_id,project_id,id,source_snapshot_id,work_id,actor_id,client_id,authority_binding,read_set_json,
            candidate_digest,selection_version,metadata_revision,note_json,filesystem_identity_json,before_fingerprint,after_fingerprint,before_bytes,after_bytes,
            projection_json,fence,expires_at,phase,confirmation_json)
        SELECT tenant_id,project_id,'fixture-publication',source_snapshot_id,work_id,'agent','cli-a',repeat('c',64),'{}',binding_digest,selection_version,1,'{}','{}',
            'sha256:'||repeat('a',64),'sha256:'||repeat('b',64),'','', '{}',1,clock_timestamp()+interval '1 minute','confirmed','{}'
        FROM awr_team.delivery_selections WHERE work_id='a'").await.unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
    admin.batch_execute("UPDATE awr_team.work_runtime SET state='cancelled',selected_completion_id=NULL WHERE work_id='a'").await.unwrap();
    assert!(item(&inbox(&store, A).await, "a").is_null());
}
