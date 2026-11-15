use super::*;

async fn dependency_contract(
    admin: &tokio_postgres::Client,
    work: &str,
    required: &[&str],
) -> String {
    let row = admin.query_one("SELECT contract_json FROM awr_team.work_contracts c
        JOIN awr_team.projects p ON p.tenant_id=c.tenant_id AND p.id=c.project_id AND p.active_snapshot_id=c.snapshot_id
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$3 AND c.scope_id='main'",
        &[&TENANT,&PROJECT,&work]).await.unwrap();
    let mut contract: awr_team::WorkContract = serde_json::from_value(row.get(0)).unwrap();
    contract.required_dependencies = required.iter().map(|work| (*work).into()).collect();
    contract.dependency_acceptance.clear();
    let hash = contract.hash().unwrap();
    admin
        .execute(
            "UPDATE awr_team.work_contracts SET contract_json=$4,contract_hash=$5
        WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
            &[
                &TENANT,
                &PROJECT,
                &work,
                &serde_json::to_value(contract).unwrap(),
                &hash,
            ],
        )
        .await
        .unwrap();
    hash
}

async fn dependency_completion(admin: &tokio_postgres::Client, work: &str, receipt: &str) {
    admin.execute("INSERT INTO awr_team.completion_receipts(tenant_id,project_id,id,work_id,scope_id,
        contract_hash,result_digest,dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json)
        SELECT $1,$2,$4,$3,'main',c.contract_hash,repeat('a',64),repeat('b',64),repeat('c',64),'review','{}'
        FROM awr_team.work_contracts c JOIN awr_team.projects p
          ON p.tenant_id=c.tenant_id AND p.id=c.project_id AND p.active_snapshot_id=c.snapshot_id
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$3 AND c.scope_id='main'",
        &[&TENANT,&PROJECT,&work,&receipt]).await.unwrap();
    admin.execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,selected_completion_id)
        VALUES($1,$2,'main',$3,'completed',$4) ON CONFLICT(tenant_id,project_id,scope_id,work_id)
        DO UPDATE SET state='completed',selected_completion_id=EXCLUDED.selected_completion_id",
        &[&TENANT,&PROJECT,&work,&receipt]).await.unwrap();
}

async fn dependency_command(
    store: &WorkstreamReadStore,
    work: &str,
    key: &str,
    op: &str,
    args: Value,
) -> Value {
    let p = prepare(store, A, work).await;
    Box::pin(
        store
            .commands()
            .execute(TENANT, PROJECT, A, command(&p, key, op, args)),
    )
    .await
    .unwrap_or_else(|error| panic!("{op}: {error:?}"))["receipt"]["data"]
        .clone()
}

async fn consume_dependency(store: &WorkstreamReadStore, work: &str) -> Value {
    let session = dependency_command(
        store,
        work,
        "dependency-session",
        "session.start",
        json!({"conversation_id":"dependency-observation"}),
    )
    .await;
    let p = prepare(store, A, work).await;
    let claim = dependency_command(
        store,
        work,
        "dependency-claim",
        "task.claim_available",
        json!({
        "session_id":session["session_id"],"expected_session_version":"1","ttl_seconds":600,
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
        "expected_responsibility_version":p["data"]["responsibility"]["version"]}),
    )
    .await;
    let p = prepare(store, A, work).await;
    let intent = dependency_command(store,work,"dependency-intent","execution.prepare",json!({
        "session_id":session["session_id"],"expected_session_version":"1",
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
        "input_digest":"d".repeat(64),"declared_scope":["src"]})).await;
    let p = prepare(store, A, work).await;
    dependency_command(store,work,"dependency-start","execution.start",json!({
        "session_id":session["session_id"],"expected_session_version":"1",
        "execution_id":intent["execution_id"],"expected_execution_version":intent["execution_version"],
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"execution_mode":"caller_managed"})).await
}

async fn dependency_read(store: &WorkstreamReadStore, work: &str) -> Value {
    let mut q = query("dependencies.list");
    q.work_id = Some(work.into());
    store.query(TENANT, PROJECT, A, q).await.unwrap()
}

