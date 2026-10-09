// These checks use isolated PostgreSQL fixtures, not native business acceptance.
async fn v2(admin: &Client) {
    policy_mode(
        admin,
        "a",
        "clone-alpha",
        ExecutionSettlementMode::IndependentWorkspaceV2,
    )
    .await;
}

async fn expire(admin: &Client) {
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn v2_live_and_late_terminal_outcomes_preserve_claim_and_caller_trust() {
    for late in [false, true] {
        for outcome in ["succeeded", "failed", "cancelled"] {
            let (_g, admin, _, store) = setup().await;
            enable_writes(&admin).await;
            v2(&admin).await;
            let (c, e) = ready_intent(&store).await;
            let started = start(&store, &c, &e).await;
            for guidance in [&c["lease_guidance"], &started["lease_guidance"]] {
                assert!(guidance.to_string().len() <= 650);
                assert_eq!(guidance["basis"]["claim_id"], c["claim_id"]);
                assert_eq!(guidance["basis"]["lease_version"], "1");
                for key in ["condition", "basis", "next_action", "recheck"] {
                    assert!(!guidance[key].is_null(), "Missing lease guidance: {key}");
                }
            }
            if late {
                expire(&admin).await;
            }
            let before = snapshot(&admin).await;
            let cmd = declaration(&store, &c, &started, "v2-report", outcome).await;
            let response = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd.clone())
                .await
                .unwrap();
            let result = &response["receipt"]["data"];
            assert_eq!(result["state"], outcome);
            assert_eq!(result["effects_settled"], true);
            assert_eq!(result["artifact_verified"], false);
            assert_eq!(result["work_completed"], false);
            assert_eq!(result["recovery_blocked"], false);
            let after = snapshot(&admin).await;
            assert_eq!(after["claims"], before["claims"]);
            let payload = &after["receipts"][0]["payload_json"];
            assert_eq!(after["receipts"][0]["receipt_kind"], "caller_asserted");
            assert_eq!(
                payload["settlement_policy"]["mode"],
                "independent_workspace_v2"
            );
            assert_eq!(
                payload["lease_observation"]["basis"],
                if late { "expired_current" } else { "live" }
            );
            let valid: bool = admin.query_one(
                "SELECT ($1::jsonb->>'basis'='live') =
                   (($1::jsonb->>'expires_at')::timestamptz > ($1::jsonb->>'observed_at')::timestamptz)",
                &[&payload["lease_observation"]],
            ).await.unwrap().get(0);
            assert!(valid);
            assert_eq!(after["resources"][0]["state"], "released");
            let observed = inspect(&store, &e).await;
            assert_eq!(observed["execution_authorized"], false);
            assert_eq!(observed["artifact_verified"], false);
            let replay = store
                .commands()
                .execute(TENANT, PROJECT, A, cmd)
                .await
                .unwrap();
            assert_eq!(replay["receipt"], response["receipt"]);
            assert_eq!(replay["replayed"], true);
            assert_eq!(snapshot(&admin).await, after);
        }
    }
}

#[tokio::test]
async fn late_settlement_never_revives_lease_or_authorizes_new_execution() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    v2(&admin).await;
    let (c, e) = ready_intent(&store).await;
    let started = start(&store, &c, &e).await;
    expire(&admin).await;
    let cmd = declaration(&store, &c, &started, "late-success", "succeeded").await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    let p = prepare(&store, A, "a").await;
    let renewal = command(
        &p,
        "late-renew",
        "claim.renew",
        json!({
            "session_id":"session-a","expected_session_version":"1","claim_id":c["claim_id"],
            "expected_fence":c["fence"],"expected_lease_version":c["lease_version"],"ttl_seconds":600
        }),
    );
    let before = snapshot(&admin).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, renewal).await,
        Err(PgError::LeaseExpired)
    ));
    let old_intent = intent(&store, &c, "expired-intent").await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, old_intent)
            .await,
        Err(PgError::LeaseExpired)
    ));
    assert_eq!(snapshot(&admin).await, before);
    // A new attempt must obtain a new current fence and admission normally.
    let p = prepare(&store, A, "a").await;
    let c2 = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "new-claim",
                "claim.acquire",
                json!({
                    "session_id":"session-a","expected_session_version":"1",
                    "expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":600
                }),
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    assert_ne!(c2["claim_id"], c["claim_id"]);
    assert_eq!(c2["fence"], "2");
    let e2 = store
        .commands()
        .execute(TENANT, PROJECT, A, intent(&store, &c2, "new-intent").await)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let admitted = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            admission(&store, &c2, &e2, "new-start").await,
        )
        .await
        .unwrap();
    assert_eq!(admitted["execution_authorized"], true);
    assert_eq!(admitted["receipt"]["data"]["state"], "running");
    assert_ne!(e2["execution_id"], e["execution_id"]);
}

