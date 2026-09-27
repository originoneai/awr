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
        return denied();
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

fn catalog() -> Vec<Tool> {
    let query = json!({"type":"object","additionalProperties":false,
    "required":["protocol_version","op"],"properties":{
        "protocol_version":{"type":"integer","const":1},
        "op":{"type":"string","enum":WorkstreamQuery::OPERATIONS},
        "workstream_id":{"type":"string"},"work_id":{"type":"string"},
        "session_id":{"type":"string"},"request_id":{"type":"string"},"claim_id":{"type":"string"},
        "execution_id":{"type":"string"},"handoff_id":{"type":"string"},
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
        "expected_sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"}
    }});
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
        "args":{"type":"object","description":"session.start: conversation_id, optional client_info. session.checkpoint: session_id, expected_session_version, context_hash, next_action, open_loops; optional client_info, progress, usage (schemas below). Batch feedback at meaningful boundaries; execution.report is terminal-only. session.end: session_id, expected_session_version. All claim/execution actions: session_id, expected_session_version. claim.acquire adds expected_work_version (0 when runtime absent), ttl_seconds (1..3600). claim.renew/release add claim_id, expected_fence, expected_lease_version; renew also ttl_seconds. execution.prepare adds claim_id, expected_fence, expected_lease_version, expected_work_version, input_digest (64 lowercase hex), declared_scope (canonical relative paths). execution.cancel adds execution_id, expected_execution_version. execution.start adds execution_id, expected_execution_version, claim_id, expected_fence, expected_lease_version, expected_work_version, execution_mode (caller_managed or reference_write_v1), optional expected_input_digest. reference_write_v1 requires the prepared input digest and system attestation authority; the service does not dispatch the local runner. execution.report adds execution_id, expected_execution_version, outcome (succeeded/failed/cancelled/unknown), optional output_digest (required for success), observed_paths, note. execution.attest adds execution_id, expected_execution_version, facts. execution.reconcile also adds expected_work_version, reviewed_receipt_id (latest inspected ID or null), clear_recovery_block, optional previous_epoch_recovery. Old-epoch recovery requires {execution_epoch (exact inspected epoch), executor_stopped (true to settle), review_reference (nonempty, <=2048 bytes, no controls)}; this is an authorized operator assertion, not independently verified fencing. facts: outcome, input_digest, optional output_digest (required for success), environment_digest, observed_paths, note. Digests are 64 lowercase hex. Versions are decimal strings; unknown fields fail. handoff.propose: session_id, expected_session_version, handoff_id, kind (execution|responsibility), to_person_id, package (task_id, contract_version, contract_hash, current_person_id, current_execution, consumed_context_digest, checkpoint_ids, artifact_versions, branch_id, working_directory, dependency_ids, todos, awaiting_replies, unknown_side_effects), optional proposed_successor/proposer_execution_id/proposer_fence/expires_at_ms, now_ms. handoff.inspect/accept/reject/cancel/timeout: session_id, expected_session_version, handoff_id, expected_handoff_version, now_ms; accept adds acceptor_person_id, successor_execution, prior_execution_stopped, prior_reconciled, context_reprepared, optional expected_current_fence; reject/cancel add by_person_id+reason; inspect adds inspector_person_id.  Timeout closes the proposal only and does not stop execution. evidence.submit: session_id, expected_session_version, payload, optional claimed_trust/artifact_hex/input_digest/dirty_tree/execution_id. review.open / delivery.submit_and_request_review: session_id, expected_session_version, evidence_id. review.accept/return: session_id, expected_session_version, round_id, reason. review.decide: session_id, expected_session_version, round_id, decision (approve|reject), reason — requires the matching human or Agent review grant and policy. work.rework: session_id, expected_session_version, round_id, note. work.complete / delivery.finalize: session_id, expected_session_version, evidence_id, context_complete, optional requested_policy. delivery.register_pr: repository, pr_number, pr_url, head_sha, fact_source (authorized_human_github_verification|operator_recorded_observation), observed_at (RFC3339), optional merge_sha/test_evidence_id/gh_* flags — v1 manual GitHub verification, not webhook sync. delivery.observe_pr: delivery_id, expected_head_sha, fact_source, observed_at, optional gh_approved/gh_merged/merge_sha. Query delivery.inspect separates GitHub submitted/approved/merged from AWR acceptance complete."}
    }});
    command["properties"]["args"]["properties"] = json!({
        "expected_session_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_work_version":{"type":"string","pattern":"^(0|[1-9][0-9]*)$",
            "description":"claim.acquire uses \"0\" before work runtime exists; otherwise use the current work version."},
        "expected_fence":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_lease_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_execution_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_handoff_version":{"type":"string","pattern":"^[1-9][0-9]*$"},
        "expected_current_fence":{"type":["string","null"],"pattern":"^(0|[1-9][0-9]*)$"},
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
            "phase":{"enum":["starting","implementing","testing","waiting_user","blocked","ready_for_review"]},
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
            "changes":{"type":"array","maxItems":256},
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
            "Scoped Team reads. Begin with capabilities (current identity/permissions), then work.next (own sessions and scoped task navigation). Consume work.prepare before any claim or execution. work.observe reads current scoped session/checkpoint, lease, execution and registered PR facts without changing context or authorizing execution; receipt payloads retain their original access gate, and missing model/usage remains explicit. audit.requests and audit.development provide paged metadata/history under the same personal/project permission boundary. Ops audit: audit.history / audit.export / audit.count (TMCP-040) — members see own allowed records; project-wide requires audit.read_project. Counts/exports use the same scope. Not full chat/tool-IO/token billing; PG audit does not claim DB-owner non-repudiation. Tool discovery is navigation-only; each query rechecks authority. Re-prepare after relevant changes. No execution admission.",
            query.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false)),
        Tool::new("awr_team_command",
            "Durable sessions, claims, confirmed handoffs and caller-managed execution under the shared TMCP action gate. Use work.prepare preconditions and a stable request_id. Readers cannot claim or write. Developers may maintain own session/execution on authorized work but cannot edit/publish plans or grant permissions. Only a fresh execution.start response with execution_authorized=true permits one run under the live lease. Exact replay reuses the original receipt; changed intent or expired/revoked authority is refused. Body fields cannot forge identity. Preparation, inspection and replay grant no execution rights. On unknown outcome inspect command.inspect before an exact retry; never repeat effects from a receipt. Refresh after conflicts or lease/contract changes. Cancellation is a request after start. Reports remain caller_asserted. Attestation requires operator-issued system authority at admission and now. For unknown effects, execution.inspect then operator execution.reconcile; confirm current versions and latest receipt. Recheck on permission, receipt or work changes. Settlement is not work completion. Evidence/review/rework/complete reuse WS-018 under explicit review.decide and delivery.finalize permissions on the same MCP plane: evidence.submit, review.open, delivery.submit_and_request_review, review.accept, review.return, review.decide, work.rework, work.complete, delivery.finalize, plus delivery.register_pr / delivery.observe_pr for versioned PR bindings. Author, owner, executor, reviewer and final-submitter are attributed separately on completion. GitHub submitted/approved/merged are distinct from AWR acceptance; URL, green CI, admin role or already-merged cannot skip acceptance. Head/contract/artifact mismatch invalidates approvals. Independence is by person; a second agent of the same person is not team-independent. Explicit caller_managed_execution_and_agent_review contracts allow review.decide with both agent_review membership and live Review delegation, a distinct author actor/client, and false human/team-acceptance flags; completion additionally requires artifact bytes, matching input/output bindings and a successful caller report reconciled by an authorized operator. Its evidence stays caller_asserted. Agent-reviewed receipts require explicit V2 dependency_acceptance on the same-stream consumer; unmapped dependencies remain blocked and WS-030 cross-stream adoption remains human-independent-only. PR facts require fact_source+observed_at: Agent verification uses operator_recorded_observation; authorized_human_github_verification requires an actual human observation. No webhook auto-sync claim. Completion receipts are for WS-030 adoption and omit provider-private sessions.",
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
            "Create or edit a planning draft candidate (planning.edit_draft). Split/cancel/archive are DraftChange ops inside changes. No hard-delete of history and no forging done via status. Stable request_id.",
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
            .with_instructions("The URL binds one operator-registered project; bearer credentials are checked on every request. Begin with awr_team_query capabilities, then work.next to resume own work or discover scoped candidates without manual task selection. Project admins manage members via awr_team_access_* tools after local owner bootstrap of the first admin. Raw credentials are never accepted or returned over MCP — generate them through protected Inspector issuance or awr-server access token and register only secret_hash. Work/session selectors bind a workstream; missing permissions never mean satisfied dependencies. Consume work.prepare before checkpointing. Follow its single guidance item (when/because/action/recheck_on); it grants no authority. Report client_info from known host metadata at session start or next checkpoint. Batch progress at phase/test completion, blocker, user wait or delivery boundaries; no per-tool reporting loop. Only submit usage with known host counter scope. Routine progress uses session.checkpoint; execution.report is for terminal outcomes. Session journals and claims grant no execution rights. Claim replay is a historical receipt; use claim.inspect for current lease state. If a command outcome is unknown, inspect its original request_id before an exact retry. Planning mutations use awr_team_planning_* with stable request_id; on disconnect call awr_team_planning_outcome or planning.outcome before any new ID. Controlled source/artifact content uses source.content / artifact.content — never path/URL/history bypass. Ops history uses audit.history/export/count within authorized scope. Recheck context and permission after relevant changes. MCP connection closure never closes a durable work session.")
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
