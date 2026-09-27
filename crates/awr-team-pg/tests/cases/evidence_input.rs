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
