async fn provenance_round(
    admin: &Client,
    store: &WorkstreamReadStore,
    execution: Option<&Value>,
    key: &str,
) -> Value {
    seed_review_actors(admin).await;
    let mut args = json!({"session_id":"session-a","expected_session_version":"1",
        "payload":{"passed":true,"output_digest":RESULT},
        "artifact_text":"review provenance artifact","input_digest":INPUT,"dirty_tree":false});
    if let Some(execution) = execution {
        args["execution_id"] = execution["execution_id"].clone();
    }
    let evidence = run(store, A, &format!("{key}-evidence"), "evidence.submit", args).await;
    run(
        store,
        A,
        &format!("{key}-open"),
        "review.open",
        json!({"session_id":"session-a","expected_session_version":"1",
            "evidence_id":evidence["evidence_id"]}),
    )
    .await
}

async fn inspect_provenance(
    store: &WorkstreamReadStore,
    token: &str,
    round: &Value,
) -> Result<Value, PgError> {
    let mut q = query("review.inspect");
    q.work_id = Some("a".into());
    q.review_round_id = Some(round["round_id"].as_str().unwrap().into());
    store
        .query(TENANT, PROJECT, token, q)
        .await
        .map(|value| value["data"]["review"]["evidence_provenance"].clone())
}

fn assert_unverifiable_without_execution_facts(provenance: &Value, reason: &str) {
    let consistency = &provenance["execution_consistency"];
    assert_eq!(consistency["status"], "unverifiable");
    assert_eq!(consistency["reason"], reason);
    assert!(consistency["execution_success"].is_null());
    assert!(consistency["recorded_state"].is_null());
    assert!(consistency["fact_source"].is_null());
    assert_eq!(consistency["independent_verification"], false);
}

#[tokio::test]
async fn review_inspect_exposes_exact_caller_provenance_and_accessible_scoped_queries() {
    let (_guard, admin, db, store) = setup().await;
    let chain = caller_chain(&admin, &db, &store).await;
    let round = json!({"round_id":chain["round_id"]});
    let provenance = inspect_provenance(&store, REVIEWER_TOKEN, &round)
        .await
        .unwrap();
    assert_eq!(provenance["binding_status"], "exact");
    assert_eq!(provenance["trust_basis"], "caller_asserted");
    assert_eq!(provenance["author_self_report"], true);
    assert_eq!(provenance["review_contract_matches_current"], true);
    assert_eq!(provenance["currentness"], "current_contract");
    assert_eq!(provenance["evidence_inspection"]["available"], true);
    assert_eq!(provenance["execution_inspection"]["available"], true);
    assert_eq!(provenance["execution_consistency"]["status"], "consistent");
    assert_eq!(provenance["execution_consistency"]["claimed_success"], true);
    assert_eq!(
        provenance["execution_consistency"]["execution_success"],
        true
    );
    assert_eq!(
        provenance["execution_consistency"]["independent_verification"],
        false
    );
    let boundary = provenance["boundary"].as_str().unwrap();
    assert!(boundary.contains("caller_asserted is not independent verification"));
    assert!(boundary.contains("does not execute or verify commands"));

    let evidence_query = serde_json::from_value(
        provenance["evidence_inspection"]["query"].clone(),
    )
    .unwrap();
    let evidence = store
        .query(TENANT, PROJECT, REVIEWER_TOKEN, evidence_query)
        .await
        .unwrap();
    assert_eq!(evidence["data"]["evidence"]["trust_basis"], "caller_asserted");
    assert_eq!(
        evidence["data"]["evidence"]["execution_consistency"],
        provenance["execution_consistency"]
    );
    let execution_query = serde_json::from_value(
        provenance["execution_inspection"]["query"].clone(),
    )
    .unwrap();
    let inspected = store
        .query(TENANT, PROJECT, REVIEWER_TOKEN, execution_query)
        .await
        .unwrap();
    assert_eq!(inspected["data"]["execution_id"], chain["execution_id"]);
    assert_eq!(inspected["data"]["state"], "succeeded");
    assert_eq!(inspected["data"]["receipt_details_available"], false);
    assert!(inspected["data"]["latest_receipt"].is_null());
    assert!(serde_json::to_vec(&provenance).unwrap().len() < 4096);
}