#[tokio::test]
async fn simultaneous_late_reports_commit_only_one_terminal_receipt() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    v2(&admin).await;
    let (c, e) = ready_intent(&store).await;
    let started = start(&store, &c, &e).await;
    expire(&admin).await;
    let a = declaration(&store, &c, &started, "late-a", "succeeded").await;
    let b = declaration(&store, &c, &started, "late-b", "succeeded").await;
    let commands = store.commands();
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, a),
        commands.execute(TENANT, PROJECT, A, b)
    );
    assert!(matches!(
        (a, b),
        (Ok(_), Err(PgError::PreconditionsChanged)) | (Err(PgError::PreconditionsChanged), Ok(_))
    ));
    let after = snapshot(&admin).await;
    assert_eq!(after["receipts"].as_array().unwrap().len(), 1);
    assert_eq!(after["resources"][0]["state"], "released");
}

#[tokio::test]
async fn expiry_during_resource_writes_is_rechecked_and_v1_rolls_back() {
    for mode in [
        ExecutionSettlementMode::IndependentWorkspaceV1,
        ExecutionSettlementMode::IndependentWorkspaceV2,
    ] {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        policy_mode(&admin, "a", "clone-alpha", mode).await;
        let (c, e) = ready_intent(&store).await;
        let started = start(&store, &c, &e).await;
        // Reservations have no claim_id: use the execution's exact originating claim.
        admin.batch_execute("CREATE FUNCTION awr_team.expire_at_release() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'
            WHERE id=(SELECT claim_id FROM awr_team.executions WHERE id=NEW.execution_id); RETURN NEW; END $$;
            CREATE TRIGGER expire_at_release BEFORE UPDATE ON awr_team.resource_reservations
            FOR EACH ROW WHEN (NEW.state='released') EXECUTE FUNCTION awr_team.expire_at_release()").await.unwrap();
        let before = snapshot(&admin).await;
        let cmd = declaration(&store, &c, &started, "cross-expiry", "succeeded").await;
        let result = store.commands().execute(TENANT, PROJECT, A, cmd).await;
        if mode == ExecutionSettlementMode::IndependentWorkspaceV1 {
            assert!(matches!(result, Err(PgError::LeaseExpired)));
            assert_eq!(snapshot(&admin).await, before);
        } else {
            assert_eq!(result.unwrap()["receipt"]["data"]["effects_settled"], true);
            let after = snapshot(&admin).await;
            assert_eq!(
                after["receipts"][0]["payload_json"]["lease_observation"]["basis"],
                "expired_current"
            );
        }
    }
}

#[tokio::test]
async fn late_report_refuses_changed_live_authority_and_identity_without_writes() {
    for case in [
        "session",
        "actor",
        "client",
        "ownership",
        "epoch",
        "fence",
        "generation",
        "environment",
        "released",
        "grant",
    ] {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        v2(&admin).await;
        let (c, e) = ready_intent(&store).await;
        let started = start(&store, &c, &e).await;
        expire(&admin).await;
        let cmd = declaration(&store, &c, &started, case, "succeeded").await;
        let sql = match case {
            "session" => "UPDATE awr_team.sessions SET state='ended' WHERE id='session-a'",
            "actor" => "UPDATE awr_team.executions SET executor_actor_id='reviewer'",
            "client" => "UPDATE awr_team.executions SET executor_client_id='cli-b'",
            "ownership" => "UPDATE awr_team.executions SET ownership_version=ownership_version+1",
            "epoch" => "UPDATE awr_team.projects SET coordinator_epoch='epoch-b'",
            "fence" => "UPDATE awr_team.work_runtime SET last_fence=last_fence+1 WHERE work_id='a'",
            "generation" => "UPDATE awr_team.claims SET lease_version=lease_version+1",
            "environment" => "UPDATE awr_team.executions SET environment_digest=repeat('d',64)",
            "released" => "UPDATE awr_team.claims SET state='released'",
            _ => {
                "UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-a'"
            }
        };
        admin.batch_execute(sql).await.unwrap();
        let before = snapshot(&admin).await;
        let result = store.commands().execute(TENANT, PROJECT, A, cmd).await;
        if case == "released" {
            let data = result.unwrap()["receipt"]["data"].clone();
            assert_eq!(data["effects_settled"], false);
            assert_eq!(data["recovery_blocked"], true);
            assert_eq!(snapshot(&admin).await["resources"][0]["state"], "unknown");
        } else {
            assert!(result.is_err(), "{case}");
            assert_eq!(snapshot(&admin).await, before, "{case}");
        }
    }
}

#[tokio::test]
async fn v2_never_reinterprets_an_old_contract_or_clears_an_unknown_report() {
    for old_unknown in [false, true] {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        if old_unknown {
            v2(&admin).await;
        } else {
            policy(&admin, "a", "clone-alpha").await;
        }
        let (c, e) = ready_intent(&store).await;
        let mut started = start(&store, &c, &e).await;
        expire(&admin).await;
        if old_unknown {
            started = store
                .commands()
                .execute(
                    TENANT,
                    PROJECT,
                    A,
                    report(&store, &started, "uncertain-first", "succeeded").await,
                )
                .await
                .unwrap()["receipt"]["data"]
                .clone();
            assert_eq!(started["state"], "unknown");
        } else {
            v2(&admin).await;
        }
        let prior = snapshot(&admin).await;
        let cmd = declaration(&store, &c, &started, "cannot-upgrade", "succeeded").await;
        let result = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap();
        assert_eq!(result["receipt"]["data"]["state"], "unknown");
        assert_eq!(result["receipt"]["data"]["effects_settled"], false);
        assert_eq!(result["receipt"]["data"]["recovery_blocked"], true);
        let after = snapshot(&admin).await;
        if let Some(prior_receipts) = prior["receipts"].as_array() {
            for receipt in prior_receipts {
                assert!(after["receipts"].as_array().unwrap().contains(receipt));
            }
        }
        assert_eq!(after["resources"][0]["state"], "unknown");
    }
}

#[tokio::test]
async fn late_v2_keeps_dependency_planning_and_controlled_execution_barriers() {
    for case in ["dependency", "planning", "controlled"] {
        let (_g, admin, db, store) = setup().await;
        enable_writes(&admin).await;
        v2(&admin).await;
        if case == "controlled" {
            admin.batch_execute("UPDATE awr_team.actors SET kind='system' WHERE id='agent';
                UPDATE awr_team.workstream_grants SET can_attest_execution=true,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
        }
        let (c, e) = ready_intent(&store).await;
        let mut admission = admission(&store, &c, &e, "guard-start").await;
        if case == "controlled" {
            admission.args["execution_mode"] = json!("reference_write_v1");
            admission.args["expected_input_digest"] = json!("a".repeat(64));
        }
        let started = store
            .commands()
            .execute(TENANT, PROJECT, A, admission)
            .await
            .unwrap()["receipt"]["data"]
            .clone();
        expire(&admin).await;
        if case == "dependency" {
            admin.batch_execute("INSERT INTO awr_team.dependency_bindings(tenant_id,project_id,downstream_work_id,upstream_work_id,binding_hash,valid)
                VALUES('reader-tenant','reader-project','a','b-private','changed-input',false)").await.unwrap();
        } else if case == "planning" {
            SelectiveInvalidationStore::from_config(with_app_role(&test_config(), &db))
                .record_planning_change(
                    TENANT,
                    PROJECT,
                    &RecordPlanningChangeRequest {
                        request_key: "plan-late".into(),
                        change_id: "plan-late".into(),
                        discovered_by: "agent".into(),
                        old_graph_version: "g0".into(),
                        new_graph_version: "g1".into(),
                        old_acceptance_contract: "a0".into(),
                        new_acceptance_contract: "a1".into(),
                        affected_work_ids: vec!["a".into()],
                        cancel_split_relations: vec![],
                        continue_conditions: vec!["review".into()],
                        all_project_work_ids: vec!["a".into(), "b-private".into(), "c".into()],
                        now_ms: 50,
                    },
                )
                .await
                .unwrap();
        }
        let cmd = declaration(&store, &c, &started, "blocked-late", "succeeded").await;
        let before = snapshot(&admin).await;
        let result = store.commands().execute(TENANT, PROJECT, A, cmd).await;
        if case == "controlled" {
            assert!(matches!(result, Err(PgError::PreconditionsChanged)));
            assert_eq!(snapshot(&admin).await, before);
        } else {
            assert_eq!(
                result.unwrap()["receipt"]["data"]["effects_settled"],
                false,
                "{case}"
            );
            assert_eq!(snapshot(&admin).await["resources"][0]["state"], "unknown");
        }
    }
}
