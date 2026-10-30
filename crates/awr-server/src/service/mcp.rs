//! Stateless MCP transport over the same transactional Team operations as HTTP.
//! TMCP-012 adds project-admin access preview/apply/outcome tools (no raw secrets).
//! TMCP-023 adds planning suggest/draft/preview/approve/publish/outcome tools.
use super::*;
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    middleware::{self, Next},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::*,
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio::{sync::OwnedSemaphorePermit, time::Instant};

mod input_contracts;
mod input_diagnostics;
mod planning_schema;
pub(super) use input_diagnostics::add_input_diagnostic;

/// Publish an operation field in the flat args vocabulary.
/// An existing shared definition stays in place: a later operation branch may be narrower.
/// When the validation shape is identical, retain operation guidance supplied by
/// the later publisher instead of losing it to initialization order.
fn publish_missing_flat_arg(command: &mut Value, name: &str, schema: Value) {
    let properties = command["properties"]["args"]["properties"]
        .as_object_mut()
        .unwrap();
    let Some(existing) = properties.get_mut(name) else {
        properties.insert(name.to_owned(), schema);
        return;
    };
    if existing.get("description").is_some() || schema.get("description").is_none() {
        return;
    }
    let mut existing_shape = existing.clone();
    let mut published_shape = schema.clone();
    existing_shape
        .as_object_mut()
        .unwrap()
        .remove("description");
    published_shape
        .as_object_mut()
        .unwrap()
        .remove("description");
    if existing_shape == published_shape {
        existing["description"] = schema["description"].clone();
    }
}

fn add_execution_prepare_input_schema(command: &mut Value) {
    let claim_id = json!({"type":"string","minLength":1,"maxLength":128,
        "description":"Exact current owned claim ID from task intake or work.prepare."});
    let declared_scope = json!({"type":"array","maxItems":128,"uniqueItems":true,
        "items":{"type":"string","minLength":1,"maxLength":4096},
        "description":"Canonical project-relative paths within the published scope. No empty, dot or parent components, backslashes, colons or control characters. The server rechecks scope and authority."});
    publish_missing_flat_arg(command, "claim_id", claim_id.clone());
    publish_missing_flat_arg(command, "declared_scope", declared_scope.clone());
    // Keep the shared args definitions wider. claim.acquire still accepts
    // expected_work_version "0", and input_digest stays null-compatible.
    let expected_work_version = json!({"type":"string","pattern":"^[1-9][0-9]*$",
        "description":"Current runtime work version after acquiring the coordination claim."});
    let input_digest = json!({"type":"string","pattern":"^[0-9a-f]{64}$",
        "description":"SHA-256 of the measured execution inputs; retain it for execution.start and reporting."});
    let fields = &command["properties"]["args"]["properties"];
    let properties = json!({
        "session_id":fields["session_id"].clone(),
        "expected_session_version":fields["expected_session_version"].clone(),
        "claim_id":claim_id,
        "expected_fence":fields["expected_fence"].clone(),
        "expected_lease_version":fields["expected_lease_version"].clone(),
        "expected_work_version":expected_work_version,
        "input_digest":input_digest,
        "declared_scope":declared_scope
    });
    command["allOf"].as_array_mut().unwrap().push(json!({
        "if":{"properties":{"op":{"const":"execution.prepare"}},"required":["op"]},
        "then":{"properties":{"args":{"type":"object","additionalProperties":false,
            "required":["session_id","expected_session_version","claim_id","expected_fence",
                "expected_lease_version","expected_work_version","input_digest","declared_scope"],
            "properties":properties}}}
    }));
}

fn add_execution_start_input_schema(command: &mut Value) {
    let execution_mode = json!({"type":"string","enum":["caller_managed","reference_write_v1"],
        "description":"Required for execution.start. caller_managed admits the caller. reference_write_v1 also requires expected_input_digest and system attestation authority. The service does not infer or default a mode and does not dispatch the local runner."});
    let expected_input_digest = json!({"type":["string","null"],"pattern":"^[0-9a-f]{64}$",
        "description":"execution.start input binding. caller_managed accepts omission, null, or the prepared digest; any supplied digest must match. reference_write_v1 requires the prepared digest as 64 lowercase hexadecimal characters."});
    publish_missing_flat_arg(command, "execution_mode", execution_mode.clone());
    publish_missing_flat_arg(
        command,
        "expected_input_digest",
        expected_input_digest.clone(),
    );
    // Reuse wider shared fields. claim.acquire still accepts work version 0, and
    // evidence input_digest stays null-compatible. This branch narrows only start.
    let fields = &command["properties"]["args"]["properties"];
    let execution_id = json!({"type":"string","minLength":1,"maxLength":128,
        "description":"execution.start: execution_id from the matching execution.prepare receipt."});
    let expected_work_version = json!({"type":"string","pattern":"^[1-9][0-9]*$",
        "description":"Current runtime work version. execution.start does not accept 0."});
    let properties = json!({
        "session_id":fields["session_id"].clone(),
        "expected_session_version":fields["expected_session_version"].clone(),
        "execution_id":execution_id,
        "expected_execution_version":fields["expected_execution_version"].clone(),
        "expected_work_version":expected_work_version,
        "claim_id":fields["claim_id"].clone(),
        "expected_fence":fields["expected_fence"].clone(),
        "expected_lease_version":fields["expected_lease_version"].clone(),
        "execution_mode":execution_mode,
        "expected_input_digest":expected_input_digest
    });
    command["allOf"].as_array_mut().unwrap().push(json!({
        "if":{"properties":{"op":{"const":"execution.start"}},"required":["op"]},
        "then":{"properties":{"args":{"type":"object","additionalProperties":false,
            "required":["session_id","expected_session_version","execution_id",
                "expected_execution_version","expected_work_version","claim_id","expected_fence",
                "expected_lease_version","execution_mode"],
            "properties":properties,
            "allOf":[{"if":{"properties":{"execution_mode":{"const":"reference_write_v1"}},
                "required":["execution_mode"]},
                "then":{"required":["expected_input_digest"],"properties":{"expected_input_digest":{
                    "type":"string","pattern":"^[0-9a-f]{64}$"}}}}]}}}
    }));
}

fn add_execution_report_input_schema(command: &mut Value) {
    let outcome = json!({"type":"string","enum":["succeeded","failed","cancelled","unknown"],
        "description":"Required for execution.report. Terminal caller assertion: succeeded, failed, cancelled, or unknown. The service does not infer or default an outcome."});
    let mut note = json!({"type":"string","minLength":1,"maxLength":4096,
        "description":"Required for execution.report. Nonblank text, at most 4096 UTF-8 bytes, without a NUL. Whitespace-only text is rejected. The service does not infer this note."});
    // Flat minLength/maxLength stay JSON Schema character counts. Only the closed
    // branch sets x-nonblank-utf8, opting that note into parser byte, trim, and NUL checks.
    publish_missing_flat_arg(command, "outcome", outcome.clone());
    publish_missing_flat_arg(command, "note", note.clone());
    note["x-nonblank-utf8"] = json!(true);
    let fields = &command["properties"]["args"]["properties"];
    let execution_id = json!({"type":"string","minLength":1,"maxLength":128,
        "description":"execution.report: execution_id from the matching execution.prepare or execution.start receipt."});
    let properties = json!({
        "session_id":fields["session_id"].clone(),
        "expected_session_version":fields["expected_session_version"].clone(),
        "execution_id":execution_id,
        "expected_execution_version":fields["expected_execution_version"].clone(),
        "outcome":outcome,
        "output_digest":fields["output_digest"].clone(),
        "observed_paths":fields["observed_paths"].clone(),
        "note":note,
        "workspace_settlement":fields["workspace_settlement"].clone(),
        "baseline_transition":fields["baseline_transition"].clone()
    });
    command["allOf"].as_array_mut().unwrap().push(json!({
        "if":{"properties":{"op":{"const":"execution.report"}},"required":["op"]},
        "then":{"properties":{"args":{"type":"object","additionalProperties":false,
            "required":["session_id","expected_session_version","execution_id",
                "expected_execution_version","outcome","observed_paths","note"],
            "properties":properties,
            "allOf":[{"if":{"properties":{"outcome":{"const":"succeeded"}},"required":["outcome"]},
                "then":{"required":["output_digest"],"properties":{"output_digest":{
                    "type":"string","pattern":"^[0-9a-f]{64}$"}}}}]}}}
    }));
}

fn add_checkpoint_input_schema(command: &mut Value) {
    let context_hash = json!({"type":"string","pattern":"^[0-9a-f]{64}$"});
    let next_action = json!({"type":"string","minLength":1,"maxLength":8192});
    let open_loops = json!({"type":"array","maxItems":32,"items":{
        "type":"string","minLength":1,"maxLength":4096}});
    let mut flat_context_hash = context_hash.clone();
    flat_context_hash["description"] = json!(
        "64 lowercase hexadecimal characters from the consumed work.prepare context_hash, without a sha256: prefix. A session.inspect context_hash is historical."
    );
    let mut flat_next_action = next_action.clone();
    flat_next_action["description"] =
        json!("Nonblank next action recorded by session.checkpoint, at most 8192 UTF-8 bytes.");
    let mut flat_open_loops = open_loops.clone();
    flat_open_loops["description"] = json!(
        "Unfinished items retained by session.checkpoint. At most 32 nonblank entries, each at most 4096 UTF-8 bytes."
    );
    publish_missing_flat_arg(command, "context_hash", flat_context_hash);
    publish_missing_flat_arg(command, "next_action", flat_next_action);
    publish_missing_flat_arg(command, "open_loops", flat_open_loops);
    let fields = &command["properties"]["args"]["properties"];
    let properties = json!({
        "session_id":fields["session_id"].clone(),
        "expected_session_version":fields["expected_session_version"].clone(),
        "context_hash":context_hash,
        "next_action":next_action,
        "open_loops":open_loops,
        "client_info":fields["client_info"].clone(),
        "progress":fields["progress"].clone(),
        "usage":fields["usage"].clone()
    });
    command["allOf"].as_array_mut().unwrap().push(json!({
        "if":{"properties":{"op":{"const":"session.checkpoint"}},"required":["op"]},
        "then":{"properties":{"args":{"type":"object","additionalProperties":false,
            "required":["session_id","expected_session_version","context_hash","next_action","open_loops"],
            "properties":properties}}}
    }));
}

fn add_evidence_input_schema(command: &mut Value, query: &mut Value) {
    let fields = command["properties"]["args"]["properties"]
        .as_object_mut()
        .unwrap();
    fields.insert("claimed_trust".into(), json!({"type":["string","null"],
        "description":"Optional reported trust label; never grants authority or upgrades evidence."}));
    fields.insert("delivery_payload".into(), json!({
        "type":"object","additionalProperties":true,
        "required":["delivery_candidate_digest"],
        "properties":{
            "delivery_candidate_digest":{"type":"string","pattern":"^[0-9a-f]{64}$"},
            "passed":{"type":"boolean"},
            "output_digest":{"type":"string","pattern":"^[0-9a-f]{64}$"}},
        "description":"delivery.evidence.submit only: required typed delivery evidence object, stored and hashed exactly as submitted. delivery_candidate_digest is the successful selection receipt's data.candidate_digest (64 lowercase hex); passed (boolean) and output_digest (64 lowercase hex) are optional; other JSON properties are preserved unchanged. The generic payload alias is not accepted by this operation."}));
    let names = [
        "session_id",
        "expected_session_version",
        "claimed_trust",
        "payload",
        "artifact_hex",
        "artifact_text",
        "input_digest",
        "dirty_tree",
        "execution_id",
    ];
    let properties: serde_json::Map<String, Value> = names
        .into_iter()
        .map(|name| (name.to_owned(), fields[name].clone()))
        .collect();
    let delivery_names = [
        "session_id",
        "expected_session_version",
        "claimed_trust",
        "delivery_payload",
        "artifact_hex",
        "artifact_text",
        "input_digest",
        "dirty_tree",
        "execution_id",
    ];
    // Both property maps own cloned values; the fields borrow ends before either
    // allOf branch push mutates command again.
    let delivery_properties: serde_json::Map<String, Value> = delivery_names
        .into_iter()
        .map(|name| (name.to_owned(), fields[name].clone()))
        .collect();
    let artifact_encoding = json!([{
        "if":{"required":["artifact_text"],
            "properties":{"artifact_text":{"type":"string"}}},
        "then":{"properties":{"artifact_hex":{"type":"null"}}}
    }]);
    command["allOf"].as_array_mut().unwrap().push(json!({
        "if":{"properties":{"op":{"const":"evidence.submit"}},"required":["op"]},
        "then":{"properties":{"args":{"type":"object","additionalProperties":false,
            "required":["session_id","expected_session_version","payload","dirty_tree"],
            "properties":properties,
            "allOf":artifact_encoding.clone(),
            "description":"Submit measured artifact_text (or legacy artifact_hex); the service computes artifact_digest; do not add artifact_sha256. Generic payload remains arbitrary JSON. All simulated-member evidence requires args.execution_id, including auxiliary files. For Agent completion, use payload.passed=true and the actual execution output_digest; keep input_digest and execution_id in args. Inspection does not approve the evidence."}}}}));
    command["allOf"].as_array_mut().unwrap().push(json!({
        "if":{"properties":{"op":{"const":"delivery.evidence.submit"}},"required":["op"]},
        "then":{"properties":{"args":{"type":"object","additionalProperties":false,
            "required":["session_id","expected_session_version","delivery_payload","dirty_tree"],
            "properties":delivery_properties,
            "allOf":artifact_encoding,
            "description":"Typed bound delivery evidence. delivery_payload is a required JSON object with delivery_candidate_digest (64 lowercase hex from the selection receipt), optional passed and output_digest; other keys are preserved. Artifact bytes, simulated-member execution_id and Agent completion bindings follow the evidence.submit rules. Submission does not assert candidate selection, currentness or acceptance."}}}}));
    for (op, selector) in [
        ("claim.inspect", "claim_id"),
        ("review.inspect", "review_round_id"),
        ("evidence.inspect", "evidence_id"),
    ] {
        query["allOf"].as_array_mut().unwrap().push(json!({
            "if":{"properties":{"op":{"const":op}},"required":["op"]},
            "then":{"required":[selector]}}));
    }
}

fn add_baseline_transition_schema(command: &mut Value) {
    let revision = json!({"type":"object","additionalProperties":false,
        "required":["resource","format","value"],"properties":{
            "resource":{"type":"string","minLength":1,"maxLength":1024,
                "description":"Opaque repository identity, at most 1024 UTF-8 bytes; not a path or permission."},
            "format":{"enum":["git_sha1","git_sha256"]},
            "value":{"type":"string"}},
        "allOf":[{"if":{"properties":{"format":{"const":"git_sha1"}},"required":["format"]},
            "then":{"properties":{"value":{"pattern":"^[0-9a-f]{40}$"}}},
            "else":{"properties":{"value":{"pattern":"^[0-9a-f]{64}$"}}}}]});
    let path = json!({"type":"string","minLength":1,"maxLength":4096,
        "description":"Canonical project-relative path, at most 4096 UTF-8 bytes; no empty, dot or parent components, backslashes, colons or controls."});
    let fields = command["properties"]["args"]["properties"]
        .as_object_mut()
        .unwrap();
    fields.insert("observed_paths".into(), json!({"type":"array","maxItems":128,"items":path,
        "description":"execution.report: all actual workspace effects from before to after this execution, including imported baseline paths. Always scope checked; never substitute the contribution diff against the imported target."}));
    fields.insert("baseline_transition".into(), json!({"type":["object","null"],"additionalProperties":false,
        "description":"execution.report only: optional caller-asserted Git attribution, never scope, release, target approval or filesystem proof. Revisions share one resource and format. If importing a baseline would exceed the coding scope, first settle coding, then use a separately source-reviewed exact-path baseline task; publish only after workspace settlement. Recheck source, claim, scope and actual effects.",
        "required":["kind","operation","workspace_before_revision","imported_revision","workspace_after_revision","contribution_paths"],
        "properties":{
            "kind":{"const":"caller_asserted_git_baseline_v1"},
            "operation":{"enum":["fast_forward_only","merge_no_ff"],
                "description":"Caller-reported local operation; does not verify Git ancestry or parents."},
            "workspace_before_revision":revision,
            "imported_revision":revision,
            "workspace_after_revision":revision,
            "contribution_paths":{"type":"array","maxItems":128,"uniqueItems":true,"items":path,
                "description":"Paths in the contribution diff from imported revision to result; may be empty. Provenance only: no path is subtracted from observed_paths."}
        }}));
}

fn add_review_input_schema(command: &mut Value) {
    let identity = json!({"type":"string","minLength":1,"maxLength":128,
        "description":"Opaque identity; at most 128 UTF-8 bytes, without control characters."});
    let text = json!({"type":"string","minLength":1,"maxLength":4096,
        "description":"Nonblank text; at most 4096 UTF-8 bytes."});
    let fields = json!({
        "session_id":identity,"expected_session_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "evidence_id":identity,"round_id":identity,"note":text,"reason":text,
        "decision":{"type":"string","enum":["approve","reject"]},
        "context_complete":{"type":"boolean"},"requested_policy":{"type":["string","null"]},
        "findings":{"type":"array","maxItems":32,"items":{
            "type":"object","additionalProperties":false,"required":["finding_key","summary"],
            "properties":{
                "finding_key":identity,
                "summary":{"type":"string","minLength":1,"maxLength":2048},
                "criterion":{"type":"string","minLength":1,"maxLength":2048},
                "rule":{"type":"string","minLength":1,"maxLength":2048},
                "counterexample":{"type":"object","additionalProperties":false,"required":["given","when","then"],
                    "properties":{
                        "given":{"type":"string","minLength":1,"maxLength":2048},
                        "when":{"type":"string","minLength":1,"maxLength":2048},
                        "then":{"type":"string","minLength":1,"maxLength":2048}}},
                "risk":{"type":"object","additionalProperties":false,"required":["kind","reasoning"],
                    "properties":{
                        "kind":{"enum":["security","data_loss"]},
                        "reasoning":{"type":"string","minLength":1,"maxLength":2048}}},
                "scope":{"oneOf":[
                    {"type":"object","additionalProperties":false,"required":["kind"],
                        "properties":{"kind":{"const":"delta"}}},
                    {"type":"object","additionalProperties":false,"required":["kind","finding_key"],
                        "properties":{"kind":{"const":"unresolved"},"finding_key":identity}}]}}}},
        "resolutions":{"type":"array","maxItems":32,"items":{
            "type":"object","additionalProperties":false,"required":["finding_key","resolution"],
            "properties":{"finding_key":identity,
                "resolution":{"type":"string","minLength":1,"maxLength":2048}}}}
    });
    // Publish the review/completion selectors in the flat args vocabulary so
    // clients discover them directly. event_id stays a delivery.facts.ingest
    // field and is never an alias. Shared publications of session_id,
    // expected_session_version, note and reason stay untouched.
    for (name, description) in [
        (
            "evidence_id",
            "review.open / delivery.submit_and_request_review: the evidence under review. work.complete / delivery.finalize: the current bound evidence. Not the delivery.facts.ingest event_id.",
        ),
        (
            "round_id",
            "review.accept / review.return / review.decide / work.rework: round_id from the review receipt; inspect it as review_round_id.",
        ),
        ("decision", "review.decide only: approve or reject."),
        (
            "findings",
            "review.accept / review.return / review.decide: findings of this decision. Never state a severity: it is derived from what the finding is measured against (a required criterion or a hard rule of the contract, shown with a counterexample, or a stated security or data-loss risk blocks; everything else is a deferred record). From the second round on a blocking finding must name its scope: the delta since the reviewed bundle or an unresolved finding of an earlier round, or it is deferred.",
        ),
        (
            "resolutions",
            "review.accept / review.decide with approve: how the approval deals with each blocking finding that was open. For governed work an approval must resolve all of them.",
        ),
        (
            "context_complete",
            "work.complete / delivery.finalize: caller assertion that the required context was consumed completely.",
        ),
        (
            "requested_policy",
            "Optional for work.complete / delivery.finalize; a downgrade of the bound completion policy is rejected.",
        ),
    ] {
        let mut flat = fields[name].clone();
        flat["description"] = json!(description);
        publish_missing_flat_arg(command, name, flat);
    }
    for (op, extra, optional) in [
        ("review.open", vec!["evidence_id"], vec![]),
        (
            "delivery.submit_and_request_review",
            vec!["evidence_id"],
            vec![],
        ),
        (
            "review.accept",
            vec!["round_id", "reason"],
            vec!["findings", "resolutions"],
        ),
        (
            "review.return",
            vec!["round_id", "reason"],
            vec!["findings"],
        ),
        (
            "review.decide",
            vec!["round_id", "decision", "reason"],
            vec!["findings", "resolutions"],
        ),
        ("work.rework", vec!["round_id", "note"], vec![]),
        (
            "work.complete",
            vec!["evidence_id", "context_complete"],
            vec!["requested_policy"],
        ),
        (
            "delivery.finalize",
            vec!["evidence_id", "context_complete"],
            vec!["requested_policy"],
        ),
    ] {
        let required: Vec<_> = ["session_id", "expected_session_version"]
            .into_iter()
            .chain(extra)
            .collect();
        let properties: serde_json::Map<String, Value> = required
            .iter()
            .copied()
            .chain(optional)
            .map(|name| (name.to_owned(), fields[name].clone()))
            .collect();
        command["allOf"].as_array_mut().unwrap().push(json!({
            "if":{"properties":{"op":{"const":op}},"required":["op"]},
            "then":{"properties":{"args":{"type":"object","additionalProperties":false,
                "required":required,"properties":properties}}}
        }));
    }
}

