//! Synthetic lifecycle writeback, recovery and source reconstruction regressions.
use super::*;

#[tokio::test]
async fn definition_archive_definition_visible_and_retained() {
    use awr_team_pg::WorkstreamReadStore;
    let (_g, _admin, db, store) = store_and_roles().await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let tmp = tempfile_ledger();
    let mut original = draft("DEFINITION-ARCHIVE", &[], DraftDefinitionState::Enabled);
    original.workstream = Some("alpha".into());
    let created = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: original.clone(),
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &created).await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "definition-create"),
        )
        .await
        .unwrap();
    let mut after = original.clone();
    after.definition_state = DraftDefinitionState::Archived;
    let candidate = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::Archive,
            before: Some(original),
            after,
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    let req = workspace_activation(&tmp, receipt, "definition-archive");
    let result = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(result["source_writeback_pending"], false);
    assert_eq!(
        prepare(&read, A, "DEFINITION-ARCHIVE").await["data"]["definition_state"],
        "archived",
        "successful publication and activation must expose the approved archive"
    );
    store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    let mut unrelated = draft("DEFINITION-UNRELATED", &[], DraftDefinitionState::Enabled);
    unrelated.workstream = Some("alpha".into());
    let candidate = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: unrelated,
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "definition-unrelated"),
        )
        .await
        .unwrap();
    assert_eq!(
        prepare(&read, A, "DEFINITION-ARCHIVE").await["data"]["definition_state"],
        "archived"
    );
}

#[tokio::test]
async fn definition_new_draft_definition_visible() {
    use awr_team_pg::WorkstreamReadStore;
    let (_g, _admin, db, store) = store_and_roles().await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let tmp = tempfile_ledger();
    let mut task = draft("DEFINITION-DRAFT", &[], DraftDefinitionState::Draft);
    task.workstream = Some("alpha".into());
    let candidate = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: task,
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "definition-draft"),
        )
        .await
        .unwrap();
    assert_eq!(
        prepare(&read, A, "DEFINITION-DRAFT").await["data"]["definition_state"],
        "draft"
    );
}

#[tokio::test]
async fn definition_title_persisted() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let tmp = tempfile_ledger();
    let mut task = draft("DEFINITION-TITLE", &[], DraftDefinitionState::Enabled);
    task.workstream = Some("alpha".into());
    task.title = "Approved human title".into();
    let candidate = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: task,
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "definition-title"),
        )
        .await
        .unwrap();
    let row = admin.query_one("SELECT c.title FROM awr_team.work_contracts c JOIN awr_team.projects p ON p.tenant_id=c.tenant_id AND p.id=c.project_id AND p.active_snapshot_id=c.snapshot_id WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id='DEFINITION-TITLE' AND c.scope_id='main'", &[&TENANT, &PROJECT]).await.unwrap();
    assert_eq!(row.get::<_, String>(0), "Approved human title");
}

#[tokio::test]
async fn definition_cancelled_definition_visible() {
    use awr_team_pg::WorkstreamReadStore;
    let (_g, _admin, db, store) = store_and_roles().await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let tmp = tempfile_ledger();
    let mut original = draft("DEFINITION-CANCEL", &[], DraftDefinitionState::Enabled);
    original.workstream = Some("alpha".into());
    let candidate = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: original.clone(),
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "definition-cancel-create"),
        )
        .await
        .unwrap();
    let mut after = original.clone();
    after.definition_state = DraftDefinitionState::Cancelled;
    let candidate = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::Cancel,
            before: Some(original),
            after,
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "definition-cancel"),
        )
        .await
        .unwrap();
    assert_eq!(
        prepare(&read, A, "DEFINITION-CANCEL").await["data"]["definition_state"],
        "cancelled"
    );
}

