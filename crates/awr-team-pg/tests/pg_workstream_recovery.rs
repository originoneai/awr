#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::{PgError, WorkstreamCommand, WorkstreamReadStore, workstream_credential_hash};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

const OP: &str = "awr1.operator.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
async fn operator(admin: &Client, store: &WorkstreamReadStore) -> String {
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
        VALUES('reader-tenant','operator','human','Recovery operator','active');
        INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role)
        VALUES('reader-tenant','reader-project','operator','admin')",
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
        VALUES($1,'operator','operator','operator-cli',$2)",
            &[&TENANT, &workstream_credential_hash(OP).unwrap()],
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,
        can_read,can_write,can_manage,can_reconcile_execution) VALUES($1,$2,'operator','operator-cli',$3,1,true,true,true,true)",
        &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    let r = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            OP,
            command(
                &prepare(store, OP, "a").await,
                "operator-session",
                "session.start",
                json!({"conversation_id":"recovery"}),
            ),
        )
        .await
        .unwrap();
    r["receipt"]["data"]["session_id"].as_str().unwrap().into()
}
async fn take(store: &WorkstreamReadStore) -> Value {
    store.commands().execute(TENANT,PROJECT,A,command(&prepare(store,A,"a").await,"take","claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":"0","ttl_seconds":600})))
        .await.unwrap()["receipt"]["data"].clone()
}
async fn start(store: &WorkstreamReadStore, claim: &Value, name: &str) -> Value {
    let p = prepare(store, A, "a").await;
    let r=store.commands().execute(TENANT,PROJECT,A,command(&p,&format!("prepare-{name}"),"execution.prepare",
        json!({"session_id":"session-a","expected_session_version":"1","claim_id":claim["claim_id"],
        "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":"a".repeat(64),"declared_scope":["src/api"]})))
        .await.unwrap();
    let p = prepare(store, A, "a").await;
    store.commands().execute(TENANT,PROJECT,A,command(&p,&format!("start-{name}"),"execution.start",
        json!({"session_id":"session-a","expected_session_version":"1","execution_id":r["receipt"]["data"]["execution_id"],
        "expected_execution_version":"1","claim_id":claim["claim_id"],"expected_fence":claim["fence"],
        "expected_lease_version":claim["lease_version"],"expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"caller_managed"})))
        .await.unwrap()["receipt"]["data"].clone()
}
fn facts(outcome: &str) -> Value {
    json!({"outcome":outcome,"input_digest":"a".repeat(64),"output_digest":"b".repeat(64),
        "environment_digest":"c".repeat(64),"observed_paths":["src/api/result.json"],"note":"Verified the actual process result and effects."})
}
async fn inspect(store: &WorkstreamReadStore, token: &str, e: &Value) -> Value {
    let mut q = query("execution.inspect");
    q.work_id = Some("a".into());
    q.execution_id = Some(e["execution_id"].as_str().unwrap().into());
    store.query(TENANT, PROJECT, token, q).await.unwrap()["data"].clone()
}
async fn report(store: &WorkstreamReadStore, e: &Value, name: &str) -> Value {
    store.commands().execute(TENANT,PROJECT,A,command(&prepare(store,A,"a").await,name,"execution.report",
        json!({"session_id":"session-a","expected_session_version":"1","execution_id":e["execution_id"],
        "expected_execution_version":e["execution_version"],"outcome":"succeeded","output_digest":"b".repeat(64),
        "observed_paths":["src/api/result.json"],"note":"Caller observation, awaiting verification."})))
        .await.unwrap()["receipt"]["data"].clone()
}
async fn attest(
    store: &WorkstreamReadStore,
    e: &Value,
    name: &str,
    outcome: &str,
) -> WorkstreamCommand {
    command(
        &prepare(store, A, "a").await,
        name,
        "execution.attest",
        json!({"session_id":"session-a",
        "expected_session_version":"1","execution_id":e["execution_id"],"expected_execution_version":e["execution_version"],"facts":facts(outcome)}),
    )
}
async fn reconcile(
    store: &WorkstreamReadStore,
    session: &str,
    e: &Value,
    name: &str,
    outcome: &str,
) -> WorkstreamCommand {
    let now = inspect(store, OP, e).await;
    let p = prepare(store, OP, "a").await;
    command(
        &p,
        name,
        "execution.reconcile",
        json!({"session_id":session,"expected_session_version":"1",
        "execution_id":e["execution_id"],"expected_execution_version":now["execution_version"],
        "expected_work_version":p["data"]["runtime"]["work_version"],
        "reviewed_receipt_id":now["latest_receipt"]["receipt_id"],"clear_recovery_block":true,"facts":facts(outcome)}),
    )
}
async fn snapshot(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'executions',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.executions e),
        'resources',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.resource_reservations r),
        'receipts',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM awr_team.execution_receipts r),
        'runtime',(SELECT jsonb_agg(to_jsonb(w) ORDER BY work_id) FROM awr_team.work_runtime w),
        'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM awr_team.events e),
        'operations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM awr_team.operations o),
        'revision',(SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2))",
        &[&TENANT,&PROJECT]).await.unwrap().get(0)
}
async fn trusted_runner(admin: &Client) {
    admin.batch_execute("UPDATE awr_team.actors SET kind='system' WHERE id='agent';
        UPDATE awr_team.workstream_grants SET can_attest_execution=true,grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
}

async fn controlled_start(store: &WorkstreamReadStore, claim: &Value) -> Value {
    let p = prepare(store, A, "a").await;
    let intent = store.commands().execute(TENANT,PROJECT,A,command(&p,"controlled-prepare","execution.prepare",
        json!({"session_id":"session-a","expected_session_version":"1","claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "expected_work_version":p["data"]["runtime"]["work_version"],"input_digest":"a".repeat(64),"declared_scope":["src/api"]})))
        .await.unwrap()["receipt"]["data"].clone();
    let p = prepare(store, A, "a").await;
    store.commands().execute(TENANT,PROJECT,A,command(&p,"controlled-start","execution.start",
        json!({"session_id":"session-a","expected_session_version":"1","execution_id":intent["execution_id"],
            "expected_execution_version":intent["execution_version"],"claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "expected_work_version":p["data"]["runtime"]["work_version"],"execution_mode":"reference_write_v1",
            "expected_input_digest":"a".repeat(64)}))).await.unwrap()["receipt"]["data"].clone()
}

async fn confirm(
    store: &WorkstreamReadStore,
    execution: &Value,
    name: &str,
    outcome: &str,
) -> WorkstreamCommand {
    let observed = inspect(store, A, execution).await;
    let mut cmd = attest(store, &observed, name, outcome).await;
    cmd.args["reviewed_receipt_id"] = observed["latest_receipt"]["receipt_id"].clone();
    cmd.args["facts"]["executor_stopped"] = json!(true);
    cmd
}

#[tokio::test]
async fn controlled_report_confirmation_clears_only_its_cause_and_replays_without_effects() {
    for outcome in ["succeeded", "failed", "cancelled"] {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        trusted_runner(&admin).await;
        let claim = take(&store).await;
        let execution = controlled_start(&store, &claim).await;
        let reported = report(&store, &execution, "report").await;
        let observed = inspect(&store, A, &reported).await;
        assert_eq!(observed["terminal_reported"], true);
        assert_eq!(observed["artifact_verified"], false);
        assert_eq!(observed["effects_settled"], false);
        assert_eq!(observed["controlled_confirmation_available"], true);
        let mut q = query("work.observe");
        q.work_id = Some("a".into());
        assert_eq!(
            store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["guidance"]["action"]["op"],
            "execution.attest"
        );
        let cmd = confirm(&store, &reported, "confirm", outcome).await;
        let receipt = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await
            .unwrap();
        let data = &receipt["receipt"]["data"];
        assert_eq!(data["state"], outcome);
        assert_eq!(data["controlled_recovery_cleared"], true);
        assert_eq!(data["recovery_blocked"], false);
        assert_eq!(data["artifact_verified"], false);
        let before = snapshot(&admin).await;
        let replay = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap();
        assert_eq!(replay["receipt"]["data"], receipt["receipt"]["data"]);
        assert_eq!(snapshot(&admin).await, before);
        let observed = inspect(&store, A, &reported).await;
        assert_eq!(observed["effects_settled"], true);
        assert_eq!(observed["artifact_verified"], false);
        assert_eq!(observed["recovery_cause"], "none");
    }
}

#[tokio::test]
async fn controlled_confirmation_requires_the_original_current_grant_version() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let c = take(&store).await;
    let e = controlled_start(&store, &c).await;
    let e = report(&store, &e, "report").await;
    admin.batch_execute("UPDATE awr_team.workstream_grants SET grant_version=grant_version+1 WHERE client_id='cli-a'").await.unwrap();
    let observed = inspect(&store, A, &e).await;
    assert_eq!(observed["attestation_authority"], false);
    assert_eq!(observed["controlled_confirmation_available"], false);
    let cmd = confirm(&store, &e, "changed-grant", "succeeded").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, cmd).await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn incomplete_or_unknown_controlled_confirmation_preserves_the_barrier() {
    for mutation in [
        "missing_receipt",
        "old_receipt",
        "missing_stop",
        "false_stop",
        "unknown",
    ] {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        trusted_runner(&admin).await;
        let c = take(&store).await;
        let e = controlled_start(&store, &c).await;
        let first = report(&store, &e, "report").await;
        let e = report(&store, &first, "new-report").await;
        let mut cmd = confirm(&store, &e, "confirm", "succeeded").await;
        match mutation {
            "missing_receipt" => {
                cmd.args
                    .as_object_mut()
                    .unwrap()
                    .remove("reviewed_receipt_id");
            }
            "old_receipt" => cmd.args["reviewed_receipt_id"] = first["receipt_id"].clone(),
            "missing_stop" => {
                cmd.args["facts"]
                    .as_object_mut()
                    .unwrap()
                    .remove("executor_stopped");
            }
            "false_stop" => cmd.args["facts"]["executor_stopped"] = json!(false),
            _ => cmd.args["facts"]["outcome"] = json!("unknown"),
        }
        let r = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap();
        let r = &r["receipt"]["data"];
        assert_eq!(r["controlled_recovery_cleared"], false, "{mutation}");
        assert_eq!(r["recovery_blocked"], true, "{mutation}");
        assert_eq!(
            r["effects_settled"],
            !matches!(mutation, "unknown" | "false_stop"),
            "{mutation}"
        );
        if matches!(mutation, "unknown" | "false_stop") {
            assert_eq!(r["state"], "unknown");
            assert_eq!(r["resources_released"], 0);
        }
        assert_eq!(inspect(&store, A, &e).await["artifact_verified"], false);
    }
}

#[tokio::test]
async fn controlled_confirmation_cannot_clear_changed_fences_or_other_effects() {
    for mutation in ["fence", "outside_scope", "outbox", "other_run"] {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        trusted_runner(&admin).await;
        let c = take(&store).await;
        let e = controlled_start(&store, &c).await;
        let e = report(&store, &e, "report").await;
        match mutation {
            "fence" => {
                admin.batch_execute("UPDATE awr_team.work_runtime SET last_fence=last_fence+1 WHERE work_id='a'").await.unwrap();
            }
            "outbox" => {
                admin.execute("INSERT INTO awr_team.outbox(tenant_id,project_id,id,state,payload_json,aggregate_id)
                VALUES($1,$2,'external-effect','pending','{}',$3)", &[&TENANT,&PROJECT,&e["execution_id"].as_str().unwrap()]).await.unwrap();
            }
            "other_run" => {
                admin.batch_execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
                VALUES('reader-tenant','reader-project','other-unknown','a',0,'old','old-runner','unknown')").await.unwrap();
            }
            _ => {}
        }
        let mut cmd = confirm(&store, &e, "confirm", "failed").await;
        if mutation == "outside_scope" {
            cmd.args["facts"]["observed_paths"] = json!(["src/elsewhere/result.json"]);
        } else {
            assert_eq!(
                inspect(&store, A, &e).await["controlled_confirmation_available"],
                false,
                "{mutation}"
            );
        }
        let r = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap();
        assert_eq!(r["receipt"]["data"]["recovery_blocked"], true, "{mutation}");
        assert_eq!(
            r["receipt"]["data"]["controlled_recovery_cleared"], false,
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn controlled_report_and_confirmation_roll_back_with_their_events() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let c = take(&store).await;
    let e = controlled_start(&store, &c).await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_controlled_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type IN ('execution.report','execution.attest') THEN RAISE EXCEPTION 'synthetic failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER reject_controlled_event BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_controlled_event()").await.unwrap();
    let mut report_cmd = attest(&store, &e, "report", "succeeded").await;
    report_cmd.op = "execution.report".into();
    report_cmd.args = json!({"session_id":"session-a","expected_session_version":"1","execution_id":e["execution_id"],
        "expected_execution_version":e["execution_version"],"outcome":"succeeded","output_digest":"b".repeat(64),
        "observed_paths":["src/api/result.json"],"note":"Stopped under controlled admission."});
    let before = snapshot(&admin).await;
    assert!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, report_cmd.clone())
            .await
            .is_err()
    );
    assert_eq!(snapshot(&admin).await, before);
    admin
        .batch_execute("DROP TRIGGER reject_controlled_event ON awr_team.events")
        .await
        .unwrap();
    let e = store
        .commands()
        .execute(TENANT, PROJECT, A, report_cmd)
        .await
        .unwrap()["receipt"]["data"]
        .clone();
    admin.batch_execute("CREATE TRIGGER reject_controlled_event BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_controlled_event()").await.unwrap();
    let cmd = confirm(&store, &e, "confirm", "succeeded").await;
    let before = snapshot(&admin).await;
    assert!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await
            .is_err()
    );
    assert_eq!(snapshot(&admin).await, before);
    admin.batch_execute("DROP TRIGGER reject_controlled_event ON awr_team.events; DROP FUNCTION awr_team.reject_controlled_event()").await.unwrap();
    let commands = store.commands();
    let (a, b) = tokio::join!(
        commands.execute(TENANT, PROJECT, A, cmd.clone()),
        commands.execute(TENANT, PROJECT, A, cmd)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a["receipt"], b["receipt"]);
    assert_ne!(a["replayed"], b["replayed"]);
    assert_eq!(a["receipt"]["data"]["controlled_recovery_cleared"], true);
}

#[tokio::test]
async fn controlled_confirmation_refuses_an_old_epoch_without_mutation() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let c = take(&store).await;
    let e = controlled_start(&store, &c).await;
    let e = report(&store, &e, "report").await;
    let cmd = confirm(&store, &e, "confirm", "succeeded").await;
    admin
        .batch_execute(
            "UPDATE awr_team.executions SET coordinator_epoch='older-epoch';
            UPDATE awr_team.claims SET coordinator_epoch='older-epoch'",
        )
        .await
        .unwrap();
    assert_eq!(
        inspect(&store, A, &e).await["controlled_confirmation_available"],
        false
    );
    let before = snapshot(&admin).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, cmd).await,
        Err(PgError::EpochChanged)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn another_barrier_write_and_residual_resources_prevent_automatic_clear() {
    for mutation in [
        "same_boolean_write",
        "residual_resource",
        "corrupt_receipt",
        "late_report",
    ] {
        let (_g, admin, _, store) = setup().await;
        enable_writes(&admin).await;
        trusted_runner(&admin).await;
        let c = take(&store).await;
        let e = controlled_start(&store, &c).await;
        if mutation == "late_report" {
            admin
                .batch_execute(
                    "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
                )
                .await
                .unwrap();
        }
        let e = report(&store, &e, "report").await;
        match mutation {
            "same_boolean_write" => {
                admin
                    .batch_execute(
                        "UPDATE awr_team.work_runtime SET recovery_blocked=true WHERE work_id='a'",
                    )
                    .await
                    .unwrap();
            }
            "residual_resource" => {
                admin.batch_execute("INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
                VALUES('reader-tenant','reader-project','residual','a','named','shared-effect','unknown')").await.unwrap();
            }
            "corrupt_receipt" => {
                admin
                    .batch_execute("UPDATE awr_team.execution_receipts SET digest=repeat('d',64)")
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let observed = inspect(&store, A, &e).await;
        assert_eq!(
            observed["controlled_confirmation_available"], false,
            "{mutation}"
        );
        let cmd = confirm(&store, &e, "confirm", "failed").await;
        let result = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap();
        assert_eq!(
            result["receipt"]["data"]["controlled_recovery_cleared"], false,
            "{mutation}"
        );
        assert_eq!(
            result["receipt"]["data"]["recovery_blocked"], true,
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn repeated_controlled_reports_bind_the_latest_receipt_without_admitting_more_work() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let c = take(&store).await;
    let e = controlled_start(&store, &c).await;
    let first = report(&store, &e, "first-report").await;
    let second = report(&store, &first, "second-report").await;
    assert_ne!(first["receipt_id"], second["receipt_id"]);
    assert_eq!(
        inspect(&store, A, &second).await["controlled_confirmation_available"],
        true
    );
    let cmd = confirm(&store, &second, "confirm", "succeeded").await;
    let result = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    assert_eq!(
        result["receipt"]["data"]["controlled_recovery_cleared"],
        true
    );
}

#[tokio::test]
async fn trusted_executor_requires_both_operator_grant_and_exact_system_client() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    assert_eq!(e["result_authority"], "trusted_executor");
    let cmd = attest(&store, &e, "attest", "succeeded").await;
    let before = snapshot(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE id='agent';
        UPDATE awr_team.workstream_grants SET can_attest_execution=false WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_attest_execution=true WHERE client_id='cli-a'").await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    admin
        .batch_execute(
            "UPDATE awr_team.actors SET kind='system' WHERE id='agent';
        UPDATE awr_team.workstream_grants SET can_attest_execution=false WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    // Restore the same fixture grant. A reissued grant version cannot attest
    // this admission and is covered separately above.
    admin
        .batch_execute(
            "UPDATE awr_team.actors SET kind='system' WHERE id='agent';
        UPDATE awr_team.workstream_grants SET can_attest_execution=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write,can_attest_execution)
        VALUES($1,$2,'agent','cli-b',$3,1,true,true,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    let mut forged = cmd.clone();
    forged.args["receipt_kind"] = json!("trusted_executor");
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, forged).await,
        Err(PgError::Protocol(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
    let result = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    assert_eq!(
        result["receipt"]["data"]["receipt_kind"],
        "trusted_executor"
    );
    assert_eq!(result["receipt"]["data"]["state"], "succeeded");
}

#[tokio::test]
async fn later_authority_cannot_upgrade_an_ordinary_admission_to_trusted_execution() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "ordinary").await;
    assert_eq!(e["result_authority"], "caller_asserted");
    trusted_runner(&admin).await;
    assert_eq!(inspect(&store, A, &e).await["attestation_authority"], false);
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                attest(&store, &e, "retroactive", "succeeded").await
            )
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let r = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            OP,
            reconcile(&store, &os, &e, "review", "succeeded").await,
        )
        .await
        .unwrap();
    assert_eq!(r["receipt"]["data"]["receipt_kind"], "reconcile");
    assert_eq!(r["receipt"]["data"]["recovery_blocked"], false);
    assert!(inspect(&store, OP, &e).await["latest_receipt"]["payload"]["admission_attestation_grant_version"].is_null());
}

#[tokio::test]
async fn trusted_results_release_only_bound_resources_and_do_not_complete_work() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let c = take(&store).await;
    for outcome in ["succeeded", "failed", "cancelled"] {
        let e = start(&store, &c, outcome).await;
        let cmd = attest(&store, &e, outcome, outcome).await;
        let result = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd.clone())
            .await
            .unwrap();
        let d = &result["receipt"]["data"];
        assert_eq!(d["state"], outcome);
        assert_eq!(d["resources_released"], 1);
        assert_eq!(d["work_completed"], false);
        assert_eq!(d["recovery_blocked"], false);
        assert_eq!(result["execution_authorized"], false);
        let before = snapshot(&admin).await;
        let replay = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .unwrap();
        assert_eq!(replay["receipt"], result["receipt"]);
        assert_eq!(snapshot(&admin).await, before);
        let v = inspect(&store, A, &e).await;
        assert_eq!(
            v["latest_receipt"]["payload"]["output_digest"],
            "b".repeat(64)
        );
        assert_eq!(v["latest_receipt"]["receipt_kind"], "trusted_executor");
        assert_eq!(v["automatic_resume"], false);
    }
    let s = snapshot(&admin).await;
    assert!(s["runtime"][0]["selected_completion_id"].is_null());
    assert_eq!(s["resources"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn operator_reconciliation_after_expiry_unblocks_a_new_claim_without_promoting_trust_grade() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let e = report(&store, &e, "observation").await;
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    let cmd = reconcile(&store, &os, &e, "settle", "succeeded").await;
    let result = store
        .commands()
        .execute(TENANT, PROJECT, OP, cmd.clone())
        .await
        .unwrap();
    let d = &result["receipt"]["data"];
    assert_eq!(d["receipt_kind"], "reconcile");
    assert_eq!(d["resources_released"], 1);
    assert_eq!(d["recovery_blocked"], false);
    assert_eq!(d["work_completed"], false);
    let p = prepare(&store, A, "a").await;
    let new=store.commands().execute(TENANT,PROJECT,A,command(&p,"take-again","claim.acquire",
        json!({"session_id":"session-a","expected_session_version":"1","expected_work_version":p["data"]["runtime"]["work_version"],"ttl_seconds":60})))
        .await.unwrap();
    assert_ne!(new["receipt"]["data"]["fence"], c["fence"]);
    let before = snapshot(&admin).await;
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, OP, cmd)
        .await
        .unwrap();
    assert_eq!(replay["receipt"], result["receipt"]);
    assert_eq!(snapshot(&admin).await, before);
    assert_eq!(admin.query_one("SELECT count(*) FROM awr_team.execution_receipts WHERE receipt_kind='trusted_executor'",&[]).await.unwrap().get::<_,i64>(0),0);
}

#[tokio::test]
async fn admin_or_manage_alone_and_agent_self_certification_cannot_reconcile() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let e = report(&store, &e, "observe").await;
    let cmd = reconcile(&store, &os, &e, "settle", "succeeded").await;
    let before = snapshot(&admin).await;
    for sql in [
        "UPDATE awr_team.workstream_grants SET can_reconcile_execution=false WHERE client_id='operator-cli'",
        "UPDATE awr_team.workstream_grants SET can_reconcile_execution=true,can_manage=false WHERE client_id='operator-cli'",
        "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='operator-cli'; UPDATE awr_team.project_memberships SET role='worker' WHERE actor_id='operator'",
        "UPDATE awr_team.project_memberships SET role='admin' WHERE actor_id='operator'; UPDATE awr_team.actors SET kind='agent' WHERE id='operator'",
    ] {
        admin.batch_execute(sql).await.unwrap();
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, OP, cmd.clone())
                .await,
            Err(PgError::Forbidden)
        ));
    }
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn reconciliation_keeps_other_execution_and_legacy_resource_barriers() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let e = report(&store, &e, "observe").await;
    admin.batch_execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state)
        VALUES('reader-tenant','reader-project','other-unknown','a',0,'old','old-runner','unknown');
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state,execution_id)
        VALUES('reader-tenant','reader-project','other-resource','a','prefix','other','unknown','other-unknown'),
        ('reader-tenant','reader-project','legacy-resource','a','named','legacy','unknown',NULL);
        UPDATE awr_team.work_runtime SET recovery_blocked=false WHERE work_id='a'").await.unwrap();
    let mut command = reconcile(&store, &os, &e, "settle", "succeeded").await;
    command.args["clear_recovery_block"] = json!(false);
    let result = store
        .commands()
        .execute(TENANT, PROJECT, OP, command)
        .await
        .unwrap();
    let d = &result["receipt"]["data"];
    assert_eq!(d["state"], "succeeded");
    assert_eq!(d["resources_released"], 1);
    assert_eq!(d["recovery_blocked"], true);
    assert_eq!(d["recovery_clear_requested"], false);
    assert_eq!(d["unresolved_work_effects"], true);
    let r = admin
        .query_one(
            "SELECT (SELECT state FROM awr_team.executions WHERE id='other-unknown'),
        (SELECT count(*) FROM awr_team.resource_reservations WHERE state='unknown')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(r.get::<_, String>(0), "unknown");
    assert_eq!(r.get::<_, i64>(1), 2);
}

#[tokio::test]
async fn stale_receipt_or_work_version_and_diverging_terminal_facts_are_rejected() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let e = report(&store, &e, "observe").await;
    let cmd = reconcile(&store, &os, &e, "settle", "succeeded").await;
    let before = snapshot(&admin).await;
    for (field, value) in [
        ("reviewed_receipt_id", json!("not-reviewed")),
        ("expected_work_version", json!("999")),
        ("expected_execution_version", json!("999")),
    ] {
        let mut bad = cmd.clone();
        bad.args[field] = value;
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, OP, bad).await,
            Err(PgError::PreconditionsChanged)
        ));
    }
    let mut bad = cmd.clone();
    bad.args["facts"]["input_digest"] = json!("d".repeat(64));
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, OP, bad).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let r = store
        .commands()
        .execute(TENANT, PROJECT, OP, cmd)
        .await
        .unwrap();
    let e = &r["receipt"]["data"];
    let mut changed = reconcile(&store, &os, e, "rewrite", "succeeded").await;
    changed.args["facts"]["output_digest"] = json!("f".repeat(64));
    let before = snapshot(&admin).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, OP, changed).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn runner_cannot_clear_a_recovery_block_and_scope_violation_needs_operator_resolution() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let mut cmd = attest(&store, &e, "escaped", "succeeded").await;
    cmd.args["facts"]["observed_paths"] = json!(["outside/output"]);
    let r = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    let e = &r["receipt"]["data"];
    assert_eq!(e["state"], "unknown");
    assert_eq!(e["scope_violation"], true);
    assert_eq!(e["resources_released"], 0);
    let mut cmd = reconcile(&store, &os, e, "confirm-escape", "succeeded").await;
    cmd.args["facts"]["observed_paths"] = json!(["outside/output"]);
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, OP, cmd).await,
        Err(PgError::ScopeExceeded)
    ));
    // Corrected runner facts can settle the attempt, but only an operator can
    // explicitly review and lift a previously recorded work recovery barrier.
    let r = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            attest(&store, e, "verified-stop", "failed").await,
        )
        .await
        .unwrap();
    assert_eq!(r["receipt"]["data"]["state"], "failed");
    assert_eq!(r["receipt"]["data"]["recovery_blocked"], true);
    let r = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            OP,
            reconcile(
                &store,
                &os,
                &r["receipt"]["data"],
                "operator-clear",
                "failed",
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(r["receipt"]["data"]["recovery_blocked"], false);
}

