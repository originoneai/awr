use crate::{Id, Revision};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    ManagedLocal,
    External,
}

/// Claimed provenance of an external report; neither value grants supervisor authority.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalReportOrigin {
    CallerReported,
    HostObserved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalReportPhase {
    Started,
    Progress,
    WaitingUser,
    Succeeded,
    Failed,
    Interrupted,
    Unknown,
}

/// Immutable host observation. Reports are separate from AWR's execution state machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalExecutionReport {
    pub version: u32,
    pub request_key: String,
    pub execution_id: Id,
    pub host_id: String,
    pub host_work_key: String,
    pub native_session: String,
    pub agent_id: String,
    pub origin: ExternalReportOrigin,
    pub phase: ExternalReportPhase,
    pub observed_at: i64,
    pub summary: String,
    #[serde(default)]
    pub detail_references: Vec<String>,
}
impl ExternalExecutionReport {
    pub fn validate(&self) -> crate::Result<()> {
        if self.version != 1
            || self.observed_at < 0
            || [
                &self.request_key,
                &self.host_id,
                &self.host_work_key,
                &self.native_session,
                &self.agent_id,
            ]
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 512 || s.contains('\0'))
            || self.summary.trim().is_empty()
            || self.summary.len() > 8192
            || self.detail_references.len() > 32
            || self
                .detail_references
                .iter()
                .any(|s| s.trim().is_empty() || s.len() > 4096 || s.contains('\0'))
        {
            return Err(crate::Error::InvalidInput("external report requires version 1, bounded identities, timestamp, summary and references".into()));
        }
        crate::ensure_public_data(self)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Registered,
    Starting,
    Running,
    Succeeded,
    Failed,
}
impl ExecutionState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

/// How an input file entered the receipt: named with `--bind`, or detected from argv.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    Bind,
    ArgvScript,
}
/// What the receipt can say about the bytes that were executed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputBinding {
    Declared,
    ArgvScript,
    /// No input could be named or hashed; receipts written before inputs existed decode to this.
    #[default]
    None,
}
/// One input file bound to the receipt, hashed when the execution was registered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionInput {
    /// The path as the caller wrote it; it is an identifier, not a promise that another node has the file.
    pub path: String,
    /// Lowercase hex, the same bytes-to-hex form the source locators use.
    pub sha256: String,
    pub size: u64,
    /// Diagnostic only: excluded from intent equality so that a retry after `touch`
    /// still matches the stored intent, while different bytes do not.
    pub mtime_ns: i64,
    pub source: InputSource,
}
/// Lexical spelling of a path for intent equality only: separators are unified and
/// empty / `.` components are dropped. No filesystem access, no `..` resolution and
/// no case folding, so two strings that name different files never compare equal.
fn normalized_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            other => parts.push(other),
        }
    }
    parts.join("/")
}
impl PartialEq for ExecutionInput {
    fn eq(&self, other: &Self) -> bool {
        normalized_path(&self.path) == normalized_path(&other.path)
            && self.sha256 == other.sha256
            && self.size == other.size
            && self.source == other.source
    }
}
impl Eq for ExecutionInput {}
/// Immutable intent. An operation key is project-wide, including across session handoffs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIntent {
    pub operation_key: String,
    pub purpose: String,
    pub executor: ExecutorKind,
    pub command: Vec<String>,
    pub cwd: String,
    pub external_reference: Option<String>,
    #[serde(default)]
    pub inputs: Vec<ExecutionInput>,
    #[serde(default)]
    pub input_binding: InputBinding,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    pub nonce: Id,
    pub pid: u32,
    pub port: u16,
    pub child_pid: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub id: Id,
    pub project_id: Id,
    pub work_item_id: Id,
    pub session_id: Id,
    pub branch_id: Option<Id>,
    pub revision: Revision,
    pub intent: ExecutionIntent,
    pub state: ExecutionState,
    pub worker: Option<WorkerIdentity>,
    pub registered_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub error: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub receipt: Option<String>,
}

