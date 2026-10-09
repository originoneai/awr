// Included in agent_review_tests so the actual review/command fixtures are shared.
async fn caller_chain(admin: &Client, db: &str, store: &WorkstreamReadStore) -> Value {
    caller_chain_in_workspace(admin, db, store, None).await
}

async fn caller_chain_in_workspace(admin: &Client, db: &str, store: &WorkstreamReadStore, workspace: Option<&str>) -> Value {
    caller_chain_with_policy(admin, db, store, workspace, awr_team::ExecutionSettlementMode::IndependentWorkspaceV1, false).await
}

async fn caller_chain_with_policy(admin: &Client, db: &str, store: &WorkstreamReadStore, workspace: Option<&str>, mode: awr_team::ExecutionSettlementMode, late: bool) -> Value {
    use sha2::Digest;
    seed_agent_reviewer(admin, db).await;
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
        UPDATE awr_team.actors SET kind='human' WHERE id='runner';
        UPDATE awr_team.project_memberships SET role='admin' WHERE actor_id='runner';
        UPDATE awr_team.workstream_grants SET can_manage=true,can_reconcile_execution=true WHERE client_id='cli-runner';").await.unwrap();
    let original: Value = admin
        .query_one(
            "SELECT body_json FROM awr_team.agent_authorizations WHERE id='review-grant'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let mut grant: AgentAuthorization = serde_json::from_value(original).unwrap();
    grant.id = "author-work".into();
    grant.subject_id = "agent".into();
    grant.client_id = "cli-a".into();
    grant.binding_id = Some("bind-agent".into());
    grant.actions = BTreeSet::from([
        AuthorizedAction::Inspect,
        AuthorizedAction::StartWork,
        AuthorizedAction::ClaimCoordination,
    ]);
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "author-work".into(),
                authorization: grant,
            },
        )
        .await
        .unwrap();
    let mut contract = current_contract(admin).await;
    contract.completion_policy = POLICY.into();
    if let Some(workspace) = workspace {
        contract.codec = WorkContract::CODEC_V3.into();
        contract.execution_settlement = Some(awr_team::ExecutionSettlementPolicy {
            mode,
            workspace_id: workspace.into(),
        });
    }
    put_contract(admin, &contract).await;
    let result = if workspace.is_some() { hex_encode(&sha2::Sha256::digest(b"caller artifact")) } else { RESULT.into() };
    let claim = run(
        store,
        A,
        "caller-take",
        "claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1",
        "expected_work_version":"0","ttl_seconds":600}),
    )
    .await;
    let p = prepare(store, A, "a").await;
    let execution=run(store,A,"caller-intent","execution.prepare",json!({"session_id":"session-a","expected_session_version":"1",
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":INPUT,"declared_scope":["src/api"]})).await;
    let p = prepare(store, A, "a").await;
    let started=run(store,A,"caller-start","execution.start",json!({"session_id":"session-a","expected_session_version":"1",
        "execution_id":execution["execution_id"],"expected_execution_version":execution["execution_version"],
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"})).await;
    let mut report_args=json!({"session_id":"session-a","expected_session_version":"1",
        "execution_id":execution["execution_id"],"expected_execution_version":started["execution_version"],
        "outcome":"succeeded","output_digest":result,"observed_paths":["src/api/result.json"],"note":"Caller observed its own result."});
    if let Some(workspace) = workspace {
        report_args["workspace_settlement"] = json!({"workspace_id":workspace,"input_digest":INPUT,
            "environment_digest":"c".repeat(64),"claim_id":claim["claim_id"],"expected_fence":claim["fence"],
            "expected_lease_version":claim["lease_version"],"executor_stopped":true,"no_external_effects":true});
    }
    if late {
        admin.batch_execute("UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'").await.unwrap();
    }
    let reported=run(store,A,"caller-report","execution.report",report_args).await;
    assert_eq!(reported["state"], if workspace.is_some() {"succeeded"} else {"unknown"});
    assert_eq!(reported["artifact_verified"],false);
    let mut evidence_args = submit_args(
        "session-a",
        execution["execution_id"].as_str().unwrap(),
        "",
    );
    evidence_args.as_object_mut().unwrap().remove("artifact_hex");
    evidence_args["artifact_text"] = json!("caller artifact");
    evidence_args["payload"]["output_digest"] = json!(result);
    let evidence = run(
        store,
        A,
        "caller-evidence",
        "evidence.submit",
        evidence_args,
    )
    .await;
    assert_eq!(evidence["trust_basis"], "caller_asserted");
    let opened=run(store,A,"caller-open","review.open",json!({"session_id":"session-a","expected_session_version":"1","evidence_id":evidence["evidence_id"]})).await;
    run(
        store,
        REVIEWER_TOKEN,
        "caller-review",
        "review.decide",
        args(&opened),
    )
    .await;
    if workspace.is_some() {
        return json!({"evidence_id":evidence["evidence_id"],"execution_id":execution["execution_id"],
            "round_id":opened["round_id"],"caller_receipt_id":reported["receipt_id"],"result_digest":result});
    }
    assert!(matches!(
        run_err(
            store,
            RUNNER,
            "before-reconcile",
            "work.complete",
            completion_args(&evidence)
        )
        .await,
        PgError::RecoveryBlocked
    ));
    let p = prepare(store, RUNNER, "a").await;
    let reconciled=run(store,RUNNER,"caller-reconcile","execution.reconcile",json!({"session_id":"session-runner","expected_session_version":"1",
        "execution_id":execution["execution_id"],"expected_execution_version":reported["execution_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"reviewed_receipt_id":reported["receipt_id"],"clear_recovery_block":true,
        "facts":{"outcome":"succeeded","input_digest":INPUT,"output_digest":RESULT,"environment_digest":"c".repeat(64),
            "observed_paths":["src/api/result.json"],"note":"Operator reconciled actual effects; no trusted execution or human acceptance."}})).await;
    assert_eq!(reconciled["state"], "succeeded");
    json!({"evidence_id":evidence["evidence_id"],"execution_id":execution["execution_id"],
        "round_id":opened["round_id"],"caller_receipt_id":reported["receipt_id"],"reconcile_receipt_id":reconciled["receipt_id"]})
}
fn completion_args(evidence: &Value) -> Value {
    json!({"session_id":"session-runner","expected_session_version":"1","evidence_id":evidence["evidence_id"],"context_complete":true})
}

#[tokio::test]
async fn reconciled_caller_and_exact_agent_review_complete_without_upgrading_trust() {
    let (_g, admin, db, store) = setup().await;
    let chain = caller_chain(&admin, &db, &store).await;
    let p = prepare(&store, RUNNER, "a").await;
    let command = command(
        &p,
        "caller-finalize",
        "work.complete",
        completion_args(&chain),
    );
    let response = store
        .commands()
        .execute(TENANT, PROJECT, RUNNER, command.clone())
        .await
        .unwrap();
    let completed = &response["receipt"]["data"];
    assert_eq!(completed["task_complete"], true);
    assert_eq!(completed["human_approval"], false);
    assert_eq!(completed["team_independent_acceptance"], false);
    assert_eq!(completed["author_self_report"], true);
    assert_eq!(completed["execution_basis"], "caller_asserted_reconciled");
    assert_eq!(completed["approval_basis"], "agent_review");
    assert_eq!(completed["reviewer_client_id"], "cli-reviewer");
    assert_eq!(
        completed["caller_execution_binding"]["caller_receipt_id"],
        chain["caller_receipt_id"]
    );
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, RUNNER, command)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], response["receipt"]);
    let evidence = admin
        .query_one(
            "SELECT trust_basis FROM awr_team.evidence WHERE id=$1",
            &[&chain["evidence_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(evidence.get::<_, String>(0), "caller_asserted");
    let mut q = query("completion.inspect");
    q.work_id = Some("a".into());
    let inspected = store.query(TENANT, PROJECT, RUNNER, q).await.unwrap();
    assert!(inspected.to_string().contains("caller_asserted_reconciled"));
}

#[tokio::test]
async fn agent_completion_does_not_implicitly_release_a_required_dependency() {
    let (_g, admin, db, store) = setup().await;
    let chain = caller_chain(&admin, &db, &store).await;
    run(&store, RUNNER, "complete-upstream", "work.complete", completion_args(&chain)).await;
    let mut downstream: WorkContract = serde_json::from_value(
        admin.query_one("SELECT contract_json FROM awr_team.work_contracts WHERE work_id='c'", &[])
            .await.unwrap().get(0),
    ).unwrap();
    downstream.required_dependencies = vec!["a".into()];
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='c'",
        &[&json!(downstream), &downstream.hash().unwrap()]).await.unwrap();
    admin.batch_execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        SELECT tenant_id,project_id,'session-c',scope_id,'c',actor_id,client_id,'session-c','active',workstream_id,ownership_version
        FROM awr_team.sessions WHERE id='session-runner'").await.unwrap();
    let next = store.query(TENANT, PROJECT, RUNNER, query("work.next")).await.unwrap();
    let candidate = next["data"]["items"].as_array().unwrap().iter()
        .find(|item| item["work_id"] == "c").unwrap();
    assert_eq!(candidate["navigation"], "waiting_dependency");
    async fn downstream_command(store: &WorkstreamReadStore, key: &str, op: &str, args: Value) -> awr_team_pg::PgResult<Value> {
        let prepared = prepare(store, RUNNER, "c").await;
        store.commands().execute(TENANT, PROJECT, RUNNER, command(&prepared, key, op, args)).await
    }
    // Dual-intake protection now refuses unresolved dependency work before
    // ownership/lease acquisition, rather than waiting until execution start.
    let result = downstream_command(&store, "downstream-claim", "claim.acquire",
        json!({"session_id":"session-c","expected_session_version":"1","expected_work_version":"0","ttl_seconds":600})).await;
    assert!(matches!(result, Err(PgError::MissingDependency)));
    let stored = admin.query_one("SELECT
        (SELECT count(*) FROM awr_team.claims WHERE work_id='c'),
        (SELECT count(*) FROM awr_team.executions WHERE work_id='c'),
        (SELECT count(*) FROM awr_team.task_responsibilities WHERE work_id='c')", &[]).await.unwrap();
    for column in 0..3 { assert_eq!(stored.get::<_, i64>(column), 0); }
}

#[tokio::test]
async fn agent_completion_refuses_changed_artifacts_review_and_receipt_chain() {
    let (_g, admin, db, store) = setup().await;
    let chain = caller_chain(&admin, &db, &store).await;
    let exec = chain["execution_id"].as_str().unwrap();
    let caller = chain["caller_receipt_id"].as_str().unwrap();
    let reconcile = chain["reconcile_receipt_id"].as_str().unwrap();
    let round = chain["round_id"].as_str().unwrap();
    let evidence_id = chain["evidence_id"].as_str().unwrap();
    let artifact_id: String = admin
        .query_one(
            "SELECT artifact_id FROM awr_team.evidence WHERE id=$1",
            &[&evidence_id],
        )
        .await
        .unwrap()
        .get(0);
    let old_artifact: Vec<u8> = admin
        .query_one(
            "SELECT content FROM awr_team.artifacts WHERE id=$1",
            &[&artifact_id],
        )
        .await
        .unwrap()
        .get(0);
    admin
        .execute(
            "UPDATE awr_team.artifacts SET content=decode('00','hex') WHERE id=$1",
            &[&artifact_id],
        )
        .await
        .unwrap();
    assert!(matches!(
        run_err(
            &store,
            RUNNER,
            "changed-bytes",
            "work.complete",
            completion_args(&chain)
        )
        .await,
        PgError::EvidenceInvalid
    ));
    admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$1 WHERE id=$2",
            &[&old_artifact, &artifact_id],
        )
        .await
        .unwrap();
    for (key, sql, restore) in [
        (
            "changed-input",
            format!(
                "UPDATE awr_team.executions SET input_digest='{}' WHERE id='{exec}'",
                "d".repeat(64)
            ),
            format!("UPDATE awr_team.executions SET input_digest='{INPUT}' WHERE id='{exec}'"),
        ),
        (
            "wrong-reconcile-kind",
            format!(
                "UPDATE awr_team.execution_receipts SET receipt_kind='trusted_executor' WHERE id='{reconcile}'"
            ),
            format!(
                "UPDATE awr_team.execution_receipts SET receipt_kind='reconcile' WHERE id='{reconcile}'"
            ),
        ),
        (
            "caller-digest",
            format!(
                "UPDATE awr_team.execution_receipts SET digest='{}' WHERE id='{caller}'",
                "d".repeat(64)
            ),
            String::new(),
        ),
        (
            "stale-review",
            format!("UPDATE awr_team.review_rounds SET state='invalidated' WHERE id='{round}'"),
            format!("UPDATE awr_team.review_rounds SET state='approved' WHERE id='{round}'"),
        ),
    ] {
        let original: String = admin
            .query_one(
                "SELECT digest FROM awr_team.execution_receipts WHERE id=$1",
                &[&caller],
            )
            .await
            .unwrap()
            .get(0);
        admin.batch_execute(&sql).await.unwrap();
        let err = run_err(
            &store,
            RUNNER,
            key,
            "work.complete",
            completion_args(&chain),
        )
        .await;
        assert!(
            matches!(err, PgError::EvidenceInvalid | PgError::ReviewRequired),
            "{key}: {err:?}"
        );
        if restore.is_empty() {
            admin
                .execute(
                    "UPDATE awr_team.execution_receipts SET digest=$1 WHERE id=$2",
                    &[&original, &caller],
                )
                .await
                .unwrap();
        } else {
            admin.batch_execute(&restore).await.unwrap();
        }
    }
    assert_eq!(
        run(
            &store,
            RUNNER,
            "after-repairs",
            "work.complete",
            completion_args(&chain)
        )
        .await["human_approval"],
        false
    );
}
