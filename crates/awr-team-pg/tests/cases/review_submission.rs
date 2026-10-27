//! Synthetic authenticated PG regressions; no native business acceptance credit.
use super::*;
use awr_team::delivery::{ArtifactEntry, ArtifactManifest, DeliveryCandidate, FactSource};
use awr_team_pg::{
    ConfigureDeliveryConnector, DeliveryConnectorMapping, DeliveryReadSet, DeliverySyncStore,
    PgResult, ReviewStore, SelectDeliveryCandidate,
};
use sha2::{Digest, Sha256};

#[path = "../fixtures/manifest_codec.rs"]
mod manifest_codec;

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

async fn delivery_evidence(
    reads: &WorkstreamReadStore,
    key: &str,
    text: &str,
    digest: &str,
) -> Value {
    run(
        reads,
        A,
        key,
        "delivery.evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
        "dirty_tree":false,"artifact_text":text,
        "delivery_payload":{"delivery_candidate_digest":digest,"passed":true,"output_digest":sha(text)}}),
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

async fn describe(reads: &WorkstreamReadStore, bearer: &str, work: &str) -> PgResult<Value> {
    let mut q = query("delivery.submission.describe");
    q.work_id = Some(work.into());
    reads.query(TENANT, PROJECT, bearer, q).await
}

async fn delivery_snapshot(admin: &Client) -> Value {
    admin.query_one("SELECT jsonb_build_object(
        'candidates',(SELECT jsonb_agg(to_jsonb(c) ORDER BY binding_digest) FROM awr_team.delivery_candidates c),
        'selections',(SELECT jsonb_agg(to_jsonb(s) ORDER BY work_id) FROM awr_team.delivery_selections s),
        'requests',(SELECT jsonb_agg(to_jsonb(r) ORDER BY request_id) FROM awr_team.delivery_sync_requests r))", &[])
        .await.unwrap().get(0)
}

