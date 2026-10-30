use thiserror::Error;

pub type PgResult<T> = Result<T, PgError>;

#[derive(Debug, Error)]
pub enum PgError {
    #[error(transparent)]
    Workstream(#[from] awr_core::WorkstreamError),
    #[error("schema incompatible: {0}")]
    SchemaIncompatible(String),
    #[error("idempotency conflict")]
    IdempotencyConflict,
    #[error("project not available")]
    ProjectNotAvailable,
    #[error("unsafe source path: {0}")]
    UnsafeSourcePath(String),
    #[error("file is not valid UTF-8: {0}")]
    InvalidUtf8(String),
    #[error("snapshot content drifted from its recorded digest: {0}")]
    SnapshotDrift(String),
    #[error("stale or unbound approval")]
    StaleApproval,
    #[error("authority epoch mismatch")]
    EpochMismatch,
    #[error("parser version mismatch")]
    ParserMismatch,
    #[error("candidate is not approved")]
    CandidateNotApproved,
    #[error("author cannot approve their own candidate")]
    AuthorCannotApprove,
    #[error("candidate is not the active source")]
    InactiveCandidate,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("event cursor expired")]
    CursorExpired,
    #[error("coordinator epoch changed")]
    EpochChanged,
    #[error("session not found")]
    SessionNotFound,
    #[error("claim held")]
    ClaimHeld,
    #[error("lease expired")]
    LeaseExpired,
    #[error("recovery blocked")]
    RecoveryBlocked,
    #[error("open wait blocks progress")]
    WaitOpen,
    #[error("forbidden")]
    Forbidden,
    #[error("stale fence")]
    StaleFence,
    /// Directed closed path (first == last) explaining the hard cycle.
    #[error("dependency cycle: {}", .0.join(" -> "))]
    DependencyCycle(Vec<String>),
    #[error("missing required dependency")]
    MissingDependency,
    #[error("resource conflict")]
    ResourceConflict,
    #[error("scope unsupported")]
    ScopeUnsupported,
    #[error("parent evidence required")]
    ParentEvidenceRequired,
    #[error("dependency binding invalid")]
    BindingInvalid,
    #[error("action blocked by selective invalidation or planning change: {0}")]
    ActionBlockedByInvalidation(String),
    #[error("planning change pending confirmation")]
    PlanningChangePending,
    #[error("claimed work blocks activation")]
    ClaimBlocksActivation,
    #[error("planning writeback refused: {0}")]
    WritebackRefused(String),
    #[error("planning activation impact unproven: {0}")]
    ActivationImpactUnproven(String),
    #[error("graph budget exceeded")]
    GraphBudgetExceeded,
    #[error("execution not found")]
    ExecutionNotFound,
    #[error("scope exceeded")]
    ScopeExceeded,
    #[error("exactly-once unsupported")]
    ExactlyOnceUnsupported,
    #[error("evidence invalid")]
    EvidenceInvalid,
    #[error("review required")]
    ReviewRequired,
    #[error("rework requires a returned review")]
    ReworkRequiresReturnedReview,
    #[error("review unavailable in selected work")]
    ReviewUnavailable,
    #[error("author cannot review")]
    AuthorCannotReview,
    #[error("completion rejected")]
    CompletionRejected,
    #[error("policy downgrade")]
    PolicyDowngrade,
    /// Governed work needs a current admission decision. `code` is one of the closed `admission_*`
    /// codes of the Core evaluator and `detail` is the reason it gives.
    #[error("admission required ({code}): {detail}")]
    AdmissionRequired { code: &'static str, detail: String },
    /// A review decision or a completion contradicts the findings of the work: a return without a
    /// blocking finding, an approval while blocking findings are open, or a completion with any.
    #[error("review findings block this: {detail}")]
    FindingsBlock { detail: String },
    #[error("context incomplete")]
    ContextIncomplete,
    #[error("serialized response exceeds service limit")]
    ResponseTooLarge,
    #[error("command preconditions changed; refresh the selected work or session")]
    PreconditionsChanged,
    #[error("source divergence")]
    SourceDivergence,
    #[error("restore incomplete")]
    RestoreIncomplete,
    #[error("outbox replay forbidden")]
    OutboxReplayForbidden,
    #[error("rollback forbidden")]
    RollbackForbidden,
    #[error("{0}")]
    Db(#[from] tokio_postgres::Error),
    #[error("connection pool: {0}")]
    Pool(#[from] deadpool_postgres::PoolError),
    #[error("{0}")]
    Protocol(String),
}

impl PgError {
    /// Validation records only fixed field/rule identifiers. Never store caller
    /// values here or forward arbitrary Protocol text to a transport.
    pub(crate) fn query_field(field: &'static str, rule: &'static str) -> Self {
        Self::Protocol(format!("query_input/{field}/{rule}"))
    }

    /// Bounded diagnostics from the query validator, shared by HTTP and MCP.
    /// Preserve the public enum for downstream exhaustive matches. Both parts
    /// are allowlisted again so an unrelated Protocol error cannot echo input.
    pub fn query_input_diagnostic(&self) -> Option<serde_json::Value> {
        let Self::Protocol(message) = self else {
            return None;
        };
        let (field, rule) = message.strip_prefix("query_input/")?.split_once('/')?;
        let field = [
            "workstream_id",
            "work_id",
            "session_id",
            "request_id",
            "claim_id",
            "execution_id",
            "handoff_id",
            "evidence_id",
            "review_round_id",
            "artifact_id",
            "export_id",
            "change_id",
            "member_actor_id",
            "category",
            "include_denies",
            "limit",
            "cursor",
            "search",
            "max_context_bytes",
            "source_path",
            "expected_sha256",
        ]
        .into_iter()
        .find(|known| *known == field)?;
        let (constraint, correction) = match rule {
            "unsupported" => (
                "unsupported_for_operation",
                "Omit this field for the selected query operation.",
            ),
            "required" => (
                "required",
                "Supply this field using the selected operation's discovered input schema.",
            ),
            "selector" => (
                "selector_format",
                "Use a nonempty selector of at most 128 UTF-8 bytes, without control characters.",
            ),
            "limit" => (
                "bounds",
                "Use an integer from 1 through 100, or omit this optional field.",
            ),
            "cursor" => (
                "max_bytes",
                "Use the returned cursor unchanged, at most 4096 UTF-8 bytes; omit it to start a new read.",
            ),
            "context" => (
                "bounds",
                "Use an integer from 1 through 262144 bytes, or omit this optional budget.",
            ),
            "segment_context" => (
                "bounds",
                "Use an integer from 1 through 4096 raw bytes per segment, or omit the budget for 2048 bytes.",
            ),
            "artifact_context" => (
                "bounds",
                "Use an integer from 1 through 1048576 bytes, or omit this optional budget.",
            ),
            "search" => (
                "search_format",
                "Use a nonempty search of at most 512 UTF-8 bytes, without control characters.",
            ),
            "required_digest" => (
                "sha256_format",
                "Supply exactly 64 lowercase hexadecimal characters from the current authorized artifact reference; artifact.segment requires this digest.",
            ),
            "digest" => (
                "sha256_format",
                "Use exactly 64 lowercase hexadecimal characters for the expected SHA-256, or omit it.",
            ),
            "source_path" => (
                "relative_path",
                "Use a registered source path relative to the active snapshot, at most 512 UTF-8 bytes; no absolute path, URL, '..', backslash, colon or control characters.",
            ),
            "source_unavailable" => (
                "registered_source_required",
                "Use a registered source reference from work.prepare. For repository delivery files, follow an authorized artifact.content query from the delivery or review manifest; source.content does not read arbitrary repository files.",
            ),
            "artifact_selection" => (
                "exactly_one",
                "Supply exactly one of artifact_id or export_id from the authorized artifact reference.",
            ),
            "work_selection" => (
                "required_selection",
                "Supply work_id or session_id from an authorized work or session reference.",
            ),
            _ => return None,
        };
        Some(serde_json::json!({
            "code":"InvalidInput", "message":"query field violates the selected operation's contract",
            "invalid_field":format!("/{field}"), "constraint":constraint,
            "action_guidance":{
                "condition":"This read query was rejected.",
                "basis":"The query validator requires the indicated field constraint.",
                "next_action":correction,
                "recheck":"After correcting the query inputs. Current permission and state checks still apply; this advice does not authorize execution or command replay."
            }
        }))
    }

    const EXPIRED_AGENT_AUTHORIZATION: &'static str = "authenticated Agent read delegation expired";

    /// A safe discovery diagnosis, never an authentication or execution permit.
    /// Keep the public enum stable for downstream exhaustive matches.
    pub(crate) fn expired_agent_authorization() -> Self {
        Self::Protocol(Self::EXPIRED_AGENT_AUTHORIZATION.into())
    }

    pub fn is_expired_agent_authorization(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::EXPIRED_AGENT_AUTHORIZATION)
    }

    // Keep the existing Protocol representation: PgError is a public,
    // exhaustively matched enum, and callers already classify it as bad input.
    const SOURCE_STORAGE_IO: &'static str = "authoritative source storage is unavailable";
    const SOURCE_STORAGE_PERMISSION: &'static str =
        "authoritative source storage permission denied";

    /// Classify filesystem failures without retaining private paths or raw OS errors.
    /// Keep Protocol for compatibility with downstream exhaustive enum matches.
    pub fn source_storage_unavailable(error: std::io::Error) -> Self {
        Self::Protocol(
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                Self::SOURCE_STORAGE_PERMISSION
            } else {
                Self::SOURCE_STORAGE_IO
            }
            .into(),
        )
    }

    /// PostgreSQL rolled the whole transaction back because of a deadlock or a
    /// serialization failure. Nothing was committed, so repeating the same request
    /// with its original request ID is safe. Every other database error, including
    /// a lost connection, leaves the outcome open and is not classified here.
    pub fn is_retryable(&self) -> bool {
        use tokio_postgres::error::SqlState;
        matches!(
            self,
            Self::Db(error) if error.code().is_some_and(|code| {
                *code == SqlState::T_R_DEADLOCK_DETECTED
                    || *code == SqlState::T_R_SERIALIZATION_FAILURE
            })
        )
    }

    pub fn source_storage_reason(&self) -> Option<&'static str> {
        match self {
            Self::Protocol(message) if message == Self::SOURCE_STORAGE_PERMISSION => {
                Some("permission_denied")
            }
            Self::Protocol(message) if message == Self::SOURCE_STORAGE_IO => Some("io_error"),
            _ => None,
        }
    }

