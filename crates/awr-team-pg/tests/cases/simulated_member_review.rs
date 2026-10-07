// Synthetic member fixtures exercise authenticated commands, not native acceptance.
const SIMULATED_POLICY: &str = "caller_managed_execution_and_simulated_member_review";

async fn issue_member_grant(
    db: &str,
    id: &str,
    member: &str,
    actor: &str,
    client: &str,
    binding: &str,
    review: bool,
) {
    let mut actions = BTreeSet::from([
        AuthorizedAction::Inspect,
        AuthorizedAction::StartWork,
        AuthorizedAction::ClaimCoordination,
    ]);
    if review {
        actions.insert(AuthorizedAction::Review);
    }
    AuthorizationStore::from_config(common::with_app_role(&common::test_config(), db))
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: id.into(),
                authorization: AgentAuthorization {
                    id: id.into(),
                    authorizer_person_id: PersonId::new("runner").unwrap(),
                    responsible_person_id: PersonId::new(member).unwrap(),
                    subject_kind: ExecutionSubjectKind::Agent,
                    subject_id: actor.into(),
                    client_id: client.into(),
                    session_id: None,
                    model_id: None,
                    scope: AuthorizationScope::Project {
                        project_id: PROJECT.into(),
                    },
                    actions,
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
                },
            },
        )
        .await
        .unwrap();
}