#[tokio::test]
async fn receipt_details_are_visible_only_to_original_client_or_scoped_recovery_operator() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let _os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let e = report(&store, &e, "observe").await;
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'agent','cli-b',$3,1,true,true)",&[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    assert!(inspect(&store, A, &e).await["latest_receipt"]["payload"].is_object());
    assert!(inspect(&store, OP, &e).await["latest_receipt"]["payload"].is_object());
    let b = inspect(&store, B, &e).await;
    assert!(b["latest_receipt"].is_null());
    assert_eq!(b["receipt_details_available"], false);
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_reconcile_execution=false WHERE client_id='operator-cli'").await.unwrap();
    assert!(inspect(&store, OP, &e).await["latest_receipt"].is_null());
}

#[tokio::test]
async fn failed_recovery_event_rolls_back_receipt_terminal_state_resource_release_and_clear() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let e = report(&store, &e, "observe").await;
    admin.batch_execute("CREATE FUNCTION awr_team.reject_settlement() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.event_type IN ('execution.attest','execution.reconcile') THEN RAISE EXCEPTION 'synthetic failure'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER reject_settlement BEFORE INSERT ON awr_team.events FOR EACH ROW EXECUTE FUNCTION awr_team.reject_settlement()").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                attest(&store, &e, "attest", "succeeded").await
            )
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
    assert!(matches!(
        store
            .commands()
            .execute(
                TENANT,
                PROJECT,
                OP,
                reconcile(&store, &os, &e, "reconcile", "succeeded").await
            )
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn schema_thirteen_preserves_legacy_resources_and_grants_no_new_authority() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let claim = take(&store).await;
    let execution = start(&store, &claim, "legacy").await;
    let admitted_resource = execution["resources"][0]["reservation_id"]
        .as_str()
        .unwrap()
        .to_string();
    // Exercise migration 13 in isolation. Retain later schema additions rather
    // than pretending a partially downgraded current schema is a version-12 DB.
    admin.batch_execute("ALTER TABLE awr_team.resource_reservations DROP COLUMN execution_id;
        ALTER TABLE awr_team.executions DROP CONSTRAINT executions_resource_identity;
        ALTER TABLE awr_team.executions DROP COLUMN attestation_grant_version;
        ALTER TABLE awr_team.workstream_grants DROP COLUMN can_attest_execution,DROP COLUMN can_reconcile_execution;
        UPDATE awr_team.schema_state SET version=12;
        INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','legacy','a','named','deployment','unknown')").await.unwrap();
    let sql = include_str!("../migrations/20260921000013_workstream_execution_authority.sql");
    assert!(
        admin
            .batch_execute(&sql.replace(
                "UPDATE awr_team.schema_state",
                "SELECT 1/0; UPDATE awr_team.schema_state"
            ))
            .await
            .is_err()
    );
    admin.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(
        admin
            .query_one("SELECT version FROM awr_team.schema_state", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        12
    );
    admin.batch_execute(sql).await.unwrap();
    assert_eq!(
        admin
            .query_one("SELECT version FROM awr_team.schema_state", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        13
    );
    // All later schema objects were retained; restore the marker for the
    // current store's recovery assertions below, without replaying migrations.
    admin
        .execute(
            "UPDATE awr_team.schema_state SET version=$1 WHERE component='awr_team'",
            &[&awr_team_pg::EXPECTED_SCHEMA_VERSION],
        )
        .await
        .unwrap();
    let r=admin.query_one("SELECT (SELECT state FROM awr_team.resource_reservations WHERE id='legacy'),
        (SELECT execution_id FROM awr_team.resource_reservations WHERE id='legacy'),
        (SELECT count(*) FROM awr_team.workstream_grants WHERE can_attest_execution OR can_reconcile_execution),
        (SELECT state FROM awr_team.resource_reservations WHERE id=$1),
        (SELECT execution_id FROM awr_team.resource_reservations WHERE id=$1)",&[&admitted_resource]).await.unwrap();
    assert_eq!(r.get::<_, String>(0), "unknown");
    assert_eq!(r.get::<_, Option<String>>(1), None);
    assert_eq!(r.get::<_, i64>(2), 0);
    assert_eq!(r.get::<_, String>(3), "reserved");
    assert_eq!(r.get::<_, Option<String>>(4), None);
    let legacy = admin
        .query_one(
            "SELECT state,attestation_grant_version FROM awr_team.executions WHERE id=$1",
            &[&execution["execution_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(legacy.get::<_, String>(0), "running");
    assert_eq!(legacy.get::<_, Option<i64>>(1), None);
    // The unrelated legacy row already proved migration preservation. Remove
    // it from the barrier calculation so the admitted-but-unbound reservation
    // alone proves that schema 13 recovery remains fail-closed.
    admin
        .execute(
            "UPDATE awr_team.resource_reservations SET state='released' WHERE id='legacy'",
            &[],
        )
        .await
        .unwrap();
    let observed = report(&store, &execution, "legacy-observation").await;
    assert_eq!(observed["state"], "unknown");
    assert_eq!(observed["recovery_blocked"], true);
    let os = operator(&admin, &store).await;
    let reconciled = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            OP,
            reconcile(&store, &os, &observed, "legacy-reconciliation", "failed").await,
        )
        .await
        .unwrap();
    let data = &reconciled["receipt"]["data"];
    assert_eq!(data["resources_released"], 0);
    assert_eq!(data["unresolved_work_effects"], true);
    assert_eq!(data["recovery_blocked"], true);
    let admitted = admin
        .query_one(
            "SELECT state,execution_id FROM awr_team.resource_reservations WHERE id=$1",
            &[&admitted_resource],
        )
        .await
        .unwrap();
    assert_eq!(admitted.get::<_, String>(0), "reserved");
    assert_eq!(admitted.get::<_, Option<String>>(1), None);
}

#[tokio::test]
async fn unknown_results_hold_resources_and_settlement_requires_explicit_operator_clear() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let result = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            attest(&store, &e, "unknown", "unknown").await,
        )
        .await
        .unwrap();
    let e = result["receipt"]["data"].clone();
    assert_eq!(e["state"], "unknown");
    assert_eq!(e["resources_released"], 0);
    assert_eq!(e["recovery_blocked"], true);
    let bad = reconcile(&store, &os, &e, "clear-unknown", "unknown").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, OP, bad.clone())
            .await,
        Err(PgError::Protocol(_))
    ));
    assert_eq!(snapshot(&admin).await, before);
    let mut still_unknown = bad;
    still_unknown.args["clear_recovery_block"] = json!(false);
    let r = store
        .commands()
        .execute(TENANT, PROJECT, OP, still_unknown)
        .await
        .unwrap();
    assert_eq!(r["receipt"]["data"]["recovery_blocked"], true);
    let mut settle = reconcile(&store, &os, &e, "settle-only", "failed").await;
    settle.args["clear_recovery_block"] = json!(false);
    let r = store
        .commands()
        .execute(TENANT, PROJECT, OP, settle)
        .await
        .unwrap();
    assert_eq!(r["receipt"]["data"]["state"], "failed");
    assert_eq!(r["receipt"]["data"]["resources_released"], 1);
    assert_eq!(r["receipt"]["data"]["recovery_blocked"], true);
    let r = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            OP,
            reconcile(&store, &os, &e, "clear-reviewed", "failed").await,
        )
        .await
        .unwrap();
    assert_eq!(r["receipt"]["data"]["recovery_blocked"], false);
}

#[tokio::test]
async fn recovery_cannot_adopt_executions_from_another_epoch_or_ownership_generation() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let os = operator(&admin, &store).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let attest_cmd = attest(&store, &e, "attest", "succeeded").await;
    let reconcile_cmd = reconcile(&store, &os, &e, "reconcile", "succeeded").await;
    for field in ["coordinator_epoch", "ownership_version"] {
        let sql = if field == "coordinator_epoch" {
            "UPDATE awr_team.executions SET coordinator_epoch='prior';
             UPDATE awr_team.claims SET coordinator_epoch='prior'"
        } else {
            "UPDATE awr_team.executions SET coordinator_epoch='epoch-a',ownership_version=2;
             UPDATE awr_team.claims SET coordinator_epoch='epoch-a'"
        };
        admin.batch_execute(sql).await.unwrap();
        let before = snapshot(&admin).await;
        for (token, cmd) in [(A, attest_cmd.clone()), (OP, reconcile_cmd.clone())] {
            let err = store
                .commands()
                .execute(TENANT, PROJECT, token, cmd)
                .await
                .unwrap_err();
            if field == "coordinator_epoch" {
                assert!(matches!(err, PgError::EpochChanged), "{err:?}");
            } else {
                assert!(matches!(err, PgError::Forbidden), "{err:?}");
            }
        }
        assert_eq!(snapshot(&admin).await, before);
    }
}

