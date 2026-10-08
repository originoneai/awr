// Synthetic authenticated member workflows; not native business acceptance.
async fn complete_simulated_upstream(store: &WorkstreamReadStore) -> Value {
    let evidence = simulated_evidence(store).await;
    finalize_simulated_input(store, &evidence).await
}

async fn finalize_simulated_input(store: &WorkstreamReadStore, evidence: &Value) -> Value {
    let round = open_simulated(store, evidence, RUNNER, "session-runner", "dependency-open").await;
    run(
        store,
        REVIEWER_TOKEN,
        "dependency-review",
        "review.decide",
        args(&round),
    )
    .await;
    run(
        store,
        RUNNER,
        "dependency-finalize",
        "delivery.finalize",
        completion_args(evidence),
    )
    .await
}

#[tokio::test]
async fn simulated_dependency_keeps_authorized_reconciliation_as_its_actual_execution_basis() {
    use sha2::Digest;
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let chain = simulated_intent(&store).await;
    let started = run(
        &store,
        A,
        "reconciled-input-start",
        "execution.start",
        start_args(&store, &chain).await,
    )
    .await;
    let output = hex_encode(&sha2::Sha256::digest(b"reconciled artifact"));
    let reported = run(&store, A, "reconciled-input-report", "execution.report", json!({
        "session_id":"session-a","expected_session_version":"1","execution_id":chain["intent"]["execution_id"],
        "expected_execution_version":started["execution_version"],"outcome":"succeeded",
        "output_digest":output,"observed_paths":["src/api/result.json"],"note":"Observed caller effects without a settlement assertion",
    })).await;
    let p = prepare(&store, RUNNER, "a").await;
    run(&store, RUNNER, "reconciled-input-facts", "execution.reconcile", json!({
        "session_id":"session-runner","expected_session_version":"1","execution_id":chain["intent"]["execution_id"],
        "expected_execution_version":reported["execution_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "reviewed_receipt_id":reported["receipt_id"],"clear_recovery_block":true,
        "facts":{"outcome":"succeeded","input_digest":INPUT,"output_digest":output,"environment_digest":"c".repeat(64),
            "observed_paths":["src/api/result.json"],"note":"Operator reconciled the synthetic caller result"},
    })).await;
    let evidence = run(&store, A, "reconciled-input-evidence", "evidence.submit", json!({
        "session_id":"session-a","expected_session_version":"1","execution_id":chain["intent"]["execution_id"],
        "input_digest":INPUT,"dirty_tree":false,"artifact_text":"reconciled artifact","payload":{"passed":true,"output_digest":output},
    })).await;
    let upstream = finalize_simulated_input(&store, &evidence).await;
    assert_eq!(upstream["execution_basis"], "caller_asserted_reconciled");
    assert_eq!(upstream["human_approval"], false);
    let mut consumer = dependency_consumer(&admin).await;
    consumer.codec = WorkContract::CODEC_V5.into();
    consumer.required_dependencies = vec!["a".into()];
    consumer.dependency_acceptance.insert(
        "a".into(),
        awr_team::DependencyAcceptanceMode::SimulatedMemberIndependent,
    );
    set_dependency_consumer(&admin, &consumer).await;
    assert_eq!(consumer.completion_policy, "review");
    assert_ne!(
        next_consumer(&store).await["navigation"],
        "waiting_dependency"
    );
    let session = consumer_session(&store, A, "reconciled-consumer-session").await;
    assert!(
        take_consumer(&store, &session, "reconciled-consumer-claim")
            .await
            .is_ok()
    );
}

async fn set_dependency_consumer(admin: &Client, consumer: &WorkContract) {
    admin.execute(
        "UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='c'",
        &[&json!(consumer), &consumer.hash().unwrap()],
    ).await.unwrap();
}

async fn dependency_consumer(admin: &Client) -> WorkContract {
    serde_json::from_value(
        admin
            .query_one(
                "SELECT contract_json FROM awr_team.work_contracts WHERE work_id='c'",
                &[],
            )
            .await
            .unwrap()
            .get(0),
    )
    .unwrap()
}

async fn consumer_session(store: &WorkstreamReadStore, token: &str, key: &str) -> String {
    on_work(
        store,
        token,
        "c",
        key,
        "session.start",
        json!({"conversation_id":key}),
    )
    .await["session_id"]
        .as_str()
        .unwrap()
        .into()
}

async fn take_consumer(
    store: &WorkstreamReadStore,
    session: &str,
    key: &str,
) -> Result<Value, PgError> {
    let p = prepare(store, A, "c").await;
    store.commands().execute(TENANT, PROJECT, A, command(&p, key, "task.claim_available", json!({
        "session_id":session,"expected_session_version":"1",
        "expected_responsibility_version":p["data"]["responsibility"]["version"],
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
        "ttl_seconds":600,
    }))).await.map(|v| v["receipt"]["data"].clone())
}

async fn next_consumer(store: &WorkstreamReadStore) -> Value {
    let next = store
        .query(TENANT, PROJECT, A, query("work.next"))
        .await
        .unwrap();
    next["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["work_id"] == "c")
        .unwrap()
        .clone()
}

#[tokio::test]
async fn unmapped_simulated_receipt_does_not_unlock_ordinary_member_navigation() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let receipt = complete_simulated_upstream(&store).await;
    assert_eq!(receipt["independence_kind"], "simulated_member_independent");
    let mut consumer = dependency_consumer(&admin).await;
    consumer.required_dependencies = vec!["a".into()];
    assert!(consumer.dependency_acceptance.is_empty());
    set_dependency_consumer(&admin, &consumer).await;
    let candidate = next_consumer(&store).await;
    assert_eq!(
        candidate["navigation"], "waiting_dependency",
        "An unmapped predecessor must not implicitly accept simulated-member review: {candidate}"
    );
    let session = consumer_session(&store, A, "unmapped-session").await;
    assert!(matches!(
        take_consumer(&store, &session, "unmapped-claim").await,
        Err(PgError::MissingDependency)
    ));
}

#[tokio::test]
async fn explicit_simulated_dependency_drives_normal_intake_execution_and_completion() {
    use sha2::Digest;
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let mut consumer = dependency_consumer(&admin).await;
    consumer.codec = WorkContract::CODEC_V5.into();
    consumer.completion_policy = SIMULATED_POLICY.into();
    consumer.scope_paths = vec!["src/integration".into()];
    consumer.verification_requirements = vec!["Verify the integrated artifact".into()];
    consumer.execution_settlement = Some(awr_team::ExecutionSettlementPolicy {
        mode: awr_team::ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "consumer-workspace".into(),
    });
    consumer.required_dependencies = vec!["a".into()];
    consumer.dependency_acceptance.insert(
        "a".into(),
        awr_team::DependencyAcceptanceMode::SimulatedMemberIndependent,
    );
    set_dependency_consumer(&admin, &consumer).await;
    assert_ne!(
        next_consumer(&store).await["navigation"],
        "waiting_dependency"
    );
    let session = consumer_session(&store, A, "consumer-session").await;
    let reviewer_session = consumer_session(&store, REVIEWER_TOKEN, "consumer-reviewer").await;
    let finalizer_session = consumer_session(&store, RUNNER, "consumer-finalizer").await;
    let claim = take_consumer(&store, &session, "consumer-claim")
        .await
        .unwrap();
    assert_eq!(claim["coordination_claim_acquired"], true);
    assert_eq!(claim["responsibility"]["owner"], "person-author");
    let p = prepare(&store, A, "c").await;
    let intent = on_work(
        &store,
        A,
        "c",
        "consumer-intent",
        "execution.prepare",
        json!({
            "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":INPUT,
            "declared_scope":["src/integration"],
        }),
    )
    .await;
    let p = prepare(&store, A, "c").await;
    let started = on_work(&store, A, "c", "consumer-start", "execution.start", json!({
        "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "execution_id":intent["execution_id"],"expected_execution_version":intent["execution_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed",
    })).await;
    let output = hex_encode(&sha2::Sha256::digest(b"integrated artifact"));
    on_work(&store, A, "c", "consumer-report", "execution.report", json!({
        "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],
        "expected_execution_version":started["execution_version"],"outcome":"succeeded",
        "output_digest":output,"observed_paths":["src/integration/result.json"],"note":"Observed the integrated fixture",
        "workspace_settlement":{"workspace_id":"consumer-workspace","input_digest":INPUT,
            "environment_digest":"c".repeat(64),"claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "executor_stopped":true,"no_external_effects":true},
    })).await;
    let evidence = on_work(&store, A, "c", "consumer-evidence", "evidence.submit", json!({
        "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],
        "input_digest":INPUT,"dirty_tree":false,"artifact_text":"integrated artifact",
        "payload":{"passed":true,"output_digest":output},
    })).await;
    assert_eq!(evidence["trust_basis"], "caller_asserted");
    let round = on_work(&store, A, "c", "consumer-open", "review.open", json!({
        "session_id":session,"expected_session_version":"1","evidence_id":evidence["evidence_id"],
    })).await;
    let mut decision = args(&round);
    decision["session_id"] = json!(reviewer_session);
    on_work(
        &store,
        REVIEWER_TOKEN,
        "c",
        "consumer-decision",
        "review.decide",
        decision,
    )
    .await;
    let mut complete = completion_args(&evidence);
    complete["session_id"] = json!(finalizer_session);
    let request = command(
        &prepare(&store, RUNNER, "c").await,
        "consumer-stale-complete",
        "work.complete",
        complete.clone(),
    );
    admin.batch_execute("UPDATE awr_team.work_runtime SET state='unclaimed',selected_completion_id=NULL WHERE work_id='a'").await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, RUNNER, request)
            .await,
        Err(PgError::CompletionRejected)
    ));
    admin.execute("UPDATE awr_team.work_runtime SET state='completed',selected_completion_id=$1 WHERE work_id='a'",
        &[&upstream["receipt_id"].as_str().unwrap()]).await.unwrap();
    let finalized = on_work(
        &store,
        RUNNER,
        "c",
        "consumer-complete",
        "work.complete",
        complete,
    )
    .await;
    assert_eq!(finalized["human_approval"], false);
    assert_eq!(finalized["team_independent_acceptance"], false);
    let links = admin.query("SELECT predecessor_work_id,predecessor_completion_id FROM awr_team.completion_dependencies WHERE completion_id=$1",
        &[&finalized["receipt_id"].as_str().unwrap()]).await.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].get::<_, String>(0), "a");
    assert_eq!(
        links[0].get::<_, String>(1),
        upstream["receipt_id"].as_str().unwrap()
    );
}