#[derive(Clone)]
struct Endpoint {
    state: Arc<StateData>,
    project: ProjectBinding,
}

// Request-local only. Retaining the permit also bounds SDK handlers that outlive
// a disconnected HTTP receiver. No authenticated authority is cached here.
#[derive(Clone)]
struct RequestAccess {
    bearer: String,
    _permit: Arc<OwnedSemaphorePermit>,
    deadline: Instant,
}

pub(super) fn router(state: Arc<StateData>, project: ProjectBinding) -> Router {
    let mut config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true);
    config.allowed_hosts = state.hosts.clone();
    config.max_request_body_bytes = 65536;
    let path = format!("/v1/projects/{}/mcp", project.key);
    let endpoint = Endpoint { state, project };
    let handler = endpoint.clone();
    let service: StreamableHttpService<Endpoint, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(handler.clone()), Default::default(), config);
    Router::new()
        .nest_service(&path, service)
        .layer(middleware::from_fn_with_state(endpoint, authorize))
}

async fn authorize(State(endpoint): State<Endpoint>, request: Request, next: Next) -> Response {
    if !allowed_request(&endpoint.state, request.headers()) {
        return denied();
    }
    let Some(token) = bearer(request.headers()).map(str::to_owned) else {
        return oauth::challenge(&endpoint.state, &endpoint.project.key, false);
    };
    let is_oauth = token.starts_with(crate::oauth::ACCESS_TOKEN_PREFIX);
    let token = if is_oauth {
        let resolved = endpoint.state.oauth.as_ref().and_then(|oauth| {
            oauth.store.resolve(
                &token,
                &oauth.resource(&endpoint.project.key),
                std::time::Instant::now(),
            )
        });
        let Some(token) = resolved else {
            return oauth::challenge(&endpoint.state, &endpoint.project.key, true);
        };
        token
    } else {
        token
    };
    let Ok(permit) = endpoint.state.permits.clone().try_acquire_owned() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, json!({"code":"Busy"}));
    };
    let access = RequestAccess {
        bearer: token,
        _permit: Arc::new(permit),
        deadline: Instant::now() + Duration::from_secs(30),
    };
    let result = tokio::time::timeout_at(access.deadline, async {
        let (mut parts, body) = request.into_parts();
        let Ok(body) = to_bytes(body, 65536).await else {
            return response(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"code":"RequestTooLarge"}),
            );
        };
        // Initialize, discovery, ping and notifications also require current
        // access. Each actual tool then rechecks it in its own operation's tx.
        let capabilities: WorkstreamQuery =
            serde_json::from_value(json!({"protocol_version":1,"op":"capabilities"}))
                .expect("static capabilities query");
        if let Err(error) = endpoint
            .state
            .store
            .query(
                &endpoint.project.tenant_id,
                &endpoint.project.project_id,
                &access.bearer,
                capabilities,
            )
            .await
        {
            // Generic admission failures retain the login challenge. A diagnosed
            // expired delegation instead keeps its bounded403 recovery advice:
            // repeating authentication cannot renew the member's authority.
            if matches!(error, PgError::Forbidden) {
                return oauth::challenge(&endpoint.state, &endpoint.project.key, true);
            }
            return error_response(error);
        }
        parts.extensions.insert(access.clone());
        let result = next.run(Request::from_parts(parts, Body::from(body))).await;
        let (mut parts, body) = result.into_parts();
        // Bound the actual MCP envelope, including the SDK's text fallback.
        let Ok(body) = to_bytes(body, 1_048_576).await else {
            return error_response(PgError::ResponseTooLarge);
        };
        parts
            .headers
            .insert("cache-control", "no-store".parse().unwrap());
        Response::from_parts(parts, Body::from(body))
    })
    .await;
    result.unwrap_or_else(|_| unavailable())
}

// Keep handoff wire types visible without requiring their fields for other operations.
fn add_handoff_input_schema(command: &mut Value) {
    let identity = json!({"type":"string","minLength":1,"maxLength":128});
    let execution = json!({"description":"ExecutionInstance object, never prose. Use the actual authenticated person; agent_run additionally requires an existing agent binding. This does not authorize execution.","oneOf":[
        {"type":"object","required":["kind","person_id"],"properties":{
            "kind":{"const":"person"},"person_id":identity}},
        {"type":"object","required":["kind","person_id","agent_id","binding_id"],"properties":{
            "kind":{"const":"agent_run"},"person_id":identity,"agent_id":identity,"binding_id":identity}}
    ]});
    let ids = |max| json!({"type":"array","maxItems":max,"items":identity});
    let notes = json!({"type":"array","maxItems":64,"items":{"type":"string","minLength":1,"maxLength":4096}});
    let fields = command["properties"]["args"]["properties"]
        .as_object_mut()
        .unwrap();
    let handoff = json!({
        "handoff_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"Use the actual handoff.id. Execute handoff.inspect to receive the complete package and consumption receipt; a read-only query does not authorize acceptance."},
        "kind":{"enum":["execution","responsibility"],"description":"handoff.propose: execution changes the executor; responsibility changes ownership separately."},
        "to_person_id":identity,
        "package":{"type":["object","null"],"additionalProperties":false,
            "description":"Optional compatibility selectors. The server derives the package from the current recorded checkpoint, artifacts and dependencies; supplied factual selectors must match. Branch/directory and checkpoint prose are reported annotations.",
            "required":["task_id","contract_version","contract_hash","current_person_id","current_execution","consumed_context_digest","checkpoint_ids","artifact_versions","dependency_ids","todos","awaiting_replies","unknown_side_effects"],
            "properties":{
                "task_id":identity,"contract_version":{"type":"string","minLength":1,"maxLength":128},
                "contract_hash":{"type":"string","pattern":"^[0-9a-f]{64}$"},
                "current_person_id":identity,"current_execution":execution,
                "consumed_context_digest":{"type":"string","pattern":"^[0-9a-f]{64}$"},
                "checkpoint_ids":{"type":"array","minItems":1,"maxItems":64,"items":identity},
                "artifact_versions":{"type":"array","maxItems":128,"items":{"type":"object","additionalProperties":false,"required":["artifact_id","version"],"properties":{
                    "artifact_id":identity,"version":{"type":"string","minLength":1,"maxLength":128}}}},
                "branch_id":{"type":["string","null"],"minLength":1,"maxLength":128},
                "working_directory":{"type":["string","null"],"minLength":1,"maxLength":4096},
                "dependency_ids":ids(128),"todos":notes,"awaiting_replies":notes,"unknown_side_effects":notes
            }},
        "proposed_successor":{"anyOf":[execution,{"type":"null"}]},
        "proposer_execution_id":{"type":["string","null"]},
        "proposer_fence":{"type":["string","null"],"pattern":"^(0|[1-9][0-9]*)$"},
        "expires_at_ms":{"type":["integer","null"]},
        "now_ms":{"type":["integer","null"],"description":"Optional compatibility annotation. All handoff expiry and transitions use the database clock."},
        "inspector_person_id":identity,"acceptor_person_id":identity,"by_person_id":identity,
        "successor_execution":execution,
        "inspection_request_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"handoff.accept: the original request_id of this member/client/session's actual handoff.inspect command. Read its full package and prepared context. Reinspect after relevant changes."},
        "prior_execution_stopped":{"type":["boolean","null"],"description":"Optional compatibility annotation; cannot authorize transfer. The server verifies actual settlement."},
        "prior_reconciled":{"type":["boolean","null"],"description":"Optional compatibility annotation; cannot establish reconciliation."},
        "context_reprepared":{"type":["boolean","null"],"description":"Optional compatibility annotation; cannot substitute for the bound inspection receipt."},
        "reason":{"type":"string","description":"Use the operation-specific reason bound; handoff.reject/cancel accept at most 2048 bytes."}
    });
    fields.extend(handoff.as_object().unwrap().clone());
    let published = fields.clone();
    let conditions = command["allOf"].as_array_mut().unwrap();
    for (op, extra, optional) in [
        (
            "handoff.propose",
            vec!["kind", "to_person_id"],
            vec![
                "package",
                "proposed_successor",
                "proposer_execution_id",
                "proposer_fence",
                "expires_at_ms",
                "now_ms",
            ],
        ),
        (
            "handoff.inspect",
            vec!["expected_handoff_version", "inspector_person_id"],
            vec!["now_ms"],
        ),
        (
            "handoff.accept",
            vec![
                "expected_handoff_version",
                "acceptor_person_id",
                "successor_execution",
                "inspection_request_id",
            ],
            vec![
                "prior_execution_stopped",
                "prior_reconciled",
                "context_reprepared",
                "expected_current_fence",
                "now_ms",
            ],
        ),
        (
            "handoff.reject",
            vec!["expected_handoff_version", "by_person_id", "reason"],
            vec!["now_ms"],
        ),
        (
            "handoff.cancel",
            vec!["expected_handoff_version", "by_person_id", "reason"],
            vec!["now_ms"],
        ),
        (
            "handoff.timeout",
            vec!["expected_handoff_version"],
            vec!["now_ms"],
        ),
    ] {
        let mut required = vec!["session_id", "expected_session_version", "handoff_id"];
        required.extend(extra);
        let properties: serde_json::Map<String, Value> = required
            .iter()
            .chain(&optional)
            .map(|name| ((*name).to_owned(), published[*name].clone()))
            .collect();
        let mut args = json!({"type":"object","additionalProperties":false,
            "required":required,"properties":properties});
        if matches!(op, "handoff.reject" | "handoff.cancel") {
            args["properties"]["reason"] = json!({"type":"string","minLength":1,"maxLength":2048});
        }
        conditions.push(
            json!({"if":{"properties":{"op":{"const":op}},"required":["op"]},
            "then":{"properties":{"args":args}}}),
        );
    }
}

fn add_neutral_delivery_schema(command: &mut Value) {
    let identity = json!({"type":"string","minLength":1,"maxLength":128});
    let version = json!({"type":"string","pattern":"^(0|[1-9][0-9]*)$"});
    let digest = json!({"type":"string","pattern":"^[0-9a-f]{64}$"});
    let fields = command["properties"]["args"]["properties"]
        .as_object_mut()
        .unwrap();
    fields.extend(json!({
        "source_snapshot_id":identity,
        "expected_connector_version":version,
        "mapping":{"type":"object","additionalProperties":false,
            "required":["connector_id","provider","resource","principal_actor_id","principal_client_id","fact_source","enabled"],
            "properties":{"connector_id":identity,"provider":{"type":"string","maxLength":128},
                "resource":{"type":"string","maxLength":1024},"principal_actor_id":identity,
                "principal_client_id":identity,"fact_source":{"enum":["caller_declared","adapter_observation","operator_recorded"]},
                "enabled":{"type":"boolean"},
                "source_locator":{"oneOf":[
                    {"type":"object","additionalProperties":false,"required":["action","locator"],
                        "properties":{"action":{"const":"set"},"locator":{"type":"object","additionalProperties":false,
                            "required":["contract","url"],"properties":{"contract":{"const":"awr-git-source-v1"},
                                "url":{"type":"string","maxLength":2048,"pattern":"^(https://|ssh://git@)"}}}}},
                    {"type":"object","additionalProperties":false,"required":["action"],
                        "properties":{"action":{"const":"clear"}}},
                    {"type":"null"}]}}},
        "expected_selected_digest":{"anyOf":[digest,{"type":"null"}]},
        "candidate":{"type":"object","additionalProperties":false,"required":["binding","manifest"],
            "properties":{"binding":{"type":"object"},"manifest":{"type":"object"}},
            "description":"Strict DeliveryCandidate: binding and manifest. Query delivery.submission.describe for the complete input shape, current binding values and publication guidance; these fields cannot grant authority."},
        "fence":version,"lease_version":version,
        "connector_id":identity,"connector_version":version,"candidate_digest":digest,
        "selection_version":version,"review_decision_id":identity,"integration_id":identity,
        "operation":{"type":"string","enum":["fast_forward"],
            "description":"Version-bound integration.prepare reserves an intent; only the configured server integrator may execute it."},
        "lease_seconds":{"type":"integer","minimum":5,"maximum":300},
        "inspection_id":identity,"event_id":identity,
        "records":{"type":"array","minItems":1,"maxItems":32,
            "items":{"type":"object","additionalProperties":false,"required":["protocol","protocol_version","record"],
                "properties":{"protocol":{"const":awr_team::delivery::DELIVERY_PROTOCOL},"protocol_version":{"const":awr_team::delivery::DELIVERY_PROTOCOL_VERSION},
                    "record":{"type":"object","additionalProperties":false,"required":["kind","data"],
                        "properties":{"kind":{"enum":["candidate","change_request","verification","review_decision","integration_request","integration_observation","adapter_capabilities"]},"data":{"type":"object"}}}}},
            "description":"Strict neutral DeliveryEnvelope records; no raw provider payload or credentials."},
        "expected_selection_version":version,"expected_metadata_revision":version,
        "expected_source_fingerprint":{"type":"string","pattern":"^sha256:[0-9a-f]{64}$"},
        "completion_receipt_id":{"anyOf":[identity,{"type":"null"}]},"publication_id":identity
    }).as_object().unwrap().clone());
    fields.insert("evidence_id".into(), identity.clone());
    fields.insert("review_round_id".into(), identity.clone());
    let published = fields.clone();
    for (op, fields, optional) in [
        (
            "delivery.connector.configure",
            vec!["expected_connector_version", "mapping"],
            vec![],
        ),
        (
            "delivery.candidate.select",
            vec![
                "session_id",
                "claim_id",
                "fence",
                "lease_version",
                "candidate",
            ],
            vec!["expected_selected_digest"],
        ),
        (
            "delivery.inspection.reserve",
            vec![
                "connector_id",
                "connector_version",
                "candidate_digest",
                "lease_seconds",
            ],
            vec![],
        ),
        (
            "delivery.facts.ingest",
            vec!["connector_id", "inspection_id", "event_id", "records"],
            vec![],
        ),
        (
            "delivery.source.prepare",
            vec![
                "candidate_digest",
                "expected_selection_version",
                "expected_metadata_revision",
                "expected_source_fingerprint",
                "lease_seconds",
            ],
            vec!["completion_receipt_id"],
        ),
        (
            "delivery.source.renew",
            vec!["publication_id", "fence", "lease_seconds"],
            vec![],
        ),
        (
            "delivery.source.write",
            vec!["publication_id", "fence"],
            vec![],
        ),
        (
            "delivery.source.confirm",
            vec!["publication_id", "fence"],
            vec![],
        ),
        (
            "delivery.source.abandon",
            vec!["publication_id", "fence"],
            vec![],
        ),
        (
            "delivery.integration.prepare",
            vec![
                "connector_id",
                "connector_version",
                "candidate_digest",
                "selection_version",
                "evidence_id",
                "review_round_id",
                "review_decision_id",
                "operation",
            ],
            vec![],
        ),
        (
            "delivery.integration.reject_prepared",
            vec!["integration_id", "reason"],
            vec![],
        ),
    ] {
        let mut required = vec!["source_snapshot_id"];
        required.extend(fields);
        let properties: serde_json::Map<String, Value> = required
            .iter()
            .chain(&optional)
            .map(|name| ((*name).to_owned(), published[*name].clone()))
            .collect();
        let mut args = json!({"type":"object","additionalProperties":false,
            "required":required,"properties":properties});
        if op == "delivery.integration.reject_prepared" {
            args["properties"]["reason"] = json!({"type":"string","minLength":1,"maxLength":1024,
                "description":"A nonblank reason of at most 1,024 UTF-8 bytes."});
        }
        command["allOf"].as_array_mut().unwrap().push(json!({
            "if":{"properties":{"op":{"const":op}},"required":["op"]},
            "then":{"properties":{"args":args}}
        }));
    }
}

fn add_closed_command_args_schema(
    command: &mut Value,
    op: &str,
    required: &[&str],
    optional: &[&str],
) {
    let fields = &command["properties"]["args"]["properties"];
    let mut properties: serde_json::Map<String, Value> = required
        .iter()
        .chain(optional)
        .map(|name| {
            let schema = fields
                .get(*name)
                .unwrap_or_else(|| panic!("missing catalog schema for {op} args.{name}"));
            ((*name).to_owned(), schema.clone())
        })
        .collect();
    if op == "delivery.register_pr" {
        properties["observed_at"]["maxLength"] = json!(64);
        properties["observed_at"]["x-max-utf8-bytes"] = json!(64);
        properties["observed_at"]["description"] = json!(
            "Observed timestamp text. JSON Schema maxLength counts characters; the backend also limits this value to 64 UTF-8 bytes."
        );
    }
    command["allOf"].as_array_mut().unwrap().push(json!({
        "if":{"properties":{"op":{"const":op}},"required":["op"]},
        "then":{"properties":{"args":{
            "type":"object","additionalProperties":false,
            "required":required,"properties":properties
        }}}
    }));
}