#[tokio::test]
async fn discovered_submission_constructs_a_real_candidate_and_bound_readable_review() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    connector(&admin, &db, &reads, "configure", "0", true).await;
    let before = rework_business_snapshot(&admin).await;
    let description = describe(&reads, A, "a").await.unwrap()["data"].clone();
    assert_eq!(rework_business_snapshot(&admin).await, before);
    assert_eq!(description["contract"], "awr-delivery-submission-v1");
    assert_eq!(description["stored_selection"], Value::Null);
    assert_eq!(description["execution_authorized"], false);
    assert_eq!(description["acceptance_ready"], false);
    assert!(serde_json::to_vec(&description).unwrap().len() < 16384);
    let schema = &description["command"]["args_schema"];
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"].get("read_set").is_none());
    assert_eq!(
        schema["properties"]["candidate"]["properties"]["manifest"]["properties"]["entries"]["maxItems"],
        128
    );
    let resource = description["connectors"][0]["resource"].clone();
    let manifest = json!({"entries":[
        {"artifact_id":"source-code","sha256":sha(CODE),"byte_length":CODE.len().to_string(),"locator":"git-blob:src/filter.js"},
        {"artifact_id":"test-report","sha256":sha(REPORT),"byte_length":REPORT.len().to_string(),"locator":"git-blob:reports/result.txt"}]});
    let manifest_digest = manifest_codec::manifest_digest(&description, &manifest);
    let mut binding = description["binding_values"].clone();
    binding.as_object_mut().unwrap().extend(json!({
        "candidate_id":"described-submission","candidate_version":"1","manifest_digest":manifest_digest,
        "source_revision":{"resource":resource,"format":"git_sha256","value":"a".repeat(64)},
        "target":{"resource":resource,"reference":"refs/heads/main","precondition":{"kind":"missing"}}
    }).as_object().unwrap().clone());
    let candidate: DeliveryCandidate =
        serde_json::from_value(json!({"binding":binding,"manifest":manifest})).unwrap();
    assert!(candidate.binding.validate().is_ok());
    assert_eq!(candidate.manifest.digest().unwrap(), manifest_digest);
    let claim = claim(&reads).await;
    let p = prepare(&reads, A, "a").await;
    let mut args = json!({"source_snapshot_id":p["source_snapshot_id"],
        "session_id":"session-a","claim_id":claim["claim_id"],"fence":claim["fence"],
        "lease_version":claim["lease_version"],"expected_selected_digest":null,"candidate":candidate});
    let before = rework_business_snapshot(&admin).await;
    let delivery_before = delivery_snapshot(&admin).await;
    for incorrect in [
        // Reproduce the previously advertised preimage, without its outer envelope.
        manifest_codec::digest_json(
            &json!({"codec":"awr-delivery-manifest-v1","manifest":candidate.manifest}),
        ),
        "0".repeat(64),
    ] {
        let mut rejected = args.clone();
        rejected["candidate"]["binding"]["manifest_digest"] = json!(incorrect);
        let error = reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &p,
                    "select-described",
                    "delivery.candidate.select",
                    rejected,
                ),
            )
            .await
            .unwrap_err();
        assert!(error.is_manifest_digest_mismatch(), "{error:?}");
        assert_eq!(rework_business_snapshot(&admin).await, before);
        assert_eq!(delivery_snapshot(&admin).await, delivery_before);
    }
    // A definite rejected input recorded no request; its corrected request can succeed.
    let selected = reads
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &p,
                "select-described",
                description["command"]["op"].as_str().unwrap(),
                args.clone(),
            ),
        )
        .await
        .unwrap();
    let digest = selected["receipt"]["data"]["candidate_digest"]
        .as_str()
        .unwrap();
    assert_eq!(digest, candidate.binding.digest().unwrap());
    let state = describe(&reads, A, "a").await.unwrap()["data"].clone();
    assert_eq!(state["stored_selection"]["binding_digest"], digest);
    assert_eq!(state["stored_selection"]["current"], true);
    evidence(&reads, "described-code", CODE, Some(digest)).await;
    let report = evidence(&reads, "described-report", REPORT, Some(digest)).await;
    let round = run(
        &reads,
        A,
        "described-review",
        "delivery.submit_and_request_review",
        opening(&report),
    )
    .await;
    assert_eq!(round["state"], "open");
    // Discovery never admitted a command or broadened its current ownership checks.
    args["claim_id"] = json!("not-owned");
    let p = prepare(&reads, A, "a").await;
    assert!(
        reads
            .commands()
            .execute(
                TENANT,
                PROJECT,
                A,
                command(
                    &p,
                    "invalid-described-claim",
                    "delivery.candidate.select",
                    args
                )
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn discovered_manifest_codec_preserves_utf8_empty_artifacts_and_array_order() {
    let (_guard, admin, _, reads) = setup().await;
    seed_review_actors(&admin).await;
    let description = describe(&reads, A, "a").await.unwrap()["data"].clone();
    let manifest = json!({"entries":[
        {"artifact_id":"empty","sha256":sha(""),"byte_length":"0","locator":"git-blob:empty.txt"},
        {"artifact_id":"结果","sha256":sha("客户 \"雪\"\\\n"),
            "byte_length":"客户 \"雪\"\\\n".len().to_string(),"locator":"artifact:结果-\"雪\"\\.txt"}]});
    let digest = manifest_codec::manifest_digest(&description, &manifest);
    let domain: ArtifactManifest = serde_json::from_value(manifest.clone()).unwrap();
    assert_eq!(domain.digest().unwrap(), digest);
    // Start from a different raw JSON key order, including nested entry objects.
    let mut reordered = manifest.clone();
    for entry in reordered["entries"].as_array_mut().unwrap() {
        let fields = entry.as_object().unwrap();
        let reversed = fields
            .iter()
            .rev()
            .map(|(key, value)| format!("{}:{}", serde_json::to_string(key).unwrap(), value))
            .collect::<Vec<_>>()
            .join(",");
        *entry = serde_json::from_str(&format!("{{{reversed}}}")).unwrap();
    }
    assert_eq!(
        manifest_codec::manifest_digest(&description, &reordered),
        digest
    );
    reordered["entries"].as_array_mut().unwrap().reverse();
    assert_ne!(
        manifest_codec::manifest_digest(&description, &reordered),
        digest
    );
    let mut binding = description["binding_values"].clone();
    binding.as_object_mut().unwrap().extend(json!({
        "candidate_id":"utf8-empty","candidate_version":"1","manifest_digest":digest,
        "source_revision":null,"target":{"resource":"fixture://artifacts","precondition":{"kind":"missing"}}
    }).as_object().unwrap().clone());
    let claim = claim(&reads).await;
    let p = prepare(&reads, A, "a").await;
    let selected = reads.commands().execute(TENANT, PROJECT, A,
        command(&p, "select-utf8", "delivery.candidate.select", json!({
            "source_snapshot_id":p["source_snapshot_id"],"session_id":"session-a",
            "claim_id":claim["claim_id"],"fence":claim["fence"],"lease_version":claim["lease_version"],
            "candidate":{"binding":binding,"manifest":manifest}}))).await.unwrap();
    assert_eq!(selected["receipt"]["data"]["state"], "selected");
}

#[tokio::test]
async fn submission_discovery_is_lazy_read_only_scoped_bounded_and_provider_neutral() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let before = rework_business_snapshot(&admin).await;
    let state = describe(&reads, A, "a").await.unwrap()["data"].clone();
    assert_eq!(state["connectors"], json!([]));
    assert_eq!(state["stored_selection"], Value::Null);
    assert!(state.get("local_git").is_none());
    assert!(
        state["material_inputs"]["target"]
            .as_str()
            .unwrap()
            .contains("never mean missing")
    );
    assert_eq!(rework_business_snapshot(&admin).await, before);
    for (bearer, work) in [(B, "a"), (NONE, "a"), (A, "b-private"), (A, "missing")] {
        assert!(matches!(
            describe(&reads, bearer, work).await,
            Err(PgError::Forbidden)
        ));
    }
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    let selection = select(&admin, &reads, &delivery).await;
    let before = rework_business_snapshot(&admin).await;
    let state = describe(&reads, A, "a").await.unwrap()["data"].clone();
    assert!(state.get("local_git").is_some());
    let mut q = query("delivery.neutral.inspect");
    q.work_id = Some("a".into());
    let ordinary = reads.query(TENANT, PROJECT, A, q).await.unwrap();
    for forbidden in [
        "cli-runner",
        "principal_actor_id",
        "repository-workers.toml",
        "args_schema",
    ] {
        // Private identities/configurations and the full contract are not eager.
        assert!(!ordinary.to_string().contains(forbidden));
    }
    assert_eq!(
        ordinary["data"]["submission"]["describe_query"],
        json!({"protocol_version":1,"op":"delivery.submission.describe","work_id":"a"})
    );
    let mut q = query("delivery.submission.describe");
    q.work_id = Some("a".into());
    q.max_context_bytes = Some(64);
    assert!(matches!(
        reads.query(TENANT, PROJECT, A, q).await,
        Err(PgError::ResponseTooLarge)
    ));
    assert_eq!(rework_business_snapshot(&admin).await, before);
    connector(&admin, &db, &reads, "disable", "1", false).await;
    let state = describe(&reads, A, "a").await.unwrap()["data"].clone();
    assert_eq!(state["connectors"][0]["enabled"], false);
    assert!(state.get("local_git").is_none());
    // Make the selection stale without rewriting it; current binding values must differ.
    admin
        .execute(
            "UPDATE awr_team.work_contracts SET contract_hash=$1 WHERE work_id='a'",
            &[&"f".repeat(64)],
        )
        .await
        .unwrap();
    let state = describe(&reads, A, "a").await.unwrap()["data"].clone();
    assert_eq!(state["binding_values"]["contract_hash"], "f".repeat(64));
    assert_eq!(state["stored_selection"]["current"], false);
    assert_eq!(
        state["stored_selection"]["binding_digest"],
        selection.candidate.binding.digest().unwrap()
    );
    assert!(state.get("candidate").is_none());
}

#[tokio::test]
async fn submission_description_never_reads_artifact_evidence_review_or_fact_history() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    select(&admin, &reads, &delivery).await;
    let before = describe(&reads, A, "a").await.unwrap()["data"].clone();
    // Scoped to this disposable fixture database; ordinary inspection still requires
    // its normal table access. Description must use only current submission metadata.
    admin
        .batch_execute(
            "REVOKE SELECT ON awr_team.artifacts, awr_team.evidence,
        awr_team.review_rounds, awr_team.delivery_facts, awr_team.delivery_inbox FROM awr_app",
        )
        .await
        .unwrap();
    assert_eq!(describe(&reads, A, "a").await.unwrap()["data"], before);
    let mut q = query("delivery.neutral.inspect");
    q.work_id = Some("a".into());
    assert!(reads.query(TENANT, PROJECT, A, q).await.is_err());
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
        assert_eq!(err.review_submission_reason(), Some("candidate_missing"));
        assert_eq!(rework_business_snapshot(&admin).await, before);
    }
    connector(&admin, &db, &reads, "disable", "1", false).await;
    let err = run_err(&reads, A, "disabled-open", "review.open", opening(&report)).await;
    assert!(err.is_review_submission_incomplete());
    assert_eq!(err.review_submission_reason(), Some("candidate_missing"));
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
    let err = run_err(&reads, A, "missing-code", "review.open", opening(&report)).await;
    assert!(err.is_review_submission_incomplete());
    // The manifest artifact exists but is not bound to the selected candidate,
    // so its content cannot be established as readable for this submission.
    assert_eq!(
        err.review_submission_reason(),
        Some("manifest_content_unavailable")
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
async fn standalone_primary_report_keeps_exact_readable_code_manifest() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    let mut selection = select_manifest(
        &admin,
        &reads,
        &delivery,
        ArtifactManifest {
            entries: vec![ArtifactEntry {
                artifact_id: "source-code".into(),
                sha256: sha(CODE),
                byte_length: CODE.len().to_string(),
                locator: "git-blob:src/filter.js".into(),
            }],
        },
    )
    .await;
    let binding = selection.candidate.binding.digest().unwrap();
    let report = evidence(&reads, "report", REPORT, Some(&binding)).await;
    evidence(&reads, "wrong-code", CODE, Some(&"f".repeat(64))).await;
    let before = rework_business_snapshot(&admin).await;
    assert!(
        run_err(&reads, A, "missing-code", "review.open", opening(&report))
            .await
            .is_review_submission_incomplete()
    );
    assert_eq!(rework_business_snapshot(&admin).await, before);
    evidence(&reads, "code", CODE, Some(&binding)).await;
    let round = run(&reads, A, "open", "review.open", opening(&report)).await;
    let review = inspect_round(&reads, &round).await;
    let submission = &review["submission"];
    assert_eq!(submission["inspectable"], true);
    assert_eq!(submission["candidate_current"], true);
    let artifacts = submission["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 2);
    assert_eq!(artifacts[0]["manifest_artifact_id"], Value::Null);
    assert_eq!(artifacts[0]["primary_evidence"], true);
    assert_eq!(artifacts[0]["query"]["artifact_id"], report["artifact_id"]);
    assert_eq!(artifacts[1]["manifest_artifact_id"], "source-code");
    for (reference, expected) in artifacts.iter().zip([REPORT, CODE]) {
        let q: awr_team_pg::WorkstreamQuery =
            serde_json::from_value(reference["query"].clone()).unwrap();
        let content = reads
            .query(TENANT, PROJECT, REVIEWER_TOKEN, q.clone())
            .await
            .unwrap();
        assert_eq!(content["data"]["text"], expected);
        assert!(reads.query(TENANT, PROJECT, B, q).await.is_err());
    }
    // Another identical report must not replace the exact originally submitted ID.
    evidence(&reads, "duplicate-report", REPORT, Some(&binding)).await;
    admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$2 WHERE id=$1",
            &[
                &report["artifact_id"].as_str().unwrap(),
                &b"changed report".to_vec(),
            ],
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
    assert_eq!(review["submission"]["artifacts"], json!([]));
    assert_eq!(rework_business_snapshot(&admin).await, before);
    admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$2 WHERE id=$1",
            &[
                &report["artifact_id"].as_str().unwrap(),
                &REPORT.as_bytes().to_vec(),
            ],
        )
        .await
        .unwrap();
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
    selection.request_id = "select-replacement".into();
    selection.expected_selected_digest = Some(binding.clone());
    selection.candidate.binding.candidate_version = "2".into();
    selection.read_set = read_set(&prepare(&reads, A, "a").await);
    delivery
        .select_candidate(TENANT, PROJECT, A, selection)
        .await
        .unwrap();
    let historical = inspect_round(&reads, &round).await;
    assert_eq!(historical["submission"]["candidate_digest"], binding);
    assert_eq!(historical["submission"]["inspectable"], true);
    assert_eq!(historical["submission"]["candidate_current"], false);
    assert!(
        run_err(&reads, A, "stale-open", "review.open", opening(&report))
            .await
            .is_review_submission_incomplete()
    );
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

#[tokio::test]
async fn typed_delivery_evidence_binds_selection_to_a_readable_review() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    let selection = select(&admin, &reads, &delivery).await;
    let binding = selection.candidate.binding.digest().unwrap();
    // Discovery and neutral inspection name the typed entry and its binding
    // argument exactly; an agent never has to guess the payload shape.
    let description = describe(&reads, A, "a").await.unwrap()["data"].clone();
    assert_eq!(
        description["evidence"]["command"],
        "delivery.evidence.submit"
    );
    assert!(
        description["evidence"]["candidate_binding"]
            .as_str()
            .unwrap()
            .contains("args.delivery_payload.delivery_candidate_digest")
    );
    let mut q = query("delivery.neutral.inspect");
    q.work_id = Some("a".into());
    let neutral = reads.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    let note = neutral["submission"]["guidance"]["action"]["note"]
        .as_str()
        .unwrap();
    assert!(note.contains("delivery.evidence.submit"));
    assert!(note.contains("args.delivery_payload.delivery_candidate_digest"));
    // Typed evidence with preserved extra keys binds the selected candidate and
    // stores the object unchanged.
    let report_payload = json!({"delivery_candidate_digest":binding,"passed":true,
        "output_digest":sha(REPORT),"note":"typed submission report"});
    let report = run(
        &reads,
        A,
        "typed-report",
        "delivery.evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
            "dirty_tree":false,"artifact_text":REPORT,"delivery_payload":report_payload.clone()}),
    )
    .await;
    assert_eq!(report["review_state_basis"], "at_commit");
    let stored: Value = admin
        .query_one(
            "SELECT payload_json FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &report["evidence_id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored, report_payload);
    delivery_evidence(&reads, "typed-code", CODE, &binding).await;
    let round = run(
        &reads,
        A,
        "typed-review",
        "delivery.submit_and_request_review",
        opening(&report),
    )
    .await;
    assert_eq!(round["state"], "open");
    let before = rework_business_snapshot(&admin).await;
    let review = inspect_round(&reads, &round).await;
    let submission = &review["submission"];
    assert_eq!(submission["inspectable"], true);
    assert_eq!(submission["candidate_current"], true);
    assert_eq!(submission["candidate_digest"], binding);
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
}

