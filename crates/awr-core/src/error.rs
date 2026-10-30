use crate::secrets::SensitiveCategory;
use serde::Serialize;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Workstream(#[from] crate::WorkstreamError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("source unavailable: {0}")]
    SourceUnavailable(String),
    #[error("source is stale: {0}")]
    SourceStale(String),
    #[error("source preflight rejected this mapping; inspect the source issues before applying")]
    IntakePreflightRejected {
        issues: serde_json::Value,
        intake_staged: bool,
    },
    #[error("source fingerprint conflict: {0}")]
    SourceConflict(String),
    #[error("revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error(
        "project revision conflict: expected {expected}, actual {actual}; checkpoint requires project_revision, not session.revision. Read session show or awr_session_get, review intervening changes and retry with the project revision (CLI: --expected-project-revision; MCP: expected_revision)"
    )]
    CheckpointRevisionConflict { expected: u64, actual: u64 },
    #[error("dependency blocked: {0}")]
    DependencyBlocked(String),
    #[error("claim conflict: {0}")]
    ClaimConflict(String),
    /// Governed work needs a current approved admission decision. `code` is the primary reason;
    /// `diagnostics` lists every reason as `{code, detail}`.
    #[error("admission required ({code}): {message}")]
    Admission {
        code: &'static str,
        message: String,
        diagnostics: serde_json::Value,
    },
    #[error("rule violation: {0}")]
    RuleViolation(String),
    #[error("rule violation: {message}")]
    SensitiveSource {
        message: String,
        location: DiagnosticLocation,
    },
    #[error("required evidence missing: {0}")]
    EvidenceMissing(String),
    #[error("required evidence missing: {}", .0.message)]
    EvidenceDiagnostic(Box<SourceDiagnostic>),
    #[error("mutation unsupported: {0}")]
    MutationUnsupported(String),
    #[error("proposal {proposal_id} requires manual handling: {reason}")]
    ProposalRequired {
        proposal_id: crate::Id,
        reason: String,
    },
    #[error(
        "mutation application {attempt_event_id} for proposal {proposal_id} needs recovery: {reason}"
    )]
    MutationIncomplete {
        proposal_id: crate::Id,
        attempt_event_id: crate::Id,
        reason: String,
    },
    #[error(
        "work action proposal {proposal_id} stopped during {stage}: {reason}; inspect `awr doctor` findings and, for an interrupted application, close it with `awr proposal recover {proposal_id} --actor <actor> --reason <reason> --expected-revision <current revision>` before retrying"
    )]
    WorkActionIncomplete {
        proposal_id: crate::Id,
        stage: String,
        reason: String,
    },
    #[error("mutation conflict: {0}")]
    MutationConflict(String),
    #[error("workspace conflict: {0}")]
    WorkspaceConflict(String),
    #[error("workspace contended: {0}")]
    WorkspaceContended(String),
    #[error(
        "checkpoint attempt {attempt_id} did not complete: {reason}; inspect session show before retrying"
    )]
    CheckpointIncomplete {
        attempt_id: crate::Id,
        reason: String,
    },
    #[error("context incomplete: {0}. {guidance}", guidance = crate::INCOMPLETE_CONTEXT_GUIDANCE)]
    ContextIncomplete(String),
    #[error("context budget exceeded: required {required}, budget {budget}")]
    BudgetExceeded { required: usize, budget: usize },
    #[error("invalid transition: {0}")]
    InvalidTransition(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid source: {}", .0.message)]
    InvalidSource(Box<SourceDiagnostic>),
    #[error("operation is not implemented: {0}")]
    Unsupported(String),
    #[error("host protocol version {requested} is not supported")]
    ProtocolUnsupported { requested: u32, supported: Vec<u32> },
    #[error("required host capabilities are unavailable")]
    CapabilityUnavailable {
        unknown: Vec<String>,
        unsupported: Vec<String>,
    },
    #[error("storage error: {0}")]
    Storage(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Serialize)]
