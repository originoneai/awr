//! Synthetic authenticated PG regressions; no native business acceptance credit.
use super::*;
use awr_team::delivery::{ArtifactEntry, ArtifactManifest, DeliveryCandidate, FactSource};
use awr_team_pg::{
    ConfigureDeliveryConnector, DeliveryConnectorMapping, DeliveryReadSet, DeliverySyncStore,
    ReviewStore, SelectDeliveryCandidate,
};
use sha2::{Digest, Sha256};

const CODE: &str = "export const filter = (records) => records.filter(x => x.open);\n";
const REPORT: &str = "The filter preserves input records. Two fixture tests passed.\n";

fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn read_set(p: &Value) -> DeliveryReadSet {
    serde_json::from_value(json!({"work_id":"a","workstream_id":p["workstream_id"],
        "coordinator_epoch":p["coordinator_epoch"],"source_snapshot_id":p["source_snapshot_id"],
        "authority_version":p["authority_version"],"ownership_version":p["data"]["ownership_version"],
        "contract_hash":p["data"]["contract_hash"]})).unwrap()
}

async fn connector(
    admin: &Client,
    db: &str,
    reads: &WorkstreamReadStore,
    request: &str,
    version: &str,
    enabled: bool,
) -> DeliverySyncStore {
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants SET can_manage=true WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
    let store = DeliverySyncStore::from_config(common::with_app_role(&common::test_config(), db));
    let p = prepare(reads, A, "a").await;
    store
        .configure_connector(
            TENANT,
            PROJECT,
            A,
            ConfigureDeliveryConnector {
                request_id: request.into(),
                read_set: read_set(&p),
                expected_connector_version: version.into(),
                mapping: DeliveryConnectorMapping {
                    connector_id: "shared-delivery".into(),
                    provider: "local_git".into(),
                    resource: "fixture://shared-repository".into(),
                    principal_actor_id: "runner".into(),
                    principal_client_id: "cli-runner".into(),
                    fact_source: FactSource::AdapterObservation,
                    enabled,
                },
            },
        )
        .await
        .unwrap();
    store
}

async fn claim(reads: &WorkstreamReadStore) -> Value {
    let p = prepare(reads, A, "a").await;
    reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "claim-submission",
                "task.claim_available",
                json!({"session_id":"session-a","expected_session_version":"1","ttl_seconds":3600,
            "expected_responsibility_version":p["data"]["responsibility"]["version"],
            "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0")}),
            ),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone()
}

async fn select(
    admin: &Client,
    reads: &WorkstreamReadStore,
    delivery: &DeliverySyncStore,
) -> SelectDeliveryCandidate {
    let manifest = ArtifactManifest {
        entries: vec![
            ArtifactEntry {
                artifact_id: "source-code".into(),
                sha256: sha(CODE),
                byte_length: CODE.len().to_string(),
                locator: "git-blob:src/filter.js".into(),
            },
            ArtifactEntry {
                artifact_id: "test-report".into(),
                sha256: sha(REPORT),
                byte_length: REPORT.len().to_string(),
                locator: "git-blob:reports/result.txt".into(),
            },
        ],
    };
    select_manifest(admin, reads, delivery, manifest).await
}

async fn select_manifest(
    admin: &Client,
    reads: &WorkstreamReadStore,
    delivery: &DeliverySyncStore,
    manifest: ArtifactManifest,
) -> SelectDeliveryCandidate {
    let claim = claim(reads).await;
    let p = prepare(reads, A, "a").await;
    let set = read_set(&p);
    let contract = current_contract(admin).await;
    let candidate: DeliveryCandidate = serde_json::from_value(json!({"binding":{
        "tenant_id":TENANT,"project_id":PROJECT,"scope_id":"main","workstream_id":set.workstream_id,
        "work_id":"a","candidate_id":"submission","candidate_version":"1","contract_hash":set.contract_hash,
        "manifest_digest":manifest.digest().unwrap(),"source_revision":{"resource":"fixture://shared-repository","format":"git_sha256","value":"a".repeat(64)},
        "required_checks":contract.verification_requirements,"target":{"resource":"fixture://shared-repository","reference":"main","precondition":{"kind":"missing"}}
    },"manifest":manifest})).unwrap();
    let request = SelectDeliveryCandidate {
        request_id: "select-submission".into(),
        read_set: set,
        expected_selected_digest: None,
        session_id: "session-a".into(),
        claim_id: claim["claim_id"].as_str().unwrap().into(),
        fence: claim["fence"].as_str().unwrap().into(),
        lease_version: claim["lease_version"].as_str().unwrap().into(),
        candidate,
    };
    delivery
        .select_candidate(TENANT, PROJECT, A, request.clone())
        .await
        .unwrap();
    request
}

