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
    const INVALID_COMMAND_FIELDS: &'static str = "invalid scoped command fields or bounds";

    pub fn invalid_command_fields() -> Self {
        Self::Protocol(Self::INVALID_COMMAND_FIELDS.into())
    }

    pub fn is_invalid_command_fields(&self) -> bool {
        matches!(self, Self::Protocol(message) if message == Self::INVALID_COMMAND_FIELDS)
    }
}
