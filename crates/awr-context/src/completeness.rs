use crate::{
    EvidenceGap, HardContext, RelatedWorkContext, RuleScopeInput, SourceVersion, hard_context,
    related_work,
};
use awr_core::*;
use awr_source::IndexReport;
use awr_store::Store;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletenessRequest {
    pub work_item_key: String,
    pub branch_id: Option<Id>,
    pub scope: RuleScopeInput,
    pub source_sha: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CompletenessIssue {
    pub field: &'static str,
    pub code: &'static str,
    pub reference: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct ContextCompleteness {
    pub project_id: Id,
    pub project_revision: Revision,
    pub work_item_key: String,
    pub branch_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_context: Option<crate::BranchContextBinding>,
    pub source_refresh_performed: bool,
    pub freshness_basis: &'static str,
    pub source_fresh: bool,
    pub work_item_found: bool,
    pub work_state_complete: bool,
    pub acceptance_complete: bool,
    /// All possibly applicable hard rules are classified and their sources are fresh.
    pub rules_complete: bool,
    /// The required closure has known, fresh facts; pending but fully described work is allowed.
    pub dependencies_complete: bool,
    pub decision_context_complete: bool,
    /// Filled by the L1 compiler; the standalone required-fact assessor does not select goals.
    pub goal_context_complete: Option<bool>,
    pub unresolved_required_dependencies: Vec<String>,
    pub evidence_gaps: Vec<EvidenceGap>,
    pub source_versions: Vec<SourceVersion>,
    pub issues: Vec<CompletenessIssue>,
    /// Unconfirmed project goals that are not linked to this work. They do not change `complete`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_notices: Vec<CompletenessIssue>,
    pub complete: bool,
    pub status: &'static str,
    pub assessment_scope: &'static str,
}

pub(crate) struct CompletenessFacts<'a> {
    pub project: &'a Project,
    pub branch: Option<Id>,
    pub work_key: &'a str,
    pub work: Option<&'a Projected<WorkItem>>,
    pub hard: Option<&'a HardContext>,
    pub related: Option<&'a RelatedWorkContext>,
    pub sources: &'a [Source],
    pub refresh: Option<&'a IndexReport>,
}

pub(crate) fn assess_completeness(facts: CompletenessFacts<'_>) -> Result<ContextCompleteness> {
    let CompletenessFacts {
        project,
        branch,
        work_key,
        work,
        hard,
        related,
        sources,
        refresh,
    } = facts;
    if let Some(report) = refresh {
        if report.project_id != project.id {
            return Err(Error::InvalidInput(
                "freshness report belongs to another project".into(),
            ));
        }
        if report.project_revision != project.project_revision {
            return Err(Error::RevisionConflict {
                expected: report.project_revision,
                actual: project.project_revision,
            });
        }
    }
    for revision in work
        .map(|w| w.project_revision)
        .into_iter()
        .chain(hard.map(|h| h.project_revision))
        .chain(related.map(|r| r.project_revision))
    {
        if revision != project.project_revision {
            return Err(Error::RevisionConflict {
                expected: project.project_revision,
                actual: revision,
            });
        }
    }
    if work
        .is_some_and(|w| w.source.project_id != project.id || w.item.meta.external_key != work_key)
        || hard.is_some_and(|h| {
            h.project_id != project.id
                || h.work.meta.external_key != work_key
                || h.branch_id != branch
        })
        || related.is_some_and(|r| {
            r.project_id != project.id || r.work_item_key != work_key || r.branch_id != branch
        })
    {
        return Err(Error::InvalidInput(
            "completeness facts must refer to the same project, work and branch".into(),
        ));
    }
    let mut issues = Vec::new();
    let mut issue = |field, code, reference: String, reason: String| {
        issues.push(CompletenessIssue {
            field,
            code,
            reference,
            reason,
        })
    };
    if refresh.is_none() {
        issue(
            "source_fresh",
            "source_refresh_missing",
            project.external_key.clone(),
            "No Source refresh was performed for this assessment".into(),
        );
    }
    if let Some(report) = refresh {
        for problem in &report.issues {
            issue(
                "source_fresh",
                "source_refresh_failed",
                problem
                    .locator
                    .clone()
                    .unwrap_or_else(|| problem.mapping.clone()),
                format!("{}: {}", problem.code, problem.message),
            );
        }
        if report.pending > 0 {
            issue(
                "source_fresh",
                "source_projection_pending",
                project.external_key.clone(),
                format!("{} sources await projection", report.pending),
            );
        }
        if !report.ok && report.issues.is_empty() {
            issue(
                "source_fresh",
                "source_refresh_failed",
                project.external_key.clone(),
                "Source refresh did not complete".into(),
            );
        }
    }
    for source in sources.iter().filter(|s| s.freshness != Freshness::Fresh) {
        issue(
            "source_fresh",
            "source_not_fresh",
            source.id.to_string(),
            format!("{} is {:?}", source.locator, source.freshness),
        );
    }
    let source_fresh = refresh.is_some_and(|r| r.ok && r.pending == 0)
        && sources.iter().all(|s| s.freshness == Freshness::Fresh);
    let domain_failed = |domain: &str| {
        refresh.is_some_and(|r| {
            r.issues
                .iter()
                .any(|i| i.mapping.split('|').next() == Some(domain))
        })
    };
    let work_item_found = work.is_some();
    let mut work_state_complete = false;
    let mut acceptance_complete = false;
    if let Some(work) = work {
        acceptance_complete = !domain_failed(&work.source.domain)
            && work.source.freshness == Freshness::Fresh
            && !work.item.acceptance.is_empty()
            && work.item.acceptance.iter().all(|s| !s.trim().is_empty());
        if !acceptance_complete {
            issue(
                "acceptance_complete",
                "acceptance_missing_or_stale",
                work_key.into(),
                "Current work needs fresh, nonempty acceptance criteria with no blank entries"
                    .into(),
            );
        }
        work_state_complete = !domain_failed(&work.source.domain)
            && work.source.freshness == Freshness::Fresh
            && work.item.status != WorkStatus::Unknown
            && !work.item.next_action.trim().is_empty()
            && (work.item.status != WorkStatus::Blocked
                || work
                    .item
                    .blocker
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty()));
        if !work_state_complete {
            issue(
                "work_state_complete",
                "work_state_incomplete",
                work_key.into(),
                "Status and next action must be known/current; blocked work needs a reason".into(),
            );
        }
    } else {
        issue(
            "work_item_found",
            "work_item_not_found",
            work_key.into(),
            "No active source projection identifies this work item".into(),
        );
    }
    let rule_sources = sources
        .iter()
        .filter(|s| s.domain == "rules")
        .collect::<Vec<_>>();
    let minimal = awr_source::minimal_context(sources);
    if rule_sources.is_empty() && !minimal {
        issue(
            "rules_complete",
            "rules_source_missing",
            project.external_key.clone(),
            "No active rules source defines the required rule context".into(),
        );
    }
    if let Some(hard) = hard {
        for rule in &hard.unresolved {
            issue(
                "rules_complete",
                "hard_rule_unresolved",
                rule.rule.item.meta.external_key.clone(),
                rule.reasons.join("; "),
            );
        }
    }
    let rules_complete = hard.is_some_and(|h| h.unresolved.is_empty())
        && (!rule_sources.is_empty() || minimal)
        && !domain_failed("rules")
        && rule_sources.iter().all(|s| s.freshness == Freshness::Fresh);
    let mut dependencies_complete = related.is_some();
    let mut decision_context_complete = related.is_some();
    let mut unresolved_required_dependencies = Vec::new();
    let mut evidence_gaps = Vec::new();
    if let Some(related) = related {
        for dependency in &related.unavailable_dependencies {
            let reference = format!("dependency:{}", dependency.edge_id);
            issue("dependencies_complete", "required_dependency_unavailable", reference.clone(),
                "Required dependency is unavailable in this workstream; obtain an authorized delivery before proceeding".into());
            unresolved_required_dependencies.push(reference);
            dependencies_complete = false;
        }
        for key in &related.missing_dependencies {
            issue(
                "dependencies_complete",
                "required_dependency_missing",
                key.clone(),
                "Required dependency has no active source fact".into(),
            );
            dependencies_complete = false;
        }
        for key in &related.dependency_cycles {
            issue(
                "dependencies_complete",
                "dependency_cycle",
                key.clone(),
                "Required dependency participates in a cycle".into(),
            );
            dependencies_complete = false;
        }
        for dependency in &related.unresolved_dependencies {
            unresolved_required_dependencies.push(dependency.meta.external_key.clone());
            if dependency.freshness != Freshness::Fresh
                || dependency.status == WorkStatus::Unknown
                || dependency.next_action.trim().is_empty()
                || (dependency.status == WorkStatus::Blocked
                    && dependency
                        .blocker
                        .as_ref()
                        .is_none_or(|s| s.trim().is_empty()))
            {
                issue("dependencies_complete", "required_dependency_state_incomplete", dependency.meta.external_key.clone(), "Required dependency needs fresh known status, next action and any blocked reason".into());
                dependencies_complete = false;
            }
        }
        let mut dependency_sources = std::collections::BTreeSet::new();
        if let Some(work) = work {
            dependency_sources.insert(work.source.id);
        }
        for edge in &related.required_edges {
            dependency_sources.insert(edge.source_ref.source_id);
        }
        for dependency in related
            .resolved_dependencies
            .iter()
            .chain(&related.unresolved_dependencies)
        {
            dependency_sources.insert(dependency.meta.source_ref.source_id);
        }
        for id in dependency_sources {
            if !sources
                .iter()
                .any(|s| s.id == id && s.freshness == Freshness::Fresh && !domain_failed(&s.domain))
            {
                dependencies_complete = false;
                issue(
                    "dependencies_complete",
                    "dependency_source_not_fresh",
                    id.to_string(),
                    "Required dependency/edge authority is missing, stale or failed its refresh"
                        .into(),
                );
            }
        }
        for decision in &related.uncertain_decisions {
            issue(
                "decision_context_complete",
                "decision_unresolved",
                decision.meta.external_key.clone(),
                decision.reasons.join("; "),
            );
            decision_context_complete = false;
        }
        evidence_gaps = related.evidence_gaps.clone();
    } else {
        evidence_gaps.push(EvidenceGap {
            code: "work_context_unavailable",
            reference: work_key.into(),
            reason: "Evidence selection requires an existing work item".into(),
        });
    }
    if domain_failed("decisions")
        || sources
            .iter()
            .any(|s| s.domain == "decisions" && s.freshness != Freshness::Fresh)
    {
        decision_context_complete = false;
        issue(
            "decision_context_complete",
            "decision_source_not_fresh",
            project.external_key.clone(),
            "Decision authority must be refreshed before absence/relevance can be concluded".into(),
        );
    }
    unresolved_required_dependencies.sort();
    unresolved_required_dependencies.dedup();
    evidence_gaps
        .sort_by(|a, b| (a.code, &a.reference, &a.reason).cmp(&(b.code, &b.reference, &b.reason)));
    issues.sort();
    issues.dedup();
    let mut source_versions = sources
        .iter()
        .cloned()
        .map(SourceVersion::from)
        .collect::<Vec<_>>();
    source_versions.sort_by_key(|s| s.id);
    let complete = source_fresh
        && work_item_found
        && work_state_complete
        && acceptance_complete
        && rules_complete
        && dependencies_complete
        && decision_context_complete;
    Ok(ContextCompleteness {
        project_id: project.id,
        project_revision: project.project_revision,
        work_item_key: work_key.into(),
        branch_id: branch,
        branch_context: None,
        source_refresh_performed: refresh.is_some(),
        freshness_basis: if refresh.is_some() {
            "source_refresh_at_project_revision"
        } else {
            "last_recorded_projection_only"
        },
        source_fresh,
        work_item_found,
        work_state_complete,
        acceptance_complete,
        rules_complete,
        dependencies_complete,
        decision_context_complete,
        goal_context_complete: None,
        unresolved_required_dependencies,
        evidence_gaps,
        source_versions,
        issues,
        project_notices: Vec::new(),
        complete,
        status: if complete {
            "CONTEXT COMPLETE"
        } else {
            "CONTEXT INCOMPLETE"
        },
        assessment_scope: "Completeness of current required context facts and associations. Known unresolved dependencies and evidence gaps are retained; this does not certify task completion, verification execution or release readiness.",
    })
}