#[tokio::test]
async fn a_resource_cannot_be_bound_to_an_execution_of_another_work() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = take(&store).await;
    let e = start(&store, &c, "one").await;
    let before = snapshot(&admin).await;
    let err = admin.execute("INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state,execution_id)
        VALUES($1,$2,'wrong-work','b','prefix','other','reserved',$3)",
        &[&TENANT,&PROJECT,&e["execution_id"].as_str().unwrap()]).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some(&tokio_postgres::error::SqlState::FOREIGN_KEY_VIOLATION)
    );
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn revocation_while_recovery_waits_prevents_receipts_and_resource_release() {
    for recovering in [false, true] {
        let (_g, mut admin, db, store) = setup().await;
        enable_writes(&admin).await;
        trusted_runner(&admin).await;
        let os = operator(&admin, &store).await;
        let c = take(&store).await;
        let e = start(&store, &c, "one").await;
        let (token, cmd, sql) = if recovering {
            (
                OP,
                reconcile(&store, &os, &e, "reconcile", "succeeded").await,
                "UPDATE awr_team.workstream_grants SET can_reconcile_execution=false,grant_version=grant_version+1 WHERE client_id='operator-cli'",
            )
        } else {
            (
                A,
                attest(&store, &e, "attest", "succeeded").await,
                "UPDATE awr_team.workstream_grants SET can_attest_execution=false,grant_version=grant_version+1 WHERE client_id='cli-a'",
            )
        };
        let before = snapshot(&admin).await;
        let observer = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
        let revoke = admin.transaction().await.unwrap();
        revoke.batch_execute(sql).await.unwrap();
        let commands = store.commands();
        let pending =
            tokio::spawn(async move { commands.execute(TENANT, PROJECT, token, cmd).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let blocked: bool = observer.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND query LIKE '%ORDER BY workstream_id FOR SHARE%')", &[]).await.unwrap().get(0);
                if blocked { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("recovery must wait on current grant");
        revoke.commit().await.unwrap();
        assert!(matches!(pending.await.unwrap(), Err(PgError::Forbidden)));
        assert_eq!(snapshot(&admin).await, before);
    }
}

// A synthetic restored projection, not a claim that the production import API
// can yet restore enabled workstreams. Preserve the original execution epoch.
async fn restored_boundary(admin: &Client) {
    admin.batch_execute("UPDATE awr_team.projects SET coordinator_epoch='restored-generation',project_revision=project_revision+1
        WHERE tenant_id='reader-tenant' AND id='reader-project';
        UPDATE awr_team.sessions SET state='interrupted',session_version=session_version+1 WHERE state='active';
        UPDATE awr_team.claims SET state='revoked',lease_version=lease_version+1 WHERE state='active';
        UPDATE awr_team.executions SET state='unknown',cancel_requested=true,execution_version=execution_version+1;
        UPDATE awr_team.work_runtime SET recovery_blocked=true,last_fence=last_fence+1,work_version=work_version+1;
        UPDATE awr_team.resource_reservations SET state='unknown' WHERE state='reserved'").await.unwrap();
}
fn previous_epoch_review(stopped: bool) -> Value {
    json!({"execution_epoch":"epoch-a","executor_stopped":stopped,"review_reference":"fixture:reviewed-executor-and-resource-barrier"})
}

#[tokio::test]
async fn previous_epoch_settlement_requires_explicit_review_and_preserves_original_attribution() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let c = take(&store).await;
    let e = start(&store, &c, "old").await;
    let e = report(&store, &e, "original-observation").await;
    let old_receipt = admin
        .query_one("SELECT to_jsonb(r) FROM awr_team.execution_receipts r", &[])
        .await
        .unwrap()
        .get::<_, Value>(0);
    restored_boundary(&admin).await;
    let os = operator(&admin, &store).await;
    let info = inspect(&store, OP, &e).await;
    assert_eq!(info["execution_coordinator_epoch"], "epoch-a");
    assert_eq!(info["previous_epoch_review_required"], true);
    assert_eq!(info["previous_epoch_recovery_available"], true);
    assert_eq!(inspect(&store, A, &e).await["attestation_authority"], false);
    let mut cmd = reconcile(&store, &os, &e, "review-old", "succeeded").await;
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, OP, cmd.clone())
            .await,
        Err(PgError::EpochChanged)
    ));
    assert_eq!(snapshot(&admin).await, before);
    cmd.args["previous_epoch_recovery"] = previous_epoch_review(true);
    let r = store
        .commands()
        .execute(TENANT, PROJECT, OP, cmd.clone())
        .await
        .unwrap();
    let data = &r["receipt"]["data"];
    assert_eq!(data["state"], "succeeded");
    assert_eq!(data["previous_epoch_reconciled"], true);
    assert_eq!(data["execution_coordinator_epoch"], "epoch-a");
    assert_eq!(data["reporting_coordinator_epoch"], "restored-generation");
    assert_eq!(data["recovery_blocked"], false);
    assert_eq!(data["work_completed"], false);
    let row=admin.query_one("SELECT coordinator_epoch,executor_actor_id,executor_client_id,ownership_version FROM awr_team.executions",&[]).await.unwrap();
    assert_eq!(row.get::<_, String>(0), "epoch-a");
    assert_eq!(row.get::<_, String>(1), "agent");
    assert_eq!(row.get::<_, String>(2), "cli-a");
    assert_eq!(row.get::<_, i64>(3), 1);
    let original = admin
        .query_one(
            "SELECT to_jsonb(r) FROM awr_team.execution_receipts r WHERE id=$1",
            &[&old_receipt["id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get::<_, Value>(0);
    assert_eq!(original, old_receipt);
    let latest = inspect(&store, OP, &e).await;
    assert_eq!(latest["latest_receipt"]["receipt_kind"], "reconcile");
    assert_eq!(
        latest["latest_receipt"]["payload"]["recovery_review_basis"],
        "authorized_operator_assertion"
    );
    let after = snapshot(&admin).await;
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, OP, cmd)
        .await
        .unwrap();
    assert_eq!(replay["receipt"], r["receipt"]);
    assert_eq!(replay["execution_authorized"], false);
    assert_eq!(snapshot(&admin).await, after);
}

#[tokio::test]
async fn previous_epoch_review_requires_stop_confirmation_and_exact_epoch_and_ownership() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = take(&store).await;
    let e = start(&store, &c, "old").await;
    restored_boundary(&admin).await;
    let os = operator(&admin, &store).await;
    let mut cmd = reconcile(&store, &os, &e, "review-old", "succeeded").await;
    for review in [
        previous_epoch_review(false),
        json!({"execution_epoch":"other","executor_stopped":true,"review_reference":"fixture:barrier"}),
        json!({"execution_epoch":"epoch-a","executor_stopped":true,"review_reference":""}),
        json!({"execution_epoch":"epoch-a","executor_stopped":true,"review_reference":"x".repeat(2049)}),
        json!({"execution_epoch":"epoch-a","executor_stopped":true,"review_reference":"fixture:\nbarrier"}),
        json!({"execution_epoch":"epoch-a","executor_stopped":true,"review_reference":"fixture:barrier","force":true}),
    ] {
        cmd.args["previous_epoch_recovery"] = review;
        let before = snapshot(&admin).await;
        assert!(
            store
                .commands()
                .execute(TENANT, PROJECT, OP, cmd.clone())
                .await
                .is_err()
        );
        assert_eq!(snapshot(&admin).await, before);
    }
    cmd.args["previous_epoch_recovery"] = previous_epoch_review(true);
    for mutation in [
        "UPDATE awr_team.executions SET ownership_version=2",
        // A legacy row has no scoped attribution. Preserve the schema's all-or-none binding.
        "UPDATE awr_team.executions SET workstream_id=NULL,ownership_version=NULL,executor_client_id=NULL,coordinator_epoch=NULL",
    ] {
        admin.batch_execute(mutation).await.unwrap();
        let before = snapshot(&admin).await;
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, OP, cmd.clone())
                .await,
            Err(PgError::Forbidden)
        ));
        assert_eq!(snapshot(&admin).await, before);
    }
}