/// Written by the owned supervisor after wait/try_wait observes its direct child exit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionResult {
    pub execution_id: Id,
    pub nonce: Id,
    pub finished_at: i64,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProbe {
    pub execution_id: Id,
    pub nonce: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProbeReply {
    pub execution_id: Id,
    pub nonce: Id,
    pub worker_pid: u32,
    pub child_pid: u32,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObservedExecutionState {
    Running,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionObservation {
    pub execution_id: Id,
    pub operation_key: String,
    pub purpose: String,
    pub origin_session_id: Id,
    pub branch_id: Option<Id>,
    pub recorded_revision: Revision,
    pub recorded_state: ExecutionState,
    pub state: ObservedExecutionState,
    pub verified: bool,
    pub observed_at: i64,
    pub evidence_at: Option<i64>,
    pub basis: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub error: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub receipt: Option<String>,
    pub next_action: String,
}
impl Execution {
    /// Deterministic journal facts for hashed L0/L1 context; live probes are separate observations.
    pub fn continuity_text(&self) -> crate::Result<String> {
        Ok(format!(
            "Execution: {} | operation: {} | purpose: {}\nOrigin session: {} | branch: {} | revision: {}\nRecorded state: {} | started: {:?} | finished: {:?} | exit: {:?} | signal: {:?}\nLogs: {}; {} | receipt: {}\n{}",
            self.id,
            serde_json::to_string(&self.intent.operation_key)?,
            serde_json::to_string(&self.intent.purpose)?,
            self.session_id,
            self.branch_id
                .map(|v| v.to_string())
                .unwrap_or_else(|| "main".into()),
            self.revision,
            serde_json::to_string(&self.state)?,
            self.started_at,
            self.finished_at,
            self.exit_code,
            self.signal,
            self.stdout.as_deref().unwrap_or("none"),
            self.stderr.as_deref().unwrap_or("none"),
            self.receipt.as_deref().unwrap_or("none"),
            if self.state.terminal() {
                "A supervisor completion is recorded; read the result and logs before advancing work."
            } else {
                "Current outcome is unverified by context compilation. Use awr execution inspect before acting; do not automatically retry or kill this operation."
            }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(path: &str, sha256: &str, mtime_ns: i64) -> ExecutionInput {
        ExecutionInput {
            path: path.into(),
            sha256: sha256.into(),
            size: 3,
            mtime_ns,
            source: InputSource::Bind,
        }
    }

    #[test]
    fn a_receipt_written_before_inputs_existed_decodes_with_no_binding() {
        let legacy = r#"{"operation_key":"k","purpose":"p","executor":"managed_local","command":["true"],"cwd":"/tmp","external_reference":null}"#;
        let intent: ExecutionIntent = serde_json::from_str(legacy).unwrap();
        assert!(intent.inputs.is_empty());
        assert_eq!(intent.input_binding, InputBinding::None);
        assert_eq!(
            serde_json::to_value(intent.input_binding).unwrap(),
            json!("none")
        );
    }

    #[test]
    fn input_equality_ignores_mtime_and_path_spelling() {
        assert_eq!(input("a.py", "x", 1), input("a.py", "x", 2));
        assert_eq!(input("./a.py", "x", 1), input("a.py", "x", 1));
        assert_eq!(input("dir//a.py", "x", 1), input("dir/a.py", "x", 1));
        assert_ne!(input("a.py", "x", 1), input("a.py", "y", 1));
        assert_ne!(input("a.py", "x", 1), input("dir/a.py", "x", 1));
        assert_ne!(input("a.py", "x", 1), input("b.py", "x", 1));
    }

    #[test]
    fn binding_and_source_variants_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_value(InputBinding::Declared).unwrap(),
            json!("declared")
        );
        assert_eq!(
            serde_json::to_value(InputBinding::ArgvScript).unwrap(),
            json!("argv_script")
        );
        assert_eq!(
            serde_json::to_value(InputBinding::None).unwrap(),
            json!("none")
        );
        assert_eq!(
            serde_json::to_value(InputSource::ArgvScript).unwrap(),
            json!("argv_script")
        );
        assert_eq!(
            serde_json::to_value(InputSource::Bind).unwrap(),
            json!("bind")
        );
    }
}
