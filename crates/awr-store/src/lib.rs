//! SQLite persistence. Callers use domain operations, never an exposed SQL handle.
mod admission;
mod branch;
mod branch_close;
mod catalog;
mod check;
mod checkpoint;
mod checkpoint_save;
mod claim_renewal;
mod client;
mod compaction;
mod containment;
mod contract;
mod definition;
mod delivery_adoption;
mod delta;
mod events;
mod evidence;
mod execution;
mod finding;
mod gap;
mod handoff;
mod management;
mod mcp;
mod team_handoff;
mod workstream_eta;
mod workstream_usage;
pub use mcp::with_mcp_operation;
mod mutation;
mod mutation_apply;
mod object_catalog;
mod operation_readset;
mod outcome_history;
mod preview_snapshot;
mod projection;
mod query;
mod reassessment;
mod reconcile;
mod resume;
mod runtime_lease;
mod schema;
mod scope_membership;
mod scoped_context;
mod scoped_read;
mod search;
mod session;
mod source_changes;
mod source_lock;
mod source_terminal;
pub use runtime_lease::{RuntimeLease, pending_path as restore_pending_path};
pub use source_lock::SourceLock;
mod transaction;
mod work;
mod work_action;
mod workstream;
mod workstream_runtime;
use awr_core::{Error, Result, now_millis};
pub use catalog::SourceRegistration;
pub use checkpoint_save::{
    CheckpointAttempt, CheckpointAttempts, SessionDeltaSnapshot, SourceObservation,
};
pub use contract::WorkContract;
pub use delta::{
    DeltaEvents, EntityDelta, EventReference, HistoryCount, ImportantEvent, SourceDelta,
};
pub use events::{BranchFilter, EventCursor, EventPage, EventQuery};
pub use object_catalog::{CatalogCursor, CatalogKind, CatalogPage, CatalogRow, CatalogScope};
pub use operation_readset::SQLITE_COORDINATOR_EPOCH;
pub use reconcile::{ReconcileAction, ReconcileReceipt, RuntimeFinding, RuntimeInspection};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
pub use scoped_read::{ScopedCursor, WorkstreamRead, WorkstreamReadSelection};
pub use search::{SearchHit, SearchQuery, SearchReport};
use serde::Serialize;
pub use source_changes::{ProjectionChange, SourceState};
pub use source_terminal::{
    FinishedWorkPlan, FinishedWorkReconcile, PlannedClaim, PlannedSession, ReconcileSkip,
    SOURCE_TERMINAL,
};
use std::{borrow::Cow, path::Path, time::Duration};
pub use work::{ScopedDependencyGraph, UnavailableDependency};
pub use workstream_eta::EtaIngestReceipt;
pub use workstream_usage::{UsageIngestReceipt, UsageRecord};

const APPLICATION_ID: i64 = 0x41575231;
/// Schema written by this build. Exposed for offline host compatibility negotiation.
pub const SCHEMA_VERSION: i64 = 14;
const CONTENT_REVIEWS_SQL: &str = include_str!("../migrations/007_content_reviews.sql");
const RESPONSIBILITY_SQL: &str = include_str!("../migrations/008_responsibility.sql");
const OPERATION_READSET_RECEIPTS_SQL: &str =
    include_str!("../migrations/009_operation_readset_receipts.sql");
const AGENT_AUTHORIZATION_SQL: &str = include_str!("../migrations/010_agent_authorization.sql");
const TEAM_HANDOFF_SQL: &str = include_str!("../migrations/011_team_handoff.sql");
const DELIVERY_DEPS_SQL: &str = include_str!("../migrations/012_delivery_deps.sql");
const USAGE_TIME_SQL: &str = include_str!("../migrations/013_usage_time.sql");
const ETA_CHECKPOINTS_SQL: &str = include_str!("../migrations/014_eta_checkpoints.sql");
const CATALOG_SQL: &str = include_str!("../migrations/001_catalog.sql");
const DOMAIN_SQL: &str = include_str!("../migrations/002_domain.sql");
const SEARCH_SQL: &str = include_str!("../migrations/003_search.sql");
const DRAFT_WORK_SQL: &str = include_str!("../migrations/004_draft_work.sql");
const WORKSTREAM_SQL: &str = include_str!("../migrations/005_workstreams.sql");
const WORKSTREAM_SESSIONS_SQL: &str = include_str!("../migrations/006_workstream_sessions.sql");

/// Domain operations own database writes; no SQL handle is exposed to callers.
///
/// ```compile_fail
/// fn overwrite(store: &mut awr_store::Store) {
///     store.conn.execute("UPDATE events SET summary='rewritten'", []).unwrap();
/// }
/// ```
///
/// ```compile_fail
/// fn arbitrary_write(store: &mut awr_store::Store, project: awr_core::Id) {
///     store.runtime_transaction(project, 0,
///         awr_core::EventDraft::new("report.observed", "Review report"),
///         |_transaction, _revision| Ok(())).unwrap();
/// }
/// ```
pub struct Store {
    pub(crate) conn: Connection,
    runtime_lease: Option<RuntimeLease>,
}

