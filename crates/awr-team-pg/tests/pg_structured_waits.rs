#![cfg(feature = "pg-tests")]
//! AWR-PPD-050: structured waits on progress checkpoints, read back through
//! `work.snapshot`, `work.observe`, and `work.graph` with legacy `not_provided`
//! compatibility, strict field bounds, non-member `external_party` support, and
//! project/workstream/ownership isolation.
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{PgError, WorkstreamCommand, WorkstreamReadStore};
use fixture::*;
use serde_json::{Value, json};

async fn checkpoint_cmd(
    store: &WorkstreamReadStore,
    request_id: &str,
    session_version: &str,
    progress: Option<Value>,
) -> WorkstreamCommand {
    let p = prepare(store, A, "a").await;
    let mut args = json!({
        "session_id": "session-a",
        "expected_session_version": session_version,
        "context_hash": p["data"]["context_hash"],
        "next_action": "Continue once waiting condition clears",
        "open_loops": []
    });
    if let Some(progress) = progress {
        args["progress"] = progress;
    }
    command(&p, request_id, "session.checkpoint", args)
}

async fn read_snapshot(store: &WorkstreamReadStore, token: &str, work_id: &str) -> Value {
    let mut q = query("work.snapshot");
    q.work_id = Some(work_id.into());
    store.query(TENANT, PROJECT, token, q).await.unwrap()
}

async fn read_observe(store: &WorkstreamReadStore, token: &str, work_id: &str) -> Value {
    let mut q = query("work.observe");
    q.work_id = Some(work_id.into());
    store.query(TENANT, PROJECT, token, q).await.unwrap()
}

async fn read_graph(store: &WorkstreamReadStore, token: &str) -> Value {
    store
        .query(TENANT, PROJECT, token, query("work.graph"))
        .await
        .unwrap()
}

fn graph_item<'a>(graph: &'a Value, work_id: &str) -> &'a Value {
    graph["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["work_id"] == work_id)
        .unwrap_or_else(|| panic!("missing work {work_id} in graph: {graph}"))
}