#[tokio::test]
async fn simulated_dependency_rechecks_current_contract_selection_and_original_review() {
    for changed in [
        "agent_mode",
        "deselected",
        "source_contract",
        "missing_decision",
        "invalidated_round",
    ] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let upstream = complete_simulated_upstream(&store).await;
        let mut consumer = dependency_consumer(&admin).await;
        consumer.codec = WorkContract::CODEC_V5.into();
        consumer.required_dependencies = vec!["a".into()];
        consumer.dependency_acceptance.insert(
            "a".into(),
            awr_team::DependencyAcceptanceMode::SimulatedMemberIndependent,
        );
        set_dependency_consumer(&admin, &consumer).await;
        assert_ne!(
            next_consumer(&store).await["navigation"],
            "waiting_dependency"
        );
        match changed {
            "agent_mode" => {
                consumer.codec = WorkContract::CODEC_V2.into();
                consumer.dependency_acceptance.insert(
                    "a".into(),
                    awr_team::DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
                );
                set_dependency_consumer(&admin, &consumer).await;
            }
            "deselected" => {
                admin.batch_execute("UPDATE awr_team.work_runtime SET state='unclaimed',selected_completion_id=NULL WHERE work_id='a'").await.unwrap();
            }
            "source_contract" => {
                let mut contract = current_contract(&admin).await;
                contract
                    .acceptance
                    .push("Reverify changed API shape".into());
                put_contract(&admin, &contract).await;
            }
            "missing_decision" => {
                // Synthetic fault: remove the original referenced decision, never invent one.
                admin.execute("DELETE FROM awr_team.review_decisions WHERE id=(SELECT approved_by_json->>'review_decision_id' FROM awr_team.completion_receipts WHERE id=$1)",
                    &[&upstream["receipt_id"].as_str().unwrap()]).await.unwrap();
            }
            _ => {
                admin.execute("UPDATE awr_team.review_rounds SET state='invalidated' WHERE id=(SELECT approved_by_json->>'review_round_id' FROM awr_team.completion_receipts WHERE id=$1)",
                    &[&upstream["receipt_id"].as_str().unwrap()]).await.unwrap();
            }
        }
        assert_eq!(
            next_consumer(&store).await["navigation"],
            "waiting_dependency",
            "{changed}"
        );
        let session = consumer_session(&store, A, "changed-session").await;
        assert!(
            matches!(
                take_consumer(&store, &session, "changed-claim").await,
                Err(PgError::MissingDependency)
            ),
            "{changed}"
        );
    }
}
