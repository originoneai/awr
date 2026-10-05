#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::PgError;
use fixture::*;
use serde_json::{Value, json};

#[tokio::test]
async fn navigation_and_audit_obey_identity_visibility_and_stable_cursors() {
    let (_g, owner, _db, store) = setup().await;
    enable_writes(&owner).await;
    let initial = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(initial["data"]["items"][0]["navigation"], "prepare");
    assert_eq!(
        initial["data"]["items"][1]["navigation"],
        "waiting_dependency"
    );
    owner.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state) SELECT tenant_id,project_id,scope_id,work_id,'pending' FROM awr_team.work_contracts ON CONFLICT DO NOTHING").await.unwrap();
    let caps = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    assert_eq!(caps["identity"]["actor_id"], "agent");
    assert_eq!(caps["identity"]["can_manage_members"], false);
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(next["data"]["resume"][0]["session_id"], "session-a");
    assert!(!next.to_string().contains("session-b"));
    assert!(!next.to_string().contains("b-private"));
    assert_eq!(next["data"]["execution_authorized"], false);
    let items = next["data"]["items"].as_array().unwrap();
    assert_eq!(
        items.iter().find(|w| w["work_id"] == "a").unwrap()["navigation"],
        "prepare"
    );
    assert_eq!(
        items.iter().find(|w| w["work_id"] == "c").unwrap()["navigation"],
        "waiting_dependency"
    );
    let mut q = query("work.next");
    q.limit = Some(1);
    let page = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    q.cursor = page["data"]["next_cursor"].as_str().map(str::to_owned);
    let page2 = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_ne!(
        page["data"]["items"][0]["work_id"],
        page2["data"]["items"][0]["work_id"]
    );
    assert!(matches!(
        store.query(TENANT, PROJECT, B, q).await,
        Err(PgError::CursorExpired)
    ));
    owner.batch_execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version) SELECT tenant_id,project_id,'other-session',scope_id,work_id,'reviewer','review-cli','other-conversation','active',workstream_id,ownership_version FROM awr_team.sessions WHERE id='session-a'; INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,expires_at,state) VALUES('reader-tenant','reader-project','other-claim','main','a','other-session','reviewer',1,clock_timestamp()+interval '5 minutes','active')").await.unwrap();
    let held = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(held["data"]["items"][0]["navigation"], "held");
    owner.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 minute' WHERE id='other-claim'").await.unwrap();
    let expired = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(
        expired["data"]["items"][0]["navigation"],
        "recovery_required"
    );
    owner.batch_execute("DELETE FROM awr_team.claims WHERE id='other-claim'; DELETE FROM awr_team.sessions WHERE id='other-session'").await.unwrap();
    let prepared = prepare(&store, A, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "navigation-start",
                "session.start",
                json!({"conversation_id":"navigation"}),
            ),
        )
        .await
        .unwrap();
    let development = store
        .query(TENANT, PROJECT, A, query("audit.development"))
        .await
        .unwrap();
    assert!(
        development["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "session.start"
                && r["client_id"] == "cli-a"
                && r["work_id"] == "a")
    );
    assert!(!development.to_string().contains("payload"));
    assert!(!development.to_string().contains("b-private"));
    let first = store
        .request_audit_begin(TENANT, PROJECT, A, "work.prepare", Some("a"))
        .await
        .unwrap();
    store
        .request_audit_finish(TENANT, PROJECT, &first, "succeeded")
        .await
        .unwrap();
    let second = store
        .request_audit_begin(TENANT, PROJECT, A, "access.preview", Some("b-private"))
        .await
        .unwrap();
    store
        .request_audit_finish(TENANT, PROJECT, &second, "denied")
        .await
        .unwrap();
    let mut q = query("audit.requests");
    q.limit = Some(1);
    let page = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(page["data"]["items"][0]["result"], "denied");
    assert!(page["data"]["items"][0]["work_id"].is_null());
    assert_eq!(page["data"]["items"][0]["credential_id"], "reader-a");
    q.cursor = page["data"]["next_cursor"].as_str().map(str::to_owned);
    // A newer request must not repeat or displace already paged records.
    let unknown = store
        .request_audit_begin(TENANT, PROJECT, A, "work.next", None)
        .await
        .unwrap();
    let next = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(next["data"]["items"][0]["id"], first);
    let all = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert!(
        all["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == unknown && r["result"] == "unknown")
    );
    assert!(!all.to_string().contains(A));
    assert!(!all.to_string().contains("secret_hash"));
    owner.batch_execute("UPDATE awr_team.project_memberships SET role='developer',membership_version=membership_version+1 WHERE actor_id='agent'; INSERT INTO awr_team.request_audit(tenant_id,project_id,id,actor_id,client_id,credential_id,action) VALUES('reader-tenant','reader-project','other-record','reviewer','review-cli','review-credential','work.next')").await.unwrap();
    let own = store
        .query(TENANT, PROJECT, A, query("audit.requests"))
        .await
        .unwrap();
    assert_eq!(own["data"]["scope"], "self");
    assert!(
        own["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["actor_id"] == "agent")
    );
    let mut other = query("audit.requests");
    other.member_actor_id = Some("reviewer".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, other).await,
        Err(PgError::Forbidden)
    ));
    let mut other = query("audit.development");
    other.member_actor_id = Some("reviewer".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, other).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        store.query(TENANT, PROJECT, NONE, query("work.next")).await,
        Err(PgError::Forbidden)
    ));
    let rows: Value = owner
        .query_one(
            "SELECT coalesce(jsonb_agg(to_jsonb(a)), '[]'::jsonb) FROM awr_team.request_audit a",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!rows.to_string().contains(A));
}