#[tokio::test]
async fn structured_waits_round_trip_consistently_across_snapshot_observe_and_graph() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;

    // Capabilities advertise structured waits metadata.
    let caps = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    let sw_caps = &caps["session_feedback"]["structured_waits"];
    assert_eq!(sw_caps["field"], "progress.waits");
    assert_eq!(
        sw_caps["party_kinds"],
        json!(["person", "group", "external_party"])
    );
    assert_eq!(sw_caps["max_name_chars"], 80);
    assert_eq!(sw_caps["max_text_chars"], 200);
    assert_eq!(sw_caps["legacy_status"], "not_provided");
    assert_eq!(sw_caps["external_party_requires_membership"], false);
    assert_eq!(sw_caps["grants_authority"], false);

    // Before any progress report exists, snapshot, observe, and graph all report not_provided.
    let before_prepare = prepare(&store, A, "a").await;
    let before_snap = read_snapshot(&store, A, "a").await;
    let before_obs = read_observe(&store, A, "a").await;
    let before_graph = read_graph(&store, A).await;
    for view in [
        &before_snap["data"]["observation"]["structured_waits"],
        &before_obs["data"]["structured_waits"],
        &graph_item(&before_graph, "a")["structured_waits"],
    ] {
        assert_eq!(view["status"], "not_provided");
        assert!(view["items"].is_null());
        assert!(view["checkpoint_id"].is_null());
        assert!(view["reported_at_unix_ms"].is_null());
        assert!(view["contract_matches_current"].is_null());
        assert_eq!(view["provenance"], "caller_declared");
        assert_eq!(view["grants_authority"], false);
    }

    // Write structured waits covering all three party kinds, including a non-member external_party.
    let waits = json!([
        {
            "waiting_on": { "kind": "external_party", "name": "Partner Billing Desk" },
            "what": "Confirm settlement export field format.",
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Partner Billing Desk confirms the column list in writing."
        },
        {
            "waiting_on": { "kind": "person", "name": "Morgan Chen" },
            "what": "Clarify retry backoff window for webhook delivery.",
            "since_unix_ms": 1_731_628_800_000_i64,
            "release_condition": "Backoff bounds confirmed on the decision record."
        },
        {
            "waiting_on": { "kind": "group", "name": "Platform Security" },
            "what": "Approve outbound IP allowlist range.",
            "since_unix_ms": 1_731_715_200_000_i64,
            "release_condition": "Allowlist ticket marked approved."
        }
    ]);
    let cmd = checkpoint_cmd(
        &store,
        "wait-write-1",
        "1",
        Some(json!({
            "phase": "blocked",
            "summary": "Waiting on partner format and security sign-off.",
            "waits": waits
        })),
    )
    .await;
    let committed = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd.clone())
        .await
        .unwrap();
    assert_eq!(committed["execution_authorized"], false);
    let checkpoint_id = committed["receipt"]["data"]["checkpoint_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Idempotent replay returns the exact same receipt.
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, A, cmd)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], committed["receipt"]);

    // Recording structured waits never alters work.prepare context_hash or creates memberships.
    let after_prepare = prepare(&store, A, "a").await;
    assert_eq!(
        after_prepare["data"]["context_hash"],
        before_prepare["data"]["context_hash"]
    );
    let external_members: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.persons WHERE display_name='Partner Billing Desk'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(external_members, 0);

    // work.snapshot, work.observe, and work.graph all read the exact same structured waits.
    let snap = read_snapshot(&store, A, "a").await;
    let obs = read_observe(&store, A, "a").await;
    let graph = read_graph(&store, A).await;
    let snap_waits = &snap["data"]["observation"]["structured_waits"];
    let obs_waits = &obs["data"]["structured_waits"];
    let graph_waits = &graph_item(&graph, "a")["structured_waits"];
    assert_eq!(snap_waits, obs_waits);
    assert_eq!(snap_waits, graph_waits);
    assert_eq!(snap_waits["status"], "provided");
    assert_eq!(snap_waits["items"], waits);
    assert_eq!(snap_waits["checkpoint_id"], checkpoint_id);
    assert!(snap_waits["reported_at_unix_ms"].is_i64());
    assert_eq!(snap_waits["contract_matches_current"], true);
    assert_eq!(snap_waits["provenance"], "caller_declared");
    assert_eq!(snap_waits["grants_authority"], false);

    // Unrelated work "c" in the same stream still reports not_provided.
    let c_graph = &graph_item(&graph, "c")["structured_waits"];
    assert_eq!(c_graph["status"], "not_provided");
    assert!(c_graph["items"].is_null());

    // An ordinary follow-up checkpoint omitting progress preserves the structured wait.
    let ordinary = checkpoint_cmd(&store, "ordinary-cp-2", "2", None).await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, ordinary)
        .await
        .unwrap();
    let after_ordinary_snap = read_snapshot(&store, A, "a").await;
    let after_ordinary_graph = read_graph(&store, A).await;
    assert_eq!(
        after_ordinary_snap["data"]["observation"]["structured_waits"],
        *snap_waits
    );
    assert_eq!(
        graph_item(&after_ordinary_graph, "a")["structured_waits"],
        *snap_waits
    );

    // Explicitly clearing waits with waits: [] produces status "provided" with items: [] (not "not_provided").
    let cleared = checkpoint_cmd(
        &store,
        "clear-waits-3",
        "3",
        Some(json!({
            "phase": "implementing",
            "summary": "All external waits resolved; resuming implementation.",
            "waits": []
        })),
    )
    .await;
    let cleared_res = store
        .commands()
        .execute(TENANT, PROJECT, A, cleared)
        .await
        .unwrap();
    let cleared_cp = cleared_res["receipt"]["data"]["checkpoint_id"]
        .as_str()
        .unwrap();
    let cleared_snap = read_snapshot(&store, A, "a").await;
    let cleared_graph = read_graph(&store, A).await;
    for view in [
        &cleared_snap["data"]["observation"]["structured_waits"],
        &graph_item(&cleared_graph, "a")["structured_waits"],
    ] {
        assert_eq!(view["status"], "provided");
        assert_eq!(view["items"], json!([]));
        assert_eq!(view["checkpoint_id"], cleared_cp);
        assert_eq!(view["grants_authority"], false);
    }
}

