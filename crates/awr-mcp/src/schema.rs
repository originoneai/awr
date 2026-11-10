use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Value, json};

pub const TOOL_NAMES: [&str; 38] = [
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
    "awr_spec_check",
    "awr_change_preview",
    "awr_change_apply",
    "awr_change_status",
    "awr_change_recover",
    "awr_compaction_observe",
    "awr_compaction_get",
    "awr_compaction_defer",
    "awr_work_admit",
    "awr_work_contain",
    "awr_work_reassess",
    "awr_work_gap",
    "awr_work_finding",
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
            "Record completed native compaction telemetry. Returns the observation receipt and next action.",
            selector(
                json!({"expected_revision":revision(),"observation":observation,"policy":object(json!({"post_compaction_threshold_percent":{"type":"integer","minimum":1,"maximum":100,"default":50}}), &["post_compaction_threshold_percent"])}),
                &["expected_revision", "observation"],
            ),
            false,
            false,
        ),
        tool(
            "awr_compaction_get",
            "Inspect the latest compaction action. Returns guidance and optional measurements and costs.",
            selector(
                json!({"include_observation":{"type":"boolean","default":false}}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            "awr_compaction_defer",
            "Defer advice after the user postpones the latest observation. Returns the deferral receipt.",
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
            "Assess management requirements from the current contract. Returns the assessment and optional explanation.",
            object(
                json!({"work":text(),"branch":branch(),"explain":{"type":"boolean","default":false,"description":"Opt-in assessment.explain field; default false preserves legacy shape."}}),
                &["work"],
            ),
            true,
            false,
        ),
        tool(
            "awr_work_manage",
            "Record attributed management observations for a contract. Returns the assessment receipt.",
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
            "awr_work_admit",
            "Record an admission proposal or decision for one work item. Returns the receipt and current admission state.",
            object(
                json!({"work":text(),"session":optional(text()),"agent":optional(text()),"expected_revision":revision(),"request_key":text(),
                "decision":{"type":"string","enum":["proposed","approved","denied"]},"contract_fingerprint":text(),
                "judgment":optional(object(json!({"contribution":text(),"necessity":text(),"alternatives":text(),
                "deliverable":text(),"exit":text(),"responsibility":text()}),
                    &["contribution","necessity","alternatives","deliverable","exit","responsibility"])),
                "proposal_event":optional(text()),"reason":optional(text())}),
                &[
                    "work",
                    "expected_revision",
                    "request_key",
                    "decision",
                    "contract_fingerprint",
                ],
            ),
            false,
            false,
        ),
        tool(
            "awr_work_contain",
            "Record a containment proposal or decision for a parent’s children. Returns the receipt and bound change set.",
            object(
                json!({"parent":text(),"session":optional(text()),"agent":optional(text()),"expected_revision":revision(),"request_key":text(),
                "decision":{"type":"string","enum":["proposed","approved","denied"]},"change_set_fingerprint":text(),
                "judgment":optional(object(json!({"contained_because":text(),"reuse_checked":text(),"out_of_scope_excluded":text()}),
                    &["contained_because","reuse_checked","out_of_scope_excluded"])),
                "proposal_event":optional(text()),"reason":optional(text())}),
                &[
                    "parent",
                    "expected_revision",
                    "request_key",
                    "decision",
                    "change_set_fingerprint",
                ],
            ),
            false,
            false,
        ),
        tool(
            "awr_work_reassess",
            "Record a decision on repeated failure, unknown outcomes or spent budgets. Returns the reassessment receipt.",
            object(
                json!({"work":text(),"session":optional(text()),"agent":optional(text()),"expected_revision":revision(),"request_key":text(),
                "decision":{"type":"string","enum":["continue","wait","reuse","alter_method","split","replan","deliver_with_known_issues","reduce_scope","escalate"]},
                "answers":optional(json!({"type":"string","enum":["facts","budget"]})),"facts_fingerprint":text(),
                "judgment":object(json!({"reason":text(),"next_step":text(),"evidence_considered":text()}),
                    &["reason","next_step","evidence_considered"]),
                "root_cause":optional(object(json!({"label":text(),"references":{"type":"array","items":{"type":"string"}}}),
                    &["label","references"])),
                "known_issues":optional(json!({"type":"array","items":{"type":"string"}})),
                "reduced_scope":optional(text()),"escalated_to":optional(text())}),
                &[
                    "work",
                    "expected_revision",
                    "request_key",
                    "decision",
                    "facts_fingerprint",
                    "judgment",
                ],
            ),
            false,
            false,
        ),
        tool(
            "awr_work_gap",
            "Open, resolve or withdraw a gap in one work item. Returns the gap record and affected judgments.",
            object(
                json!({"work":text(),"session":optional(text()),"agent":optional(text()),"expected_revision":revision(),"request_key":text(),
                "gap_key":text(),"action":{"type":"string","enum":["open","resolve","withdraw"]},
                "route":optional(json!({"type":"string","enum":["intent","acceptance","design","plan","work_definition"]})),
                "resolution":optional(json!({"type":"string","enum":["fact_lookup","feasibility_experiment","authorized_choice","environment_wait"]})),
                "subject":optional(text()),"experiment":optional(text()),
                "authority":optional(json!({"type":"array","items":{"type":"string"}})),
                "waiting_for":optional(text()),"outcome":optional(text()),
                "sources":optional(json!({"type":"array","items":{"type":"string"}})),
                "observed":optional(text()),"reason":optional(text())}),
                &[
                    "work",
                    "expected_revision",
                    "request_key",
                    "gap_key",
                    "action",
                ],
            ),
            false,
            false,
        ),
        tool(
            "awr_work_finding",
            "Record or resolve a review finding. Returns its derived severity and completion impact.",
            object(
                json!({"work":text(),"session":optional(text()),"agent":optional(text()),"expected_revision":revision(),"request_key":text(),
                "finding_key":text(),"action":{"type":"string","enum":["record","resolve"]},
                "round":optional(json!({"type":"integer","minimum":1})),"summary":optional(text()),
                "criterion":optional(text()),"rule":optional(text()),
                "counterexample":optional(object(json!({"given":text(),"when":text(),"then":text()}),&["given","when","then"])),
                "risk":optional(object(json!({"kind":{"type":"string","enum":["security","data_loss"]},"reasoning":text()}),&["kind","reasoning"])),
                "resolution":optional(text())}),
                &[
                    "work",
                    "expected_revision",
                    "request_key",
                    "finding_key",
                    "action",
                ],
            ),
            false,
            false,
        ),
        tool(
            "awr_work_prepare",
            "Prepare work readiness and required context. Returns management requirements, context and the next action.",
            object(
                json!({"work":text(),"session":optional(text()),"branch":branch(),"goals":strings(),"source_sha":optional(text()),"budget":{"type":"integer","minimum":1,"maximum":100000},"explain":{"type":"boolean","default":false,"description":"Opt-in assessment.explain field; default false omits the explanation independently of response_view."}}),
                &["work"],
            ),
            true,
            false,
        ),
        tool(
            "awr_completion_prepare",
            "Validate a completion report against current acceptance. Returns evidence and completion inputs with any format advisory.",
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
            "Read a task’s dependency impact. Returns required ancestors, dependents, readiness, claims and graph bindings.",
            object(
                json!({"roots":strings(),"branch":branch(),"limit":{"type":"integer","minimum":1,"maximum":1000,"default":100}}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            "awr_spec_check",
            "Inspect approved specification bindings and coverage. Returns declared coverage, supersessions and unresolved references.",
            object(
                json!({"spec":optional(text()),"limit":{"type":"integer","minimum":1,"maximum":10000,"default":1000}}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            "awr_change_preview",
            "Preview a proposed source change. Returns reviewed fields or source text and a preview fingerprint.",
            object(
                json!({"request_id":key,"reason":text(),"change":change}),
                &["request_id", "reason", "change"],
            ),
            true,
            false,
        ),
        tool(
            "awr_change_apply",
            "Apply a reviewed source preview at the expected revision. Returns the source-operation receipt.",
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
            "Inspect this client’s source-change operation. Returns its durable outcome and recovery state.",
            object(
                json!({"request_id":key,"kind":kind}),
                &["request_id", "kind"],
            ),
            true,
            false,
        ),
        tool(
            "awr_change_recover",
            "Recover an inspected source-change operation. Returns the phases completed and remaining recovery state.",
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
            "Read the daily action queues or full project status. Returns work summaries, diagnostics and evidence currency.",
            object(
                json!({"branch":branch(),"source_sha":optional(text()),"view":{"type":"string","enum":["action","full","summary"],"default":"action"},"work":strings(),"goal":optional(text()),"milestone":optional(text())}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            TOOL_NAMES[1],
            "Find work eligible for a new claim. Returns a bounded list with readiness diagnostics.",
            object(json!({"branch":branch(),"limit":limit()}), &[]),
            true,
            false,
        ),
        tool(
            TOOL_NAMES[2],
            "Read one work item. Returns acceptance criteria, dependencies, decisions and evidence summaries.",
            object(
                json!({"work":text(),"branch":branch(),"source_sha":optional(text())}),
                &["work"],
            ),
            true,
            false,
        ),
        tool(
            TOOL_NAMES[3],
            "Compile task context within a token budget. Returns required facts, source bindings, a context hash and any gaps.",
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
            "Apply one work lifecycle action. Returns the source-change receipt and current work state.",
            object(
                json!({
                    "work":text(),"action":{"type":"string","enum":["progress","block","unblock","cancel","reopen","complete"]},"session":text(),"expected_revision":revision(),"reason":text(),
                    "next_action":optional(text()),"summary":optional(text()),"blocker":optional(text()),"completion":optional(completion),
                    "end_session":optional(json!({"type":"boolean","description":"complete only: end the session in the same request; refused while it holds claims on other work."})),
                }),
                &["work", "action", "session", "expected_revision", "reason"],
            ),
            false,
            true,
        ),
        tool(
            TOOL_NAMES[5],
            "Append a bounded generic event. Returns the recorded event and project revision.",
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
            "Record evidence bindings and a report reference. Returns the evidence record and event receipt.",
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
            "Search project summaries with structured filters. Returns bounded matching summaries and references.",
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
    let no_expiry = json!({"type":"boolean","default":false});
    vec![
        tool(
            "awr_session_start",
            "Bind a client conversation to work and optionally claim it. Returns the session, binding and claim receipt.",
            object(
                json!({"work":text(),"conversation":text(),"agent":text(),"provider":text(),"model":text(),"expected_revision":revision(),"claim":{"type":"boolean","default":false},"ttl_ms":ttl,"no_expiry":no_expiry,"branch":branch()}),
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
            "Inspect a work session. Returns its binding, claims, checkpoints, interrupted saves and successor.",
            selector(json!({}), &[]),
            true,
            false,
        ),
        tool(
            "awr_session_list",
            "List this client’s persistent sessions. Returns newest entries and a cursor for older sessions.",
            object(
                json!({"limit":limit(),"before_revision":optional(revision())}),
                &[],
            ),
            true,
            false,
        ),
        tool(
            "awr_session_checkpoint",
            "Save progress and the last consumed context hash. Returns the checkpoint and observed project delta.",
            selector(
                json!({"expected_revision":revision(),"agent":text(),"context_hash":text(),"digest":text(),"next_action":text(),"open_loops":strings(),"changed_entities":strings()}),
                &["expected_revision", "context_hash", "digest", "next_action"],
            ),
            false,
            false,
        ),
        tool(
            "awr_session_end",
            "Close a work session and release its claims. Returns the session-end receipt.",
            selector(
                json!({"expected_revision":revision(),"outcome":{"type":"string","enum":["ended","interrupted","incomplete"]}}),
                &["expected_revision", "outcome"],
            ),
            false,
            false,
        ),
        tool(
            "awr_session_resume",
            "Continue from a predecessor in a bound conversation. Returns the successor session, refreshed context and claim outcome.",
            object(
                json!({"session":text(),"conversation":text(),"agent":text(),"provider":text(),"model":text(),"expected_revision":revision(),"claim":{"type":"string","enum":["inherit","acquire","none"],"default":"inherit"},"ttl_ms":ttl,"no_expiry":no_expiry,"budget":{"type":["integer","null"],"minimum":1,"maximum":100000},"paths":optional(strings()),"tags":optional(strings()),"goals":strings(),"source_sha":optional(text())}),
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
            "Acquire or release a session’s work claim. Returns the claim receipt and current project revision.",
            selector(
                json!({"action":{"type":"string","enum":["acquire","release"]},"expected_revision":revision(),"claim":optional(text()),"ttl_ms":ttl,"no_expiry":no_expiry}),
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
            "Save a checkpoint and persist a user-input wait. Returns the checkpoint and wait receipt.",
            wait,
            false,
            false,
        ),
        tool(
            "awr_session_reply",
            "Record a reply or cancellation for this client’s wait. Returns the updated wait receipt.",
            object(
                json!({"wait":text(),"expected_revision":revision(),"reply":text(),"cancel":{"type":"boolean","default":false}}),
                &["wait", "expected_revision", "reply"],
            ),
            false,
            false,
        ),
        tool(
            "awr_operation_get",
            "Inspect this client’s durable request outcome. Returns its state and correlated domain receipts.",
            object(json!({"request_id":text()}), &["request_id"]),
            true,
            false,
        ),
        tool(
            "awr_operation_recover",
            "Resolve an unknown request from a committed terminal receipt. Returns the recovered outcome or remaining uncertainty.",
            object(
                json!({"request_id":text(),"expected_revision":revision()}),
                &["request_id", "expected_revision"],
            ),
            false,
            false,
        ),
        tool(
            "awr_source_reindex",
            "Refresh this project’s source projections. Returns indexing receipts and any source diagnostics.",
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
    catalog.insert(0, tool("awr_projects_list", "List this authenticated client’s registered projects. Returns project keys and access grants.", object(json!({}), &[]), true, false));
    catalog.push(crate::workstreams::tool());
    catalog
}