#[cfg(test)]
#[path = "../../../tests/concurrency/event_guards.rs"]
mod isolation_event_tests;

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub application_id: i64,
    pub schema_version: i64,
    pub sqlite_version: String,
    pub journal_mode: String,
    pub foreign_keys: bool,
    pub busy_timeout_ms: i64,
    pub migrations: Vec<MigrationInfo>,
    pub integrity: Vec<String>,
    pub foreign_key_violations: usize,
    pub schema_issues: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreign_key_check_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MigrationInfo {
    pub version: i64,
    pub name: String,
    pub applied_at: i64,
}

pub(crate) fn db_error(error: rusqlite::Error) -> Error {
    Error::Storage(storage_error_detail(None, error))
}

/// Wrap a SQLite error with the failing operation and database path so that
/// low-level failures (disk I/O, permissions, sandbox denials) carry a
/// diagnosable context instead of a bare "disk I/O error".
pub(crate) fn db_error_at(operation: &str, path: &Path, error: rusqlite::Error) -> Error {
    Error::Storage(storage_error_detail(Some((operation, path)), error))
}

fn storage_error_detail(context: Option<(&str, &Path)>, error: rusqlite::Error) -> String {
    let mut detail = match context {
        Some((operation, path)) => format!("sqlite {operation} on {}: {error}", path.display()),
        None => error.to_string(),
    };
    let hint = match error.sqlite_error_code() {
        Some(rusqlite::ffi::ErrorCode::SystemIoFailure)
        | Some(rusqlite::ffi::ErrorCode::DiskFull)
        | Some(rusqlite::ffi::ErrorCode::CannotOpen)
        | Some(rusqlite::ffi::ErrorCode::PermissionDenied) => Some(
            "the database file or its directory is not writable (check disk space, file permissions, and OS file-access or sandbox policies)",
        ),
        Some(rusqlite::ffi::ErrorCode::ReadOnly) => {
            Some("the database was opened read-only; writes require a writable database path")
        }
        Some(rusqlite::ffi::ErrorCode::DatabaseCorrupt)
        | Some(rusqlite::ffi::ErrorCode::NotADatabase) => Some(
            "the file may not be an AWR SQLite database or is damaged; run doctor to inspect it",
        ),
        Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
        | Some(rusqlite::ffi::ErrorCode::DatabaseLocked) => {
            Some("another process holds the database lock; retry after it exits")
        }
        Some(rusqlite::ffi::ErrorCode::FileLockingProtocolFailed) => Some(
            "SQLite WAL locking protocol failed while coordinating shared memory; retry or check filesystem locking support",
        ),
        _ => None,
    };
    if let Some(hint) = hint {
        detail.push_str("; ");
        detail.push_str(hint);
    }
    detail
}

const LOCKING_PROTOCOL_RETRY_BACKOFF_MS: [u64; 3] = [50, 100, 200];

/// Reserve headroom below Windows `MAX_PATH` (260 UTF-16 code units, including
/// the terminating NUL) for SQLite sidecars (`-journal`, `-wal`, `-shm`) when
/// converting a verbatim local-drive path (`\\?\C:\...`) to an ordinary drive
/// path (`C:\...`).
#[cfg(any(windows, test))]
const WINDOWS_SQLITE_MAX_STRIPPED_UNITS: usize = 240;

pub(crate) fn is_locking_protocol_error(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ffi::ErrorCode::FileLockingProtocolFailed)
    ) || matches!(
        error,
        rusqlite::Error::SqliteFailure(err, _)
            if err.extended_code == rusqlite::ffi::SQLITE_PROTOCOL
    )
}

pub(crate) enum OpenError {
    LockingProtocol(Error),
    Other(Error),
}

impl OpenError {
    pub(crate) fn from_sqlite(error: rusqlite::Error) -> Self {
        let protocol = is_locking_protocol_error(&error);
        let mapped = db_error(error);
        if protocol {
            Self::LockingProtocol(mapped)
        } else {
            Self::Other(mapped)
        }
    }

    fn from_sqlite_at(operation: &str, path: &Path, error: rusqlite::Error) -> Self {
        let protocol = is_locking_protocol_error(&error);
        let mapped = db_error_at(operation, path, error);
        if protocol {
            Self::LockingProtocol(mapped)
        } else {
            Self::Other(mapped)
        }
    }

    pub(crate) fn into_error(self) -> Error {
        match self {
            Self::LockingProtocol(error) | Self::Other(error) => error,
        }
    }
}

impl From<Error> for OpenError {
    fn from(error: Error) -> Self {
        Self::Other(error)
    }
}

fn retry_on_locking_protocol<T>(
    attempt: impl FnMut() -> std::result::Result<T, OpenError>,
) -> Result<T> {
    retry_on_locking_protocol_with(
        &LOCKING_PROTOCOL_RETRY_BACKOFF_MS,
        std::thread::sleep,
        attempt,
    )
}

fn retry_on_locking_protocol_with<T>(
    backoff_ms: &[u64],
    mut sleep: impl FnMut(Duration),
    mut attempt: impl FnMut() -> std::result::Result<T, OpenError>,
) -> Result<T> {
    let mut retry = 0;
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(OpenError::LockingProtocol(_)) if retry < backoff_ms.len() => {
                sleep(Duration::from_millis(backoff_ms[retry]));
                retry += 1;
            }
            Err(error) => return Err(error.into_error()),
        }
    }
}