#[tokio::test]
async fn review_inspect_handles_missing_evidence_and_execution_without_granting_access() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let round = provenance_round(&admin, &store, None, "provenance-missing").await;
    let provenance = inspect_provenance(&store, REVIEWER_TOKEN, &round)
        .await
        .unwrap();
    assert_eq!(provenance["binding_status"], "exact");
    assert_eq!(
        provenance["execution_inspection"]["reason"],
        "no_bound_execution"
    );
    assert!(provenance["execution_inspection"]["query"].is_null());

    admin
        .execute(
            "UPDATE awr_team.review_rounds
             SET evidence_id=NULL, execution_id=NULL, artifact_digest=NULL,
                 execution_result_digest=NULL WHERE id=$1",
            &[&round["round_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    let missing = inspect_provenance(&store, REVIEWER_TOKEN, &round)
        .await
        .unwrap();
    assert_eq!(missing["binding_status"], "not_bound");
    assert_eq!(missing["evidence_inspection"]["reason"], "no_bound_evidence");
    assert!(missing["trust_basis"].is_null());
    assert!(missing["evidence_inspection"]["query"].is_null());
    assert!(matches!(
        inspect_provenance(&store, NONE, &round).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn review_inspect_does_not_disclose_cross_work_evidence_provenance() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let round = provenance_round(&admin, &store, None, "provenance-scope").await;
    let private_contract: String = admin
        .query_one(
            "SELECT contract_hash FROM awr_team.work_contracts
             WHERE tenant_id=$1 AND project_id=$2 AND work_id='b-private'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    admin
        .execute(
            "INSERT INTO awr_team.evidence(
                tenant_id,project_id,id,work_id,contract_hash,evidence_kind,
                trust_basis,digest,payload_json,created_by)
             VALUES($1,$2,'private-evidence','b-private',$3,'report',
                'trusted_executor',$4,$5,'agent')",
            &[
                &TENANT,
                &PROJECT,
                &private_contract,
                &"f".repeat(64),
                &json!({"private":"scope-sentinel"}),
            ],
        )
        .await
        .unwrap();
    admin
        .execute(
            "UPDATE awr_team.review_rounds SET evidence_id='private-evidence' WHERE id=$1",
            &[&round["round_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    let provenance = inspect_provenance(&store, REVIEWER_TOKEN, &round)
        .await
        .unwrap();
    assert_eq!(provenance["binding_status"], "not_inspectable");
    assert_eq!(
        provenance["evidence_inspection"]["reason"],
        "not_in_review_work_scope"
    );
    assert!(provenance["trust_basis"].is_null());
    assert!(provenance["evidence_inspection"]["query"].is_null());
    assert!(!provenance.to_string().contains("trusted_executor"));
    assert!(!provenance.to_string().contains("scope-sentinel"));
}

#[tokio::test]
async fn review_inspect_marks_stale_contract_and_withholds_stale_execution_query() {
    let (_guard, admin, db, store) = setup().await;
    let chain = caller_chain(&admin, &db, &store).await;
    let round = json!({"round_id":chain["round_id"]});
    let mut contract = current_contract(&admin).await;
    contract.acceptance.push("new current acceptance".into());
    put_contract(&admin, &contract).await;
    let provenance = inspect_provenance(&store, REVIEWER_TOKEN, &round)
        .await
        .unwrap();
    assert_eq!(provenance["binding_status"], "exact");
    assert_eq!(provenance["trust_basis"], "caller_asserted");
    assert_eq!(provenance["review_contract_matches_current"], false);
    assert_eq!(provenance["currentness"], "stale_contract");
    assert_eq!(
        provenance["execution_inspection"]["reason"],
        "stale_review_contract"
    );
    assert!(provenance["execution_inspection"]["query"].is_null());
    assert_unverifiable_without_execution_facts(&provenance, "stale_review_contract");
    let mut execution_query = query("execution.inspect");
    execution_query.work_id = Some("a".into());
    execution_query.execution_id = Some(chain["execution_id"].as_str().unwrap().into());
    let direct = store
        .query(TENANT, PROJECT, REVIEWER_TOKEN, execution_query)
        .await
        .unwrap();
    assert_eq!(direct["data"]["contract_matches_current"], false);
}

#[tokio::test]
async fn review_inspect_withholds_execution_facts_after_ownership_changes() {
    let (_guard, admin, db, store) = setup().await;
    let chain = caller_chain(&admin, &db, &store).await;
    let round = json!({"round_id":chain["round_id"]});
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_ownership SET ownership_version=2 WHERE work_id='a';
             UPDATE awr_team.workstream_snapshot_ownership
             SET ownership_version=2 WHERE work_id='a'",
        )
        .await
        .unwrap();

    let provenance = inspect_provenance(&store, REVIEWER_TOKEN, &round)
        .await
        .unwrap();
    assert_eq!(provenance["binding_status"], "exact");
    assert_eq!(provenance["currentness"], "current_contract");
    assert_eq!(
        provenance["execution_inspection"]["reason"],
        "not_inspectable_in_current_work_binding"
    );
    assert!(provenance["execution_inspection"]["query"].is_null());
    assert_unverifiable_without_execution_facts(&provenance, "execution_not_inspectable");

    let mut evidence_query = query("evidence.inspect");
    evidence_query.work_id = Some("a".into());
    evidence_query.evidence_id = Some(chain["evidence_id"].as_str().unwrap().into());
    let evidence = store
        .query(TENANT, PROJECT, REVIEWER_TOKEN, evidence_query)
        .await
        .unwrap();
    let consistency = &evidence["data"]["evidence"]["execution_consistency"];
    assert_eq!(consistency["status"], "unverifiable");
    assert_eq!(consistency["reason"], "execution_not_inspectable");
    assert!(consistency["execution_success"].is_null());
    assert!(consistency["recorded_state"].is_null());
    assert!(consistency["fact_source"].is_null());
}

#[tokio::test]
async fn review_inspect_does_not_trust_tampered_caller_payload() {
    let (_guard, admin, db, store) = setup().await;
    let chain = caller_chain(&admin, &db, &store).await;
    admin
        .execute(
            "UPDATE awr_team.evidence SET payload_json=$1 WHERE id=$2",
            &[
                &json!({"passed":false,"output_digest":RESULT}),
                &chain["evidence_id"].as_str().unwrap(),
            ],
        )
        .await
        .unwrap();
    let round = json!({"round_id":chain["round_id"]});

    let provenance = inspect_provenance(&store, REVIEWER_TOKEN, &round)
        .await
        .unwrap();
    assert_eq!(provenance["binding_status"], "exact");
    assert_eq!(provenance["currentness"], "current_contract");
    assert_eq!(
        provenance["execution_inspection"]["reason"],
        "evidence_integrity_invalid"
    );
    assert!(provenance["execution_inspection"]["query"].is_null());
    assert_unverifiable_without_execution_facts(&provenance, "evidence_digest_mismatch");
    assert!(provenance["execution_consistency"]["claimed_success"].is_null());
}

#[tokio::test]
async fn review_inspect_does_not_leak_cross_work_or_cross_scope_execution_facts() {
    let (_guard, admin, db, store) = setup().await;
    let chain = caller_chain(&admin, &db, &store).await;
    let source_execution = chain["execution_id"].as_str().unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.executions(
                tenant_id,project_id,id,work_id,session_id,claim_id,fence,
                contract_hash,input_digest,executor_actor_id,state,result_digest,
                scope_id,workstream_id,ownership_version,executor_client_id,
                coordinator_epoch)
             SELECT tenant_id,project_id,'cross-work-execution','b-private',
                session_id,claim_id,fence,contract_hash,input_digest,
                executor_actor_id,state,result_digest,'main',workstream_id,
                ownership_version,executor_client_id,coordinator_epoch
             FROM awr_team.executions WHERE id=$1",
            &[&source_execution],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.executions(
                tenant_id,project_id,id,work_id,session_id,claim_id,fence,
                contract_hash,input_digest,executor_actor_id,state,result_digest,
                scope_id,workstream_id,ownership_version,executor_client_id,
                coordinator_epoch)
             SELECT tenant_id,project_id,'cross-scope-execution','a',
                session_id,claim_id,fence,contract_hash,input_digest,
                executor_actor_id,state,result_digest,'private',workstream_id,
                ownership_version,executor_client_id,coordinator_epoch
             FROM awr_team.executions WHERE id=$1",
            &[&source_execution],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.executions(
                tenant_id,project_id,id,work_id,session_id,claim_id,fence,
                contract_hash,input_digest,executor_actor_id,state,result_digest,
                scope_id,workstream_id,ownership_version,executor_client_id,
                coordinator_epoch)
             SELECT tenant_id,project_id,'external-binding-execution','a',
                'session-runner',claim_id,fence,contract_hash,input_digest,
                executor_actor_id,state,result_digest,'main','external-workstream',
                ownership_version,executor_client_id,coordinator_epoch
             FROM awr_team.executions WHERE id=$1",
            &[&source_execution],
        )
        .await
        .unwrap();

    for execution_id in [
        "cross-work-execution",
        "cross-scope-execution",
        "external-binding-execution",
    ] {
        admin
            .execute(
                "UPDATE awr_team.evidence SET execution_id=$1 WHERE id=$2",
                &[&execution_id, &chain["evidence_id"].as_str().unwrap()],
            )
            .await
            .unwrap();
        admin
            .execute(
                "UPDATE awr_team.review_rounds SET execution_id=$1 WHERE id=$2",
                &[&execution_id, &chain["round_id"].as_str().unwrap()],
            )
            .await
            .unwrap();
        let round = json!({"round_id":chain["round_id"]});
        let provenance = inspect_provenance(&store, REVIEWER_TOKEN, &round)
            .await
            .unwrap();
        assert_eq!(provenance["binding_status"], "exact", "{execution_id}");
        assert_eq!(
            provenance["execution_inspection"]["reason"],
            "not_inspectable_in_current_work_binding",
            "{execution_id}"
        );
        assert_unverifiable_without_execution_facts(&provenance, "execution_not_inspectable");
        assert!(
            !provenance["execution_consistency"]
                .to_string()
                .contains("succeeded"),
            "{execution_id}"
        );
        let mut evidence_query = query("evidence.inspect");
        evidence_query.work_id = Some("a".into());
        evidence_query.evidence_id = Some(chain["evidence_id"].as_str().unwrap().into());
        let evidence = store
            .query(TENANT, PROJECT, REVIEWER_TOKEN, evidence_query)
            .await
            .unwrap();
        let consistency = &evidence["data"]["evidence"]["execution_consistency"];
        assert_eq!(consistency["status"], "unverifiable", "{execution_id}");
        assert_eq!(
            consistency["reason"], "execution_not_inspectable",
            "{execution_id}"
        );
        assert!(consistency["execution_success"].is_null(), "{execution_id}");
        assert!(consistency["recorded_state"].is_null(), "{execution_id}");
        assert!(consistency["fact_source"].is_null(), "{execution_id}");
    }
}