#[tokio::test]
async fn definition_archive_running_task_refused_before_source_write() {
    use awr_team_pg::WorkstreamReadStore;
    let (_g, _admin, db, store) = store_and_roles().await;
    let tmp = tempfile_ledger();
    register_actual_ledger_baseline(&store, &tmp).await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let claim = claim_scope(&read, A, "a", "session-a", "definition-live").await;
    let execution = read
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            scope_prepare_command(
                &read,
                A,
                "a",
                "session-a",
                "definition-live",
                "src/api",
                &claim,
            )
            .await,
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    read.commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            scope_start_command(
                &read,
                A,
                "a",
                "session-a",
                "definition-live",
                &claim,
                &execution,
            )
            .await,
        )
        .await
        .unwrap();
    let mut original = draft("a", &[], DraftDefinitionState::Enabled);
    original.title = "Alpha work".into();
    original.workstream = Some("alpha".into());
    original.goals = vec!["alpha".into()];
    original.scope_paths = vec!["src".into()];
    original.acceptance = vec!["verified".into()];
    let mut after = original.clone();
    after.definition_state = DraftDefinitionState::Archived;
    let candidate = store
        .create_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &DraftCandidateCreate {
                changes: vec![DraftChange {
                    op: DraftOpKind::Archive,
                    before: Some(original),
                    after,
                }],
                suggestion_ids: vec![],
                allowed_spec_roots: vec!["src".into(), "specs".into()],
                project_goal_keys: vec!["alpha".into()],
                self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
                author_person_id: Some("agent".into()),
                predetermined_candidate_id: None,
            },
        )
        .await
        .unwrap();
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    let before = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let result = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "definition-live-archive"),
        )
        .await;
    assert!(
        result.is_err(),
        "a definition-only archive must not bypass settlement: {result:?}"
    );
    assert_eq!(std::fs::read(tmp.root.join("ledger.yaml")).unwrap(), before);
}

#[tokio::test]
async fn definition_historical_rows_upgrade_to_cancelled_without_changing_existing_definitions() {
    let (_g, admin, _db, _store) = store_and_roles().await;
    common::historical_team_schema_preserving_rows(&admin, 58).await;
    let before = admin
        .query_one(
            "SELECT title,definition_state FROM awr_team.work_contracts WHERE work_id='a'",
            &[],
        )
        .await
        .unwrap();
    let error = admin
        .execute(
            "UPDATE awr_team.work_contracts SET definition_state='cancelled' WHERE work_id='a'",
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.as_db_error().unwrap().code(),
        &tokio_postgres::error::SqlState::CHECK_VIOLATION
    );
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::migrate(&admin).await.unwrap();
    let after = admin
        .query_one(
            "SELECT title,definition_state FROM awr_team.work_contracts WHERE work_id='a'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(after.get::<_, String>(0), before.get::<_, String>(0));
    assert_eq!(after.get::<_, String>(1), before.get::<_, String>(1));
    assert_eq!(
        admin
            .execute(
                "UPDATE awr_team.work_contracts SET definition_state='cancelled' WHERE work_id='a'",
                &[]
            )
            .await
            .unwrap(),
        1
    );
}

async fn publish_definition_change(
    store: &SourceStore,
    admin: &tokio_postgres::Client,
    target: DraftDefinitionState,
    title: &str,
) -> String {
    let contract = activated_contract(admin, "a").await;
    let mut before = draft("a", &[], DraftDefinitionState::Enabled);
    before.title = "Alpha work".into();
    before.goals = contract.goals;
    before.scope_paths = contract.scope_paths;
    before.acceptance = contract.acceptance;
    before.completion_policy = contract.completion_policy;
    before.workstream = Some("alpha".into());
    let mut after = before.clone();
    after.definition_state = target;
    after.title = title.into();
    let candidate = store
        .create_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &DraftCandidateCreate {
                changes: vec![DraftChange {
                    op: match target {
                        DraftDefinitionState::Archived => DraftOpKind::Archive,
                        DraftDefinitionState::Cancelled => DraftOpKind::Cancel,
                        _ => DraftOpKind::EditFields,
                    },
                    before: Some(before),
                    after,
                }],
                suggestion_ids: vec![],
                allowed_spec_roots: vec!["src".into()],
                project_goal_keys: vec!["alpha".into(), "private-beta".into()],
                self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
                author_person_id: Some("agent".into()),
                predetermined_candidate_id: None,
            },
        )
        .await
        .unwrap();
    approve_workspace_candidate(store, &candidate).await
}