async fn evidence(
    reads: &WorkstreamReadStore,
    key: &str,
    text: &str,
    candidate: Option<&str>,
) -> Value {
    let mut payload = json!({"passed":true,"output_digest":sha(text)});
    if let Some(value) = candidate {
        payload["delivery_candidate_digest"] = json!(value);
    }
    run(
        reads,
        A,
        key,
        "evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
        "dirty_tree":false,"artifact_text":text,"payload":payload}),
    )
    .await
}

fn opening(evidence: &Value) -> Value {
    json!({"session_id":"session-a","expected_session_version":"1","evidence_id":evidence["evidence_id"]})
}

async fn inspect_round(reads: &WorkstreamReadStore, round: &Value) -> Value {
    let mut q = query("review.inspect");
    q.work_id = Some("a".into());
    q.review_round_id = Some(round["round_id"].as_str().unwrap().into());
    reads
        .query(TENANT, PROJECT, REVIEWER_TOKEN, q)
        .await
        .unwrap()["data"]["review"]
        .clone()
}

fn decision(round: &Value, outcome: &str) -> Value {
    json!({"session_id":"session-reviewer","expected_session_version":"1",
        "round_id":round["round_id"],"decision":outcome,"reason":"Reviewed the exact submitted artifacts"})
}

#[tokio::test]
async fn configured_and_disabled_delivery_refuse_report_only_review_atomically() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    connector(&admin, &db, &reads, "configure", "0", true).await;
    let report = evidence(&reads, "report-only", REPORT, None).await;
    for (i, op) in ["review.open", "delivery.submit_and_request_review"]
        .into_iter()
        .enumerate()
    {
        let before = rework_business_snapshot(&admin).await;
        let err = run_err(&reads, A, &format!("missing-{i}"), op, opening(&report)).await;
        assert!(err.is_review_submission_incomplete());
        assert_eq!(rework_business_snapshot(&admin).await, before);
    }
    connector(&admin, &db, &reads, "disable", "1", false).await;
    assert!(
        run_err(&reads, A, "disabled-open", "review.open", opening(&report))
            .await
            .is_review_submission_incomplete()
    );
    let mut q = query("delivery.neutral.inspect");
    q.work_id = Some("a".into());
    let state = reads.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    assert_eq!(state["submission"]["delivery_required"], true);
    assert_eq!(state["submission"]["reason"], "candidate_missing");
    assert_eq!(
        state["submission"]["candidate_context"]["contract_hash"],
        report["contract_hash"]
    );
    assert_eq!(
        state["submission"]["guidance"]["action"]["op"],
        "delivery.neutral.inspect"
    );
    assert_eq!(state["acceptance_ready"], false);
}

