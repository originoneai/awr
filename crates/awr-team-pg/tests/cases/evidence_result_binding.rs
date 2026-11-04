// Included in agent_review_tests so the actual review/command fixtures are shared.
// Completion needs one digest in four places to agree: the reported output of the settled execution, the output
// digest the evidence declares, the receipt's, and the sha256 of the stored artifact. The evidence that a review
// round is bound to must satisfy that when the round is opened, not three steps later.
fn open_args(chain: &Value) -> Value {
    json!({"session_id":"session-a","expected_session_version":"1","evidence_id":chain["evidence_id"]})
}

async fn round_count(admin: &Client) -> i64 {
    admin.query_one("SELECT count(*) FROM awr_team.review_rounds", &[]).await.unwrap().get(0)
}

async fn shaped(shape: ChainShape) -> (Client, WorkstreamReadStore, Value, impl Sized) {
    let (guard, admin, db, store) = setup().await;
    let chain = caller_chain_shaped(
        &admin, &db, &store, Some("clone-author"),
        awr_team::ExecutionSettlementMode::IndependentWorkspaceV1, false, shape,
    ).await;
    (admin, store, chain, guard)
}

const NO_DECLARED: &str = "evidence_declares_no_output_digest";
const DECLARED_DIFFERS: &str = "declared_output_digest_differs_from_execution_result";
const ARTIFACT_DIFFERS: &str = "artifact_digest_differs_from_execution_result";

fn binding_refusal(error: PgError, shape: ChainShape) -> (Vec<&'static str>, String, String, String) {
    match error {
        PgError::EvidenceResultBinding { reasons, execution_result, declared, artifact } => (reasons, execution_result, declared, artifact),
        other => panic!("{shape:?}: expected EvidenceResultBinding, got {other:?}"),
    }
}

#[tokio::test]
async fn review_open_refuses_evidence_that_cannot_match_the_reported_execution_result() {
    for (shape, expected) in [
        (ChainShape::ArtifactDiffers, vec![ARTIFACT_DIFFERS]),
        (ChainShape::DeclaresNothing, vec![NO_DECLARED]),
        (ChainShape::DeclaresOther, vec![DECLARED_DIFFERS]),
        // The shape of run 14's page evidence: another artifact and no declared digest. Both are named at once.
        (ChainShape::ArtifactDiffersAndDeclaresNothing, vec![NO_DECLARED, ARTIFACT_DIFFERS]),
    ] {
        let (admin, store, chain, _guard) = shaped(shape).await;
        let error = run_err(&store, A, "open-refused", "review.open", open_args(&chain)).await;
        let (reasons, recorded, declared, artifact) = binding_refusal(error, shape);
        assert_eq!(reasons, expected, "{shape:?}");
        let reported = chain["result_digest"].as_str().unwrap();
        assert_eq!(recorded, &reported[..12], "{shape:?}: the refusal shows the reported digest");
        assert_eq!(declared == "none", matches!(shape, ChainShape::DeclaresNothing | ChainShape::ArtifactDiffersAndDeclaresNothing), "{shape:?}");
        assert_ne!(artifact, "none", "{shape:?}: the artifact is stored in every shape");
        assert_eq!(round_count(&admin).await, 0, "{shape:?}: no review round may exist");
    }
}

#[tokio::test]
async fn evidence_that_binds_opens_a_round_unchanged() {
    let (admin, store, chain, _guard) = shaped(ChainShape::MatchingStopAfterEvidence).await;
    let opened = run(&store, A, "open-ok", "review.open", open_args(&chain)).await;
    assert!(opened["round_id"].is_string());
    assert_eq!(round_count(&admin).await, 1);
}

#[tokio::test]
async fn evidence_submit_reports_how_the_artifact_relates_to_the_reported_result() {
    for (shape, status, reasons) in [
        (ChainShape::MatchingStopAfterEvidence, "matches", vec![]),
        (ChainShape::ArtifactDiffers, "differs", vec![ARTIFACT_DIFFERS]),
        (ChainShape::DeclaresNothing, "differs", vec![NO_DECLARED]),
        (ChainShape::DeclaresOther, "differs", vec![DECLARED_DIFFERS]),
        (ChainShape::ArtifactDiffersAndDeclaresNothing, "differs", vec![NO_DECLARED, ARTIFACT_DIFFERS]),
    ] {
        let (_admin, _store, chain, _guard) = shaped(shape).await;
        let binding = &chain["evidence"]["execution_consistency"]["result_binding"];
        assert_eq!(binding["status"], status, "{shape:?}: {binding}");
        assert_eq!(binding["reasons"], json!(reasons), "{shape:?}: {binding}");
        let reported = chain["result_digest"].as_str().unwrap();
        assert_eq!(binding["execution_result_digest"], &reported[..12], "{shape:?}");
        // The submit itself is never refused: auxiliary evidence of the same execution stays possible.
        assert!(chain["evidence"]["evidence_id"].is_string(), "{shape:?}");
    }
}

#[tokio::test]
async fn review_decide_rechecks_the_result_binding_of_a_round_opened_earlier() {
    let (admin, store, chain, _guard) = shaped(ChainShape::MatchingStopAfterEvidence).await;
    let opened = run(&store, A, "open-ok", "review.open", open_args(&chain)).await;
    // A round that was opened before this rule existed, or whose execution record drifted afterwards.
    admin
        .execute(
            "UPDATE awr_team.executions SET result_digest=$1 WHERE id=$2",
            &[&"f".repeat(64), &chain["execution_id"].as_str().unwrap()],
        )
        .await
        .unwrap();
    let error = run_err(&store, REVIEWER_TOKEN, "decide-refused", "review.decide", args(&opened)).await;
    let (reasons, ..) = binding_refusal(error, ChainShape::MatchingStopAfterEvidence);
    assert_eq!(reasons, vec![DECLARED_DIFFERS, ARTIFACT_DIFFERS]);
    let decisions: i64 = admin.query_one("SELECT count(*) FROM awr_team.review_decisions", &[]).await.unwrap().get(0);
    assert_eq!(decisions, 0, "an approval of a round that can never complete must not be recorded");
}
