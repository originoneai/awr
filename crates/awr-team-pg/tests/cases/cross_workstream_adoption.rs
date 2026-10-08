// Heap-bound command futures keep this multi-stage debug fixture within the
// standard test thread stack; no runtime or test stack limit is changed.
fn cross_step<'a>(
    store: &'a WorkstreamReadStore,
    token: &'a str,
    work: &'a str,
    key: &'a str,
    op: &'a str,
    args: Value,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Value> + 'a>> {
    Box::pin(on_work(store, token, work, key, op, args))
}

fn cross_execute<'a>(
    store: &'a WorkstreamReadStore,
    token: &'a str,
    request: awr_team_pg::WorkstreamCommand,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, PgError>> + 'a>> {
    Box::pin(async move {
        store
            .commands()
            .execute(TENANT, PROJECT, token, request)
            .await
    })
}

// Authenticated mechanism fixtures; native business acceptance is separate.
async fn adoption_session(store: &WorkstreamReadStore, key: &str) -> String {
    on_work(
        store,
        B,
        "b-private",
        key,
        "session.start",
        json!({"conversation_id":key}),
    )
    .await["session_id"]
        .as_str()
        .unwrap()
        .into()
}

async fn adoption_request(
    store: &WorkstreamReadStore,
    session: &str,
    exported: &Value,
    key: &str,
) -> awr_team_pg::WorkstreamCommand {
    let p = prepare(store, B, "b-private").await;
    command(
        &p,
        key,
        "delivery.adopt",
        json!({
            "session_id":session,"expected_session_version":"1",
            "expected_responsibility_version":p["data"]["responsibility"]["version"],
            "expected_adoption_version":p["data"]["adopted_dependencies"][0]["adoption_version"].as_str().unwrap_or("0"),
            "export_id":exported["receipt"]["data"]["export_id"],
            "expected_export_version":exported["receipt"]["data"]["export_version"],
            "expected_disclosure_sha256":exported["receipt"]["data"]["disclosure_sha256"],
        }),
    )
}

async fn claim_cross_consumer(
    store: &WorkstreamReadStore,
    session: &str,
    key: &str,
) -> Result<Value, PgError> {
    let p = prepare(store, B, "b-private").await;
    store.commands().execute(TENANT,PROJECT,B,command(&p,key,"task.claim_available",json!({
        "session_id":session,"expected_session_version":"1",
        "expected_responsibility_version":p["data"]["responsibility"]["version"],
        "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
        "ttl_seconds":600,
    }))).await
}

async fn adoption_roles(admin: &Client) {
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1 WHERE client_id='cli-b';
        UPDATE awr_team.project_memberships SET assignment_grant=true,membership_version=membership_version+1 WHERE actor_id='runner'").await.unwrap();
    for (actor, client) in [("runner", "cli-runner"), ("reviewer", "cli-reviewer")] {
        admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
            VALUES($1,$2,$3,$4,$5,1,true,true)", &[&TENANT,&PROJECT,&actor,&client,&awr_core::Id::from(2).to_string()]).await.unwrap();
    }
}

async fn adopt_export(
    store: &WorkstreamReadStore,
    session: &str,
    export: &Value,
    key: &str,
) -> Value {
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            B,
            adoption_request(store, session, export, key).await,
        )
        .await
        .unwrap_or_else(|error| panic!("{key}: {error:?}"))
}

async fn revoke_adoption_export(store: &WorkstreamReadStore, export: &Value, key: &str) {
    let p = prepare(store, RUNNER, "a").await;
    store.commands().execute(TENANT,PROJECT,RUNNER,command(&p,key,"delivery.export.revoke",json!({
        "session_id":"session-runner","expected_session_version":"1","export_id":export["receipt"]["data"]["export_id"],
        "expected_export_version":export["receipt"]["data"]["export_version"],
    }))).await.unwrap();
}