pub struct ErrorReport {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// A source location is exact when supplied. Missing coordinates must not be guessed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiagnosticLocation {
    pub locator: Option<String>,
    pub pointer: Option<String>,
    /// One-based line and Unicode character column.
    pub line: Option<usize>,
    pub column: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceDiagnostic {
    pub message: String,
    pub location: DiagnosticLocation,
    pub rule: String,
    pub repair: String,
}

/// Render the same structured diagnostic carried by JSON and MCP responses.
pub fn render_diagnostic_details(details: Option<&serde_json::Value>) -> String {
    let Some(details) = details else {
        return String::new();
    };
    let mut output = String::new();
    if let Some(location) = details.get("location") {
        if let Some(locator) = location["locator"].as_str() {
            output.push_str(&format!("\n  Source: {locator}"));
            if let Some(line) = location["line"].as_u64() {
                output.push_str(&format!(":{line}"));
                if let Some(column) = location["column"].as_u64() {
                    output.push_str(&format!(":{column}"));
                }
            }
        }
        if let Some(pointer) = location["pointer"].as_str() {
            output.push_str(&format!("\n  Field: {pointer}"));
        }
    }
    for (key, label) in [("rule", "Rule"), ("repair", "Repair")] {
        if let Some(value) = details[key].as_str() {
            output.push_str(&format!("\n  {label}: {value}"));
        }
    }
    crate::safe_diagnostic(&output)
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Workstream(error) => error.code(),
            Self::NotFound(_) => "NotFound",
            Self::SourceUnavailable(_) => "SourceUnavailable",
            Self::SourceStale(_) => "SourceStale",
            Self::IntakePreflightRejected { .. } => "SourceStale",
            Self::SourceConflict(_) => "SourceConflict",
            Self::RevisionConflict { .. } | Self::CheckpointRevisionConflict { .. } => {
                "RevisionConflict"
            }
            Self::DependencyBlocked(_) => "DependencyBlocked",
            Self::ClaimConflict(_) => "ClaimConflict",
            Self::Admission { .. } => "AdmissionRequired",
            Self::RuleViolation(_) | Self::SensitiveSource { .. } => "RuleViolation",
            Self::EvidenceMissing(_) | Self::EvidenceDiagnostic(_) => "EvidenceMissing",
            Self::MutationUnsupported(_) => "MutationUnsupported",
            Self::ProposalRequired { .. } => "proposal_required",
            Self::MutationIncomplete { .. } => "MutationIncomplete",
            Self::WorkActionIncomplete { .. } => "WorkActionIncomplete",
            Self::MutationConflict(_) => "MutationConflict",
            Self::WorkspaceConflict(_) => "WorkspaceConflict",
            Self::WorkspaceContended(_) => "WorkspaceContended",
            Self::CheckpointIncomplete { .. } => "CheckpointIncomplete",
            Self::ContextIncomplete(_) => "ContextIncomplete",
            Self::BudgetExceeded { .. } => "BudgetExceeded",
            Self::InvalidTransition(_) => "InvalidTransition",
            Self::InvalidInput(_) => "InvalidInput",
            Self::InvalidSource(_) => "InvalidInput",
            Self::Unsupported(_) => "Unsupported",
            Self::ProtocolUnsupported { .. } => "ProtocolUnsupported",
            Self::CapabilityUnavailable { .. } => "CapabilityUnavailable",
            Self::Storage(_) => "Storage",
            Self::Io(_) => "Io",
            Self::Json(_) => "Json",
        }
    }
    pub fn report(&self) -> ErrorReport {
        ErrorReport {
            code: self.code(),
            message: crate::safe_diagnostic(&self.to_string()),
            details: (match self {
                Self::SensitiveSource { message, location } => {
                    let mut details = crate::secrets::sensitive_rejection_details(message).unwrap_or_default();
                    details["location"] = serde_json::json!(location);
                    details["rule"] = serde_json::json!("source.public_content");
                    details["repair"] = serde_json::json!(
                        crate::secrets::sensitive_category_for_message(message)
                            .map_or("Inspect the indicated source locally.", SensitiveCategory::repair)
                    );
                    Some(details)
                }
                Self::InvalidSource(diagnostic) | Self::EvidenceDiagnostic(diagnostic) => Some(serde_json::json!({
                    "location":diagnostic.location,"rule":diagnostic.rule,"repair":diagnostic.repair
                })),
                Self::IntakePreflightRejected { issues, intake_staged } => Some(serde_json::json!({
                    "can_apply":false,"source_issues":issues,"source_write_performed":intake_staged,
                    "configuration_write_performed":false,"runtime_write_performed":intake_staged,
                    "intake_staged":intake_staged
                })),
                Self::RuleViolation(message) => crate::secrets::sensitive_rejection_details(message),
                Self::Admission { code, diagnostics, .. } => Some(serde_json::json!({
                    "reason": code, "diagnostics": diagnostics,
                    "next_action": "Read work prepare for the admission state, repair the listed reasons, then request a decision with work admit; recovery operations stay available."
                })),
                Self::RevisionConflict { expected, actual } => {
                    Some(serde_json::json!({"expected": expected, "actual": actual}))
                }
                Self::CheckpointRevisionConflict { expected, actual } => Some(serde_json::json!({
                    "expected": expected, "actual": actual, "revision_scope": "project",
                    "next_action": "Read project_revision from session show or awr_session_get, review intervening changes and retry with the project revision (CLI: --expected-project-revision; MCP: expected_revision); session.revision is not the write precondition."
                })),
                Self::ContextIncomplete(_) => Some(serde_json::json!({
                    "next_action": crate::INCOMPLETE_CONTEXT_GUIDANCE,
                    "incomplete_checkpoint_allowed": true, "checkpoint_updates_source_work": false
                })),
                Self::BudgetExceeded { required, budget } => {
                    Some(serde_json::json!({"required": required, "budget": budget}))
                }
                Self::ProtocolUnsupported { requested, supported } => Some(serde_json::json!({
                    "requested": requested, "supported": supported,
                    "source_write_performed": false, "runtime_write_performed": false
                })),
                Self::CapabilityUnavailable { unknown, unsupported } => Some(serde_json::json!({
                    "unknown": unknown, "unsupported": unsupported,
                    "source_write_performed": false, "runtime_write_performed": false
                })),
                Self::CheckpointIncomplete { attempt_id, reason } => {
                    Some(serde_json::json!({"attempt_id":attempt_id,"reason":reason}))
                }
                Self::ProposalRequired {
                    proposal_id,
                    reason,
                } => Some(
                    serde_json::json!({"proposal_id":proposal_id,"reason":reason,"source_write_performed":false}),
                ),
                Self::MutationIncomplete {
                    proposal_id,
                    attempt_event_id,
                    reason,
                } => Some(
                    serde_json::json!({"proposal_id":proposal_id,"attempt_event_id":attempt_event_id,"reason":reason}),
                ),
                Self::WorkActionIncomplete {
                    proposal_id,
                    stage,
                    reason,
                } => Some(
                    serde_json::json!({"proposal_id":proposal_id,"stage":stage,"reason":reason,"source_write_performed":null}),
                ),
                _ => None,
            }).map(crate::redact_sensitive_value),
        }
    }

    /// Add checkpoint-specific guidance without changing the public conflict code.
    pub fn for_checkpoint(self) -> Self {
        match self {
            Self::RevisionConflict { expected, actual } => {
                Self::CheckpointRevisionConflict { expected, actual }
            }
            other => other,
        }
    }
}