async fn assert_delivery_parity(store: &WorkstreamReadStore, work: &str, page: &Value) {
    let mut q = query("delivery.inspect");
    q.work_id = Some(work.into());
    let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap();
    let mut listed = page["data"]["items"].as_array().unwrap().clone();
    for item in &mut listed {
        item.as_object_mut().unwrap().remove("dependency_position");
        item.as_object_mut().unwrap().remove("work_title");
    }
    assert_eq!(
        json!(listed),
        inspected["data"]["delivery"]["dependencies"],
        "Dependency listing diverged from delivery inspection"
    );
}

#[tokio::test]
async fn dependency_versions_require_recorded_inputs_and_match_delivery_inspection() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    dependency_contract(&admin, "c", &["a"]).await;
    dependency_completion(&admin, "a", "receipt-one").await;
    let before = dependency_read(&store, "c").await;
    let item = &before["data"]["items"][0];
    assert_eq!(item["validity"], "valid");
    assert_eq!(item["available_version"]["receipt_id"], "receipt-one");
    assert!(item["adopted_version"].is_null());
    assert!(item["consumed_version"].is_null());
    assert_eq!(item["missing"]["adopted_version"], "not_recorded");
    assert_delivery_parity(&store, "c", &before).await;
    let start = Box::pin(consume_dependency(&store, "c")).await;
    assert_eq!(start["dependency_receipts"], json!([["a", "receipt-one"]]));
    let consumed = dependency_read(&store, "c").await;
    assert_eq!(
        consumed["data"]["items"][0]["adopted_version"]["receipt_id"],
        "receipt-one"
    );
    assert_delivery_parity(&store, "c", &consumed).await;
    dependency_completion(&admin, "a", "receipt-two").await;
    let changed = dependency_read(&store, "c").await;
    let item = &changed["data"]["items"][0];
    assert_eq!(item["validity"], "upstream_changed");
    assert_eq!(item["reason"], "execution_inputs_changed");
    assert_eq!(item["available_version"]["receipt_id"], "receipt-two");
    assert_eq!(item["consumed_version"]["receipt_id"], "receipt-one");
    assert_delivery_parity(&store, "c", &changed).await;
}

#[tokio::test]
async fn dependency_observations_preserve_unreadable_edges_with_reasoned_nulls() {
    let (_g, admin, _, store) = setup().await;
    dependency_completion(&admin, "b-private", "private-receipt").await;
    let before: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.events", &[])
        .await
        .unwrap()
        .get(0);
    let page = dependency_read(&store, "c").await;
    let item = &page["data"]["items"][0];
    assert_eq!(item["provider_work_id"], "b-private");
    assert_eq!(item["validity"], "unavailable");
    assert_eq!(item["reason"], "permission_restricted");
    for field in [
        "provider_title",
        "required_version",
        "available_version",
        "adopted_version",
        "consumed_version",
    ] {
        assert!(item[field].is_null(), "{field}");
        let reason = if matches!(field, "adopted_version" | "consumed_version") {
            "not_recorded"
        } else {
            "permission_restricted"
        };
        assert_eq!(item["missing"][field], reason);
    }
    assert!(!page.to_string().contains("private-receipt"));
    assert_delivery_parity(&store, "c", &page).await;
    let after: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.events", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after);
}

