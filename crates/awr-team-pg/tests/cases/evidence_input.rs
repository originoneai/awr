use super::*;
use sha2::{Digest, Sha256};

fn evidence_args() -> Value {
    json!({"session_id":"session-a","expected_session_version":"1",
        "dirty_tree":false,"payload":{"passed":true,"output_digest":RESULT}})
}

#[tokio::test]
async fn text_and_hex_round_trip_with_identical_binding_and_scoped_reads() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let text = "  {\"summary\":\"验证 🦀\",\"note\":\"e\u{301}\"}\r\n";
    let expected = format!("{:x}", Sha256::digest(text.as_bytes()));
    let mut receipts = Vec::new();
    for (field, content) in [
        ("artifact_text", text.to_owned()),
        ("artifact_hex", hex_encode(text.as_bytes()).to_uppercase()),
    ] {
        let mut args = evidence_args();
        args[field] = json!(content);
        let receipt = run(&store, A, field, "evidence.submit", args).await;
        assert_eq!(receipt["artifact_digest"], expected);
        assert_eq!(receipt["human_approval"], false);
        assert_eq!(receipt["task_complete"], false);

        let mut inspect = query("evidence.inspect");
        inspect.work_id = Some("a".into());
        inspect.evidence_id = Some(receipt["evidence_id"].as_str().unwrap().into());
        let evidence = store.query(TENANT, PROJECT, A, inspect).await.unwrap();
        assert_eq!(
            evidence["data"]["evidence"]["artifact_id"],
            receipt["artifact_id"]
        );
        assert_eq!(evidence["data"]["evidence"]["artifact_digest"], expected);

        let mut read = query("artifact.content");
        read.work_id = Some("a".into());
        read.artifact_id = Some(receipt["artifact_id"].as_str().unwrap().into());
        read.expected_sha256 = Some(expected.clone());
        let content = store.query(TENANT, PROJECT, A, read.clone()).await.unwrap();
        assert_eq!(content["data"]["text"], text);
        assert_eq!(content["data"]["byte_length"], text.len());
        assert!(matches!(
            store.query(TENANT, PROJECT, B, read.clone()).await,
            Err(PgError::Forbidden)
        ));
        read.work_id = Some("c".into());
        assert!(matches!(
            store.query(TENANT, PROJECT, A, read.clone()).await,
            Err(PgError::Forbidden)
        ));
        read.work_id = Some("a".into());
        read.expected_sha256 = Some("0".repeat(64));
        assert!(matches!(
            store.query(TENANT, PROJECT, A, read).await,
            Err(PgError::SnapshotDrift(_))
        ));
        receipts.push(receipt);
    }
    assert_eq!(receipts[0]["digest"], receipts[1]["digest"]);
    assert_eq!(receipts[0]["trust_basis"], receipts[1]["trust_basis"]);
    assert_ne!(receipts[0]["artifact_id"], receipts[1]["artifact_id"]);
}

#[tokio::test]
async fn invalid_encodings_leave_no_evidence_or_artifacts_and_legacy_notes_still_work() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    // The shared source fixture already contains an artifact; compare deltas.
    let counts = "SELECT (SELECT count(*) FROM awr_team.evidence),
                         (SELECT count(*) FROM awr_team.artifacts)";
    let before = admin.query_one(counts, &[]).await.unwrap();
    for (index, encoding) in [
        json!({"artifact_text":"a","artifact_hex":"61"}),
        json!({"artifact_text":"","artifact_hex":""}),
        json!({"artifact_hex":"7r"}),
        json!({"artifact_hex":"a"}),
        json!({"artifact_text":17}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut args = evidence_args();
        args.as_object_mut()
            .unwrap()
            .extend(encoding.as_object().unwrap().clone());
        let error = run_err(
            &store,
            A,
            &format!("invalid-{index}"),
            "evidence.submit",
            args,
        )
        .await;
        assert!(matches!(error, PgError::Protocol(_)));
    }
    let after = admin.query_one(counts, &[]).await.unwrap();
    for (index, table) in ["evidence", "artifacts"].into_iter().enumerate() {
        assert_eq!(
            after.get::<_, i64>(index),
            before.get::<_, i64>(index),
            "invalid submission must not persist {table}"
        );
    }
    for (index, payload) in [json!("plain report"), json!(["note", 1]), Value::Null]
        .into_iter()
        .enumerate()
    {
        let mut args = evidence_args();
        args["payload"] = payload;
        let evidence = run(
            &store,
            A,
            &format!("legacy-{index}"),
            "evidence.submit",
            args,
        )
        .await;
        assert!(evidence["artifact_id"].is_null());
        assert!(evidence["artifact_digest"].is_null());
        assert_eq!(evidence["task_complete"], false);
    }
}

