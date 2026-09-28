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
    #[error("author cannot review")]
    AuthorCannotReview,
    #[error("completion rejected")]
    CompletionRejected,
    #[error("policy downgrade")]
    PolicyDowngrade,
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

    const WORK_NEXT_SELECTORS: &'static str = "work.next does not accept work selectors";
    const MISSING_REVIEW_SESSION_VERSION: &'static str =
        "review command requires expected_session_version";

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
}