async fn assert_title_update_preserves_execution(running: bool) {
    use awr_team_pg::WorkstreamReadStore;
    let (_g, admin, db, store) = store_and_roles().await;
    let tmp = tempfile_ledger();
    register_actual_ledger_baseline(&store, &tmp).await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let (claim, execution) =
        claim_and_prepare_scope(&read, A, "a", "session-a", "title-edit", "src/api").await;
    if running {
        read.commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                scope_start_command(&read, A, "a", "session-a", "title-edit", &claim, &execution)
                    .await,
            )
            .await
            .unwrap();
    }
    let before = prepare(&read, A, "a").await;
    let version_before: i64 = admin
        .query_one(
            "SELECT execution_version FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &execution["execution_id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    let receipt = publish_definition_change(
        &store,
        &admin,
        DraftDefinitionState::Enabled,
        "Corrected human title",
    )
    .await;
    let result = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "title-edit-publish"),
        )
        .await
        .unwrap();
    assert_eq!(result["affected_work_ids"], serde_json::json!([]));
    let after = prepare(&read, A, "a").await;
    assert_eq!(after["data"]["title"], "Corrected human title");
    assert_eq!(
        after["data"]["contract_hash"],
        before["data"]["contract_hash"]
    );
    assert_eq!(
        after["data"]["runtime"]["work_version"],
        before["data"]["runtime"]["work_version"]
    );
    let row = admin
        .query_one(
            "SELECT state,execution_version FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &execution["execution_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(
        row.get::<_, String>(0),
        if running { "running" } else { "prepared" }
    );
    assert_eq!(row.get::<_, i64>(1), version_before);
    if !running {
        read.commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                scope_start_command(&read, A, "a", "session-a", "title-edit", &claim, &execution)
                    .await,
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn title_only_writeback_preserves_prepared_execution() {
    assert_title_update_preserves_execution(false).await;
}

#[tokio::test]
async fn title_only_writeback_preserves_running_execution() {
    assert_title_update_preserves_execution(true).await;
}

#[tokio::test]
async fn definition_writeback_refuses_newer_source_state_before_mutation() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let tmp = tempfile_ledger();
    register_actual_ledger_baseline(&store, &tmp).await;
    let receipt = publish_definition_change(
        &store,
        &admin,
        DraftDefinitionState::Enabled,
        "Corrected human title",
    )
    .await;
    let path = tmp.root.join("ledger.yaml");
    let before = std::fs::read_to_string(&path).unwrap();
    let archived = before.replacen(
        "    title: Alpha work\n",
        "    title: Alpha work\n    definition_state: archived\n",
        1,
    );
    assert_ne!(before, archived);
    std::fs::write(&path, &archived).unwrap();
    let facts = activation_facts(&admin).await;
    let result = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "title-edit-stale-source"),
        )
        .await;
    assert!(
        matches!(result, Err(PgError::Protocol(ref text)) if text.contains("definition_state")),
        "a title edit must refuse the stale definition state: {result:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), archived);
    assert_eq!(activation_facts(&admin).await, facts);
}

