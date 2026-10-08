//! Projection-impact regressions on independent isolated databases, not native acceptance.
use super::*;
use awr_team_pg::ActivationImpactGate;

fn independent_bundle() -> WorkstreamBundle {
    let mut value = bundle();
    for entry in &mut value.contracts {
        entry.contract.required_dependencies.clear();
    }
    value
}

async fn install_independent(store: &SourceStore) -> WorkstreamBundle {
    let value = independent_bundle();
    let c = approved(store, package(&value)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
        .await
        .unwrap();
    value
}

async fn fixture_execution(admin: &Client, work: &str, state: &str) {
    admin.execute("INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
        SELECT $1,$2,'impact-session','main',$3,'author','impact-client','impact-conversation','active',workstream_id,ownership_version
        FROM awr_team.workstream_ownership WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3", &[&TENANT,&PROJECT,&work]).await.unwrap();
    admin.execute("INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,expires_at,state,workstream_id,ownership_version,coordinator_epoch)
        SELECT $1,$2,'impact-claim','main',$3,'impact-session','author',1,clock_timestamp()+interval '1 hour','active',workstream_id,ownership_version,'epoch-1'
        FROM awr_team.workstream_ownership WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3", &[&TENANT,&PROJECT,&work]).await.unwrap();
    admin.execute("INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,fence,contract_hash,executor_actor_id,state,session_id,claim_id,workstream_id,ownership_version,executor_client_id,coordinator_epoch)
        SELECT $1,$2,'impact-execution',$3,1,c.contract_hash,'author',$4,'impact-session','impact-claim',o.workstream_id,o.ownership_version,'impact-client','epoch-1'
        FROM awr_team.work_contracts c JOIN awr_team.projects p ON p.tenant_id=c.tenant_id AND p.id=c.project_id
        AND p.active_snapshot_id=c.snapshot_id JOIN awr_team.workstream_ownership o ON o.tenant_id=c.tenant_id AND o.project_id=c.project_id AND o.work_id=c.work_id
        WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$3",
        &[&TENANT,&PROJECT,&work,&state]).await.unwrap();
}

fn asserted_gate(affected: &[&str], stopped: &[&str]) -> ActivationImpactGate {
    ActivationImpactGate {
        impact_proven: true,
        allow_activation: true,
        affected_work_ids: affected.iter().map(|s| (*s).into()).collect(),
        stopped_work_ids: stopped.iter().map(|s| (*s).into()).collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn unrelated_running_execution_survives_ordinary_activation() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    fixture_execution(&admin, "sdk", "running").await;
    let before: serde_json::Value = admin
        .query_one("SELECT to_jsonb(e) FROM awr_team.executions e", &[])
        .await
        .unwrap()
        .get(0);
    value.contracts[0]
        .contract
        .acceptance
        .push("new interface condition".into());
    let c = approved(&store, package(&value)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
        .await
        .expect("A source change must preserve an unrelated running execution");
    let after: serde_json::Value = admin
        .query_one("SELECT to_jsonb(e) FROM awr_team.executions e", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after);
    assert_eq!(current_snapshot(&admin).await, Some(c.snapshot_id));
}

#[tokio::test]
async fn empty_caller_impact_cannot_hide_changed_unknown_effects() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "interface", "unknown").await;
    value.contracts[0]
        .contract
        .acceptance
        .push("new interface condition".into());
    let c = approved(&store, package(&value)).await;
    let outcome = store
        .activate_workstreams_with_impact(
            TENANT,
            PROJECT,
            "author",
            &c.proposal_id,
            &plan(&c),
            &asserted_gate(&[], &[]),
        )
        .await;
    assert!(
        outcome.is_err(),
        "Caller-supplied empty impact cannot authorize unknown effects: {outcome:?}"
    );
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn stopped_assertion_and_cancel_request_cannot_settle_running_effects() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "interface", "running").await;
    admin
        .execute("UPDATE awr_team.executions SET cancel_requested=true", &[])
        .await
        .unwrap();
    value.contracts[0]
        .contract
        .acceptance
        .push("new interface condition".into());
    let c = approved(&store, package(&value)).await;
    let outcome = store
        .activate_workstreams_with_impact(
            TENANT,
            PROJECT,
            "author",
            &c.proposal_id,
            &plan(&c),
            &asserted_gate(&["interface"], &["interface"]),
        )
        .await;
    assert!(
        outcome.is_err(),
        "A stop assertion and cancellation request are not persisted settlement: {outcome:?}"
    );
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn selective_entry_never_bypasses_ownership_migration() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "sdk", "running").await;
    value.contracts[0].workstream_id = Id::from(2);
    let c = approved(&store, package(&value)).await;
    let outcome = store
        .activate_workstreams_with_impact(
            TENANT,
            PROJECT,
            "author",
            &c.proposal_id,
            &plan(&c),
            &asserted_gate(&[], &[]),
        )
        .await;
    assert!(
        matches!(outcome, Err(PgError::Unsupported(_))),
        "Every activation entry must enforce immutable ownership: {outcome:?}"
    );
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn changed_undispatched_preparation_is_invalidated_atomically() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    fixture_execution(&admin, "interface", "prepared").await;
    value.contracts[0]
        .contract
        .acceptance
        .push("new interface condition".into());
    let c = approved(&store, package(&value)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
        .await
        .expect(
            "An unexposed preparation can be invalidated without inventing external stop proof",
        );
    let row = admin
        .query_one(
            "SELECT state,cancel_requested,execution_version FROM awr_team.executions",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "cancelled");
    assert_eq!(row.get::<_, i64>(2), 2);
    assert_eq!(current_snapshot(&admin).await, Some(c.snapshot_id));
}

#[tokio::test]
async fn transitive_current_dependency_blocks_activation_of_running_consumer() {
    let (_g, admin, _, store) = setup().await;
    let mut value = bundle();
    let first = approved(&store, package(&value)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &first.proposal_id, &plan(&first))
        .await
        .unwrap();
    fixture_execution(&admin, "integration", "running").await;
    value.contracts[0]
        .contract
        .acceptance
        .push("changed upstream".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams_with_impact(
                TENANT,
                PROJECT,
                "author",
                &c.proposal_id,
                &plan(&c),
                &asserted_gate(&[], &[])
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(current_snapshot(&admin).await, Some(first.snapshot_id));
}

#[tokio::test]
async fn upstream_change_invalidates_old_preparation_with_unchanged_consumer_hash() {
    let (_g, admin, _, store) = setup().await;
    let mut value = bundle();
    let first = approved(&store, package(&value)).await;
    store
        .activate_workstreams(TENANT, PROJECT, "author", &first.proposal_id, &plan(&first))
        .await
        .unwrap();
    let original = value.contracts[1].contract.hash().unwrap();
    fixture_execution(&admin, "sdk", "prepared").await;
    value.contracts[0]
        .contract
        .acceptance
        .push("changed upstream".into());
    let c = approved(&store, package(&value)).await;
    let current = store
        .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
        .await
        .unwrap();
    assert_eq!(current.contract_hashes["sdk"], original);
    let row = admin
        .query_one(
            "SELECT state,execution_version FROM awr_team.executions",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "cancelled");
    assert_eq!(row.get::<_, i64>(1), 2);
    let payload: serde_json::Value = admin.query_one("SELECT payload_json FROM awr_team.events WHERE event_type='source.activated' ORDER BY project_revision DESC LIMIT 1", &[]).await.unwrap().get(0);
    assert_eq!(
        payload["activation_impact"]["affected_work_ids"],
        serde_json::json!(["integration", "interface", "sdk"])
    );
}

#[tokio::test]
async fn terminal_state_without_a_bound_settlement_is_not_stop_proof() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "interface", "failed").await;
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn retained_unknown_reservation_is_not_cleared_by_activation() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    admin.execute("INSERT INTO awr_team.resource_reservations(tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
        VALUES($1,$2,'retained-resource','interface','named','database-write','unknown')", &[&TENANT,&PROJECT]).await.unwrap();
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams_with_impact(
                TENANT,
                PROJECT,
                "author",
                &c.proposal_id,
                &plan(&c),
                &asserted_gate(&["interface"], &["interface"])
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.resource_reservations", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "unknown"
    );
}

#[tokio::test]
async fn failed_install_rolls_back_preparation_and_authority_together() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "interface", "prepared").await;
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    store
        .abort_workstreams_after_installing_projection(
            TENANT,
            PROJECT,
            "author",
            &c.proposal_id,
            &plan(&c),
        )
        .await
        .unwrap();
    assert_eq!(current_snapshot(&admin).await, baseline);
    let row = admin
        .query_one(
            "SELECT state,execution_version FROM awr_team.executions",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "prepared");
    assert_eq!(row.get::<_, i64>(1), 1);
}

#[tokio::test]
async fn missing_archived_ownership_refuses_instead_of_deriving_empty_impact() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    admin
        .execute(
            "DELETE FROM awr_team.workstream_snapshot_ownership WHERE work_id='sdk'",
            &[],
        )
        .await
        .unwrap();
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::SourceDivergence)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn changed_stream_catalog_is_an_independent_impact_seed() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "sdk", "running").await;
    value.catalog.workstreams[1].title = "SDK new instructions".into();
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams_with_impact(
                TENANT,
                PROJECT,
                "author",
                &c.proposal_id,
                &plan(&c),
                &asserted_gate(&[], &[])
            )
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn explicit_refusal_remains_a_compatibility_veto() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams_with_impact(
                TENANT,
                PROJECT,
                "author",
                &c.proposal_id,
                &plan(&c),
                &ActivationImpactGate::default()
            )
            .await,
        Err(PgError::ActivationImpactUnproven(_))
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn missing_enabled_catalog_is_corruption_not_initial_migration() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    admin
        .batch_execute(
            "DELETE FROM awr_team.workstream_snapshot_ownership;
             DELETE FROM awr_team.workstream_catalogs;",
        )
        .await
        .unwrap();
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::SourceDivergence)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn absent_settlement_bindings_cannot_equal_unknown_stored_facts() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "interface", "failed").await;
    admin.execute("UPDATE awr_team.executions SET terminal_reported=true,execution_version=2,observed_paths_json='[]'", &[]).await.unwrap();
    let row = admin
        .query_one(
            "SELECT contract_hash,workstream_id,ownership_version FROM awr_team.executions",
            &[],
        )
        .await
        .unwrap();
    let payload = serde_json::json!({
        "execution_version":"1", "outcome":"failed", "effects_settled":true,
        "contract_hash":row.get::<_,String>(0), "workstream_id":row.get::<_,String>(1),
        "ownership_version":row.get::<_,i64>(2).to_string(), "observed_paths":[],
        "execution_session_id":"impact-session", "execution_coordinator_epoch":"epoch-1",
        "executor_stopped":true,
    });
    // Missing input/environment facts in both the row and receipt are unknown,
    // not proof that the exact admitted effects were settled.
    let hash = awr_team::request_hash(&payload).unwrap();
    admin.execute("INSERT INTO awr_team.execution_receipts(tenant_id,project_id,id,execution_id,reporter_actor_id,receipt_kind,digest,payload_json)
        VALUES($1,$2,'impact-settlement','impact-execution','author','reconcile',$3,$4)", &[&TENANT,&PROJECT,&hash,&payload]).await.unwrap();
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn corrupt_archived_hash_cannot_produce_a_trusted_impact_set() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    admin
        .execute(
            "UPDATE awr_team.work_contracts SET contract_hash=$1 WHERE work_id='sdk'",
            &[&"0".repeat(64)],
        )
        .await
        .unwrap();
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::SourceDivergence)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn database_read_failure_is_not_an_empty_effect_set() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    admin
        .batch_execute("REVOKE SELECT ON awr_team.executions FROM awr_app")
        .await
        .unwrap();
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::Db(_))
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn unattributed_live_effects_cannot_be_treated_as_unrelated() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "sdk", "running").await;
    admin
        .execute(
            "UPDATE awr_team.executions SET workstream_id=NULL,ownership_version=NULL,executor_client_id=NULL",
            &[],
        )
        .await
        .unwrap();
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
}

#[tokio::test]
async fn pending_dispatch_prevents_preparation_invalidation() {
    let (_g, admin, _, store) = setup().await;
    let mut value = install_independent(&store).await;
    let baseline = current_snapshot(&admin).await;
    fixture_execution(&admin, "interface", "prepared").await;
    admin.execute("INSERT INTO awr_team.outbox(tenant_id,project_id,id,state,payload_json,action_kind,aggregate_id)
        VALUES($1,$2,'impact-dispatch','pending','{}','execution.dispatch','impact-execution')", &[&TENANT,&PROJECT]).await.unwrap();
    value.contracts[0]
        .contract
        .acceptance
        .push("changed condition".into());
    let c = approved(&store, package(&value)).await;
    assert!(matches!(
        store
            .activate_workstreams(TENANT, PROJECT, "author", &c.proposal_id, &plan(&c))
            .await,
        Err(PgError::RecoveryBlocked)
    ));
    assert_eq!(current_snapshot(&admin).await, baseline);
    assert_eq!(
        admin
            .query_one("SELECT state FROM awr_team.executions", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "prepared"
    );
}
