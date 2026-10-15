//! Typed facts for context assembly. No underlying Store or writes escape.
use crate::{ScopedDependencyGraph, WorkstreamRead, db_error};
use awr_core::*;
use rusqlite::params;

impl WorkstreamRead {
    pub fn authority_source(&self) -> Result<Option<Source>> {
        let id: Option<String> = self
            .store
            .conn
            .query_row(
                "SELECT source_id FROM workstream_catalogs WHERE project_id=?1",
                [self.project.to_string()],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        id.map(|id| {
            self.store.source(
                self.project,
                id.parse()
                    .map_err(|_| Error::Storage("invalid authority source identity".into()))?,
            )
        })
        .transpose()
    }

    pub fn ownership(&self, work: &str) -> Result<WorkstreamOwnership> {
        let work = self.work_item(work)?;
        self.store
            .workstream_ownership(self.project, work.item.meta.id)
    }
    pub fn context_branch(&self, reference: &str) -> Result<Option<Id>> {
        let id =
            self.store
                .resolve_branch(self.project, reference)
                .map_err(|error| match error {
                    Error::NotFound(_) => Error::Workstream(WorkstreamError::AccessDenied),
                    e => e,
                })?;
        if let Some(id) = id {
            self.require("branches", id)?;
        }
        Ok(id)
    }
    pub fn session_recovery_revision(&self, id: Id) -> Result<Revision> {
        self.context_session(id)?;
        self.store.session_recovery_revision(self.project, id)
    }

    /// Facts are a narrowing selector, never an access grant. The compiler uses
    /// this after selecting its goals/rules/decisions/dependencies. Filtering
    /// precedes source folding, runtime counts and all limits.
    pub fn context_delta_events(
        &self,
        work: &str,
        branch: Option<Id>,
        after: Revision,
        event_limit: usize,
        entity_limit_per_source: usize,
        facts: &[(String, Id)],
    ) -> Result<crate::DeltaEvents> {
        let work = self.work_item(work)?;
        if let Some(id) = branch {
            self.require("branches", id)?;
        }
        if !self.explicit_workstreams()? {
            return self.store.delta_events(
                self.project,
                self.project_revision(),
                work.item.meta.id,
                branch,
                after,
                event_limit,
                entity_limit_per_source,
            );
        }
        let mut entities = std::collections::BTreeSet::new();
        for (kind, id) in facts {
            self.require(kind, *id)?;
            entities.insert((kind.clone(), *id));
        }
        let graph = self.dependency_closure(&work.item.meta.external_key, true)?;
        let mut work_keys = std::collections::BTreeMap::new();
        let mut work_generations = std::collections::BTreeMap::new();
        for selected in std::iter::once(&work).chain(&graph.graph.dependencies) {
            let id = selected.item.meta.id;
            let generation = self.store.conn.query_row(
                "SELECT coalesce(max(at_revision),0) FROM temp.awr_read_moves WHERE work_item_id=?1",
                [id.to_string()], |r| crate::catalog::revision_at(r,0),
            ).map_err(db_error)?;
            entities.insert(("work_items".into(), id));
            work_keys.insert(selected.item.meta.external_key.clone(), generation);
            work_generations.insert(id, generation);
        }
        for edge in graph.graph.edges {
            entities.insert(("edges".into(), edge.item.id));
        }
        self.store.delta_events_selected(
            self.project,
            self.project_revision(),
            work.item.meta.id,
            branch,
            after,
            event_limit,
            entity_limit_per_source,
            Some(&crate::delta::DeltaVisibility {
                entities,
                work_keys,
                work_generations,
            }),
        )
    }
    fn explicit_workstreams(&self) -> Result<bool> {
        self.store
            .conn
            .query_row(
                "SELECT mode!='legacy' FROM workstream_catalogs WHERE project_id=?1",
                [self.project.to_string()],
                |r| r.get(0),
            )
            .map_err(db_error)
    }

    pub fn goals(&self) -> Result<Vec<Projected<Goal>>> {
        crate::query::scoped_projections(&self.store.conn, self.project, EntityKind::Goal)
    }
    /// Goal keys declared on this work item. Visibility of the goal body is applied by the caller.
    pub fn goal_links_for(&self, work: &str) -> Result<Vec<String>> {
        let work = self.work_item(work)?;
        let mut statement = self.store.conn.prepare(
            "SELECT e.to_key FROM edges e JOIN sources s ON s.id=e.source_id AND s.project_id=e.project_id WHERE e.project_id=?1 AND e.active=1 AND s.active=1 AND e.from_kind='work_item' AND e.from_key=?2 AND e.to_kind='goal' AND e.relation='supports' ORDER BY e.to_key",
        ).map_err(db_error)?;
        statement
            .query_map(
                params![self.project.to_string(), work.item.meta.external_key],
                |row| row.get(0),
            )
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<String>>>()
            .map_err(db_error)
    }
    /// Applicability remains the compiler's responsibility; unknown hard rules survive.
    pub fn rules(&self) -> Result<Vec<Projected<Rule>>> {
        crate::query::scoped_projections(&self.store.conn, self.project, EntityKind::Rule)
    }
    pub fn work_items(&self) -> Result<Vec<Projected<WorkItem>>> {
        crate::query::scoped_projections(&self.store.conn, self.project, EntityKind::WorkItem)
    }
    /// Walk only visible tasks. An inaccessible or missing target becomes an
    /// opaque boundary; neither its status nor any of its descendants is read.
    /// This is descriptive context, never a versioned delivery/admission proof.
    pub fn dependency_closure(
        &self,
        work: &str,
        required_only: bool,
    ) -> Result<ScopedDependencyGraph> {
        let work = self.work_item(work)?;
        let key = &work.item.meta.external_key;
        let explicit = self.explicit_workstreams()?;
        let graph = crate::work::graph(
            &self.store.conn,
            self.project,
            key,
            required_only,
            self.project_revision(),
            explicit,
        )?;
        let unavailable_dependencies = if explicit {
            crate::work::unavailable_dependencies(
                &self.store.conn,
                self.project,
                key,
                required_only,
            )?
        } else {
            vec![]
        };
        Ok(ScopedDependencyGraph {
            graph,
            unavailable_dependencies,
        })
    }
    pub fn decisions_for_work(
        &self,
        work: &str,
        paths: Option<&[String]>,
    ) -> Result<Vec<RelatedDecision>> {
        let work = self.work_item(work)?;
        self.store.decisions_for_work_scoped(
            self.project,
            &work.item.meta.external_key,
            paths,
            true,
        )
    }
    /// Current ownership context only. Historical evidence remains available
    /// through evidence(); movement never relabels or deletes its receipt.
    pub fn context_evidence_for_work(
        &self,
        work: &str,
        source_sha: Option<&str>,
        branch: Option<Id>,
    ) -> Result<Vec<EvidenceAssessment>> {
        let work = self.work_item(work)?;
        if let Some(id) = branch {
            self.require("branches", id)?;
        }
        let assessments = self.store.evidence_for_work_scoped(
            self.project,
            &work.item.meta.external_key,
            source_sha,
            branch,
            true,
        )?;
        if !self.explicit_workstreams()? {
            return Ok(assessments);
        }
        let mut current = Vec::new();
        for assessment in assessments {
            let in_generation = assessment.evidence.source.is_some() || self.store.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM events e WHERE e.project_id=?1 AND e.event_type='evidence.recorded'
                 AND json_extract(e.payload_json,'$.evidence_id')=?2
                 AND NOT EXISTS(SELECT 1 FROM temp.awr_read_moves m WHERE m.work_item_id=?3 AND m.at_revision>e.project_revision))",
                params![self.project.to_string(),assessment.evidence.item.id.to_string(),work.item.meta.id.to_string()], |r|r.get::<_,bool>(0),
            ).map_err(db_error)?;
            if in_generation {
                current.push(assessment);
            }
        }
        Ok(current)
    }
    /// History remains accessible through session(); use this guard before
    /// treating a saved session/checkpoint as the current task's context.
    pub fn context_session(&self, id: Id) -> Result<Session> {
        let session = self.session(id)?;
        if !self.current_session_ownership(&session)? {
            return Err(WorkstreamError::BindingMismatch.into());
        }
        Ok(session)
    }
    fn current_session_ownership(&self, session: &Session) -> Result<bool> {
        let saved = self.store.session_workstream(self.project, session.id)?;
        if saved.work_item_id != session.work_item_id.map(|id| id.to_string()) {
            return Ok(false);
        }
        let Some(work) = session.work_item_id else {
            return Ok(true);
        };
        let current = self.store.workstream_ownership(self.project, work)?;
        Ok(saved.workstream_id == Some(current.binding.workstream_id)
            && saved.ownership_revision == Some(current.revision))
    }
    /// Visibility and current ownership are checked before the ambiguity limit.
    pub fn select_active_session(
        &self,
        work: Option<Id>,
        agent: Option<&str>,
        branch: Option<Id>,
    ) -> Result<Session> {
        if let Some(id) = work {
            self.require("work_items", id)?;
        }
        if let Some(id) = branch {
            self.require("branches", id)?;
        }
        self.store
            .select_active_session_scoped(self.project, None, work, agent, branch, true)
    }
    pub fn context_checkpoint(&self, id: Id, work: &str, branch: Option<Id>) -> Result<Checkpoint> {
        let cp = self.checkpoint(id)?;
        let owner = self.context_session(cp.session_id)?;
        let work = self.work_item(work)?;
        if owner.work_item_id != Some(work.item.meta.id) || owner.branch_id != branch {
            return Err(WorkstreamError::BindingMismatch.into());
        }
        Ok(cp)
    }
    pub fn context_recovery_checkpoint(&self, session: Id) -> Result<Option<Checkpoint>> {
        let current = self.context_session(session)?;
        let checkpoint = self.recovery_checkpoint(session)?;
        if let Some(cp) = &checkpoint {
            let owner = self.context_session(cp.session_id)?;
            if owner.work_item_id != current.work_item_id || owner.branch_id != current.branch_id {
                return Err(WorkstreamError::BindingMismatch.into());
            }
        }
        Ok(checkpoint)
    }
    pub fn latest_work_checkpoint(
        &self,
        work: &str,
        branch: Option<Id>,
    ) -> Result<Option<Checkpoint>> {
        let work = self.work_item(work)?;
        if let Some(id) = branch {
            self.require("branches", id)?;
        }
        let checkpoint =
            self.store
                .latest_work_checkpoint(self.project, work.item.meta.id, branch)?;
        if let Some(cp) = &checkpoint {
            self.context_checkpoint(cp.id, &work.item.meta.external_key, branch)?;
        }
        Ok(checkpoint)
    }
    /// Keep terminal and nonterminal executions, but never import another
    /// ownership generation's execution history into a current task packet.
    pub fn context_executions(&self, work: &str, branch: Option<Id>) -> Result<Vec<Execution>> {
        let work = self.work_item(work)?;
        if let Some(id) = branch {
            self.require("branches", id)?;
        }
        let mut stmt = self
            .store
            .conn
            .prepare(&format!(
                "SELECT json_extract(e.payload_json,'$.execution.id') FROM events e
             WHERE e.project_id=?1 AND e.work_item_id=?2 AND e.branch_id IS ?3
             AND e.event_type='execution.registered' AND {} ORDER BY e.project_revision",
                crate::scoped_read::visible("events", "e.id"),
            ))
            .map_err(db_error)?;
        let ids = stmt
            .query_map(
                params![
                    self.project.to_string(),
                    work.item.meta.id.to_string(),
                    branch.map(|id| id.to_string())
                ],
                |r| crate::catalog::id_at(r, 0),
            )
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        let mut result = Vec::new();
        for id in ids {
            let execution = self.store.execution(self.project, id)?;
            let session = self.session(execution.session_id)?;
            if self.current_session_ownership(&session)? {
                result.push(execution);
            }
        }
        Ok(result)
    }
}
