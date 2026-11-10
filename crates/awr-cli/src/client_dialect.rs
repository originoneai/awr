//! Native envelopes are translated here; decisions use only AWR state.
use awr_core::{CompactionObservation, CompactionPolicy, Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    SessionStart,
    PreCompact,
    PostCompact,
    PreToolUse,
    Stop,
    SessionEnd,
    Interrupt,
}
impl LifecycleEvent {
    pub fn name(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
            Self::PreToolUse => "PreToolUse",
            Self::Stop => "Stop",
            Self::SessionEnd => "SessionEnd",
            Self::Interrupt => "Interrupt",
        }
    }
}

#[derive(Debug)]
pub struct HookEvent {
    pub native_session: String,
    pub event: LifecycleEvent,
    pub native_event: String,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub turn_id: Option<String>,
    pub trigger: Option<String>,
    pub stop_hook_active: bool,
    // Preserve absence for the existing lifecycle delivery-key contract.
    pub reported_stop_hook_active: Option<bool>,
    pub tool: Option<ToolUse>,
    pub compaction: Option<CompactionInput>,
}
#[derive(Debug)]
pub struct ToolUse {
    pub name: String,
    pub paths: Vec<PathBuf>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionInput {
    pub observation: CompactionObservation,
    #[serde(default)]
    pub policy: CompactionPolicy,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecision {
    Continue { context: Option<String> },
    Ask { reason: String },
    Deny { reason: String },
    BlockStop { reason: String },
}
impl HookDecision {
    pub fn allow() -> Self {
        Self::Continue { context: None }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Continue { .. } => "continue",
            Self::Ask { .. } => "ask",
            Self::Deny { .. } => "deny",
            Self::BlockStop { .. } => "block_stop",
        }
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Continue { .. } => None,
            Self::Ask { reason } | Self::Deny { reason } | Self::BlockStop { reason } => {
                Some(reason)
            }
        }
    }
}
pub struct Rendered {
    pub stdout: Value,
    pub exit_code: u8,
}
pub trait HookDialect {
    fn parse(&self, stdin: &[u8]) -> Result<HookEvent>;
    fn render(&self, decision: &HookDecision, event: &HookEvent) -> Rendered;
    fn write_tools(&self) -> &'static [&'static str];
    fn supports(&self, decision: &HookDecision) -> bool;
}
pub struct CodexDialect;
pub struct KimiDialect;
pub struct GenericDialect;
pub struct ClaudeDialect;

