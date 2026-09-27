#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
#[path = "fixtures/scoped_runner.rs"]
mod runner_fixture;
use awr_team_pg::{PgError, ReferenceReportRequest, ScopedReferenceRunner};
use fixture::*;
use runner_fixture::*;
use serde_json::json;

#[tokio::test]
async fn actual_artifact_is_attested_without_completing_work_and_replay_cannot_overwrite_it() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let req = request(&store, writes()).await;
    let result = runner.run(A, req.clone()).await.unwrap();
    assert_eq!(result["replayed"], false);
    assert_eq!(result["report_required"], false);
    assert_eq!(result["execution_authorized"], false);
    assert_eq!(result["outcome"]["state"], "succeeded");
    assert_eq!(result["outcome"]["exactly_once_supported"], false);
    let root = runner.project_root(TENANT, PROJECT).unwrap();
    let path = root.join("worktree/src/api/result.txt");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "verified artifact\n"
    );
    let receipt = admin
        .query_one(
            "SELECT receipt_kind,payload_json FROM awr_team.execution_receipts",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(receipt.get::<_, String>(0), "trusted_executor");
    let r = admin
        .query_one(
            "SELECT state,selected_completion_id FROM awr_team.work_runtime WHERE work_id='a'",
            &[],
        )
        .await
        .unwrap();
    assert_ne!(r.get::<_, String>(0), "completed");
    assert!(r.get::<_, Option<String>>(1).is_none());
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "released"
    );
    // Retrying the original request does not run or re-attest, even after the
    // artifact has independently changed or the lease has expired.
    std::fs::write(&path, "later user edit").unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let replay = runner.run(A, req).await.unwrap();
    assert_eq!(replay["replayed"], true);
    assert!(replay["report_required"].is_null());
    assert_eq!(replay["report_request_file"], result["report_request_file"]);
    assert_eq!(replay["effects_attempted"], false);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "later user edit");
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.execution_receipts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn concurrent_duplicate_has_exactly_one_effect_owner() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let req = request(&store, writes()).await;
    let (a, b) = tokio::join!(runner.run(A, req.clone()), runner.run(A, req));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a["effects_attempted"], b["effects_attempted"]);
    assert_ne!(a["replayed"], b["replayed"]);
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.execution_receipts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn ordinary_identity_wrong_digest_and_revoked_identity_cannot_admit_effects() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let req = request(&store, writes()).await;
    assert!(matches!(
        runner.run(A, req.clone()).await,
        Err(PgError::Forbidden)
    ));
    trust(&admin).await;
    let mut changed = req.clone();
    changed.plan.writes[0].content = "changed".into();
    assert!(matches!(
        runner.run(A, changed.clone()).await,
        Err(PgError::Protocol(_))
    ));
    // Updating only the claimed hash cannot change the prepared server input.
    changed.command.args["expected_input_digest"] = json!(changed.plan.digest().unwrap());
    assert!(matches!(
        runner.run(A, changed).await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    assert!(matches!(runner.run(A, req).await, Err(PgError::Forbidden)));
    assert!(!dir.0.exists());
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.executions", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "prepared"
    );
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn lost_admission_response_never_restarts_even_with_an_empty_local_directory() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let req = request(&store, writes()).await;
    let committed = store
        .commands()
        .execute(TENANT, PROJECT, A, req.command.clone())
        .await
        .unwrap();
    assert_eq!(committed["execution_authorized"], true);
    // Simulate a process lost after commit, before it observed the reply.
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let result = runner.run(A, req).await.unwrap();
    assert_eq!(result["replayed"], true);
    assert_eq!(result["effects_attempted"], false);
    assert!(!dir.0.exists());
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.executions", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "running"
    );
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "reserved"
    );
}

#[tokio::test]
async fn failed_local_journal_returns_committed_admission_without_permission_to_retry_effects() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let req = request(&store, writes()).await;
    let dir = Directory::new();
    std::fs::write(&dir.0, "not a runner directory").unwrap();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let result = runner.run(A, req.clone()).await.unwrap();
    assert_eq!(result["admission_receipt"]["data"]["state"], "running");
    assert_eq!(result["effects_attempted"], false);
    assert_eq!(result["execution_authorized"], false);
    assert_eq!(result["report_required"], true);
    std::fs::remove_file(&dir.0).unwrap();
    let retried = runner.run(A, req).await.unwrap();
    assert_eq!(retried["replayed"], true);
    assert_eq!(retried["effects_attempted"], false);
    assert!(!dir.0.exists());
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "reserved"
    );
}