#[tokio::test]
async fn definition_only_pending_intent_fences_execution_and_recovers_each_commit_boundary() {
    use awr_team_pg::WorkstreamReadStore;
    use serde_json::json;
    for boundary in [
        "after_intent",
        "after_source_write",
        "after_source_written",
        "after_pg_activating",
        "before_final_commit",
        "after_final_commit",
    ] {
        let (_g, admin, db, store) = store_and_roles().await;
        let tmp = tempfile_ledger();
        register_actual_ledger_baseline(&store, &tmp).await;
        admin.batch_execute("UPDATE awr_team.workstream_grants SET can_write=true,grant_version=grant_version+1 WHERE client_id='cli-b'").await.unwrap();
        let read =
            WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
        let claim = claim_scope(&read, A, "a", "session-a", "definition-boundary").await;
        let prepared = read
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                scope_prepare_command(
                    &read,
                    A,
                    "a",
                    "session-a",
                    "definition-before",
                    "src/api",
                    &claim,
                )
                .await,
            )
            .await
            .unwrap()["receipt"]["data"]
            .clone();
        let publication = publish_definition_change(
            &store,
            &admin,
            DraftDefinitionState::Cancelled,
            "Cancelled source task",
        )
        .await;
        let req = workspace_activation(&tmp, publication, boundary);
        let baseline = activation_facts(&admin).await;
        let error = store
            .activate_planning_writeback_abort_for_test(TENANT, PROJECT, A, &req, boundary)
            .await
            .unwrap_err();
        assert!(
            matches!(error, PgError::Protocol(ref text) if text.contains("injected writeback interruption")),
            "{boundary}: {error:?}"
        );
        let intent: serde_json::Value = admin
            .query_one(
                "SELECT intent_json FROM awr_team.planning_writeback_journals WHERE request_id=$1",
                &[&req.request_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(intent["affected_work_ids"], json!(["a"]));
        assert_eq!(intent["dependency_work_ids"], json!(["a"]));
        let current = prepare(&read, A, "a").await;
        let committed = boundary == "after_final_commit";
        assert_eq!(
            current["data"]["definition_state"],
            if committed { "cancelled" } else { "enabled" }
        );
        assert_eq!(
            current["data"]["title"],
            if committed {
                "Cancelled source task"
            } else {
                "Alpha work"
            }
        );
        if !committed {
            let error = read
                .commands()
                .execute(
                    TENANT,
                    PROJECT,
                    A,
                    scope_start_command(
                        &read,
                        A,
                        "a",
                        "session-a",
                        "definition-blocked",
                        &claim,
                        &prepared,
                    )
                    .await,
                )
                .await
                .unwrap_err();
            assert!(
                matches!(error, PgError::ActionBlockedByInvalidation(_)),
                "{boundary}: {error:?}"
            );
            assert_eq!(activation_facts(&admin).await, baseline);
            let mut competing = req.clone();
            competing.request_id = "definition-competing".into();
            assert!(matches!(
                store
                    .activate_planning_writeback(TENANT, PROJECT, A, &competing)
                    .await,
                Err(PgError::WritebackRefused(_))
            ));
        }
        let (claim_b, execution_b) = claim_and_prepare_scope(
            &read,
            B,
            "b-private",
            "session-b",
            "definition-unrelated",
            "src/other",
        )
        .await;
        read.commands()
            .execute(
                TENANT,
                PROJECT,
                B,
                scope_start_command(
                    &read,
                    B,
                    "b-private",
                    "session-b",
                    "definition-unrelated",
                    &claim_b,
                    &execution_b,
                )
                .await,
            )
            .await
            .unwrap();
        let restarted =
            SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
        restarted
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap();
        let final_facts = activation_facts(&admin).await;
        assert_eq!(
            (final_facts.1, final_facts.2, final_facts.3),
            (baseline.1 + 1, baseline.2 + 1, baseline.3 + 1)
        );
        let bytes = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
        let replay = restarted
            .activate_planning_writeback(TENANT, PROJECT, A, &req)
            .await
            .unwrap();
        assert_eq!(replay["already_recorded"], true);
        assert_eq!(std::fs::read(tmp.root.join("ledger.yaml")).unwrap(), bytes);
        assert_eq!(activation_facts(&admin).await, final_facts);
        let current = prepare(&read, A, "a").await;
        assert_eq!(current["data"]["definition_state"], "cancelled");
        assert_eq!(current["data"]["title"], "Cancelled source task");
        let error = read.commands().execute(TENANT, PROJECT, A,
            command(&current, "cancelled-claim", "claim.acquire", json!({
                "session_id":"session-a", "expected_session_version":"1",
                "expected_work_version":current["data"]["runtime"]["work_version"], "ttl_seconds":600
            }))
        ).await.unwrap_err();
        assert!(matches!(error, PgError::PreconditionsChanged), "{error:?}");
        let error = read
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                scope_prepare_command(
                    &read,
                    A,
                    "a",
                    "session-a",
                    "cancelled-prepare",
                    "src/api",
                    &claim,
                )
                .await,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, PgError::PreconditionsChanged), "{error:?}");
        let navigation = read
            .query(TENANT, PROJECT, A, query("work.next"))
            .await
            .unwrap();
        assert!(
            navigation["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["work_id"] != "a")
        );
        assert!(
            navigation["data"]["resume"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["work_id"] != "a")
        );
        let mut historical = query("work.observe");
        historical.work_id = Some("a".into());
        historical.session_id = Some("session-a".into());
        let observed = read.query(TENANT, PROJECT, A, historical).await.unwrap();
        assert_eq!(observed["data"]["definition_state"], "cancelled");
        let state: String = admin
            .query_one(
                "SELECT state FROM awr_team.executions WHERE id=$1",
                &[&prepared["execution_id"].as_str().unwrap()],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "cancelled");
        let state: String = admin
            .query_one(
                "SELECT state FROM awr_team.executions WHERE id=$1",
                &[&execution_b["execution_id"].as_str().unwrap()],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "running");
        assert_eq!(
            admin
                .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            0
        );
    }
}