#[tokio::test]
async fn typed_delivery_evidence_shares_the_existing_submission_storage_path() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let payload = json!({
        "delivery_candidate_digest": "c".repeat(64),
        "passed": true,
        "output_digest": RESULT,
        "note": "验证 🦀",
        "labels": ["delivery", "typed"]
    });
    let text = "typed delivery report\n";
    let typed = run(
        &store,
        A,
        "typed-evidence",
        "delivery.evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
            "dirty_tree":false,"artifact_text":text,"delivery_payload":payload.clone()}),
    )
    .await;
    assert_eq!(typed["review_state_basis"], "at_commit");
    let generic = run(
        &store,
        A,
        "generic-evidence",
        "evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
            "dirty_tree":false,"artifact_text":text,"payload":payload.clone()}),
    )
    .await;
    // The typed entry only re-labels the argument; hashing, trust semantics and
    // artifact storage come from the same shared submission path.
    assert_eq!(typed["digest"], generic["digest"]);
    assert_eq!(typed["artifact_digest"], generic["artifact_digest"]);
    assert_eq!(typed["trust_basis"], generic["trust_basis"]);
    assert_eq!(typed["contract_hash"], generic["contract_hash"]);
    assert_ne!(typed["evidence_id"], generic["evidence_id"]);
    let stored: Value = admin
        .query_one(
            "SELECT payload_json FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &typed["evidence_id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored, payload);
}

#[tokio::test]
async fn malformed_delivery_payloads_record_nothing_and_the_corrected_request_succeeds() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let digest = "c".repeat(64);
    let counts = "SELECT (SELECT count(*) FROM awr_team.evidence),
                         (SELECT count(*) FROM awr_team.artifacts),
                         (SELECT count(*) FROM awr_team.events),
                         (SELECT count(*) FROM awr_team.operations)";
    let before_counts = admin.query_one(counts, &[]).await.unwrap();
    let before = rework_business_snapshot(&admin).await;
    for (index, delivery_payload) in [
        json!(format!(
            "{{\"delivery_candidate_digest\":\"{digest}\",\"note\":\"private-payload-sentinel\"}}"
        )),
        Value::Null,
        json!([]),
        json!({}),
        json!({"delivery_candidate_digest":"C".repeat(64)}),
        json!({"delivery_candidate_digest":digest,"passed":"private-payload-sentinel"}),
        json!({"delivery_candidate_digest":digest,"output_digest":"not-hex"}),
    ]
    .into_iter()
    .enumerate()
    {
        let error = run_err(
            &store,
            A,
            &format!("bad-payload-{index}"),
            "delivery.evidence.submit",
            json!({"session_id":"session-a","expected_session_version":"1",
                "dirty_tree":false,"delivery_payload":delivery_payload}),
        )
        .await;
        assert!(error.is_delivery_payload_invalid(), "{error:?}");
        let diagnostic = error.delivery_payload_invalid_diagnostic().unwrap();
        assert_eq!(diagnostic["code"], "InvalidInput");
        assert_eq!(diagnostic["reason"], "delivery_payload_invalid");
        assert!(serde_json::to_vec(&diagnostic).unwrap().len() < 1024);
        assert!(!error.to_string().contains("private-payload-sentinel"));
        assert!(!diagnostic.to_string().contains("private-payload-sentinel"));
    }
    // The generic payload alias is not accepted by the typed entry.
    let error = run_err(
        &store,
        A,
        "bad-payload-alias",
        "delivery.evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
            "dirty_tree":false,"payload":{"delivery_candidate_digest":digest}}),
    )
    .await;
    assert!(error.is_delivery_payload_invalid(), "{error:?}");
    // Definite rejections persist nothing: no evidence, artifact, event or
    // operation rows, and no business or revision movement.
    let after_counts = admin.query_one(counts, &[]).await.unwrap();
    for (index, table) in ["evidence", "artifacts", "events", "operations"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            after_counts.get::<_, i64>(index),
            before_counts.get::<_, i64>(index),
            "invalid delivery_payload must not persist {table}"
        );
    }
    assert_eq!(rework_business_snapshot(&admin).await, before);
    // A rejected request recorded no operation; the same request id corrected
    // to a valid payload executes for real.
    let corrected = run(
        &store,
        A,
        "bad-payload-0",
        "delivery.evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
            "dirty_tree":false,"delivery_payload":{"delivery_candidate_digest":digest}}),
    )
    .await;
    assert!(corrected["evidence_id"].is_string());
    assert_eq!(corrected["review_state_basis"], "at_commit");
}

