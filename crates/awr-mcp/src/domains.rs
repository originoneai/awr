//! Two-level progressive disclosure: domains at tools/list, children on demand.
use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Value, json};

pub(crate) const MANIFEST_VERSION: &str = "1";
pub(crate) const EXPOSURE_ENV: &str = "AWR_MCP_TOOL_EXPOSURE_MODE";

#[derive(Clone, Copy)]
pub struct DomainDef {
    pub name: &'static str,
    pub description: &'static str,
    pub children: &'static [&'static str],
    pub limits: &'static [&'static str],
}

const DISCOVERY_SUFFIX: &str =
    " Returns child schemas on an empty call; use child_tool and arguments to act.";

pub const DOMAINS: &[DomainDef] = &[
    DomainDef {
        name: "awr_query",
        description: "Read current action status, claimable work, one work item, or bounded search.",
        children: &[
            "awr_project_status",
            "awr_work_ready",
            "awr_work_get",
            "awr_search",
        ],
        limits: &[
            "Project discovery lists registered keys and access without reading source files.",
            "Queries verify sources without persistent writes. Reindex changed sources explicitly before retrying.",
            "Status defaults to action queues with aggregated history; view=full expands earlier diagnostics. Explicit source_sha verifies reports.",
            "Claimable work excludes active/in-progress work; blocked_total counts unselectable work. Status includes continuation and waiting.",
            "Search builds its cache in RAM and searches summaries, excluding event payloads and file bodies.",
        ],
    },
    DomainDef {
        name: "awr_context",
        description: "Compile L1 context, prepare work or completion, and assess management.",
        children: &[
            "awr_context_compile",
            "awr_work_prepare",
            "awr_work_assess",
            "awr_completion_prepare",
        ],
        limits: &[
            "Context completeness describes available facts. Understanding, passing checks and completion require their own evidence; checkpoints retain source-backed next actions.",
            "Consume required context before checkpointing; preparation acquires no claim and leaves completion policy unchanged.",
            "Incomplete context returns gaps with isError. An explicit context branch uses the fork delta.",
            "Preparation summary/action views aggregate optional omissions with count, reason and full_result query; full retains details.",
            "Assessments keep unknowns unknown, grant no execution permission and leave completion policy unchanged. explain adds assessment.explain under the existing tool name.",
            "Completion preparation reads actual report bytes and current acceptance, computes the digest and derives the evidence mapping; it executes no check and completes no work.",
            "The completion evidence level is caller asserted. A nullable advisory warns about deviation from exactly one trailing LF; that format alone neither modifies nor rejects the report.",
        ],
    },
    DomainDef {
        name: "awr_work",
        description: "Apply reviewed work transitions, manage progress, and read dependency graphs.",
        children: &[
            "awr_work_transition",
            "awr_work_check",
            "awr_work_manage",
            "awr_work_admit",
            "awr_work_contain",
            "awr_work_reassess",
            "awr_work_gap",
            "awr_work_finding",
            "awr_work_graph",
            "awr_spec_check",
        ],
        limits: &[
            "Work actions enforce revision, source fingerprint and owned claim gates. Completion requires an evidence mapping for every criterion. Start sessions and claims through lifecycle tools or CLI.",
            "Management observations bind the current contract/session and a stable key. Continuous management never automatically downgrades.",
            "Admission proposals request a decision. Checks establish provenance and currency, grant only the recorded scope and leave completion policy unchanged.",
            "Containment binds the parent contract, exact children and their dependencies. Proposals are not decisions; checks cover scope, currency and authorization rather than intent.",
            "Reassessment binds the exact failure/outcome facts or spent rework/effort budget; a request alone is not a decision. Checks establish currency and attribution rather than cause. It opens no task and holds no other work.",
            "Gap closure requires a fact lookup, an experiment actually run, an authorized choice or an observed environment change. Only judgments reading that layer on that work are questioned; no task is opened and no other work is held.",
            "Finding severity is derived: only a reproducible required-criterion/hard-rule failure or a reasoned security/data-loss risk blocks that work's completion. Other findings are linked deferred records.",
            "Graphs include required ancestors and refuse oversized closures instead of truncating them. Execution and concurrency belong to the host.",
            "Specification coverage describes explicit approved bindings; unknown references remain unknown. It neither edits sources nor proves acceptance.",
            "Specification checks require owner stdio or an unisolated project-level shared grant; shared workstream scopes are unsupported.",
        ],
    },
    DomainDef {
        name: "awr_evidence",
        description: "Append bounded generic events and record evidence bindings.",
        children: &["awr_event_append", "awr_evidence_record"],
        limits: &[
            "Reserved lifecycle events require their domain operation. Event payloads are stored data and are never executed.",
            "Evidence verification fields are caller assertions; recording evidence never executes a report command.",
        ],
    },
    DomainDef {
        name: "awr_session",
        description: "Start, inspect, list, checkpoint, end, resume, or claim bound work sessions.",
        children: &[
            "awr_session_start",
            "awr_session_get",
            "awr_session_list",
            "awr_session_checkpoint",
            "awr_session_end",
            "awr_session_resume",
            "awr_session_claim",
        ],
        limits: &[
            "Session start atomically binds this client's conversation to work. Identical bindings reuse the session; inspect current claims and status.",
            "Omitted ttl_ms uses the project's default lifetime, or no expiry if absent; no_expiry explicitly requests no expiry.",
            "Session inspection is runtime-only and available with stale sources. Session lists are client-scoped, newest first; next_before_revision pages older records.",
            "Checkpoint progress includes failures and gaps and leaves source work unchanged. Use the actual last-consumed context hash and project revision, rather than session.revision.",
            "Checkpoint agent is an unverified caller declaration. Changing agents requires resume; omission records an undeclared caller instead of the session agent.",
            "Transport connections are not work sessions. Disconnecting leaves sessions open; explicit end releases claims and works with stale sources.",
            "Resume refreshes context, inherits checkpoints and transfers/acquires claims through resume gates. Release requires an explicit claim owned by the session.",
        ],
    },
    DomainDef {
        name: "awr_continuity",
        description: "Persist user waits/replies, read durable operation outcomes, reindex sources.",
        children: &[
            "awr_session_wait",
            "awr_session_reply",
            "awr_operation_get",
            "awr_operation_recover",
            "awr_source_reindex",
        ],
        limits: &[
            "Wait progress and context hashes are caller supplied. The host collects replies; AWR starts no model turn and a reply executes no work.",
            "After reply, inspect the session and compile current context before continuing.",
            "Request outcomes and correlated receipts survive disconnect/restart. A started request remains unknown; never replay it automatically.",
            "Operation recovery uses only a correlated committed terminal receipt and never re-executes work. Missing or partial receipts remain unknown; compile fresh context afterward.",
            "Reindex refreshes registered projections without editing authoritative files; partial failures remain visible.",
        ],
    },
    DomainDef {
        name: "awr_change",
        description: "Preview, apply, inspect, and recover reviewed source changes.",
        children: &[
            "awr_change_preview",
            "awr_change_apply",
            "awr_change_status",
            "awr_change_recover",
        ],
        limits: &[
            "Preview persists no source change. work_edit shows changed fields only; retain request_id and review before apply, status or recovery.",
            "Apply checks expected_revision and the reviewed preview. Changed sources/revisions or occupied contracts conflict; drafts stay non-executable.",
            "Source-operation status uses the original client, kind and request_id and remains available with stale sources. Missing receipts do not prove no effect.",
            "Recovery uses the same client/kind/request_id and launches no Agent. Related-file writes are recoverable rather than atomic.",
        ],
    },
    DomainDef {
        name: "awr_compaction",
        description: "Observe native compaction telemetry, read assessments, or defer advice.",
        children: &[
            "awr_compaction_observe",
            "awr_compaction_get",
            "awr_compaction_defer",
        ],
        limits: &[
            "Record completed native compaction with a stable ID, increasing sequence and actual request usage/capacity; omit unknown measurements.",
            "Observation returns one action; it neither switches sessions nor disables compaction. Reads are runtime-only and leave source freshness unverified.",
            "include_observation expands measurements and costs. Deferral requires user postponement and lasts until the next compaction; it grants no permission or session switch.",
        ],
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExposureMode {
    Hierarchical,
    Flat,
}

impl ExposureMode {
    pub(crate) fn from_env() -> Self {
        match std::env::var(EXPOSURE_ENV).as_deref() {
            Ok("flat") => Self::Flat,
            _ => Self::Hierarchical,
        }
    }
}

pub fn is_public_domain(name: &str) -> bool {
    is_domain(name)
}
pub(crate) fn is_domain(name: &str) -> bool {
    DOMAINS.iter().any(|d| d.name == name)
}

fn input_schema(shared: bool) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "child_tool": {"type": "string", "minLength": 1},
            "arguments": {"type": "object"},
            "manifest_version": {"type": "string", "minLength": 1},
        },
        "additionalProperties": false,
    });
    if shared {
        schema["properties"]["project"] = json!({
            "type": "string",
            "description": "Registered project key from awr_projects_list. Required on every call; never a filesystem path."
        });
        schema["required"] = json!(["project"]);
    }
    schema
}