async fn cross_navigation(store: &WorkstreamReadStore) -> Value {
    store
        .query(TENANT, PROJECT, B, query("work.next"))
        .await
        .unwrap()["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["work_id"] == "b-private")
        .unwrap()
        .clone()
}

#[tokio::test]
async fn cross_stream_adoption_unlocks_pool_claim_only_after_explicit_selection() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = exported_consumer(&admin, &store, &db).await;
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1 WHERE client_id='cli-b'").await.unwrap();
    let session = adoption_session(&store, "pool-adoption-session").await;
    let exported = publish_export(&store, &consumer, &upstream, "pool-export").await;
    assert!(matches!(
        claim_cross_consumer(&store, &session, "before-adoption").await,
        Err(PgError::MissingDependency)
    ));
    store
        .query(TENANT, PROJECT, B, export_content(&exported))
        .await
        .unwrap();
    assert!(matches!(
        claim_cross_consumer(&store, &session, "read-without-adoption").await,
        Err(PgError::MissingDependency)
    ));
    let adopted = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            B,
            adoption_request(&store, &session, &exported, "pool-adopt").await,
        )
        .await
        .expect("Explicit authenticated current-contract adoption must be supported");
    assert_eq!(adopted["receipt"]["data"]["adopted"], true);
    let p = prepare(&store, B, "b-private").await;
    assert_eq!(p["data"]["context_complete"], true);
    assert_eq!(p["data"]["dependency_export_unavailable"], false);
    assert_eq!(
        p["data"]["adopted_dependencies"][0]["receipt_id"],
        upstream["receipt_id"]
    );
    assert_ne!(
        cross_navigation(&store).await["navigation"],
        "waiting_dependency"
    );
    assert_eq!(discover_exports(&store).await["items"][0]["adopted"], true);
    assert_eq!(
        store
            .query(TENANT, PROJECT, B, export_content(&exported))
            .await
            .unwrap()["data"]["adopted"],
        true
    );
    assert_eq!(
        publish_export(&store, &consumer, &upstream, "already-adopted-export").await["receipt"]["data"]
            ["adopted"],
        true
    );
    assert!(
        claim_cross_consumer(&store, &session, "adopted-claim")
            .await
            .is_ok()
    );
    let mut private = query("work.snapshot");
    private.work_id = Some("a".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, B, private).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn cross_stream_adoption_allows_pending_assignment_before_acceptance() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = exported_consumer(&admin, &store, &db).await;
    adoption_roles(&admin).await;
    let session = adoption_session(&store, "assigned-adoption-session").await;
    let p = prepare(&store, RUNNER, "b-private").await;
    let assignment=store.commands().execute(TENANT,PROJECT,RUNNER,command(&p,"cross-assign","task.assign",json!({
        "assignee_person_id":"person-author","expected_responsibility_version":p["data"]["responsibility"]["version"],
    }))).await.unwrap();
    assert_eq!(
        assignment["receipt"]["data"]["coordination_claim_acquired"],
        false
    );
    let exported = publish_export(&store, &consumer, &upstream, "assigned-export").await;
    for (key, allowed) in [("blocked-accept", false), ("adopted-accept", true)] {
        if allowed {
            adopt_export(&store, &session, &exported, "assigned-adopt").await;
        }
        let p = prepare(&store, B, "b-private").await;
        let accept = command(
            &p,
            key,
            "task.accept_assignment",
            json!({
                "session_id":session,"expected_session_version":"1","expected_responsibility_version":p["data"]["responsibility"]["version"],
                "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),"ttl_seconds":600,
                "assignment_request_key":p["data"]["responsibility"]["pending"]["transfer_request_key"],
            }),
        );
        let result = store.commands().execute(TENANT, PROJECT, B, accept).await;
        if allowed {
            assert_eq!(
                result.unwrap()["receipt"]["data"]["coordination_claim_acquired"],
                true
            );
        } else {
            assert!(matches!(result, Err(PgError::MissingDependency)));
        }
    }
}

#[tokio::test]
async fn cross_stream_adoption_serializes_replay_and_invalidates_after_revocation() {
    adoption_replay_and_revocation(false).await;
}

#[tokio::test]
async fn fixed_delivery_adoption_replay_and_revocation() {
    adoption_replay_and_revocation(true).await;
}

async fn adoption_replay_and_revocation(fixed: bool) {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = if fixed {
        fixed_export_consumer(&admin, &store, &db).await
    } else {
        exported_consumer(&admin, &store, &db).await
    };
    adoption_roles(&admin).await;
    let session = adoption_session(&store, "replay-adoption-session").await;
    let exported = publish_export(&store, &consumer, &upstream, "replay-adoption-export").await;
    let request = adoption_request(&store, &session, &exported, "concurrent-adopt").await;
    let commands = store.commands();
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, B, request.clone()),
        commands.execute(TENANT, PROJECT, B, request.clone())
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a["replayed"], b["replayed"]);
    assert_eq!(a["receipt"], b["receipt"]);
    assert_eq!(a["execution_authorized"], false);
    let mut changed = request.clone();
    changed.args["expected_adoption_version"] = json!("1");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, B, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    assert_eq!(
        admin
            .query_one(
                "SELECT count(*) FROM awr_team.workstream_artifact_adoptions",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    revoke_adoption_export(&store, &exported, "replay-revoke").await;
    let mut inspect = query("command.inspect");
    inspect.work_id = Some("b-private".into());
    inspect.request_id = Some("concurrent-adopt".into());
    let known = store.query(TENANT, PROJECT, B, inspect).await.unwrap();
    assert_eq!(known["data"]["state"], "committed");
    assert_eq!(known["data"]["receipt"], a["receipt"]);
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, B, request)
        .await
        .unwrap();
    assert_eq!(replay["receipt"], a["receipt"]);
    assert_eq!(replay["execution_authorized"], false);
    assert_eq!(
        prepare(&store, B, "b-private").await["data"]["adopted_dependencies"][0]["valid"],
        false
    );
    assert_eq!(discover_exports(&store).await["items"][0]["adopted"], false);
    assert_eq!(
        cross_navigation(&store).await["navigation"],
        "waiting_dependency"
    );
    assert!(matches!(
        claim_cross_consumer(&store, &session, "revoked-claim").await,
        Err(PgError::MissingDependency)
    ));
    assert!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                B,
                adoption_request(&store, &session, &exported, "revoked-adopt").await
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cross_stream_assignment_adoption_race_preserves_the_supervisor_assignment() {
    for assignee in ["person-author", "person-reviewer"] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let upstream = complete_simulated_upstream(&store).await;
        let consumer = exported_consumer(&admin, &store, &db).await;
        adoption_roles(&admin).await;
        let session = adoption_session(&store, "assignment-race-session").await;
        let export = publish_export(&store, &consumer, &upstream, "assignment-race-export").await;
        let adoption = adoption_request(&store, &session, &export, "assignment-race-adopt").await;
        let p = prepare(&store, RUNNER, "b-private").await;
        let assign = command(
            &p,
            "assignment-race-dispatch",
            "task.assign",
            json!({
                "assignee_person_id":assignee,"expected_responsibility_version":p["data"]["responsibility"]["version"],
            }),
        );
        let commands = store.commands();
        let (adopted, assigned) = tokio::join!(
            commands.execute(TENANT, PROJECT, B, adoption),
            commands.execute(TENANT, PROJECT, RUNNER, assign)
        );
        assert!(
            assigned.is_ok(),
            "Adoption must not steal or invalidate supervisor responsibility"
        );
        assert!(
            adopted.is_ok()
                || matches!(
                    adopted,
                    Err(PgError::PreconditionsChanged | PgError::ClaimHeld)
                )
        );
        let p = prepare(&store, B, "b-private").await;
        assert_eq!(
            p["data"]["responsibility"]["pending"]["person_id"],
            assignee
        );
        assert_eq!(p["data"]["responsibility"]["version"], "1");
        assert_eq!(p["data"]["responsibility"]["owner"], assignee);
        assert!(p["data"]["responsibility"]["current_executor"].is_null());
        assert!(
            claim_cross_consumer(&store, &session, "cannot-self-claim-assigned-work")
                .await
                .is_err()
        );
        let retry = store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                B,
                adoption_request(&store, &session, &export, "fresh-assigned-adoption").await,
            )
            .await;
        if assignee == "person-author" {
            assert!(retry.is_ok());
        } else {
            assert!(matches!(retry, Err(PgError::ClaimHeld)));
        }
    }
}