pub fn dialect(client: &str) -> Result<Box<dyn HookDialect>> {
    match client {
        "codex" => Ok(Box::new(CodexDialect)),
        "kimi" => Ok(Box::new(KimiDialect)),
        "generic" => Ok(Box::new(GenericDialect)),
        "claude" => Ok(Box::new(ClaudeDialect)),
        _ => Err(Error::InvalidInput(
            "client must be generic, codex, kimi or claude".into(),
        )),
    }
}
fn parse(stdin: &[u8], claude: bool) -> Result<HookEvent> {
    if stdin.len() > 1024 * 1024 {
        return Err(Error::InvalidInput("client event exceeds 1 MiB".into()));
    }
    let input: Value = serde_json::from_slice(stdin)
        .map_err(|_| Error::InvalidInput("invalid client event JSON".into()))?;
    let field = |name: &str| {
        input
            .get(name)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let external = field("session_id")
        .ok_or_else(|| Error::InvalidInput("client event requires session_id".into()))?;
    if external.trim().is_empty() || external.len() > 512 {
        return Err(Error::InvalidInput("invalid native session ID".into()));
    }
    let native_event = field("hook_event_name").unwrap_or_default();
    let event = match native_event.as_str() {
        "SessionStart" if claude && input["source"] == "compact" => LifecycleEvent::PostCompact,
        "SessionStart" => LifecycleEvent::SessionStart,
        "PreCompact" => LifecycleEvent::PreCompact,
        "PostCompact" => LifecycleEvent::PostCompact,
        "PreToolUse" => LifecycleEvent::PreToolUse,
        "Stop" => LifecycleEvent::Stop,
        "SessionEnd" => LifecycleEvent::SessionEnd,
        "Interrupt" if !claude => LifecycleEvent::Interrupt,
        _ => {
            return Err(Error::Unsupported(
                "unsupported client lifecycle event".into(),
            ));
        }
    };
    let compaction = input
        .get("awr_compaction")
        .map(|value| {
            serde_json::from_value::<CompactionInput>(value.clone())
                .map_err(|_| Error::InvalidInput("invalid awr_compaction host envelope".into()))
        })
        .transpose()?;
    if let Some(c) = &compaction {
        if event != LifecycleEvent::PostCompact {
            return Err(Error::InvalidInput(
                "awr_compaction is valid only after PostCompact".into(),
            ));
        }
        c.observation.validate(awr_core::now_millis()?)?;
        c.policy.validate()?;
    }
    let tool = if event == LifecycleEvent::PreToolUse {
        let value = if claude {
            &input["tool_input"]
        } else {
            &input["tool"]
        };
        let name = if claude {
            field("tool_name")
        } else {
            value["name"].as_str().map(str::to_owned)
        };
        name.map(|name| {
            let mut paths = Vec::new();
            for key in ["file_path", "notebook_path", "path"] {
                if let Some(path) = value[key].as_str().filter(|p| !p.is_empty()) {
                    paths.push(PathBuf::from(path));
                }
            }
            if !claude {
                for path in value["paths"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    paths.push(PathBuf::from(path));
                }
            }
            ToolUse { name, paths }
        })
    } else {
        None
    };
    // Transcript paths and assistant/tool message bodies are never opened or retained.
    Ok(HookEvent {
        native_session: external,
        event,
        native_event,
        cwd: field("cwd")
            .map(PathBuf::from)
            .ok_or_else(|| Error::InvalidInput("client event requires cwd".into()))?,
        model: field("model"),
        turn_id: field("turn_id"),
        trigger: field("trigger"),
        stop_hook_active: input["stop_hook_active"].as_bool().unwrap_or(false),
        reported_stop_hook_active: input["stop_hook_active"].as_bool(),
        tool,
        compaction,
    })
}
fn legacy(decision: &HookDecision, event: &HookEvent) -> Rendered {
    let mut stdout = json!({"continue":true});
    if let HookDecision::Continue {
        context: Some(context),
    } = decision
    {
        stdout["hookSpecificOutput"] =
            json!({"hookEventName":event.event.name(),"additionalContext":context});
    } else if !matches!(decision, HookDecision::Continue { .. }) {
        stdout["awr"] = json!({"unsupported_decision":decision.name()});
    }
    Rendered {
        stdout,
        exit_code: 0,
    }
}
macro_rules! legacy_dialect {
    ($name:ty) => {
        impl HookDialect for $name {
            fn parse(&self, stdin: &[u8]) -> Result<HookEvent> {
                parse(stdin, false)
            }
            fn render(&self, decision: &HookDecision, event: &HookEvent) -> Rendered {
                legacy(decision, event)
            }
            fn write_tools(&self) -> &'static [&'static str] {
                &["apply_patch", "Edit", "Write"]
            }
            fn supports(&self, decision: &HookDecision) -> bool {
                matches!(decision, HookDecision::Continue { .. })
            }
        }
    };
}
legacy_dialect!(CodexDialect);
legacy_dialect!(KimiDialect);
impl HookDialect for GenericDialect {
    fn parse(&self, stdin: &[u8]) -> Result<HookEvent> {
        parse(stdin, false)
    }
    fn render(&self, decision: &HookDecision, event: &HookEvent) -> Rendered {
        let mut rendered = legacy(decision, event);
        rendered.stdout.as_object_mut().unwrap().remove("awr");
        rendered.stdout["continue"] = json!(matches!(decision, HookDecision::Continue { .. }));
        rendered.stdout["decision"] = json!(decision.name());
        rendered.stdout["reason"] = json!(decision.reason());
        rendered.stdout["context"] = match decision {
            HookDecision::Continue { context } => json!(context),
            _ => Value::Null,
        };
        rendered
    }
    fn write_tools(&self) -> &'static [&'static str] {
        &["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"]
    }
    fn supports(&self, _: &HookDecision) -> bool {
        true
    }
}
impl HookDialect for ClaudeDialect {
    fn parse(&self, stdin: &[u8]) -> Result<HookEvent> {
        parse(stdin, true)
    }
    fn render(&self, decision: &HookDecision, event: &HookEvent) -> Rendered {
        let stdout = match (event.native_event.as_str(), decision) {
            (
                "SessionStart",
                HookDecision::Continue {
                    context: Some(context),
                },
            ) => {
                json!({"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":context}})
            }
            ("PreToolUse", HookDecision::Ask { reason } | HookDecision::Deny { reason }) => {
                json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":decision.name(),"permissionDecisionReason":reason}})
            }
            ("Stop", HookDecision::BlockStop { reason }) => {
                json!({"decision":"block","reason":reason})
            }
            // Omitting a permission decision preserves the host's own approval checks.
            _ => json!({}),
        };
        Rendered {
            stdout,
            exit_code: 0,
        }
    }
    fn write_tools(&self) -> &'static [&'static str] {
        &["Edit", "Write", "MultiEdit", "NotebookEdit"]
    }
    fn supports(&self, _: &HookDecision) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn criterion_5_unsupported_legacy_decisions_continue_with_metadata() {
        for client in ["codex", "kimi"] {
            let dialect = dialect(client).unwrap();
            let event = dialect
                .parse(br#"{"session_id":"s","hook_event_name":"PreToolUse","cwd":"."}"#)
                .unwrap();
            for decision in [
                HookDecision::Ask {
                    reason: "Claim work".into(),
                },
                HookDecision::Deny {
                    reason: "Claim work".into(),
                },
                HookDecision::BlockStop {
                    reason: "Complete work".into(),
                },
            ] {
                let output = dialect.render(&decision, &event);
                assert_eq!(output.stdout["continue"], true);
                assert_eq!(
                    output.stdout["awr"]["unsupported_decision"],
                    decision.name()
                );
                assert_eq!(output.exit_code, 0);
            }
        }
    }
    #[test]
    fn claude_continue_does_not_approve_native_permissions() {
        let event = ClaudeDialect
            .parse(br#"{"session_id":"s","hook_event_name":"PreToolUse","cwd":"."}"#)
            .unwrap();
        assert_eq!(
            ClaudeDialect.render(&HookDecision::allow(), &event).stdout,
            json!({})
        );
    }
}