#[tokio::test]
async fn failed_report_retries_only_saved_facts_and_keeps_resources_until_confirmation() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let req = request(&store, writes()).await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_runner_report() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type='execution.attest' THEN RAISE EXCEPTION 'synthetic report failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER reject_runner_report BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_runner_report()").await.unwrap();
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let result = runner.run(A, req).await.unwrap();
    assert_eq!(result["report_required"], true);
    assert_eq!(result["outcome"]["state"], "succeeded");
    let path = runner
        .project_root(TENANT, PROJECT)
        .unwrap()
        .join("worktree/src/api/result.txt");
    assert!(path.exists());
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "reserved"
    );
    let report: ReferenceReportRequest = serde_json::from_slice(
        &std::fs::read(result["report_request_file"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    admin.batch_execute("DROP TRIGGER reject_runner_report ON awr_team.events; DROP FUNCTION awr_team.reject_runner_report()").await.unwrap();
    let mut forged = report.clone();
    forged.command.args["facts"]["output_digest"] = json!("f".repeat(64));
    assert!(matches!(
        runner.report(A, forged).await,
        Err(PgError::Protocol(_))
    ));
    assert!(matches!(
        runner.report(B, report.clone()).await,
        Err(PgError::Forbidden)
    ));
    std::fs::write(&path, "later edit after execution").unwrap();
    let done = runner.report(A, report.clone()).await.unwrap();
    let replay = runner.report(A, report).await.unwrap();
    assert_eq!(done["receipt"], replay["receipt"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "later edit after execution"
    );
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "released"
    );
}

#[tokio::test]
async fn replay_recovers_report_from_native_journal_without_reexecuting_effects() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let req = request(&store, writes()).await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_recovered_report() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type='execution.attest' THEN RAISE EXCEPTION 'synthetic report failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER reject_recovered_report BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_recovered_report()")
        .await
        .unwrap();
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let first = runner.run(A, req.clone()).await.unwrap();
    assert_eq!(first["outcome"]["state"], "succeeded");
    let root = runner.project_root(TENANT, PROJECT).unwrap();
    let artifact = root.join("worktree/src/api/result.txt");
    std::fs::write(&artifact, "later user edit").unwrap();
    let execution = first["outcome"]["execution_id"].as_str().unwrap();
    let report_path = root.join("reports").join(format!("{execution}.json"));
    let expected: ReferenceReportRequest =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    std::fs::remove_file(root.join("observations").join(format!("{execution}.json"))).unwrap();
    std::fs::remove_file(report_path).unwrap();

    let replay = runner.run(A, req).await.unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["effects_attempted"], false);
    assert!(replay["report_required"].is_null());
    assert_eq!(
        std::fs::read_to_string(&artifact).unwrap(),
        "later user edit"
    );
    let report: ReferenceReportRequest = serde_json::from_slice(
        &std::fs::read(replay["report_request_file"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(report.command.args["facts"], expected.command.args["facts"]);
    admin.batch_execute("DROP TRIGGER reject_recovered_report ON awr_team.events; DROP FUNCTION awr_team.reject_recovered_report()")
        .await
        .unwrap();
    assert_eq!(
        runner.report(A, report).await.unwrap()["receipt"]["data"]["state"],
        "succeeded"
    );
}

#[tokio::test]
async fn stale_authority_or_expired_admission_writes_nothing() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let req = request(&store, writes()).await;
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let mut stale = req.clone();
    stale.command.expected_authority_version = "2".into();
    assert!(matches!(
        runner.run(A, stale).await,
        Err(PgError::PreconditionsChanged)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    assert!(runner.run(A, req).await.is_err());
    assert!(!dir.0.exists());
}

#[tokio::test]
async fn unrelated_audit_revision_change_preserves_report_and_idempotent_replay() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let req = request(&store, writes()).await;
    // Project revision is an audit cursor. Another task's progress after
    // admission must not invalidate this execution's unchanged read set.
    admin.batch_execute("CREATE FUNCTION awr_team.advance_after_start() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type='execution.start' THEN UPDATE awr_team.projects SET project_revision=project_revision+1
        WHERE tenant_id=NEW.tenant_id AND id=NEW.project_id; END IF; RETURN NEW; END $$;
        CREATE TRIGGER advance_after_start AFTER INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.advance_after_start()").await.unwrap();
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let result = runner.run(A, req.clone()).await.unwrap();
    assert_eq!(result["report_required"], false);
    assert_eq!(result["outcome"]["state"], "succeeded");
    let report: ReferenceReportRequest = serde_json::from_slice(
        &std::fs::read(result["report_request_file"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert_ne!(
        report.command.expected_project_revision,
        prepare(&store, A, "a").await["project_revision"]
            .as_str()
            .unwrap()
    );
    let artifact = runner
        .project_root(TENANT, PROJECT)
        .unwrap()
        .join("worktree/src/api/result.txt");
    std::fs::write(&artifact, "subsequent work").unwrap();
    let replayed_report = runner.report(A, report).await.unwrap();
    assert_eq!(replayed_report["replayed"], true);
    assert_eq!(replayed_report["receipt"]["data"]["state"], "succeeded");
    let replayed_run = runner.run(A, req).await.unwrap();
    assert_eq!(replayed_run["replayed"], true);
    assert_eq!(replayed_run["effects_attempted"], false);
    assert_eq!(
        std::fs::read_to_string(artifact).unwrap(),
        "subsequent work"
    );
}

#[tokio::test]
async fn revocation_after_admission_preserves_unreported_observation_and_held_resources() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let req = request(&store, writes()).await;
    admin.batch_execute("CREATE FUNCTION awr_team.revoke_after_start() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type='execution.start' THEN UPDATE awr_team.workstream_grants SET can_attest_execution=false,grant_version=grant_version+1
        WHERE tenant_id=NEW.tenant_id AND project_id=NEW.project_id AND client_id='cli-a'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER revoke_after_start AFTER INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.revoke_after_start()").await.unwrap();
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let result = runner.run(A, req.clone()).await.unwrap();
    assert_eq!(result["outcome"]["state"], "succeeded");
    assert_eq!(result["report_required"], true);
    let report: ReferenceReportRequest = serde_json::from_slice(
        &std::fs::read(result["report_request_file"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        runner.report(A, report).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(
        runner.run(A, req).await.unwrap()["effects_attempted"],
        false
    );
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM awr_team.execution_receipts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "reserved"
    );
}

#[tokio::test]
async fn scope_violation_remains_unknown_and_does_not_release_resources() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let mut plan = writes();
    plan[0].path = "src/private/file.txt".into();
    let req = request(&store, plan).await;
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let result = runner.run(A, req).await.unwrap();
    assert_eq!(result["outcome"]["scope_violation"], true);
    assert_eq!(result["report_required"], false);
    let row = admin
        .query_one("SELECT state FROM awr_team.executions", &[])
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "unknown");
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "unknown"
    );
    assert!(
        !runner
            .project_root(TENANT, PROJECT)
            .unwrap()
            .join("worktree/src/private/file.txt")
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn native_runner_refuses_symlink_escape_and_cannot_adopt_a_different_project_journal() {
    let (_g, admin, _, store) = setup().await;
    trust(&admin).await;
    let req = request(&store, writes()).await;
    let dir = Directory::new();
    let runner = ScopedReferenceRunner::new(store.commands(), &dir.0);
    let root = runner.project_root(TENANT, PROJECT).unwrap();
    let external = dir.0.join("unrelated");
    std::fs::create_dir_all(&external).unwrap();
    std::fs::create_dir_all(root.join("worktree/src")).unwrap();
    std::os::unix::fs::symlink(&external, root.join("worktree/src/api")).unwrap();
    let result = runner.run(A, req).await.unwrap();
    assert_eq!(result["outcome"]["scope_violation"], true);
    assert!(!external.join("result.txt").exists());
    let mut report: ReferenceReportRequest = serde_json::from_slice(
        &std::fs::read(result["report_request_file"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    report.project_id = "different-project".into();
    assert!(runner.report(A, report).await.is_err());
    assert_ne!(
        root,
        runner.project_root(TENANT, "different-project").unwrap()
    );
}