/// Close operations whose strict backend request type previously inherited the
/// shared flat args vocabulary. The shared definitions keep tools/list compact;
/// these operation branches are the authoritative allowed-field boundaries.
fn add_remaining_closed_command_schemas(command: &mut Value) {
    let identity_runtime = json!({"maxUtf8Bytes":128,"rejectControls":true});
    let identity = json!({"type":"string","minLength":1,"maxLength":128,
        "x-identity-utf8":identity_runtime.clone()});
    let nullable_identity = json!({"type":["string","null"],"minLength":1,"maxLength":128,
        "x-identity-utf8":identity_runtime});
    let positive_version = json!({"type":"string","pattern":"^[1-9][0-9]*$"});
    let version = json!({"type":"string","pattern":"^(0|[1-9][0-9]*)$"});
    let digest = json!({"type":"string","pattern":"^[0-9a-f]{64}$"});
    let git_sha = json!({"type":"string","pattern":"^[0-9a-f]{40}$"});
    let nullable_git_sha = json!({"type":["string","null"],"pattern":"^[0-9a-f]{40}$"});
    let canonical_path = json!({"type":"string","minLength":1,"maxLength":4096});
    let recovery_facts = json!({
        "type":"object","additionalProperties":false,
        "required":["outcome","input_digest","environment_digest","observed_paths","note"],
        "properties":{
            "outcome":{"type":"string","enum":["succeeded","failed","cancelled","unknown"]},
            "input_digest":digest.clone(),
            "output_digest":{"type":["string","null"],"pattern":"^[0-9a-f]{64}$"},
            "environment_digest":digest.clone(),
            "observed_paths":{"type":"array","maxItems":128,"items":canonical_path},
            "note":{"type":"string","minLength":1,"maxLength":4096,"x-nonblank-utf8":true},
            "executor_stopped":{"type":["boolean","null"]}
        },
        "allOf":[{"if":{"required":["outcome"],
            "properties":{"outcome":{"const":"succeeded"}}},
            "then":{"required":["output_digest"],
                "properties":{"output_digest":{"type":"string","pattern":"^[0-9a-f]{64}$"}}}}]
    });
    let previous_epoch_recovery = json!({
        "type":["object","null"],"additionalProperties":false,
        "required":["execution_epoch","executor_stopped","review_reference"],
        "properties":{
            "execution_epoch":identity.clone(),
            "executor_stopped":{"type":"boolean"},
            "review_reference":{"type":"string","minLength":1,"maxLength":2048,
                "x-nonblank-utf8":true}
        }
    });
    let fields = command["properties"]["args"]["properties"]
        .as_object_mut()
        .unwrap();
    fields.extend(
        json!({
            "reviewed_receipt_id":nullable_identity.clone(),
            "facts":recovery_facts,
            "clear_recovery_block":{"type":"boolean"},
            "previous_epoch_recovery":previous_epoch_recovery,
            "repository":{"type":"string","minLength":1,"maxLength":256,
                "x-nonblank-utf8":true},
            "pr_number":{"type":"integer","minimum":1,"maximum":2147483647},
            "pr_url":{"type":"string","minLength":1,"maxLength":512,
                "x-nonblank-utf8":true},
            "head_sha":git_sha.clone(),
            "merge_sha":nullable_git_sha,
            "test_evidence_id":nullable_identity.clone(),
            "fact_source":{"type":"string","enum":[
                "authorized_human_github_verification","operator_recorded_observation"]},
            "observed_at":{"type":"string","minLength":1,"x-nonblank-utf8":true},
            "author_actor_id":nullable_identity.clone(),
            "owner_person_id":nullable_identity.clone(),
            "executor_actor_id":nullable_identity,
            "gh_submitted":{"type":["boolean","null"]},
            "gh_approved":{"type":["boolean","null"]},
            "gh_merged":{"type":["boolean","null"]},
            "delivery_id":identity.clone(),
            "expected_head_sha":git_sha,
            "review_round_id":identity.clone(),
            "consumer_work_id":identity.clone(),
            "expected_consumer_contract_hash":digest.clone(),
            "expected_consumer_ownership_version":positive_version,
            "receipt_id":identity,
            "expected_artifact_sha256":digest.clone(),
            "export_id":{"type":"string","minLength":1,"maxLength":128},
            "expected_export_version":positive_version,
            "expected_disclosure_sha256":digest,
            "expected_adoption_version":version
        })
        .as_object()
        .unwrap()
        .clone(),
    );

    // The admission decision has its own fields so that it never shares a shared property with
    // the review and execution actions. A separate literal also keeps each macro expansion small.
    command["properties"]["args"]["properties"].as_object_mut().unwrap().extend(
        json!({
            "admission_decision":{"enum":["proposed","approved","denied"],
                "description":"admission.decide: proposed states a judgment and admits nothing; approved admits the work for the scope it binds; denied needs admission_reason."},
            "binding_fingerprint":digest.clone(),
            "expected_admission_version":{"type":"string","pattern":"^(0|[1-9][0-9]*)$",
                "description":"admission.decide: admission_version from work.admission (0 when no decision is recorded); a decision recorded since is refused as a changed precondition."},
            "admission_judgment":{"type":"object","additionalProperties":false,
                "required":["contribution","necessity","alternatives","deliverable","exit","responsibility"],
                "properties":{
                    "contribution":{"type":"string","minLength":1,"maxLength":2048},
                    "necessity":{"type":"string","minLength":1,"maxLength":2048},
                    "alternatives":{"type":"string","minLength":1,"maxLength":2048},
                    "deliverable":{"type":"string","minLength":1,"maxLength":2048},
                    "exit":{"type":"string","minLength":1,"maxLength":2048},
                    "responsibility":{"type":"string","minLength":1,"maxLength":2048}}},
            "admission_reason":{"type":"string","minLength":1,"maxLength":2048},
            "admission_proposal_id":{"type":"string","minLength":1,"maxLength":128}
        })
        .as_object()
        .unwrap()
        .clone(),
    );

    for (op, required, optional) in [
        (
            "admission.decide",
            vec![
                "admission_decision",
                "binding_fingerprint",
                "expected_admission_version",
            ],
            vec![
                "admission_judgment",
                "admission_reason",
                "admission_proposal_id",
            ],
        ),
        (
            "session.end",
            vec!["session_id", "expected_session_version"],
            vec![],
        ),
        (
            "claim.acquire",
            vec![
                "session_id",
                "expected_session_version",
                "expected_work_version",
                "ttl_seconds",
            ],
            vec![],
        ),
        (
            "claim.renew",
            vec![
                "session_id",
                "expected_session_version",
                "claim_id",
                "expected_fence",
                "expected_lease_version",
                "ttl_seconds",
            ],
            vec![],
        ),
        (
            "claim.release",
            vec![
                "session_id",
                "expected_session_version",
                "claim_id",
                "expected_fence",
                "expected_lease_version",
            ],
            vec![],
        ),
        (
            "execution.cancel",
            vec![
                "session_id",
                "expected_session_version",
                "execution_id",
                "expected_execution_version",
            ],
            vec![],
        ),
        (
            "execution.attest",
            vec![
                "session_id",
                "expected_session_version",
                "execution_id",
                "expected_execution_version",
                "facts",
            ],
            vec!["reviewed_receipt_id"],
        ),
        (
            "execution.reconcile",
            vec![
                "session_id",
                "expected_session_version",
                "execution_id",
                "expected_execution_version",
                "expected_work_version",
                "clear_recovery_block",
                "facts",
            ],
            vec!["reviewed_receipt_id", "previous_epoch_recovery"],
        ),
        (
            "delivery.register_pr",
            vec![
                "session_id",
                "expected_session_version",
                "repository",
                "pr_number",
                "pr_url",
                "head_sha",
                "fact_source",
                "observed_at",
            ],
            vec![
                "merge_sha",
                "test_evidence_id",
                "author_actor_id",
                "owner_person_id",
                "executor_actor_id",
                "gh_submitted",
                "gh_approved",
                "gh_merged",
            ],
        ),
        (
            "delivery.observe_pr",
            vec![
                "session_id",
                "expected_session_version",
                "delivery_id",
                "expected_head_sha",
                "fact_source",
                "observed_at",
            ],
            vec!["gh_approved", "gh_merged", "merge_sha"],
        ),
        (
            "delivery.export.publish",
            vec![
                "session_id",
                "expected_session_version",
                "consumer_work_id",
                "expected_consumer_contract_hash",
                "expected_consumer_ownership_version",
                "receipt_id",
                "expected_artifact_sha256",
            ],
            vec![],
        ),
        (
            "delivery.export.revoke",
            vec![
                "session_id",
                "expected_session_version",
                "export_id",
                "expected_export_version",
            ],
            vec![],
        ),
        (
            "delivery.adopt",
            vec![
                "session_id",
                "expected_session_version",
                "expected_responsibility_version",
                "export_id",
                "expected_export_version",
                "expected_disclosure_sha256",
                "expected_adoption_version",
            ],
            vec![],
        ),
    ] {
        add_closed_command_args_schema(command, op, &required, &optional);
    }
}

