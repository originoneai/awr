use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Value, json};

pub const TOOL_NAMES: [&str; 32] = [
    "awr_project_status",
    "awr_work_ready",
    "awr_work_get",
    "awr_context_compile",
    "awr_work_transition",
    "awr_event_append",
    "awr_evidence_record",
    "awr_search",
    "awr_session_start",
    "awr_session_get",
    "awr_session_list",
    "awr_session_checkpoint",
    "awr_session_end",
    "awr_session_resume",
    "awr_session_claim",
    "awr_session_wait",
    "awr_session_reply",
    "awr_operation_get",
    "awr_operation_recover",
    "awr_source_reindex",
    "awr_work_prepare",
    "awr_completion_prepare",
    "awr_work_assess",
    "awr_work_manage",
    "awr_work_graph",
    "awr_change_preview",
    "awr_change_apply",
    "awr_change_status",
    "awr_change_recover",
    "awr_compaction_observe",
    "awr_compaction_get",
    "awr_compaction_defer",
];

fn compaction_tools() -> Vec<Tool> {
    let selector = |mut properties: Value, required: &[&str]| {
        properties["session"] = text();
        properties["conversation"] = text();
        let mut schema = object(properties, required);
        schema["anyOf"] = json!([{"required":["session"]},{"required":["conversation"]}]);
        schema
    };
    let count = optional(json!({"type":"integer","minimum":0}));
    let observation = object(
        json!({
            "compaction_id":{"type":"string","minLength":1,"maxLength":256},
            "sequence":{"type":"integer","minimum":1},"observed_at":{"type":"integer","minimum":1},
            "trigger":{"type":"string","enum":["automatic","manual"]},"model":text(),"source":text(),
            "measurement_scope":{"type":"string","enum":["full_request","history_only","unknown"]},
            "measurement_basis":{"type":"string","enum":["host_reported","estimated","unknown"]},
            "before_tokens":count,"after_tokens":count,"context_window_tokens":optional(json!({"type":"integer","minimum":1,"maximum":1000000000})),"duration_ms":count,
            "usage":optional(object(json!({"input_tokens":count,"output_tokens":count,"cached_input_tokens":count,"cost_usd":optional(json!({"type":"number","minimum":0}))}), &[]))
        }),
        &[
            "compaction_id",
            "sequence",
            "observed_at",
            "trigger",
            "model",
            "source",
            "measurement_scope",
            "measurement_basis",
        ],
    );
    vec![
        tool(
            "awr_compaction_observe",
            "Record completed native compaction: stable ID, increasing sequence, actual request usage/capacity; omit unknowns. One action; no session switch or compaction disable.",
            selector(
                json!({"expected_revision":revision(),"observation":observation,"policy":object(json!({"post_compaction_threshold_percent":{"type":"integer","minimum":1,"maximum":100,"default":50}}), &["post_compaction_threshold_percent"])}),
                &["expected_revision", "observation"],
            ),
            false,
            false,
        ),
        tool(
            "awr_compaction_get",
            "Read latest compaction action. include_observation adds measurements/costs. Runtime-only; source freshness is not verified.",
            selector(
                json!({"include_observation":{"type":"boolean","default":false}}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            "awr_compaction_defer",
            "After user postponement, defer the latest observation until next compaction. No permission grant or session switch.",
            selector(
                json!({"expected_revision":revision(),"observation_event_id":text()}),
                &["expected_revision", "observation_event_id"],
            ),
            false,
            false,
        ),
    ]
}

fn workflow_tools() -> Vec<Tool> {
    vec![
        tool(
            "awr_work_assess",
            "Assess management from contracts and attributed observations. Unknowns stay unknown; no execution grant or completion-policy change. Optional explain attaches assessment.explain without a new tool name.",
            object(
                json!({"work":text(),"branch":branch(),"explain":{"type":"boolean","default":false,"description":"Opt-in assessment.explain field; default false preserves legacy shape."}}),
                &["work"],
            ),
            true,
            false,
        ),
        tool(
            "awr_work_manage",
            "Record attributed management observations for the current contract/session and stable key. Continuous management never automatically downgrades.",
            object(
                json!({"work":text(),"session":text(),"expected_revision":revision(),"request_key":text(),"contract_fingerprint":text(),"observation":object(json!({
                "observed_at":{"type":"integer","minimum":0},"note":text(),
                "single_outcome":optional(json!({"type":"boolean"})),"bounded_scope":optional(json!({"type":"boolean"})),"single_executor":optional(json!({"type":"boolean"})),"no_deferred_wait":optional(json!({"type":"boolean"})),
                "independently_schedulable_units":optional(json!({"type":"integer","minimum":1})),"plan_valid":optional(json!({"type":"boolean"})),"outcome_known":optional(json!({"type":"boolean"})),"active_elapsed_ms":optional(json!({"type":"integer","minimum":0})),"completed_rework_cycles":optional(json!({"type":"integer","minimum":0}))
            }), &["observed_at","note"])}),
                &[
                    "work",
                    "session",
                    "expected_revision",
                    "request_key",
                    "contract_fingerprint",
                    "observation",
                ],
            ),
            false,
            false,
        ),
        tool(
            "awr_work_prepare",
            "Prepare readiness and required context. Consume context before checkpointing; no claim or completion-policy change. Optional explain attaches assessment.explain without a new tool name.",
            object(
                json!({"work":text(),"session":optional(text()),"branch":branch(),"goals":strings(),"source_sha":optional(text()),"budget":{"type":"integer","minimum":1,"maximum":100000},"compact_indexes":{"type":"boolean","default":false,"description":"Opt-in optional omission index compaction for successful, complete summary/action responses; full output and error semantics stay unchanged. Details remain at response_view.full_result."},"explain":{"type":"boolean","default":false,"description":"Opt-in assessment.explain field; default false preserves legacy full/summary/action."}}),
                &["work"],
            ),
            true,
            false,
        ),
        tool(
            "awr_completion_prepare",
            "Validate actual report bytes and current acceptance coverage; derive hash/evidence mapping. No verification execution or completion. Level is caller asserted.",
            object(
                json!({"work":text(),"report":text(),"evidence_key":text(),"source_sha":text(),"level":levels(),"branch":branch()}),
                &["work", "report", "evidence_key", "source_sha", "level"],
            ),
            true,
            false,
        ),
    ]
}

fn change_tools() -> Vec<Tool> {
    let fields = json!({"type":"object","description":"No identity/lifecycle/evidence writes."});
    let change = json!({"oneOf":[
        object(json!({"kind":{"const":"work_edit"},"work":text(),"fields":object(json!({"title":text(),"summary":text(),"priority":text(),"next_action":text()}),&[]),"source_fingerprint":optional(text())}), &["kind","work","fields"]),
        object(json!({"kind":{"const":"create"},"title":text(),"source_id":optional(text()),"fields":fields}), &["kind","title"]),
        object(json!({"kind":{"const":"batch"},"change":{"type":"object","description":"BatchChange: ledger(source_id,source_fingerprint,operations:fields/import/archive) or related(changes:document/adopt/ledger)."}}), &["kind","change"]),
        object(json!({"kind":{"const":"edit"},"change":{"oneOf":[
            object(json!({"operation":{"const":"fields"},"kind":{"const":"work_item"},"target":text(),"source_fingerprint":text(),"fields":fields}), &["operation","kind","target","source_fingerprint","fields"]),
            object(json!({"operation":{"const":"activate_draft"},"work":text(),"source_fingerprint":text()}), &["operation","work","source_fingerprint"])
        ]}}), &["kind","change"])
    ]});
    let key = json!({"type":"string","minLength":1,"maxLength":256});
    let kind = json!({"type":"string","enum":["create","batch","edit","work_edit"]});
    vec![
        tool(
            "awr_work_graph",
            "Read dependencies, dependents, readiness and claims. Roots include required ancestors; no silent closure truncation. Host owns execution/concurrency.",
            object(
                json!({"roots":strings(),"branch":branch(),"limit":{"type":"integer","minimum":1,"maximum":1000,"default":100}}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            "awr_change_preview",
            "Preview source change without writing. work_edit shows only changed fields. Keep request_id; review before apply/status/recover.",
            object(
                json!({"request_id":key,"reason":text(),"change":change}),
                &["request_id", "reason", "change"],
            ),
            true,
            false,
        ),
        tool(
            "awr_change_apply",
            "Apply reviewed preview at expected_revision. Query awr_change_status for outcome. Changed sources/revisions or occupied contracts conflict; drafts remain non-executable.",
            object(
                json!({"request_id":key,"reason":text(),"change":change,"expected_revision":revision(),"expected_preview":text()}),
                &[
                    "request_id",
                    "reason",
                    "change",
                    "expected_revision",
                    "expected_preview",
                ],
            ),
            false,
            true,
        ),
        tool(
            "awr_change_status",
            "Inspect this client's source-change outcome even with stale sources. Use original kind/request_id. Missing receipt does not prove no effect.",
            object(
                json!({"request_id":key,"kind":kind}),
                &["request_id", "kind"],
            ),
            true,
            false,
        ),
        tool(
            "awr_change_recover",
            "Recover inspected source change with the same client/kind/request_id. No Agent launch. Related-file writes are recoverable, not atomic.",
            object(
                json!({"request_id":key,"kind":kind,"expected_revision":revision()}),
                &["request_id", "kind", "expected_revision"],
            ),
            false,
            true,
        ),
    ]
}

fn object(properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({"type":"object","properties":properties,"additionalProperties":false});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}
fn text() -> Value {
    json!({"type":"string"})
}
fn strings() -> Value {
    json!({"type":"array","items":{"type":"string"}})
}
fn optional(mut schema: Value) -> Value {
    let kind = schema["type"].clone();
    schema["type"] = json!([kind, "null"]);
    schema
}
fn branch() -> Value {
    json!({"type":["string","null"],"description":"Name/ID; main=baseline; default=current."})
}
fn revision() -> Value {
    json!({"type":"integer","minimum":0,"maximum":i64::MAX})
}
fn levels() -> Value {
    json!({"type":"string","enum":["designed","implemented","locally_verified","real_environment_validated","release_candidate","released","unknown"]})
}
fn limit() -> Value {
    json!({"type":"integer","minimum":1,"maximum":100,"default":10})
}
fn tool(
    name: &'static str,
    description: &'static str,
    schema: Value,
    read_only: bool,
    destructive: bool,
) -> Tool {
    let mut tool = Tool::new(
        name,
        description,
        schema.as_object().expect("object schema").clone(),
    );
    tool.annotations = Some(
        ToolAnnotations::new()
            .read_only(read_only)
            .destructive(destructive)
            .idempotent(read_only)
            .open_world(false),
    );
    tool
}
pub fn tools() -> Vec<Tool> {
    let completion = object(
        json!({
            "version":{"type":"integer","const":1},"source_sha":text(),"minimum_level":levels(),
            "acceptance":{"type":"array","items":object(json!({"criterion":text(),"evidence":strings()}), &["criterion","evidence"])},
            "required_evidence":strings(),
        }),
        &["version", "source_sha", "acceptance"],
    );
    let mut catalog = vec![
        tool(
            TOOL_NAMES[0],
            "Daily action queue: continue, claimable, waiting, blocked; history is aggregated. Use view=full for earlier diagnostics. Explicit source_sha verifies reports. Reindex changed sources first; this query never writes.",
            object(
                json!({"branch":branch(),"source_sha":optional(text()),"view":{"type":"string","enum":["action","full","summary"],"default":"action"},"work":strings(),"goal":optional(text()),"milestone":optional(text())}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            TOOL_NAMES[1],
            "Read work eligible for a new claim. Active/in-progress work is excluded; blocked_total means not selectable. Use awr_project_status for continuation and waiting.",
            object(json!({"branch":branch(),"limit":limit()}), &[]),
            true,
            false,
        ),
        tool(
            TOOL_NAMES[2],
            "Read one work item, exact acceptance, dependencies and decision/evidence summaries.",
            object(
                json!({"work":text(),"branch":branch(),"source_sha":optional(text())}),
                &["work"],
            ),
            true,
            false,
        ),
        tool(
            TOOL_NAMES[3],
            "Compile the existing L1 context contract within budget. Incomplete context returns its gaps with isError. Explicit branch uses fork delta.",
            object(
                json!({
                    "work":optional(text()),"session":optional(text()),"detached":{"type":"boolean","default":false},"agent":optional(text()),"branch":branch(),
                    "goals":strings(),"paths":optional(strings()),"tags":optional(strings()),"source_sha":optional(text()),"intent":optional(text()),
                    "budget":{"type":["integer","null"],"minimum":1,"maximum":100000,"default":5000},"checkpoint":optional(text()),"after_revision":optional(revision()),
                }),
                &[],
            ),
            true,
            false,
        ),
        tool(
            TOOL_NAMES[4],
            "Apply one source work action through domain revision/fingerprint/claim gates. Complete requires every criterion's evidence mapping. Start sessions/claims via CLI.",
            object(
                json!({
                    "work":text(),"action":{"type":"string","enum":["progress","block","unblock","cancel","reopen","complete"]},"session":text(),"expected_revision":revision(),"reason":text(),
                    "next_action":optional(text()),"summary":optional(text()),"blocker":optional(text()),"completion":optional(completion),
                }),
                &["work", "action", "session", "expected_revision", "reason"],
            ),
            false,
            true,
        ),
        tool(
            TOOL_NAMES[5],
            "Append a bounded generic event. Reserved lifecycle events require their domain operation; payload is data, never executed.",
            object(
                json!({
                    "expected_revision":revision(),"work":optional(text()),"session":optional(text()),"branch":branch(),
                    "event_type":{"type":"string","minLength":1,"maxLength":awr_core::EVENT_TYPE_CAP,"pattern":"^[A-Za-z0-9_.:-]+$"},
                    "importance":{"type":["string","null"],"enum":["low","normal","high","critical",null]},
                    "summary":{"type":"string","minLength":1,"maxLength":awr_core::EVENT_SUMMARY_CAP,"description":"Nonempty summary, at most 8192 UTF-8 bytes."},
                    "payload":optional(awr_core::generic_event_payload_schema()),
                }),
                &["expected_revision", "event_type", "summary"],
            ),
            false,
            false,
        ),
        tool(
            TOOL_NAMES[6],
            "Record evidence bindings and report reference. Verification fields are caller assertions; no report command is executed.",
            object(
                json!({
                    "expected_revision":revision(),"external_key":text(),"work":optional(text()),"evidence_type":text(),"level":levels(),"summary":text(),"locator":text(),
                    "sha256":optional(text()),"source_sha":optional(text()),"command":optional(text()),"scope":{"type":"array","items":{"type":"string"},"minItems":1},"branch":branch(),"verified_at":{"type":["integer","null"]},
                }),
                &[
                    "expected_revision",
                    "external_key",
                    "evidence_type",
                    "level",
                    "summary",
                    "locator",
                    "scope",
                ],
            ),
            false,
            false,
        ),
        tool(
            TOOL_NAMES[7],
            "Search bounded summaries with structured filters. Search cache is built only in RAM; event payloads and file bodies are excluded.",
            object(
                json!({
                    "text":optional(text()),"kind":{"type":["string","null"],"enum":["goal","plan","rule","work_item","work","decision","evidence","event",null]},
                    "status":optional(text()),"work":optional(text()),"limit":limit(),
                }),
                &[],
            ),
            true,
            false,
        ),
    ];
    catalog.extend(lifecycle_tools());
    catalog.extend(continuity_tools());
    catalog.extend(workflow_tools());
    catalog.extend(change_tools());
    catalog.extend(compaction_tools());
    catalog.push(crate::team_handoff::tool());
    for entry in &mut catalog {
        if matches!(
            entry.name.as_ref(),
            "awr_work_prepare" | "awr_work_transition"
        ) {
            let schema = std::sync::Arc::make_mut(&mut entry.input_schema);
            let views = if entry.name == "awr_work_prepare" {
                json!(["full", "summary", "action"])
            } else {
                json!(["full", "summary"])
            };
            schema["properties"].as_object_mut().unwrap().insert("response_view".into(),json!({"type":"string","enum":views,"default":"full","description":"Views preserve identity and required context. Summary writes require request_id."}));
        }
        if entry
            .annotations
            .as_ref()
            .is_some_and(|a| a.read_only_hint == Some(false))
            && entry.name != "awr_operation_recover"
            && !crate::changes::NAMES.contains(&entry.name.as_ref())
        {
            let schema = std::sync::Arc::make_mut(&mut entry.input_schema);
            schema["properties"].as_object_mut().unwrap().insert("request_id".into(),json!({"type":"string","minLength":1,"maxLength":256,"description":"Stable HTTP ID; query loss before exact retry."}));
        }
        if matches!(
            entry.name.as_ref(),
            "awr_context_compile" | "awr_work_transition" | "awr_event_append"
        ) {
            let schema = std::sync::Arc::make_mut(&mut entry.input_schema);
            schema["properties"]
                .as_object_mut()
                .unwrap()
                .insert("conversation".into(), optional(text()));
            if entry.name == "awr_work_transition" {
                schema["required"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|v| v != "session");
                schema.insert(
                    "anyOf".into(),
                    json!([{"required":["session"]},{"required":["conversation"]}]),
                );
            }
        }
    }
    catalog
}

fn lifecycle_tools() -> Vec<Tool> {
    let selector = |properties: Value, required: &[&str]| {
        let mut value = object(properties, required);
        value["properties"]["session"] = optional(text());
        value["properties"]["conversation"] = optional(text());
        value["anyOf"] = json!([{"required":["session"]},{"required":["conversation"]}]);
        value
    };
    let ttl = optional(json!({"type":"integer","minimum":1}));
    vec![
        tool(
            "awr_session_start",
            "Atomically bind this client's conversation to work; optionally claim. Identical bindings reuse the session. Inspect current claims/status.",
            object(
                json!({"work":text(),"conversation":text(),"agent":text(),"provider":text(),"model":text(),"expected_revision":revision(),"claim":{"type":"boolean","default":false},"ttl_ms":ttl,"branch":branch()}),
                &[
                    "work",
                    "conversation",
                    "agent",
                    "provider",
                    "model",
                    "expected_revision",
                ],
            ),
            false,
            false,
        ),
        tool(
            "awr_session_get",
            "Read session, binding, claims, checkpoints, interrupted saves and successor. Runtime-only; available with stale sources.",
            selector(json!({}), &[]),
            true,
            false,
        ),
        tool(
            "awr_session_list",
            "List this client's persistent sessions newest first. Use next_before_revision to read older entries; transport connections are not work sessions.",
            object(
                json!({"limit":limit(),"before_revision":optional(revision())}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            "awr_session_checkpoint",
            "Save actual progress, including failures and context gaps, without changing source work. Never invent a context hash. expected_revision is the project revision, not session.revision. Supply agent to record an unverified caller declaration; changing agents requires awr_session_resume. Omission retains compatibility and records an undeclared caller, not the session agent.",
            selector(
                json!({"expected_revision":revision(),"agent":text(),"context_hash":text(),"digest":text(),"next_action":text(),"open_loops":strings(),"changed_entities":strings()}),
                &["expected_revision", "context_hash", "digest", "next_action"],
            ),
            false,
            false,
        ),
        tool(
            "awr_session_end",
            "Explicitly close a work session and release its claims. A transport disconnect does not perform this action. Available with stale sources.",
            selector(
                json!({"expected_revision":revision(),"outcome":{"type":"string","enum":["ended","interrupted","incomplete"]}}),
                &["expected_revision", "outcome"],
            ),
            false,
            false,
        ),
        tool(
            "awr_session_resume",
            "Resume a predecessor into a bound conversation: refresh context, inherit checkpoint and transfer/acquire claims through resume gates.",
            object(
                json!({"session":text(),"conversation":text(),"agent":text(),"provider":text(),"model":text(),"expected_revision":revision(),"claim":{"type":"string","enum":["inherit","acquire","none"],"default":"inherit"},"ttl_ms":ttl,"budget":{"type":["integer","null"],"minimum":1,"maximum":100000},"paths":optional(strings()),"tags":optional(strings()),"goals":strings(),"source_sha":optional(text())}),
                &[
                    "session",
                    "conversation",
                    "agent",
                    "provider",
                    "model",
                    "expected_revision",
                ],
            ),
            false,
            false,
        ),
        tool(
            "awr_session_claim",
            "Acquire a work claim with optional TTL, or release an explicitly identified claim owned by the session.",
            selector(
                json!({"action":{"type":"string","enum":["acquire","release"]},"expected_revision":revision(),"claim":optional(text()),"ttl_ms":ttl}),
                &["action", "expected_revision"],
            ),
            false,
            false,
        ),
    ]
}

fn continuity_tools() -> Vec<Tool> {
    let mut wait = object(
        json!({"session":optional(text()),"conversation":optional(text()),"expected_revision":revision(),"question":text(),"context_hash":text(),"digest":text(),"next_action":text(),"open_loops":strings()}),
        &[
            "expected_revision",
            "question",
            "context_hash",
            "digest",
            "next_action",
        ],
    );
    wait["anyOf"] = json!([{"required":["session"]},{"required":["conversation"]}]);
    vec![
        tool(
            "awr_session_wait",
            "Save a checkpoint with caller-supplied progress and consumed context hash, then persist a user-input wait. The host collects input; AWR does not start a new model turn.",
            wait,
            false,
            false,
        ),
        tool(
            "awr_session_reply",
            "Record a user reply or explicit cancellation reason for this client's wait. Does not execute work. Query the session and compile current context before continuing.",
            object(
                json!({"wait":text(),"expected_revision":revision(),"reply":text(),"cancel":{"type":"boolean","default":false}}),
                &["wait", "expected_revision", "reply"],
            ),
            false,
            false,
        ),
        tool(
            "awr_operation_get",
            "Read this client's durable request outcome and exactly correlated domain receipts, including after disconnect or restart. A started request has unknown outcome; never replay it automatically.",
            object(json!({"request_id":text()}), &["request_id"]),
            true,
            false,
        ),
        tool(
            "awr_operation_recover",
            "Recover an unknown request only from its correlated committed terminal domain receipt. Never re-executes the operation; absence or partial receipts stays unknown. Compile fresh context after recovery.",
            object(
                json!({"request_id":text(),"expected_revision":revision()}),
                &["request_id", "expected_revision"],
            ),
            false,
            false,
        ),
        tool(
            "awr_source_reindex",
            "Explicitly refresh this registered project's source projections after authoritative files change. Does not edit source files; partial failures remain visible.",
            object(
                json!({"expected_revision":revision()}),
                &["expected_revision"],
            ),
            false,
            false,
        ),
    ]
}

pub(crate) fn shared_tools() -> Vec<Tool> {
    let mut catalog = tools();
    for tool in &mut catalog {
        let schema = std::sync::Arc::make_mut(&mut tool.input_schema);
        schema["properties"].as_object_mut().unwrap().insert("project".into(),
            json!({"type":"string","description":"Registered project key from awr_projects_list. Required on every call; never a filesystem path."}));
        schema
            .entry("required")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!("project"));
        if tool
            .annotations
            .as_ref()
            .is_some_and(|a| a.read_only_hint == Some(false))
            && tool.name != "awr_operation_recover"
        {
            schema["required"]
                .as_array_mut()
                .unwrap()
                .push(json!("request_id"));
        }
    }
    catalog.insert(0, tool("awr_projects_list", "List this authenticated client's registered project keys and access. Does not read project source files.", object(json!({}), &[]), true, false));
    catalog.push(crate::workstreams::tool());
    catalog
}
