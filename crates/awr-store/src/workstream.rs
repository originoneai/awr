//! Scope projections share the source transaction. None of these operations
//! authenticates a caller or replaces the execution guards in the runtime.
use crate::{Store, db_error};
use awr_core::{
    Error, Id, Result, Source, SourceRef, Workstream, WorkstreamCatalog, WorkstreamProjection,
    WorkstreamWorkBinding, validate_workstream_ownership,
};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn migrate_legacy(conn: &Connection) -> Result<()> {
    let projects = conn
        .prepare("SELECT id FROM projects ORDER BY id")
        .map_err(db_error)?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for project in projects {
        ensure_legacy(conn, &project)?;
    }
    Ok(())
}

pub(crate) fn ensure_legacy(conn: &Connection, project: &str) -> Result<()> {
    let mode: Option<String> = conn
        .query_row(
            "SELECT mode FROM workstream_catalogs WHERE project_id=?1",
            [project],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if mode.as_deref() == Some("source") {
        return Ok(());
    }
    let catalog = WorkstreamCatalog::legacy(project)?;
    let stream = &catalog.workstreams[0];
    if mode.is_none() {
        conn.execute("INSERT INTO workstream_catalogs(project_id,version,mode,legacy_default,revision) VALUES(?1,1,'legacy',?2,1)",
            params![project,stream.id.to_string()]).map_err(db_error)?;
        conn.execute("INSERT INTO workstreams(project_id,id,external_key,active,payload_json) VALUES(?1,?2,?3,1,?4)",
            params![project,stream.id.to_string(),stream.external_key,serde_json::to_string(stream)?]).map_err(db_error)?;
    }
    // Preserve historical bindings as well as all pre-existing object IDs.
    conn.execute(
        "INSERT INTO workstream_ownership(project_id,work_item_id,workstream_id,revision)
        SELECT project_id,id,?2,1 FROM work_items WHERE project_id=?1
        ON CONFLICT(project_id,work_item_id) DO NOTHING",
        params![project, stream.id.to_string()],
    )
    .map_err(db_error)?;
    Ok(())
}

pub(crate) fn recorded_catalog(conn: &Connection, project: &str) -> Result<WorkstreamCatalog> {
    let (version, legacy_default): (u32, Option<String>) = conn
        .query_row(
            "SELECT version,legacy_default FROM workstream_catalogs WHERE project_id=?1",
            [project],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| Error::NotFound("workstream catalog".into()))?;
    let rows = conn.prepare("SELECT payload_json FROM workstreams WHERE project_id=?1 AND active=1 ORDER BY external_key").map_err(db_error)?
        .query_map([project], |row| row.get::<_, String>(0)).map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
    let catalog = WorkstreamCatalog {
        version,
        project_id: project.into(),
        legacy_default: legacy_default
            .map(|id| {
                id.parse()
                    .map_err(|_| Error::Storage("invalid workstream default".into()))
            })
            .transpose()?,
        workstreams: rows
            .into_iter()
            .map(|value| serde_json::from_str(&value).map_err(Error::from))
            .collect::<Result<_>>()?,
    };
    catalog.validate()?;
    Ok(catalog)
}

pub(crate) fn require_fresh_catalog(conn: &Connection, project: &str) -> Result<()> {
    let stale: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workstream_catalogs c
        LEFT JOIN sources s ON s.project_id=c.project_id AND s.id=c.source_id
        WHERE c.project_id=?1 AND c.mode='source' AND
        (s.id IS NULL OR s.active!=1 OR s.freshness!='fresh'
        OR s.fingerprint!=json_extract(c.source_ref_json,'$.source_fingerprint')
        OR s.revision!=json_extract(c.source_ref_json,'$.source_revision')))",
            [project],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if stale {
        return Err(Error::SourceStale(
            "workstream authority requires a fresh source projection".into(),
        ));
    }
    let missing_goal: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM workstreams w,json_each(w.payload_json,'$.goal_keys') r
        WHERE w.project_id=?1 AND w.active=1 AND NOT EXISTS(SELECT 1 FROM goals g
        JOIN sources s ON s.project_id=g.project_id AND s.id=g.source_id
        WHERE g.project_id=w.project_id AND g.external_key=r.value AND g.active=1 AND s.active=1 AND s.freshness='fresh'))",
        [project], |row| row.get(0)).map_err(db_error)?;
    if missing_goal {
        return Err(Error::SourceStale(
            "workstream goal references require current authoritative projections".into(),
        ));
    }
    Ok(())
}

