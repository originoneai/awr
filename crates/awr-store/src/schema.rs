//! Validate the owned schema against the shipped migrations without repairing it.
use crate::{
    AGENT_AUTHORIZATION_SQL, CATALOG_SQL, CONTENT_REVIEWS_SQL, DELIVERY_DEPS_SQL, DOMAIN_SQL,
    DRAFT_WORK_SQL, ETA_CHECKPOINTS_SQL, MigrationInfo, OPERATION_READSET_RECEIPTS_SQL,
    RESPONSIBILITY_SQL, SCHEMA_VERSION, SEARCH_SQL, TEAM_HANDOFF_SQL, USAGE_TIME_SQL,
    WORKSTREAM_SESSIONS_SQL, WORKSTREAM_SQL,
};
use awr_core::{Error, Result};
use rusqlite::Connection;
use std::{collections::BTreeMap, sync::OnceLock};

type Definition = (String, String, String);
type Objects = BTreeMap<String, Definition>;
static EXPECTED: OnceLock<std::result::Result<Vec<Objects>, String>> = OnceLock::new();
const MIGRATION_NAMES: [&str; 14] = [
    "catalog",
    "domain",
    "search",
    "draft_work",
    "workstreams",
    "workstream_sessions",
    "content_reviews",
    "responsibility",
    "operation_readset_receipts",
    "agent_authorization",
    "team_handoff",
    "delivery_deps",
    "usage_time",
    "eta_checkpoints",
];

fn objects(conn: &Connection) -> rusqlite::Result<Objects> {
    conn.prepare("SELECT name,type,tbl_name,sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get::<_,String>(3)?.trim().to_owned()))))?
        .collect()
}

fn expected() -> Result<&'static Vec<Objects>> {
    EXPECTED
        .get_or_init(|| {
            let build = || -> rusqlite::Result<Vec<Objects>> {
                let conn = Connection::open_in_memory()?;
                conn.pragma_update(None, "legacy_alter_table", true)?;
                let mut versions = Vec::new();
                for sql in [
                    CATALOG_SQL,
                    DOMAIN_SQL,
                    SEARCH_SQL,
                    DRAFT_WORK_SQL,
                    WORKSTREAM_SQL,
                    WORKSTREAM_SESSIONS_SQL,
                    CONTENT_REVIEWS_SQL,
                    RESPONSIBILITY_SQL,
                    OPERATION_READSET_RECEIPTS_SQL,
                    AGENT_AUTHORIZATION_SQL,
                    TEAM_HANDOFF_SQL,
                    DELIVERY_DEPS_SQL,
                    USAGE_TIME_SQL,
                    ETA_CHECKPOINTS_SQL,
                ] {
                    conn.execute_batch(sql)?;
                    versions.push(objects(&conn)?);
                }
                Ok(versions)
            };
            build().map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| Error::Storage(format!("cannot validate shipped schema: {error}")))
}

pub(crate) fn issues_open(
    conn: &Connection,
    version: i64,
) -> std::result::Result<Vec<String>, crate::OpenError> {
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Ok(vec![format!("unsupported schema version {version}")]);
    }
    let observed = objects(conn).map_err(crate::OpenError::from_sqlite)?;
    let expected = &expected()?[(version - 1) as usize];
    let mut issues = Vec::new();
    for (name, definition) in expected {
        match observed.get(name) {
            None => issues.push(format!("missing {} {name}", definition.0)),
            Some(actual) if actual != definition => {
                issues.push(format!("definition mismatch for {} {name}", definition.0));
            }
            _ => {}
        }
    }
    // Extra objects are allowed, e.g. diagnostic indexes. They cannot substitute
    // for or redefine an owned object. Never emit SQL definitions or row values.
    Ok(issues)
}

pub(crate) fn migrations_raw(conn: &Connection) -> rusqlite::Result<Vec<MigrationInfo>> {
    conn.prepare("SELECT version,name,applied_at FROM schema_migrations ORDER BY version")?
        .query_map([], |row| {
            Ok(MigrationInfo {
                version: row.get(0)?,
                name: row.get(1)?,
                applied_at: row.get(2)?,
            })
        })?
        .collect()
}

pub(crate) fn catalog_matches(migrations: &[MigrationInfo], version: i64) -> bool {
    (1..=SCHEMA_VERSION).contains(&version)
        && migrations.len() == version as usize
        && migrations.iter().enumerate().all(|(index, entry)| {
            entry.version == index as i64 + 1 && entry.name == MIGRATION_NAMES[index]
        })
}

pub(crate) fn verify(conn: &Connection, version: i64) -> Result<()> {
    verify_open(conn, version).map_err(crate::OpenError::into_error)
}

pub(crate) fn verify_open(
    conn: &Connection,
    version: i64,
) -> std::result::Result<(), crate::OpenError> {
    let applied = migrations_raw(conn).map_err(crate::OpenError::from_sqlite)?;
    if !catalog_matches(&applied, version) {
        return Err(crate::OpenError::Other(Error::Storage(
            "migration catalog does not match the supported schema".into(),
        )));
    }
    let issues = issues_open(conn, version)?;
    if !issues.is_empty() {
        return Err(crate::OpenError::Other(Error::Storage(format!(
            "AWR schema mismatch: {}",
            issues.join("; ")
        ))));
    }
    Ok(())
}