async fn start_owned_run(store: &awr_team_pg::WorkstreamReadStore) -> (Value, Value) {
    let claim = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepare(store, A, "a").await,
                "resume-claim",
                "claim.acquire",
                json!({"session_id":"session-a","expected_session_version":"1",
                "expected_work_version":"0","ttl_seconds":3600}),
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let p = prepare(store, A, "a").await;
    let intent = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "resume-intent",
                "execution.prepare",
                json!({"session_id":"session-a","expected_session_version":"1",
                "claim_id":claim["claim_id"],"expected_fence":claim["fence"],
                "expected_lease_version":claim["lease_version"],
                "expected_work_version":p["data"]["runtime"]["work_version"],
                "input_digest":"a".repeat(64),"declared_scope":["src/api"]}),
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let p = prepare(store, A, "a").await;
    let started = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "resume-start",
                "execution.start",
                json!({"session_id":"session-a","expected_session_version":"1",
                "execution_id":intent["execution_id"],
                "expected_execution_version":intent["execution_version"],
                "claim_id":claim["claim_id"],"expected_fence":claim["fence"],
                "expected_lease_version":claim["lease_version"],
                "expected_work_version":p["data"]["runtime"]["work_version"],
                "execution_mode":"caller_managed"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(started["execution_authorized"], true);
    (claim, started["receipt"]["data"].clone())
}

async fn execution_state(owner: &tokio_postgres::Client) -> Value {
    owner
        .query_one(
            "SELECT jsonb_build_object(
        'sessions',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM awr_team.sessions s),
        'claims',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM awr_team.claims c),
        'executions',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.executions e),
        'work',(SELECT jsonb_agg(to_jsonb(w) ORDER BY work_id) FROM awr_team.work_runtime w),
        'operations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM awr_team.operations o),
        'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.events e))",
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

fn own_query(op: &str) -> awr_team_pg::WorkstreamQuery {
    let mut q = query(op);
    q.work_id = Some("a".into());
    q.session_id = Some("session-a".into());
    q
}

#[tokio::test]
async fn native_resume_navigation_and_checkpoint_reads_use_current_guidance_without_new_effects() {
    let (_g, owner, _, store) = setup().await;
    enable_writes(&owner).await;
    let (claim, _) = start_owned_run(&store).await;
    let before = execution_state(&owner).await;
    let observed = store
        .query(TENANT, PROJECT, A, own_query("work.observe"))
        .await
        .unwrap();
    let navigation = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(navigation["data"]["resume"][0]["session_id"], "session-a");
    assert_eq!(
        navigation["data"]["resume"][0]["next_query"]["op"],
        "work.observe"
    );
    assert_eq!(navigation["data"]["items"][0]["navigation"], "resume");
    assert_eq!(
        navigation["data"]["items"][0]["next_query"]["op"],
        "work.observe"
    );
    assert_eq!(navigation["data"]["execution_authorized"], false);
    assert_eq!(observed["data"]["claim"]["id"], claim["claim_id"]);
    assert_eq!(observed["data"]["execution"]["lease_live"], true);
    for op in ["session.inspect", "work.recovery"] {
        let history = store
            .query(TENANT, PROJECT, A, own_query(op))
            .await
            .unwrap();
        assert_eq!(history["data"]["guidance"], observed["data"]["guidance"]);
        assert_eq!(history["data"]["checkpoint_actions_are_historical"], true);
        assert_eq!(history["data"]["automatic_resume"], false);
        assert_eq!(history["data"]["items"][0]["next_action"], "continue alpha");
        assert!(history["data"]["guidance"].to_string().len() < 900);
    }
    assert_eq!(execution_state(&owner).await, before);
}

#[tokio::test]
async fn concurrent_navigation_preserves_owned_resume_and_peer_visibility() {
    let (_g, owner, _, store) = setup().await;
    enable_writes(&owner).await;
    start_owned_run(&store).await;
    owner.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-b',$3,1,true)", &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let before = execution_state(&owner).await;
    let (own, peer) = tokio::join!(
        store.query(TENANT, PROJECT, A, query("work.next")),
        store.query(TENANT, PROJECT, B, query("work.next"))
    );
    let own = own.unwrap();
    let peer = peer.unwrap();
    assert_eq!(own["data"]["items"][0]["navigation"], "resume");
    let other = peer["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["work_id"] == "a")
        .unwrap();
    assert_ne!(other["navigation"], "resume");
    assert!(!peer["data"]["resume"].to_string().contains("session-a"));
    assert!(!own.to_string().contains("b-private"));
    for result in [own, peer] {
        assert_eq!(result["data"]["execution_authorized"], false);
    }
    assert_eq!(execution_state(&owner).await, before);
}

#[tokio::test]
async fn expired_run_guidance_never_revives_the_lease_or_repeats_execution() {
    let (_g, owner, _, store) = setup().await;
    enable_writes(&owner).await;
    let (claim, _) = start_owned_run(&store).await;
    owner.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second' WHERE work_id='a'").await.unwrap();
    let before = execution_state(&owner).await;
    let navigation = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(
        navigation["data"]["items"][0]["navigation"],
        "recovery_required"
    );
    let observed = store
        .query(TENANT, PROJECT, A, own_query("work.observe"))
        .await
        .unwrap();
    assert_eq!(observed["data"]["claim"]["lease_live"], false);
    assert_eq!(observed["data"]["execution"]["state"], "running");
    for op in ["session.inspect", "work.recovery", "work.observe"] {
        let current = store
            .query(TENANT, PROJECT, A, own_query(op))
            .await
            .unwrap();
        let guidance = &current["data"]["guidance"];
        assert_eq!(guidance["code"], "inspect_expired_claim");
        assert!(
            guidance["action"]["note"]
                .as_str()
                .unwrap()
                .contains("cannot be renewed")
        );
        assert!(
            guidance["action"]["note"]
                .as_str()
                .unwrap()
                .contains("settle")
        );
        assert!(guidance.to_string().len() < 900);
    }
    assert_eq!(execution_state(&owner).await, before);
    let p = prepare(&store, A, "a").await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &p,
                    "expired-renew",
                    "claim.renew",
                    json!({
            "session_id":"session-a","expected_session_version":"1",
            "claim_id":claim["claim_id"],"expected_fence":claim["fence"],
            "expected_lease_version":claim["lease_version"],"ttl_seconds":3600})
                )
            )
            .await,
        Err(PgError::LeaseExpired)
    ));
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &p,
                    "expired-reacquire",
                    "claim.acquire",
                    json!({
            "session_id":"session-a","expected_session_version":"1",
            "expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":3600})
                )
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(execution_state(&owner).await, before);
}