impl Store {
    /// Explicit source enablement is distinct from a legacy catalog with one scope.
    pub fn workstreams_enabled(&self, project: Id) -> Result<bool> {
        self.workstream_catalog(project)?;
        self.conn
            .query_row(
                "SELECT mode='source' FROM workstream_catalogs WHERE project_id=?1",
                [project.to_string()],
                |r| r.get(0),
            )
            .map_err(db_error)
    }

    /// Internal domain query. Service adapters must authorize access before
    /// exposing this project-wide catalog to a caller.
    pub fn workstream_catalog(&self, project: Id) -> Result<WorkstreamCatalog> {
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        require_fresh_catalog(&tx, &project.to_string())?;
        let catalog = recorded_catalog(&tx, &project.to_string())?;
        tx.commit().map_err(db_error)?;
        Ok(catalog)
    }

    pub fn workstream_binding(&self, project: Id, work: Id) -> Result<WorkstreamWorkBinding> {
        Ok(self.workstream_ownership(project, work)?.binding)
    }

    /// Capture the work's current ownership token before editing its source.
    pub fn workstream_ownership(
        &self,
        project: Id,
        work: Id,
    ) -> Result<awr_core::WorkstreamOwnership> {
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        require_fresh_catalog(&tx, &project.to_string())?;
        let (stream, revision): (Id, awr_core::Revision) = tx.query_row("SELECT workstream_id,revision FROM workstream_ownership WHERE project_id=?1 AND work_item_id=?2",
            params![project.to_string(),work.to_string()], |row| Ok((crate::catalog::id_at(row,0)?,crate::catalog::revision_at(row,1)?))).optional().map_err(db_error)?
            .ok_or_else(|| Error::NotFound("workstream ownership".into()))?;
        tx.commit().map_err(db_error)?;
        Ok(awr_core::WorkstreamOwnership {
            binding: WorkstreamWorkBinding {
                project_id: project.to_string(),
                work_item_id: work.to_string(),
                workstream_id: stream,
            },
            revision,
        })
    }
}

