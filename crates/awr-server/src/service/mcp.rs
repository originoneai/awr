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
            // Admission failures need a fresh login even when a native client
            // retained a static credential. Tool permissions are checked below.
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
            "description":"The actual handoff.id, not an event, checkpoint, session or artifact ID. Query handoff.inspect with this ID before receiving."},
        "kind":{"enum":["execution","responsibility"],"description":"handoff.propose: execution changes the executor; responsibility changes ownership separately."},
        "to_person_id":identity,
        "package":{"type":"object","additionalProperties":false,
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
        "now_ms":{"type":"integer","description":"Required for every handoff command. Current Unix time in milliseconds, distinct from a version or expiry."},
        "inspector_person_id":identity,"acceptor_person_id":identity,"by_person_id":identity,
        "successor_execution":execution,
        "prior_execution_stopped":{"type":"boolean","description":"Report verified stopped state; expiry or disconnect alone is not proof."},
        "prior_reconciled":{"type":"boolean","description":"Report actual reconciliation of original effects; do not infer it from a success message."},
        "context_reprepared":{"type":"boolean","description":"Receiver must consume current work.prepare context before acceptance."},
        "reason":{"type":"string","description":"Use the operation-specific reason bound; handoff.reject/cancel accept at most 2048 bytes."}
    });
    fields.extend(handoff.as_object().unwrap().clone());
    let conditions = command["allOf"].as_array_mut().unwrap();
    for (op, extra) in [
        ("handoff.propose", vec!["kind", "to_person_id", "package"]),
        (
            "handoff.inspect",
            vec!["expected_handoff_version", "inspector_person_id"],
        ),
        (
            "handoff.accept",
            vec![
                "expected_handoff_version",
                "acceptor_person_id",
                "successor_execution",
                "prior_execution_stopped",
                "prior_reconciled",
                "context_reprepared",
            ],
        ),
        (
            "handoff.reject",
            vec!["expected_handoff_version", "by_person_id", "reason"],
        ),
        (
            "handoff.cancel",
            vec!["expected_handoff_version", "by_person_id", "reason"],
        ),
        ("handoff.timeout", vec!["expected_handoff_version"]),
    ] {
        let mut required = vec![
            "session_id",
            "expected_session_version",
            "handoff_id",
            "now_ms",
        ];
        required.extend(extra);
        let mut args = json!({"required":required});
        if matches!(op, "handoff.reject" | "handoff.cancel") {
            args["properties"] = json!({"reason":{"type":"string","minLength":1,"maxLength":2048}});
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
                "enabled":{"type":"boolean"}}},
        "expected_selected_digest":{"anyOf":[digest,{"type":"null"}]},
        "candidate":{"type":"object","additionalProperties":false,"required":["binding","manifest"],
            "properties":{"binding":{"type":"object"},"manifest":{"type":"object"}},
            "description":"Strict DeliveryCandidate: binding and manifest. Bind actual tenant/project/main/workstream/work, contract, artifact manifest digest, required checks and target; these fields cannot grant authority."},
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
    for (op, fields) in [
        (
            "delivery.connector.configure",
            vec!["expected_connector_version", "mapping"],
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
        ),
        (
            "delivery.inspection.reserve",
            vec![
                "connector_id",
                "connector_version",
                "candidate_digest",
                "lease_seconds",
            ],
        ),
        (
            "delivery.facts.ingest",
            vec!["connector_id", "inspection_id", "event_id", "records"],
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
        ),
        (
            "delivery.source.renew",
            vec!["publication_id", "fence", "lease_seconds"],
        ),
        ("delivery.source.write", vec!["publication_id", "fence"]),
        ("delivery.source.confirm", vec!["publication_id", "fence"]),
        ("delivery.source.abandon", vec!["publication_id", "fence"]),
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
        ),
        (
            "delivery.integration.reject_prepared",
            vec!["integration_id", "reason"],
        ),
    ] {
        let mut required = vec!["source_snapshot_id"];
        required.extend(fields);
        let mut args = json!({"required":required});
        if op == "delivery.integration.reject_prepared" {
            args["properties"] = json!({"reason":{"type":"string","minLength":1,"maxLength":1024,
                "description":"A nonblank reason of at most 1,024 UTF-8 bytes."}});
        }
        command["allOf"].as_array_mut().unwrap().push(json!({
            "if":{"properties":{"op":{"const":op}},"required":["op"]},
            "then":{"properties":{"args":args}}
        }));
    }
}

