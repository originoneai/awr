//! AWR-TMCP-011: per-action authorization on real PG command/query paths.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_core::Id;
use awr_team_pg::{PgError, WorkstreamCommandStore};
use fixture::*;
use serde_json::json;

#[tokio::test]
async fn reader_cannot_claim_or_start_session_via_any_command_variant() {
    let (_guard, admin, _db, store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='reader', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    for (op, args) in [
        ("session.start", json!({"conversation_id":"c1"})),
        (
            "claim.acquire",
            json!({
                "session_id":"missing","expected_session_version":"1",
                "expected_work_version":"0","ttl_seconds":60
            }),
        ),
        (
            "execution.prepare",
            json!({
                "session_id":"missing","expected_session_version":"1",
                "claim_id":"missing",
                "expected_fence":"1",
                "expected_lease_version":"1",
                "expected_work_version":"1",
                "input_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "declared_scope":["src/api"]
            }),
        ),
    ] {
        let err = commands
            .execute(
                TENANT,
                PROJECT,
                A,
                command(&prepared, &format!("r-{op}"), op, args),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PgError::Forbidden), "{op} => {err:?}");
    }
}

#[tokio::test]
async fn worker_may_maintain_session_but_planning_ops_stay_unsupported() {
    let (_guard, admin, _db, store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='worker', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let started = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "worker-session",
                "session.start",
                json!({"conversation_id":"conv-worker"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(started["replayed"], false);
    assert!(started["receipt"]["data"]["session_id"].is_string());

    // Planning/access are not workstream commands yet; shared gate still maps them
    // and the command dispatcher refuses unsupported capabilities with no write.
    let err = commands
        .execute(TENANT, PROJECT, A, {
            let mut c = command(
                &prepared,
                "worker-publish",
                "session.start",
                json!({"conversation_id":"other"}),
            );
            // Force an unsupported op through deserialization bypass by rebuilding.
            c.op = "planning.publish".into();
            c
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Unsupported(_)),
        "planning.publish => {err:?}"
    );
}

#[tokio::test]
async fn exact_replay_reuses_receipt_after_permission_still_valid() {
    let (_guard, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let commands: WorkstreamCommandStore = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let req = command(
        &prepared,
        "replay-1",
        "session.start",
        json!({"conversation_id":"conv-replay"}),
    );
    let first = commands
        .execute(TENANT, PROJECT, A, req.clone())
        .await
        .unwrap();
    assert_eq!(first["replayed"], false);
    let second = commands.execute(TENANT, PROJECT, A, req).await.unwrap();
    assert_eq!(second["replayed"], true);
    assert_eq!(second["receipt"], first["receipt"]);
}

#[tokio::test]
async fn revoked_credential_cannot_mutate_and_search_stays_scoped() {
    let (_guard, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let _ = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "before-revoke",
                "session.start",
                json!({"conversation_id":"conv-rev"}),
            ),
        )
        .await
        .unwrap();
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
        )
        .await
        .unwrap();
    let err = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "after-revoke",
                "session.start",
                json!({"conversation_id":"conv-rev-2"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden));

    // Client B remains authorized for its own stream reads; search does not leak
    // stream-1 private works, and capabilities stay navigation-only metadata.
    let mut q = query("work.search");
    q.workstream_id = Some(Id::from(2));
    q.search = Some("alpha".into());
    let listed = store.query(TENANT, PROJECT, B, q).await.unwrap();
    let blob = listed.to_string();
    assert!(!blob.contains("\"external_key\":\"a\""));
    assert!(!blob.contains("\"work_id\":\"a\""));
    let caps = store
        .query(TENANT, PROJECT, B, query("capabilities"))
        .await
        .unwrap();
    assert_eq!(caps["action_authorization"], "tmcp_010_shared_decision");
    assert_eq!(caps["permission_policy_id"], "awr-team-mcp-permission-v2");
}

#[tokio::test]
async fn changed_intent_on_same_request_id_is_conflict_not_new_write() {
    let (_guard, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let first = command(
        &prepared,
        "intent-1",
        "session.start",
        json!({"conversation_id":"conv-a"}),
    );
    commands.execute(TENANT, PROJECT, A, first).await.unwrap();
    let changed = command(
        &prepared,
        "intent-1",
        "session.start",
        json!({"conversation_id":"conv-b"}),
    );
    let err = commands
        .execute(TENANT, PROJECT, A, changed)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::IdempotencyConflict));
}