#[tokio::test]
async fn dependency_pages_are_project_scoped_bounded_and_reauthorize_cursors() {
    let (_g, admin, _, store) = setup_with_assigned_tasks().await;
    for work in ["c", "aa-assigned", "ab-assigned"] {
        dependency_contract(&admin, work, &["a"]).await;
    }
    let mut q = query("dependencies.list");
    q.limit = Some(1);
    let mut ids = Vec::new();
    for position in 1..=3 {
        let page = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
        assert_eq!(page["data"]["total"], 3);
        assert_eq!(page["data"]["items"][0]["dependency_position"], position);
        assert_eq!(page["data"]["count_limited"], true);
        assert!(page["data"]["hidden_count"].is_null());
        assert_eq!(
            page["data"]["as_of"]["project_revision"],
            page["project_revision"]
        );
        assert_eq!(
            page["data"]["as_of"]["source_snapshot_id"],
            page["source_snapshot_id"]
        );
        ids.push(
            page["data"]["items"][0]["work_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        q.cursor = page["data"]["next_cursor"].as_str().map(Into::into);
    }
    assert_eq!(ids, vec!["aa-assigned", "ab-assigned", "c"]);
    assert!(q.cursor.is_none());
    let first = store.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    q.cursor = first["data"]["next_cursor"].as_str().map(Into::into);
    admin.batch_execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q.clone()).await,
        Err(PgError::CursorExpired)
    ));
    q.cursor = None;
    q.max_context_bytes = Some(1);
    assert!(matches!(
        store.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ResponseTooLarge)
    ));
    let mut foreign = query("dependencies.list");
    foreign.work_id = Some("b-private".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, foreign).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn dependency_binding_invalidation_cannot_appear_ready() {
    let (_g, admin, _, store) = setup().await;
    dependency_contract(&admin, "c", &["a"]).await;
    dependency_completion(&admin, "a", "receipt-one").await;
    admin.execute("INSERT INTO awr_team.dependency_bindings(tenant_id,project_id,downstream_work_id,upstream_work_id,binding_hash,valid)
        VALUES($1,$2,'c','a',repeat('a',64),false)",&[&TENANT,&PROJECT]).await.unwrap();
    let page = dependency_read(&store, "c").await;
    assert_eq!(page["data"]["items"][0]["validity"], "upstream_changed");
    assert_eq!(
        page["data"]["items"][0]["reason"],
        "dependency_binding_invalidated"
    );
    assert_delivery_parity(&store, "c", &page).await;
}

async fn cross_dependency_fixture(
    policy: awr_core::DeliveryVersionPolicy,
) -> (integration_fixture::Fixture, Value) {
    use awr_core::{
        AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
        ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
    };
    let f = Box::pin(integration_fixture::setup_simulated_report_integration(
        "Reviewed dependency report",
    ))
    .await;
    f.integrate().await;
    let receipt = Box::pin(integration_fixture::run(
        &f.reads,
        integration_fixture::SUPERVISOR,
        "dependency-complete",
        "work.complete",
        json!({"session_id":"session-supervisor","expected_session_version":"1",
            "evidence_id":f.evidence["evidence_id"],"context_complete":true}),
    ))
    .await;
    let row = f
        .admin
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='b-private'",
            &[],
        )
        .await
        .unwrap();
    let mut contract: awr_team::WorkContract = serde_json::from_value(row.get(0)).unwrap();
    contract.codec = awr_team::WorkContract::CODEC_V6.into();
    contract.required_dependencies = vec!["a".into()];
    contract.dependency_acceptance.insert(
        "a".into(),
        awr_team::DependencyAcceptanceMode::CrossWorkstream(
            awr_team::CrossWorkstreamDependencyPolicy {
                review_assurance:
                    awr_team::CrossWorkstreamReviewAssurance::SimulatedMemberIndependent,
                version_policy: policy,
            },
        ),
    );
    let hash = contract.hash().unwrap();
    f.admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='b-private'",
        &[&json!(contract),&hash]).await.unwrap();
    AuthorizationStore::from_config(f.config.clone())
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "dependency-reader".into(),
                authorization: AgentAuthorization {
                    id: "dependency-reader".into(),
                    authorizer_person_id: PersonId::new("member-a").unwrap(),
                    responsible_person_id: PersonId::new("member-a").unwrap(),
                    subject_kind: ExecutionSubjectKind::Agent,
                    subject_id: "agent".into(),
                    client_id: "cli-b".into(),
                    session_id: None,
                    model_id: None,
                    scope: AuthorizationScope::Workstream {
                        project_id: PROJECT.into(),
                        workstream_id: Id::from(2).to_string(),
                    },
                    actions: [AuthorizedAction::Inspect].into_iter().collect(),
                    expires_at_ms: None,
                    status: AuthorizationStatus::Active,
                    revoked_at_ms: None,
                    revoked_by: None,
                    verifiable_capabilities: vec![],
                    self_reported_skill_hints: vec![],
                    parent_authorization_id: None,
                    maintainer_person_id: None,
                    created_at_ms: 1000,
                    binding_id: Some("bind-agent".into()),
                },
            },
        )
        .await
        .unwrap();
    let export=Box::pin(integration_fixture::run(&f.reads,integration_fixture::SUPERVISOR,"dependency-export","delivery.export.publish",
        json!({"session_id":"session-supervisor","expected_session_version":"1","consumer_work_id":"b-private",
            "expected_consumer_contract_hash":hash,"expected_consumer_ownership_version":"1",
            "receipt_id":receipt["receipt_id"],"expected_artifact_sha256":f.evidence["artifact_digest"]}))).await;
    // The read fixture selects the exact ordinary-command disclosure. Adoption
    // command authorization and CAS have their own existing mechanism tests.
    f.admin.execute("INSERT INTO awr_team.workstream_artifact_adoptions(tenant_id,project_id,id,consumer_work_id,
        provider_work_id,export_id,export_version,disclosure_sha256,consumer_contract_hash,consumer_ownership_version,
        version,adopted_by_actor_id,adopted_by_client_id) VALUES($1,$2,'observed-adoption','b-private','a',$3,1,$4,$5,1,1,'agent','cli-b')",
        &[&TENANT,&PROJECT,&export["export_id"].as_str().unwrap(),&export["disclosure_sha256"].as_str().unwrap(),&hash]).await.unwrap();
    (f, export)
}

