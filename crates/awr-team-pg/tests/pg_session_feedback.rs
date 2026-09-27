#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::{PgError, WorkstreamCommand, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};

fn client() -> Value {
    json!({"product":"Example Agent","version":"1.2.3",
        "model":{"id":"example-model","provider":"example","source":"host_metadata"},
        "capabilities":{"model":"supported","usage":"supported","progress":"supported"}})
}
fn progress() -> Value {
    json!({"phase":"testing","summary":"Storage implementation is ready for durability checks.",
        "completed":["Implemented atomic storage writes"],"blockers":[],
        "artifacts":[{"label":"Implementation","reference":"src/store.rs"}],
        "tests":[{"name":"durability","outcome":"not_run"}]})
}
fn usage() -> Value {
    json!({"source":"native_host_event","source_ref":"logs/synthetic-session.jsonl#event-3",
        "counter_id":"host-counter-1","scope":"host_session","coverage":"partial",
        "observed_at_unix_ms":1_700_000_000_000_i64,"input_tokens":4000,"output_tokens":500,"cached_input_tokens":1000})
}
async fn cp(store: &WorkstreamReadStore, request: &str, version: &str) -> WorkstreamCommand {
    let p = prepare(store, A, "a").await;
    command(
        &p,
        request,
        "session.checkpoint",
        json!({"session_id":"session-a","expected_session_version":version,
        "context_hash":p["data"]["context_hash"],"next_action":"Run durability tests","open_loops":[]}),
    )
}
async fn observe(store: &WorkstreamReadStore, token: &str) -> Value {
    let mut q = query("work.observe");
    q.work_id = Some("a".into());
    q.session_id = Some("session-a".into());
    store.query(TENANT, PROJECT, token, q).await.unwrap()["data"].clone()
}

#[tokio::test]
async fn feedback_preserves_identity_context_and_independently_timed_observations() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let before = prepare(&store, A, "a").await;
    let mut cmd = cp(&store, "report-1", "1").await;
    cmd.args["client_info"] = client();
    cmd.args["progress"] = progress();
    cmd.args["usage"] = usage();
    let result = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd.clone())
        .await
        .unwrap();
    assert_eq!(result["execution_authorized"], false);
    let data = observe(&store, A).await;
    assert_eq!(data["session"]["actor_id"], "agent");
    assert_eq!(data["session"]["client_id"], "cli-a");
    assert!(data["responsibility"].is_null());
    assert_eq!(data["client"]["product"], "Example Agent");
    assert_eq!(data["model"]["id"], "example-model");
    assert_eq!(data["progress"]["phase"], "testing");
    assert_eq!(data["progress"]["next_action"], "Run durability tests");
    assert_eq!(data["progress"]["provenance"], "caller_declared");
    assert_eq!(data["progress"]["stale"], false);
    assert_eq!(data["usage"]["stale"], true); // Fresh submission of an old measurement.
    assert_eq!(data["usage"]["input_tokens"], 4000);
    assert_eq!(data["reporting"]["usage_aggregation"], "none");
    assert_eq!(
        before["data"]["context_hash"],
        prepare(&store, A, "a").await["data"]["context_hash"]
    );
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    assert_eq!(replay["receipt"], result["receipt"]);
    // A normal checkpoint preserves the last separately reported facts and times.
    store
        .commands()
        .execute(TENANT, PROJECT, A, cp(&store, "legacy-2", "2").await)
        .await
        .unwrap();
    let later = observe(&store, A).await;
    assert_eq!(data["usage"], later["usage"]);
    assert_eq!(data["progress"], later["progress"]);
    assert_eq!(data["reporting"], later["reporting"]);
    assert_ne!(data["checkpoint"]["id"], later["checkpoint"]["id"]);
    // Business summaries are deliberately WorkRead-visible, unlike raw receipts.
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read)
        VALUES($1,$2,'agent','cli-b',$3,1,true)", &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    assert_eq!(observe(&store, B).await["progress"], data["progress"]);
    let mut private = query("work.observe");
    private.work_id = Some("b-private".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, private).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn legacy_unknown_unsupported_and_missing_reports_are_distinct() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    assert_eq!(
        observe(&store, A).await["missing"]["model"],
        "client_capability_unknown"
    );
    let mut cmd = cp(&store, "capabilities", "1").await;
    cmd.args["client_info"] = json!({"product":"Other Agent","capabilities":{"model":"unsupported","usage":"unsupported","progress":"supported"}});
    store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    let data = observe(&store, A).await;
    assert_eq!(data["missing"]["model"], "client_collection_unsupported");
    assert_eq!(data["missing"]["usage"], "client_collection_unsupported");
    assert_eq!(data["missing"]["progress"], "not_reported_by_client");
    let mut contradictory = cp(&store, "contradictory-usage", "2").await;
    contradictory.args["usage"] = usage();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, contradictory)
            .await,
        Err(PgError::Protocol(_))
    ));
    let prepared = prepare(&store, A, "a").await;
    let started = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "new-host",
                "session.start",
                json!({"conversation_id":"new-host","client_info":client()}),
            ),
        )
        .await
        .unwrap();
    let mut q = query("work.observe");
    q.session_id = started["receipt"]["data"]["session_id"]
        .as_str()
        .map(str::to_owned);
    assert_eq!(
        store.query(TENANT, PROJECT, A, q).await.unwrap()["data"]["client"]["product"],
        "Example Agent"
    );
}

