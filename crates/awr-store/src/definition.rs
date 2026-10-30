//! Gathers the facts the work definition checker reads from the current projections.
//! Read-only: no source is touched and no event is written.
use crate::{Store, query::projections, work::dependency_links, workstream::binding_at};
use awr_core::*;
use rusqlite::Connection;
use std::collections::BTreeMap;

/// Check one work's declared definition against the project as currently indexed.
/// `state` is `not_declared` for work without a definition, which keeps every existing
/// source and output unchanged.
pub(crate) fn work_definition_report(
    conn: &Connection,
    project: Id,
    key: &str,
) -> Result<DefinitionReport> {
    let works: Vec<Projected<WorkItem>> = projections(conn, project, EntityKind::WorkItem, None)?;
    let me = works
        .iter()
        .find(|w| w.item.meta.external_key == key)
        .ok_or_else(|| Error::NotFound(format!("WorkItem {key}")))?;
    if me.item.definition.is_none() {
        return Ok(check_work_definition(&DefinitionFacts {
            work: key,
            acceptance: &me.item.acceptance,
            paths: &me.item.paths,
            dependencies: &[],
            spec_refs: &me.item.spec_refs,
            spec_dispositions: &BTreeMap::new(),
            goals: &BTreeMap::new(),
            decisions: &BTreeMap::new(),
            works: &BTreeMap::from([(
                key.to_string(),
                DefinitionWork {
                    workstream: None,
                    status: me.item.status,
                    definition: None,
                },
            )]),
        }));
    }
    let mut known: BTreeMap<String, DefinitionWork> = works
        .iter()
        .map(|w| {
            (
                w.item.meta.external_key.clone(),
                DefinitionWork {
                    workstream: None,
                    status: w.item.status,
                    definition: w.item.definition.clone(),
                },
            )
        })
        .collect();
    // A workstream is only needed to compare the work with its declared parent. A work that is
    // not bound to one is compared as unbound; any other failure is surfaced, never skipped.
    if let Some(parent) = me.item.definition.as_ref().and_then(|d| d.parent.clone()) {
        for target in [key.to_string(), parent] {
            let Some(found) = works.iter().find(|w| w.item.meta.external_key == target) else {
                continue;
            };
            let stream = match binding_at(conn, project, found.item.meta.id) {
                Ok(stream) => Some(stream.to_string()),
                Err(Error::NotFound(_)) => None,
                Err(error) => return Err(error),
            };
            if let Some(entry) = known.get_mut(&target) {
                entry.workstream = stream;
            }
        }
    }
    let dependencies: Vec<String> = dependency_links(conn, project)?
        .into_iter()
        .filter(|edge| edge.from_key == key)
        .map(|edge| edge.to_key)
        .collect();
    let goals = projections::<Goal>(conn, project, EntityKind::Goal, None)?
        .into_iter()
        .map(|g| (g.item.meta.external_key, g.item.success_criteria))
        .collect::<BTreeMap<_, _>>();
    // Only an accepted decision can be adopted intent; the store keeps one decision per key.
    let mut decisions = BTreeMap::new();
    let mut spec_dispositions = BTreeMap::new();
    for decision in projections::<Decision>(conn, project, EntityKind::Decision, None)? {
        if decision.item.status != DecisionStatus::Accepted {
            continue;
        }
        let decision_key = decision.item.meta.external_key.clone();
        for item in decision
            .item
            .specification
            .iter()
            .flat_map(|spec| spec.items.iter())
        {
            spec_dispositions.insert((decision_key.clone(), item.id.clone()), item.disposition);
        }
        decisions.insert(
            decision_key,
            decision.item.meta.source_ref.source_fingerprint,
        );
    }
    Ok(check_work_definition(&DefinitionFacts {
        work: key,
        acceptance: &me.item.acceptance,
        paths: &me.item.paths,
        dependencies: &dependencies,
        spec_refs: &me.item.spec_refs,
        spec_dispositions: &spec_dispositions,
        goals: &goals,
        decisions: &decisions,
        works: &known,
    }))
}

impl Store {
    /// The deterministic definition report of one work; see [`DefinitionReport`].
    pub fn work_definition_report(&self, project: Id, key: &str) -> Result<DefinitionReport> {
        self.project(project)?;
        work_definition_report(&self.conn, project, key)
    }
}