async fn seed_simulated_members(admin: &Client, db: &str) {
    seed_review_actors(admin).await;
    admin.batch_execute("UPDATE awr_team.actors SET kind='agent' WHERE id IN ('agent','reviewer');
        UPDATE awr_team.actors SET kind='human' WHERE id='runner';
        UPDATE awr_team.project_memberships SET agent_review=true,membership_version=membership_version+1 WHERE actor_id IN ('agent','reviewer');
        UPDATE awr_team.project_memberships SET role='admin' WHERE actor_id='runner';
        UPDATE awr_team.workstream_grants SET can_manage=true,can_reconcile_execution=true WHERE client_id='cli-runner';
        INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
          ('reader-tenant','person-author','agent','Simulated author','active'),
          ('reader-tenant','person-reviewer','agent','Simulated reviewer','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
          ('reader-tenant','reader-project','person-author','developer'),
          ('reader-tenant','reader-project','person-reviewer','developer');
        UPDATE awr_team.persons SET member_identity='{\"kind\":\"simulated_member\",\"controller_ref\":\"one-controller\"}' WHERE id='person-author';
        INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status,member_identity) VALUES
          ('reader-tenant','reader-project','person-reviewer','Simulated reviewer','active','{\"kind\":\"simulated_member\",\"controller_ref\":\"one-controller\"}'),
          ('reader-tenant','reader-project','runner','Supervisor','active',NULL);
        INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
          ('reader-tenant','reader-project','bind-reviewer','person-reviewer','reviewer','active');").await.unwrap();
    issue_member_grant(
        db,
        "simulation-author",
        "person-author",
        "agent",
        "cli-a",
        "bind-agent",
        true,
    )
    .await;
    issue_member_grant(
        db,
        "simulation-reviewer",
        "person-reviewer",
        "reviewer",
        "cli-reviewer",
        "bind-reviewer",
        true,
    )
    .await;
    let mut contract = current_contract(admin).await;
    contract.codec = WorkContract::CODEC_V4.into();
    contract.completion_policy = SIMULATED_POLICY.into();
    contract.execution_settlement = Some(awr_team::ExecutionSettlementPolicy {
        mode: awr_team::ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "member-workspace".into(),
    });
    contract.scope_paths = vec!["src/api".into()];
    contract.verification_requirements = vec!["Verify the produced API artifact".into()];
    put_contract(admin, &contract).await;
}

async fn simulated_intent(store: &WorkstreamReadStore) -> Value {
    let claim = run(
        store,
        A,
        "simulation-claim",
        "claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1",
        "expected_work_version":"0","ttl_seconds":600}),
    )
    .await;
    let p = prepare(store, A, "a").await;
    let intent=run(store,A,"simulation-intent","execution.prepare",json!({"session_id":"session-a","expected_session_version":"1",
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":INPUT,"declared_scope":["src/api"]})).await;
    assert_eq!(intent["member_attribution"]["member_id"], "person-author");
    json!({"claim":claim,"intent":intent})
}

async fn start_args(store: &WorkstreamReadStore, chain: &Value) -> Value {
    let p = prepare(store, A, "a").await;
    let claim = &chain["claim"];
    let intent = &chain["intent"];
    json!({"session_id":"session-a","expected_session_version":"1",
        "execution_id":intent["execution_id"],"expected_execution_version":intent["execution_version"],
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"})
}

async fn simulated_evidence(store: &WorkstreamReadStore) -> Value {
    use sha2::Digest;
    let chain = simulated_intent(store).await;
    let started = run(
        store,
        A,
        "simulation-start",
        "execution.start",
        start_args(store, &chain).await,
    )
    .await;
    let result = hex_encode(&sha2::Sha256::digest(b"simulated artifact"));
    let claim = &chain["claim"];
    let reported=run(store,A,"simulation-report","execution.report",json!({"session_id":"session-a","expected_session_version":"1",
        "execution_id":chain["intent"]["execution_id"],"expected_execution_version":started["execution_version"],
        "outcome":"succeeded","output_digest":result,"observed_paths":["src/api/result.json"],"note":"Observed the synthetic workspace result",
        "workspace_settlement":{"workspace_id":"member-workspace","input_digest":INPUT,"environment_digest":"c".repeat(64),
            "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "executor_stopped":true,"no_external_effects":true}})).await;
    assert_eq!(reported["state"], "succeeded");
    let evidence = run(
        store,
        A,
        "simulation-evidence",
        "evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
        "execution_id":chain["intent"]["execution_id"],"input_digest":INPUT,"dirty_tree":false,
        "artifact_text":"simulated artifact","payload":{"passed":true,"output_digest":result}}),
    )
    .await;
    assert_eq!(evidence["trust_basis"], "caller_asserted");
    json!({"evidence_id":evidence["evidence_id"],"execution_id":chain["intent"]["execution_id"]})
}

async fn open_simulated(
    store: &WorkstreamReadStore,
    evidence: &Value,
    token: &str,
    session: &str,
    key: &str,
) -> Value {
    run(store,token,key,"review.open",json!({"session_id":session,"expected_session_version":"1","evidence_id":evidence["evidence_id"]})).await
}

#[tokio::test]
async fn simulated_members_review_with_one_controller_and_immutable_inspectable_basis() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let caps = store
        .query(TENANT, PROJECT, REVIEWER_TOKEN, query("capabilities"))
        .await
        .unwrap();
    assert_eq!(
        caps["simulated_member_review"]["completion_supported"],
        true
    );
    let evidence = simulated_evidence(&store).await;
    let round = open_simulated(
        &store,
        &evidence,
        RUNNER,
        "session-runner",
        "supervisor-open",
    )
    .await;
    assert!(round.get("member_origins").is_none());
    assert!(round["member_attribution"]["kind"].is_null());
    let p = prepare(&store, REVIEWER_TOKEN, "a").await;
    let cmd = command(&p, "simulation-decision", "review.decide", args(&round));
    let response = store
        .commands()
        .execute(TENANT, PROJECT, REVIEWER_TOKEN, cmd.clone())
        .await
        .unwrap();
    let data = &response["receipt"]["data"];
    assert_eq!(
        data["approval_basis"],
        "simulated_member_independent_review"
    );
    assert_eq!(data["independence_kind"], "simulated_member_independent");
    for flag in [
        "human_approval",
        "team_independent_acceptance",
        "task_complete",
    ] {
        assert_eq!(data[flag], false);
    }
    assert_eq!(
        data["member_review_basis"]["executor_member_id"],
        "person-author"
    );
    assert!(data["member_review_basis"].get("origins").is_none());
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, REVIEWER_TOKEN, cmd)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], response["receipt"]);
    let mut q = query("review.inspect");
    q.work_id = Some("a".into());
    q.review_round_id = Some(round["round_id"].as_str().unwrap().into());
    let inspected = store
        .query(TENANT, PROJECT, REVIEWER_TOKEN, q)
        .await
        .unwrap();
    let basis = &inspected["data"]["review"]["decisions"][0]["member_review_basis"];
    assert_eq!(basis["origins"]["executor"]["member_id"], "person-author");
    assert_eq!(basis["origins"]["submitter"]["actor_id"], "agent");
    assert_eq!(basis["origins"]["opener"]["actor_kind"], "human");
    assert!(basis["origins"]["opener"]["member_identity"].is_null());
    assert_eq!(basis["reviewer"]["binding_id"], "bind-reviewer");
    assert_eq!(basis["authority"]["delegation_id"], "simulation-reviewer");
    let event: Value = admin
        .query_one(
            "SELECT payload_json FROM awr_team.events WHERE event_type='review.decided'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        event["member_review_basis"]["basis_digest"],
        data["member_review_basis"]["basis_digest"]
    );
    let finalized = run(
        &store,
        RUNNER,
        "simulation-finalize",
        "delivery.finalize",
        completion_args(&evidence),
    )
    .await;
    assert_eq!(finalized["task_complete"], true);
    assert_eq!(finalized["human_approval"], false);
    assert_eq!(finalized["team_independent_acceptance"], false);
    assert_eq!(
        finalized["member_review_basis"],
        data["member_review_basis"]
    );
    assert!(finalized["member_review_basis"].get("origins").is_none());
    let stored: Value = admin
        .query_one(
            "SELECT approved_by_json FROM awr_team.completion_receipts WHERE id=$1",
            &[&finalized["receipt_id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(&stored["member_review_basis"], basis);
    assert_eq!(stored["review_decision_id"], data["decision_id"]);
    assert!(admin.execute("UPDATE awr_team.completion_receipts SET approved_by_json=jsonb_set(approved_by_json,'{human_approval}','true') WHERE id=$1",
        &[&finalized["receipt_id"].as_str().unwrap()]).await.is_err());
    for (table, column, id) in [
        (
            "executions",
            "executor_origin_json",
            evidence["execution_id"].as_str().unwrap(),
        ),
        (
            "evidence",
            "member_origins_json",
            evidence["evidence_id"].as_str().unwrap(),
        ),
        (
            "review_rounds",
            "member_origins_json",
            round["round_id"].as_str().unwrap(),
        ),
        (
            "review_decisions",
            "member_review_basis_json",
            data["decision_id"].as_str().unwrap(),
        ),
    ] {
        let sql = format!("UPDATE awr_team.{table} SET {column}=NULL WHERE id=$1");
        assert!(
            admin.execute(&sql, &[&id]).await.is_err(),
            "{table} provenance can never be replaced"
        );
    }
}

#[tokio::test]
async fn simulated_admission_refuses_changed_origin_without_dispatch() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let chain = simulated_intent(&store).await;
    admin.batch_execute("UPDATE awr_team.project_memberships SET membership_version=membership_version+1 WHERE actor_id='person-author'").await.unwrap();
    let arguments = start_args(&store, &chain).await;
    assert!(matches!(
        run_err(
            &store,
            A,
            "changed-origin-start",
            "execution.start",
            arguments
        )
        .await,
        PgError::PreconditionsChanged
    ));
    let row = admin
        .query_one(
            "SELECT state,(SELECT count(*) FROM awr_team.outbox) FROM awr_team.executions",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "prepared");
    assert_eq!(row.get::<_, i64>(1), 0);
}

#[tokio::test]
async fn simulated_intents_refuse_missing_ambiguous_or_inactive_members() {
    for change in [
        "unspecified",
        "ambiguous",
        "disabled_member",
        "disabled_anchor",
        "missing_membership",
    ] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let claim=run(&store,A,"claim-before-change","claim.acquire",json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":"0","ttl_seconds":600})).await;
        let sql = match change {
            "unspecified" => {
                "UPDATE awr_team.persons SET member_identity=NULL WHERE id='person-author'"
            }
            "ambiguous" => {
                "INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','ambiguous','person-reviewer','agent','active')"
            }
            "disabled_member" => {
                "UPDATE awr_team.persons SET status='disabled' WHERE id='person-author'"
            }
            "disabled_anchor" => {
                "UPDATE awr_team.actors SET status='disabled' WHERE id='person-author'"
            }
            _ => "DELETE FROM awr_team.project_memberships WHERE actor_id='person-author'",
        };
        admin.batch_execute(sql).await.unwrap();
        let p = prepare(&store, RUNNER, "a").await;
        let result=store.commands().execute(TENANT,PROJECT,A,command(&p,"denied-member-intent","execution.prepare",
            json!({"session_id":"session-a","expected_session_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
                "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":INPUT,"declared_scope":["src/api"]}))).await;
        assert!(
            matches!(result, Err(PgError::Forbidden)),
            "{change}: {result:?}"
        );
        let count: i64 = admin
            .query_one("SELECT count(*) FROM awr_team.executions", &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn simulated_review_refuses_same_member_and_original_actor_after_rebinding() {
    for change in ["same_member", "original_actor"] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let evidence = simulated_evidence(&store).await;
        let round = open_simulated(
            &store,
            &evidence,
            RUNNER,
            "session-runner",
            "supervisor-open",
        )
        .await;
        let (token, session) = if change == "same_member" {
            admin.batch_execute("UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='bind-reviewer';
                INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','reviewer-as-author','person-author','reviewer','active')").await.unwrap();
            issue_member_grant(
                &db,
                "reviewer-rebound",
                "person-author",
                "reviewer",
                "cli-reviewer",
                "reviewer-as-author",
                true,
            )
            .await;
            (REVIEWER_TOKEN, "session-reviewer")
        } else {
            admin.batch_execute("UPDATE awr_team.person_agent_bindings SET status='disabled' WHERE id='bind-agent';
                INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','author-as-reviewer','person-reviewer','agent','active')").await.unwrap();
            issue_member_grant(
                &db,
                "author-rebound",
                "person-reviewer",
                "agent",
                "cli-a",
                "author-as-reviewer",
                true,
            )
            .await;
            (A, "session-a")
        };
        let mut arguments = args(&round);
        arguments["session_id"] = json!(session);
        assert!(
            matches!(
                run_err(
                    &store,
                    token,
                    "rebound-self-review",
                    "review.decide",
                    arguments
                )
                .await,
                PgError::AuthorCannotReview
            ),
            "{change}"
        );
        let count: i64 = admin
            .query_one("SELECT count(*) FROM awr_team.review_decisions", &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn simulated_review_refuses_a_distinct_member_using_an_original_client() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let evidence = simulated_evidence(&store).await;
    let round = open_simulated(
        &store,
        &evidence,
        RUNNER,
        "session-runner",
        "supervisor-open",
    )
    .await;
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET client_id='cli-a' WHERE id='reviewer-h';
        UPDATE awr_team.sessions SET client_id='cli-a' WHERE id='session-reviewer';
        UPDATE awr_team.workstream_grants SET client_id='cli-a' WHERE actor_id='reviewer'",
        )
        .await
        .unwrap();
    issue_member_grant(
        &db,
        "reviewer-client-rebound",
        "person-reviewer",
        "reviewer",
        "cli-a",
        "bind-reviewer",
        true,
    )
    .await;
    assert!(matches!(
        run_err(
            &store,
            REVIEWER_TOKEN,
            "same-client-review",
            "review.decide",
            args(&round)
        )
        .await,
        PgError::AuthorCannotReview
    ));
}

#[tokio::test]
async fn simulated_review_preserves_submitter_and_opener_origins() {
    for contribution in ["submitter", "opener"] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let mut evidence = simulated_evidence(&store).await;
        let round = if contribution == "submitter" {
            evidence=run(&store,REVIEWER_TOKEN,"reviewer-submitted-evidence","evidence.submit",json!({"session_id":"session-reviewer","expected_session_version":"1",
                "execution_id":evidence["execution_id"],"input_digest":INPUT,"dirty_tree":false,"artifact_text":"reviewer feedback",
                "payload":{"passed":true}})).await;
            open_simulated(
                &store,
                &evidence,
                RUNNER,
                "session-runner",
                "supervisor-open",
            )
            .await
        } else {
            open_simulated(
                &store,
                &evidence,
                REVIEWER_TOKEN,
                "session-reviewer",
                "reviewer-open",
            )
            .await
        };
        assert!(
            matches!(
                run_err(
                    &store,
                    REVIEWER_TOKEN,
                    "contributor-self-review",
                    "review.decide",
                    args(&round)
                )
                .await,
                PgError::AuthorCannotReview
            ),
            "{contribution}"
        );
    }
}

#[tokio::test]
async fn simulated_review_requires_current_grant_metadata_and_matching_policy() {
    for change in [
        "membership",
        "delegation",
        "metadata",
        "ambiguous",
        "policy",
        "legacy_alias",
    ] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let evidence = simulated_evidence(&store).await;
        let round = open_simulated(
            &store,
            &evidence,
            RUNNER,
            "session-runner",
            "supervisor-open",
        )
        .await;
        match change {
            "membership" => {
                admin.batch_execute("UPDATE awr_team.project_memberships SET agent_review=false,membership_version=membership_version+1 WHERE actor_id='reviewer'").await.unwrap();
            }
            "delegation" => {
                admin.batch_execute("UPDATE awr_team.agent_authorizations SET body_json=jsonb_set(body_json,'{actions}','[\"inspect\",\"start_work\"]') WHERE id='simulation-reviewer'").await.unwrap();
            }
            "metadata" => {
                admin.batch_execute("UPDATE awr_team.persons SET member_identity=NULL WHERE id='person-reviewer'").await.unwrap();
            }
            "ambiguous" => {
                admin.batch_execute("INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES('reader-tenant','reader-project','ambiguous-review','person-author','reviewer','active')").await.unwrap();
            }
            "policy" => {
                let mut c = current_contract(&admin).await;
                c.hard_rules.push("Changed review contract".into());
                put_contract(&admin, &c).await;
            }
            _ => {}
        }
        let (op, arguments) = if change == "legacy_alias" {
            let mut arguments = args(&round);
            arguments.as_object_mut().unwrap().remove("decision");
            ("review.accept", arguments)
        } else {
            ("review.decide", args(&round))
        };
        let p = prepare(&store, RUNNER, "a").await;
        let result = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                REVIEWER_TOKEN,
                command(&p, "revoked-review", op, arguments),
            )
            .await;
        assert!(
            matches!(result, Err(PgError::Forbidden | PgError::ReviewRequired)),
            "{change}: {result:?}"
        );
        let state: String = admin
            .query_one(
                "SELECT state FROM awr_team.review_rounds WHERE id=$1",
                &[&round["round_id"].as_str().unwrap()],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "open");
    }
}

#[tokio::test]
async fn simulated_sources_never_promote_legacy_unattributed_evidence() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let hash = current_contract(&admin).await.hash().unwrap();
    admin.execute("INSERT INTO awr_team.evidence(tenant_id,project_id,id,work_id,contract_hash,evidence_kind,trust_basis,digest,payload_json,created_by)
        VALUES($1,$2,'legacy-evidence','a',$3,'report','caller_asserted','legacy','{}','agent')",&[&TENANT,&PROJECT,&hash]).await.unwrap();
    assert!(matches!(run_err(&store,RUNNER,"legacy-open","review.open",json!({"session_id":"session-runner","expected_session_version":"1","evidence_id":"legacy-evidence"})).await,PgError::EvidenceInvalid));
    let count: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.review_rounds", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
}