/// Prepare a filesystem path for `rusqlite::Connection::open*`.
///
/// On Windows, `std::fs::canonicalize` returns verbatim local-drive paths such
/// as `\\?\C:\...`. Bundled SQLite 3.53.x classifies any path beginning with
/// `\\` as a UNC share (`winIsUNCPath`) and routes WAL shared-memory locking
/// through a shared-handle path (`bUseSharedLockHandle`) whose mutex boundaries
/// race under concurrent in-process opens and can fail with `SQLITE_PROTOCOL`.
/// Converting safe local-drive verbatim paths to `C:\...` keeps them on
/// SQLite's per-connection `-shm` lock handle path while preserving `\\?\` for
/// UNC paths, volume GUIDs, long paths, and verbatim-only path segments.
fn sqlite_open_path(path: &Path) -> Cow<'_, Path> {
    #[cfg(windows)]
    {
        use std::{
            ffi::OsString,
            os::windows::ffi::{OsStrExt, OsStringExt},
            path::PathBuf,
        };

        let units: Vec<u16> = path.as_os_str().encode_wide().collect();
        if let Some(stripped) = strip_windows_verbatim_disk_utf16(&units) {
            return Cow::Owned(PathBuf::from(OsString::from_wide(&stripped)));
        }
    }
    Cow::Borrowed(path)
}

#[cfg(any(windows, test))]
fn strip_windows_verbatim_disk_utf16(units: &[u16]) -> Option<Vec<u16>> {
    const BACKSLASH: u16 = b'\\' as u16;
    const SLASH: u16 = b'/' as u16;
    const QUESTION: u16 = b'?' as u16;
    const COLON: u16 = b':' as u16;
    const DOT: u16 = b'.' as u16;
    const SPACE: u16 = b' ' as u16;

    // Require `\\?\<ASCII letter>:\<at least one char>`.
    if units.len() < 8
        || units[0] != BACKSLASH
        || units[1] != BACKSLASH
        || units[2] != QUESTION
        || units[3] != BACKSLASH
        || !u8::try_from(units[4])
            .ok()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        || units[5] != COLON
        || units[6] != BACKSLASH
    {
        return None;
    }

    let rest = &units[4..];
    if rest.len() > WINDOWS_SQLITE_MAX_STRIPPED_UNITS {
        return None;
    }

    for segment in rest[3..].split(|&unit| unit == BACKSLASH || unit == SLASH) {
        if segment.is_empty()
            || segment == [DOT]
            || segment == [DOT, DOT]
            || matches!(segment.last(), Some(&SPACE | &DOT))
            || segment
                .iter()
                .any(|&unit| is_invalid_win32_component_unit(unit))
            || is_reserved_windows_device_name(segment)
        {
            return None;
        }
    }

    let mut normalized = Vec::with_capacity(rest.len());
    for &unit in rest {
        normalized.push(if unit == SLASH { BACKSLASH } else { unit });
    }
    Some(normalized)
}