    const INVALID_COMMAND_FIELDS: &'static str = "invalid scoped command fields or bounds";
    const MISSING_EXECUTION_PREPARE_SESSION_BINDING: &'static str =
        "execution.prepare requires session binding";
    const MISSING_SESSION_START_CONVERSATION_ID: &'static str =
        "session.start requires conversation_id";

    const MISSING_HANDOFF_FENCE: &'static str = "handoff.accept requires the current runtime fence";
    const HANDOFF_UNAVAILABLE: &'static str = "handoff unavailable in selected work";

    pub fn handoff_unavailable() -> Self {
        Self::Protocol(Self::HANDOFF_UNAVAILABLE.into())
    }

    pub fn is_handoff_unavailable(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::HANDOFF_UNAVAILABLE)
    }

    pub fn missing_handoff_fence() -> Self {
        Self::Protocol(Self::MISSING_HANDOFF_FENCE.into())
    }

    pub fn is_missing_handoff_fence(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::MISSING_HANDOFF_FENCE)
    }

    pub fn invalid_command_fields() -> Self {
        Self::Protocol(Self::INVALID_COMMAND_FIELDS.into())
    }

    pub fn is_invalid_command_fields(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::INVALID_COMMAND_FIELDS)
    }

    pub fn missing_execution_prepare_session_binding() -> Self {
        Self::Protocol(Self::MISSING_EXECUTION_PREPARE_SESSION_BINDING.into())
    }

    pub fn is_missing_execution_prepare_session_binding(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::MISSING_EXECUTION_PREPARE_SESSION_BINDING)
    }

    pub fn missing_session_start_conversation_id() -> Self {
        Self::Protocol(Self::MISSING_SESSION_START_CONVERSATION_ID.into())
    }

    pub fn is_missing_session_start_conversation_id(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::MISSING_SESSION_START_CONVERSATION_ID)
    }

    const DELIVERY_PAYLOAD_INVALID: &'static str = "delivery_payload_invalid";

    /// Definite pre-persistence rejection of a malformed delivery_payload.
    /// Field and constraint are fixed allowlisted labels; caller values are
    /// never stored or echoed.
    pub fn delivery_payload_invalid(field: &'static str, constraint: &'static str) -> Self {
        Self::Protocol(format!(
            "{}/{field}/{constraint}",
            Self::DELIVERY_PAYLOAD_INVALID
        ))
    }

    pub fn is_delivery_payload_invalid(&self) -> bool {
        matches!(self, Self::Protocol(message) if message.starts_with("delivery_payload_invalid/"))
    }

    /// Bounded diagnostics for the typed delivery evidence entry, shared by HTTP
    /// and MCP. Both parts are allowlisted again so an unrelated Protocol error
    /// cannot echo input.
    pub fn delivery_payload_invalid_diagnostic(&self) -> Option<serde_json::Value> {
        let Self::Protocol(message) = self else {
            return None;
        };
        let rest = message.strip_prefix("delivery_payload_invalid/")?;
        let (field, constraint) = rest.split_once('/')?;
        let invalid_field = match field {
            "delivery_payload" => "/args/delivery_payload",
            "delivery_candidate_digest" => "/args/delivery_payload/delivery_candidate_digest",
            "passed" => "/args/delivery_payload/passed",
            "output_digest" => "/args/delivery_payload/output_digest",
            _ => return None,
        };
        let correction = match (field, constraint) {
            ("delivery_payload", "required") => {
                "Include args.delivery_payload as a JSON object; the generic payload alias is not accepted here."
            }
            ("delivery_payload", "type") => {
                "Submit args.delivery_payload as a JSON object, not a stringified JSON text, null, array or scalar."
            }
            ("delivery_candidate_digest", "required") => {
                "Include delivery_candidate_digest in args.delivery_payload."
            }
            ("delivery_candidate_digest", "pattern") => {
                "Use exactly 64 lowercase hexadecimal characters from the successful selection receipt's data.candidate_digest."
            }
            ("passed", "type") => "Use a JSON boolean for passed, or omit it.",
            ("output_digest", "type") => "Use a string for output_digest, or omit it.",
            ("output_digest", "pattern") => {
                "Use exactly 64 lowercase hexadecimal characters for output_digest, or omit it."
            }
            _ => return None,
        };
        Some(serde_json::json!({
            "code":"InvalidInput","reason":"delivery_payload_invalid",
            "message":"args.delivery_payload violates the delivery.evidence.submit input contract",
            "invalid_field":invalid_field,"constraint":constraint,
            "next_step":"Correct the indicated field and retry with the same request_id; query delivery.submission.describe for this work's typed input shape. Nothing was recorded.",
            "action_guidance":{
                "condition":"delivery.evidence.submit rejected the delivery_payload before any persistence.",
                "basis":"The typed entry requires a JSON object whose declared fields match its discovered schema.",
                "action":correction,
                "recheck":"After correcting args.delivery_payload. Current session, ownership, contract and authority checks still apply; this advice does not authorize execution or command replay."
            }
        }))
    }

    const WORK_NEXT_SELECTORS: &'static str = "work.next does not accept work selectors";
    const MISSING_REVIEW_SESSION_VERSION: &'static str =
        "review command requires expected_session_version";
    const MISSING_SIMULATED_EVIDENCE_EXECUTION: &'static str =
        "simulated member evidence requires execution_id";
    const REVIEW_SUBMISSION_INCOMPLETE: &'static str =
        "neutral review submission is incomplete or no longer current";
    const MANIFEST_DIGEST_MISMATCH: &'static str =
        "candidate manifest digest does not match its canonical manifest";

    pub fn manifest_digest_mismatch() -> Self {
        Self::Protocol(Self::MANIFEST_DIGEST_MISMATCH.into())
    }

    pub fn is_manifest_digest_mismatch(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::MANIFEST_DIGEST_MISMATCH)
    }

    pub fn review_submission_incomplete() -> Self {
        Self::Protocol(Self::REVIEW_SUBMISSION_INCOMPLETE.into())
    }

    /// A rejected neutral review submission carrying the precise failed check
    /// on the caller-selected evidence. The reason is a fixed allowlisted
    /// label; caller values are never stored or echoed.
    pub fn review_submission_incomplete_reason(reason: &'static str) -> Self {
        Self::Protocol(format!("{}/{reason}", Self::REVIEW_SUBMISSION_INCOMPLETE))
    }

    pub fn is_review_submission_incomplete(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::REVIEW_SUBMISSION_INCOMPLETE
            || message
                .strip_prefix(Self::REVIEW_SUBMISSION_INCOMPLETE)
                .is_some_and(|rest| rest.starts_with('/')))
    }

    /// The allowlisted reason label of a rejected submission, re-validated so
    /// an unrelated Protocol error cannot echo input. `None` for the bare
    /// legacy form and for unknown labels.
    pub fn review_submission_reason(&self) -> Option<&'static str> {
        let Self::Protocol(message) = self else {
            return None;
        };
        let reason = message
            .strip_prefix(Self::REVIEW_SUBMISSION_INCOMPLETE)?
            .strip_prefix('/')?;
        [
            "candidate_missing",
            "evidence_missing",
            "candidate_not_bound_in_evidence",
            "evidence_binding_changed",
            "original_candidate_unavailable",
            "evidence_integrity_unavailable",
            "manifest_content_unavailable",
            "selection_changed",
            "submission_incomplete",
        ]
        .into_iter()
        .find(|known| *known == reason)
    }

    /// Bounded selected-evidence diagnosis shared by HTTP and MCP. Condition,
    /// basis, next step and recheck trigger name only the caller's own
    /// submitted evidence and current candidate state; they never reveal
    /// foreign existence, secret values or artifact bytes.
    pub fn review_submission_incomplete_diagnostic(&self) -> Option<serde_json::Value> {
        let reason = self.review_submission_reason()?;
        let (condition, basis, next_action) = match reason {
            "candidate_missing" => (
                "No current delivery candidate is selected for this work.",
                "This delivery chain requires a version-bound candidate before review.",
                "Select the exact candidate through delivery.candidate.select (discover its input with delivery.submission.describe), submit the primary report through delivery.evidence.submit with args.delivery_payload.delivery_candidate_digest set to the selection receipt's data.candidate_digest, then open the review with that new evidence_id.",
            ),
            "evidence_missing" => (
                "The submitted evidence_id is not recorded as this work's review evidence.",
                "A current candidate is selected, but the evidence under review is missing from this work's records.",
                "Inspect delivery.neutral.inspect for this work, submit readable bound evidence through delivery.evidence.submit and retry review.open with the newly returned evidence_id.",
            ),
            "candidate_not_bound_in_evidence" => (
                "The evidence_id you submitted does not bind the current delivery candidate.",
                "Its stored payload has no valid delivery_candidate_digest, so it cannot stand as the candidate's primary report even when the candidate and its manifest artifacts are readable.",
                "Resubmit the same report through delivery.evidence.submit with args.delivery_payload.delivery_candidate_digest set to the current selection receipt's data.candidate_digest, then open the review with the new evidence_id. A global delivery.neutral.inspect lookup reports overall submission state; it does not identify which evidence_id binds the candidate.",
            ),
            "evidence_binding_changed" => (
                "The evidence digest no longer matches the bundle under review.",
                "The submission being decided differs from the evidence's current stored digest.",
                "Refresh review.inspect for the round and delivery.neutral.inspect for the work, then reopen the review against the current evidence instead of replaying the stale bundle.",
            ),
            "original_candidate_unavailable" => (
                "The candidate bound by the submitted evidence is no longer available.",
                "Its recorded binding digest has no stored candidate on this work.",
                "Select the candidate again through delivery.candidate.select, resubmit bound evidence through delivery.evidence.submit and open the review with the new evidence_id.",
            ),
            "evidence_integrity_unavailable" => (
                "The submitted evidence no longer matches its immutable receipt.",
                "Its stored payload and digests diverged from the recorded evidence digest.",
                "Record fresh bound evidence through delivery.evidence.submit and open the review with the new evidence_id; do not replay the drifted one.",
            ),
            "manifest_content_unavailable" => (
                "The original report or a manifest artifact is missing or its stored bytes changed.",
                "The pinned report and every manifest entry must match their stored bytes and bindings.",
                "Restore or republish the exact artifact bytes, verify readability through the delivery.neutral.inspect artifact queries, then resubmit bound evidence and reopen the review.",
            ),
            "selection_changed" => (
                "The submitted evidence binds a candidate that is not the current selection.",
                "Historical artifacts cannot approve a replacement selection; candidate, execution or contract versions drifted.",
                "Refresh work.prepare and claim.inspect, inspect the current selection through delivery.neutral.inspect, bind fresh evidence to its data.candidate_digest through delivery.evidence.submit and open a new review with that evidence_id.",
            ),
            _ => (
                "The original submission binding is unavailable.",
                "Candidate and evidence integrity must be established before review.",
                "Inspect delivery.neutral.inspect for submission context; refresh before retrying.",
            ),
        };
        Some(serde_json::json!({
            "code":"ReviewSubmissionIncomplete",
            "message":"review requires a current candidate and its readable manifest evidence",
            "reason":reason,
            "action_guidance":{
                "condition":condition,
                "basis":basis,
                "next_action":next_action,
                "recheck":"After the stated correction. Candidate, contract, selection, artifact and authority checks still apply; this advice does not authorize execution or command replay."
            }
        }))
    }

    pub fn work_next_selectors() -> Self {
        Self::Protocol(Self::WORK_NEXT_SELECTORS.into())
    }

    pub fn is_work_next_selectors(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::WORK_NEXT_SELECTORS)
    }

    pub fn missing_review_session_version() -> Self {
        Self::Protocol(Self::MISSING_REVIEW_SESSION_VERSION.into())
    }

    pub fn is_missing_review_session_version(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::MISSING_REVIEW_SESSION_VERSION)
    }

    pub fn missing_simulated_evidence_execution() -> Self {
        Self::Protocol(Self::MISSING_SIMULATED_EVIDENCE_EXECUTION.into())
    }

    pub fn is_missing_simulated_evidence_execution(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::MISSING_SIMULATED_EVIDENCE_EXECUTION)
    }
}
