// Shares the established caller/evidence/independent Agent-review fixture.
#[tokio::test]
async fn v2_live_and_expired_reports_keep_artifact_and_independent_review_gates() {
    for late in [false, true] {
        let (_g, admin, db, store) = setup().await;
        let chain = caller_chain_with_policy(&admin, &db, &store, Some("clone-author"),
            awr_team::ExecutionSettlementMode::IndependentWorkspaceV2, late).await;
        let complete = run(&store, RUNNER, "v2-complete", "work.complete", completion_args(&chain)).await;
        assert_eq!(complete["task_complete"], true);
        assert_eq!(complete["caller_execution_binding"]["settlement_mode"], "independent_workspace_v2");
        assert_eq!(complete["approval_basis"], "agent_review");
        assert_eq!(complete["human_approval"], false);
        assert_eq!(complete["team_independent_acceptance"], false);
        let reconciliations: i64 = admin.query_one("SELECT count(*) FROM awr_team.execution_receipts WHERE receipt_kind<>'caller_asserted'", &[]).await.unwrap().get(0);
        assert_eq!(reconciliations, 0);
    }
}

#[tokio::test]
async fn v2_completion_refuses_missing_inconsistent_or_malformed_lease_observations() {
    for mutation in ["missing", "null", "basis", "time", "extra"] {
        let (_g, admin, db, store) = setup().await;
        let chain = caller_chain_with_policy(&admin, &db, &store, Some("clone-author"),
            awr_team::ExecutionSettlementMode::IndependentWorkspaceV2, true).await;
        let id = chain["caller_receipt_id"].as_str().unwrap();
        let mut payload: Value = admin.query_one("SELECT payload_json FROM awr_team.execution_receipts WHERE id=$1", &[&id]).await.unwrap().get(0);
        match mutation {
            "missing" => { payload.as_object_mut().unwrap().remove("lease_observation"); }
            "null" => { payload["lease_observation"] = Value::Null; }
            "basis" => { payload["lease_observation"]["basis"] = json!("live"); }
            "time" => { payload["lease_observation"]["observed_at"] = json!("invalid time"); }
            _ => { payload["lease_observation"]["trusted_executor"] = json!(true); }
        }
        let digest = awr_team::request_hash(&payload).unwrap();
        admin.execute("UPDATE awr_team.execution_receipts SET payload_json=$1,digest=$2 WHERE id=$3", &[&payload,&digest,&id]).await.unwrap();
        assert!(matches!(run_err(&store, RUNNER, mutation, "work.complete", completion_args(&chain)).await, PgError::EvidenceInvalid), "{mutation}");
        let count: i64 = admin.query_one("SELECT count(*) FROM awr_team.completion_receipts", &[]).await.unwrap().get(0);
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn workspace_completion_verifies_artifact_without_reconciliation_or_trust_upgrade() {
    let (_g, admin, db, store) = setup().await;
    let chain = caller_chain_in_workspace(&admin, &db, &store, Some("clone-author")).await;
    let mut observation = query("execution.inspect");
    observation.work_id = Some("a".into());
    observation.execution_id = Some(chain["execution_id"].as_str().unwrap().into());
    let before = store.query(TENANT, PROJECT, A, observation.clone()).await.unwrap();
    assert_eq!(before["data"]["terminal_reported"], true);
    assert_eq!(before["data"]["effects_settled"], true);
    assert_eq!(before["data"]["artifact_verified"], false);
    assert_eq!(before["data"]["settlement_basis"], "caller_asserted");
    // Review may outlive the old execution lease once effects are settled.
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let complete = run(
        &store,
        RUNNER,
        "workspace-complete",
        "work.complete",
        completion_args(&chain),
    )
    .await;
    assert_eq!(complete["task_complete"], true);
    assert_eq!(
        complete["execution_basis"],
        "caller_asserted_workspace_settled"
    );
    assert_eq!(complete["approval_basis"], "agent_review");
    assert_eq!(complete["human_approval"], false);
    assert_eq!(complete["team_independent_acceptance"], false);
    assert_eq!(complete["author_self_report"], true);
    let observed = store.query(TENANT, PROJECT, A, observation).await.unwrap();
    assert_eq!(observed["data"]["artifact_verified"], true);
    assert_eq!(observed["data"]["artifact_verification_basis"], "current_completion_readable_artifact_digest");
    assert_eq!(observed["data"]["settlement_scope"], "admitted_workspace_paths");
    assert_eq!(observed["data"]["settlement_basis"], "caller_asserted");
    let binding = &complete["caller_execution_binding"];
    assert_eq!(binding["caller_receipt_id"], chain["caller_receipt_id"]);
    assert_eq!(binding["workspace_id"], "clone-author");
    for flag in ["terminal_reported", "artifact_verified", "effects_settled"] {
        assert_eq!(binding[flag], true);
    }
    let row = admin
        .query_one(
            "SELECT trust_basis FROM awr_team.evidence WHERE id=$1",
            &[&chain["evidence_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "caller_asserted");
    let recovery: i64 = admin.query_one("SELECT count(*) FROM awr_team.execution_receipts WHERE receipt_kind<>'caller_asserted'", &[]).await.unwrap().get(0);
    assert_eq!(recovery, 0);
}

#[tokio::test]
async fn workspace_artifact_observation_rechecks_bytes_and_current_completion_binding() {
    for mutation in ["content", "missing", "state", "evidence", "completion", "selection", "contract"] {
        let (_g, admin, db, store) = setup().await;
        let chain = caller_chain_in_workspace(&admin, &db, &store, Some("clone-author")).await;
        run(&store, RUNNER, "complete", "work.complete", completion_args(&chain)).await;
        match mutation {
            "content" => { admin.batch_execute("UPDATE awr_team.artifacts SET content=decode('00','hex')").await.unwrap(); }
            "missing" => { admin.batch_execute("UPDATE awr_team.artifacts SET content=NULL").await.unwrap(); }
            "state" => { admin.batch_execute("UPDATE awr_team.artifacts SET state='missing'").await.unwrap(); }
            "evidence" => { admin.batch_execute("UPDATE awr_team.evidence SET payload_json=payload_json || '{\"altered\":true}'::jsonb").await.unwrap(); }
            "completion" => { admin.batch_execute("UPDATE awr_team.completion_receipts SET evidence_bundle_hash=repeat('d',64)").await.unwrap(); }
            "selection" => { admin.batch_execute("UPDATE awr_team.work_runtime SET selected_completion_id=NULL,state='in_progress'").await.unwrap(); }
            _ => {
                let mut contract = current_contract(&admin).await;
                contract.hard_rules.push("changed acceptance constraint".into());
                put_contract(&admin, &contract).await;
            }
        }
        let mut q = query("execution.inspect");
        q.work_id = Some("a".into());
        q.execution_id = Some(chain["execution_id"].as_str().unwrap().into());
        let observed = store.query(TENANT, PROJECT, A, q).await.unwrap();
        assert_eq!(observed["data"]["artifact_verified"], false, "{mutation}");
        assert!(observed["data"]["artifact_verification_basis"].is_null(), "{mutation}");
        assert_eq!(observed["data"]["effects_settled"], true, "{mutation}");
        assert_eq!(observed["data"]["settlement_basis"], "caller_asserted", "{mutation}");
    }
}

#[tokio::test]
async fn workspace_completion_refuses_missing_corrupt_and_unrelated_artifact_bytes() {
    for mutation in ["missing", "corrupt", "unrelated"] {
        let (_g, admin, db, store) = setup().await;
        let mut chain = caller_chain_in_workspace(&admin, &db, &store, Some("clone-author")).await;
        match mutation {
            "missing" => {
                admin
                    .batch_execute("UPDATE awr_team.artifacts SET content=NULL")
                    .await
                    .unwrap();
            }
            "corrupt" => {
                admin
                    .batch_execute("UPDATE awr_team.artifacts SET content=decode('00','hex')")
                    .await
                    .unwrap();
            }
            _ => {
                // Submit valid, independently hashed but unrelated bytes via
                // the real evidence entry; the bundle must still match this run.
                let mut args =
                    submit_args("session-a", chain["execution_id"].as_str().unwrap(), "");
                args.as_object_mut().unwrap().remove("artifact_hex");
                args["artifact_text"] = json!("unrelated artifact");
                args["payload"]["output_digest"] = chain["result_digest"].clone();
                let evidence = run(&store, A, "unrelated-artifact", "evidence.submit", args).await;
                chain["evidence_id"] = evidence["evidence_id"].clone();
            }
        }
        assert!(
            matches!(
                run_err(
                    &store,
                    RUNNER,
                    mutation,
                    "work.complete",
                    completion_args(&chain)
                )
                .await,
                PgError::EvidenceInvalid
            ),
            "{mutation}"
        );
        let completed: i64 = admin
            .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(completed, 0);
    }
}

#[tokio::test]
async fn workspace_completion_binds_closed_receipt_and_released_reservation_provenance() {
    for field in [
        "execution_id",
        "workspace_id",
        "input_digest",
        "admission_lease_version",
        "artifact_verified",
        "extra_trust",
        "resource_fence",
        "outside_scope",
        "noncanonical_scope",
        "oversized_paths",
    ] {
        let (_g, admin, db, store) = setup().await;
        let chain = caller_chain_in_workspace(&admin, &db, &store, Some("clone-author")).await;
        let id = chain["caller_receipt_id"].as_str().unwrap();
        let mut payload: Value = admin
            .query_one(
                "SELECT payload_json FROM awr_team.execution_receipts WHERE id=$1",
                &[&id],
            )
            .await
            .unwrap()
            .get(0);
        match field {
            "workspace_id" => {
                payload["workspace_settlement"]["workspace_id"] = json!("other-clone")
            }
            "admission_lease_version" => payload[field] = json!("999"),
            "artifact_verified" => payload[field] = json!(true),
            "extra_trust" => payload["trusted_executor"] = json!(true),
            "resource_fence" => payload["resource_proof"][0]["fence"] = json!("999"),
            "input_digest" => payload[field] = json!("d".repeat(64)),
            "outside_scope" | "noncanonical_scope" | "oversized_paths" => {
                payload["observed_paths"] = match field {
                    "outside_scope" => json!(["src/other/result.json"]),
                    "noncanonical_scope" => json!(["src/api/../other/result.json"]),
                    _ => json!(vec!["src/api/result.json"; 129]),
                };
                // Even matching persisted facts and recomputed receipt hashes
                // cannot replace the admitted scope or closed wire bounds.
                admin
                    .execute(
                        "UPDATE awr_team.executions SET observed_paths_json=$1 WHERE id=$2",
                        &[
                            &payload["observed_paths"],
                            &chain["execution_id"].as_str().unwrap(),
                        ],
                    )
                    .await
                    .unwrap();
            }
            _ => payload[field] = json!("different-run"),
        }
        let digest = awr_team::request_hash(&payload).unwrap();
        admin
            .execute(
                "UPDATE awr_team.execution_receipts SET payload_json=$1,digest=$2 WHERE id=$3",
                &[&payload, &digest, &id],
            )
            .await
            .unwrap();
        assert!(
            matches!(
                run_err(
                    &store,
                    RUNNER,
                    field,
                    "work.complete",
                    completion_args(&chain)
                )
                .await,
                PgError::EvidenceInvalid
            ),
            "{field}"
        );
    }
}

#[tokio::test]
async fn workspace_completion_keeps_independent_review_and_current_contract_requirements() {
    for mutation in ["review", "contract", "resource"] {
        let (_g, admin, db, store) = setup().await;
        let chain = caller_chain_in_workspace(&admin, &db, &store, Some("clone-author")).await;
        match mutation {
            "review" => {
                admin
                    .batch_execute("UPDATE awr_team.review_rounds SET state='invalidated'")
                    .await
                    .unwrap();
            }
            "contract" => {
                let mut contract = current_contract(&admin).await;
                contract.hard_rules.push("new required constraint".into());
                put_contract(&admin, &contract).await;
            }
            _ => {
                admin
                    .batch_execute("UPDATE awr_team.resource_reservations SET fence=fence+1")
                    .await
                    .unwrap();
            }
        }
        let error = run_err(
            &store,
            RUNNER,
            mutation,
            "work.complete",
            completion_args(&chain),
        )
        .await;
        assert!(
            matches!(error, PgError::EvidenceInvalid | PgError::ReviewRequired),
            "{mutation}: {error:?}"
        );
    }
}
