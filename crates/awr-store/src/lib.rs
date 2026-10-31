//! SQLite persistence. Callers use domain operations, never an exposed SQL handle.
mod admission;
mod branch;
mod branch_close;
mod catalog;
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
pub use source_terminal::{FinishedWorkReconcile, ReconcileSkip, SOURCE_TERMINAL};
use std::{path::Path, time::Duration};
pub use work::{ScopedDependencyGraph, UnavailableDependency};
pub use workstream_eta::EtaIngestReceipt;
pub use workstream_usage::UsageIngestReceipt;

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
        _ => None,
    };
    if let Some(hint) = hint {
        detail.push_str("; ");
        detail.push_str(hint);
    }
    detail
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
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| db_error_at("open read-only", path, error))?;
        Self::configure(&conn)?;
        let owner: i64 = conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(db_error)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        if owner != APPLICATION_ID || version != SCHEMA_VERSION {
            return Err(Error::Storage(
                "read queries require a current AWR database; use doctor to inspect it".into(),
            ));
        }
        let store = Self {
            conn,
            runtime_lease,
        };
        store.verify_catalog()?;
        Ok(store)
    }

    /// Open an existing current-schema database for domain writes without creating or migrating it.
    pub fn open_existing(path: &Path) -> Result<Self> {
        let runtime_lease = RuntimeLease::shared(path, true)?;
        if !path.is_file() {
            return Err(Error::NotFound(format!("AWR database: {}", path.display())));
        }
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|error| db_error_at("open read-write", path, error))?;
        Self::configure(&conn)?;
        let owner: i64 = conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(db_error)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        if owner != APPLICATION_ID || version != SCHEMA_VERSION {
            return Err(Error::Storage(
                "writes require an existing current AWR database; inspect it before repair".into(),
            ));
        }
        let store = Self {
            conn,
            runtime_lease,
        };
        store.verify_catalog()?;
        Ok(store)
    }

    /// Create or reopen an AWR database. Refuse unrelated databases.
    pub fn open(path: &Path) -> Result<Self> {
        let lease = RuntimeLease::shared(path, true)?;
        let mut store = Self::initialize_connection(
            Connection::open(path).map_err(|error| db_error_at("create or open", path, error))?,
            true,
        )?;
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
        let conn =
            Connection::open_with_flags(&snapshot.database, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| {
                    db_error_at("open snapshot read-only", &snapshot.database, error)
                })?;
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

    fn initialize_connection(mut conn: Connection, persistent: bool) -> Result<Self> {
        Self::configure(&conn)?;
        let application_id: i64 = conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(db_error)?;
        let current: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        if application_id == 0 {
            let objects: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                    [],
                    |r| r.get(0),
                )
                .map_err(db_error)?;
            if objects != 0 || current != 0 {
                return Err(Error::Storage(
                    "database is not empty or owned by AWR".into(),
                ));
            }
        } else if application_id != APPLICATION_ID {
            return Err(Error::Storage(
                "database belongs to a different application".into(),
            ));
        }
        if current > SCHEMA_VERSION {
            return Err(Error::Storage(format!(
                "schema {current} is newer than supported {SCHEMA_VERSION}"
            )));
        }
        if current > 0 {
            schema::verify(&conn, current)?;
        }
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .map_err(db_error)?;
        if persistent && mode != "wal" {
            let actual: String = conn
                .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
                .map_err(db_error)?;
            if actual != "wal" {
                return Err(Error::Storage(format!(
                    "WAL could not be enabled: {actual}"
                )));
            }
        }
        if current < SCHEMA_VERSION {
            // SQLite table rebuild protocol: suppress FK actions only on this unexposed
            // connection, then check all references before committing and restore enforcement.
            conn.pragma_update(None, "foreign_keys", false)
                .map_err(db_error)?;
            conn.pragma_update(None, "legacy_alter_table", true)
                .map_err(db_error)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_error)?;
            let version: i64 = tx
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(db_error)?;
            if version > 0 {
                // Validate the actual base under the writer lock before adding
                // anything. A failed upgrade must not commit a partial new schema.
                schema::verify(&tx, version)?;
            }
            if version == 0 {
                tx.execute_batch(CATALOG_SQL).map_err(db_error)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(1,'catalog',?1)",
                    [now_millis()?],
                )
                .map_err(db_error)?;
                tx.pragma_update(None, "application_id", APPLICATION_ID)
                    .map_err(db_error)?;
            }
            if version < 2 {
                tx.execute_batch(DOMAIN_SQL).map_err(db_error)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(2,'domain',?1)",
                    [now_millis()?],
                )
                .map_err(db_error)?;
            }
            if version < 3 {
                tx.execute_batch(SEARCH_SQL).map_err(db_error)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(3,'search',?1)",
                    [now_millis()?],
                )
                .map_err(db_error)?;
            }
            if version < 4 {
                // Preserve user diagnostic indexes/triggers as well as owned schema objects.
                let extras:Vec<String> = tx.prepare("SELECT sql FROM sqlite_master WHERE tbl_name='work_items' AND type IN ('index','trigger') AND sql IS NOT NULL AND name NOT IN ('work_items_source','work_status') ORDER BY name").map_err(db_error)?
                    .query_map([],|row|row.get(0)).map_err(db_error)?.collect::<rusqlite::Result<_>>().map_err(db_error)?;
                tx.execute_batch(DRAFT_WORK_SQL).map_err(db_error)?;
                for sql in extras {
                    tx.execute_batch(&sql).map_err(db_error)?;
                }
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(4,'draft_work',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 5 {
                tx.execute_batch(WORKSTREAM_SQL).map_err(db_error)?;
                workstream::migrate_legacy(&tx)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(5,'workstreams',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 6 {
                tx.execute_batch(WORKSTREAM_SESSIONS_SQL)
                    .map_err(db_error)?;
                workstream_runtime::migrate(&tx)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(6,'workstream_sessions',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 7 {
                tx.execute_batch(CONTENT_REVIEWS_SQL).map_err(db_error)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(7,'content_reviews',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 8 {
                tx.execute_batch(RESPONSIBILITY_SQL).map_err(db_error)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(8,'responsibility',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 9 {
                tx.execute_batch(OPERATION_READSET_RECEIPTS_SQL)
                    .map_err(db_error)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,applied_at) VALUES(9,'operation_readset_receipts',?1)",
                    [now_millis()?],
                )
                .map_err(db_error)?;
            }
            if version < 10 {
                tx.execute_batch(AGENT_AUTHORIZATION_SQL)
                    .map_err(db_error)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(10,'agent_authorization',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 11 {
                tx.execute_batch(TEAM_HANDOFF_SQL).map_err(db_error)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(11,'team_handoff',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 12 {
                tx.execute_batch(DELIVERY_DEPS_SQL).map_err(db_error)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(12,'delivery_deps',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 13 {
                tx.execute_batch(USAGE_TIME_SQL).map_err(db_error)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(13,'usage_time',?1)",[now_millis()?]).map_err(db_error)?;
            }
            if version < 14 {
                tx.execute_batch(ETA_CHECKPOINTS_SQL).map_err(db_error)?;
                tx.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES(14,'eta_checkpoints',?1)",[now_millis()?]).map_err(db_error)?;
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(db_error)?;
            schema::verify(&tx, SCHEMA_VERSION)?;
            if tx
                .prepare("PRAGMA foreign_key_check")
                .map_err(db_error)?
                .exists([])
                .map_err(db_error)?
            {
                return Err(Error::Storage(
                    "migration would leave invalid foreign-key references".into(),
                ));
            }
            tx.commit().map_err(db_error)?;
            conn.pragma_update(None, "legacy_alter_table", false)
                .map_err(db_error)?;
            conn.pragma_update(None, "foreign_keys", true)
                .map_err(db_error)?;
        }
        let store = Self {
            conn,
            runtime_lease: None,
        };
        store.verify_catalog()?;
        Ok(store)
    }

    fn configure(conn: &Connection) -> Result<()> {
        conn.busy_timeout(Duration::from_millis(5000))
            .map_err(db_error)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(db_error)?;
        // REPLACE performs an implicit delete. Without recursive triggers SQLite
        // can bypass event_no_delete and overwrite an existing append-only receipt.
        conn.pragma_update(None, "recursive_triggers", true)
            .map_err(db_error)
    }

    fn verify_catalog(&self) -> Result<()> {
        schema::verify(&self.conn, SCHEMA_VERSION)
    }

    /// Inspect without creating, upgrading, or repairing the database.
    pub fn inspect(path: &Path) -> Result<DoctorReport> {
        let _lease = RuntimeLease::shared(path, false)?;
        if !path.is_file() {
            return Err(Error::NotFound(format!("AWR database: {}", path.display())));
        }
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| db_error_at("open read-only", path, error))?;
        Self::configure(&conn)?;
        Self {
            conn,
            runtime_lease: None,
        }
        .doctor()
    }

    pub fn doctor(&self) -> Result<DoctorReport> {
        let application_id: i64 = self
            .conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(db_error)?;
        if application_id != APPLICATION_ID {
            return Err(Error::Storage("not an AWR database".into()));
        }
        let schema_version = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        let integrity = self
            .conn
            .prepare("PRAGMA integrity_check")
            .map_err(db_error)?
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
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
            Err(_) => (
                0,
                Some("foreign-key check could not run for this schema".into()),
            ),
        };
        let mut schema_issues = schema::issues(&self.conn, schema_version)?;
        let migrations = match schema::migrations(&self.conn) {
            Ok(migrations) => migrations,
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
                .map_err(db_error)?,
            journal_mode: self
                .conn
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .map_err(db_error)?,
            foreign_keys: self
                .conn
                .pragma_query_value(None, "foreign_keys", |r| r.get(0))
                .map_err(db_error)?,
            busy_timeout_ms: self
                .conn
                .pragma_query_value(None, "busy_timeout", |r| r.get(0))
                .map_err(db_error)?,
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
        let mut target = Connection::open(path).map_err(db_error)?;
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