#[tokio::test]
async fn legacy_progress_report_without_waits_reads_as_not_provided_not_empty() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;

    // Submit a legacy-shaped progress report (no `waits` key at all).
    let legacy_cmd = checkpoint_cmd(
        &store,
        "legacy-report-1",
        "1",
        Some(json!({
            "phase": "testing",
            "summary": "Legacy progress report without structured waits field.",
            "completed": ["Added unit tests"],
            "blockers": ["Waiting for staging credentials"]
        })),
    )
    .await;
    let committed = store
        .commands()
        .execute(TENANT, PROJECT, A, legacy_cmd)
        .await
        .unwrap();
    let cp_id = committed["receipt"]["data"]["checkpoint_id"]
        .as_str()
        .unwrap();

    // Confirm raw stored progress_json truly omits the `waits` key.
    let stored: Value = admin
        .query_one(
            "SELECT progress_json FROM awr_team.checkpoints WHERE id=$1",
            &[&cp_id],
        )
        .await
        .unwrap()
        .get(0);
    assert!(stored.get("waits").is_none());

    // All three observation reads must report status = "not_provided" and items = null (never []).
    let snap = read_snapshot(&store, A, "a").await;
    let obs = read_observe(&store, A, "a").await;
    let graph = read_graph(&store, A).await;
    for view in [
        &snap["data"]["observation"]["structured_waits"],
        &obs["data"]["structured_waits"],
        &graph_item(&graph, "a")["structured_waits"],
    ] {
        assert_eq!(view["status"], "not_provided");
        assert!(
            view["items"].is_null(),
            "legacy report must return null items, not empty array: {view}"
        );
        assert_eq!(view["checkpoint_id"], cp_id);
        assert!(view["reported_at_unix_ms"].is_i64());
        assert_eq!(view["contract_matches_current"], true);
        assert_eq!(view["grants_authority"], false);
    }
}