fn catalog() -> Vec<Tool> {
    let mut query = json!({"type":"object","additionalProperties":false,
    "required":["protocol_version","op"],"properties":{
        "protocol_version":{"type":"integer","const":1},
        "op":{"type":"string","enum":WorkstreamQuery::OPERATIONS},
        "workstream_id":{"type":"string","description":"Omit for work.next and work.inbox; follow the returned next_query to select work."},
        "work_id":{"type":"string","description":"Omit for work.next and work.inbox."},
        "session_id":{"type":"string","description":"Omit for work.next and work.inbox; session.inspect requires this selector."},
        "request_id":{"type":"string","description":"For delivery.integration.inspect, use IntegrationRequest.request_id (receipt data.integration_id). For delivery.neutral.outcome, use the original client command retry ID."},"claim_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"Required for claim.inspect together with either work_id or session_id; use claim_id from the claim.acquire receipt. Omit for other operations. The runtime bound is 128 UTF-8 bytes; JSON Schema maxLength counts characters."},
        "execution_id":{"type":"string"},"handoff_id":{"type":"string","description":"Actual handoff.id from a proposal receipt or handoff.inspect; events.list item IDs identify events, not handoffs. Read the exact handoff before receiving it."},
        "evidence_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"Required for evidence.inspect; omit for other operations."},
        "review_round_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"Required for review.inspect; use round_id from the review receipt. Omit for other operations."},
        "change_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"Optional filter for audit.history, audit.export or audit.count."},
        "member_actor_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"Optional actor filter for audit operations; does not expand visibility."},
        "category":{"type":"string","minLength":1,"maxLength":128,
            "description":"Optional category filter for audit.history, audit.export or audit.count."},
        "include_denies":{"type":"boolean",
            "description":"Optional for audit.history, audit.export or audit.count; authorization still applies."},
        "search":{"type":"string","maxLength":512,"description":"Required for work.search; optional literal, case-insensitive member-name substring for task.assignees."},"cursor":{"type":"string","maxLength":4096},
        "limit":{"type":"integer","minimum":1,"maximum":100},
        "max_context_bytes":{"type":"integer","minimum":1,"maximum":WorkstreamQuery::MAX_ARTIFACT_BYTES,
            "description":"Defaults stay bounded. Only artifact.content accepts up to 1 MiB of raw bytes; other query budgets are at most 256 KiB. artifact.segment requires expected_sha256 and uses 1..4096 raw bytes per page (default 2048); follow next_query and verify the complete digest. JSON/base64 and metadata add transport overhead."},
        "source_path":{"type":"string","maxLength":512},
        "artifact_id":{"type":"string","maxLength":128},
        "export_id":{"type":"string","maxLength":128},
        "expected_sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"}
    }});
    query["allOf"] = json!([
        {"if":{"properties":{"op":{"enum":WorkstreamQuery::WORK_SELECTION_OPERATIONS}},"required":["op"]},
            "then":{"anyOf":[{"required":["work_id"]},{"required":["session_id"]}]}},
        {"if":{"properties":{"op":{"const":"artifact.segment"}},"required":["op"]},
            "then":{"required":["expected_sha256"],"properties":{"max_context_bytes":{"maximum":WorkstreamQuery::MAX_ARTIFACT_SEGMENT_BYTES}}}},
        {"if":{"properties":{"op":{"enum":WorkstreamQuery::CONTEXT_BUDGET_OPERATIONS}},"required":["op"]},
            "else":{"properties":{"max_context_bytes":{"type":"null"}}}},
        {"if":{"properties":{"op":{"const":"artifact.content"}},"required":["op"]},
            "then":{"properties":{"max_context_bytes":{"maximum":WorkstreamQuery::MAX_ARTIFACT_BYTES}}},
            "else":{"properties":{"max_context_bytes":{"maximum":262144}}}},
        {"if":{"properties":{"op":{"enum":["delivery.neutral.inspect","delivery.submission.describe","delivery.neutral.outcome","delivery.source.status","delivery.integration.inspect"]}},"required":["op"]},
            "then":{"required":["work_id"],"not":{"required":["session_id"]}}},
        {"if":{"properties":{"op":{"enum":["delivery.neutral.outcome","delivery.integration.inspect"]}},"required":["op"]},
            "then":{"required":["request_id"]}},
        {"if":{"properties":{"op":{"const":"task.assignees"}},"required":["op"]},
            "then":{"required":["work_id"],"not":{"anyOf":[
                {"required":["session_id"]},{"required":["request_id"]},
                {"required":["claim_id"]},{"required":["execution_id"]},
                {"required":["handoff_id"]},{"required":["evidence_id"]},
                {"required":["review_round_id"]},{"required":["source_path"]},
                {"required":["artifact_id"]},{"required":["export_id"]},
                {"required":["expected_sha256"]},{"required":["max_context_bytes"]},
                {"required":["change_id"]},{"required":["member_actor_id"]},
                {"required":["category"]},{"required":["include_denies"]}
            ]}}}
    ]);
    let identity_runtime = json!({"maxUtf8Bytes":128,"rejectControls":true});
    let mut command = json!({"type":"object","additionalProperties":false,
    "required":["protocol_version","request_id","op","workstream_id","work_id","coordinator_epoch","expected_project_revision","expected_authority_version","expected_ownership_version","expected_contract_hash","args"],
    "properties":{
        "protocol_version":{"type":"integer","const":1},
        "op":{"type":"string","enum":WorkstreamCommand::OPERATIONS},
        "request_id":{"type":"string","maxLength":128,
            "x-identity-utf8":identity_runtime.clone(),
            "description":"Caller-supplied request identifier. JSON Schema maxLength counts characters; runtime also limits this identity to 128 UTF-8 bytes and rejects control characters."},
        "workstream_id":{"type":"string"},"work_id":{"type":"string"},
        "coordinator_epoch":{"type":"string"},
        "expected_project_revision":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},
        "expected_authority_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_ownership_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_contract_hash":{"type":"string"},
        "args":{"type":"object","description":"session.start: conversation_id, optional client_info. session.checkpoint: session_id, expected_session_version, context_hash, next_action, open_loops; optional client_info, progress, usage (schemas below). Batch feedback at meaningful boundaries; execution.report is terminal-only. session.end: session_id, expected_session_version. admission.decide: admission_decision (proposed|approved|denied), binding_fingerprint and expected_admission_version (admission_version) from work.admission, checked against the scope and the decisions recorded now, admission_judgment (approved or proposed), admission_reason (denied), admission_proposal_id (an approval answering a proposal); it is attributed to the verified caller and needs the planning approval permission. All claim/execution actions: session_id, expected_session_version. claim.acquire adds expected_work_version (0 when runtime absent), ttl_seconds (1..3600). claim.renew/release add claim_id, expected_fence, expected_lease_version; renew also ttl_seconds. execution.prepare adds claim_id, expected_fence, expected_lease_version, expected_work_version, input_digest (64 lowercase hex), declared_scope (canonical relative paths). execution.cancel adds execution_id, expected_execution_version. execution.start adds execution_id, expected_execution_version, claim_id, expected_fence, expected_lease_version, expected_work_version, execution_mode (caller_managed or reference_write_v1), optional expected_input_digest. reference_write_v1 requires the prepared input digest and system attestation authority; the service does not dispatch the local runner. execution.report adds execution_id, expected_execution_version, outcome (succeeded/failed/cancelled/unknown), optional output_digest (required for success), observed_paths (all actual workspace effects, including baseline imports), note, optional workspace_settlement and baseline_transition (schemas below). Contribution provenance never exempts observed paths from scope. For an admitted caller_managed independent_workspace_v1/v2 run, include workspace_settlement to settle a terminal result even while the lease is live; execution.start/inspect expose terminal_reporting bindings. V1 requires a live claim; V2 also permits elapsed time on the unchanged original current claim. Omission keeps conservative recovery; tests and expiry do not prove stopping. execution.attest adds execution_id, expected_execution_version, facts; optional reviewed_receipt_id (latest inspected caller receipt) and facts.executor_stopped=true may clear only its attributed reference_write_v1 report barrier with unchanged admission authority and exact resources. Missing confirmation fields preserve legacy settlement without automatic recovery clearing. execution.reconcile also adds expected_work_version, reviewed_receipt_id (latest inspected ID or null), clear_recovery_block, optional previous_epoch_recovery. Old-epoch recovery requires {execution_epoch (exact inspected epoch), executor_stopped (true to settle), review_reference (nonempty, <=2048 bytes, no controls)}; this is an authorized operator assertion, not independently verified fencing. facts: outcome, input_digest, optional output_digest (required for success), environment_digest, observed_paths, note. Digests are 64 lowercase hex. Versions are decimal strings; unknown fields fail. handoff.propose: session_id, expected_session_version, handoff_id, kind (execution|responsibility), to_person_id; the server derives the package from the latest actual checkpoint, artifacts and dependencies. Optional supplied package selectors must match. handoff.inspect returns the complete package, prepared context and consumption receipt; a read-only query is not consumption. handoff.accept adds acceptor_person_id, successor_execution, inspection_request_id from this member/client/session's actual inspection, and current runtime.last_fence as expected_current_fence whenever runtime exists. Read the returned package and context, then accept; reinspect after related facts, session or authority changes. The server verifies execution settlement and expiry using its database clock. Legacy now_ms and assurance booleans are optional annotations and confer no authority. All handoff receiving/closing commands use session_id, expected_session_version, handoff_id, expected_handoff_version; inspect adds inspector_person_id, reject/cancel add by_person_id and reason. Timeout closes the proposal only and does not stop execution. evidence.submit: session_id, expected_session_version, payload, dirty_tree (required boolean); optional claimed_trust/input_digest, conditional execution_id, and artifact_text or legacy artifact_hex (schemas below). Agent completion requires payload.passed=true, payload.output_digest matching execution.report, input_digest matching execution.prepare, execution_id and artifact bytes. For workspace-settled success, execution output_digest and payload.output_digest bind the SHA-256 of the exact submitted artifact bytes. Other policies retain separate output/artifact digest semantics. Inspect evidence and artifact.content before review. delivery.evidence.submit: session_id, expected_session_version, delivery_payload (required typed object, schemas below), dirty_tree (required boolean); optional claimed_trust/input_digest, conditional execution_id, and artifact_text or legacy artifact_hex exactly as evidence.submit. Bound delivery evidence uses this typed entry with args.delivery_payload.delivery_candidate_digest; the generic payload alias is refused, while generic evidence.submit keeps its arbitrary JSON payload. review.open / delivery.submit_and_request_review: session_id, expected_session_version, evidence_id. Configured neutral delivery or a selected candidate requires current payload.delivery_candidate_digest and readable bytes for every manifest entry; report-only submissions return ReviewSubmissionIncomplete. Inspect delivery.neutral.inspect for scoped submission context, then review.inspect submission.artifacts[].query for exact content; never peer paths. Approval rechecks the current binding. review.accept/return: session_id, expected_session_version, round_id, reason. review.decide: session_id, expected_session_version, round_id, decision (approve|reject), reason — requires the matching human or Agent review grant and policy. work.rework: session_id, expected_session_version, round_id, note. work.complete / delivery.finalize: session_id, expected_session_version, evidence_id, context_complete, optional requested_policy. Finalization needs maintainer/project_admin permission and, for an Agent, an explicit finalize_delivery delegation; independent review and current evidence remain required. delivery.register_pr: session_id, expected_session_version, repository, pr_number, pr_url, head_sha, fact_source (authorized_human_github_verification|operator_recorded_observation), observed_at (RFC3339), optional merge_sha/test_evidence_id/gh_* flags — v1 manual GitHub verification, not webhook sync. delivery.observe_pr: session_id, expected_session_version, delivery_id, expected_head_sha, fact_source, observed_at, optional gh_approved/gh_merged/merge_sha. Query delivery.inspect separates GitHub submitted/approved/merged from AWR acceptance complete."}
    }});
    command["properties"]["args"]["properties"] = json!({
        "conversation_id":{"type":"string","minLength":1,"maxLength":128,
            "x-identity-utf8":identity_runtime.clone(),
            "description":"Required for session.start. Stable host conversation, thread, or session identifier used to bind the durable AWR session. JSON Schema maxLength counts characters; runtime also limits this identity to 128 UTF-8 bytes and rejects control characters."},
        "session_id":{"type":"string","minLength":1,"maxLength":128,
            "x-identity-utf8":identity_runtime.clone(),
            "description":"Required for session lifecycle, claim, execution, handoff, evidence, review, rework and delivery actions except session.start. Use the owned session_id from work.next resume or session.inspect. JSON Schema maxLength counts characters; runtime also limits this identity to 128 UTF-8 bytes and rejects control characters."},
        "dirty_tree":{"type":"boolean","description":"Required for evidence.submit and delivery.evidence.submit. Report the actual workspace state; not a completion or trust assertion."},
        "payload":{"description":"Required for evidence.submit; generic evidence accepts any JSON value. Agent completion requires an object with passed=true and output_digest (64 lowercase hex) matching the successful execution report."},
        "artifact_text":{"type":["string","null"],"maxLength":1048576,
            "description":"evidence.submit / delivery.evidence.submit: exact UTF-8 text, preserving whitespace and line endings. At most 1 MiB in bytes; the whole MCP request still must fit 64 KiB. Keep reports compact. Omit artifact_hex when using this field."},
        "artifact_hex":{"type":["string","null"],"maxLength":2097152,"pattern":"^([0-9a-fA-F]{2})*$",
            "description":"evidence.submit / delivery.evidence.submit: legacy exact bytes encoded as even-length hex, at most 1 MiB decoded; whole MCP request limit is 64 KiB. Generate mechanically for binary artifacts; prefer artifact_text for text. Omit artifact_text when using this field."},
        "input_digest":{"type":["string","null"],"pattern":"^[0-9a-f]{64}$",
            "description":"Evidence input binding; Agent completion must match execution.prepare input_digest."},
        "execution_id":{"type":["string","null"],"description":"Required for execution actions except execution.prepare; conditional for evidence (see evidence.submit). Use the matching execution.prepare receipt."},
        "expected_session_version":{"type":"string","pattern":"^[1-9][0-9]*$",
            "description":"Required for session lifecycle, claim, execution, handoff, evidence, review, rework and delivery actions except session.start. Use current session_version from work.next resume or session.inspect items; send a JSON decimal string."},
        "expected_work_version":{"type":"string","pattern":"^(0|[1-9][0-9]*)$",
            "description":"claim.acquire uses \"0\" before work runtime exists; otherwise use the current work version."},
        "expected_fence":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_lease_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_execution_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_handoff_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_current_fence":{"type":["string","null"],"pattern":"^(0|[1-9][0-9]*)$","description":"handoff.accept: required whenever work runtime exists; use runtime.last_fence from work.prepare. Omit or null only before runtime exists. Terminal execution and expired claims retain this fence."},
        "client_info":{"type":["object","null"],"additionalProperties":false,"required":["product"],"properties":{
            "product":{"type":"string","maxLength":128},"version":{"type":["string","null"],"maxLength":128},
            "model":{"type":["object","null"],"additionalProperties":false,"required":["id","source"],"properties":{
                "id":{"type":"string","maxLength":128},"provider":{"type":["string","null"],"maxLength":128},
                "source":{"enum":["host_metadata","client_configuration"]}}},
            "capabilities":{"type":"object","additionalProperties":false,"properties":{
                "model":{"enum":["supported","unsupported","unknown"]},
                "usage":{"enum":["supported","unsupported","unknown"]},
                "progress":{"enum":["supported","unsupported","unknown"]}}}
        }},
        "progress":{"type":["object","null"],"additionalProperties":false,"required":["phase","summary"],"properties":{
            "phase":{"enum":["starting","implementing","testing","waiting_user","blocked","ready_for_review","waiting_dependency","reviewing","reworking","integrating","delivered"]},
            "summary":{"type":"string","maxLength":2048},
            "completed":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":1024}},
            "blockers":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":1024}},
            "artifacts":{"type":"array","maxItems":8,"items":{"type":"object","additionalProperties":false,"required":["label","reference"],"properties":{
                "label":{"type":"string","maxLength":256},"reference":{"type":"string","maxLength":1024}}}},
            "tests":{"type":"array","maxItems":8,"items":{"type":"object","additionalProperties":false,"required":["name","outcome"],"properties":{
                "name":{"type":"string","maxLength":256},"outcome":{"enum":["passed","failed","not_run"]},"reference":{"type":["string","null"],"maxLength":1024}}}}
        }},
        "usage":{"type":["object","null"],"additionalProperties":false,
            "description":"Only measured cumulative host-session counters; omit unknown semantics. Never sum snapshots or infer task cost. Include input or output; cached input is a subset of input. Bounds on text fields are UTF-8 bytes.",
            "required":["source","source_ref","counter_id","scope","coverage","observed_at_unix_ms"],"properties":{
                "source":{"type":"string","maxLength":128},"source_ref":{"type":"string","maxLength":1024},"counter_id":{"type":"string","maxLength":128},
                "scope":{"const":"host_session"},"coverage":{"enum":["complete","partial","unknown"]},
                "observed_at_unix_ms":{"type":"integer","minimum":1,"maximum":9007199254740991u64},
                "input_tokens":{"type":["integer","null"],"minimum":0,"maximum":9007199254740991u64},
                "output_tokens":{"type":["integer","null"],"minimum":0,"maximum":9007199254740991u64},
                "cached_input_tokens":{"type":["integer","null"],"minimum":0,"maximum":9007199254740991u64}
            }}
    });
    command["properties"]["args"]["properties"]["output_digest"] = json!({"type":["string","null"],"pattern":"^[0-9a-f]{64}$",
            "description":"execution.report: required for success. For workspace-settled success, SHA-256 of the exact artifact bytes subsequently submitted as artifact_text or decoded artifact_hex. Other execution policies retain their output digest semantics."});
    command["properties"]["args"]["properties"]["workspace_settlement"] = json!({"type":["object","null"],"additionalProperties":false,
    "description":"execution.report only: include this declaration to settle an admitted caller_managed independent_workspace_v1/v2 terminal result, including with a live lease. True stop/effect assertions settle only exact admitted workspace paths. V1 requires a live claim; V2 also permits only elapsed time on the unchanged original current claim. Omitted, false or unknown declarations preserve recovery. No external effects, trust or approval are inferred. Recheck contract, execution, claim and effects.",
    "required":["workspace_id","input_digest","environment_digest","claim_id","expected_fence","expected_lease_version"],
    "properties":{
        "workspace_id":{"type":"string","description":"Exact workspace_id from the admitted execution's settlement_policy; an opaque identity, not a filesystem path."},
        "input_digest":{"type":"string","pattern":"^[0-9a-f]{64}$","description":"Exact input_digest supplied to execution.prepare for this run."},
        "environment_digest":{"type":"string","pattern":"^[0-9a-f]{64}$","description":"SHA-256 of a measured non-sensitive environment description; keep secrets and environment dumps outside the record."},
        "claim_id":{"type":"string","description":"This execution's unchanged current owned claim; V1 additionally requires it to be live."},
        "expected_fence":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_lease_version":{"type":"string","pattern":"^[1-9][0-9]*$","description":"Current claim lease version, including same-fence renewals."},
        "executor_stopped":{"type":["boolean","null"],"description":"Explicit caller assertion that this execution has actually stopped; never infer from tests or disconnection."},
        "no_external_effects":{"type":["boolean","null"],"description":"Explicit caller assertion that the run has no external effects to settle; repository delivery remains a separate flow."}
    }});
    let fields = &command["properties"]["args"]["properties"];
    let start_properties = json!({
        "conversation_id":fields["conversation_id"].clone(),
        "client_info":fields["client_info"].clone()
    });
    command["allOf"] = json!([{
        "if":{"properties":{"op":{"const":"session.start"}},"required":["op"]},
        "then":{"properties":{"args":{"type":"object","additionalProperties":false,
            "required":["conversation_id"],"properties":start_properties}}}
    }]);
    add_execution_prepare_input_schema(&mut command);
    add_execution_start_input_schema(&mut command);
    add_checkpoint_input_schema(&mut command);
    add_handoff_input_schema(&mut command);
    add_neutral_delivery_schema(&mut command);
    add_evidence_input_schema(&mut command, &mut query);
    add_review_input_schema(&mut command);
    add_baseline_transition_schema(&mut command);
    add_execution_report_input_schema(&mut command);
    command["properties"]["args"]["properties"]["assignee_person_id"] = json!({"type":"string","minLength":1,"maxLength":128,
        "x-identity-utf8":identity_runtime.clone(),
        "description":"task.assign: active eligible project member. This is a target, never the acting identity. JSON Schema maxLength counts characters; runtime also limits this identity to 128 UTF-8 bytes and rejects control characters."});
    command["properties"]["args"]["properties"]["expected_responsibility_version"] = json!({"type":"string","pattern":"^(0|[1-9][0-9]*)$",
        "description":"Use work.prepare responsibility.version; distinct from source ownership and temporary lease versions."});
    command["properties"]["args"]["properties"]["assignment_request_key"] = json!({"type":"string","minLength":1,"maxLength":128,
        "x-identity-utf8":identity_runtime,
        "description":"task.accept_assignment: current responsibility.pending.transfer_request_key. Not a handoff ID. JSON Schema maxLength counts characters; runtime also limits this identity to 128 UTF-8 bytes and rejects control characters."});
    command["properties"]["args"]["properties"]["ttl_seconds"] =
        json!({"type":"integer","minimum":1,"maximum":3600});
    let old_description = command["properties"]["args"]["description"]
        .as_str()
        .unwrap()
        .to_owned();
    command["properties"]["args"]["description"] = json!(format!(
        "task.assign: assignee_person_id, expected_responsibility_version; no execution session required. An optional session_id must include expected_session_version. task.accept_assignment / task.claim_available: session_id, expected_session_version, expected_responsibility_version, expected_work_version, ttl_seconds; acceptance also requires assignment_request_key. Assignment may reserve blocked dependencies; taking work cannot bypass them. Ownership and the coordination lease commit atomically; execution.start remains separate. {old_description}"
    ));
    let session_id = command["properties"]["args"]["properties"]["session_id"].clone();
    let expected_session_version =
        command["properties"]["args"]["properties"]["expected_session_version"].clone();
    let mut nullable_session_id = session_id.clone();
    nullable_session_id["type"] = json!(["string", "null"]);
    let mut nullable_expected_session_version = expected_session_version.clone();
    nullable_expected_session_version["type"] = json!(["string", "null"]);
    for (op, required, optional) in [
        (
            "task.assign",
            vec!["assignee_person_id", "expected_responsibility_version"],
            vec!["session_id", "expected_session_version"],
        ),
        (
            "task.accept_assignment",
            vec![
                "session_id",
                "expected_session_version",
                "expected_responsibility_version",
                "expected_work_version",
                "ttl_seconds",
                "assignment_request_key",
            ],
            vec![],
        ),
        (
            "task.claim_available",
            vec![
                "session_id",
                "expected_session_version",
                "expected_responsibility_version",
                "expected_work_version",
                "ttl_seconds",
            ],
            vec![],
        ),
    ] {
        let mut properties: serde_json::Map<String, Value> = {
            let fields = &command["properties"]["args"]["properties"];
            required
                .iter()
                .chain(&optional)
                .map(|name| ((*name).to_owned(), fields[*name].clone()))
                .collect()
        };
        if op == "task.assign" {
            properties.insert("session_id".into(), nullable_session_id.clone());
            properties.insert(
                "expected_session_version".into(),
                nullable_expected_session_version.clone(),
            );
        }
        let session_pair = (op == "task.assign").then(|| {
            json!([
                {"if":{"required":["session_id"],"properties":{"session_id":session_id}},
                    "then":{"required":["expected_session_version"],
                        "properties":{"expected_session_version":expected_session_version}}},
                {"if":{"required":["expected_session_version"],
                        "properties":{"expected_session_version":expected_session_version}},
                    "then":{"required":["session_id"],"properties":{"session_id":session_id}}}
            ])
        });
        let mut args = json!({
            "type":"object",
            "additionalProperties":false,
            "required":required,
            "properties":properties
        });
        if let Some(session_pair) = session_pair {
            args["allOf"] = session_pair;
        }
        command["allOf"].as_array_mut().unwrap().push(json!({
            "if":{"properties":{"op":{"const":op}},"required":["op"]},
            "then":{"properties":{"args":args}}}));
    }
    add_remaining_closed_command_schemas(&mut command);
    command["properties"]["args"]["properties"]["session_id"] = nullable_session_id;
    command["properties"]["args"]["properties"]["expected_session_version"] =
        nullable_expected_session_version;
    let access_plan = json!({
        "type":"object","additionalProperties":false,
        "required":["protocol_version","subject","subject_client_id","role","grants"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "subject":{"type":"object","additionalProperties":false,
                "required":["id","kind","display_name"],
                "properties":{
                    "id":{"type":"string","maxLength":128},
                    "kind":{"type":"string","enum":["human","agent","system"]},
                    "display_name":{"type":"string","maxLength":512}
                }},
            "subject_client_id":{"type":"string","maxLength":128},
            "role":{"type":"string","enum":["reader","reviewer","worker","admin","developer","maintainer","project_admin"]},
            "grants":{"type":"array","maxItems":256,"items":{"type":"object","additionalProperties":false,
                "required":["workstream_id","authority_version","read","write","manage","attest_execution","reconcile_execution"],
                "properties":{
                    "workstream_id":{"type":"string"},
                    "authority_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
                    "read":{"type":"boolean"},"write":{"type":"boolean"},"manage":{"type":"boolean"},
                    "attest_execution":{"type":"boolean","const":false},
                    "reconcile_execution":{"type":"boolean","const":false}
                }}},
            "credential":{"type":["object","null"],"additionalProperties":false,
                "required":["id","secret_hash"],
                "properties":{
                    "id":{"type":"string","maxLength":128},
                    "secret_hash":{"type":"string","pattern":"^sha256:[0-9a-f]{64}$"},
                    "expires_at_unix_ms":{"type":["integer","null"]}
                }},
            "independent_review":{"type":"boolean","default":false},
            "business_roles":{"type":["array","null"],"minItems":1,"maxItems":6,"uniqueItems":true,
                "items":{"type":"string","enum":["observer","developer","reviewer","supervisor","deliverer","administrator"]},
                "description":"Explicit combined duty ceilings intersect existing grants. Omission/null on update preserves the current declaration; names alone grant no authority."},
            "assignment_grant":{"type":["boolean","null"],"description":"Explicit work.assign membership grant; omission preserves the current grant. Requires maintainer or project_admin and a matching Agent delegation."},
            "agent_review":{"type":"boolean","default":false},
            "credential_project_scoped":{"type":"boolean","default":false},
            "revoke_project_credentials":{"type":"array","maxItems":256,"items":{"type":"string","maxLength":128}},
            "remove_membership":{"type":"boolean","default":false},
            "revoke_tenant_credentials":{"type":"array","maxItems":256,"items":{"type":"string"},
                "description":"Must be empty for project-admin MCP; tenant credential revoke is owner-only."}
        }
    });
    let access_preview = json!({"type":"object","additionalProperties":false,
        "required":["protocol_version","plan"],
        "properties":{"protocol_version":{"type":"integer","const":1},"plan":access_plan}});
    let access_apply = json!({"type":"object","additionalProperties":false,
    "required":["protocol_version","request_id","expected_state","expected_plan","plan"],
    "properties":{
        "protocol_version":{"type":"integer","const":1},
        "request_id":{"type":"string","maxLength":128},
        "expected_state":{"type":"string","pattern":"^[0-9a-f]{64}$"},
        "expected_plan":{"type":"string","pattern":"^[0-9a-f]{64}$"},
        "plan":access_plan
    }});
    let access_outcome = json!({"type":"object","additionalProperties":false,
    "required":["protocol_version","request_id"],
    "properties":{
        "protocol_version":{"type":"integer","const":1},
        "request_id":{"type":"string","maxLength":128}
    }});
    let access_inspect = json!({"type":"object","additionalProperties":false,
    "required":["protocol_version"],
    "properties":{
        "protocol_version":{"type":"integer","const":1},
        "subject_actor_id":{"type":"string","maxLength":128},
        "subject_client_id":{"type":"string","maxLength":128},
        "cursor":{"type":"string","maxLength":128},
        "limit":{"type":"integer","minimum":1,"maximum":100}
    }});
    let planning_suggest = json!({
        "type":"object","additionalProperties":false,
        "required":["protocol_version","request_id","rationale","affected_work_keys"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "request_id":{"type":"string","maxLength":128},
            "rationale":{"type":"string","minLength":1,"maxLength":8192},
            "affected_work_keys":{"type":"array","maxItems":256,"items":{"type":"string","maxLength":128}},
            "proposed_notes":{},
            "author_person_id":{"type":["string","null"],"maxLength":128}
        }
    });
    let mut planning_draft = json!({
        "type":"object","additionalProperties":false,
        "required":["protocol_version","request_id","mode","changes"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "request_id":{"type":"string","maxLength":128},
            "mode":{"type":"string","enum":["create","edit"]},
            "candidate_id":{"type":["string","null"],"maxLength":128},
            "changes":{"type":"array","maxItems":256},
            "suggestion_ids":{"type":"array","maxItems":256,"items":{"type":"string"}},
            "allowed_spec_roots":{"type":"array","maxItems":64,"items":{"type":"string"}},
            "project_goal_keys":{"type":"array","maxItems":64,"items":{"type":"string"}},
            "self_approve_policy":{"type":["object","null"]},
            "author_person_id":{"type":["string","null"],"maxLength":128}
        }
    });
    planning_schema::add(&mut planning_draft);
    let planning_preview = json!({
        "type":"object","additionalProperties":false,
        "required":["protocol_version","candidate_id"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "candidate_id":{"type":"string","maxLength":128}
        }
    });
    let planning_approve = json!({
        "type":"object","additionalProperties":false,
        "required":["protocol_version","request_id","candidate_id","candidate_digest"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "request_id":{"type":"string","maxLength":128},
            "candidate_id":{"type":"string","maxLength":128},
            "candidate_digest":{"type":"string","pattern":"^[0-9a-f]{64}$"},
            "author_person_id":{"type":["string","null"],"maxLength":128}
        }
    });
    let planning_publish = json!({
        "type":"object","additionalProperties":false,
        "required":["protocol_version","request_id","candidate_id","candidate_digest"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "request_id":{"type":"string","maxLength":128},
            "candidate_id":{"type":"string","maxLength":128},
            "candidate_digest":{"type":"string","pattern":"^[0-9a-f]{64}$"},
            "activate":{"type":"boolean","default":false},
            "impact_proven":{"type":"boolean","default":false},
            "publish_receipt_id":{"type":["string","null"],"maxLength":128},"stopped_work_ids":{"type":"array","maxItems":256,"items":{"type":"string","maxLength":128}}
        }
    });
    let planning_outcome = json!({
        "type":"object","additionalProperties":false,
        "required":["protocol_version","request_id"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "request_id":{"type":"string","maxLength":128}
        }
    });
    vec![
        input_contracts::tool(),
        Tool::new("awr_team_query",
            "Scoped Team reads. Begin with capabilities (current identity/permissions), then work.next (own sessions and scoped task navigation). Consume work.prepare before any claim or execution. Use work.snapshot for context and observation in one read transaction, within max_context_bytes; its context_hash remains compatible with work.prepare. Supervisors use selector-free work.inbox. work.observe shares bounded neutral candidate/check/review/integration/publication facts and independently scoped role guidance with prepare/snapshot/inbox, preserving context hashes. Follow one action.op and its optional action.query; recheck_on names refresh triggers. Required context and unknown effects retain priority. max_context_bytes bounds the full observation response. Receipt access and unknown model/usage remain explicit. audit.requests and audit.development provide paged metadata/history under the same personal/project permission boundary. Ops audit: audit.history / audit.export / audit.count (TMCP-040) — members see own allowed records; project-wide requires audit.read_project. Counts/exports use the same scope. Not full chat/tool-IO/token billing; PG audit does not claim DB-owner non-repudiation. Tool discovery is navigation-only; each query rechecks authority. Re-prepare after relevant changes. No execution admission. Approved cross-stream content: delivery.exports with consumer work_id, then artifact.content or bounded artifact.segment with export_id and expected_sha256. For segments follow every next_query, concatenate contiguous ranges from zero and verify the full length/digest; the last page alone is not a complete read. Disclosure never satisfies adoption. Neutral facts: delivery.neutral.inspect. Current source synchronization: delivery.source.status. Missing neutral command response: delivery.neutral.outcome with actual work_id and original client retry ID. delivery.integration.inspect uses the generated IntegrationRequest.request_id (data.integration_id), preserving the original candidate. Neutral inspection includes scoped connector versions; review.inspect includes decision IDs. Descriptions grant no authority.",
            query.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_command",
            "Supervisor assignment: task.assign. Member intake: task.accept_assignment or task.claim_available (self-claim). These commands use the current work.prepare header and responsibility versions; existing-work assignment needs no planning draft. Durable sessions, claims, confirmed handoffs and caller-managed execution under the shared TMCP action gate. Use work.prepare preconditions and a stable request_id. Readers cannot claim or write. Developers may maintain own session/execution on authorized work but cannot edit/publish plans or grant permissions. Only a fresh execution.start response with execution_authorized=true permits one run under the live lease. Exact replay reuses the original receipt; changed intent or expired/revoked authority is refused. Body fields cannot forge identity. Preparation, inspection and replay grant no execution rights. For a missing or uncertain neutral-delivery response use delivery.neutral.outcome; for a missing or uncertain legacy-command response use command.inspect before an exact retry. A returned outcome=known_rejection is final for that attempt, so follow its bounded action guidance instead. Never repeat effects from a receipt. Refresh after conflicts or lease/contract changes. Cancellation is a request after start. Reports remain caller_asserted. Attestation requires operator-issued system authority at admission and now. For unknown effects, execution.inspect then operator execution.reconcile; confirm current versions and latest receipt. Recheck on permission, receipt or work changes. Settlement is not work completion. Evidence/review/rework/complete reuse WS-018 under explicit review.decide and delivery.finalize permissions on the same MCP plane: evidence.submit, delivery.evidence.submit, review.open, delivery.submit_and_request_review, review.accept, review.return, review.decide, work.rework, work.complete, delivery.finalize, plus delivery.register_pr / delivery.observe_pr for versioned PR bindings. Neutral candidate/inspection/facts/source operations use args.source_snapshot_id and the common header; eligible human/system management is required for connector and source publication. They store observations or source metadata. delivery.integration.prepare requires current delivery.finalize permission plus exact connector, selection, evidence and review IDs; it reserves an intent. delivery.integration.reject_prepared only rejects before dispatch. Neither exposes a worker permit or executes Git; inspect the generated integration_id through delivery.integration.inspect. No implicit approval or finalization. Author, owner, executor, reviewer and final-submitter are attributed separately on completion. GitHub submitted/approved/merged are distinct from AWR acceptance; URL, green CI, admin role or already-merged cannot skip acceptance. Head/contract/artifact mismatch invalidates approvals. Independence is by person; a second agent of the same person is not team-independent. The explicit caller_managed_execution_and_simulated_member_review policy supports review.decide only when capabilities.simulated_member_review is present: unique active member bindings, explicit Agent review grants and live Review delegation, with member/actor/client distinct from the immutable executor, submitter and round opener. One controller may operate distinct simulated members. Inspect full origins through review.inspect; default receipts carry short summaries. Simulated finalization requires current delivery.finalize authority, actual artifact bytes, settled successful execution and the exact immutable review basis. Agents need an explicit live finalize_delivery delegation; Review or development grants are insufficient. Neutral fast-forward integration eligibility reuses that basis; preparation grants no repository rights or effect. Default completion receipts add only a basis summary; human/team-human approval flags stay false. Recheck after authority, source, evidence or review changes. AWR acceptance, repository integration and confirmed source publication remain distinct facts. Explicit caller_managed_execution_and_agent_review contracts allow review.decide with both agent_review membership and live Review delegation, a distinct author actor/client, and false human/team-acceptance flags; completion additionally requires artifact bytes, matching input/output bindings and a successful caller report reconciled by an authorized operator. Its evidence stays caller_asserted. Same-stream inputs require explicit per-edge dependency_acceptance: V2 agent_reviewed_caller_asserted_reconciled or V5 simulated_member_independent. Omitted Agent or simulated inputs remain blocked; original selected receipt, current contract and immutable review must match. Approved disclosure uses delivery.export.publish/revoke under delivery.finalize; capabilities.approved_artifact_exports lists exact arguments. V6 current_contract cross-stream inputs require delivery.exports, exact artifact.content, then consumer delivery.adopt under execution.request_and_report_own with a live session and responsibility/export/disclosure/adoption selectors from fresh work.prepare. Pool members and their pending assignees may adopt before claiming/accepting; reading alone cannot unlock work. All intake, start and finalization gates revalidate the selected adoption. Revocation or relevant version/proof changes require re-prepare and suitable republication/adoption. fixed_delivery history remains unavailable; never substitute current_contract. Adoption and replays grant no execution rights. PR facts require fact_source+observed_at: Agent verification uses operator_recorded_observation; authorized_human_github_verification requires an actual human observation. No webhook auto-sync claim. Completion receipts are for WS-030 adoption and omit provider-private sessions.",
            command.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_access_inspect",
            "Project-admin only. Omit both subject selectors to list a bounded member directory (cursor/limit); supply both to inspect one actor/client. Returns permitted memberships, grants and credential metadata, never raw secrets. Requires access.manage_project and explicit manage grants.",
            access_inspect.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_access_preview",
            "Project-admin only. Preview a project-bounded member/role/grant/credential-hash plan without mutation. Impact labels membership sharing and refuses tenant-wide credential revoke. Generate credentials in a protected operator or browser channel; only secret_hash may appear here. Set credential_project_scoped for member credentials; revoke_project_credentials cannot revoke legacy tenant credentials. Requires access.manage_project.",
            access_preview.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_access_apply",
            "Project-admin only. Apply the exact reviewed access plan digests. Exact request_id replay returns the historical receipt. Last-admin removal without handoff, tenant credential revoke, attest/reconcile grants, and non-admin callers are refused. Never accepts or returns raw secrets. Requires access.manage_project.",
            access_apply.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(true).idempotent(true).open_world(false)),
        Tool::new("awr_team_access_outcome",
            "Project-admin only. Query the receipt for an original access.apply request_id before retrying. Returns redacted identity and auth metadata only. Requires access.manage_project.",
            access_outcome.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_planning_suggest",
            "Submit a planning suggestion (planning.propose). Not claimable and does not add formal work or mutate live deps/acceptance. Uses the same identity/action gate as HTTP. Stable request_id for idempotent receipts.",
            planning_suggest.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_planning_draft",
            "Create or edit a planning draft candidate (planning.edit_draft). Split/cancel/archive are DraftChange ops inside changes. V4 supports reviewed workspace declarations; V5 adds explicit simulated-member dependency assurance. Neither grants execution/review authority. Preview and approve the current digest before publishing. No hard-delete of history or forging done via status. Stable request_id.",
            planning_draft.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_planning_preview",
            "Preview exact diffs, affected tasks, runtime impact and review requirements for a planning candidate. Read-only; does not mutate.",
            planning_preview.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_planning_approve",
            "Approve a planning candidate bound to its current digest (planning.approve). Edited drafts cannot reuse old approvals.",
            planning_approve.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_planning_publish",
            "Publish an approved candidate (planning.publish). Optional activate uses the registered sole source only — no client path/URL/SQL. On disconnect, query outcome with the same request_id.",
            planning_publish.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(true).idempotent(true).open_world(false)),
        Tool::new("awr_team_planning_outcome",
            "Query the receipt for an original planning request_id before retrying. Absent receipt is unknown — never resubmit with a new ID.",
            planning_outcome.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false)),
    ]
}

