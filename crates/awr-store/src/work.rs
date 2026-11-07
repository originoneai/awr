use crate::{
    Store,
    catalog::{SOURCE_COLUMNS, id_at, revision_at, source_row},
    db_error,
    query::{projection, projections},
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};

// UNION, rather than UNION ALL, terminates cycles and deduplicates diamonds/duplicate edges.
fn reachable(scoped: bool) -> String {
    let visibility = if scoped {
        format!("{} AND EXISTS(SELECT 1 FROM work_items w JOIN sources ws ON ws.id=w.source_id AND ws.project_id=w.project_id
            WHERE w.project_id=e.project_id AND w.external_key=e.to_key AND w.active=1 AND ws.active=1 AND {})",
            crate::scoped_read::visible("edges", "e.id"),
            crate::scoped_read::visible("work_items", "w.id"))
    } else {
        "1".into()
    };
    format!("WITH RECURSIVE deps(from_key,to_key) AS (
        SELECT DISTINCT e.from_key,e.to_key FROM edges e JOIN sources s ON e.source_id=s.id AND e.project_id=s.project_id
        WHERE e.project_id=?1 AND e.active=1 AND s.active=1 AND e.from_kind='work_item'
        AND e.to_kind='work_item' AND e.relation='depends_on' AND (?3=0 OR e.required=1) AND ({visibility})
    ), reachable(key) AS (VALUES(?2) UNION SELECT d.to_key FROM deps d JOIN reachable r ON d.from_key=r.key)")
}

/// A boundary reference says nothing about the target's identity or existence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnavailableDependency {
    pub edge_id: Id,
    pub from_work_key: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScopedDependencyGraph {
    pub graph: DependencyGraph,
    pub unavailable_dependencies: Vec<UnavailableDependency>,
}