#[tokio::test]
async fn typed_delivery_evidence_replay_is_exact_and_changed_intent_conflicts() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let counts = "SELECT (SELECT count(*) FROM awr_team.evidence),
                         (SELECT count(*) FROM awr_team.artifacts),
                         (SELECT count(*) FROM awr_team.events),
                         (SELECT count(*) FROM awr_team.operations)";
    let revision = "SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2";
    let args = || {
        json!({"session_id":"session-a","expected_session_version":"1",
            "dirty_tree":false,"artifact_text":"replay report\n",
            "delivery_payload":{"delivery_candidate_digest":"c".repeat(64),
                "passed":true,"output_digest":RESULT}})
    };
    let prepared = prepare(&store, A, "a").await;
    let request = command(
        &prepared,
        "typed-replay",
        "delivery.evidence.submit",
        args(),
    );
    let first = store
        .commands()
        .execute(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    assert_eq!(first["replayed"], false);
    let once_counts = admin.query_one(counts, &[]).await.unwrap();
    let once_revision: i64 = admin
        .query_one(revision, &[&TENANT, &PROJECT])
        .await
        .unwrap()
        .get(0);
    // An exact replay returns the historical receipt and records nothing new.
    let replay = store
        .commands()
        .execute(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], first["receipt"]);
    let after_counts = admin.query_one(counts, &[]).await.unwrap();
    for (index, table) in ["evidence", "artifacts", "events", "operations"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            after_counts.get::<_, i64>(index),
            once_counts.get::<_, i64>(index),
            "exact replay must not persist another {table} row"
        );
    }
    assert_eq!(
        admin
            .query_one(revision, &[&TENANT, &PROJECT])
            .await
            .unwrap()
            .get::<_, i64>(0),
        once_revision,
        "exact replay must not advance the project revision"
    );
    // A changed payload under the same successful request id conflicts and
    // mutates nothing.
    let mut changed = request;
    changed.args["delivery_payload"] =
        json!({"delivery_candidate_digest":"d".repeat(64),"passed":true,"output_digest":RESULT});
    assert!(matches!(
        store.commands().execute(TENANT, PROJECT, A, changed).await,
        Err(PgError::IdempotencyConflict)
    ));
    let final_counts = admin.query_one(counts, &[]).await.unwrap();
    for (index, table) in ["evidence", "artifacts", "events", "operations"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            final_counts.get::<_, i64>(index),
            once_counts.get::<_, i64>(index),
            "conflicting retry must not persist {table}"
        );
    }
    assert_eq!(
        admin
            .query_one(revision, &[&TENANT, &PROJECT])
            .await
            .unwrap()
            .get::<_, i64>(0),
        once_revision
    );
}

#[tokio::test]
async fn generic_and_typed_evidence_share_session_and_actor_rejections_without_mutation() {
    let (_g, admin, _, store) = setup().await;
    enable_writes(&admin).await;
    let before = rework_business_snapshot(&admin).await;
    for (index, op) in ["evidence.submit", "delivery.evidence.submit"]
        .into_iter()
        .enumerate()
    {
        let field = if op == "evidence.submit" {
            "payload"
        } else {
            "delivery_payload"
        };
        let base = || {
            json!({"session_id":"session-a","expected_session_version":"1","dirty_tree":false,
                "payload":{"passed":true,"output_digest":RESULT},
                "delivery_payload":{"delivery_candidate_digest":"c".repeat(64),
                    "passed":true,"output_digest":RESULT}})
        };
        let mut args = base();
        // Keep only this operation's declared payload field.
        args.as_object_mut().unwrap().remove(if field == "payload" {
            "delivery_payload"
        } else {
            "payload"
        });
        // A session owned by another actor/client/work is not usable.
        let mut wrong_session = args.clone();
        wrong_session["session_id"] = json!("session-b");
        assert!(matches!(
            run_err(
                &store,
                A,
                &format!("parity-{index}-wrong-session"),
                op,
                wrong_session
            )
            .await,
            PgError::Forbidden
        ));
        // A stale session version keeps its own precondition error.
        let mut stale = args.clone();
        stale["expected_session_version"] = json!("2");
        assert!(matches!(
            run_err(&store, A, &format!("parity-{index}-stale"), op, stale).await,
            PgError::PreconditionsChanged
        ));
        // A denied actor cannot submit against this session or scoped work;
        // preconditions come from an independent permitted prepare.
        let p = prepare(&store, A, "a").await;
        assert!(matches!(
            store
                .commands()
                .execute(
                    TENANT,
                    PROJECT,
                    B,
                    command(&p, &format!("parity-{index}-denied"), op, args)
                )
                .await,
            Err(PgError::Forbidden)
        ));
    }
    assert_eq!(rework_business_snapshot(&admin).await, before);
}