#[tokio::test]
async fn epoch_review_cannot_extend_executor_reports_or_override_current_epoch_preconditions() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    trusted_runner(&admin).await;
    let c = take(&store).await;
    let e = start(&store, &c, "current").await;
    let os = operator(&admin, &store).await;
    let mut review = reconcile(&store, &os, &e, "unnecessary-review", "succeeded").await;
    review.args["previous_epoch_recovery"] = previous_epoch_review(true);
    let before = snapshot(&admin).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, OP, review).await,
        Err(PgError::PreconditionsChanged)
    ));
    let mut attestation = attest(&store, &e, "forged-review", "succeeded").await;
    attestation.args["previous_epoch_recovery"] = previous_epoch_review(true);
    let report = command(
        &prepare(&store, A, "a").await,
        "forged-report-review",
        "execution.report",
        json!({"session_id":"session-a","expected_session_version":"1",
            "execution_id":e["execution_id"],"expected_execution_version":e["execution_version"],
            "outcome":"succeeded","output_digest":"b".repeat(64),
            "observed_paths":["src/api/result.json"],"note":"Caller cannot grant recovery authority.",
            "previous_epoch_recovery":previous_epoch_review(true)}),
    );
    for cmd in [attestation, report] {
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, cmd).await,
            Err(PgError::Protocol(_))
        ));
    }
    assert_eq!(snapshot(&admin).await, before);
}