fn catalog() -> Vec<Tool> {
    let mut query = json!({"type":"object","additionalProperties":false,
    "required":["protocol_version","op"],"properties":{
        "protocol_version":{"type":"integer","const":1},
        "op":{"type":"string","enum":WorkstreamQuery::OPERATIONS},
        "workstream_id":{"type":"string","description":"Omit for work.next; follow its returned next_query to select work."},
        "work_id":{"type":"string","description":"Omit for work.next."},
        "session_id":{"type":"string","description":"Omit for work.next; session.inspect requires this selector."},
        "request_id":{"type":"string","description":"For delivery.integration.inspect, use IntegrationRequest.request_id (receipt data.integration_id). For delivery.neutral.outcome, use the original client command retry ID."},"claim_id":{"type":"string"},
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
        "search":{"type":"string","maxLength":512},"cursor":{"type":"string","maxLength":4096},
        "limit":{"type":"integer","minimum":1,"maximum":100},
        "max_context_bytes":{"type":"integer","minimum":1,"maximum":262144},
        "source_path":{"type":"string","maxLength":512},
        "artifact_id":{"type":"string","maxLength":128},
        "export_id":{"type":"string","maxLength":128},
        "expected_sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"}
    }});
    query["allOf"] = json!([
        {"if":{"properties":{"op":{"enum":["delivery.neutral.inspect","delivery.neutral.outcome","delivery.source.status","delivery.integration.inspect"]}},"required":["op"]},
            "then":{"required":["work_id"],"not":{"required":["session_id"]}}},
        {"if":{"properties":{"op":{"enum":["delivery.neutral.outcome","delivery.integration.inspect"]}},"required":["op"]},
            "then":{"required":["request_id"]}}
    ]);
    let mut command = json!({"type":"object","additionalProperties":false,
    "required":["protocol_version","request_id","op","workstream_id","work_id","coordinator_epoch","expected_project_revision","expected_authority_version","expected_ownership_version","expected_contract_hash","args"],
    "properties":{
        "protocol_version":{"type":"integer","const":1},
        "op":{"type":"string","enum":WorkstreamCommand::OPERATIONS},
        "request_id":{"type":"string","maxLength":128},
        "workstream_id":{"type":"string"},"work_id":{"type":"string"},
        "coordinator_epoch":{"type":"string"},
        "expected_project_revision":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},
        "expected_authority_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_ownership_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_contract_hash":{"type":"string"},
        "args":{"type":"object","description":"session.start: conversation_id, optional client_info. session.checkpoint: session_id, expected_session_version, context_hash, next_action, open_loops; optional client_info, progress, usage (schemas below). Batch feedback at meaningful boundaries; execution.report is terminal-only. session.end: session_id, expected_session_version. All claim/execution actions: session_id, expected_session_version. claim.acquire adds expected_work_version (0 when runtime absent), ttl_seconds (1..3600). claim.renew/release add claim_id, expected_fence, expected_lease_version; renew also ttl_seconds. execution.prepare adds claim_id, expected_fence, expected_lease_version, expected_work_version, input_digest (64 lowercase hex), declared_scope (canonical relative paths). execution.cancel adds execution_id, expected_execution_version. execution.start adds execution_id, expected_execution_version, claim_id, expected_fence, expected_lease_version, expected_work_version, execution_mode (caller_managed or reference_write_v1), optional expected_input_digest. reference_write_v1 requires the prepared input digest and system attestation authority; the service does not dispatch the local runner. execution.report adds execution_id, expected_execution_version, outcome (succeeded/failed/cancelled/unknown), optional output_digest (required for success), observed_paths, note. execution.attest adds execution_id, expected_execution_version, facts; optional reviewed_receipt_id (latest inspected caller receipt) and facts.executor_stopped=true may clear only its attributed reference_write_v1 report barrier with unchanged admission authority and exact resources. Missing confirmation fields preserve legacy settlement without automatic recovery clearing. execution.reconcile also adds expected_work_version, reviewed_receipt_id (latest inspected ID or null), clear_recovery_block, optional previous_epoch_recovery. Old-epoch recovery requires {execution_epoch (exact inspected epoch), executor_stopped (true to settle), review_reference (nonempty, <=2048 bytes, no controls)}; this is an authorized operator assertion, not independently verified fencing. facts: outcome, input_digest, optional output_digest (required for success), environment_digest, observed_paths, note. Digests are 64 lowercase hex. Versions are decimal strings; unknown fields fail. handoff.propose: session_id, expected_session_version, handoff_id, kind (execution|responsibility), to_person_id, package (task_id, contract_version, contract_hash, current_person_id, current_execution, consumed_context_digest, checkpoint_ids, artifact_versions, branch_id, working_directory, dependency_ids, todos, awaiting_replies, unknown_side_effects), optional proposed_successor/proposer_execution_id/proposer_fence/expires_at_ms, now_ms. handoff.inspect/accept/reject/cancel/timeout: session_id, expected_session_version, handoff_id, expected_handoff_version, now_ms; accept adds acceptor_person_id, successor_execution, prior_execution_stopped, prior_reconciled, context_reprepared, optional expected_current_fence; reject/cancel add by_person_id+reason; inspect adds inspector_person_id.  Timeout closes the proposal only and does not stop execution. evidence.submit: session_id, expected_session_version, payload, dirty_tree (required boolean); optional claimed_trust/input_digest/execution_id and one of artifact_text or legacy artifact_hex (schemas below). Agent completion requires payload.passed=true, payload.output_digest matching execution.report, input_digest matching execution.prepare, execution_id and artifact bytes. The server hashes artifact bytes separately; never substitute that hash for the execution output digest. Inspect evidence and artifact.content before review. review.open / delivery.submit_and_request_review: session_id, expected_session_version, evidence_id. review.accept/return: session_id, expected_session_version, round_id, reason. review.decide: session_id, expected_session_version, round_id, decision (approve|reject), reason — requires the matching human or Agent review grant and policy. work.rework: session_id, expected_session_version, round_id, note. work.complete / delivery.finalize: session_id, expected_session_version, evidence_id, context_complete, optional requested_policy. Finalization needs maintainer/project_admin permission and, for an Agent, an explicit finalize_delivery delegation; independent review and current evidence remain required. delivery.register_pr: session_id, expected_session_version, repository, pr_number, pr_url, head_sha, fact_source (authorized_human_github_verification|operator_recorded_observation), observed_at (RFC3339), optional merge_sha/test_evidence_id/gh_* flags — v1 manual GitHub verification, not webhook sync. delivery.observe_pr: session_id, expected_session_version, delivery_id, expected_head_sha, fact_source, observed_at, optional gh_approved/gh_merged/merge_sha. Query delivery.inspect separates GitHub submitted/approved/merged from AWR acceptance complete."}
    }});
    command["properties"]["args"]["properties"] = json!({
        "conversation_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"Required for session.start. Stable host conversation, thread, or session identifier used to bind the durable AWR session."},
        "session_id":{"type":"string","minLength":1,"maxLength":128,
            "description":"Required for session lifecycle, claim, execution, handoff, evidence, review, rework and delivery actions except session.start. Use the owned session_id from work.next resume or session.inspect."},
        "dirty_tree":{"type":"boolean","description":"Required for evidence.submit. Report the actual workspace state; not a completion or trust assertion."},
        "payload":{"description":"Required for evidence.submit; generic evidence accepts any JSON value. Agent completion requires an object with passed=true and output_digest (64 lowercase hex) matching the successful execution report."},
        "artifact_text":{"type":["string","null"],"maxLength":1048576,
            "description":"evidence.submit: exact UTF-8 text, preserving whitespace and line endings. At most 1 MiB in bytes; the whole MCP request still must fit 64 KiB. Keep reports compact. Omit artifact_hex when using this field."},
        "artifact_hex":{"type":["string","null"],"maxLength":2097152,"pattern":"^([0-9a-fA-F]{2})*$",
            "description":"evidence.submit: legacy exact bytes encoded as even-length hex, at most 1 MiB decoded; whole MCP request limit is 64 KiB. Generate mechanically for binary artifacts; prefer artifact_text for text. Omit artifact_text when using this field."},
        "input_digest":{"type":["string","null"],"pattern":"^[0-9a-f]{64}$",
            "description":"Evidence input binding; Agent completion must match execution.prepare input_digest."},
        "execution_id":{"type":["string","null"],"description":"Required for execution actions and Agent-completion evidence; use execution_id from the matching execution.prepare receipt."},
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
    command["allOf"] = json!([
        {
            "if":{
                "properties":{"op":{"const":"session.start"}},
                "required":["op"]
            },
            "then":{
                "properties":{"args":{"required":["conversation_id"]}}
            }
        },
        {
            "if":{
                "properties":{"op":{"const":"execution.prepare"}},
                "required":["op"]
            },
            "then":{
                "properties":{"args":{"required":["session_id","expected_session_version"]}}
            }
        }
    ]);
    add_handoff_input_schema(&mut command);
    add_neutral_delivery_schema(&mut command);
    command["properties"]["args"]["properties"]["assignee_person_id"] = json!({"type":"string","minLength":1,"maxLength":128,
        "description":"task.assign: active eligible project member. This is a target, never the acting identity."});
    command["properties"]["args"]["properties"]["expected_responsibility_version"] = json!({"type":"string","pattern":"^(0|[1-9][0-9]*)$",
        "description":"Use work.prepare responsibility.version; distinct from source ownership and temporary lease versions."});
    command["properties"]["args"]["properties"]["assignment_request_key"] = json!({"type":"string","minLength":1,"maxLength":128,
        "description":"task.accept_assignment: current responsibility.pending.transfer_request_key. Not a handoff ID."});
    command["properties"]["args"]["properties"]["ttl_seconds"] =
        json!({"type":"integer","minimum":1,"maximum":3600});
    let old_description = command["properties"]["args"]["description"]
        .as_str()
        .unwrap()
        .to_owned();
    command["properties"]["args"]["description"] = json!(format!(
        "task.assign: assignee_person_id, expected_responsibility_version; no execution session required. An optional session_id must include expected_session_version. task.accept_assignment / task.claim_available: session_id, expected_session_version, expected_responsibility_version, expected_work_version, ttl_seconds; acceptance also requires assignment_request_key. Assignment may reserve blocked dependencies; taking work cannot bypass them. Ownership and the coordination lease commit atomically; execution.start remains separate. {old_description}"
    ));
    for (op, required) in [
        (
            "task.assign",
            vec!["assignee_person_id", "expected_responsibility_version"],
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
        ),
    ] {
        command["allOf"].as_array_mut().unwrap().push(json!({
            "if":{"properties":{"op":{"const":op}},"required":["op"]},
            "then":{"properties":{"args":{"required":required}}}}));
    }
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
    let planning_draft = json!({
        "type":"object","additionalProperties":false,
        "required":["protocol_version","request_id","mode","changes"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "request_id":{"type":"string","maxLength":128},
            "mode":{"type":"string","enum":["create","edit"]},
            "candidate_id":{"type":["string","null"],"maxLength":128},
            "changes":{"type":"array","maxItems":256,"description":"DraftChange objects with op, before and after TaskDraft. V5 dependency_acceptance selects simulated_member_independent per required same-stream predecessor, independently of the consumer completion policy; omitted maps retain source values and replacements bind exact prior maps. V4 execution_settlement is {mode: independent_workspace_v1, workspace_id: opaque identity}. New simulated-member tasks require settlement, scope_paths and verification_requirements. Optional execution fields omitted on edits retain source values; explicit replacement must include their exact prior values in before."},
            "suggestion_ids":{"type":"array","maxItems":256,"items":{"type":"string"}},
            "allowed_spec_roots":{"type":"array","maxItems":64,"items":{"type":"string"}},
            "project_goal_keys":{"type":"array","maxItems":64,"items":{"type":"string"}},
            "self_approve_policy":{"type":["object","null"]},
            "author_person_id":{"type":["string","null"],"maxLength":128}
        }
    });
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
        Tool::new("awr_team_query",
            "Scoped Team reads. Begin with capabilities (current identity/permissions), then work.next (own sessions and scoped task navigation). Consume work.prepare before any claim or execution. Use work.snapshot for context and observation in one read transaction, within max_context_bytes; its context_hash remains compatible with work.prepare. work.observe reads current scoped session/checkpoint, lease, execution and registered PR facts without changing context or authorizing execution; receipt payloads retain their original access gate, and missing model/usage remains explicit. audit.requests and audit.development provide paged metadata/history under the same personal/project permission boundary. Ops audit: audit.history / audit.export / audit.count (TMCP-040) — members see own allowed records; project-wide requires audit.read_project. Counts/exports use the same scope. Not full chat/tool-IO/token billing; PG audit does not claim DB-owner non-repudiation. Tool discovery is navigation-only; each query rechecks authority. Re-prepare after relevant changes. No execution admission. Approved cross-stream content: delivery.exports with consumer work_id, then artifact.content with export_id and expected_sha256. Disclosure never satisfies adoption. Neutral facts: delivery.neutral.inspect. Current source synchronization: delivery.source.status. Missing neutral command response: delivery.neutral.outcome with actual work_id and original client retry ID. delivery.integration.inspect uses the generated IntegrationRequest.request_id (data.integration_id), preserving the original candidate. Neutral inspection includes scoped connector versions; review.inspect includes decision IDs. Descriptions grant no authority.",
            query.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_command",
            "Durable sessions, claims, confirmed handoffs and caller-managed execution under the shared TMCP action gate. Use work.prepare preconditions and a stable request_id. Readers cannot claim or write. Developers may maintain own session/execution on authorized work but cannot edit/publish plans or grant permissions. Only a fresh execution.start response with execution_authorized=true permits one run under the live lease. Exact replay reuses the original receipt; changed intent or expired/revoked authority is refused. Body fields cannot forge identity. Preparation, inspection and replay grant no execution rights. On unknown outcome use delivery.neutral.outcome for neutral delivery commands and command.inspect for legacy commands before an exact retry; never repeat effects from a receipt. Refresh after conflicts or lease/contract changes. Cancellation is a request after start. Reports remain caller_asserted. Attestation requires operator-issued system authority at admission and now. For unknown effects, execution.inspect then operator execution.reconcile; confirm current versions and latest receipt. Recheck on permission, receipt or work changes. Settlement is not work completion. Evidence/review/rework/complete reuse WS-018 under explicit review.decide and delivery.finalize permissions on the same MCP plane: evidence.submit, review.open, delivery.submit_and_request_review, review.accept, review.return, review.decide, work.rework, work.complete, delivery.finalize, plus delivery.register_pr / delivery.observe_pr for versioned PR bindings. Neutral candidate/inspection/facts/source operations use args.source_snapshot_id and the common header; eligible human/system management is required for connector and source publication. They store observations or source metadata. delivery.integration.prepare requires current delivery.finalize permission plus exact connector, selection, evidence and review IDs; it reserves an intent. delivery.integration.reject_prepared only rejects before dispatch. Neither exposes a worker permit or executes Git; inspect the generated integration_id through delivery.integration.inspect. No implicit approval or finalization. Author, owner, executor, reviewer and final-submitter are attributed separately on completion. GitHub submitted/approved/merged are distinct from AWR acceptance; URL, green CI, admin role or already-merged cannot skip acceptance. Head/contract/artifact mismatch invalidates approvals. Independence is by person; a second agent of the same person is not team-independent. The explicit caller_managed_execution_and_simulated_member_review policy supports review.decide only when capabilities.simulated_member_review is present: unique active member bindings, explicit Agent review grants and live Review delegation, with member/actor/client distinct from the immutable executor, submitter and round opener. One controller may operate distinct simulated members. Inspect full origins through review.inspect; default receipts carry short summaries. Simulated finalization requires current delivery.finalize authority, actual artifact bytes, settled successful execution and the exact immutable review basis. Agents need an explicit live finalize_delivery delegation; Review or development grants are insufficient. Neutral fast-forward integration eligibility reuses that basis; preparation grants no repository rights or effect. Default completion receipts add only a basis summary; human/team-human approval flags stay false. Recheck after authority, source, evidence or review changes. AWR acceptance, repository integration and confirmed source publication remain distinct facts. Explicit caller_managed_execution_and_agent_review contracts allow review.decide with both agent_review membership and live Review delegation, a distinct author actor/client, and false human/team-acceptance flags; completion additionally requires artifact bytes, matching input/output bindings and a successful caller report reconciled by an authorized operator. Its evidence stays caller_asserted. Same-stream inputs require explicit per-edge dependency_acceptance: V2 agent_reviewed_caller_asserted_reconciled or V5 simulated_member_independent. Omitted Agent or simulated inputs remain blocked; original selected receipt, current contract and immutable review must match. Approved disclosure uses delivery.export.publish/revoke under delivery.finalize; capabilities.approved_artifact_exports lists exact arguments. Cross-stream adoption is separately capability-gated. PR facts require fact_source+observed_at: Agent verification uses operator_recorded_observation; authorized_human_github_verification requires an actual human observation. No webhook auto-sync claim. Completion receipts are for WS-030 adoption and omit provider-private sessions.",
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
            .with_instructions("The URL binds one operator-registered project; bearer credentials are checked on every request. Begin with awr_team_query capabilities, then work.next to resume own work or discover scoped candidates without manual task selection. Project admins manage members via awr_team_access_* tools after local owner bootstrap of the first admin. Raw credentials are never accepted or returned over MCP — generate them through protected Inspector issuance or awr-server access token and register only secret_hash. Work/session selectors bind a workstream; missing permissions never mean satisfied dependencies. Consume work.prepare before checkpointing. Follow its single guidance item (when/because/action/recheck_on); it grants no authority. Report client_info from known host metadata at session start or next checkpoint. Batch progress at phase/test completion, blocker, user wait or delivery boundaries; no per-tool reporting loop. Only submit usage with known host counter scope. Routine progress uses session.checkpoint; execution.report is for terminal outcomes. Session journals and claims grant no execution rights. Claim replay is a historical receipt; use claim.inspect for current lease state. If a command outcome is unknown, inspect its original request_id before an exact retry: delivery.neutral.outcome for neutral operations, command.inspect for legacy coordination. Check delivery.neutral.inspect and delivery.source.status for current state. Planning mutations use awr_team_planning_* with stable request_id; on disconnect call awr_team_planning_outcome or planning.outcome before any new ID. Controlled source/artifact content uses source.content / artifact.content — never path/URL/history bypass. Ops history uses audit.history/export/count within authorized scope. Recheck context and permission after relevant changes. MCP connection closure never closes a durable work session.")
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
        let result = tokio::time::timeout_at(access.deadline, super::audited(&self.state, &self.project, &access.bearer, &action, work.as_deref(), async {
            match request.name.as_ref() {
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
            Ok(Ok(value)) => CallToolResult::structured(value),
            Ok(Err(error)) => CallToolResult::structured_error(public_error(error).1),
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
        assert_eq!(fields["now_ms"]["type"], "integer");
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
        for op in [
            "task.assign",
            "task.accept_assignment",
            "task.claim_available",
        ] {
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
            let required = condition["then"]["properties"]["args"]["required"]
                .as_array()
                .unwrap();
            assert!(required.contains(&json!("expected_responsibility_version")));
            if op == "task.assign" {
                assert!(required.contains(&json!("assignee_person_id")));
                assert!(!required.contains(&json!("session_id")));
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
            }
        }
    }

    #[test]
    fn handoff_requirements_are_conditional_and_include_current_time() {
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
            for key in [
                "session_id",
                "expected_session_version",
                "handoff_id",
                "now_ms",
            ] {
                assert!(required.contains(&json!(key)), "{operation} requires {key}");
            }
            if operation == "handoff.accept" {
                for key in [
                    "successor_execution",
                    "prior_execution_stopped",
                    "prior_reconciled",
                    "context_reprepared",
                ] {
                    assert!(required.contains(&json!(key)));
                }
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
    fn discovery_types_nested_command_versions_without_requiring_them_for_every_op() {
        let tool = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let args = &tool.input_schema["properties"]["args"];
        assert!(args.get("required").is_none());
        let fields = &args["properties"];
        for name in [
            "expected_session_version",
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
                "stable, nonempty host conversation, thread, or session identifier",
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
        let args = &command.input_schema["properties"]["args"];
        let conversation_id = &args["properties"]["conversation_id"];
        assert_eq!(conversation_id["type"], "string");
        assert_eq!(conversation_id["minLength"], 1);
        assert_eq!(conversation_id["maxLength"], 128);
        let description = conversation_id["description"].as_str().unwrap();
        assert!(description.contains("Required for session.start"));
        assert!(description.contains("Stable host conversation, thread, or session identifier"));
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
    }

    #[test]
    fn discovery_types_and_requires_execution_prepare_session_binding() {
        let command = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap();
        let args = &command.input_schema["properties"]["args"];
        let session_id = &args["properties"]["session_id"];
        assert_eq!(session_id["type"], "string");
        assert_eq!(session_id["minLength"], 1);
        assert_eq!(session_id["maxLength"], 128);
        let description = session_id["description"].as_str().unwrap();
        assert!(description.contains("except session.start"));
        assert!(description.contains("work.next resume or session.inspect"));

        let condition = command.input_schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|condition| condition["if"]["properties"]["op"]["const"] == "execution.prepare")
            .unwrap();
        assert_eq!(condition["if"]["required"], json!(["op"]));
        assert_eq!(
            condition["then"]["properties"]["args"]["required"],
            json!(["session_id", "expected_session_version"])
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