#[tokio::test]
async fn cross_stream_competing_export_versions_never_select_a_withdrawn_input() {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = exported_consumer(&admin, &store, &db).await;
    adoption_roles(&admin).await;
    let session = adoption_session(&store, "export-version-race-session").await;
    let old = publish_export(&store, &consumer, &upstream, "export-version-old").await;
    revoke_adoption_export(&store, &old, "export-version-withdraw").await;
    let new = publish_export(&store, &consumer, &upstream, "export-version-new").await;
    assert_ne!(
        old["receipt"]["data"]["export_id"],
        new["receipt"]["data"]["export_id"]
    );
    let stale = adoption_request(&store, &session, &old, "export-version-stale-adopt").await;
    let current = adoption_request(&store, &session, &new, "export-version-current-adopt").await;
    let commands = store.commands();
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, B, stale),
        commands.execute(TENANT, PROJECT, B, current)
    );
    assert!(matches!(a, Err(PgError::Forbidden)));
    assert!(b.is_ok());
    let selected = admin
        .query_one(
            "SELECT export_id,version FROM awr_team.workstream_artifact_adoptions WHERE selected",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        selected.get::<_, String>(0),
        new["receipt"]["data"]["export_id"].as_str().unwrap()
    );
    assert_eq!(selected.get::<_, i64>(1), 1);
    assert!(
        claim_cross_consumer(&store, &session, "claim-current-export-version")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn cross_stream_adoption_rechecks_version_selectors() {
    for (field, value) in [
        ("expected_session_version", json!("2")),
        ("expected_responsibility_version", json!("1")),
        ("expected_adoption_version", json!("1")),
        ("expected_export_version", json!("2")),
        ("expected_disclosure_sha256", json!("f".repeat(64))),
        ("expected_session_version", json!("01")),
        ("expected_adoption_version", json!("-1")),
    ] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let upstream = complete_simulated_upstream(&store).await;
        let consumer = exported_consumer(&admin, &store, &db).await;
        adoption_roles(&admin).await;
        let session = adoption_session(&store, "selector-session").await;
        let exported = publish_export(&store, &consumer, &upstream, "selector-export").await;
        let mut request = adoption_request(&store, &session, &exported, "invalid-selector").await;
        request.args[field] = value.clone();
        assert!(
            store
                .commands()
                .execute(TENANT, PROJECT, B, request)
                .await
                .is_err(),
            "{field}={value}"
        );
        assert_eq!(
            admin
                .query_one(
                    "SELECT count(*) FROM awr_team.workstream_artifact_adoptions",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
    }
}

#[tokio::test]
async fn cross_stream_adoption_never_reuses_invalid_upstream_proof_or_consumer_contract() {
    for mutation in [
        "artifact",
        "original_review",
        "provider_contract",
        "consumer_contract",
        "provider_selection",
        "consumer_ownership",
    ] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let upstream = complete_simulated_upstream(&store).await;
        let consumer = exported_consumer(&admin, &store, &db).await;
        adoption_roles(&admin).await;
        let session = adoption_session(&store, "invalidation-session").await;
        let exported = publish_export(&store, &consumer, &upstream, "invalidation-export").await;
        adopt_export(&store, &session, &exported, "valid-adopt").await;
        match mutation {
            "artifact" => {
                admin
                    .batch_execute("UPDATE awr_team.artifacts SET content=NULL")
                    .await
                    .unwrap();
            }
            "original_review" => {
                admin
                    .batch_execute("UPDATE awr_team.review_rounds SET state='invalidated'")
                    .await
                    .unwrap();
            }
            "provider_selection" => {
                admin.batch_execute("UPDATE awr_team.work_runtime SET state='unclaimed',selected_completion_id=NULL WHERE work_id='a'").await.unwrap();
            }
            "consumer_ownership" => {
                admin.batch_execute("UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=ownership_version+1 WHERE work_id='b-private'").await.unwrap();
            }
            other => {
                let work = if other == "provider_contract" {
                    "a"
                } else {
                    "b-private"
                };
                let mut contract: WorkContract = serde_json::from_value(
                    admin
                        .query_one(
                            "SELECT contract_json FROM awr_team.work_contracts WHERE work_id=$1",
                            &[&work],
                        )
                        .await
                        .unwrap()
                        .get(0),
                )
                .unwrap();
                contract.acceptance.push("Changed requirement".into());
                admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id=$3",&[&json!(contract),&contract.hash().unwrap(),&work]).await.unwrap();
            }
        }
        let p = prepare(&store, B, "b-private").await;
        assert_eq!(
            p["data"]["adopted_dependencies"][0]["valid"], false,
            "{mutation}"
        );
        assert_eq!(
            p["data"]["dependency_export_unavailable"], true,
            "{mutation}"
        );
        assert!(
            claim_cross_consumer(&store, &session, "invalid-proof-claim")
                .await
                .is_err(),
            "{mutation}"
        );
        assert_eq!(
            admin
                .query_one(
                    "SELECT count(*) FROM awr_team.workstream_artifact_adoptions",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            1
        );
    }
}

#[tokio::test]
async fn cross_stream_adoption_rolls_back_selection_and_recovers_after_reading_unknown_result() {
    adoption_rollback_and_read_first_recovery(false).await;
}

#[tokio::test]
async fn fixed_delivery_adoption_rollback_and_read_first_recovery() {
    adoption_rollback_and_read_first_recovery(true).await;
}

async fn adoption_rollback_and_read_first_recovery(fixed: bool) {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = if fixed {
        fixed_export_consumer(&admin, &store, &db).await
    } else {
        exported_consumer(&admin, &store, &db).await
    };
    adoption_roles(&admin).await;
    let session = adoption_session(&store, "rollback-adoption-session").await;
    let first = publish_export(&store, &consumer, &upstream, "rollback-first-export").await;
    let original = adopt_export(&store, &session, &first, "rollback-first-adopt").await;
    revoke_adoption_export(&store, &first, "rollback-first-revoke").await;
    let second = publish_export(&store, &consumer, &upstream, "rollback-second-export").await;
    let request = adoption_request(&store, &session, &second, "interrupted-adopt").await;
    admin.batch_execute("CREATE FUNCTION awr_team.fixture_fail_adoption() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture write failure'; END $$;
        CREATE TRIGGER fixture_fail_adoption BEFORE INSERT ON awr_team.workstream_artifact_adoptions FOR EACH ROW EXECUTE FUNCTION awr_team.fixture_fail_adoption()").await.unwrap();
    assert!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, request.clone())
            .await
            .is_err()
    );
    let selected = admin
        .query_one(
            "SELECT id,version FROM awr_team.workstream_artifact_adoptions WHERE selected",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        selected.get::<_, String>(0),
        original["receipt"]["data"]["adoption_id"].as_str().unwrap()
    );
    assert_eq!(selected.get::<_, i64>(1), 1);
    let mut inspect = query("command.inspect");
    inspect.work_id = Some("b-private".into());
    inspect.request_id = Some("interrupted-adopt".into());
    assert_eq!(
        store.query(TENANT, PROJECT, B, inspect).await.unwrap()["data"]["state"],
        "unknown"
    );
    assert_eq!(
        prepare(&store, B, "b-private").await["project_revision"],
        request.expected_project_revision
    );
    admin.batch_execute("DROP TRIGGER fixture_fail_adoption ON awr_team.workstream_artifact_adoptions; DROP FUNCTION awr_team.fixture_fail_adoption()").await.unwrap();
    let mut contender = request.clone();
    contender.request_id = "competing-adoption".into();
    let commands = store.commands();
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, B, request),
        commands.execute(TENANT, PROJECT, B, contender)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let failed = if a.is_err() {
        a.as_ref().unwrap_err()
    } else {
        b.as_ref().unwrap_err()
    };
    assert!(matches!(failed, PgError::PreconditionsChanged));
    let rows=admin.query_one("SELECT count(*),count(*) FILTER(WHERE selected),max(version) FROM awr_team.workstream_artifact_adoptions",&[]).await.unwrap();
    assert_eq!(rows.get::<_, i64>(0), 2);
    assert_eq!(rows.get::<_, i64>(1), 1);
    assert_eq!(rows.get::<_, i64>(2), 2);
}