#[tokio::test]
async fn previous_epoch_unknown_observation_keeps_barriers_and_stale_reviews_are_rejected() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = take(&store).await;
    let e = start(&store, &c, "old").await;
    restored_boundary(&admin).await;
    let os = operator(&admin, &store).await;
    let mut stale = reconcile(&store, &os, &e, "stale-review", "failed").await;
    stale.args["previous_epoch_recovery"] = previous_epoch_review(true);
    let mut unknown = reconcile(&store, &os, &e, "unknown-review", "unknown").await;
    unknown.args["clear_recovery_block"] = json!(false);
    unknown.args["previous_epoch_recovery"] = previous_epoch_review(false);
    let observed = store
        .commands()
        .execute(TENANT, PROJECT, OP, unknown)
        .await
        .unwrap();
    assert_eq!(observed["receipt"]["data"]["resources_released"], 0);
    assert_eq!(observed["receipt"]["data"]["recovery_blocked"], true);
    let before = snapshot(&admin).await;
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, OP, stale).await,
        Err(PgError::PreconditionsChanged)
    ));
    assert_eq!(snapshot(&admin).await, before);
    let mut finish = reconcile(&store, &os, &e, "finish-reviewed", "failed").await;
    finish.args["previous_epoch_recovery"] = previous_epoch_review(true);
    // Legacy resource ownership is not inferred even when this execution settles.
    admin.batch_execute("INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES('reader-tenant','reader-project','legacy-hold','a','prefix','legacy','unknown')").await.unwrap();
    let r = store
        .commands()
        .execute(TENANT, PROJECT, OP, finish)
        .await
        .unwrap();
    assert_eq!(r["receipt"]["data"]["resources_released"], 1);
    assert_eq!(r["receipt"]["data"]["recovery_blocked"], true);
    assert_eq!(
        admin
            .query_one(
                "SELECT state FROM awr_team.resource_reservations WHERE id='legacy-hold'",
                &[]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "unknown"
    );
}