#[tokio::test]
async fn review_needs_every_bound_manifest_artifact_and_returns_exact_authorized_queries() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    let selection = select(&admin, &reads, &delivery).await;
    let binding = selection.candidate.binding.digest().unwrap();
    let report = evidence(&reads, "report", REPORT, Some(&binding)).await;
    evidence(&reads, "wrong-source-binding", CODE, Some(&"f".repeat(64))).await;
    assert!(
        run_err(&reads, A, "missing-code", "review.open", opening(&report))
            .await
            .is_review_submission_incomplete()
    );
    evidence(&reads, "code", CODE, Some(&binding)).await;
    let round = run(
        &reads,
        A,
        "open",
        "delivery.submit_and_request_review",
        opening(&report),
    )
    .await;
    let before = rework_business_snapshot(&admin).await;
    let review = inspect_round(&reads, &round).await;
    let submission = &review["submission"];
    assert_eq!(submission["inspectable"], true);
    assert_eq!(submission["candidate_current"], true);
    assert_eq!(submission["candidate_digest"], binding);
    assert_eq!(submission["repository_verification_inferred"], false);
    assert_eq!(submission["artifacts"].as_array().unwrap().len(), 2);
    for (reference, expected) in submission["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .zip([CODE, REPORT])
    {
        let q: awr_team_pg::WorkstreamQuery =
            serde_json::from_value(reference["query"].clone()).unwrap();
        let content = reads
            .query(TENANT, PROJECT, REVIEWER_TOKEN, q.clone())
            .await
            .unwrap();
        assert_eq!(content["data"]["text"], expected);
        assert_eq!(content["data"]["path_bypass"], false);
        assert!(reads.query(TENANT, PROJECT, NONE, q.clone()).await.is_err());
        assert!(reads.query(TENANT, PROJECT, B, q).await.is_err());
    }
    assert_eq!(rework_business_snapshot(&admin).await, before);
    let serialized = submission.to_string();
    assert!(!serialized.contains(CODE.trim()));
    assert!(!serialized.contains("git-blob:"));
    let approved = run(
        &reads,
        REVIEWER_TOKEN,
        "approve",
        "review.decide",
        decision(&round, "approve"),
    )
    .await;
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["task_complete"], false);
    let mut limited = query("review.inspect");
    limited.work_id = Some("a".into());
    limited.review_round_id = Some(round["round_id"].as_str().unwrap().into());
    limited.max_context_bytes = Some(64);
    assert!(matches!(
        reads.query(TENANT, PROJECT, REVIEWER_TOKEN, limited).await,
        Err(PgError::ResponseTooLarge)
    ));
}

