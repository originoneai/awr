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
}

const DISCOVERY_SUFFIX: &str =
    " Empty call returns child schemas; then pass child_tool and arguments.";

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
    },
    DomainDef {
        name: "awr_work",
        description: "Apply reviewed work transitions, manage progress, and read dependency graphs.",
        children: &["awr_work_transition", "awr_work_manage", "awr_work_graph"],
    },
    DomainDef {
        name: "awr_evidence",
        description: "Append bounded generic events and record evidence bindings.",
        children: &["awr_event_append", "awr_evidence_record"],
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
    },
    DomainDef {
        name: "awr_compaction",
        description: "Observe native compaction telemetry, read assessments, or defer advice.",
        children: &[
            "awr_compaction_observe",
            "awr_compaction_get",
            "awr_compaction_defer",
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
        "usage": if shared {
            "Call this domain again with project, child_tool, and arguments."
        } else {
            "Call this domain again with child_tool and arguments."
        },
    })
}
