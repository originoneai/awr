use super::*;

async fn seed(admin: &tokio_postgres::Client) {
    admin
        .batch_execute(
            "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','reader-project','reviewer','Reviewer One','active');
        UPDATE awr_team.project_memberships SET business_roles='[\"reviewer\",\"observer\"]'
          WHERE actor_id='reviewer';",
        )
        .await
        .unwrap();
    for (id, work, kind, days, state) in [
        ("old-role", "a", None, 9, "open"),
        (
            "named",
            "a",
            Some(json!({"kind":"member","person_id":"reviewer"})),
            3,
            "open",
        ),
        (
            "recent-role",
            "c",
            Some(json!({"kind":"role","role":"reviewer"})),
            1,
            "open",
        ),
        ("private", "b-private", None, 20, "open"),
        ("finished", "a", None, 30, "approved"),
    ] {
        admin.execute("INSERT INTO awr_team.review_rounds(tenant_id,project_id,id,work_id,round_index,
          state,bundle_hash,contract_hash,author_actor_id,requested_reviewer,created_at)
          SELECT c.tenant_id,c.project_id,$1,c.work_id,$2,$3,repeat('a',64),c.contract_hash,'agent',$4,
            transaction_timestamp()-make_interval(days=>$5)
          FROM awr_team.work_contracts c JOIN awr_team.projects p
            ON p.tenant_id=c.tenant_id AND p.id=c.project_id AND p.active_snapshot_id=c.snapshot_id
          WHERE c.tenant_id=$6 AND c.project_id=$7 AND c.work_id=$8",
          &[&id,&days,&state,&kind,&days,&TENANT,&PROJECT,&work]).await.unwrap();
    }
}

#[tokio::test]
async fn queue_counts_maximum_wait_and_legacy_role_without_disclosing_hidden_work() {
    let (_guard, admin, _, store) = setup().await;
    seed(&admin).await;
    let result = store
        .query(TENANT, PROJECT, A, query("review.queue"))
        .await
        .unwrap();
    let data = &result["data"];
    assert_eq!(data["total"], 3);
    assert_eq!(data["items"][0]["round_id"], "old-role");
    assert_eq!(
        data["items"][0]["requested_reviewer"],
        json!({"kind":"role","role":"reviewer"})
    );
    let groups = data["groups"].as_array().unwrap();
    let role = groups
        .iter()
        .find(|g| g["requested_reviewer"]["kind"] == "role")
        .unwrap();
    assert_eq!(role["queue_count"], 2);
    assert!(role["oldest_wait_ms"].as_i64().unwrap() >= 9 * 86400000);
    assert!(role["oldest_wait_ms"].as_i64().unwrap() < 9 * 86400000 + 60000);
    assert!(!result.to_string().contains("b-private"));
    assert!(!result.to_string().contains("finished"));
    let roster = store
        .query(TENANT, PROJECT, A, query("team.reviewers"))
        .await
        .unwrap();
    assert_eq!(
        roster["data"]["items"],
        json!([{"name":"Reviewer One","review_duty":"reviewer","queue_count":3}])
    );
    // The exact object equality above also protects every non-review duty and identity field.
    let mut q = query("review.queue");
    q.recipient_person_id = Some("reviewer".into());
    assert_eq!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["total"],
        3
    );
    let mut q = query("review.rounds");
    q.work_id = Some("a".into());
    let history = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(
        history["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["round_id"] == "named")
            .unwrap()["requested_reviewer"]["person_id"],
        "reviewer"
    );
    let mut q = query("review.inspect");
    q.work_id = Some("a".into());
    q.review_round_id = Some("named".into());
    let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(
        inspected["data"]["review"]["requested_reviewer"]["person_id"],
        "reviewer"
    );
    assert_eq!(inspected["data"]["review"]["decisions"], json!([]));
}