/// Read an indexed snapshot. Omitting a matching refresh report explicitly prevents a current verdict.
pub fn inspect_completeness(
    store: &Store,
    project_id: Id,
    request: &CompletenessRequest,
    refresh: Option<&IndexReport>,
) -> Result<ContextCompleteness> {
    let project = store.project(project_id)?;
    if request.work_item_key.trim().is_empty() {
        return Err(Error::InvalidInput("work key must not be blank".into()));
    }
    let read = crate::compile::scoped_read(
        store,
        &project,
        &crate::ContextRequest {
            work_item_key: Some(request.work_item_key.clone()),
            ..Default::default()
        },
        None,
    )?;
    if let Some(read) = &read {
        if refresh.is_some_and(|r| !r.ok || r.pending > 0) {
            return Err(Error::ContextIncomplete(
                "Source refresh failed; the workstream boundary cannot be proven current".into(),
            ));
        }
        if let Some(branch) = request.branch_id {
            read.context_branch(&branch.to_string())?;
        }
    }
    let binding = crate::branch::branch_binding(store, &project, request.branch_id)?;
    let work = match read.as_ref().map_or_else(
        || store.work_item(project_id, &request.work_item_key),
        |read| read.work_item(&request.work_item_key),
    ) {
        Ok(work) => Some(work),
        Err(Error::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let hard = work
        .as_ref()
        .map(|_| {
            hard_context(
                store,
                project_id,
                &request.work_item_key,
                request.branch_id,
                &request.scope,
            )
        })
        .transpose()?;
    let related = work
        .as_ref()
        .map(|_| {
            if let Some(read) = &read {
                crate::related_work_in_workstream(
                    read,
                    &request.work_item_key,
                    request.branch_id,
                    request.source_sha.as_deref(),
                    request.scope.paths.as_deref(),
                )
            } else {
                related_work(
                    store,
                    project_id,
                    &request.work_item_key,
                    request.branch_id,
                    request.source_sha.as_deref(),
                    request.scope.paths.as_deref(),
                )
            }
        })
        .transpose()?;
    let mut sources = store.sources(project_id)?;
    if let Some(read) = &read {
        let mut ids = std::collections::BTreeSet::new();
        ids.extend(work.iter().map(|w| w.source.id));
        ids.extend(
            hard.iter()
                .flat_map(|h| h.source_revisions.iter().map(|s| s.id)),
        );
        ids.extend(
            related
                .iter()
                .flat_map(|r| r.source_revisions.iter().map(|s| s.id)),
        );
        if let Some(source) = read.authority_source()? {
            ids.insert(source.id);
        }
        sources.retain(|s| ids.contains(&s.id) || s.domain == "rules");
    }
    let mut report = assess_completeness(CompletenessFacts {
        project: &project,
        branch: request.branch_id,
        work_key: &request.work_item_key,
        work: work.as_ref(),
        hard: hard.as_ref(),
        related: related.as_ref(),
        sources: &sources,
        refresh,
    })?;
    report.branch_context = Some(binding);
    if read
        .as_ref()
        .is_some_and(|r| r.workstream().state != WorkstreamState::Active)
    {
        report.work_state_complete = false;
        report.complete = false;
        report.status = "CONTEXT INCOMPLETE";
        report.issues.push(CompletenessIssue {
            field: "work_state_complete",
            code: "workstream_inactive",
            reference: request.work_item_key.clone(),
            reason: "Workstream is paused or archived; inspect current policy before continuing execution".into(),
        });
    }
    let actual = store.project(project_id)?.project_revision;
    if actual != project.project_revision {
        return Err(Error::RevisionConflict {
            expected: project.project_revision,
            actual,
        });
    }
    Ok(report)
}

/// Refresh configured files (including failed/new mappings) before assessing the cached facts.
pub fn check_completeness(
    store: &mut Store,
    root: &Path,
    request: &CompletenessRequest,
) -> Result<ContextCompleteness> {
    let snapshot = awr_source::refresh_snapshot(store, root)?;
    inspect_completeness(
        &snapshot.store,
        snapshot.refresh.project_id,
        request,
        Some(&snapshot.refresh),
    )
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../awr-store/tests/support/workstreams.rs"]
mod scoped_fixture;
#[cfg(test)]
mod scoped_tests {
    use super::scoped_fixture as fixture;
    use super::*;

    #[test]
    fn opaque_dependency_boundary_is_an_incomplete_required_fact() {
        let mut f = fixture::Fixture::new();
        f.dependency("W0", "W1", true);
        f.batch.work_items[1].status = WorkStatus::Completed;
        f.batch.work_items[1].raw_status = "completed".into();
        f.reproject();
        let read = f.read(0);
        let mut related = crate::related_work_in_workstream(&read, "W0", None, None, None).unwrap();
        let project = f.store.project(f.project).unwrap();
        let work = read.work_item("W0").unwrap();
        let sources = f.store.sources(f.project).unwrap();
        let assess = |related: &RelatedWorkContext| {
            assess_completeness(CompletenessFacts {
                project: &project,
                branch: None,
                work_key: "W0",
                work: Some(&work),
                hard: None,
                related: Some(related),
                sources: &sources,
                refresh: None,
            })
            .unwrap()
        };
        let report = assess(&related);
        assert!(!report.dependencies_complete);
        assert_eq!(report.unresolved_required_dependencies.len(), 1);
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.code == "required_dependency_unavailable")
        );
        assert!(!serde_json::to_string(&report).unwrap().contains("\"W1\""));
        related.unavailable_dependencies.clear();
        // The unavailable boundary, not an unrelated missing hard-rule/refresh
        // diagnostic, is what makes the dependency fact set incomplete.
        assert!(assess(&related).dependencies_complete);
    }
}