#[tokio::test]
async fn cross_stream_adoption_rechecks_live_authority_and_assignee_without_mutation() {
    adoption_current_authority_and_assignee(false).await;
}

#[tokio::test]
async fn fixed_delivery_adoption_current_authority_and_assignee() {
    adoption_current_authority_and_assignee(true).await;
}

async fn adoption_current_authority_and_assignee(fixed: bool) {
    for mutation in [
        "grant",
        "delegation",
        "credential",
        "session",
        "other_assignee",
        "terminal",
        "recovery_block",
    ] {
        let (_g, admin, db, store) = setup().await;
        seed_simulated_members(&admin, &db).await;
        let upstream = complete_simulated_upstream(&store).await;
        let consumer = if fixed {
            fixed_export_consumer(&admin, &store, &db).await
        } else {
            exported_consumer(&admin, &store, &db).await
        };
        adoption_roles(&admin).await;
        let session = adoption_session(&store, "authority-adoption-session").await;
        let exported =
            publish_export(&store, &consumer, &upstream, "authority-adoption-export").await;
        if mutation == "other_assignee" {
            let p = prepare(&store, RUNNER, "b-private").await;
            store.commands().execute(TENANT,PROJECT,RUNNER,command(&p,"assign-other-member","task.assign",json!({
                "assignee_person_id":"person-reviewer","expected_responsibility_version":p["data"]["responsibility"]["version"],
            }))).await.unwrap();
        }
        let request = adoption_request(&store, &session, &exported, "unauthorized-adoption").await;
        match mutation {
            "grant" => {
                admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=false,grant_version=grant_version+1 WHERE client_id='cli-b'").await.unwrap();
            }
            "delegation" => {
                admin.batch_execute("UPDATE awr_team.agent_authorizations SET status='revoked' WHERE client_id='cli-b'").await.unwrap();
            }
            "credential" => {
                admin.batch_execute("UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE client_id='cli-b'").await.unwrap();
            }
            "session" => {
                admin
                    .execute(
                        "UPDATE awr_team.sessions SET state='ended' WHERE id=$1",
                        &[&session],
                    )
                    .await
                    .unwrap();
            }
            "terminal" => {
                admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,work_id,scope_id,state,work_version)
                VALUES('reader-tenant','reader-project','b-private','main','cancelled',1)").await.unwrap();
            }
            "recovery_block" => {
                admin.batch_execute("INSERT INTO awr_team.work_runtime(tenant_id,project_id,work_id,scope_id,state,work_version,recovery_blocked)
                    VALUES('reader-tenant','reader-project','b-private','main','unclaimed',1,true)").await.unwrap();
            }
            _ => {}
        }
        assert!(
            store
                .commands()
                .execute(TENANT, PROJECT, B, request)
                .await
                .is_err(),
            "{mutation}"
        );
        assert_eq!(
            admin
                .query_one(
                    "SELECT count(*) FROM awr_team.workstream_artifact_adoptions",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            0,
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn cross_stream_adoption_and_revoke_race_never_unlocks_a_withdrawn_export() {
    adoption_and_revoke_race(false).await;
}

#[tokio::test]
async fn fixed_delivery_adoption_and_revoke_race() {
    adoption_and_revoke_race(true).await;
}

async fn adoption_and_revoke_race(fixed: bool) {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let consumer = if fixed {
        fixed_export_consumer(&admin, &store, &db).await
    } else {
        exported_consumer(&admin, &store, &db).await
    };
    adoption_roles(&admin).await;
    let session = adoption_session(&store, "revoke-race-session").await;
    let exported = publish_export(&store, &consumer, &upstream, "revoke-race-export").await;
    let adoption = adoption_request(&store, &session, &exported, "race-adopt").await;
    let p = prepare(&store, RUNNER, "a").await;
    let revoke = command(
        &p,
        "race-revoke",
        "delivery.export.revoke",
        json!({
            "session_id":"session-runner","expected_session_version":"1","export_id":exported["receipt"]["data"]["export_id"],"expected_export_version":"1",
        }),
    );
    let commands = store.commands();
    let (adopted, revoked) = tokio::join!(
        commands.execute(TENANT, PROJECT, B, adoption),
        commands.execute(TENANT, PROJECT, RUNNER, revoke)
    );
    // Different task read sets may commit serially in either order. If adoption
    // wins first, withdrawal still invalidates it; the final gate must be closed.
    assert!(
        revoked.is_ok(),
        "Withdrawal of the unchanged provider export must commit"
    );
    assert!(
        adopted.is_ok()
            || matches!(
                adopted,
                Err(PgError::Forbidden | PgError::PreconditionsChanged)
            )
    );
    assert_eq!(
        prepare(&store, B, "b-private").await["data"]["adopted_dependencies"][0]["valid"],
        false
    );
    assert!(matches!(
        claim_cross_consumer(&store, &session, "claim-after-race").await,
        Err(PgError::MissingDependency)
    ));
}

#[tokio::test]
async fn cross_stream_adoption_gates_execution_and_finalization_and_links_exact_input() {
    adoption_execution_completion_and_original_inputs(false).await;
}

#[tokio::test]
async fn fixed_delivery_adoption_execution_completion_and_original_inputs() {
    adoption_execution_completion_and_original_inputs(true).await;
}

async fn adoption_execution_completion_and_original_inputs(fixed: bool) {
    let (_g, admin, db, store) = setup().await;
    seed_simulated_members(&admin, &db).await;
    let upstream = complete_simulated_upstream(&store).await;
    let mut consumer = if fixed {
        fixed_export_consumer(&admin, &store, &db).await
    } else {
        exported_consumer(&admin, &store, &db).await
    };
    adoption_roles(&admin).await;
    consumer.completion_policy = SIMULATED_POLICY.into();
    consumer.scope_paths = vec!["src/integration".into()];
    consumer.verification_requirements = vec!["Verify the integrated artifact".into()];
    consumer.execution_settlement = Some(awr_team::ExecutionSettlementPolicy {
        mode: awr_team::ExecutionSettlementMode::IndependentWorkspaceV1,
        workspace_id: "cross-consumer-workspace".into(),
    });
    admin.execute("UPDATE awr_team.work_contracts SET contract_json=$1,contract_hash=$2 WHERE work_id='b-private'",&[&json!(consumer),&consumer.hash().unwrap()]).await.unwrap();
    let session = adoption_session(&store, "complete-adoption-session").await;
    let reviewer_session = cross_step(
        &store,
        REVIEWER_TOKEN,
        "b-private",
        "cross-reviewer-session",
        "session.start",
        json!({"conversation_id":"cross-reviewer"}),
    )
    .await["session_id"]
        .clone();
    let finalizer_session = cross_step(
        &store,
        RUNNER,
        "b-private",
        "cross-supervisor-session",
        "session.start",
        json!({"conversation_id":"cross-supervisor"}),
    )
    .await["session_id"]
        .clone();
    let exported = publish_export(&store, &consumer, &upstream, "complete-adoption-export").await;
    adopt_export(&store, &session, &exported, "complete-adopt").await;
    let claim = claim_cross_consumer(&store, &session, "cross-claim")
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    let p = prepare(&store, B, "b-private").await;
    let intent=cross_step(&store,B,"b-private","cross-intent","execution.prepare",json!({
        "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],
        "expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "input_digest":INPUT,"declared_scope":["src/integration"],
    })).await;
    revoke_adoption_export(&store, &exported, "cross-start-revoke").await;
    let p = prepare(&store, B, "b-private").await;
    let start_args = json!({"session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "execution_id":intent["execution_id"],"expected_execution_version":intent["execution_version"],"execution_mode":"caller_managed"});
    assert!(
        cross_execute(
            &store,
            B,
            command(
                &p,
                "invalid-cross-start",
                "execution.start",
                start_args.clone()
            )
        )
        .await
        .is_err()
    );
    let replacement = publish_export(&store, &consumer, &upstream, "cross-replacement").await;
    assert!(matches!(
        cross_execute(
            &store,
            B,
            adoption_request(&store, &session, &replacement, "adopt-with-prepared-intent").await
        )
        .await,
        Err(PgError::RecoveryBlocked)
    ));
    let cancelled=cross_step(&store,B,"b-private","cancel-stale-intent","execution.cancel",json!({
        "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],
        "expected_execution_version":intent["execution_version"],
    })).await;
    assert_eq!(cancelled["stop_confirmed"], true);
    let p = prepare(&store, B, "b-private").await;
    assert!(matches!(cross_execute(&store,B,command(&p,"prepare-with-withdrawn-adoption","execution.prepare",json!({
        "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],
        "expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "input_digest":INPUT,"declared_scope":["src/integration"],
    }))).await,Err(PgError::MissingDependency)));
    adopt_export(&store, &session, &replacement, "cross-readopt").await;
    let p = prepare(&store, B, "b-private").await;
    let intent=cross_step(&store,B,"b-private","fresh-cross-intent","execution.prepare",json!({
        "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],
        "expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "input_digest":INPUT,"declared_scope":["src/integration"],
    })).await;
    let p = prepare(&store, B, "b-private").await;
    let started=cross_execute(&store, B,command(&p,"cross-start","execution.start",json!({
        "session_id":session,"expected_session_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],
        "expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],
        "execution_id":intent["execution_id"],"expected_execution_version":intent["execution_version"],"execution_mode":"caller_managed",
    }))).await.unwrap();
    assert_eq!(started["execution_authorized"], true);
    let started = started["receipt"]["data"].clone();
    assert!(
        cross_execute(
            &store,
            B,
            adoption_request(
                &store,
                &session,
                &replacement,
                "adopt-during-live-execution"
            )
            .await
        )
        .await
        .is_err()
    );
    let output = hex_encode(&sha2::Sha256::digest(b"cross integrated artifact"));
    cross_step(&store,B,"b-private","cross-report","execution.report",json!({
        "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],
        "expected_execution_version":started["execution_version"],"outcome":"succeeded","output_digest":output,
        "observed_paths":["src/integration/result.json"],"note":"Observed the cross-stream artifact",
        "workspace_settlement":{"workspace_id":"cross-consumer-workspace","input_digest":INPUT,"environment_digest":"c".repeat(64),
            "claim_id":claim["claim_id"],"expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],"executor_stopped":true,"no_external_effects":true},
    })).await;
    let evidence=cross_step(&store,B,"b-private","cross-evidence","evidence.submit",json!({
        "session_id":session,"expected_session_version":"1","execution_id":intent["execution_id"],"input_digest":INPUT,
        "dirty_tree":false,"artifact_text":"cross integrated artifact","payload":{"passed":true,"output_digest":output},
    })).await;
    let round=cross_step(&store,RUNNER,"b-private","cross-review-open","review.open",json!({
        "session_id":finalizer_session,"expected_session_version":"1","evidence_id":evidence["evidence_id"],
    })).await;
    let mut decision = args(&round);
    decision["session_id"] = reviewer_session;
    cross_step(
        &store,
        REVIEWER_TOKEN,
        "b-private",
        "cross-review-decision",
        "review.decide",
        decision,
    )
    .await;
    revoke_adoption_export(&store, &replacement, "cross-final-revoke").await;
    let mut complete = completion_args(&evidence);
    complete["session_id"] = finalizer_session;
    let p = prepare(&store, RUNNER, "b-private").await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                RUNNER,
                command(
                    &p,
                    "invalid-cross-complete",
                    "delivery.finalize",
                    complete.clone()
                )
            )
            .await,
        Err(PgError::CompletionRejected)
    ));
    let final_export = publish_export(&store, &consumer, &upstream, "cross-final-export").await;
    adopt_export(&store, &session, &final_export, "cross-final-adoption").await;
    let admission = admin.query_one(
        "SELECT id,result_json FROM awr_team.operations WHERE op='execution.start' AND result_json->'data'->>'execution_id'=$1",
        &[&intent["execution_id"].as_str().unwrap()],
    ).await.unwrap();
    let admission_id: String = admission.get(0);
    let original_admission: Value = admission.get(1);
    for (case, receipts) in [
        ("missing", Value::Null),
        ("different", json!([["a", "another-completion-receipt"]])),
    ] {
        let mut changed = original_admission.clone();
        changed["data"]["dependency_receipts"] = receipts;
        admin
            .execute(
                "UPDATE awr_team.operations SET result_json=$1 WHERE id=$2",
                &[&changed, &admission_id],
            )
            .await
            .unwrap();
        let p = prepare(&store, RUNNER, "b-private").await;
        assert!(
            matches!(
                cross_execute(
                    &store,
                    RUNNER,
                    command(
                        &p,
                        &format!("reject-{case}-execution-input"),
                        "delivery.finalize",
                        complete.clone()
                    )
                )
                .await,
                Err(PgError::EvidenceInvalid | PgError::CompletionRejected)
            ),
            "A current adoption cannot relabel an execution whose original input is {case}"
        );
    }
    admin
        .execute(
            "UPDATE awr_team.operations SET result_json=$1 WHERE id=$2",
            &[&original_admission, &admission_id],
        )
        .await
        .unwrap();
    let finalized = cross_step(
        &store,
        RUNNER,
        "b-private",
        "cross-finalize",
        "delivery.finalize",
        complete,
    )
    .await;
    assert_eq!(finalized["human_approval"], false);
    assert_eq!(finalized["team_independent_acceptance"], false);
    let links=admin.query("SELECT predecessor_work_id,predecessor_completion_id FROM awr_team.completion_dependencies WHERE completion_id=$1",&[&finalized["receipt_id"].as_str().unwrap()]).await.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].get::<_, String>(0), "a");
    assert_eq!(
        links[0].get::<_, String>(1),
        upstream["receipt_id"].as_str().unwrap()
    );
}

// The source is generated from the actual current contracts, then registered
// and activated normally before executing the member workflow. No journal or
// affected set is planted by this fixture.
struct PendingPlanningSource { root: std::path::PathBuf, source: awr_team_pg::SourceStore }
impl Drop for PendingPlanningSource {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); }
}
async fn register_member_planning_source(admin:&Client,db:&str)->PendingPlanningSource {
    let bundle=history_source_bundle(admin).await;
    let root=std::env::temp_dir().join(format!("awr-dependency-writeback-{}",ulid::Ulid::new()));
    std::fs::create_dir(&root).unwrap();
    let root=std::fs::canonicalize(root).unwrap();
    let keys:std::collections::BTreeMap<_,_>=bundle.catalog.workstreams.iter().map(|s|(s.id,s.external_key.clone())).collect();
    let tasks:Vec<Value>=bundle.contracts.iter().map(|e| {
        let c=&e.contract;
        let mut task=json!({"id":c.external_key,"title":c.external_key,"status":"planned","workstream":keys[&e.workstream_id],
            "goals":c.goals,"paths":c.scope_paths,"acceptance":c.acceptance,"depends_on":c.required_dependencies,
            "completion_policy":c.completion_policy,"hard_rules":c.hard_rules,"verification_requirements":c.verification_requirements});
        if !c.dependency_acceptance.is_empty() {task["dependency_acceptance"]=json!(c.dependency_acceptance);}
        if let Some(policy)=&c.execution_settlement {task["execution_settlement"]=json!(policy);}
        task
    }).collect();
    let goals:std::collections::BTreeSet<_>=bundle.contracts.iter().flat_map(|e|e.contract.goals.iter().cloned())
        .chain(bundle.catalog.workstreams.iter().flat_map(|s|s.goal_keys.iter().cloned())).collect();
    let ledger=json!({"workstreams":{"version":bundle.catalog.version,"definitions":bundle.catalog.workstreams},
        "goals":goals.iter().map(|key|json!({"id":key,"title":key,"status":"active"})).collect::<Vec<_>>(),"work_items":tasks});
    let bytes=serde_json::to_vec(&ledger).unwrap();
    std::fs::write(root.join("ledger.yaml"),&bytes).unwrap();
    let location=awr_source::SoleSourceLocation::server_directory(&root,"ledger.yaml").unwrap();
    let package=awr_source::prepare_publish_from_ledger_bytes(&location,&root,&bytes,PROJECT,&awr_source::PublishPrepOptions::default()).unwrap();
    let source=awr_team_pg::SourceStore::from_config(common::with_app_role(&common::test_config(),db));
    let (candidate,_)=source.ingest_publish_candidate(awr_team_pg::IngestRequest {
        tenant_id:TENANT.into(),project_id:PROJECT.into(),actor_id:"agent".into(),parser_version:package.parser_version,
        files:package.files.into_iter().map(|f|awr_team_pg::SourceFile {path:f.path,bytes:f.bytes}).collect(),
    }).await.unwrap();
    source.approve(TENANT,PROJECT,&candidate.proposal_id,"runner",&candidate.manifest_digest).await.unwrap();
    source.activate_workstreams(TENANT,PROJECT,"runner",&candidate.proposal_id,&awr_team::SourceActivationPlan {
        candidate_digest:candidate.manifest_digest.clone(),approved_candidate_digest:candidate.manifest_digest,
        parser_version:candidate.parser_version,expected_authority_epoch:candidate.base_epoch,
    }).await.unwrap();
    PendingPlanningSource {root,source}
}
async fn stage_member_source_edit(admin:&Client,source:&awr_team_pg::SourceStore,work:&str)->String {
    let contract:WorkContract=serde_json::from_value(admin.query_one(
        "SELECT c.contract_json FROM awr_team.work_contracts c JOIN awr_team.projects p
         ON p.tenant_id=c.tenant_id AND p.id=c.project_id AND p.active_snapshot_id=c.snapshot_id
         WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$3", &[&TENANT,&PROJECT,&work],
    ).await.unwrap().get(0)).unwrap();
    let stream=if work=="b-private" {"private-beta"} else {"alpha"};
    let before=awr_team::TaskDraft {work_id:work.into(),external_key:work.into(),title:work.into(),goals:contract.goals.clone(),
        scope_paths:contract.scope_paths.clone(),acceptance:contract.acceptance.clone(),required_dependencies:contract.required_dependencies.clone(),
        completion_policy:contract.completion_policy.clone(),dependency_acceptance:if contract.dependency_acceptance.is_empty() {None} else {Some(contract.dependency_acceptance.clone())},
        hard_rules:Some(contract.hard_rules.clone()),verification_requirements:Some(contract.verification_requirements.clone()),
        execution_settlement:contract.execution_settlement.clone(),definition_state:awr_team::DraftDefinitionState::Enabled,
        workstream:Some(stream.into()),split_from:None,split_children:vec![]};
    let mut after=before.clone();after.scope_paths=vec!["src/revised".into()];
    let created=source.create_planning_candidate(TENANT,PROJECT,RUNNER,&awr_team_pg::DraftCandidateCreate {
        changes:vec![awr_team::DraftChange {op:awr_team::DraftOpKind::EditFields,before:Some(before),after}],
        suggestion_ids:vec![],allowed_spec_roots:vec!["src".into()],project_goal_keys:contract.goals,
        self_approve_policy:Some(awr_team::OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id:Some("runner".into()),predetermined_candidate_id:None,
    }).await.unwrap();
    let id=created["candidate_id"].as_str().unwrap();let digest=created["candidate_digest"].as_str().unwrap();
    source.approve_planning_candidate(TENANT,PROJECT,RUNNER,id,digest,Some("runner")).await.unwrap();
    source.publish_planning_candidate(TENANT,PROJECT,RUNNER,id,digest).await.unwrap()["receipt_id"].as_str().unwrap().into()
}
async fn dependency_selection_facts(admin:&Client)->Value {
    admin.query_one("SELECT jsonb_build_object(
        'adoptions',(SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY id),'[]') FROM awr_team.workstream_artifact_adoptions a),
        'exports',(SELECT coalesce(jsonb_agg(to_jsonb(e) ORDER BY id),'[]') FROM awr_team.workstream_artifact_exports e))",&[])
        .await.unwrap().get(0)
}