#[tokio::test]
async fn definition_states_and_titles_rebuild_from_source_after_projection_loss() {
    use awr_team_pg::WorkstreamReadStore;
    let tmp = tempfile_ledger();
    let states = [
        DraftDefinitionState::Draft,
        DraftDefinitionState::Enabled,
        DraftDefinitionState::Archived,
        DraftDefinitionState::Cancelled,
    ];
    {
        let (_g, _admin, _db, store) = store_and_roles().await;
        for state in states {
            let key = format!("SOURCE-{}", state.as_str());
            let mut task = draft(&key, &[], state);
            task.workstream = Some("alpha".into());
            task.title = format!("Human title {}", state.as_str());
            let candidate = create_workspace_candidate(
                &store,
                DraftChange {
                    op: DraftOpKind::CreateTask,
                    before: None,
                    after: task,
                },
            )
            .await;
            let receipt = approve_workspace_candidate(&store, &candidate).await;
            store
                .activate_planning_writeback(
                    TENANT,
                    PROJECT,
                    A,
                    &workspace_activation(&tmp, receipt, &key),
                )
                .await
                .unwrap();
        }
    }
    // A genuinely new schema: no prior definitions are available to inherit.
    let (_g, admin, db, store) = store_and_roles().await;
    register_actual_ledger_baseline(&store, &tmp).await;
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let historical_done = prepare(&read, A, "API-1").await;
    assert_eq!(historical_done["data"]["definition_state"], "enabled");
    assert!(historical_done["data"]["runtime"].is_null());
    for state in states {
        let key = format!("SOURCE-{}", state.as_str());
        let result = prepare(&read, A, &key).await;
        assert_eq!(result["data"]["definition_state"], state.as_str());
        assert_eq!(
            result["data"]["title"],
            format!("Human title {}", state.as_str())
        );
        assert!(result["data"]["runtime"].is_null());
        if state != DraftDefinitionState::Enabled {
            assert!(
                result["data"]["completeness_reasons"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("work_definition_inactive"))
            );
            let guidance_op = result["data"]["guidance"]["action"]["op"].as_str().unwrap();
            assert_eq!(guidance_op, "work.next");
            let next = read
                .query(TENANT, PROJECT, A, query(guidance_op))
                .await
                .unwrap();
            assert!(
                next["data"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|item| item["definition_state"] == "enabled" && item["work_id"] != key)
            );
            let enabled = next["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["work_id"] == "SOURCE-enabled")
                .expect("rebuilt enabled source task must remain discoverable");
            let selected = read
                .query(
                    TENANT,
                    PROJECT,
                    A,
                    serde_json::from_value(enabled["next_query"].clone()).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(selected["data"]["work_id"], "SOURCE-enabled");
            assert_eq!(selected["data"]["definition_state"], "enabled");
        }
    }
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.completion_receipts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn definition_change_stale_approval_refuses_before_source_mutation() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let tmp = tempfile_ledger();
    register_actual_ledger_baseline(&store, &tmp).await;
    let stale = publish_definition_change(
        &store,
        &admin,
        DraftDefinitionState::Archived,
        "Old approved title",
    )
    .await;
    let fresh = publish_definition_change(
        &store,
        &admin,
        DraftDefinitionState::Enabled,
        "New approved title",
    )
    .await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, fresh, "definition-fresh"),
        )
        .await
        .unwrap();
    let bytes = std::fs::read(tmp.root.join("ledger.yaml")).unwrap();
    let facts = activation_facts(&admin).await;
    assert!(
        store
            .activate_planning_writeback(
                TENANT,
                PROJECT,
                A,
                &workspace_activation(&tmp, stale, "definition-stale")
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(tmp.root.join("ledger.yaml")).unwrap(), bytes);
    assert_eq!(activation_facts(&admin).await, facts);
}

#[tokio::test]
async fn definition_edit_enables_a_draft_and_returns_its_approved_title_after_reload() {
    use awr_team_pg::WorkstreamReadStore;
    let (_g, _admin, db, store) = store_and_roles().await;
    let tmp = tempfile_ledger();
    let mut task = draft("EDITED-DRAFT", &[], DraftDefinitionState::Draft);
    task.workstream = Some("alpha".into());
    let candidate = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: task.clone(),
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "create-edited-draft"),
        )
        .await
        .unwrap();
    let mut after = task.clone();
    after.definition_state = DraftDefinitionState::Enabled;
    after.title = "Approved enabled title".into();
    let candidate = create_workspace_candidate(
        &store,
        DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(task),
            after,
        },
    )
    .await;
    let receipt = approve_workspace_candidate(&store, &candidate).await;
    let restarted = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    restarted
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &workspace_activation(&tmp, receipt, "enable-edited-draft"),
        )
        .await
        .unwrap();
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let result = prepare(&read, A, "EDITED-DRAFT").await;
    assert_eq!(result["data"]["definition_state"], "enabled");
    assert_eq!(result["data"]["title"], "Approved enabled title");
    assert!(result["data"]["runtime"].is_null());
}