#[tokio::test]
async fn previous_epoch_recovery_checks_current_privileges_and_rolls_back_with_its_audit() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = take(&store).await;
    let e = start(&store, &c, "old").await;
    restored_boundary(&admin).await;
    let os = operator(&admin, &store).await;
    let mut cmd = reconcile(&store, &os, &e, "review-old", "failed").await;
    cmd.args["previous_epoch_recovery"] = previous_epoch_review(true);
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_reconcile_execution=false WHERE client_id='operator-cli'").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, OP, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    assert_eq!(snapshot(&admin).await, before);
    admin.batch_execute("UPDATE awr_team.workstream_grants SET can_reconcile_execution=true WHERE client_id='operator-cli';
        CREATE FUNCTION awr_team.reject_epoch_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic failure'; END $$;
        CREATE TRIGGER reject_epoch_receipt BEFORE INSERT ON awr_team.execution_receipts FOR EACH ROW EXECUTE FUNCTION awr_team.reject_epoch_receipt()").await.unwrap();
    let before = snapshot(&admin).await;
    assert!(
        store
            .commands()
            .execute(TENANT, PROJECT, OP, cmd.clone())
            .await
            .is_err()
    );
    assert_eq!(snapshot(&admin).await, before);
    admin.batch_execute("DROP TRIGGER reject_epoch_receipt ON awr_team.execution_receipts; DROP FUNCTION awr_team.reject_epoch_receipt()").await.unwrap();
    let s = store.commands();
    let (a, b) = tokio::join!(
        s.execute(TENANT, PROJECT, OP, cmd.clone()),
        s.execute(TENANT, PROJECT, OP, cmd)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a["replayed"], b["replayed"]);
    assert_eq!(a["receipt"], b["receipt"]);
}