#[tokio::test]
async fn actual_pending_source_intent_fences_dependency_mutation_and_preserves_unrelated_adoption() {
    for (fixed,selected,related) in [(false,false,true),(false,true,true),(true,true,true),(false,false,false)] {
        let (_g,admin,db,store)=setup().await;
        seed_simulated_members(&admin,&db).await;
        if fixed {fixed_export_consumer(&admin,&store,&db).await;} else {exported_consumer(&admin,&store,&db).await;}
        adoption_roles(&admin).await;
        let bound=register_member_planning_source(&admin,&db).await;
        let upstream=complete_simulated_upstream(&store).await;
        let consumer:WorkContract=serde_json::from_value(admin.query_one("SELECT c.contract_json FROM awr_team.work_contracts c JOIN awr_team.projects p ON p.tenant_id=c.tenant_id AND p.id=c.project_id AND p.active_snapshot_id=c.snapshot_id WHERE c.work_id='b-private'",&[]).await.unwrap().get(0)).unwrap();
        let session=adoption_session(&store,"pending-consumer-session").await;
        let export=publish_export(&store,&consumer,&upstream,"pending-export").await;
        if selected {adopt_export(&store,&session,&export,"before-pending-adoption").await;}
        let receipt=stage_member_source_edit(&admin,&bound.source,if related {"a"} else {"c"}).await;
        let req=awr_team_pg::WritebackActivateRequest {request_id:"dependency-gap".into(),publish_receipt_id:receipt,
            source_root:bound.root.clone(),ledger_relative_path:"ledger.yaml".into(),impact_proven:true,stopped_work_ids:vec![]};
        let interrupted=bound.source.activate_planning_writeback_abort_for_test(TENANT,PROJECT,RUNNER,&req,"after_source_written").await.unwrap_err();
        assert!(matches!(interrupted,PgError::Protocol(ref text) if text.contains("injected")),"{interrupted:?}");
        let intent:Value=admin.query_one("SELECT intent_json FROM awr_team.planning_writeback_journals WHERE request_id='dependency-gap'",&[]).await.unwrap().get(0);
        assert_eq!(intent["dependency_work_ids"].as_array().unwrap().iter().any(|id|id=="b-private"),related);
        if fixed && selected {assert!(!intent["affected_work_ids"].as_array().unwrap().iter().any(|id|id=="b-private"),"fixed adoption must cut execution impact while its metadata remains fenced");}
        let before=dependency_selection_facts(&admin).await;
        let adoption=cross_execute(&store,B,adoption_request(&store,&session,&export,"during-pending-adoption").await).await;
        if related && !selected {
            assert!(matches!(adoption,Err(PgError::ActionBlockedByInvalidation(_))),"{adoption:?}");
        } else {
            assert_eq!(adoption.unwrap()["receipt"]["data"]["already_adopted"],selected);
        }
        let p=prepare(&store,RUNNER,"a").await;
        let revoke=command(&p,"during-pending-revoke","delivery.export.revoke",json!({
            "session_id":"session-runner","expected_session_version":"1","export_id":export["receipt"]["data"]["export_id"],
            "expected_export_version":export["receipt"]["data"]["export_version"]}));
        let revoked=cross_execute(&store,RUNNER,revoke).await;
        if related {
            assert!(matches!(revoked,Err(PgError::ActionBlockedByInvalidation(_))),"{revoked:?}");
            assert_eq!(dependency_selection_facts(&admin).await,before);
        } else {assert!(revoked.is_ok(),"{revoked:?}");}
        let restarted=awr_team_pg::SourceStore::from_config(common::with_app_role(&common::test_config(),&db));
        restarted.activate_planning_writeback(TENANT,PROJECT,RUNNER,&req).await.unwrap();
        assert_eq!(restarted.get_planning_writeback_status(TENANT,PROJECT,RUNNER,&req.request_id).await.unwrap().unwrap()["phase"],"completed");
    }
}