impl ServerHandler for Endpoint {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("awr-team-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions("The URL binds one operator-registered project; bearer credentials are checked on every request. Begin with awr_team_query capabilities, then work.next to resume own work or discover scoped candidates without manual task selection. Supervisors use work.inbox for current permission-filtered coordination; both entries omit work/workstream/session selectors. Follow one next_query and refresh after relevant changes. Project admins manage members via awr_team_access_* tools after local owner bootstrap of the first admin. Raw credentials are never accepted or returned over MCP — generate them through protected Inspector issuance or awr-server access token and register only secret_hash. Work/session selectors bind a workstream; missing permissions never mean satisfied dependencies. Consume work.prepare before checkpointing. Follow its single guidance item (when/because/action/recheck_on); it grants no authority. Report client_info from known host metadata at session start or next checkpoint. Batch progress at phase/test completion, blocker, user wait or delivery boundaries; no per-tool reporting loop. Only submit usage with known host counter scope. Routine progress uses session.checkpoint; execution.report is for terminal outcomes. Session journals and claims grant no execution rights. Claim replay is a historical receipt; use claim.inspect for current lease state. If a command outcome is unknown, inspect its original request_id before an exact retry: delivery.neutral.outcome for neutral operations, command.inspect for legacy coordination. Check delivery.neutral.inspect and delivery.source.status for current state. Planning mutations use awr_team_planning_* with stable request_id; on disconnect call awr_team_planning_outcome or planning.outcome before any new ID. Controlled source/artifact content uses source.content / artifact.content; for long artifacts use artifact.segment with the exact digest, follow every next_query, concatenate contiguous ranges and verify length/digest — never path/URL/history bypass. Ops history uses audit.history/export/count within authorized scope. Recheck context and permission after relevant changes. MCP connection closure never closes a durable work session.")
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        catalog().into_iter().find(|tool| tool.name == name)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request.is_some_and(|r| r.cursor.is_some()) {
            return Err(ErrorData::invalid_params(
                "tool catalog has no next cursor",
                None,
            ));
        }
        let mut result = ListToolsResult::default();
        result.tools = catalog();
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let access = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|p| p.extensions.get::<RequestAccess>())
            .cloned()
            .ok_or_else(|| ErrorData::invalid_params("authenticated request required", None))?;
        let args = Value::Object(request.arguments.unwrap_or_default());
        let access_tool = matches!(
            request.name.as_ref(),
            "awr_team_access_inspect"
                | "awr_team_access_preview"
                | "awr_team_access_apply"
                | "awr_team_access_outcome"
        );
        let forge_ok = if access_tool {
            super::reject_access_management_forgeries(&args)
        } else {
            super::reject_forged_authority_fields(&args)
        };
        if forge_ok.is_err() {
            return Ok(CallToolResult::structured_error(json!({
                "code":"Forbidden","message":"access denied"
            }))
            .into());
        }
        let action = match request.name.as_ref() {
            "awr_team_query" => args
                .get("op")
                .and_then(Value::as_str)
                .filter(|op| WorkstreamQuery::OPERATIONS.contains(op))
                .unwrap_or("invalid.query"),
            "awr_team_command" => args
                .get("op")
                .and_then(Value::as_str)
                .filter(|op| super::command_action_name(op).is_some())
                .unwrap_or("invalid.command"),
            name if catalog().iter().any(|t| t.name == name) => name,
            _ => "invalid.tool",
        }
        .to_owned();
        let work = args
            .get("work_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let diagnostic_input =
            matches!(request.name.as_ref(), "awr_team_query" | "awr_team_command")
                .then(|| args.clone());
        let input_contract = (request.name == input_contracts::TOOL_NAME).then(|| args.clone());
        let write = request.name == "awr_team_command";
        let segmented = request.name == "awr_team_query" && args["op"] == "artifact.segment";
        let result = tokio::time::timeout_at(access.deadline, super::audited(&self.state, &self.project, &access.bearer, &action, work.as_deref(), async {
            match request.name.as_ref() {
                input_contracts::TOOL_NAME => input_contracts::describe(&args, &catalog())
                    .map_err(|_| PgError::Protocol("invalid input contract request".into())),
                "awr_team_query" => {
                    let q: WorkstreamQuery = serde_json::from_value(args)
                        .map_err(|_| PgError::Protocol("invalid query".into()))?;
                    self.state
                        .store
                        .query(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            q,
                        )
                        .await
                }
                "awr_team_command" => {
                    let c: WorkstreamCommand = serde_json::from_value(args)
                        .map_err(|_| PgError::invalid_command_fields())?;
                    self.state
                        .commands
                        .execute(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            c,
                        )
                        .await
                }
                "awr_team_access_inspect" => {
                    let req: super::AccessInspectBody = serde_json::from_value(args)
                        .map_err(|_| PgError::Protocol("invalid access inspect".into()))?;
                    req.inspect(&self.state, &self.project, &access.bearer).await
                }
                "awr_team_access_preview" => {
                    let plan: awr_team_pg::AdminAccessPlan =
                        serde_json::from_value(args.get("plan").cloned().unwrap_or(Value::Null))
                            .map_err(|_| PgError::Protocol("invalid access plan".into()))?;
                    self.state
                        .store
                        .project_access()
                        .preview(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            &plan,
                        )
                        .await
                }
                "awr_team_access_apply" => {
                    let plan: awr_team_pg::AdminAccessPlan =
                        serde_json::from_value(args.get("plan").cloned().unwrap_or(Value::Null))
                            .map_err(|_| PgError::Protocol("invalid access plan".into()))?;
                    let request_id = args
                        .get("request_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| PgError::Protocol("invalid access apply".into()))?;
                    let expected_state = args
                        .get("expected_state")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| PgError::Protocol("invalid access apply".into()))?;
                    let expected_plan = args
                        .get("expected_plan")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| PgError::Protocol("invalid access apply".into()))?;
                    self.state
                        .store
                        .project_access()
                        .apply(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            &plan,
                            request_id,
                            expected_state,
                            expected_plan,
                        )
                        .await
                }
                "awr_team_access_outcome" => {
                    let request_id = args
                        .get("request_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| PgError::Protocol("invalid access outcome".into()))?;
                    self.state
                        .store
                        .project_access()
                        .outcome(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            request_id,
                        )
                        .await
                }
                "awr_team_planning_suggest" => {
                    let req: awr_team_pg::PlanningSuggestRequest = serde_json::from_value(args)
                        .map_err(|_| PgError::Protocol("invalid planning suggest".into()))?;
                    self.state
                        .store
                        .source()
                        .planning_suggest(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            &req,
                        )
                        .await
                }
                "awr_team_planning_draft" => {
                    let req: awr_team_pg::PlanningDraftRequest = serde_json::from_value(args)
                        .map_err(|_| PgError::Protocol("invalid planning draft".into()))?;
                    self.state
                        .store
                        .source()
                        .planning_draft(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            &req,
                        )
                        .await
                }
                "awr_team_planning_preview" => {
                    let candidate_id = args
                        .get("candidate_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| PgError::Protocol("invalid planning preview".into()))?;
                    self.state
                        .store
                        .source()
                        .preview_planning_candidate(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            candidate_id,
                        )
                        .await
                }
                "awr_team_planning_approve" => {
                    let req: awr_team_pg::PlanningApproveRequest = serde_json::from_value(args)
                        .map_err(|_| PgError::Protocol("invalid planning approve".into()))?;
                    self.state
                        .store
                        .source()
                        .planning_approve(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            &req,
                        )
                        .await
                }
                "awr_team_planning_publish" => {
                    let req: awr_team_pg::PlanningPublishRequest = serde_json::from_value(args)
                        .map_err(|_| PgError::Protocol("invalid planning publish".into()))?;
                    self.state
                        .store
                        .source()
                        .planning_publish(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            &req,
                        )
                        .await
                }
                "awr_team_planning_outcome" => {
                    let request_id = args
                        .get("request_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| PgError::Protocol("invalid planning outcome".into()))?;
                    match self
                        .state
                        .store
                        .source()
                        .get_planning_command_receipt(
                            &self.project.tenant_id,
                            &self.project.project_id,
                            &access.bearer,
                            request_id,
                        )
                        .await?
                    {
                        Some(v) => Ok(v),
                        None => Ok(json!({
                            "protocol":"awr-team-planning-command-v1",
                            "request_id":request_id,
                            "already_recorded":false,
                            "result":null,
                            "next_step":"absent receipt is unknown — wait/retry outcome before submitting a new request_id"
                        })),
                    }
                }
                _ => Err(PgError::Unsupported("tool unavailable".into())),
            }
        }))
        .await;
        let result = match result {
            Ok(Ok(value)) => {
                let result = CallToolResult::structured(value);
                // Bound the actual text + structured-content envelope, not just
                // the raw chunk. No oversized prefix is ever emitted.
                if segmented
                    && !serde_json::to_vec(&result).is_ok_and(|bytes| {
                        bytes.len() <= WorkstreamQuery::MAX_ARTIFACT_SEGMENT_WIRE_BYTES
                    })
                {
                    CallToolResult::structured_error(json!({"code":"ResponseTooLarge",
                        "message":"artifact segment transport envelope exceeds its bound",
                        "action_guidance":{"condition":"The complete segment response is too large.",
                            "basis":"The MCP text and structured result share a bounded wire budget.",
                            "next_action":"Repeat this read with a smaller max_context_bytes, such as 1024; keep the same digest and cursor.",
                            "recheck":"After reducing the read budget. This grants no execution or review authority."}}))
                } else {
                    result
                }
            }
            Ok(Err(error)) => {
                let rejection = super::claim_rejection(&action, &error)
                    .or_else(|| super::session_end_rejection(&action, &error));
                let error = super::legacy_claim_acquire_error(&action, error);
                let mut value = if let Some(input) = &input_contract {
                    input_contracts::public_error(input)
                } else {
                    public_error(super::legacy_session_end_error(&action, error)).1
                };
                if let Some(rejection) = rejection {
                    value["rejection"] = rejection;
                }
                if let Some(input) = diagnostic_input {
                    add_input_diagnostic(&mut value, &input, write);
                }
                CallToolResult::structured_error(value)
            }
            Err(_) => CallToolResult::structured_error(unavailable_value()),
        };
        Ok(result.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn manifest_mismatch_has_specific_bounded_guidance_without_echoing_content() {
        let (status, response) = public_error(PgError::manifest_digest_mismatch());
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["code"], "InvalidInput");
        assert_eq!(response["reason"], "manifest_digest_mismatch");
        assert_eq!(
            response["invalid_field"],
            "/args/candidate/binding/manifest_digest"
        );
        assert_eq!(
            response["action_guidance"]["action"]["op"],
            "delivery.submission.describe"
        );
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
        let (_, unknown) = public_error(PgError::Protocol("private-manifest-detail".into()));
        assert_eq!(unknown["code"], "InvalidInput");
        assert!(unknown.get("reason").is_none());
        assert!(!unknown.to_string().contains("private-manifest-detail"));
    }

    #[test]
    fn delivery_connector_discovery_exposes_only_safe_managed_source_locator_shapes() {
        let tool = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let locator = &tool.input_schema["properties"]["args"]["properties"]["mapping"]["properties"]
            ["source_locator"];
        assert_eq!(locator["oneOf"][0]["properties"]["action"]["const"], "set");
        assert_eq!(
            locator["oneOf"][0]["properties"]["locator"]["properties"]["contract"]["const"],
            "awr-git-source-v1"
        );
        assert_eq!(
            locator["oneOf"][0]["properties"]["locator"]["properties"]["url"]["pattern"],
            "^(https://|ssh://git@)"
        );
        assert_eq!(
            locator["oneOf"][1]["properties"]["action"]["const"],
            "clear"
        );
        assert_eq!(locator["oneOf"][2]["type"], "null");
        assert!(
            tool.input_schema["properties"]["args"]["properties"]["mapping"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .all(|field| field != "source_locator"),
            "old connector requests must remain valid"
        );
    }

    #[test]
    fn rejected_claim_acquire_names_its_reason_without_changing_the_response() {
        let rejection =
            crate::service::claim_rejection("claim.acquire", &PgError::ClaimHeld).unwrap();
        assert_eq!(rejection["reason"], "ALREADY_CLAIMED");
        assert_eq!(rejection["operation"], "claim.acquire");
        assert_eq!(rejection["authority"], "coordinator");
        for (error, reason) in [
            (PgError::StaleFence, "STALE_FENCE"),
            (PgError::EpochChanged, "EPOCH_CHANGED"),
            (PgError::WaitOpen, "WAIT_OPEN"),
            (PgError::PreconditionsChanged, "PRECONDITIONS_CHANGED"),
            (PgError::RecoveryBlocked, "RECOVERY_BLOCKED"),
        ] {
            assert_eq!(
                crate::service::claim_rejection("claim.acquire", &error).unwrap()["reason"],
                reason
            );
        }
        // Only claim.acquire carries the reason, and the legacy response is untouched.
        assert!(crate::service::claim_rejection("claim.renew", &PgError::ClaimHeld).is_none());
        assert!(
            crate::service::claim_rejection("task.accept_assignment", &PgError::ClaimHeld)
                .is_none()
        );
        assert!(crate::service::claim_rejection("claim.acquire", &PgError::Forbidden).is_none());
        let (status, value) = public_error(PgError::ClaimHeld);
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(value["code"], "ClaimHeld");
        assert!(value.get("rejection").is_none());
        // The finer epoch/fence fact never reaches the top-level code of acquire.
        for error in [PgError::EpochChanged, PgError::StaleFence] {
            let (status, value) = public_error(crate::service::legacy_claim_acquire_error(
                "claim.acquire",
                error,
            ));
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(value["code"], "RecoveryBlocked");
        }
        // Other commands keep their own codes.
        let (_, renewed) = public_error(crate::service::legacy_claim_acquire_error(
            "claim.renew",
            PgError::EpochChanged,
        ));
        assert_eq!(renewed["code"], "EpochChanged");
    }

    #[test]
    fn rejected_session_end_has_bounded_known_guidance_and_legacy_code() {
        for (error, reason, primary_action, deferred_actions, recheck_fragments) in [
            (
                PgError::RecoveryBlocked,
                "UNRESOLVED_EFFECTS",
                "work.observe",
                &[
                    "execution.report",
                    "execution.reconcile",
                    "claim.release",
                    "session.end",
                    "refresh",
                ][..],
                &[
                    "execution.report",
                    "execution.reconcile",
                    "claim.release",
                    "session.end",
                    "refresh",
                ][..],
            ),
            (
                PgError::WaitOpen,
                "WAIT_OPEN",
                "Resolve the open wait",
                &["claim.release", "session.end", "refresh"][..],
                &[
                    "confirmed wait resolution",
                    "refresh current facts",
                    "remaining close obligations",
                    "active claim",
                    "no close blocker",
                    "session.end",
                ][..],
            ),
            (
                PgError::ClaimHeld,
                "ACTIVE_CLAIM",
                "claim.release",
                &["claim.inspect", "session.end", "refresh"][..],
                &[
                    "claim.release succeeds",
                    "refresh current facts",
                    "no close blocker",
                    "session.end",
                ][..],
            ),
        ] {
            let rejection = crate::service::session_end_rejection("session.end", &error).unwrap();
            assert_eq!(rejection["reason"], reason);
            assert_eq!(rejection["operation"], "session.end");
            assert_eq!(rejection["outcome"], "known_rejection");
            for field in ["when", "because", "action", "recheck_on"] {
                assert!(rejection["action_guidance"][field].is_string());
            }
            let action = rejection["action_guidance"]["action"].as_str().unwrap();
            let recheck = rejection["action_guidance"]["recheck_on"].as_str().unwrap();
            assert!(action.contains(primary_action));
            for deferred in deferred_actions {
                assert!(!action.contains(deferred));
            }
            for fragment in recheck_fragments {
                assert!(recheck.contains(fragment));
            }
            assert!(!rejection.to_string().contains("command.inspect"));
            let (status, mut response) = public_error(crate::service::legacy_session_end_error(
                "session.end",
                error,
            ));
            response["rejection"] = rejection;
            assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(response["code"], "RecoveryBlocked");
            assert_eq!(response["message"], "recovery blocked");
        }
        assert!(
            crate::service::session_end_rejection("session.checkpoint", &PgError::ClaimHeld)
                .is_none()
        );
        assert!(
            crate::service::session_end_rejection("session.end", &PgError::Forbidden).is_none()
        );
    }

    #[test]
    fn unavailable_handoff_has_bounded_guidance_without_private_details() {
        let (status, response) = public_error(PgError::handoff_unavailable());
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(response["code"], "HandoffUnavailable");
        assert!(
            response["message"]
                .as_str()
                .unwrap()
                .contains("selected work")
        );
        let next = response["next_step"].as_str().unwrap();
        assert!(next.contains("handoff.id"));
        assert!(next.contains("event, request or checkpoint ID"));
        assert!(next.contains("only your own session"));
        assert!(next.contains("authenticated person identity"));
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
        let (_, unknown) = public_error(PgError::Protocol("private-handoff-record".into()));
        assert_eq!(unknown["code"], "InvalidInput");
        assert!(!unknown.to_string().contains("private-handoff-record"));
    }

    #[test]
    fn missing_handoff_fence_has_precise_bounded_recovery_guidance() {
        let (status, response) = public_error(PgError::missing_handoff_fence());
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["code"], "InvalidInput");
        assert!(
            response["message"]
                .as_str()
                .unwrap()
                .contains("args.expected_current_fence")
        );
        assert!(
            response["next_step"]
                .as_str()
                .unwrap()
                .contains("runtime.last_fence")
        );
        assert!(
            response["next_step"]
                .as_str()
                .unwrap()
                .contains("terminal sender")
        );
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
        let (_, unknown) = public_error(PgError::Protocol("untrusted-private-detail".into()));
        assert!(
            !serde_json::to_string(&unknown)
                .unwrap()
                .contains("untrusted-private-detail")
        );
    }

    #[test]
    fn handoff_discovery_preserves_core_execution_wire_variants() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_command")
            .unwrap();
        let fields = &tool.input_schema["properties"]["args"]["properties"];
        let cases = [
            awr_core::ExecutionInstance::Person {
                person_id: awr_core::PersonId::new("developer").unwrap(),
            },
            awr_core::ExecutionInstance::AgentRun {
                person_id: awr_core::PersonId::new("developer").unwrap(),
                agent_id: "agent".into(),
                binding_id: "bound-agent".into(),
            },
        ];
        for executor in cases {
            let wire = serde_json::to_value(executor).unwrap();
            let variants = fields["successor_execution"]["oneOf"].as_array().unwrap();
            let matching: Vec<_> = variants
                .iter()
                .filter(|v| v["properties"]["kind"]["const"] == wire["kind"])
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "Every actual wire variant must have one schema branch"
            );
            for required in matching[0]["required"].as_array().unwrap() {
                assert!(wire.get(required.as_str().unwrap()).is_some());
            }
        }
        assert_eq!(
            fields["package"]["properties"]["current_execution"],
            fields["successor_execution"]
        );
        assert_eq!(
            fields["package"]["properties"]["artifact_versions"]["items"]["required"],
            json!(["artifact_id", "version"])
        );
        assert_eq!(
            fields["package"]["properties"]["checkpoint_ids"]["minItems"],
            1
        );
        assert_eq!(fields["now_ms"]["type"], json!(["integer", "null"]));
    }

    #[test]
    fn task_intake_schema_keeps_supervisor_dispatch_separate_from_execution_sessions() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_command")
            .unwrap();
        let props = &tool.input_schema["properties"]["args"]["properties"];
        assert_eq!(
            props["expected_responsibility_version"]["pattern"],
            "^(0|[1-9][0-9]*)$"
        );
        for field in ["session_id", "assignee_person_id", "assignment_request_key"] {
            assert_eq!(props[field]["maxLength"], 128, "{field}");
            assert_eq!(props[field]["x-identity-utf8"]["maxUtf8Bytes"], 128);
            assert_eq!(props[field]["x-identity-utf8"]["rejectControls"], true);
            let description = props[field]["description"].as_str().unwrap();
            assert!(
                description.contains("maxLength counts characters"),
                "{field}"
            );
            assert!(description.contains("128 UTF-8 bytes"), "{field}");
            assert!(
                description.contains("rejects control characters"),
                "{field}"
            );
        }
        for (op, required_fields, optional_fields) in [
            (
                "task.assign",
                vec!["assignee_person_id", "expected_responsibility_version"],
                vec!["session_id", "expected_session_version"],
            ),
            (
                "task.accept_assignment",
                vec![
                    "session_id",
                    "expected_session_version",
                    "expected_responsibility_version",
                    "expected_work_version",
                    "ttl_seconds",
                    "assignment_request_key",
                ],
                vec![],
            ),
            (
                "task.claim_available",
                vec![
                    "session_id",
                    "expected_session_version",
                    "expected_responsibility_version",
                    "expected_work_version",
                    "ttl_seconds",
                ],
                vec![],
            ),
        ] {
            assert!(
                tool.description.as_deref().unwrap().contains(op),
                "Task intake must be discoverable in the command tool description"
            );
            assert!(
                tool.input_schema["properties"]["op"]["enum"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(op))
            );
            let condition = tool.input_schema["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["if"]["properties"]["op"]["const"] == op)
                .unwrap();
            let args = &condition["then"]["properties"]["args"];
            assert_eq!(args["additionalProperties"], false, "{op}");
            let required = args["required"].as_array().unwrap();
            assert!(required.contains(&json!("expected_responsibility_version")));
            assert_eq!(
                required
                    .iter()
                    .map(|field| field.as_str().unwrap())
                    .collect::<std::collections::BTreeSet<_>>(),
                required_fields.iter().copied().collect(),
                "{op}"
            );
            assert_eq!(
                args["properties"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                required_fields
                    .iter()
                    .chain(&optional_fields)
                    .copied()
                    .collect(),
                "{op}"
            );
            assert!(args["properties"].get("conversation_id").is_none());
            if op == "task.assign" {
                assert!(required.contains(&json!("assignee_person_id")));
                assert!(!required.contains(&json!("session_id")));
                let pair = args["allOf"].as_array().unwrap();
                assert_eq!(pair.len(), 2);
                for (condition, provided, counterpart) in [
                    (&pair[0], "session_id", "expected_session_version"),
                    (&pair[1], "expected_session_version", "session_id"),
                ] {
                    assert_eq!(condition["if"]["required"], json!([provided]));
                    assert_eq!(condition["if"]["properties"][provided]["type"], "string");
                    assert_eq!(condition["then"]["required"], json!([counterpart]));
                    assert_eq!(
                        condition["then"]["properties"][counterpart]["type"],
                        "string"
                    );
                }
            } else {
                for key in [
                    "session_id",
                    "expected_session_version",
                    "expected_work_version",
                    "ttl_seconds",
                ] {
                    assert!(required.contains(&json!(key)));
                }
                assert_eq!(
                    required.contains(&json!("assignment_request_key")),
                    op == "task.accept_assignment"
                );
                assert!(args.get("allOf").is_none());
            }
        }
    }

    #[test]
    fn artifact_read_schema_preserves_operation_specific_byte_bounds() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_query")
            .unwrap();
        assert_eq!(
            tool.input_schema["properties"]["max_context_bytes"]["maximum"],
            WorkstreamQuery::MAX_ARTIFACT_BYTES
        );
        assert!(
            tool.input_schema["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .any(|branch| {
                    branch["if"]["properties"]["op"]["enum"]
                        == json!(WorkstreamQuery::CONTEXT_BUDGET_OPERATIONS)
                        && branch["else"]["properties"]["max_context_bytes"]["type"] == "null"
                })
        );
        let condition = tool.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["if"]["properties"]["op"]["const"] == "artifact.content")
            .unwrap();
        assert_eq!(
            condition["then"]["properties"]["max_context_bytes"]["maximum"],
            WorkstreamQuery::MAX_ARTIFACT_BYTES
        );
        assert_eq!(
            condition["else"]["properties"]["max_context_bytes"]["maximum"],
            262144
        );
        assert!(
            tool.input_schema["properties"]["max_context_bytes"]
                .get("default")
                .is_none()
        );
    }

    #[test]
    fn assignee_query_schema_requires_task_scope_and_omits_admin_filters() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_query")
            .unwrap();
        assert!(
            tool.input_schema["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!("task.assignees"))
        );
        let condition = tool.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["if"]["properties"]["op"]["const"] == "task.assignees")
            .unwrap();
        assert_eq!(condition["then"]["required"], json!(["work_id"]));
        for key in [
            "session_id",
            "member_actor_id",
            "include_denies",
            "expected_sha256",
        ] {
            assert!(
                condition["then"]["not"]["anyOf"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["required"] == json!([key]))
            );
        }
    }

    #[test]
    fn handoff_requires_actual_inspection_instead_of_caller_time_and_assurances() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_command")
            .unwrap();
        assert!(
            tool.input_schema["properties"]["args"]
                .get("required")
                .is_none()
        );
        // Review reasons accept 4096 bytes; handoff bounds must not narrow them.
        assert!(
            tool.input_schema["properties"]["args"]["properties"]["reason"]
                .get("maxLength")
                .is_none()
        );
        for operation in [
            "handoff.propose",
            "handoff.inspect",
            "handoff.accept",
            "handoff.reject",
            "handoff.cancel",
            "handoff.timeout",
        ] {
            let condition = tool.input_schema["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["if"]["properties"]["op"]["const"] == operation)
                .unwrap();
            let required = condition["then"]["properties"]["args"]["required"]
                .as_array()
                .unwrap();
            for key in ["session_id", "expected_session_version", "handoff_id"] {
                assert!(required.contains(&json!(key)), "{operation} requires {key}");
            }
            if operation == "handoff.accept" {
                assert!(required.contains(&json!("successor_execution")));
                assert!(required.contains(&json!("inspection_request_id")));
                assert_eq!(
                    condition["then"]["properties"]["args"]["properties"]["inspection_request_id"]
                        ["type"],
                    "string"
                );
            }
            assert!(!required.contains(&json!("now_ms")));
            if operation == "handoff.propose" {
                assert!(!required.contains(&json!("package")));
            }
            if matches!(operation, "handoff.reject" | "handoff.cancel") {
                assert_eq!(
                    condition["then"]["properties"]["args"]["properties"]["reason"]["maxLength"],
                    2048
                );
            }
        }
        let query = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_query")
            .unwrap();
        assert!(
            query.input_schema["properties"]["handoff_id"]["description"]
                .as_str()
                .unwrap()
                .contains("events.list")
        );
    }

    #[test]
    fn evidence_discovery_closes_only_its_args_and_requires_current_selectors() {
        let tools = catalog();
        let command = &tools
            .iter()
            .find(|t| t.name == "awr_team_command")
            .unwrap()
            .input_schema;
        let branch = command["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["if"]["properties"]["op"]["const"] == "evidence.submit")
            .expect("Evidence submission must expose its operation-specific shape");
        let args = &branch["then"]["properties"]["args"];
        assert_eq!(args["additionalProperties"], false);
        assert_eq!(
            args["required"],
            json!([
                "session_id",
                "expected_session_version",
                "payload",
                "dirty_tree"
            ])
        );
        assert_eq!(args["properties"].as_object().unwrap().len(), 9);
        assert!(args["properties"]["payload"].get("type").is_none());
        assert!(args["properties"].get("artifact_sha256").is_none());
        assert!(
            command["properties"]["args"]
                .get("additionalProperties")
                .is_none()
        );
        let query = &tools
            .iter()
            .find(|t| t.name == "awr_team_query")
            .unwrap()
            .input_schema;
        for (op, field) in [
            ("review.inspect", "review_round_id"),
            ("evidence.inspect", "evidence_id"),
        ] {
            let branch = query["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["if"]["properties"]["op"]["const"] == op)
                .expect("Inspection must expose its required record selector");
            assert_eq!(branch["then"]["required"], json!([field]));
        }
    }

    #[test]
    fn query_discovery_reuses_runtime_work_selection_set() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_query")
            .unwrap();
        let branches: Vec<_> = tool.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|branch| {
                branch["if"]["properties"]["op"]["enum"]
                    == json!(WorkstreamQuery::WORK_SELECTION_OPERATIONS)
            })
            .collect();
        assert_eq!(
            branches.len(),
            1,
            "exactly one branch publishes the runtime work-or-session set"
        );
        let then = &branches[0]["then"];
        assert_eq!(
            then["anyOf"],
            json!([{"required":["work_id"]},{"required":["session_id"]}])
        );
        assert!(
            then.get("oneOf").is_none(),
            "anyOf keeps both consistent selectors valid"
        );
        assert!(
            then.get("required").is_none(),
            "work_id alone is never required unconditionally"
        );
        assert_eq!(
            WorkstreamQuery::WORK_SELECTION_OPERATIONS
                .iter()
                .collect::<BTreeSet<_>>()
                .len(),
            WorkstreamQuery::WORK_SELECTION_OPERATIONS.len(),
            "the shared set has no duplicates"
        );
        for op in ["claim.inspect", "review.inspect", "evidence.inspect"] {
            assert!(
                WorkstreamQuery::WORK_SELECTION_OPERATIONS.contains(&op),
                "{op} shares the runtime work-or-session set"
            );
        }
    }

    #[test]
    fn inspection_record_selectors_are_discoverable_and_bounded() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_query")
            .unwrap();
        let schema = &tool.input_schema;
        let claim_id = &schema["properties"]["claim_id"];
        assert_eq!(claim_id["type"], "string");
        assert_eq!(claim_id["minLength"], 1);
        assert_eq!(claim_id["maxLength"], 128);
        assert!(
            claim_id["description"]
                .as_str()
                .unwrap()
                .contains("claim.inspect")
        );
        let claim_description = claim_id["description"].as_str().unwrap();
        assert!(claim_description.contains("either work_id or session_id"));
        let review_round_id = &schema["properties"]["review_round_id"];
        assert_eq!(review_round_id["type"], "string");
        assert_eq!(review_round_id["minLength"], 1);
        assert_eq!(review_round_id["maxLength"], 128);
        for (op, field) in [
            ("claim.inspect", "claim_id"),
            ("review.inspect", "review_round_id"),
            ("evidence.inspect", "evidence_id"),
        ] {
            let branch = schema["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["if"]["properties"]["op"]["const"] == op)
                .expect("inspection must expose its required record selector");
            assert_eq!(branch["then"]["required"], json!([field]), "{op}");
        }
    }

    #[test]
    fn delivery_evidence_discovery_exposes_a_typed_flat_payload_object() {
        let tools = catalog();
        let command = &tools
            .iter()
            .find(|t| t.name == "awr_team_command")
            .unwrap()
            .input_schema;
        assert!(
            command["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!("delivery.evidence.submit"))
        );
        // The flat args vocabulary declares the payload object type directly.
        let flat = &command["properties"]["args"]["properties"];
        assert_eq!(flat["delivery_payload"]["type"], "object");
        assert_eq!(
            flat["delivery_payload"]["required"],
            json!(["delivery_candidate_digest"])
        );
        assert_eq!(
            flat["delivery_payload"]["properties"]["delivery_candidate_digest"]["pattern"],
            "^[0-9a-f]{64}$"
        );
        assert_eq!(
            flat["delivery_payload"]["properties"]["output_digest"]["pattern"],
            "^[0-9a-f]{64}$"
        );
        assert_eq!(
            flat["delivery_payload"]["properties"]["passed"]["type"],
            "boolean"
        );
        // The generic payload stays untyped.
        assert!(flat["payload"].get("type").is_none());
        let branch = command["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["if"]["properties"]["op"]["const"] == "delivery.evidence.submit")
            .expect("Delivery evidence submission must expose its operation-specific shape");
        let args = &branch["then"]["properties"]["args"];
        assert_eq!(args["additionalProperties"], false);
        assert_eq!(
            args["required"],
            json!([
                "session_id",
                "expected_session_version",
                "delivery_payload",
                "dirty_tree"
            ])
        );
        assert_eq!(args["properties"].as_object().unwrap().len(), 9);
        assert!(args["properties"].get("payload").is_none());
        assert_eq!(args["properties"]["delivery_payload"]["type"], "object");
        assert!(
            args["description"]
                .as_str()
                .unwrap()
                .contains("delivery_candidate_digest")
        );
    }

    #[test]
    fn delivery_payload_rejection_is_specific_bounded_and_value_free() {
        let (status, response) = public_error(PgError::delivery_payload_invalid(
            "delivery_candidate_digest",
            "pattern",
        ));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["code"], "InvalidInput");
        assert_eq!(response["reason"], "delivery_payload_invalid");
        assert_eq!(
            response["invalid_field"],
            "/args/delivery_payload/delivery_candidate_digest"
        );
        assert_eq!(response["constraint"], "pattern");
        assert!(
            response["next_step"]
                .as_str()
                .unwrap()
                .contains("delivery.submission.describe")
        );
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
        for (field, constraint) in [
            ("delivery_payload", "required"),
            ("delivery_payload", "type"),
            ("delivery_candidate_digest", "required"),
            ("passed", "type"),
            ("output_digest", "type"),
            ("output_digest", "pattern"),
        ] {
            let (_, response) = public_error(PgError::delivery_payload_invalid(field, constraint));
            assert_eq!(response["reason"], "delivery_payload_invalid", "{field}");
            assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
        }
        // An unrelated Protocol error keeps the generic shape and echoes nothing.
        let (_, generic) = public_error(PgError::Protocol("private-payload-detail".into()));
        assert_eq!(generic["code"], "InvalidInput");
        assert!(generic.get("reason").is_none());
        assert!(!generic.to_string().contains("private-payload-detail"));
    }

    #[test]
    fn discovery_exposes_evidence_inputs_without_narrowing_generic_payloads() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_command")
            .unwrap();
        let args = &tool.input_schema["properties"]["args"];
        let fields = &args["properties"];
        assert!(args.get("required").is_none());
        assert!(fields["payload"].get("type").is_none());
        assert_eq!(fields["dirty_tree"]["type"], "boolean");
        assert_eq!(fields["artifact_text"]["type"], json!(["string", "null"]));
        assert_eq!(fields["artifact_hex"]["type"], json!(["string", "null"]));
        assert_eq!(fields["artifact_text"]["maxLength"], 1_048_576);
        assert_eq!(fields["artifact_hex"]["maxLength"], 2_097_152);
        assert!(
            fields["artifact_text"]["description"]
                .as_str()
                .unwrap()
                .contains("64 KiB")
        );
        let note = args["description"].as_str().unwrap();
        assert!(note.contains("dirty_tree (required boolean)"));
        assert!(note.contains("payload.output_digest matching execution.report"));
        assert!(!note.contains("optional claimed_trust/artifact_hex/input_digest/dirty_tree"));
    }

    #[test]
    fn discovery_exposes_optional_workspace_terminal_report_without_asserting_settlement() {
        let tool = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let args = &tool.input_schema["properties"]["args"];
        let fields = &args["properties"];
        let settlement = &fields["workspace_settlement"];
        assert_eq!(settlement["type"], json!(["object", "null"]));
        assert_eq!(settlement["additionalProperties"], false);
        assert!(args.get("required").is_none());
        let required = settlement["required"].as_array().unwrap();
        assert_eq!(required.len(), 6);
        for name in [
            "workspace_id",
            "input_digest",
            "environment_digest",
            "claim_id",
            "expected_fence",
            "expected_lease_version",
        ] {
            assert!(required.contains(&json!(name)), "{name}");
        }
        for name in ["input_digest", "environment_digest"] {
            assert_eq!(settlement["properties"][name]["type"], "string");
            assert_eq!(settlement["properties"][name]["pattern"], "^[0-9a-f]{64}$");
        }
        for name in ["expected_fence", "expected_lease_version"] {
            assert_eq!(settlement["properties"][name]["type"], "string");
            assert_eq!(settlement["properties"][name]["pattern"], "^[1-9][0-9]*$");
        }
        for name in ["executor_stopped", "no_external_effects"] {
            let declaration = &settlement["properties"][name];
            assert_eq!(declaration["type"], json!(["boolean", "null"]));
            assert!(!required.contains(&json!(name)));
            for inference in ["const", "default", "enum"] {
                assert!(declaration.get(inference).is_none());
            }
        }
        let note = args["description"].as_str().unwrap();
        assert!(note.contains("optional workspace_settlement"));
        assert!(note.contains("even while the lease is live"));
        assert!(note.contains("execution.start/inspect expose terminal_reporting bindings"));
        assert!(note.contains("V2 also permits elapsed time"));
        let declaration_note = settlement["description"].as_str().unwrap();
        assert!(declaration_note.contains("including with a live lease"));
        assert!(!declaration_note.contains("V2 permits only elapsed expiry"));
        assert!(note.contains("workspace-settled success"));
        assert!(!note.contains("never substitute that hash for the execution output digest"));
        assert_eq!(fields["output_digest"]["pattern"], "^[0-9a-f]{64}$");
        assert!(
            fields["output_digest"]["description"]
                .as_str()
                .unwrap()
                .contains("exact artifact bytes")
        );
        let baseline = &fields["baseline_transition"];
        assert_eq!(baseline["additionalProperties"], false);
        assert!(
            baseline["required"]
                .as_array()
                .unwrap()
                .contains(&json!("operation"))
        );
        assert_eq!(
            baseline["properties"]["operation"]["enum"],
            json!(["fast_forward_only", "merge_no_ff"])
        );
        assert_eq!(
            baseline["properties"]["contribution_paths"]["maxItems"],
            128
        );
        assert_eq!(
            baseline["properties"]["contribution_paths"]["uniqueItems"],
            true
        );
        assert!(fields.get("baseline_transition_provenance").is_none());
        assert!(
            fields["observed_paths"]["description"]
                .as_str()
                .unwrap()
                .contains("all actual workspace effects")
        );
    }

    #[test]
    fn discovery_types_nested_command_versions_without_requiring_them_for_every_op() {
        let tool = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let args = &tool.input_schema["properties"]["args"];
        assert!(args.get("required").is_none());
        let fields = &args["properties"];
        assert_eq!(
            fields["expected_session_version"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            fields["expected_session_version"]["pattern"],
            "^[1-9][0-9]*$"
        );
        for name in [
            "expected_fence",
            "expected_lease_version",
            "expected_execution_version",
            "expected_handoff_version",
        ] {
            assert_eq!(fields[name]["type"], "string", "{name}");
            assert_eq!(fields[name]["pattern"], "^[1-9][0-9]*$", "{name}");
        }
        // An initial claim has no runtime version; optional handoff fences
        // retain the parser's existing null/zero semantics.
        assert_eq!(fields["expected_work_version"]["type"], "string");
        assert_eq!(
            fields["expected_work_version"]["pattern"],
            "^(0|[1-9][0-9]*)$"
        );
        assert_eq!(
            fields["expected_current_fence"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            fields["expected_current_fence"]["pattern"],
            "^(0|[1-9][0-9]*)$"
        );
    }

    #[test]
    fn command_diagnostics_are_bounded_and_do_not_expose_internal_protocol_details() {
        let (status, command) = public_error(PgError::invalid_command_fields());
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(command["code"], "InvalidInput");
        assert_eq!(command["message"], "command fields or bounds are invalid");
        assert!(
            command["next_step"]
                .as_str()
                .unwrap()
                .contains("decimal strings")
        );
        assert!(command.to_string().len() < 400);
        let (_, internal) = public_error(PgError::Protocol("private-diagnostic-sentinel".into()));
        assert_eq!(internal["code"], "InvalidInput");
        assert!(!internal.to_string().contains("private-diagnostic-sentinel"));
        assert!(internal.get("next_step").is_none());
    }

    #[tokio::test]
    async fn neutral_review_submission_is_a_bounded_definite_conflict_on_http_and_mcp() {
        let http = error_response(PgError::review_submission_incomplete());
        assert_eq!(http.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(http.into_body(), 1024).await.unwrap();
        assert!(bytes.len() < 800);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "ReviewSubmissionIncomplete");
        assert_eq!(
            body["action_guidance"]["action"]["op"],
            "delivery.neutral.inspect"
        );
        for field in ["when", "because", "action", "recheck_on"] {
            assert!(!body["action_guidance"][field].is_null());
        }
        assert!(!body.to_string().contains("outcome unavailable"));
        let mcp = CallToolResult::structured_error(body.clone());
        assert_eq!(mcp.is_error, Some(true));
        assert_eq!(mcp.structured_content, Some(body));
    }

    #[tokio::test]
    async fn selected_evidence_rejection_diagnoses_the_exact_submitted_binding() {
        let http = error_response(PgError::review_submission_incomplete_reason(
            "candidate_not_bound_in_evidence",
        ));
        assert_eq!(http.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(http.into_body(), 4096).await.unwrap();
        assert!(bytes.len() < 1400);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        // The public code and status are preserved; the reasoned body names
        // the exact submitted evidence and the corrective typed entry.
        assert_eq!(body["code"], "ReviewSubmissionIncomplete");
        assert_eq!(body["reason"], "candidate_not_bound_in_evidence");
        let guidance = &body["action_guidance"];
        for field in ["condition", "basis", "next_action", "recheck"] {
            assert!(
                guidance[field].as_str().is_some_and(|s| !s.is_empty()),
                "{field}"
            );
        }
        let next = guidance["next_action"].as_str().unwrap();
        assert!(next.contains("delivery.evidence.submit"));
        assert!(next.contains("delivery_candidate_digest"));
        assert!(
            guidance["condition"]
                .as_str()
                .unwrap()
                .contains("evidence_id")
        );
        let mcp = CallToolResult::structured_error(body.clone());
        assert_eq!(mcp.is_error, Some(true));
        assert_eq!(mcp.structured_content, Some(body));
        // Every allowlisted reason renders a bounded, fielded diagnosis.
        for reason in [
            "candidate_missing",
            "evidence_missing",
            "evidence_binding_changed",
            "original_candidate_unavailable",
            "evidence_integrity_unavailable",
            "manifest_content_unavailable",
            "selection_changed",
            "submission_incomplete",
        ] {
            let diagnostic = PgError::review_submission_incomplete_reason(reason)
                .review_submission_incomplete_diagnostic()
                .unwrap();
            assert_eq!(diagnostic["code"], "ReviewSubmissionIncomplete");
            assert_eq!(diagnostic["reason"], reason);
            assert!(diagnostic.to_string().len() < 1400, "{reason}");
        }
        // An unknown label never renders and can never echo stored text; the
        // bare legacy form keeps its original generic body.
        let forged = PgError::Protocol(
            "neutral review submission is incomplete or no longer current/private-sentinel".into(),
        );
        assert!(forged.is_review_submission_incomplete());
        assert_eq!(forged.review_submission_reason(), None);
        let (_, body) = public_error(forged);
        assert_eq!(body["code"], "ReviewSubmissionIncomplete");
        assert!(body.get("reason").is_none());
        assert!(!body.to_string().contains("private-sentinel"));
    }

    #[tokio::test]
    async fn simulated_evidence_execution_guidance_is_bounded_and_shared_by_http_and_mcp() {
        let http = error_response(PgError::missing_simulated_evidence_execution());
        assert_eq!(http.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(http.into_body(), 1024).await.unwrap();
        assert!(bytes.len() < 700);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "EvidenceInvalid");
        assert_eq!(body["invalid_field"], "/args/execution_id");
        assert_eq!(body["constraint"], "required_by_completion_policy");
        for field in ["when", "because", "action", "recheck_on"] {
            assert!(body["action_guidance"][field].is_string());
        }
        assert!(
            body["action_guidance"]["when"]
                .as_str()
                .unwrap()
                .contains("auxiliary files")
        );
        assert!(
            body["action_guidance"]["action"]
                .as_str()
                .unwrap()
                .contains("work.observe")
        );
        let mcp = CallToolResult::structured_error(body.clone());
        assert_eq!(mcp.is_error, Some(true));
        assert_eq!(mcp.structured_content, Some(body));
        let (_, generic) = public_error(PgError::EvidenceInvalid);
        assert!(generic.get("invalid_field").is_none());
        assert!(generic.get("action_guidance").is_none());
    }

    #[test]
    fn evidence_discovery_explains_conditional_execution_binding_without_requiring_it_globally() {
        let tool = catalog()
            .into_iter()
            .find(|t| t.name == "awr_team_command")
            .unwrap();
        let branch = tool.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["if"]["properties"]["op"]["const"] == "evidence.submit")
            .unwrap();
        let args = &branch["then"]["properties"]["args"];
        let description = args["description"].as_str().unwrap();
        assert!(description.contains("All simulated-member evidence requires args.execution_id"));
        assert!(description.contains("including auxiliary files"));
        assert!(
            !args["required"]
                .as_array()
                .unwrap()
                .contains(&json!("execution_id"))
        );
        assert_eq!(
            args["properties"]["execution_id"]["type"],
            json!(["string", "null"])
        );
    }

    #[tokio::test]
    async fn completion_rejections_are_actionable_http_conflicts_and_mcp_tool_errors() {
        for (error, code, inspection) in [
            (
                PgError::EvidenceInvalid,
                "EvidenceInvalid",
                "evidence.inspect",
            ),
            (PgError::ReviewRequired, "ReviewRequired", "review.inspect"),
            (
                PgError::CompletionRejected,
                "CompletionRejected",
                "work.prepare",
            ),
            (PgError::PolicyDowngrade, "PolicyDowngrade", "work.prepare"),
        ] {
            let http = error_response(error);
            assert_eq!(http.status(), StatusCode::CONFLICT, "{code}");
            let bytes = axum::body::to_bytes(http.into_body(), 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"], code);
            assert_eq!(body.as_object().unwrap().len(), 3);
            assert!(body["next_step"].as_str().unwrap().contains(inspection));
            assert!(bytes.len() < 700, "guidance must stay bounded: {code}");
            assert!(!body.to_string().contains("outcome unavailable"));

            let mcp = CallToolResult::structured_error(body.clone());
            assert_eq!(mcp.is_error, Some(true));
            assert_eq!(mcp.structured_content, Some(body));
        }
    }

    #[test]
    fn review_operations_discover_their_exact_closed_argument_shapes() {
        let tools = catalog();
        let command = &tools
            .iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap()
            .input_schema;
        let mut schema_bytes = 0;
        for (op, extra, optional) in [
            ("review.open", vec!["evidence_id"], vec![]),
            (
                "delivery.submit_and_request_review",
                vec!["evidence_id"],
                vec![],
            ),
            (
                "review.accept",
                vec!["round_id", "reason"],
                vec!["findings", "resolutions"],
            ),
            (
                "review.return",
                vec!["round_id", "reason"],
                vec!["findings"],
            ),
            (
                "review.decide",
                vec!["round_id", "decision", "reason"],
                vec!["findings", "resolutions"],
            ),
            ("work.rework", vec!["round_id", "note"], vec![]),
            (
                "work.complete",
                vec!["evidence_id", "context_complete"],
                vec!["requested_policy"],
            ),
            (
                "delivery.finalize",
                vec!["evidence_id", "context_complete"],
                vec!["requested_policy"],
            ),
        ] {
            let branch = command["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|branch| branch["if"]["properties"]["op"]["const"] == op)
                .unwrap_or_else(|| panic!("Missing operation-specific schema: {op}"));
            let args = &branch["then"]["properties"]["args"];
            schema_bytes += branch.to_string().len();
            assert_eq!(args["additionalProperties"], false, "{op}");
            let required = std::collections::BTreeSet::from_iter(
                ["session_id", "expected_session_version"]
                    .into_iter()
                    .chain(extra),
            );
            let actual = std::collections::BTreeSet::from_iter(
                args["required"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            );
            assert_eq!(actual, required, "{op}");
            let fields = std::collections::BTreeSet::from_iter(
                args["properties"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str),
            );
            assert_eq!(
                fields,
                required
                    .into_iter()
                    .chain(optional)
                    .collect::<std::collections::BTreeSet<_>>(),
                "{op}"
            );
            assert_eq!(
                args["properties"]["expected_session_version"]["pattern"],
                "^[1-9][0-9]*$"
            );
            if op == "work.rework" {
                assert!(args["properties"].get("reason").is_none());
                assert_eq!(args["properties"]["note"]["maxLength"], 4096);
            }
            if matches!(op, "work.complete" | "delivery.finalize") {
                assert_eq!(
                    args["properties"]["requested_policy"]["type"],
                    json!(["string", "null"])
                );
            }
        }
        // The findings of a decision publish the closed shape of a finding in three operations.
        assert!(
            schema_bytes < 12288,
            "Operation discovery must remain bounded: {schema_bytes}"
        );
    }

    #[test]
    fn review_evidence_guidance_survives_neutral_delivery_field_publication() {
        let command = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap()
            .input_schema;
        let flat = &command["properties"]["args"]["properties"];
        let guidance = flat["evidence_id"]["description"]
            .as_str()
            .expect("flat evidence_id must retain review/completion guidance");
        for operation in [
            "review.open",
            "delivery.submit_and_request_review",
            "work.complete",
            "delivery.finalize",
        ] {
            assert!(guidance.contains(operation), "{operation}: {guidance}");
            let args = command["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|branch| branch["if"]["properties"]["op"]["const"] == operation)
                .map(|branch| &branch["then"]["properties"]["args"])
                .unwrap_or_else(|| panic!("missing operation-specific schema: {operation}"));
            assert_eq!(args["additionalProperties"], false, "{operation}");
            assert!(
                args["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("evidence_id")),
                "{operation}"
            );
            let evidence = &args["properties"]["evidence_id"];
            assert_eq!(evidence["type"], "string", "{operation}");
            assert_eq!(evidence["minLength"], 1, "{operation}");
            assert_eq!(evidence["maxLength"], 128, "{operation}");
            assert!(args["properties"].get("event_id").is_none(), "{operation}");
        }
        assert_ne!(flat["evidence_id"], flat["event_id"]);
    }

    #[tokio::test]
    async fn review_state_preconditions_are_bounded_transport_errors_not_invalid_input() {
        for (error, status, code) in [
            (
                PgError::ReworkRequiresReturnedReview,
                StatusCode::CONFLICT,
                "ReworkRequiresReturnedReview",
            ),
            (
                PgError::ReviewUnavailable,
                StatusCode::NOT_FOUND,
                "ReviewUnavailable",
            ),
        ] {
            let response = error_response(error);
            assert_eq!(response.status(), status);
            let bytes = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"], code);
            assert!(body.get("invalid_field").is_none());
            for field in ["condition", "basis", "next_action", "recheck"] {
                assert!(!body["action_guidance"][field].is_null());
            }
            assert_eq!(
                body["action_guidance"]["basis"]["inspect"],
                "review.inspect"
            );
            assert!(bytes.len() < 700);
            let mcp = CallToolResult::structured_error(body.clone());
            assert_eq!(mcp.is_error, Some(true));
            assert_eq!(mcp.structured_content, Some(body));
        }
    }

    #[test]
    fn infrastructure_failure_retains_unknown_outcome_recovery_without_private_details() {
        let (status, body) = public_error(PgError::SchemaIncompatible(
            "private-source-and-credential-sentinel".into(),
        ));
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, unavailable_value());
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("inspect a command")
        );
        assert!(
            !body
                .to_string()
                .contains("private-source-and-credential-sentinel")
        );
    }

    #[test]
    fn input_guidance_names_the_rejected_field_and_next_action() {
        for (error, message, action) in [
            (
                PgError::missing_execution_prepare_session_binding(),
                "args.session_id and args.expected_session_version are required for execution.prepare",
                "that session_id and its current session_version",
            ),
            (
                PgError::missing_session_start_conversation_id(),
                "args.conversation_id is required for session.start",
                "identifier that is at most 128 UTF-8 bytes and contains no control characters",
            ),
            (
                PgError::work_next_selectors(),
                "work.next does not accept workstream_id, work_id, or session_id",
                "without those selectors",
            ),
            (
                PgError::missing_review_session_version(),
                "args.expected_session_version is required",
                "current session_version",
            ),
        ] {
            let (status, body) = public_error(error);
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["code"], "InvalidInput");
            assert_eq!(body["message"], message);
            assert!(body["next_step"].as_str().unwrap().contains(action));
            assert_eq!(body.as_object().unwrap().len(), 3);
            assert!(body.to_string().len() < 400);
        }
    }

    #[test]
    fn discovery_types_and_requires_session_start_conversation_id() {
        let command = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let request_id = &command.input_schema["properties"]["request_id"];
        assert_eq!(request_id["maxLength"], 128);
        assert_eq!(request_id["x-identity-utf8"]["maxUtf8Bytes"], 128);
        assert_eq!(request_id["x-identity-utf8"]["rejectControls"], true);
        let request_description = request_id["description"].as_str().unwrap();
        assert!(request_description.contains("maxLength counts characters"));
        assert!(request_description.contains("128 UTF-8 bytes"));
        assert!(request_description.contains("rejects control characters"));
        let args = &command.input_schema["properties"]["args"];
        let conversation_id = &args["properties"]["conversation_id"];
        assert_eq!(conversation_id["type"], "string");
        assert_eq!(conversation_id["minLength"], 1);
        assert_eq!(conversation_id["maxLength"], 128);
        let description = conversation_id["description"].as_str().unwrap();
        assert!(description.contains("Required for session.start"));
        assert!(description.contains("Stable host conversation, thread, or session identifier"));
        assert!(description.contains("maxLength counts characters"));
        assert!(description.contains("128 UTF-8 bytes"));
        assert!(description.contains("rejects control characters"));
        assert_eq!(conversation_id["x-identity-utf8"]["maxUtf8Bytes"], 128);
        assert_eq!(conversation_id["x-identity-utf8"]["rejectControls"], true);
        assert!(args.get("required").is_none());

        let condition = command.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|condition| condition["if"]["properties"]["op"]["const"] == "session.start")
            .unwrap();
        assert_eq!(condition["if"]["required"], json!(["op"]));
        assert_eq!(
            condition["then"]["properties"]["args"]["required"],
            json!(["conversation_id"])
        );
        let start_args = &condition["then"]["properties"]["args"];
        assert_eq!(start_args["additionalProperties"], false);
        assert_eq!(
            start_args["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["client_info", "conversation_id"])
        );
    }

    #[test]
    fn discovery_types_and_requires_exact_execution_prepare_args() {
        let command = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let args = &command.input_schema["properties"]["args"];
        let session_id = &args["properties"]["session_id"];
        assert_eq!(session_id["type"], json!(["string", "null"]));
        assert_eq!(session_id["minLength"], 1);
        assert_eq!(session_id["maxLength"], 128);
        let description = session_id["description"].as_str().unwrap();
        assert!(description.contains("except session.start"));
        assert!(description.contains("work.next resume or session.inspect"));
        assert!(
            args["properties"]["execution_id"]["description"]
                .as_str()
                .unwrap()
                .contains("except execution.prepare")
        );

        let condition = command.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|condition| condition["if"]["properties"]["op"]["const"] == "execution.prepare")
            .unwrap();
        assert_eq!(condition["if"]["required"], json!(["op"]));
        let prepare = &condition["then"]["properties"]["args"];
        let required = json!([
            "session_id",
            "expected_session_version",
            "claim_id",
            "expected_fence",
            "expected_lease_version",
            "expected_work_version",
            "input_digest",
            "declared_scope"
        ]);
        assert_eq!(prepare["required"], required);
        assert_eq!(prepare["additionalProperties"], false);
        let fields = prepare["properties"].as_object().unwrap();
        assert_eq!(fields.len(), required.as_array().unwrap().len());
        for field in required.as_array().unwrap() {
            assert!(fields.contains_key(field.as_str().unwrap()));
        }
        assert_eq!(fields["claim_id"]["type"], "string");
        assert_eq!(fields["claim_id"]["minLength"], 1);
        assert_eq!(fields["input_digest"]["type"], "string");
        assert_eq!(fields["input_digest"]["pattern"], "^[0-9a-f]{64}$");
        assert_eq!(fields["expected_work_version"]["pattern"], "^[1-9][0-9]*$");
        assert_eq!(fields["declared_scope"]["type"], "array");
        assert_eq!(fields["declared_scope"]["maxItems"], 128);
        assert_eq!(fields["declared_scope"]["uniqueItems"], true);
        assert_eq!(fields["declared_scope"]["items"]["type"], "string");
        assert_eq!(fields["declared_scope"]["items"]["minLength"], 1);
        assert_eq!(fields["declared_scope"]["items"]["maxLength"], 4096);
    }

    #[test]
    fn discovery_publishes_execution_start_without_narrowing_shared_args() {
        let command = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let flat = command.input_schema["properties"]["args"]["properties"]
            .as_object()
            .unwrap();
        assert!(
            command.input_schema["properties"]["args"]
                .get("required")
                .is_none()
        );
        assert_eq!(
            flat["execution_mode"]["enum"],
            json!(["caller_managed", "reference_write_v1"])
        );
        assert_eq!(
            flat["expected_input_digest"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(flat["expected_input_digest"]["pattern"], "^[0-9a-f]{64}$");
        assert_eq!(
            flat["expected_work_version"]["pattern"],
            "^(0|[1-9][0-9]*)$"
        );
        assert_eq!(flat["input_digest"]["type"], json!(["string", "null"]));
        assert_eq!(flat["execution_id"]["type"], json!(["string", "null"]));

        let start = command.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|branch| branch["if"]["properties"]["op"]["const"] == "execution.start")
            .unwrap();
        let args = &start["then"]["properties"]["args"];
        assert_eq!(args["additionalProperties"], false);
        assert_eq!(
            args["required"],
            json!([
                "session_id",
                "expected_session_version",
                "execution_id",
                "expected_execution_version",
                "expected_work_version",
                "claim_id",
                "expected_fence",
                "expected_lease_version",
                "execution_mode"
            ])
        );
        assert!(
            !args["required"]
                .as_array()
                .unwrap()
                .contains(&json!("expected_input_digest"))
        );
        assert_eq!(args["properties"]["execution_id"]["type"], "string");
        assert_eq!(args["properties"]["execution_id"]["minLength"], 1);
        assert_eq!(args["properties"]["execution_id"]["maxLength"], 128);
        assert_eq!(
            args["properties"]["expected_work_version"]["pattern"],
            "^[1-9][0-9]*$"
        );
        assert_eq!(
            args["properties"]["expected_input_digest"]["type"],
            json!(["string", "null"])
        );
        let conditional = &args["allOf"][0];
        assert_eq!(
            conditional["if"]["properties"]["execution_mode"]["const"],
            "reference_write_v1"
        );
        assert_eq!(
            conditional["then"]["required"],
            json!(["expected_input_digest"])
        );
        assert_eq!(
            conditional["then"]["properties"]["expected_input_digest"]["type"],
            "string"
        );
        let prepare = command.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|branch| branch["if"]["properties"]["op"]["const"] == "execution.prepare")
            .unwrap();
        assert_eq!(
            prepare["then"]["properties"]["args"]["required"]
                .as_array()
                .unwrap()
                .len(),
            8
        );
        assert!(
            prepare["then"]["properties"]["args"]["properties"]
                .get("execution_mode")
                .is_none()
        );
    }

    #[test]
    fn flat_args_discover_checkpoint_and_prepare_fields_without_narrowing_shared_types() {
        let command = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let args = &command.input_schema["properties"]["args"];
        assert!(args.get("required").is_none());
        let flat = args["properties"].as_object().unwrap();
        let branch = |op: &str| {
            command.input_schema["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|branch| branch["if"]["properties"]["op"]["const"] == op)
                .unwrap_or_else(|| panic!("missing operation branch: {op}"))
                .clone()
        };
        let checkpoint = branch("session.checkpoint");
        let prepare = branch("execution.prepare");
        let checkpoint_args = &checkpoint["then"]["properties"]["args"];
        let prepare_args = &prepare["then"]["properties"]["args"];
        assert_eq!(checkpoint_args["additionalProperties"], false);
        assert_eq!(prepare_args["additionalProperties"], false);
        assert_eq!(
            checkpoint_args["required"],
            json!([
                "session_id",
                "expected_session_version",
                "context_hash",
                "next_action",
                "open_loops"
            ])
        );
        assert_eq!(
            prepare_args["required"],
            json!([
                "session_id",
                "expected_session_version",
                "claim_id",
                "expected_fence",
                "expected_lease_version",
                "expected_work_version",
                "input_digest",
                "declared_scope"
            ])
        );
        for name in checkpoint_args["required"]
            .as_array()
            .unwrap()
            .iter()
            .chain(prepare_args["required"].as_array().unwrap())
        {
            assert!(
                flat.contains_key(name.as_str().unwrap()),
                "operation-required {name} must be discoverable from args.properties"
            );
        }
        let without_description = |value: &Value| {
            let mut copy = value.clone();
            copy.as_object_mut().unwrap().remove("description");
            copy
        };
        for name in ["context_hash", "next_action", "open_loops"] {
            assert_eq!(
                without_description(&flat[name]),
                without_description(&checkpoint_args["properties"][name]),
                "{name}"
            );
        }
        let context_hash = flat["context_hash"]["description"].as_str().unwrap();
        assert!(context_hash.contains("consumed work.prepare"));
        assert!(context_hash.contains("session.inspect"));
        assert!(context_hash.contains("64 lowercase hexadecimal"));
        assert!(
            flat["next_action"]["description"]
                .as_str()
                .unwrap()
                .contains("8192 UTF-8 bytes")
        );
        assert!(
            flat["open_loops"]["description"]
                .as_str()
                .unwrap()
                .contains("4096 UTF-8 bytes")
        );
        assert_eq!(flat["claim_id"], prepare_args["properties"]["claim_id"]);
        assert_eq!(
            flat["declared_scope"],
            prepare_args["properties"]["declared_scope"]
        );
        assert_eq!(
            flat["expected_work_version"]["pattern"],
            "^(0|[1-9][0-9]*)$"
        );
        assert_eq!(flat["input_digest"]["type"], json!(["string", "null"]));
        assert_eq!(flat["input_digest"]["pattern"], "^[0-9a-f]{64}$");
        assert_eq!(
            prepare_args["properties"]["expected_work_version"]["pattern"],
            "^[1-9][0-9]*$"
        );
        assert_eq!(prepare_args["properties"]["input_digest"]["type"], "string");
        assert!(
            flat["workspace_settlement"]["properties"]["claim_id"]
                .get("minLength")
                .is_none()
        );

        let envelope = |op: &str, arguments: Value| {
            json!({"protocol_version":1,"request_id":"request","op":op,
                "workstream_id":"00000000000000000000000001","work_id":"work",
                "coordinator_epoch":"epoch","expected_project_revision":"1",
                "expected_authority_version":"1","expected_ownership_version":"1",
                "expected_contract_hash":"a".repeat(64),"args":arguments})
        };
        let diagnosed = |input: Value| {
            let mut error = json!({"code":"InvalidInput","message":"invalid input"});
            add_input_diagnostic(&mut error, &input, true);
            error
        };
        let mut prepare_input = json!({
            "session_id":"session","expected_session_version":"1","claim_id":"claim",
            "expected_fence":"1","expected_lease_version":"1","expected_work_version":"1",
            "input_digest":"a".repeat(64),"declared_scope":["src/lib.rs"]
        });
        prepare_input["expected_work_version"] = json!("0");
        assert_eq!(
            diagnosed(envelope("execution.prepare", prepare_input.clone()))["constraint"],
            "pattern"
        );
        prepare_input["expected_work_version"] = json!("1");
        prepare_input["input_digest"] = Value::Null;
        assert_eq!(
            diagnosed(envelope("execution.prepare", prepare_input.clone()))["constraint"],
            "type"
        );
        prepare_input["input_digest"] = json!("a".repeat(64));
        prepare_input["unlisted_field"] = json!("hidden");
        let rejected = diagnosed(envelope("execution.prepare", prepare_input));
        assert_eq!(rejected["constraint"], "unexpected_field");
        assert!(!rejected.to_string().contains("unlisted_field"));
        let mut checkpoint_input = json!({
            "session_id":"session","expected_session_version":"1",
            "context_hash":"a".repeat(64),"next_action":"Review the schema change",
            "open_loops":["Native acceptance remains open"]
        });
        checkpoint_input
            .as_object_mut()
            .unwrap()
            .remove("open_loops");
        assert_eq!(
            diagnosed(envelope("session.checkpoint", checkpoint_input.clone()))["invalid_field"],
            "/args/open_loops"
        );
        checkpoint_input["open_loops"] = json!(["Native acceptance remains open"]);
        checkpoint_input["unlisted_field"] = json!("hidden");
        assert_eq!(
            diagnosed(envelope("session.checkpoint", checkpoint_input))["constraint"],
            "unexpected_field"
        );
        assert!(
            diagnosed(envelope(
                "claim.acquire",
                json!({
                    "session_id":"session","expected_session_version":"1",
                    "expected_work_version":"0","ttl_seconds":60
                })
            ))
            .get("invalid_field")
            .is_none()
        );
        assert!(
            diagnosed(envelope(
                "evidence.submit",
                json!({
                    "session_id":"session","expected_session_version":"1",
                    "payload":{"note":"measured"},"dirty_tree":true,"input_digest":null
                })
            ))
            .get("invalid_field")
            .is_none()
        );
    }

    #[test]
    fn discovery_explains_work_next_selectors_and_pr_session_versions() {
        let tools = catalog();
        let query = tools.iter().find(|t| t.name == "awr_team_query").unwrap();
        for field in ["workstream_id", "work_id", "session_id"] {
            assert!(
                query.input_schema["properties"][field]["description"]
                    .as_str()
                    .unwrap()
                    .contains("Omit for work.next")
            );
        }
        let command = tools.iter().find(|t| t.name == "awr_team_command").unwrap();
        let args = &command.input_schema["properties"]["args"];
        let version = &args["properties"]["expected_session_version"];
        let description = version["description"].as_str().unwrap();
        assert!(description.contains("work.next resume or session.inspect items"));
        assert!(description.contains("except session.start"));
        for op in ["delivery.register_pr", "delivery.observe_pr"] {
            assert!(
                args["description"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("{op}: session_id, expected_session_version,"))
            );
        }
        assert!(args.get("required").is_none());
    }

    #[test]
    fn checkpoint_precondition_guidance_is_bounded_and_uses_fresh_context() {
        let (status, conflict) = public_error(PgError::PreconditionsChanged);
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(conflict["code"], "PreconditionsChanged");
        assert_eq!(
            conflict["message"],
            "command preconditions changed; refresh the selected work or session"
        );
        let next = conflict["next_step"].as_str().unwrap();
        for expected in [
            "if this was session.checkpoint",
            "consume a fresh work.prepare",
            "current session_version",
            "fresh context_hash",
            "session.inspect's context_hash is historical",
        ] {
            assert!(next.contains(expected), "missing guidance: {expected}");
        }
        assert_eq!(conflict.as_object().unwrap().len(), 3);
        assert!(conflict.to_string().len() < 500);
    }

    #[test]
    fn source_storage_diagnostics_preserve_recovery_guidance_without_private_details() {
        for (kind, reason) in [
            (std::io::ErrorKind::PermissionDenied, "permission_denied"),
            (std::io::ErrorKind::NotFound, "io_error"),
            (std::io::ErrorKind::Other, "io_error"),
        ] {
            let (status, body) = public_error(PgError::source_storage_unavailable(
                std::io::Error::new(kind, "/private/source/ledger.yaml: secret-sentinel"),
            ));
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["code"], "SourceStorageUnavailable");
            assert_eq!(body["reason"], reason);
            let next = body["next_step"].as_str().unwrap();
            assert!(next.contains("parent-directory"));
            assert!(next.contains("planning.outcome"));
            assert!(next.contains("original request_id"));
            assert!(next.contains("do not create a new request"));
            assert!(next.contains("assume no changes occurred"));
            let rendered = body.to_string();
            assert!(!rendered.contains("/private/source"));
            assert!(!rendered.contains("secret-sentinel"));
            assert!(rendered.len() < 650);
        }
    }

    #[test]
    fn discovery_covers_every_query_contract_field() {
        // Populate every field, including optional fields omitted by serde, so
        // additions to the backend contract also require discovery coverage.
        let query = WorkstreamQuery {
            protocol_version: 1,
            op: "audit.history".into(),
            workstream_id: None,
            work_id: None,
            session_id: None,
            search: None,
            cursor: None,
            limit: None,
            max_context_bytes: None,
            request_id: None,
            claim_id: None,
            execution_id: None,
            handoff_id: None,
            evidence_id: None,
            review_round_id: None,
            source_path: Some("spec.md".into()),
            artifact_id: Some("artifact".into()),
            export_id: Some("export".into()),
            expected_sha256: Some("a".repeat(64)),
            change_id: Some("change".into()),
            member_actor_id: Some("member".into()),
            category: Some("access".into()),
            include_denies: Some(false),
        };
        let fields = serde_json::to_value(query).unwrap();
        let tool = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_query")
            .unwrap();
        assert_eq!(
            fields.as_object().unwrap().keys().collect::<BTreeSet<_>>(),
            tool.input_schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<BTreeSet<_>>(),
            "MCP discovery and the backend query contract must expose the same fields"
        );
    }
}