#[tokio::test]
async fn unbound_selected_primary_report_is_diagnosed_without_mutation_or_substitution() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    let selection = select(&admin, &reads, &delivery).await;
    let binding = selection.candidate.binding.digest().unwrap();
    // The candidate is current and every manifest artifact has readable,
    // correctly bound evidence; only the caller-selected primary report lacks
    // the candidate binding (the observed run13 root cause).
    evidence(&reads, "bound-code", CODE, Some(&binding)).await;
    let unbound = evidence(&reads, "unbound-report", REPORT, None).await;
    let malformed = run(
        &reads,
        A,
        "malformed-report",
        "evidence.submit",
        json!({"session_id":"session-a","expected_session_version":"1",
        "dirty_tree":false,"artifact_text":REPORT,
        "payload":{"passed":true,"output_digest":sha(REPORT),
            "delivery_candidate_digest":"private-malformed-sentinel"}}),
    )
    .await;
    for (key, report) in [("unbound", &unbound), ("malformed", &malformed)] {
        let before = rework_business_snapshot(&admin).await;
        let before_delivery = delivery_snapshot(&admin).await;
        let err = run_err(
            &reads,
            A,
            &format!("{key}-open"),
            "review.open",
            opening(report),
        )
        .await;
        assert!(err.is_review_submission_incomplete(), "{key}: {err:?}");
        // The diagnosis names the exact submitted evidence binding failure; it
        // must not silently substitute another, correctly bound evidence.
        assert_eq!(
            err.review_submission_reason(),
            Some("candidate_not_bound_in_evidence"),
            "{key}"
        );
        assert!(!err.to_string().contains("private-malformed-sentinel"));
        assert_eq!(rework_business_snapshot(&admin).await, before, "{key}");
        assert_eq!(delivery_snapshot(&admin).await, before_delivery, "{key}");
    }
    // An unbound payload is not sufficient to diagnose a missing binding when
    // the stored receipt has drifted or the selection is no longer current.
    let id = unbound["evidence_id"].as_str().unwrap();
    let payload: Value = admin.query_one(
        "SELECT payload_json FROM awr_team.evidence WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&TENANT, &PROJECT, &id],
    ).await.unwrap().get(0);
    let mut changed_payload = payload.clone();
    changed_payload["passed"] = json!(false);
    admin.execute(
        "UPDATE awr_team.evidence SET payload_json=$4 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&TENANT, &PROJECT, &id, &changed_payload],
    ).await.unwrap();
    let before = rework_business_snapshot(&admin).await;
    let err = run_err(
        &reads,
        A,
        "corrupt-unbound-open",
        "review.open",
        opening(&unbound),
    )
    .await;
    assert_eq!(
        err.review_submission_reason(),
        Some("evidence_integrity_unavailable")
    );
    assert_eq!(rework_business_snapshot(&admin).await, before);
    admin.execute(
        "UPDATE awr_team.evidence SET payload_json=$4 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&TENANT, &PROJECT, &id, &payload],
    ).await.unwrap();
    admin.execute(
        "UPDATE awr_team.delivery_selections SET fence=fence+1 WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'",
        &[&TENANT, &PROJECT],
    ).await.unwrap();
    let before = rework_business_snapshot(&admin).await;
    let err = run_err(
        &reads,
        A,
        "stale-unbound-open",
        "review.open",
        opening(&unbound),
    )
    .await;
    assert_eq!(
        err.review_submission_reason(),
        Some("submission_incomplete")
    );
    assert_eq!(rework_business_snapshot(&admin).await, before);
    admin.execute(
        "UPDATE awr_team.delivery_selections SET fence=fence-1 WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'",
        &[&TENANT, &PROJECT],
    ).await.unwrap();
    // Foreign and unknown selectors keep the existing denial: same error, no
    // reason label, no existence disclosure, no state change.
    let contract = current_contract(&admin).await.hash().unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.evidence(tenant_id,project_id,id,work_id,contract_hash,
                evidence_kind,trust_basis,digest,payload_json,created_by)
             VALUES($1,$2,'foreign-evidence','b-private',$3,'report','caller_asserted',
                'foreign-digest','{\"passed\":true}','agent')",
            &[&TENANT, &PROJECT, &contract],
        )
        .await
        .unwrap();
    for (key, id) in [
        ("foreign", "foreign-evidence"),
        ("unknown", "missing-evidence-sentinel"),
    ] {
        let before = rework_business_snapshot(&admin).await;
        let err = run_err(
            &reads,
            A,
            &format!("{key}-open"),
            "review.open",
            json!({"session_id":"session-a","expected_session_version":"1","evidence_id":id}),
        )
        .await;
        assert!(matches!(err, PgError::EvidenceInvalid), "{key}: {err:?}");
        assert_eq!(err.review_submission_reason(), None, "{key}");
        assert!(!err.to_string().contains("b-private"));
        assert_eq!(rework_business_snapshot(&admin).await, before, "{key}");
    }
    // Correctly bound evidence on the same report bytes succeeds.
    let bound = delivery_evidence(&reads, "bound-report", REPORT, &binding).await;
    let round = run(&reads, A, "bound-open", "review.open", opening(&bound)).await;
    assert_eq!(round["state"], "open");
    let review = inspect_round(&reads, &round).await;
    assert_eq!(review["submission"]["inspectable"], true);
    assert_eq!(review["submission"]["candidate_current"], true);
    assert_eq!(review["submission"]["candidate_digest"], binding);
}