#[tokio::test]
async fn structured_wait_boundary_values_and_invalid_kinds_are_rejected_without_writing() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;

    let before_cp = read_observe(&store, A, "a").await["data"]["checkpoint"].clone();

    // Exact maximum bounds (80 Unicode chars for name, 200 Unicode chars for what & release_condition, 8 items) succeed.
    let name_80 = "甲".repeat(80);
    let what_200 = "等".repeat(200);
    let release_200 = "解".repeat(200);
    let max_items: Vec<Value> = (0..8)
        .map(|i| {
            json!({
                "waiting_on": { "kind": "external_party", "name": name_80 },
                "what": what_200,
                "since_unix_ms": 1_731_542_400_000_i64 + i,
                "release_condition": release_200
            })
        })
        .collect();

    // First verify every over-bound and invalid input fails BEFORE writing any checkpoint.
    let invalid_waits = [
        // Name 81 chars
        json!([{
            "waiting_on": { "kind": "external_party", "name": "甲".repeat(81) },
            "what": "Short what",
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Short release"
        }]),
        // What 201 chars
        json!([{
            "waiting_on": { "kind": "person", "name": "Alex" },
            "what": "等".repeat(201),
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Short release"
        }]),
        // Release condition 201 chars
        json!([{
            "waiting_on": { "kind": "group", "name": "Core Ops" },
            "what": "Short what",
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "解".repeat(201)
        }]),
        // Unknown waiting_on.kind
        json!([{
            "waiting_on": { "kind": "organization", "name": "External Org" },
            "what": "Short what",
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Short release"
        }]),
        // Blank name
        json!([{
            "waiting_on": { "kind": "external_party", "name": "   " },
            "what": "Short what",
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Short release"
        }]),
        // Control char in release_condition
        json!([{
            "waiting_on": { "kind": "external_party", "name": "Vendor" },
            "what": "Short what",
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Line 1\nLine 2"
        }]),
        // Non-positive timestamp
        json!([{
            "waiting_on": { "kind": "external_party", "name": "Vendor" },
            "what": "Short what",
            "since_unix_ms": 0,
            "release_condition": "Short release"
        }]),
        // More than 8 wait items
        json!(
            (0..9)
                .map(|i| json!({
                    "waiting_on": { "kind": "external_party", "name": format!("Vendor {i}") },
                    "what": "Approval",
                    "since_unix_ms": 1_731_542_400_000_i64 + i,
                    "release_condition": "Signed"
                }))
                .collect::<Vec<_>>()
        ),
        // Extra unknown field on wait entry
        json!([{
            "waiting_on": { "kind": "external_party", "name": "Vendor" },
            "what": "Approval",
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Signed",
            "role": "admin"
        }]),
        // Secret/credential inside wait text
        json!([{
            "waiting_on": { "kind": "external_party", "name": "Vendor" },
            "what": format!("waiting for ghp_{}", "a".repeat(36)),
            "since_unix_ms": 1_731_542_400_000_i64,
            "release_condition": "Signed"
        }]),
    ];

    for (index, bad) in invalid_waits.into_iter().enumerate() {
        let cmd = checkpoint_cmd(
            &store,
            &format!("bad-wait-{index}"),
            "1",
            Some(json!({
                "phase": "blocked",
                "summary": "Testing wait validation",
                "waits": bad
            })),
        )
        .await;
        let err = store
            .commands()
            .execute(TENANT, PROJECT, A, cmd)
            .await
            .err()
            .unwrap_or_else(|| panic!("invalid wait case {index} must be rejected"));
        assert!(
            matches!(err, PgError::Protocol(_)),
            "unexpected error for case {index}: {err:?}"
        );
        assert_eq!(
            read_observe(&store, A, "a").await["data"]["checkpoint"],
            before_cp
        );
    }

    // Now write the valid max-bounds 8-item wait list and verify exact read-back.
    let valid_cmd = checkpoint_cmd(
        &store,
        "valid-max-bounds",
        "1",
        Some(json!({
            "phase": "blocked",
            "summary": "At exact character and item limits.",
            "waits": max_items
        })),
    )
    .await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, valid_cmd)
        .await
        .unwrap();
    let snap = read_snapshot(&store, A, "a").await;
    let items = snap["data"]["observation"]["structured_waits"]["items"]
        .as_array()
        .unwrap();
    assert_eq!(items.len(), 8);
    assert_eq!(items[0]["waiting_on"]["name"], name_80);
    assert_eq!(items[0]["what"], what_200);
    assert_eq!(items[0]["release_condition"], release_200);
}