#[tokio::test]
async fn original_large_stored_text_and_binary_have_readable_bounded_authorized_selectors() {
    use awr_team_pg::WorkstreamQuery;
    for length in [
        WorkstreamQuery::MAX_ARTIFACT_BYTES,
        WorkstreamQuery::MAX_ARTIFACT_BYTES + 1,
    ] {
        let (_guard, admin, db, reads) = setup().await;
        seed_review_actors(&admin).await;
        let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
        let mut text = "é".repeat(length / 2);
        if length % 2 != 0 {
            text.push('x');
        }
        let binary = vec![0xff; length];
        let escaped = "\u{0001}".repeat(length);
        let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
        let contents = [
            text.as_bytes(),
            escaped.as_bytes(),
            binary.as_slice(),
            REPORT.as_bytes(),
        ];
        let manifest = ArtifactManifest {
            entries: contents
                .iter()
                .enumerate()
                .map(|(i, bytes)| ArtifactEntry {
                    artifact_id: format!("stored-{i}"),
                    sha256: digest(bytes),
                    byte_length: bytes.len().to_string(),
                    locator: format!("fixture://stored-{i}"),
                })
                .collect(),
        };
        let selection = select_manifest(&admin, &reads, &delivery, manifest).await;
        let binding = selection.candidate.binding.digest().unwrap();
        // Seed historical evidence through its real legacy store while scopes
        // are disabled, then restore scoped mode before any review or read.
        // Legacy writes remain forbidden when enabled. The authenticated 64 KiB
        // command envelope is unchanged; this is not an MCP upload test.
        admin
            .batch_execute("UPDATE awr_team.workstream_modes SET enabled=false")
            .await
            .unwrap();
        let legacy = ReviewStore::from_config(common::with_app_role(&common::test_config(), &db));
        for bytes in &contents[..3] {
            legacy.record_evidence(TENANT, PROJECT, "agent", "a",
                &selection.candidate.binding.contract_hash, None,
                &json!({"passed":true,"output_digest":digest(bytes),"delivery_candidate_digest":binding}),
                Some(bytes), None, false, None).await.unwrap();
        }
        admin
            .batch_execute("UPDATE awr_team.workstream_modes SET enabled=true")
            .await
            .unwrap();
        assert!(matches!(
            legacy
                .record_evidence(
                    TENANT,
                    PROJECT,
                    "agent",
                    "a",
                    &selection.candidate.binding.contract_hash,
                    None,
                    &json!({}),
                    Some(b"legacy"),
                    None,
                    false,
                    None
                )
                .await,
            Err(PgError::Unsupported(_))
        ));
        let report = evidence(&reads, "report", REPORT, Some(&binding)).await;
        if length > WorkstreamQuery::MAX_ARTIFACT_BYTES {
            let before = rework_business_snapshot(&admin).await;
            assert!(
                run_err(&reads, A, "oversized-open", "review.open", opening(&report))
                    .await
                    .is_review_submission_incomplete()
            );
            assert_eq!(rework_business_snapshot(&admin).await, before);
            continue;
        }
        let round = run(&reads, A, "open", "review.open", opening(&report)).await;
        let before = rework_business_snapshot(&admin).await;
        let review = inspect_round(&reads, &round).await;
        assert_eq!(review["submission"]["inspectable"], true);
        assert!(review["submission"].to_string().len() < 6000);
        let references = review["submission"]["artifacts"].as_array().unwrap();
        for (i, reference) in references.iter().enumerate() {
            let q: WorkstreamQuery = serde_json::from_value(reference["query"].clone()).unwrap();
            assert_eq!(q.max_context_bytes, (i < 3).then_some(length));
            q.validate().unwrap();
            let data = reads
                .query(TENANT, PROJECT, REVIEWER_TOKEN, q.clone())
                .await
                .unwrap()["data"]
                .clone();
            assert_eq!(data["sha256"], digest(contents[i]));
            assert_eq!(data["byte_length"], contents[i].len());
            if i == 2 {
                assert!(data["text"].is_null());
                assert!(
                    data["content_base64"].as_str() == Some(base64_for_binary(&binary).as_str())
                );
            } else {
                assert!(data["text"].as_str() == Some(std::str::from_utf8(contents[i]).unwrap()));
            }
            for token in [B, NONE] {
                assert!(
                    reads
                        .query(TENANT, PROJECT, token, q.clone())
                        .await
                        .is_err()
                );
            }
            let mut wrong_digest = q.clone();
            wrong_digest.expected_sha256 = Some("0".repeat(64));
            assert!(matches!(
                reads
                    .query(TENANT, PROJECT, REVIEWER_TOKEN, wrong_digest)
                    .await,
                Err(PgError::SnapshotDrift(_))
            ));
            if i < 3 {
                let mut default = q.clone();
                default.max_context_bytes = None;
                assert!(matches!(
                    reads.query(TENANT, PROJECT, REVIEWER_TOKEN, default).await,
                    Err(PgError::ContextIncomplete)
                ));
                let mut insufficient = q;
                insufficient.max_context_bytes = Some(length - 1);
                assert!(matches!(
                    reads
                        .query(TENANT, PROJECT, REVIEWER_TOKEN, insufficient)
                        .await,
                    Err(PgError::ContextIncomplete)
                ));
            }
        }
        assert_eq!(rework_business_snapshot(&admin).await, before);
    }
}

fn base64_for_binary(bytes: &[u8]) -> String {
    // This fixture is all 0xff; remainder bytes have deterministic padding.
    assert!(bytes.iter().all(|b| *b == 0xff));
    let mut value = "////".repeat(bytes.len() / 3);
    value.push_str(match bytes.len() % 3 {
        1 => "/w==",
        2 => "//8=",
        _ => "",
    });
    value
}