#[tokio::test]
async fn reviewed_local_barrier_prevents_old_generation_writes_before_operator_settlement() {
    use awr_team_pg::{CrashPoint, FencingBarrier, OutboxDelivery, ReferenceRunner};
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let c = take(&store).await;
    let e = start(&store, &c, "old").await;
    let root = std::env::temp_dir().join(format!("awr-epoch-review-{}", common::nonce(0)));
    let runner = ReferenceRunner::new(&root);
    let fence = e["fence"].as_str().unwrap().parse().unwrap();
    let mut delivery = OutboxDelivery {
        outbox_id: String::new(),
        execution_id: e["execution_id"].as_str().unwrap().into(),
        effect_key: e["effect_key"].as_str().unwrap().into(),
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        scope_id: "main".into(),
        work_id: "a".into(),
        coordinator_epoch: "epoch-a".into(),
        fence,
        fencing_class: "uncontrolled".into(),
        declared_scope: json!(["src/api"]),
        payload: json!({"writes":[{"path":"src/api/result.json","content":"original artifact"}]}),
        delivery_attempts: 0,
    };
    let actual = runner.handle_delivery(&delivery, CrashPoint::None);
    assert_eq!(actual.state, "succeeded");
    restored_boundary(&admin).await;
    runner
        .install_recovery_barrier(&FencingBarrier {
            coordinator_epoch: "restored-generation".into(),
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
            scope_id: "main".into(),
            work_id: "a".into(),
            fence: fence + 1,
        })
        .unwrap();
    delivery.execution_id = "delayed-old-execution".into();
    delivery.effect_key = "delayed-old-effect".into();
    delivery.payload =
        json!({"writes":[{"path":"src/api/result.json","content":"must not overwrite"}]});
    let rejected = runner.handle_delivery(&delivery, CrashPoint::None);
    assert!(!rejected.started);
    assert_ne!(rejected.state, "succeeded");
    assert_eq!(
        std::fs::read_to_string(root.join("worktree/src/api/result.json")).unwrap(),
        "original artifact"
    );
    let os = operator(&admin, &store).await;
    let mut cmd = reconcile(&store, &os, &e, "review-local-barrier", "succeeded").await;
    cmd.args["facts"]["output_digest"] = json!(actual.output_digest);
    cmd.args["facts"]["environment_digest"] = json!(actual.environment_digest);
    cmd.args["previous_epoch_recovery"] = previous_epoch_review(true);
    let result = store
        .commands()
        .execute(TENANT, PROJECT, OP, cmd)
        .await
        .unwrap();
    assert_eq!(result["receipt"]["data"]["effects_settled"], true);
    assert_eq!(result["receipt"]["data"]["work_completed"], false);
    let _ = std::fs::remove_dir_all(root);
}