/// Runs after work upserts, before source fingerprint and event commit. A failure
/// rolls back the entire candidate, leaving the previous projection stale.
pub(crate) fn project(
    conn: &Connection,
    source: &Source,
    fingerprint: &str,
    projection: Option<&WorkstreamProjection>,
    moves: &[awr_core::WorkstreamMove],
) -> Result<()> {
    let project = source.project_id.to_string();
    ensure_legacy(conn, &project)?;
    let (mode, owner): (String, Option<String>) = conn
        .query_row(
            "SELECT mode,source_id FROM workstream_catalogs WHERE project_id=?1",
            [&project],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error)?;
    let Some(projection) = projection else {
        if !moves.is_empty() {
            return Err(Error::InvalidInput(
                "workstream moves require a complete workstream projection".into(),
            ));
        }
        if owner.as_deref() == Some(&source.id.to_string()) {
            return Err(Error::SourceConflict(
                "the authoritative ledger cannot drop its workstream declaration".into(),
            ));
        }
        if mode == "source" {
            let unbound: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM work_items w LEFT JOIN workstream_ownership o
                ON o.project_id=w.project_id AND o.work_item_id=w.id WHERE w.project_id=?1 AND w.active=1 AND o.work_item_id IS NULL)",
                [&project], |row| row.get(0)).map_err(db_error)?;
            if unbound {
                return Err(Error::SourceConflict(
                    "new work requires an authoritative workstream binding".into(),
                ));
            }
        }
        return Ok(());
    };
    if source.adapter != "yaml-workstream-ledger-v1"
        || source.domain != "ledger"
        || source.role != "primary"
    {
        return Err(Error::SourceConflict(
            "workstream definitions require a primary versioned workstream ledger".into(),
        ));
    }
    if projection.catalog.project_id != project
        || owner
            .as_deref()
            .is_some_and(|id| id != source.id.to_string())
    {
        return Err(Error::SourceConflict(
            "workstream catalog belongs to a different project or source".into(),
        ));
    }
    let works = conn
        .prepare("SELECT id,source_id FROM work_items WHERE project_id=?1 AND active=1 ORDER BY id")
        .map_err(db_error)?
        .query_map([&project], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    if works.iter().any(|(_, id)| id != &source.id.to_string()) {
        return Err(Error::SourceConflict("this adapter requires one complete authoritative ledger; partition activation is not supported".into()));
    }
    validate_workstream_ownership(
        &projection.catalog,
        &works.into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
        &projection.ownership,
    )?;
    let previous = recorded_catalog(conn, &project)?;
    if mode == "source"
        && previous.workstreams.iter().any(|old| {
            !projection
                .catalog
                .workstreams
                .iter()
                .any(|s| s.id == old.id)
        })
    {
        return Err(Error::SourceConflict(
            "archive a workstream instead of deleting its retained identity".into(),
        ));
    }
    for stream in &projection.catalog.workstreams {
        for goal in &stream.goal_keys {
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM goals g JOIN sources s ON s.project_id=g.project_id AND s.id=g.source_id
                WHERE g.project_id=?1 AND g.external_key=?2 AND g.active=1 AND s.active=1 AND (s.id=?3 OR s.freshness='fresh'))",
                params![project,goal,source.id.to_string()], |row| row.get(0)).map_err(db_error)?;
            if !exists {
                return Err(Error::SourceConflict(
                    "workstream references an undeclared goal".into(),
                ));
            }
        }
        if let Some(old) = previous.workstreams.iter().find(|old| old.id == stream.id) {
            stream.validate_successor(old)?;
        }
        // Retired identities cannot be recycled under a different key.
        let retained: Option<String> = conn.query_row("SELECT payload_json FROM workstreams WHERE project_id=?1 AND (id=?2 OR external_key=?3)",
            params![project,stream.id.to_string(),stream.external_key], |row|row.get(0)).optional().map_err(db_error)?;
        if let Some(raw) = retained {
            let old: Workstream = serde_json::from_str(&raw)?;
            stream.validate_successor(&old)?;
        }
    }
    let mut requested = std::collections::BTreeMap::new();
    for movement in moves {
        if movement.from == movement.to
            || movement.expected_ownership_revision == 0
            || requested
                .insert(movement.work_item_id.as_str(), movement)
                .is_some()
        {
            return Err(Error::InvalidInput(
                "moves require unique work, distinct scopes and an ownership revision".into(),
            ));
        }
    }
    for binding in &projection.ownership {
        let old: Option<(String,u64)> = conn.query_row("SELECT workstream_id,revision FROM workstream_ownership WHERE project_id=?1 AND work_item_id=?2",
            params![project,binding.work_item_id], |row|Ok((row.get(0)?,crate::catalog::revision_at(row,1)?))).optional().map_err(db_error)?;
        let movement = requested.remove(binding.work_item_id.as_str());
        if let Some(movement) = movement {
            let (old_scope, revision) = old.as_ref().ok_or_else(|| {
                Error::SourceConflict("cannot move work without existing ownership".into())
            })?;
            if old_scope != &movement.from.to_string()
                || *revision != movement.expected_ownership_revision
                || binding.workstream_id != movement.to
            {
                return Err(Error::SourceConflict(
                    "move no longer matches the reviewed source and ownership revision".into(),
                ));
            }
            if projection.catalog.get(movement.to)?.state != awr_core::WorkstreamState::Active {
                return Err(awr_core::WorkstreamError::Inactive.into());
            }
            crate::workstream_runtime::require_movable(
                conn,
                source.project_id,
                &binding.work_item_id,
            )?;
        } else if old
            .as_ref()
            .is_some_and(|(id, _)| id != &binding.workstream_id.to_string())
        {
            let historical: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE project_id=?1 AND work_item_id=?2)
                OR EXISTS(SELECT 1 FROM evidence WHERE project_id=?1 AND work_item_id=?2)
                OR EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND work_item_id=?2)",
                    params![project, binding.work_item_id],
                    |row| row.get(0),
                )
                .map_err(db_error)?;
            if mode != "legacy" || historical {
                return Err(Error::SourceConflict(
                    "changing existing work ownership requires an explicit runtime migration"
                        .into(),
                ));
            }
        }
    }
    if !requested.is_empty() {
        return Err(Error::SourceConflict(
            "move set contains work absent from the candidate ownership".into(),
        ));
    }
    let source_ref = SourceRef {
        source_id: source.id,
        locator: source.locator.clone(),
        source_revision: source.revision + 1,
        source_fingerprint: fingerprint.into(),
        pointer: Some("/workstreams".into()),
        start_line: None,
        end_line: None,
        section_fingerprint: None,
    };
    conn.execute("UPDATE workstream_catalogs SET mode='source',legacy_default=?2,source_id=?3,source_ref_json=?4,revision=revision+1 WHERE project_id=?1",
        params![project,projection.catalog.legacy_default.map(|id|id.to_string()),source.id.to_string(),serde_json::to_string(&source_ref)?]).map_err(db_error)?;
    conn.execute(
        "UPDATE workstreams SET active=0 WHERE project_id=?1",
        [&project],
    )
    .map_err(db_error)?;
    for stream in &projection.catalog.workstreams {
        conn.execute("INSERT INTO workstreams(project_id,id,external_key,active,payload_json) VALUES(?1,?2,?3,1,?4)
            ON CONFLICT(project_id,id) DO UPDATE SET active=1,payload_json=excluded.payload_json",
            params![project,stream.id.to_string(),stream.external_key,serde_json::to_string(stream)?]).map_err(db_error)?;
    }
    for binding in &projection.ownership {
        conn.execute("INSERT INTO workstream_ownership(project_id,work_item_id,workstream_id,revision) VALUES(?1,?2,?3,1)
            ON CONFLICT(project_id,work_item_id) DO UPDATE SET workstream_id=excluded.workstream_id,
            revision=CASE WHEN workstream_ownership.workstream_id=excluded.workstream_id THEN workstream_ownership.revision ELSE workstream_ownership.revision+1 END",
            params![project,binding.work_item_id,binding.workstream_id.to_string()]).map_err(db_error)?;
    }
    Ok(())
}