#[tokio::test]
async fn structured_waits_never_leak_across_projects_workstreams_or_ownership() {
    let (_guard, admin, _, store) = setup().await;
    enable_writes(&admin).await;

    // Seed a second project under the same tenant with the exact same work_id ("a")
    // and insert a checkpoint with a unique cross-project sentinel wait.
    let foreign_project = "foreign-project";
    let foreign_sentinel = "FOREIGN_PROJECT_ONLY_PARTY_SENTINEL";
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status,active_snapshot_id)
                VALUES('{TENANT}','{foreign_project}','foreign','team','epoch-foreign','active',NULL);
             INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                VALUES('{TENANT}','{foreign_project}','main','main','active');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                VALUES('{TENANT}','{foreign_project}','a','A');
             INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
                VALUES('{TENANT}','{foreign_project}','session-foreign','main','a','agent','cli-a','conv-foreign','active','00000000000000000000000001',1);
             INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json,progress_json)
                VALUES('{TENANT}','{foreign_project}','cp-foreign','session-foreign','{}','{}',999,'Foreign next','[]'::jsonb,
                '{{\"phase\":\"blocked\",\"summary\":\"Foreign wait\",\"waits\":[{{\"waiting_on\":{{\"kind\":\"external_party\",\"name\":\"{foreign_sentinel}\"}},\"what\":\"Foreign secret wait\",\"since_unix_ms\":1731542400000,\"release_condition\":\"Foreign release\"}}]}}'::jsonb);",
            "a".repeat(64),
            "b".repeat(64),
        ))
        .await
        .unwrap();

    // Also put a structured wait on work "b-private" in workstream 2 (unreadable to token A).
    let stream2_sentinel = "STREAM_TWO_PRIVATE_PARTY_SENTINEL";
    admin
        .batch_execute(&format!(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state,workstream_id,ownership_version)
                VALUES('{TENANT}','{PROJECT}','session-b-private','main','b-private','agent','cli-b','conv-b','active','00000000000000000000000002',1);
             INSERT INTO awr_team.checkpoints(tenant_id,project_id,id,session_id,context_hash,contract_hash,observed_revision,next_action,open_loops_json,progress_json)
                VALUES('{TENANT}','{PROJECT}','cp-b-private','session-b-private','{}','{}',500,'Private next','[]'::jsonb,
                '{{\"phase\":\"blocked\",\"summary\":\"Private stream wait\",\"waits\":[{{\"waiting_on\":{{\"kind\":\"external_party\",\"name\":\"{stream2_sentinel}\"}},\"what\":\"Private wait\",\"since_unix_ms\":1731542400000,\"release_condition\":\"Private release\"}}]}}'::jsonb);",
            "c".repeat(64),
            "d".repeat(64),
        ))
        .await
        .unwrap();

    // Reader A in PROJECT sees neither the foreign project's wait nor stream 2's wait.
    let snap = read_snapshot(&store, A, "a").await;
    let obs = read_observe(&store, A, "a").await;
    let graph = read_graph(&store, A).await;
    for reply in [&snap, &obs, &graph] {
        let text = reply.to_string();
        assert!(
            !text.contains(foreign_sentinel),
            "cross-project wait leaked: {text}"
        );
        assert!(
            !text.contains(stream2_sentinel),
            "cross-workstream wait leaked: {text}"
        );
    }
    assert_eq!(
        snap["data"]["observation"]["structured_waits"]["status"],
        "not_provided"
    );
    assert_eq!(
        graph_item(&graph, "a")["structured_waits"]["status"],
        "not_provided"
    );

    // Reader A cannot query foreign project or private workstream directly.
    assert!(matches!(
        store
            .query(TENANT, foreign_project, A, query("work.graph"))
            .await,
        Err(PgError::Forbidden)
    ));
    let mut private_snap = query("work.snapshot");
    private_snap.work_id = Some("b-private".into());
    assert!(matches!(
        store.query(TENANT, PROJECT, A, private_snap).await,
        Err(PgError::Forbidden)
    ));

    // Reader B (authorized for workstream 2) DOES see b-private's wait.
    let b_graph = read_graph(&store, B).await;
    let b_waits = &graph_item(&b_graph, "b-private")["structured_waits"];
    assert_eq!(b_waits["status"], "provided");
    assert_eq!(b_waits["items"][0]["waiting_on"]["name"], stream2_sentinel);
    assert!(!b_graph.to_string().contains(foreign_sentinel));

    // Superseded ownership version waits do not leak into current ownership.
    let own_cmd = checkpoint_cmd(
        &store,
        "ownership-v1-wait",
        "1",
        Some(json!({
            "phase": "blocked",
            "summary": "Wait under ownership v1.",
            "waits": [{
                "waiting_on": { "kind": "external_party", "name": "Old Ownership Party" },
                "what": "Old wait",
                "since_unix_ms": 1_731_542_400_000_i64,
                "release_condition": "Old release"
            }]
        })),
    )
    .await;
    store
        .commands()
        .execute(TENANT, PROJECT, A, own_cmd)
        .await
        .unwrap();
    assert_eq!(
        read_snapshot(&store, A, "a").await["data"]["observation"]["structured_waits"]["status"],
        "provided"
    );
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_ownership SET ownership_version=2 WHERE work_id='a';
             UPDATE awr_team.workstream_snapshot_ownership SET ownership_version=2 WHERE work_id='a';",
        )
        .await
        .unwrap();
    let moved_snap = read_snapshot(&store, A, "a").await;
    let moved_graph = read_graph(&store, A).await;
    assert_eq!(
        moved_snap["data"]["observation"]["structured_waits"]["status"],
        "not_provided"
    );
    assert_eq!(
        graph_item(&moved_graph, "a")["structured_waits"]["status"],
        "not_provided"
    );
    assert!(!moved_snap.to_string().contains("Old Ownership Party"));
    assert!(!moved_graph.to_string().contains("Old Ownership Party"));
}