/// Level-1 catalog: one bounded entry per problem domain.
pub(crate) fn domain_tools(shared: bool) -> Vec<Tool> {
    DOMAINS
        .iter()
        .map(|d| {
            let schema = input_schema(shared);
            let mut tool = Tool::new(
                d.name,
                format!("{}{}", d.description, DISCOVERY_SUFFIX),
                schema.as_object().expect("object schema").clone(),
            );
            tool.annotations = Some(
                ToolAnnotations::new()
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(false),
            );
            tool
        })
        .collect()
}

/// Level-2 manifest: exact child names and their real schemas, returned only
/// when a caller explicitly discovers one domain.
pub(crate) fn manifest(domain: &str, flat_tools: &[Tool], shared: bool) -> Value {
    let def = DOMAINS
        .iter()
        .find(|d| d.name == domain)
        .expect("caller validated domain");
    let children: Vec<Value> = def
        .children
        .iter()
        .map(|name| {
            flat_tools
                .iter()
                .find(|t| t.name == *name)
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                        "read_only": t.annotations.as_ref().is_some_and(|a| a.read_only_hint.unwrap_or(false)),
                    })
                })
                .expect("every domain child exists in the flat catalog")
        })
        .collect();
    json!({
        "mode": "manifest",
        "domain": def.name,
        "manifest_version": MANIFEST_VERSION,
        "child_total": children.len(),
        "children": children,
        "limits": def.limits,
        "usage": if shared {
            "Call this domain again with project, child_tool, and arguments."
        } else {
            "Call this domain again with child_tool and arguments."
        },
    })
}