#[cfg(any(windows, test))]
fn is_invalid_win32_component_unit(unit: u16) -> bool {
    unit <= 0x1F
        || matches!(
            u8::try_from(unit),
            Ok(b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*')
        )
}

#[cfg(any(windows, test))]
fn is_reserved_windows_device_name(segment: &[u16]) -> bool {
    const DOT: u16 = b'.' as u16;
    const SPACE: u16 = b' ' as u16;

    let stem = match segment.iter().position(|&unit| unit == DOT) {
        Some(index) => &segment[..index],
        None => segment,
    };
    let end = stem
        .iter()
        .rposition(|&unit| unit != SPACE)
        .map_or(0, |index| index + 1);
    let stem = &stem[..end];

    let ascii_eq = |expected: &[u8]| {
        stem.len() == expected.len()
            && stem.iter().zip(expected).all(|(&unit, &target)| {
                u8::try_from(unit)
                    .ok()
                    .is_some_and(|byte| byte.eq_ignore_ascii_case(&target))
            })
    };

    if ascii_eq(b"CON")
        || ascii_eq(b"PRN")
        || ascii_eq(b"AUX")
        || ascii_eq(b"NUL")
        || ascii_eq(b"CONIN$")
        || ascii_eq(b"CONOUT$")
    {
        return true;
    }

    if stem.len() == 4 {
        let is_com_or_lpt = |prefix: &[u8; 3]| {
            stem[..3].iter().zip(prefix).all(|(&unit, &target)| {
                u8::try_from(unit)
                    .ok()
                    .is_some_and(|byte| byte.eq_ignore_ascii_case(&target))
            })
        };
        if is_com_or_lpt(b"COM") || is_com_or_lpt(b"LPT") {
            return matches!(stem[3], 0x0030..=0x0039 | 0x00B2 | 0x00B3 | 0x00B9);
        }
    }

    false
}

impl Store {
    /// Copy a coherent database view into private RAM. Rebuilding caches on the returned
    /// store cannot persist anything here. Callers still verify source freshness and the
    /// originating project revision before returning a read result.
    pub fn memory_snapshot(&self, max_bytes: u64) -> Result<Self> {
        let pages: u32 = self
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .map_err(db_error)?;
        let page_size: u32 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(db_error)?;
        if u64::from(pages) * u64::from(page_size) > max_bytes || max_bytes == 0 {
            return Err(Error::InvalidInput(format!(
                "database exceeds the {max_bytes} byte read-snapshot limit"
            )));
        }
        let mut conn = Connection::open_in_memory().map_err(db_error)?;
        {
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut conn).map_err(db_error)?;
            if !matches!(
                backup.step(-1).map_err(db_error)?,
                rusqlite::backup::StepResult::Done
            ) {
                return Err(Error::Storage(
                    "database snapshot was busy; retry the read".into(),
                ));
            }
        }
        let copied_pages: u32 = conn
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .map_err(db_error)?;
        if u64::from(copied_pages) * u64::from(page_size) > max_bytes {
            return Err(Error::InvalidInput(
                "database grew beyond the read-snapshot limit".into(),
            ));
        }
        Self::configure(&conn)?;
        Ok(Self {
            conn,
            runtime_lease: None,
        })
    }

    /// Open the current schema for queries without creating or upgrading database state.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let runtime_lease = RuntimeLease::shared(path, false)?;
        if !path.is_file() {
            return Err(Error::NotFound(format!("AWR database: {}", path.display())));
        }
        let open_path = sqlite_open_path(path);
        let conn = retry_on_locking_protocol(|| {
            let conn = Connection::open_with_flags(&open_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| OpenError::from_sqlite_at("open read-only", path, error))?;
            Self::configure_open(&conn)?;
            let owner: i64 = conn
                .pragma_query_value(None, "application_id", |r| r.get(0))
                .map_err(OpenError::from_sqlite)?;
            let version: i64 = conn
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(OpenError::from_sqlite)?;
            if owner != APPLICATION_ID || version != SCHEMA_VERSION {
                return Err(OpenError::Other(Error::Storage(
                    "read queries require a current AWR database; use doctor to inspect it".into(),
                )));
            }
            schema::verify_open(&conn, SCHEMA_VERSION)?;
            Ok(conn)
        })?;
        Ok(Self {
            conn,
            runtime_lease,
        })
    }

    /// Open an existing current-schema database for domain writes without creating or migrating it.
    pub fn open_existing(path: &Path) -> Result<Self> {
        let runtime_lease = RuntimeLease::shared(path, true)?;
        if !path.is_file() {
            return Err(Error::NotFound(format!("AWR database: {}", path.display())));
        }
        let open_path = sqlite_open_path(path);
        let conn = retry_on_locking_protocol(|| {
            let conn =
                Connection::open_with_flags(&open_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                    .map_err(|error| OpenError::from_sqlite_at("open read-write", path, error))?;
            Self::configure_open(&conn)?;
            let owner: i64 = conn
                .pragma_query_value(None, "application_id", |r| r.get(0))
                .map_err(OpenError::from_sqlite)?;
            let version: i64 = conn
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(OpenError::from_sqlite)?;
            if owner != APPLICATION_ID || version != SCHEMA_VERSION {
                return Err(OpenError::Other(Error::Storage(
                    "writes require an existing current AWR database; inspect it before repair"
                        .into(),
                )));
            }
            schema::verify_open(&conn, SCHEMA_VERSION)?;
            Ok(conn)
        })?;
        Ok(Self {
            conn,
            runtime_lease,
        })
    }

    /// Create or reopen an AWR database. Refuse unrelated databases.
    pub fn open(path: &Path) -> Result<Self> {
        let lease = RuntimeLease::shared(path, true)?;
        let open_path = sqlite_open_path(path);
        let mut store = retry_on_locking_protocol(|| {
            let conn = Connection::open(&open_path)
                .map_err(|error| OpenError::from_sqlite_at("create or open", path, error))?;
            Self::initialize_connection_open(conn, true)
        })?;
        store.runtime_lease = lease;
        Ok(store)
    }

    /// An isolated current-schema store for semantic previews. No disk files are created.
    pub fn memory() -> Result<Self> {
        Self::initialize_connection(Connection::open_in_memory().map_err(db_error)?, false)
    }

    /// Copy and, if necessary, upgrade only a private in-memory copy of an owned database.
    pub fn preview_snapshot(path: &Path, max_bytes: u64) -> Result<Self> {
        Self::capture_snapshot(path, max_bytes, true)
    }
    /// Current-schema read snapshot with no project-file writes and no migration.
    pub fn read_snapshot(path: &Path, max_bytes: u64) -> Result<Self> {
        Self::capture_snapshot(path, max_bytes, false)
    }
    fn capture_snapshot(path: &Path, max_bytes: u64, upgrade: bool) -> Result<Self> {
        let _lease = RuntimeLease::shared(path, false)?;
        Self::capture_snapshot_unleased(path, max_bytes, upgrade)
    }
    /// Offline inspection under an exclusive runtime lease, including pending restore recovery.
    pub fn read_snapshot_with_lease(
        path: &Path,
        max_bytes: u64,
        lease: &RuntimeLease,
    ) -> Result<Self> {
        lease.check_database(path)?;
        Self::capture_snapshot_unleased(path, max_bytes, false)
    }
    fn capture_snapshot_unleased(path: &Path, max_bytes: u64, upgrade: bool) -> Result<Self> {
        // Opening even a read-only WAL database may create sidecars. Capture bounded,
        // stable file images first and let SQLite recover only the private temporary copy.
        let snapshot = preview_snapshot::Files::capture(path, max_bytes)?;
        let open_path = sqlite_open_path(&snapshot.database);
        let conn = Connection::open_with_flags(&open_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| db_error_at("open snapshot read-only", &snapshot.database, error))?;
        Self::configure(&conn)?;
        let owner: i64 = conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(db_error)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        if owner != APPLICATION_ID || !(1..=SCHEMA_VERSION).contains(&version) {
            return Err(Error::Storage(
                "preview requires an owned, supported database".into(),
            ));
        }
        if !upgrade && version != SCHEMA_VERSION {
            return Err(Error::Storage(
                "read snapshot requires the current schema; inspect and upgrade explicitly".into(),
            ));
        }
        schema::verify(&conn, version)?;
        let copy = Self {
            conn,
            runtime_lease: None,
        }
        .memory_snapshot(max_bytes)?;
        if upgrade {
            Self::initialize_connection(copy.conn, false)
        } else {
            Ok(copy)
        }
    }

    pub fn require_memory(&self) -> Result<()> {
        let file: String = self
            .conn
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name='main'",
                [],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if file.is_empty() {
            Ok(())
        } else {
            Err(Error::RuleViolation(
                "semantic preview requires an in-memory store".into(),
            ))
        }
    }

    fn initialize_connection(conn: Connection, persistent: bool) -> Result<Self> {
        Self::initialize_connection_open(conn, persistent).map_err(OpenError::into_error)
    }

    fn initialize_connection_open(
        mut conn: Connection,
        persistent: bool,
    ) -> std::result::Result<Self, OpenError> {
        Self::configure_open(&conn)?;
        let application_id: i64 = conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(OpenError::from_sqlite)?;
        let current: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(OpenError::from_sqlite)?;
        if application_id == 0 {
            let objects: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                    [],
                    |r| r.get(0),
                )
                .map_err(OpenError::from_sqlite)?;
            if objects != 0 || current != 0 {
                return Err(OpenError::Other(Error::Storage(
                    "database is not empty or owned by AWR".into(),
                )));
            }
        } else if application_id != APPLICATION_ID {
            return Err(OpenError::Other(Error::Storage(
                "database belongs to a different application".into(),
            )));
        }
        if current > SCHEMA_VERSION {
            return Err(OpenError::Other(Error::Storage(format!(
                "schema {current} is newer than supported {SCHEMA_VERSION}"
            ))));
        }
        if current > 0 {
            schema::verify_open(&conn, current)?;
        }
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .map_err(OpenError::from_sqlite)?;
        if persistent && mode != "wal" {
            let actual: String = conn
                .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
                .map_err(OpenError::from_sqlite)?;
            if actual != "wal" {
                return Err(OpenError::Other(Error::Storage(format!(
                    "WAL could not be enabled: {actual}"
                ))));
            }
        }
        if current < SCHEMA_VERSION {
            // SQLite table rebuild protocol: suppress FK actions only on this unexposed
            // connection, then check all references before committing and restore enforcement.
            conn.pragma_update(None, "foreign_keys", false)
                .map_err(OpenError::from_sqlite)?;
            conn.pragma_update(None, "legacy_alter_table", true)
                .map_err(OpenError::from_sqlite)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(OpenError::from_sqlite)?;
            let version: i64 = tx
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(OpenError::from_sqlite)?;
            if version > 0 {
                // Validate the actual base under the writer lock before adding
                // anything. A failed upgrade must not commit a partial new schema.
                schema::verify_open(&tx, version)?;
            }
            if version == 0 {
                tx.execute_batch(CATALOG_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(1,'catalog',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
                tx.pragma_update(None, "application_id", APPLICATION_ID)
                    .map_err(OpenError::from_sqlite)?;
            }
            if version < 2 {
                tx.execute_batch(DOMAIN_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(2,'domain',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 3 {
                tx.execute_batch(SEARCH_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(3,'search',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 4 {
                // Preserve user diagnostic indexes/triggers as well as owned schema objects.
                let extras: Vec<String> = tx
                    .prepare("SELECT sql FROM sqlite_master WHERE tbl_name='work_items' AND type IN ('index','trigger') AND sql IS NOT NULL AND name NOT IN ('work_items_source','work_status') ORDER BY name")
                    .map_err(OpenError::from_sqlite)?
                    .query_map([], |row| row.get(0))
                    .map_err(OpenError::from_sqlite)?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(OpenError::from_sqlite)?;
                tx.execute_batch(DRAFT_WORK_SQL)
                    .map_err(OpenError::from_sqlite)?;
                for sql in extras {
                    tx.execute_batch(&sql).map_err(OpenError::from_sqlite)?;
                }
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(4,'draft_work',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 5 {
                tx.execute_batch(WORKSTREAM_SQL)
                    .map_err(OpenError::from_sqlite)?;
                workstream::migrate_legacy(&tx)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(5,'workstreams',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 6 {
                tx.execute_batch(WORKSTREAM_SESSIONS_SQL)
                    .map_err(OpenError::from_sqlite)?;
                workstream_runtime::migrate(&tx)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(6,'workstream_sessions',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 7 {
                tx.execute_batch(CONTENT_REVIEWS_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(7,'content_reviews',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 8 {
                tx.execute_batch(RESPONSIBILITY_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(8,'responsibility',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 9 {
                tx.execute_batch(OPERATION_READSET_RECEIPTS_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(9,'operation_readset_receipts',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 10 {
                tx.execute_batch(AGENT_AUTHORIZATION_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(10,'agent_authorization',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 11 {
                tx.execute_batch(TEAM_HANDOFF_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(11,'team_handoff',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 12 {
                tx.execute_batch(DELIVERY_DEPS_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(12,'delivery_deps',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 13 {
                tx.execute_batch(USAGE_TIME_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(13,'usage_time',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            if version < 14 {
                tx.execute_batch(ETA_CHECKPOINTS_SQL)
                    .map_err(OpenError::from_sqlite)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(14,'eta_checkpoints',?1)",
                    [now_millis()?],
                )
                .map_err(OpenError::from_sqlite)?;
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(OpenError::from_sqlite)?;
            schema::verify_open(&tx, SCHEMA_VERSION)?;
            if tx
                .prepare("PRAGMA foreign_key_check")
                .map_err(OpenError::from_sqlite)?
                .exists([])
                .map_err(OpenError::from_sqlite)?
            {
                return Err(OpenError::Other(Error::Storage(
                    "migration would leave invalid foreign-key references".into(),
                )));
            }
            tx.commit().map_err(OpenError::from_sqlite)?;
            conn.pragma_update(None, "legacy_alter_table", false)
                .map_err(OpenError::from_sqlite)?;
            conn.pragma_update(None, "foreign_keys", true)
                .map_err(OpenError::from_sqlite)?;
        }
        let store = Self {
            conn,
            runtime_lease: None,
        };
        schema::verify_open(&store.conn, SCHEMA_VERSION)?;
        Ok(store)
    }

    fn configure(conn: &Connection) -> Result<()> {
        Self::configure_open(conn).map_err(OpenError::into_error)
    }

    fn configure_open(conn: &Connection) -> std::result::Result<(), OpenError> {
        conn.busy_timeout(Duration::from_millis(5000))
            .map_err(OpenError::from_sqlite)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(OpenError::from_sqlite)?;
        // REPLACE performs an implicit delete. Without recursive triggers SQLite
        // can bypass event_no_delete and overwrite an existing append-only receipt.
        conn.pragma_update(None, "recursive_triggers", true)
            .map_err(OpenError::from_sqlite)
    }

    /// Inspect without creating, upgrading, or repairing the database.
    pub fn inspect(path: &Path) -> Result<DoctorReport> {
        let _lease = RuntimeLease::shared(path, false)?;
        if !path.is_file() {
            return Err(Error::NotFound(format!("AWR database: {}", path.display())));
        }
        let open_path = sqlite_open_path(path);
        retry_on_locking_protocol(|| {
            let conn = Connection::open_with_flags(&open_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| OpenError::from_sqlite_at("open read-only", path, error))?;
            Self::configure_open(&conn)?;
            Self {
                conn,
                runtime_lease: None,
            }
            .doctor_open()
        })
    }

    pub fn doctor(&self) -> Result<DoctorReport> {
        self.doctor_open().map_err(OpenError::into_error)
    }

    fn doctor_open(&self) -> std::result::Result<DoctorReport, OpenError> {
        let application_id: i64 = self
            .conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(OpenError::from_sqlite)?;
        if application_id != APPLICATION_ID {
            return Err(OpenError::Other(Error::Storage(
                "not an AWR database".into(),
            )));
        }
        let schema_version = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(OpenError::from_sqlite)?;
        let integrity = self
            .conn
            .prepare("PRAGMA integrity_check")
            .map_err(OpenError::from_sqlite)?
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(OpenError::from_sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(OpenError::from_sqlite)?;
        let foreign_key_check = (|| -> rusqlite::Result<usize> {
            Ok(self
                .conn
                .prepare("PRAGMA foreign_key_check")?
                .query_map([], |_| Ok(()))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .len())
        })();
        let (foreign_key_violations, foreign_key_check_error) = match foreign_key_check {
            Ok(count) => (count, None),
            Err(error) if is_locking_protocol_error(&error) => {
                return Err(OpenError::from_sqlite(error));
            }
            Err(_) => (
                0,
                Some("foreign-key check could not run for this schema".into()),
            ),
        };
        let mut schema_issues = schema::issues_open(&self.conn, schema_version)?;
        let migrations = match schema::migrations_raw(&self.conn) {
            Ok(migrations) => migrations,
            Err(error) if is_locking_protocol_error(&error) => {
                return Err(OpenError::from_sqlite(error));
            }
            Err(_) => {
                schema_issues.push("migration catalog cannot be read".into());
                Vec::new()
            }
        };
        let catalog_ok = schema::catalog_matches(&migrations, schema_version);
        if !catalog_ok {
            schema_issues.push("migration catalog does not match the supported schema".into());
        }
        Ok(DoctorReport {
            ok: integrity == ["ok"]
                && foreign_key_violations == 0
                && foreign_key_check_error.is_none()
                && schema_version == SCHEMA_VERSION
                && schema_issues.is_empty(),
            application_id,
            schema_version,
            sqlite_version: self
                .conn
                .query_row("SELECT sqlite_version()", [], |r| r.get(0))
                .map_err(OpenError::from_sqlite)?,
            journal_mode: self
                .conn
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .map_err(OpenError::from_sqlite)?,
            foreign_keys: self
                .conn
                .pragma_query_value(None, "foreign_keys", |r| r.get(0))
                .map_err(OpenError::from_sqlite)?,
            busy_timeout_ms: self
                .conn
                .pragma_query_value(None, "busy_timeout", |r| r.get(0))
                .map_err(OpenError::from_sqlite)?,
            migrations,
            integrity,
            foreign_key_violations,
            schema_issues,
            foreign_key_check_error,
        })
    }
}

impl Store {
    /// Export an already coherent RAM snapshot to a new SQLite file. No private SQL API escapes.
    pub fn export_snapshot(&self, path: &Path) -> Result<()> {
        self.require_memory()?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        drop(options.open(path)?);
        let open_path = sqlite_open_path(path);
        let mut target = Connection::open(&open_path).map_err(db_error)?;
        {
            let backup =
                rusqlite::backup::Backup::new(&self.conn, &mut target).map_err(db_error)?;
            if !matches!(
                backup.step(-1).map_err(db_error)?,
                rusqlite::backup::StepResult::Done
            ) {
                return Err(Error::Storage("snapshot export did not finish".into()));
            }
        }
        target
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(db_error)?;
        drop(target);
        // Windows FlushFileBuffers requires a handle opened for writing.
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .sync_all()?;
        Ok(())
    }
}

pub mod agent_authorization;
mod content_review;
mod responsibility;
pub use agent_authorization::AuthorizationReceipt;

#[cfg(test)]
mod storage_error_tests {
    use super::*;

    #[test]
    fn opening_a_non_database_file_names_the_file_and_the_likely_cause() {
        let directory =
            std::env::temp_dir().join(format!("awr-store-diag-{}", awr_core::Id::new()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.db");
        std::fs::write(&path, b"definitely not sqlite").unwrap();
        let error = match Store::open_readonly(&path) {
            Err(Error::Storage(detail)) => detail,
            other => panic!("expected a storage error"),
        };
        std::fs::remove_dir_all(&directory).unwrap();
        // The failure surfaces from the schema probe after a successful open,
        // so the hint (not the path prefix) is the diagnosable part here.
        assert!(error.contains("not be an AWR SQLite database"), "{error}");
    }

    #[test]
    fn io_failure_codes_carry_a_root_cause_hint() {
        let error = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
            Some("disk I/O error".into()),
        );
        let detail = match db_error(error) {
            Error::Storage(detail) => detail,
            other => panic!("expected storage, got {other:?}"),
        };
        assert!(detail.contains("disk I/O error"), "{detail}");
        assert!(detail.contains("not writable"), "{detail}");
    }
}

#[cfg(test)]
mod wal_open_tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn strip_str(input: &str) -> Option<String> {
        let units: Vec<u16> = input.encode_utf16().collect();
        strip_windows_verbatim_disk_utf16(&units).map(|out| String::from_utf16(&out).unwrap())
    }

    #[test]
    fn windows_verbatim_disk_paths_strip_only_when_safe_for_win32() {
        // Canonicalized local paths and `.join(".awr/state.db")` mixed separators.
        assert_eq!(
            strip_str(r"\\?\C:\Users\dev\project\.awr\state.db").as_deref(),
            Some(r"C:\Users\dev\project\.awr\state.db")
        );
        assert_eq!(
            strip_str(r"\\?\D:\a\awr\awr\.awr/state.db").as_deref(),
            Some(r"D:\a\awr\awr\.awr\state.db")
        );
        assert_eq!(
            strip_str(r"\\?\c:\tmp\项目\.awr\state.db").as_deref(),
            Some(r"c:\tmp\项目\.awr\state.db")
        );

        // Non-disk, UNC, volume GUID, device, and already-ordinary paths stay unchanged.
        for preserved in [
            r"\\?\UNC\server\share\state.db",
            r"\\server\share\state.db",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\state.db",
            r"\\.\C:\tmp\state.db",
            r"\\?\C:\",
            r"C:\tmp\state.db",
            "/tmp/state.db",
            "state.db",
        ] {
            assert_eq!(strip_str(preserved), None, "should preserve {preserved}");
        }

        // Verbatim-only segments that Win32 would reinterpret must stay verbatim.
        for preserved in [
            r"\\?\C:\tmp\.\state.db",
            r"\\?\C:\tmp\..\state.db",
            r"\\?\C:\tmp\\state.db",
            r"\\?\C:\tmp\state.db\",
            r"\\?\C:\tmp\dir.\state.db",
            r"\\?\C:\tmp\dir \state.db",
            r"\\?\C:\tmp\CON\state.db",
            r"\\?\C:\tmp\nul.db",
            r"\\?\C:\tmp\com1\state.db",
            r"\\?\C:\tmp\lpt9 .txt\state.db",
            r"\\?\C:\tmp\COM²\state.db",
            r"\\?\C:\tmp\bad:stream\state.db",
            r"\\?\C:\tmp\wild*card\state.db",
        ] {
            assert_eq!(strip_str(preserved), None, "should preserve {preserved}");
        }

        // Enforce MAX_PATH sidecar headroom boundary (240 UTF-16 code units).
        let prefix = r"\\?\C:\";
        let exact_240 = format!(
            "{prefix}{}",
            "a".repeat(WINDOWS_SQLITE_MAX_STRIPPED_UNITS - 3)
        );
        assert_eq!(
            strip_str(&exact_240).map(|s| s.encode_utf16().count()),
            Some(WINDOWS_SQLITE_MAX_STRIPPED_UNITS)
        );
        let over_240 = format!(
            "{prefix}{}",
            "a".repeat(WINDOWS_SQLITE_MAX_STRIPPED_UNITS - 2)
        );
        assert_eq!(strip_str(&over_240), None);
    }

    #[test]
    fn locking_protocol_error_detection_and_hint_are_specific() {
        let protocol = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_PROTOCOL),
            Some("locking protocol".into()),
        );
        assert!(is_locking_protocol_error(&protocol));
        let detail = match db_error(protocol) {
            Error::Storage(detail) => detail,
            other => panic!("expected storage error, got {other:?}"),
        };
        assert!(detail.contains("locking protocol"), "{detail}");
        assert!(detail.contains("WAL locking protocol"), "{detail}");

        for non_protocol in [
            rusqlite::ffi::SQLITE_BUSY,
            rusqlite::ffi::SQLITE_LOCKED,
            rusqlite::ffi::SQLITE_IOERR,
            rusqlite::ffi::SQLITE_CANTOPEN,
        ] {
            let error =
                rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(non_protocol), None);
            assert!(!is_locking_protocol_error(&error), "code {non_protocol}");
        }
    }

    #[test]
    fn locking_protocol_retry_recovers_bounded_attempts_and_never_retries_other_errors() {
        let protocol_error = || {
            OpenError::from_sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_PROTOCOL),
                Some("locking protocol".into()),
            ))
        };

        // Recovers on the third attempt after two transient protocol failures.
        let mut attempts = 0;
        let mut delays = Vec::new();
        let value = retry_on_locking_protocol_with(
            &LOCKING_PROTOCOL_RETRY_BACKOFF_MS,
            |delay| delays.push(delay),
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(protocol_error())
                } else {
                    Ok(42)
                }
            },
        )
        .unwrap();
        assert_eq!(value, 42);
        assert_eq!(attempts, 3);
        assert_eq!(
            delays,
            vec![Duration::from_millis(50), Duration::from_millis(100)]
        );

        // Exhausts retries and returns the locking protocol error.
        let mut attempts = 0;
        let mut delays = Vec::new();
        let error = retry_on_locking_protocol_with(
            &LOCKING_PROTOCOL_RETRY_BACKOFF_MS,
            |delay| delays.push(delay),
            || -> std::result::Result<(), OpenError> {
                attempts += 1;
                Err(protocol_error())
            },
        )
        .unwrap_err();
        assert_eq!(attempts, 4);
        assert_eq!(
            delays,
            vec![
                Duration::from_millis(50),
                Duration::from_millis(100),
                Duration::from_millis(200),
            ]
        );
        assert!(matches!(error, Error::Storage(ref msg) if msg.contains("locking protocol")));

        // Never retries non-protocol SQLite errors such as SQLITE_BUSY.
        let mut attempts = 0;
        let mut delays = Vec::new();
        let error = retry_on_locking_protocol_with(
            &LOCKING_PROTOCOL_RETRY_BACKOFF_MS,
            |delay| delays.push(delay),
            || -> std::result::Result<(), OpenError> {
                attempts += 1;
                Err(OpenError::from_sqlite(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    Some("database is locked".into()),
                )))
            },
        )
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(delays.is_empty());
        assert!(matches!(error, Error::Storage(ref msg) if msg.contains("database is locked")));
    }

    #[test]
    fn concurrent_cold_opens_after_wal_close_succeed_across_store_modes() {
        let root = std::env::temp_dir().join(format!("awr-store-wal-open-{}", awr_core::Id::new()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let db = root.join("state.db");

        // Initialize the WAL database once and close it cleanly so `-wal`/`-shm` are removed.
        let store = Store::open(&db).unwrap();
        assert_eq!(store.doctor().unwrap().journal_mode, "wal");
        drop(store);

        // Repeatedly race concurrent cold opens across all store entry points right
        // after the previous connections have closed and removed `-shm`.
        for _ in 0..8 {
            let barrier = Arc::new(Barrier::new(4));
            let handles: Vec<_> = (0..4)
                .map(|index| {
                    let db = db.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        match index {
                            0 => {
                                let store = Store::open_existing(&db).unwrap();
                                assert!(store.doctor().unwrap().ok);
                            }
                            1 => {
                                let store = Store::open_readonly(&db).unwrap();
                                assert!(store.doctor().unwrap().ok);
                            }
                            2 => {
                                let store = Store::open(&db).unwrap();
                                assert!(store.doctor().unwrap().ok);
                            }
                            _ => {
                                assert!(Store::inspect(&db).unwrap().ok);
                            }
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap();
            }
        }

        std::fs::remove_dir_all(&root).unwrap();
    }
}