#[tokio::test]
async fn stale_cas_cross_client_forged_authority_and_bad_feedback_do_not_write() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let mut cmd = cp(&store, "valid", "1").await;
    cmd.args["progress"] = progress();
    let before = observe(&store, A).await["checkpoint"].clone();
    for (field, bad) in [
        ("progress", json!({"phase":"succeeded","summary":"Done"})),
        (
            "progress",
            json!({"phase":"testing","summary":"x".repeat(2049)}),
        ),
        (
            "progress",
            json!({"phase":"testing","summary":format!("credential ghp_{}","a".repeat(36))}),
        ),
        ("client_info", json!({"product":"Agent","actor_id":"admin"})),
        (
            "client_info",
            json!({"product":"Agent","model":{"id":"m","source":"guessed"}}),
        ),
        ("usage", json!({"source":"guess","input_tokens":-1})),
    ] {
        let mut bad_cmd = cmd.clone();
        bad_cmd.args[field] = bad;
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, bad_cmd).await,
            Err(PgError::Protocol(_))
        ));
        assert_eq!(observe(&store, A).await["checkpoint"], before);
    }
    admin.execute("INSERT INTO awr_team.workstream_grants(tenant_id,project_id,actor_id,client_id,workstream_id,authority_version,can_read,can_write)
        VALUES($1,$2,'agent','cli-b',$3,1,true,true)", &[&TENANT,&PROJECT,&awr_core::Id::from(1).to_string()]).await.unwrap();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, B, cmd.clone())
            .await,
        Err(PgError::Forbidden)
    ));
    store
        .commands()
        .execute(TENANT, PROJECT, A, cmd.clone())
        .await
        .unwrap();
    cmd.request_id = "stale-version".into();
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, cmd).await,
        Err(PgError::PreconditionsChanged)
    ));
}

#[tokio::test]
async fn host_counters_are_snapshots_and_reject_reordering_decreases_and_future_dates() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let mut first = cp(&store, "usage-first", "1").await;
    first.args["usage"] = usage();
    store
        .commands()
        .execute(TENANT, PROJECT, A, first)
        .await
        .unwrap();
    for (field, value) in [
        ("observed_at_unix_ms", json!(1_699_999_999_999_i64)),
        ("observed_at_unix_ms", json!(9_007_199_254_740_991_i64)),
        ("input_tokens", json!(3999)),
        ("input_tokens", json!(9007199254740992_u64)),
        ("cached_input_tokens", json!(4001)),
        ("scope", json!("task")),
    ] {
        let mut cmd = cp(&store, "invalid-counter", "2").await;
        cmd.args["usage"] = usage();
        cmd.args["usage"][field] = value;
        assert!(matches!(
            store.commands().execute(TENANT, PROJECT, A, cmd).await,
            Err(PgError::Protocol(_))
        ));
    }
    // A repeated identical snapshot is not added to previous usage.
    let mut same = cp(&store, "same-counter", "2").await;
    same.args["usage"] = usage();
    store
        .commands()
        .execute(TENANT, PROJECT, A, same)
        .await
        .unwrap();
    assert_eq!(observe(&store, A).await["usage"]["input_tokens"], 4000);
    let mut reset = cp(&store, "reset-counter", "3").await;
    reset.args["usage"] = usage();
    reset.args["usage"]["counter_id"] = json!("host-counter-2");
    reset.args["usage"]["observed_at_unix_ms"] = json!(1_700_000_000_001_i64);
    reset.args["usage"]["input_tokens"] = json!(1200);
    store
        .commands()
        .execute(TENANT, PROJECT, A, reset)
        .await
        .unwrap();
    assert_eq!(observe(&store, A).await["usage"]["input_tokens"], 1200);
    let mut late_old_counter = cp(&store, "late-old-counter", "4").await;
    late_old_counter.args["usage"] = usage();
    assert!(matches!(
        store
            .commands()
            .execute(TENANT, PROJECT, A, late_old_counter)
            .await,
        Err(PgError::Protocol(_))
    ));
}