#[tokio::test]
async fn dependency_fixed_adoption_preserves_original_disclosure_and_does_not_require_upstream_source_access()
 {
    let (f, _) = Box::pin(cross_dependency_fixture(
        awr_core::DeliveryVersionPolicy::FixedDelivery,
    ))
    .await;
    let mut q = query("dependencies.list");
    q.work_id = Some("b-private".into());
    let original = f.reads.query(TENANT, PROJECT, B, q.clone()).await.unwrap();
    let item = &original["data"]["items"][0];
    assert_eq!(item["validity"], "valid");
    assert_eq!(item["adoption_valid"], true);
    assert_eq!(item["version_policy"], "fixed_delivery");
    assert!(item["provider_title"].is_null());
    assert!(item["consumed_version"].is_null());
    assert!(!item["adopted_version"]["source_snapshot_id"].is_null());
    f.admin.batch_execute("UPDATE awr_team.work_runtime SET state='in_progress',selected_completion_id=NULL WHERE work_id='a'").await.unwrap();
    let historical = f.reads.query(TENANT, PROJECT, B, q).await.unwrap();
    assert_eq!(historical["data"]["items"], original["data"]["items"]);
    let mut inspect = query("delivery.inspect");
    inspect.work_id = Some("b-private".into());
    let result = f.reads.query(TENANT, PROJECT, B, inspect).await.unwrap();
    let mut expected = item.clone();
    expected
        .as_object_mut()
        .unwrap()
        .remove("dependency_position");
    expected.as_object_mut().unwrap().remove("work_title");
    assert_eq!(
        result["data"]["delivery"]["dependencies"],
        json!([expected])
    );
}

#[tokio::test]
async fn dependency_current_adoption_revalidates_exports_and_clears_revoked_metadata() {
    let (f, export) = Box::pin(cross_dependency_fixture(
        awr_core::DeliveryVersionPolicy::CurrentContract,
    ))
    .await;
    let mut q = query("dependencies.list");
    q.work_id = Some("b-private".into());
    let original = f.reads.query(TENANT, PROJECT, B, q.clone()).await.unwrap();
    assert_eq!(original["data"]["items"][0]["validity"], "valid");
    Box::pin(integration_fixture::run(&f.reads,integration_fixture::SUPERVISOR,"dependency-revoke","delivery.export.revoke",
        json!({"session_id":"session-supervisor","expected_session_version":"1","export_id":export["export_id"],
            "expected_export_version":export["export_version"]}))).await;
    let revoked = f.reads.query(TENANT, PROJECT, B, q).await.unwrap();
    let item = &revoked["data"]["items"][0];
    assert_eq!(item["validity"], "unavailable");
    assert_eq!(item["adoption_valid"], false);
    for field in [
        "required_version",
        "available_version",
        "adopted_version",
        "consumed_version",
    ] {
        assert!(item[field].is_null());
    }
    assert_eq!(item["reason"], "adoption_unavailable");
}