/// Connection-level catalog reads for facts evaluated inside an open transaction. They apply the
/// same freshness rule as the `Store` queries, so a stale catalog is an error, never a skip.
pub(crate) fn enabled_at(conn: &Connection, project: Id) -> Result<bool> {
    let key = project.to_string();
    require_fresh_catalog(conn, &key)?;
    recorded_catalog(conn, &key)?;
    conn.query_row(
        "SELECT mode='source' FROM workstream_catalogs WHERE project_id=?1",
        [key],
        |r| r.get(0),
    )
    .map_err(db_error)
}

pub(crate) fn binding_at(conn: &Connection, project: Id, work: Id) -> Result<Id> {
    require_fresh_catalog(conn, &project.to_string())?;
    conn.query_row(
        "SELECT workstream_id FROM workstream_ownership WHERE project_id=?1 AND work_item_id=?2",
        params![project.to_string(), work.to_string()],
        |row| crate::catalog::id_at(row, 0),
    )
    .optional()
    .map_err(db_error)?
    .ok_or_else(|| Error::NotFound("workstream ownership".into()))
}

pub(crate) fn goal_keys_at(conn: &Connection, project: Id, stream: Id) -> Result<Vec<String>> {
    require_fresh_catalog(conn, &project.to_string())?;
    let catalog = recorded_catalog(conn, &project.to_string())?;
    Ok(catalog.get(stream)?.goal_keys.clone())
}