#[tokio::test]
async fn current_resume_guidance_rechecks_epoch_recovery_and_revoked_permissions() {
    let (_g, owner, _, store) = setup().await;
    enable_writes(&owner).await;
    start_owned_run(&store).await;
    owner
        .batch_execute(
            "UPDATE awr_team.executions SET contract_hash=repeat('d',64) WHERE work_id='a'",
        )
        .await
        .unwrap();
    let changed = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(
        changed["data"]["items"][0]["navigation"],
        "recovery_required"
    );
    let changed = store
        .query(TENANT, PROJECT, A, own_query("work.recovery"))
        .await
        .unwrap();
    assert_eq!(changed["data"]["guidance"]["code"], "refresh_execution");
    owner.batch_execute("UPDATE awr_team.executions e SET contract_hash=c.contract_hash
        FROM awr_team.work_contracts c JOIN awr_team.projects p ON p.tenant_id=c.tenant_id AND p.id=c.project_id AND p.active_snapshot_id=c.snapshot_id
        WHERE e.tenant_id=c.tenant_id AND e.project_id=c.project_id AND e.work_id=c.work_id AND e.work_id='a'").await.unwrap();
    owner.batch_execute("UPDATE awr_team.projects SET coordinator_epoch='epoch-after-restart' WHERE id='reader-project' AND tenant_id='reader-tenant'").await.unwrap();
    let navigation = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(
        navigation["data"]["items"][0]["navigation"],
        "recovery_required"
    );
    for op in ["session.inspect", "work.recovery"] {
        let current = store
            .query(TENANT, PROJECT, A, own_query(op))
            .await
            .unwrap();
        assert_eq!(current["data"]["guidance"]["code"], "refresh_execution");
    }
    owner.batch_execute("UPDATE awr_team.workstream_grants SET active=false,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    for q in [
        query("work.next"),
        own_query("session.inspect"),
        own_query("work.recovery"),
    ] {
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    owner.batch_execute("UPDATE awr_team.workstream_grants SET active=true,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    let recovered = store
        .query(TENANT, PROJECT, A, own_query("work.recovery"))
        .await
        .unwrap();
    assert_eq!(recovered["data"]["guidance"]["code"], "refresh_execution");
    assert_eq!(recovered["data"]["items"][0]["session_id"], "session-a");
}

#[tokio::test]
async fn task_delegated_agent_can_resume_its_own_live_run_without_borrowing_authority() {
    use awr_core::*;
    use awr_team_pg::AuthorizationStore;
    use std::collections::BTreeSet;

    let (_g, owner, db, store) = setup().await;
    owner.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES('reader-tenant','reader-project','owner','Owner','active');
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','binding','owner','agent','active')").await.unwrap();
    enable_writes(&owner).await;
    let authorization = AgentAuthorization {
        id: "resume-authorization".into(),
        authorizer_person_id: PersonId::new("owner").unwrap(),
        responsible_person_id: PersonId::new("owner").unwrap(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: AuthorizationScope::Task {
            project_id: PROJECT.into(),
            work_item_id: "a".into(),
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
        binding_id: Some("binding".into()),
    };
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "resume-grant".into(),
                authorization,
            },
        )
        .await
        .unwrap();
    start_owned_run(&store).await;
    let before = execution_state(&owner).await;
    let current = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    assert_eq!(current["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(current["data"]["items"][0]["navigation"], "resume");
    assert_eq!(current["data"]["execution_authorized"], false);
    assert!(!current.to_string().contains("b-private"));
    for op in ["session.inspect", "work.recovery"] {
        let recovered = store
            .query(TENANT, PROJECT, A, own_query(op))
            .await
            .unwrap();
        assert!(!recovered["data"]["guidance"].is_null());
    }
    assert_eq!(execution_state(&owner).await, before);
}