pub(crate) fn unavailable_dependencies(
    conn: &Connection,
    project: Id,
    key: &str,
    required_only: bool,
) -> Result<Vec<UnavailableDependency>> {
    let reachable = reachable(true);
    conn.prepare(&format!("{reachable} SELECT e.id,e.from_key FROM edges e
        JOIN sources s ON s.id=e.source_id AND s.project_id=e.project_id
        JOIN reachable r ON r.key=e.from_key
        WHERE e.project_id=?1 AND e.active=1 AND s.active=1
        AND e.from_kind='work_item' AND e.to_kind='work_item'
        AND e.relation='depends_on' AND (?3=0 OR e.required=1)
        AND NOT EXISTS(SELECT 1 FROM work_items w JOIN sources ws ON ws.id=w.source_id AND ws.project_id=w.project_id
            WHERE w.project_id=e.project_id AND w.external_key=e.to_key AND w.active=1 AND ws.active=1 AND {})
        ORDER BY e.from_key,e.id", crate::scoped_read::visible("work_items", "w.id")))
        .map_err(db_error)?.query_map(params![project.to_string(),key,required_only], |r| Ok(UnavailableDependency {
            edge_id: id_at(r,0)?, from_work_key:r.get(1)?,
        })).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
}

pub(crate) fn graph(
    conn: &Connection,
    project: Id,
    key: &str,
    required_only: bool,
    revision: Revision,
    scoped: bool,
) -> Result<DependencyGraph> {
    let reachable = reachable(scoped);
    let keys = conn
        .prepare(&format!(
            "{reachable} SELECT key FROM reachable ORDER BY key"
        ))
        .map_err(db_error)?
        .query_map(params![project.to_string(), key, required_only], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let cycle_keys = conn
        .prepare(&format!(
            "{reachable}, paths(origin,node) AS (
        SELECT d.from_key,d.to_key FROM deps d JOIN reachable r ON d.from_key=r.key
        UNION SELECT p.origin,d.to_key FROM paths p JOIN deps d ON p.node=d.from_key
    ) SELECT origin FROM paths WHERE origin=node ORDER BY origin"
        ))
        .map_err(db_error)?
        .query_map(params![project.to_string(), key, required_only], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let mut dependencies = Vec::new();
    let mut missing_keys = Vec::new();
    for dependency in keys.iter().filter(|k| *k != key) {
        match projection(conn, project, EntityKind::WorkItem, dependency) {
            Ok(work) => dependencies.push(work),
            Err(Error::NotFound(_)) => missing_keys.push(dependency.clone()),
            Err(error) => return Err(error),
        }
    }
    let columns = SOURCE_COLUMNS
        .split(',')
        .map(|c| format!("s.{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let edge_visibility = if scoped {
        crate::scoped_read::visible("edges", "e.id")
    } else {
        "1".into()
    };
    let edges=conn.prepare(&format!("{reachable} SELECT {columns},e.id,e.from_key,e.to_key,e.required,e.revision,e.source_ref_json FROM edges e
        JOIN sources s ON s.id=e.source_id AND s.project_id=e.project_id JOIN reachable r ON e.from_key=r.key
        WHERE e.project_id=?1 AND e.active=1 AND s.active=1 AND e.from_kind='work_item' AND e.to_kind='work_item'
          AND e.relation='depends_on' AND (?3=0 OR e.required=1) AND ({edge_visibility}) AND e.to_key IN (SELECT key FROM reachable) ORDER BY e.from_key,e.to_key,e.id")).map_err(db_error)?
        .query_map(params![project.to_string(),key,required_only],|row| {
            let source_ref=serde_json::from_str(&row.get::<_,String>(16)?).map_err(|error|
                rusqlite::Error::FromSqlConversionFailure(16,rusqlite::types::Type::Text,Box::new(error)))?;
            Ok(Projected { item:Edge {id:id_at(row,11)?,project_id:project,from_kind:EntityKind::WorkItem,from_key:row.get(12)?,relation:"depends_on".into(),to_kind:EntityKind::WorkItem,to_key:row.get(13)?,required:row.get(14)?,revision:revision_at(row,15)?,source_ref},source:source_row(row)?,project_revision:revision })
        }).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
    Ok(DependencyGraph {
        work_item_key: key.into(),
        required_only,
        dependencies,
        edges,
        missing_keys,
        cycle_keys,
        project_revision: revision,
    })
}

pub(crate) fn active_claims(
    conn: &Connection,
    project: Id,
    work: Id,
    _branch: Option<Id>,
    at: i64,
) -> Result<Vec<Claim>> {
    conn.prepare(
        "SELECT id,session_id,agent_id,branch_id,status,acquired_at,expires_at,released_at,revision
        FROM claims WHERE project_id=?1 AND work_item_id=?2 AND status='active'
          AND released_at IS NULL AND (expires_at IS NULL OR expires_at>?3) ORDER BY id",
    )
    .map_err(db_error)?
    .query_map(params![project.to_string(), work.to_string(), at], |row| {
        let branch_id = if row.get::<_, Option<String>>(3)?.is_some() {
            Some(id_at(row, 3)?)
        } else {
            None
        };
        Ok(Claim {
            id: id_at(row, 0)?,
            project_id: project,
            work_item_id: work,
            session_id: id_at(row, 1)?,
            agent_id: row.get(2)?,
            branch_id,
            status: row.get(4)?,
            acquired_at: row.get(5)?,
            expires_at: row.get(6)?,
            released_at: row.get(7)?,
            revision: revision_at(row, 8)?,
        })
    })
    .map_err(db_error)?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(db_error)
}

fn check_context(conn: &Connection, project: Id, branch: Option<Id>, at: i64) -> Result<Revision> {
    if at < 0 {
        return Err(Error::InvalidInput(
            "evaluation time cannot be negative".into(),
        ));
    }
    let revision = conn
        .query_row(
            "SELECT project_revision FROM projects WHERE id=?1",
            [project.to_string()],
            |r| revision_at(r, 0),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| Error::NotFound(format!("project {project}")))?;
    if let Some(branch) = branch {
        let found:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM branches WHERE project_id=?1 AND id=?2 AND status='active')",params![project.to_string(),branch.to_string()],|r|r.get(0)).map_err(db_error)?;
        if !found {
            return Err(Error::NotFound(format!("active branch {branch}")));
        }
    }
    Ok(revision)
}

pub(crate) fn readiness(
    conn: &Connection,
    project: Id,
    key: &str,
    branch: Option<Id>,
    at: i64,
    revision: Revision,
) -> Result<WorkReadiness> {
    let work: Projected<WorkItem> = projection(conn, project, EntityKind::WorkItem, key)?;
    let dependencies = graph(conn, project, key, true, revision, false)?;
    let active_claims = active_claims(conn, project, work.item.meta.id, branch, at)?;
    evaluate_readiness(conn, project, work, dependencies, active_claims, branch, at)
}

fn evaluate_readiness(
    conn: &Connection,
    project: Id,
    work: Projected<WorkItem>,
    dependencies: DependencyGraph,
    active_claims: Vec<Claim>,
    branch: Option<Id>,
    at: i64,
) -> Result<WorkReadiness> {
    let key = work.item.meta.external_key.as_str();
    let mut diagnostics = BTreeSet::new();
    let mut add = |code: &str, work_key: &str, detail: String| {
        diagnostics.insert(ReadinessDiagnostic {
            code: code.into(),
            work_item_key: work_key.into(),
            detail,
        });
    };
    if !matches!(work.item.status, WorkStatus::Planned | WorkStatus::Ready) {
        add(
            if work.item.status == WorkStatus::Unknown {
                "unknown_status"
            } else {
                "status_not_selectable"
            },
            key,
            format!("source status is {}", work.item.raw_status),
        );
    }
    for (code, detail) in crate::admission::diagnostics(conn, project, &work, branch)? {
        add(&code, key, detail);
    }
    if work.item.archived {
        add(
            "work_archived",
            key,
            "Restore this source record before selecting it for execution".into(),
        );
    }
    if let Some(blocker) = work.item.blocker.as_ref().filter(|s| !s.trim().is_empty()) {
        add("active_blocker", key, blocker.clone());
    }
    for item in std::iter::once(&work).chain(dependencies.dependencies.iter()) {
        if item.source.freshness != Freshness::Fresh {
            add(
                "source_not_fresh",
                &item.item.meta.external_key,
                format!("source {} is {:?}", item.source.id, item.source.freshness),
            );
        }
    }
    for edge in &dependencies.edges {
        if edge.source.freshness != Freshness::Fresh {
            add(
                "source_not_fresh",
                &edge.item.from_key,
                format!(
                    "dependency source {} is {:?}",
                    edge.source.id, edge.source.freshness
                ),
            );
        }
    }
    for missing in &dependencies.missing_keys {
        add(
            "missing_dependency",
            missing,
            "required dependency is absent from active source projections".into(),
        );
    }
    for cycle in &dependencies.cycle_keys {
        add(
            "dependency_cycle",
            cycle,
            "required dependency participates in a cycle".into(),
        );
    }
    for dependency in &dependencies.dependencies {
        if dependency.item.archived || dependency.item.status != WorkStatus::Completed {
            add(
                if dependency.item.status == WorkStatus::Unknown {
                    "unknown_status"
                } else {
                    "dependency_not_completed"
                },
                &dependency.item.meta.external_key,
                format!(
                    "required dependency has source status {}",
                    dependency.item.raw_status
                ),
            );
        }
    }
    for claim in &active_claims {
        add(
            "active_claim",
            key,
            format!(
                "claimed by {} in session {}",
                claim.agent_id, claim.session_id
            ),
        );
    }
    Ok(WorkReadiness {
        ready: diagnostics.is_empty(),
        work,
        dependencies,
        active_claims,
        diagnostics: diagnostics.into_iter().collect(),
        branch_id: branch,
        evaluated_at: at,
    })
}

impl Store {
    /// Source-declared work dependency associations, including source references for diagnosis.
    pub fn work_dependency_links(&self, project: Id) -> Result<Vec<Edge>> {
        self.project(project)?;
        dependency_links(&self.conn, project)
    }
    /// Source-declared work-to-goal associations, including source references for diagnosis.
    pub fn work_goal_links(&self, project: Id) -> Result<Vec<Edge>> {
        self.project(project)?;
        goal_links(&self.conn, project)
    }
    /// Source state is shared across branches: archive changes require no active executor.
    pub fn ensure_work_unoccupied(&self, project: Id, work: Id) -> Result<()> {
        self.project(project)?;
        let claimed: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM claims WHERE project_id=?1 AND work_item_id=?2 AND status='active' AND released_at IS NULL AND (expires_at IS NULL OR expires_at>?3))", rusqlite::params![project.to_string(), work.to_string(), now_millis()?], |r|r.get(0)).map_err(db_error)?;
        if claimed {
            return Err(Error::ClaimConflict(
                "release the active execution before changing its planning contract or archive state".into(),
            ));
        }
        Ok(())
    }
    pub fn work_item(&self, project: Id, key: &str) -> Result<Projected<WorkItem>> {
        projection(&self.conn, project, EntityKind::WorkItem, key)
    }
    pub fn work_items(&self, project: Id) -> Result<Vec<Projected<WorkItem>>> {
        self.project(project)?;
        projections(&self.conn, project, EntityKind::WorkItem, None)
    }
    pub fn dependency_closure(
        &self,
        project: Id,
        key: &str,
        required_only: bool,
    ) -> Result<DependencyGraph> {
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let work: Projected<WorkItem> = projection(&tx, project, EntityKind::WorkItem, key)?;
        let result = graph(
            &tx,
            project,
            key,
            required_only,
            work.project_revision,
            false,
        )?;
        tx.commit().map_err(db_error)?;
        Ok(result)
    }
    /// None evaluates the project-wide (unbranched) claim scope; callers pass the active branch explicitly.
    pub fn work_readiness(
        &self,
        project: Id,
        key: &str,
        branch: Option<Id>,
        at: i64,
    ) -> Result<WorkReadiness> {
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let revision = check_context(&tx, project, branch, at)?;
        let result = readiness(&tx, project, key, branch, at, revision)?;
        tx.commit().map_err(db_error)?;
        Ok(result)
    }
    /// Evaluate a graph in one read transaction, retaining each dependency expansion only for its callback.
    pub fn map_work_readiness<T>(
        &self,
        project: Id,
        keys: &[String],
        branch: Option<Id>,
        at: i64,
        mut map: impl FnMut(WorkReadiness) -> Result<T>,
    ) -> Result<Vec<T>> {
        if keys.len() > MAX_WORK_GRAPH_NODES {
            return Err(Error::BudgetExceeded {
                required: keys.len(),
                budget: MAX_WORK_GRAPH_NODES,
            });
        }
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let revision = check_context(&tx, project, branch, at)?;
        let works: BTreeMap<String, Projected<WorkItem>> =
            projections::<WorkItem>(&tx, project, EntityKind::WorkItem, None)?
                .into_iter()
                .map(|work| (work.item.meta.external_key.clone(), work))
                .collect();
        let sources: BTreeMap<_, _> = self
            .sources(project)?
            .into_iter()
            .map(|source| (source.id, source))
            .collect();
        let edges: Vec<_> = dependency_links(&tx, project)?
            .into_iter()
            .filter(|edge| edge.required)
            .map(|edge| {
                let source = sources
                    .get(&edge.source_ref.source_id)
                    .ok_or_else(|| Error::Storage("dependency source is absent".into()))?;
                Ok(Projected {
                    item: edge,
                    source: source.clone(),
                    project_revision: revision,
                })
            })
            .collect::<Result<_>>()?;
        let mut links: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for edge in &edges {
            links
                .entry(edge.item.from_key.clone())
                .or_default()
                .insert(edge.item.to_key.clone());
        }
        let mut all_keys = BTreeSet::new();
        let mut pending = keys.to_vec();
        while let Some(key) = pending.pop() {
            if all_keys.insert(key.clone()) {
                if all_keys.len() > MAX_WORK_GRAPH_NODES {
                    return Err(Error::BudgetExceeded {
                        required: all_keys.len(),
                        budget: MAX_WORK_GRAPH_NODES,
                    });
                }
                if let Some(children) = links.get(&key) {
                    pending.extend(children.iter().cloned());
                }
            }
        }
        let edges: Vec<_> = edges
            .into_iter()
            .filter(|edge| {
                all_keys.contains(&edge.item.from_key) && all_keys.contains(&edge.item.to_key)
            })
            .collect();
        let mut closures = BTreeMap::new();
        for key in &all_keys {
            let mut reachable = BTreeSet::new();
            let mut pending = vec![key.clone()];
            while let Some(current) = pending.pop() {
                if reachable.insert(current.clone()) {
                    if let Some(children) = links.get(&current) {
                        pending.extend(children.iter().cloned());
                    }
                }
            }
            closures.insert(key.clone(), reachable);
        }
        let cyclic: BTreeSet<_> = all_keys
            .iter()
            .filter(|key| {
                links.get(*key).is_some_and(|children| {
                    children.iter().any(|child| closures[child].contains(*key))
                })
            })
            .cloned()
            .collect();
        let mut result = Vec::with_capacity(keys.len());
        for key in keys {
            let work = works
                .get(key)
                .ok_or_else(|| Error::NotFound(format!("work {key}")))?;
            let reachable = &closures[key];
            let dependencies = DependencyGraph {
                work_item_key: key.clone(),
                required_only: true,
                dependencies: reachable
                    .iter()
                    .filter(|dependency| *dependency != key)
                    .filter_map(|dependency| works.get(dependency).cloned())
                    .collect(),
                edges: edges
                    .iter()
                    .filter(|edge| {
                        reachable.contains(&edge.item.from_key)
                            && reachable.contains(&edge.item.to_key)
                    })
                    .cloned()
                    .collect(),
                missing_keys: reachable
                    .iter()
                    .filter(|dependency| *dependency != key && !works.contains_key(*dependency))
                    .cloned()
                    .collect(),
                cycle_keys: reachable.intersection(&cyclic).cloned().collect(),
                project_revision: revision,
            };
            let claims = active_claims(&tx, project, work.item.meta.id, branch, at)?;
            result.push(map(evaluate_readiness(
                &tx,
                project,
                work.clone(),
                dependencies,
                claims,
                branch,
                at,
            )?)?);
        }
        tx.commit().map_err(db_error)?;
        Ok(result)
    }
    pub fn ready_work(&self, project: Id, branch: Option<Id>, at: i64) -> Result<ReadyReport> {
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        let revision = check_context(&tx, project, branch, at)?;
        let works: Vec<Projected<WorkItem>> =
            projections(&tx, project, EntityKind::WorkItem, None)?;
        let mut ready = Vec::new();
        let mut blocked = Vec::new();
        for work in works {
            if work.item.archived {
                continue;
            }
            if matches!(
                work.item.status,
                WorkStatus::Completed | WorkStatus::Cancelled
            ) {
                continue;
            }
            let result = readiness(
                &tx,
                project,
                &work.item.meta.external_key,
                branch,
                at,
                revision,
            )?;
            if result.ready {
                ready.push(result)
            } else {
                blocked.push(result)
            }
        }
        // Stable ordering: priority then score (descending), then external key.
        let order = |w: &WorkReadiness| {
            (
                w.work.item.priority.clone().unwrap_or("~".into()),
                std::cmp::Reverse(w.work.item.score),
                w.work.item.meta.external_key.clone(),
            )
        };
        ready.sort_by_key(order);
        blocked.sort_by_key(order);
        tx.commit().map_err(db_error)?;
        Ok(ReadyReport {
            ready,
            blocked,
            project_revision: revision,
            branch_id: branch,
            evaluated_at: at,
        })
    }
}

pub(crate) fn dependency_links(conn: &Connection, project: Id) -> Result<Vec<Edge>> {
    conn.prepare("SELECT e.id,e.from_key,e.to_key,e.required,e.revision,e.source_ref_json FROM edges e JOIN sources s ON e.source_id=s.id AND e.project_id=s.project_id WHERE e.project_id=?1 AND e.active=1 AND s.active=1 AND e.from_kind='work_item' AND e.to_kind='work_item' AND e.relation='depends_on' ORDER BY e.from_key,e.to_key,e.id").map_err(db_error)?
            .query_map([project.to_string()], |row| {
                let source_ref = serde_json::from_str(&row.get::<_, String>(5)?).map_err(|e| rusqlite::Error::FromSqlConversionFailure(5,rusqlite::types::Type::Text,Box::new(e)))?;
                Ok(Edge { id:id_at(row,0)?, project_id:project, from_kind:EntityKind::WorkItem, from_key:row.get(1)?, relation:"depends_on".into(), to_kind:EntityKind::WorkItem, to_key:row.get(2)?, required:row.get(3)?, revision:revision_at(row,4)?, source_ref })
            }).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
}

pub(crate) fn goal_links(conn: &Connection, project: Id) -> Result<Vec<Edge>> {
    conn.prepare("SELECT e.id,e.from_key,e.to_key,e.required,e.revision,e.source_ref_json FROM edges e JOIN sources s ON e.source_id=s.id AND e.project_id=s.project_id WHERE e.project_id=?1 AND e.active=1 AND s.active=1 AND e.from_kind='work_item' AND e.to_kind='goal' AND e.relation='supports' ORDER BY e.from_key,e.to_key,e.id").map_err(db_error)?
            .query_map([project.to_string()], |row| {
                let source_ref = serde_json::from_str(&row.get::<_, String>(5)?).map_err(|e| rusqlite::Error::FromSqlConversionFailure(5,rusqlite::types::Type::Text,Box::new(e)))?;
                Ok(Edge { id:id_at(row,0)?, project_id:project, from_kind:EntityKind::WorkItem, from_key:row.get(1)?, relation:"supports".into(), to_kind:EntityKind::Goal, to_key:row.get(2)?, required:row.get(3)?, revision:revision_at(row,4)?, source_ref })
            }).map_err(db_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
}