#[tokio::test]
async fn changed_selection_preserves_original_review_content_but_blocks_approval_and_allows_return()
{
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    let mut selection = select(&admin, &reads, &delivery).await;
    let original = selection.candidate.binding.digest().unwrap();
    evidence(&reads, "code", CODE, Some(&original)).await;
    let report = evidence(&reads, "report", REPORT, Some(&original)).await;
    let round = run(&reads, A, "open", "review.open", opening(&report)).await;
    selection.request_id = "select-new".into();
    selection.expected_selected_digest = Some(original.clone());
    selection.candidate.binding.candidate_version = "2".into();
    selection
        .candidate
        .binding
        .source_revision
        .as_mut()
        .unwrap()
        .value = "b".repeat(64);
    selection.read_set = read_set(&prepare(&reads, A, "a").await);
    delivery
        .select_candidate(TENANT, PROJECT, A, selection)
        .await
        .unwrap();
    let review = inspect_round(&reads, &round).await;
    assert_eq!(review["submission"]["candidate_digest"], original);
    assert_eq!(
        review["submission"]["source_revision"]["value"],
        "a".repeat(64)
    );
    assert_eq!(review["submission"]["inspectable"], true);
    assert_eq!(review["submission"]["candidate_current"], false);
    let before = rework_business_snapshot(&admin).await;
    assert!(
        run_err(
            &reads,
            REVIEWER_TOKEN,
            "stale-approve",
            "review.decide",
            decision(&round, "approve")
        )
        .await
        .is_review_submission_incomplete()
    );
    assert_eq!(rework_business_snapshot(&admin).await, before);
    let returned = run(
        &reads,
        REVIEWER_TOKEN,
        "return",
        "review.decide",
        decision(&round, "reject"),
    )
    .await;
    assert_eq!(returned["state"], "rejected");
}

#[tokio::test]
async fn persisted_content_drift_cannot_open_or_approve_a_neutral_review() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    let selection = select(&admin, &reads, &delivery).await;
    let binding = selection.candidate.binding.digest().unwrap();
    let code = evidence(&reads, "code", CODE, Some(&binding)).await;
    let report = evidence(&reads, "report", REPORT, Some(&binding)).await;
    let round = run(&reads, A, "open", "review.open", opening(&report)).await;
    let corrupt = b"unavailable-content-sentinel".to_vec();
    admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$2 WHERE id=$1",
            &[&code["artifact_id"].as_str().unwrap(), &corrupt],
        )
        .await
        .unwrap();
    let before = rework_business_snapshot(&admin).await;
    assert!(
        run_err(&reads, A, "corrupt-open", "review.open", opening(&report))
            .await
            .is_review_submission_incomplete()
    );
    assert!(
        run_err(
            &reads,
            REVIEWER_TOKEN,
            "corrupt-approve",
            "review.decide",
            decision(&round, "approve")
        )
        .await
        .is_review_submission_incomplete()
    );
    let review = inspect_round(&reads, &round).await;
    assert_eq!(review["submission"]["inspectable"], false);
    assert_eq!(
        review["submission"]["reason"],
        "manifest_content_unavailable"
    );
    assert_eq!(review["submission"]["artifacts"], json!([]));
    assert!(!review.to_string().contains("unavailable-content-sentinel"));
    assert_eq!(rework_business_snapshot(&admin).await, before);
}

#[tokio::test]
async fn legacy_artifact_review_without_neutral_delivery_keeps_its_existing_policy() {
    let (_guard, admin, _db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let report = evidence(&reads, "legacy-report", REPORT, None).await;
    let round = run(&reads, A, "legacy-open", "review.open", opening(&report)).await;
    let review = inspect_round(&reads, &round).await;
    assert_eq!(review["submission"]["delivery_required"], false);
    assert_eq!(review["submission"]["reason"], "legacy_evidence_review");
    let approved = run(
        &reads,
        REVIEWER_TOKEN,
        "legacy-approve",
        "review.decide",
        decision(&round, "approve"),
    )
    .await;
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["task_complete"], false);
}

