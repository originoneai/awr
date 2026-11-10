//! Named coding-agent host adapters and controlled subtask parallelism (WS-024).
//!
//! Adapters declare process-adjacent capabilities separately from AWR admission,
//! cancel, and session-end. Unsupported capabilities route to human continuation.
//! Subtasks keep independent identity, claims, and resource bounds; parents only
//! retain outcome references.

mod claude_code;
mod codex_cli;
pub mod installation;
mod l0_manual;
mod parallelism;
mod registry;

pub use claude_code::ClaudeCodeAdapter;
pub use codex_cli::CodexCliAdapter;
pub use l0_manual::L0ManualAdapter;
pub use parallelism::{
    ParallelDispatchPlan, ParallelScheduler, ParentExitEffect, PauseGate, ReconnectRetry,
    ScheduleDecision, SubtaskRecord, SubtaskState, rollup_refs,
};
pub use registry::{AdapterRegistry, built_in_registry};

use awr_core::{
    AdapterCapability, AdapterCapabilityMatrix, AdapterId, AdapterNegotiationRequest,
    AdapterNegotiationResult, ExternalExecutionReport, Result, negotiate_adapter,
};
use serde::{Deserialize, Serialize};

/// Outcome of an adapter operation that may require human continuation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterActionOutcome {
    Supported {
        detail: String,
    },
    HumanContinuation {
        missing: AdapterCapability,
        instruction: String,
    },
    /// Coordination-only acknowledgement — never means a process was started/killed.
    CoordinationOnly {
        fact: String,
        note: String,
    },
}

/// Handle returned when an adapter starts or reconnects to a native execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeExecutionHandle {
    pub adapter_id: AdapterId,
    pub execution_id: String,
    pub native_session: String,
    pub operation_key: String,
}

/// Status snapshot from an adapter (or L0 report projection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterStatus {
    pub execution_id: String,
    pub phase: String,
    pub verified: bool,
    pub basis: String,
    pub summary: String,
}

/// Result forensics collected without re-running the operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterForensics {
    pub execution_id: String,
    pub references: Vec<String>,
    pub summary: String,
}

/// Trait implemented by named controlled adapters and the L0 manual reporter.
pub trait ExecutionHostAdapter: Send + Sync {
    fn matrix(&self) -> &AdapterCapabilityMatrix;

    fn adapter_id(&self) -> &AdapterId {
        &self.matrix().adapter_id
    }

    fn negotiate(&self, request: &AdapterNegotiationRequest) -> AdapterNegotiationResult {
        negotiate_adapter(Some(self.matrix()), request)
    }

    /// Attempt to start a native coding-agent client. Must not be confused with
    /// AWR admission.
    fn start(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome>;

    fn status(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome>;

    fn confirm_stop(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome>;

    fn reconnect_resume(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome>;

    fn result_forensics(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome>;

    /// Record an L0/client external report. Named adapters may accept this as
    /// supplemental observation without treating it as process control.
    fn accept_l0_report(&self, _report: &ExternalExecutionReport) -> Result<AdapterActionOutcome> {
        Ok(AdapterActionOutcome::HumanContinuation {
            missing: AdapterCapability::StatusRead,
            instruction: self.matrix().human_continuation.clone(),
        })
    }
}

fn unsupported(
    matrix: &AdapterCapabilityMatrix,
    missing: AdapterCapability,
) -> AdapterActionOutcome {
    AdapterActionOutcome::HumanContinuation {
        missing,
        instruction: matrix.human_continuation.clone(),
    }
}

fn supported(detail: impl Into<String>) -> AdapterActionOutcome {
    AdapterActionOutcome::Supported {
        detail: detail.into(),
    }
}

/// Explicitly refuse to treat AWR coordination as process control.
pub fn refuse_coordination_as_process_control(fact: &str) -> AdapterActionOutcome {
    AdapterActionOutcome::CoordinationOnly {
        fact: fact.into(),
        note: "AWR admission, cancel request, and session end are coordination facts; they do not start or kill a coding-agent process.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::{AdapterCapability, AdapterControlMode, NegotiationDecision};
    use std::collections::BTreeSet;

    #[test]
    fn registry_exposes_two_named_clients_and_l0() {
        let reg = built_in_registry();
        assert!(reg.get("codex_cli").is_some());
        assert!(reg.get("claude_code").is_some());
        assert!(reg.get("manual_report").is_some());
        assert_eq!(reg.named_controlled().len(), 2);
    }

    #[test]
    fn codex_negotiates_as_usable_for_status_without_claiming_start() {
        let reg = built_in_registry();
        let adapter = reg.get("codex_cli").unwrap();
        assert!(!adapter.matrix().auto_startable);
        let result = adapter.negotiate(&AdapterNegotiationRequest {
            adapter_id: AdapterId::new("codex_cli").unwrap(),
            required: BTreeSet::from([AdapterCapability::StatusRead]),
            optional: BTreeSet::from([AdapterCapability::Start]),
        });
        assert_eq!(result.decision, NegotiationDecision::Usable);
        assert!(!result.granted.contains(&AdapterCapability::Start));
        assert!(result.missing_optional.contains(&AdapterCapability::Start));
        let start = adapter.start(&NativeExecutionHandle {
            adapter_id: AdapterId::new("codex_cli").unwrap(),
            execution_id: "e-codex".into(),
            native_session: "codex:1".into(),
            operation_key: "op-1".into(),
        });
        assert!(matches!(
            start.unwrap(),
            AdapterActionOutcome::HumanContinuation {
                missing: AdapterCapability::Start,
                ..
            }
        ));
        let status = CodexCliAdapter::new().observed_status("missing");
        assert!(status.is_none());
    }

    #[test]
    fn claude_without_start_still_usable_for_status() {
        let reg = built_in_registry();
        let adapter = reg.get("claude_code").unwrap();
        assert!(!adapter.matrix().auto_startable);
        assert_eq!(
            adapter.matrix().control_mode,
            AdapterControlMode::NamedControlled
        );
        let start = adapter.start(&NativeExecutionHandle {
            adapter_id: AdapterId::new("claude_code").unwrap(),
            execution_id: "e1".into(),
            native_session: "claude:1".into(),
            operation_key: "op-1".into(),
        });
        assert!(matches!(
            start.unwrap(),
            AdapterActionOutcome::HumanContinuation {
                missing: AdapterCapability::Start,
                ..
            }
        ));
    }

    #[test]
    fn admission_is_not_process_start() {
        let outcome = refuse_coordination_as_process_control("awr_admission");
        assert!(matches!(
            outcome,
            AdapterActionOutcome::CoordinationOnly { .. }
        ));
    }
}