#[tokio::test]
async fn review_discovery_rejects_nonmembers_cross_project_and_out_of_scope_reads() {
    let (_guard, admin, _, store) = setup().await;
    seed(&admin).await;
    for op in ["review.queue", "team.reviewers"] {
        assert!(matches!(
            store.query(TENANT, PROJECT, NONE, query(op)).await,
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            store.query("other-tenant", PROJECT, A, query(op)).await,
            Err(PgError::Forbidden)
        ));
        let mut q = query(op);
        q.work_id = Some("b-private".into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
        let mut q = query(op);
        q.workstream_id = Some(Id::from(2));
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    admin.batch_execute("INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES('reader-tenant','outsider','human','Outsider','active'); UPDATE awr_team.credentials SET actor_id='outsider' WHERE id='reader-a';").await.unwrap();
    for op in ["review.queue", "team.reviewers"] {
        assert!(matches!(
            store.query(TENANT, PROJECT, A, query(op)).await,
            Err(PgError::Forbidden)
        ));
    }
}

#[tokio::test]
async fn review_queue_pages_bind_recipient_roster_scope_and_round_changes() {
    let (_guard, admin, _, store) = setup().await;
    seed(&admin).await;
    let mut q = query("review.queue");
    q.limit = Some(1);
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    q.cursor = first["data"]["next_cursor"].as_str().map(Into::into);
    let next = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(next["data"]["items"][0]["round_id"], "named");
    admin.execute("UPDATE awr_team.review_rounds SET requested_reviewer='{\"kind\":\"member\",\"person_id\":\"reviewer\"}' WHERE id='old-role'",&[]).await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::CursorExpired)
    ));
    let mut q = query("review.queue");
    q.max_context_bytes = Some(1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ResponseTooLarge)
    ));
}

#[tokio::test]
async fn queue_delegation_limits_counts_to_the_exact_authorized_task_and_rechecks_revocation() {
    use awr_core::{
        AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
        ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
    };
    let (_guard, admin, db, store) = setup().await;
    seed(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
      INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
        VALUES('reader-tenant','reader-project','owner','Owner','active');
      INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
        VALUES('reader-tenant','reader-project','binding','owner','agent','active');",
        )
        .await
        .unwrap();
    let authorization = AgentAuthorization {
        id: "queue-task-read".into(),
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
        actions: std::collections::BTreeSet::from([AuthorizedAction::Inspect]),
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
                request_key: "queue-read".into(),
                authorization,
            },
        )
        .await
        .unwrap();
    let result = store
        .query(TENANT, PROJECT, A, query("review.queue"))
        .await
        .unwrap();
    assert_eq!(result["data"]["total"], 2);
    assert!(
        result["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["work_id"] == "a")
    );
    let roster = store
        .query(TENANT, PROJECT, A, query("team.reviewers"))
        .await
        .unwrap();
    assert_eq!(roster["data"]["items"][0]["queue_count"], 2);
    for op in ["review.queue", "team.reviewers"] {
        let mut q = query(op);
        q.work_id = Some("c".into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, q).await,
            Err(PgError::Forbidden)
        ));
    }
    admin
        .execute(
            "UPDATE awr_team.agent_authorizations SET status='revoked' WHERE id='queue-task-read'",
            &[],
        )
        .await
        .unwrap();
    for op in ["review.queue", "team.reviewers"] {
        assert!(matches!(
            store.query(TENANT, PROJECT, A, query(op)).await,
            Err(PgError::Forbidden)
        ));
    }
}

#[tokio::test]
async fn requested_reviewer_storage_constraint_rejects_partial_and_extra_shapes() {
    let (_guard, admin, _, _store) = setup().await;
    seed(&admin).await;
    for target in [
        json!({}),
        json!({"kind":"member"}),
        json!({"person_id":"reviewer"}),
        json!({"kind":"role","role":"administrator"}),
        json!({"kind":"member","person_id":"reviewer","extra":true}),
    ] {
        assert!(
            admin
                .execute(
                    "UPDATE awr_team.review_rounds SET requested_reviewer=$1 WHERE id='named'",
                    &[&target]
                )
                .await
                .is_err(),
            "{target}"
        );
    }
}

#[tokio::test]
async fn reviewer_names_are_available_to_an_ordinary_project_reader() {
    let (_guard, admin, _, store) = setup().await;
    seed(&admin).await;
    admin.batch_execute("UPDATE awr_team.project_memberships SET role='reader',business_roles='[\"observer\"]',membership_version=membership_version+1 WHERE actor_id='agent';").await.unwrap();
    let roster = store
        .query(TENANT, PROJECT, A, query("team.reviewers"))
        .await
        .unwrap();
    assert_eq!(
        roster["data"]["items"],
        json!([{"name":"Reviewer One","review_duty":"reviewer","queue_count":3}])
    );
}