#[tokio::test]
async fn settled_producer_gets_submission_guidance_without_hiding_wait_or_recovery() {
    use awr_team::{ExecutionSettlementMode, ExecutionSettlementPolicy};
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let mut contract = current_contract(&admin).await;
    contract.codec = WorkContract::CODEC_V3.into();
    contract.completion_policy = ExecutionSettlementPolicy::COMPLETION_POLICY.into();
    contract.execution_settlement = Some(ExecutionSettlementPolicy {
        mode: ExecutionSettlementMode::IndependentWorkspaceV2,
        workspace_id: "submission-workspace".into(),
    });
    put_contract(&admin, &contract).await;
    connector(&admin, &db, &reads, "configure", "0", true).await;
    let claim = claim(&reads).await;
    let execution = run(
        &reads,
        A,
        "prepare",
        "execution.prepare",
        json!({
            "session_id":"session-a","expected_session_version":"1",
            "claim_id":claim["claim_id"],"expected_fence":claim["fence"],
            "expected_lease_version":claim["lease_version"],
            "expected_work_version":prepare(&reads,A,"a").await["data"]["runtime"]["work_version"],
            "input_digest":INPUT,"declared_scope":["src"]
        }),
    )
    .await;
    let started = run(&reads, A, "start", "execution.start", json!({
        "session_id":"session-a","expected_session_version":"1",
        "execution_id":execution["execution_id"],"expected_execution_version":execution["execution_version"],
        "claim_id":claim["claim_id"],"expected_fence":claim["fence"],
        "expected_lease_version":claim["lease_version"],
        "expected_work_version":prepare(&reads,A,"a").await["data"]["runtime"]["work_version"],
        "execution_mode":"caller_managed"
    })).await;
    run(&reads, A, "report", "execution.report", json!({
        "session_id":"session-a","expected_session_version":"1",
        "execution_id":execution["execution_id"],"expected_execution_version":started["execution_version"],
        "outcome":"succeeded","output_digest":sha(REPORT),"observed_paths":["src/filter.js"],
        "note":"Stopped the admitted independent workspace after producing the fixture result.",
        "workspace_settlement":{"workspace_id":"submission-workspace","input_digest":INPUT,
            "environment_digest":"c".repeat(64),"claim_id":claim["claim_id"],
            "expected_fence":claim["fence"],"expected_lease_version":claim["lease_version"],
            "executor_stopped":true,"no_external_effects":true}
    })).await;
    let mut q = query("work.observe");
    q.work_id = Some("a".into());
    q.session_id = Some("session-a".into());
    let before = rework_business_snapshot(&admin).await;
    let observed = reads.query(TENANT, PROJECT, A, q.clone()).await.unwrap();
    assert_eq!(observed["data"]["guidance"]["code"], "submission");
    assert_eq!(
        observed["data"]["guidance"]["action"]["op"],
        "delivery.neutral.inspect"
    );
    let inbox = reads
        .query(TENANT, PROJECT, A, query("work.inbox"))
        .await
        .unwrap();
    let item = inbox["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["work_id"] == "a")
        .unwrap();
    assert_eq!(item["guidance"]["code"], "submission");
    assert_eq!(rework_business_snapshot(&admin).await, before);
    admin
        .batch_execute(
            "UPDATE awr_team.claims SET expires_at=clock_timestamp()-interval '1 second'",
        )
        .await
        .unwrap();
    assert_eq!(
        reads.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["guidance"]["code"],
        "submission"
    );
    let p = prepare(&reads, A, "a").await;
    run(&reads, A, "wait", "session.checkpoint", json!({
        "session_id":"session-a","expected_session_version":"1","context_hash":p["data"]["context_hash"],
        "next_action":"Await product decision","open_loops":[],
        "progress":{"phase":"blocked","summary":"Waiting for product clarification","blockers":["Product decision"]}
    })).await;
    assert_eq!(
        reads.query(TENANT, PROJECT, A, q.clone()).await.unwrap()["data"]["guidance"]["code"],
        "blocked"
    );
    admin
        .batch_execute("UPDATE awr_team.work_runtime SET recovery_blocked=true WHERE work_id='a'")
        .await
        .unwrap();
    let protected = reads.query(TENANT, PROJECT, A, q).await.unwrap();
    assert_eq!(
        protected["data"]["guidance"]["action"]["op"],
        "work.recovery"
    );
    assert_ne!(protected["data"]["guidance"]["code"], "submission");
}