#[tokio::test]
async fn migration_34_preserves_legacy_checkpoints_and_can_be_reapplied() {
    let (_g, admin, _) = common::historical_team_schema(33).await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES('reader-tenant','Readers','active');
        INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status) VALUES('reader-tenant','reader-project','p','team','epoch-a','active');
        INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,latest_checkpoint_id)
          VALUES('reader-tenant','reader-project','legacy-session','main','a','agent','cli-a','conversation','active','legacy-checkpoint');
        INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json)
          VALUES('reader-tenant','reader-project','legacy-checkpoint','legacy-session','consumed-context','accepted-contract',7,'Verify the saved result','[\"pending verification\"]')",
        )
        .await
        .unwrap();
    let before: Value = admin
        .query_one("SELECT to_jsonb(c) FROM awr_team.checkpoints c", &[])
        .await
        .unwrap()
        .get(0);
    let session_before: Value = admin
        .query_one("SELECT to_jsonb(s) FROM awr_team.sessions s", &[])
        .await
        .unwrap()
        .get(0);
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::migrate(&admin).await.unwrap();
    awr_team_pg::check_schema(&admin).await.unwrap();
    let after = admin.query_one("SELECT to_jsonb(c)-'progress_json'-'usage_json', progress_json IS NULL AND usage_json IS NULL FROM awr_team.checkpoints c", &[]).await.unwrap();
    assert_eq!(after.get::<_, Value>(0), before);
    assert!(after.get::<_, bool>(1));
    let session_after = admin.query_one("SELECT to_jsonb(s)-'client_info_json'-'client_info_at', client_info_json IS NULL AND client_info_at IS NULL FROM awr_team.sessions s", &[]).await.unwrap();
    assert_eq!(session_after.get::<_, Value>(0), session_before);
    assert!(session_after.get::<_, bool>(1));
}

#[tokio::test]
async fn missing_counter_dimensions_cannot_bridge_a_decrease() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let mut first = cp(&store, "first", "1").await;
    first.args["usage"] = usage();
    store
        .commands()
        .execute(TENANT, PROJECT, A, first)
        .await
        .unwrap();
    let mut partial = cp(&store, "partial", "2").await;
    partial.args["usage"] = usage();
    partial.args["usage"]["input_tokens"] = Value::Null;
    partial.args["usage"]["cached_input_tokens"] = Value::Null;
    partial.args["usage"]["observed_at_unix_ms"] = json!(1_700_000_000_001_i64);
    store
        .commands()
        .execute(TENANT, PROJECT, A, partial)
        .await
        .unwrap();
    for (field, value) in [("input_tokens", 3000), ("cached_input_tokens", 900)] {
        let mut decreased = cp(&store, "decreased", "3").await;
        decreased.args["usage"] = usage();
        decreased.args["usage"]["observed_at_unix_ms"] = json!(1_700_000_000_002_i64);
        decreased.args["usage"][field] = json!(value);
        assert!(matches!(
            store
                .commands()
                .execute(TENANT, PROJECT, A, decreased)
                .await,
            Err(PgError::Protocol(_))
        ));
    }
}