#[tokio::test]
async fn stale_drifted_or_unreadable_submissions_keep_their_precise_reason() {
    let (_guard, admin, db, reads) = setup().await;
    seed_review_actors(&admin).await;
    let delivery = connector(&admin, &db, &reads, "configure", "0", true).await;
    let mut selection = select(&admin, &reads, &delivery).await;
    let binding = selection.candidate.binding.digest().unwrap();
    let code = evidence(&reads, "code", CODE, Some(&binding)).await;
    let report = evidence(&reads, "report", REPORT, Some(&binding)).await;
    let round = run(&reads, A, "open", "review.open", opening(&report)).await;
    assert_eq!(round["state"], "open");
    // Unreadable manifest content blocks both reopen and approval.
    admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$2 WHERE id=$1",
            &[
                &code["artifact_id"].as_str().unwrap(),
                &b"changed code".to_vec(),
            ],
        )
        .await
        .unwrap();
    let before = rework_business_snapshot(&admin).await;
    for (token, key, op, args) in [
        (A, "corrupt-open", "review.open", opening(&report)),
        (
            REVIEWER_TOKEN,
            "corrupt-approve",
            "review.decide",
            decision(&round, "approve"),
        ),
    ] {
        let err = run_err(&reads, token, key, op, args).await;
        assert!(err.is_review_submission_incomplete(), "{key}: {err:?}");
        assert_eq!(
            err.review_submission_reason(),
            Some("manifest_content_unavailable"),
            "{key}"
        );
    }
    assert_eq!(rework_business_snapshot(&admin).await, before);
    admin
        .execute(
            "UPDATE awr_team.artifacts SET content=$2 WHERE id=$1",
            &[
                &code["artifact_id"].as_str().unwrap(),
                &CODE.as_bytes().to_vec(),
            ],
        )
        .await
        .unwrap();
    // A drifted stored receipt digest is rejected as a version drift.
    let stored_digest: String = admin
        .query_one(
            "SELECT digest FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &report["evidence_id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    admin
        .execute(
            "UPDATE awr_team.evidence SET digest=$4
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &TENANT,
                &PROJECT,
                &report["evidence_id"].as_str().unwrap(),
                &"0".repeat(64),
            ],
        )
        .await
        .unwrap();
    let before = rework_business_snapshot(&admin).await;
    let err = run_err(
        &reads,
        REVIEWER_TOKEN,
        "drifted-approve",
        "review.decide",
        decision(&round, "approve"),
    )
    .await;
    assert!(err.is_review_submission_incomplete());
    assert_eq!(
        err.review_submission_reason(),
        Some("evidence_binding_changed")
    );
    assert_eq!(rework_business_snapshot(&admin).await, before);
    admin
        .execute(
            "UPDATE awr_team.evidence SET digest=$4
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &TENANT,
                &PROJECT,
                &report["evidence_id"].as_str().unwrap(),
                &stored_digest,
            ],
        )
        .await
        .unwrap();
    // A replaced selection makes the original submission historical.
    selection.request_id = "select-replacement".into();
    selection.expected_selected_digest = Some(binding.clone());
    selection.candidate.binding.candidate_version = "2".into();
    selection.read_set = read_set(&prepare(&reads, A, "a").await);
    delivery
        .select_candidate(TENANT, PROJECT, A, selection)
        .await
        .unwrap();
    let before = rework_business_snapshot(&admin).await;
    for (token, key, op, args) in [
        (A, "stale-open", "review.open", opening(&report)),
        (
            REVIEWER_TOKEN,
            "stale-approve",
            "review.decide",
            decision(&round, "approve"),
        ),
    ] {
        let err = run_err(&reads, token, key, op, args).await;
        assert!(err.is_review_submission_incomplete(), "{key}: {err:?}");
        assert_eq!(
            err.review_submission_reason(),
            Some("selection_changed"),
            "{key}"
        );
    }
    assert_eq!(rework_business_snapshot(&admin).await, before);
    // Review gating is unchanged: returning the stale round still works.
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
