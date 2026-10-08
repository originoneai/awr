use awr_core::*;
use awr_store::Store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleScopeInput {
    pub agent_id: Option<String>,
    /// Concrete paths for the requested execution scope. Directories/globs remain unknown.
    pub paths: Option<Vec<String>>,
    /// Additional scope tags; declared work tags are always retained.
    pub tags: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize)]
pub struct RuleSelection {
    pub context: RuleContext,
    pub paths_origin: &'static str,
    pub hard: Vec<Projected<Rule>>,
    pub soft: Vec<Projected<Rule>>,
    pub info: Vec<Projected<Rule>>,
    pub unknown: Vec<RuleMatch>,
    pub not_applicable: Vec<RuleMatch>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SourceVersion {
    pub id: Id,
    pub revision: Revision,
    pub fingerprint: String,
    pub freshness: Freshness,
    pub locator: String,
}
impl From<Source> for SourceVersion {
    fn from(s: Source) -> Self {
        Self {
            id: s.id,
            revision: s.revision,
            fingerprint: s.fingerprint,
            freshness: s.freshness,
            locator: s.locator,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct HardWork {
    pub meta: ProjectionMeta,
    pub status: WorkStatus,
    pub raw_status: String,
    pub acceptance: Vec<String>,
    pub blocker: Option<String>,
    pub next_action: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct HardContext {
    pub project_id: Id,
    pub project_revision: Revision,
    pub branch_id: Option<Id>,
    pub work: HardWork,
    pub rules: Vec<Rule>,
    pub scope: RuleContext,
    pub paths_origin: &'static str,
    pub source_revisions: Vec<SourceVersion>,
    pub unresolved: Vec<RuleMatch>,
    pub issues: Vec<String>,
    /// Completeness of this hard-fact subset only, not dependency/evidence execution admission.
    pub complete: bool,
}

fn sorted(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn scope_context(
    project: &Project,
    work: Option<&Projected<WorkItem>>,
    input: &RuleScopeInput,
) -> (RuleContext, &'static str) {
    let declared_paths = work
        .filter(|w| !w.item.paths.is_empty())
        .map(|w| w.item.paths.clone());
    let (paths, origin) = if let Some(paths) = &input.paths {
        (Some(paths.clone()), "caller")
    } else {
        (declared_paths, "work_source")
    };
    let concrete = concrete_scope_paths(paths.as_deref());
    let broad = paths.is_some() && concrete.is_none();
    let paths = concrete;
    let tags = if work.is_some_and(|w| !w.item.tags.is_empty()) || input.tags.is_some() {
        Some(sorted(
            work.into_iter()
                .flat_map(|w| w.item.tags.clone())
                .chain(input.tags.clone().unwrap_or_default())
                .collect(),
        ))
    } else {
        None
    };
    (
        RuleContext {
            project_key: Some(project.external_key.clone()),
            work_item_key: work.map(|w| w.item.meta.external_key.clone()),
            agent_id: input.agent_id.clone(),
            paths,
            tags,
        },
        if broad { "unknown_broad_scope" } else { origin },
    )
}

/// Evaluate every rule once. Unknown metadata and incomplete scope stay separate from exclusions.
pub fn select_rules(
    store: &Store,
    project: &Project,
    work: Option<&Projected<WorkItem>>,
    input: &RuleScopeInput,
) -> Result<RuleSelection> {
    if input
        .agent_id
        .as_deref()
        .is_some_and(|s| s.trim().is_empty())
        || input
            .paths
            .iter()
            .chain(&input.tags)
            .flatten()
            .any(|s| s.trim().is_empty())
    {
        return Err(Error::InvalidInput(
            "scope identities and paths/tags must not be blank".into(),
        ));
    }
    if work.is_some_and(|work| {
        work.source.project_id != project.id || work.project_revision != project.project_revision
    }) {
        return Err(Error::InvalidInput(
            "work and project must be from the same project revision".into(),
        ));
    }
    let (context, paths_origin) = scope_context(project, work, input);
    let mut result = RuleSelection {
        context,
        paths_origin,
        hard: vec![],
        soft: vec![],
        info: vec![],
        unknown: vec![],
        not_applicable: vec![],
    };
    for matched in store.rules_for(project.id, &result.context)? {
        match matched.applicability {
            Applicability::NotApplicable => result.not_applicable.push(matched),
            Applicability::Unknown => result.unknown.push(matched),
            Applicability::Applicable => match matched.rule.item.severity {
                Some(Severity::Hard) => result.hard.push(matched.rule),
                Some(Severity::Soft) => result.soft.push(matched.rule),
                Some(Severity::Info) => result.info.push(matched.rule),
                None => {
                    return Err(Error::ContextIncomplete(
                        "applicable rule lacks explicit severity".into(),
                    ));
                }
            },
        }
    }
    for rules in [&mut result.hard, &mut result.soft, &mut result.info] {
        rules.sort_by(|a, b| {
            a.item
                .meta
                .external_key
                .cmp(&b.item.meta.external_key)
                .then(a.item.meta.id.cmp(&b.item.meta.id))
        });
    }
    let actual = store.project(project.id)?.project_revision;
    if actual != project.project_revision {
        return Err(Error::RevisionConflict {
            expected: project.project_revision,
            actual,
        });
    }
    crate::public_source_context(store, Ok(result))
}

/// Assemble indivisible hard facts from a refreshed projection snapshot. Never summarizes or truncates.
pub fn hard_context(
    store: &Store,
    project: Id,
    work_key: &str,
    branch: Option<Id>,
    input: &RuleScopeInput,
) -> Result<HardContext> {
    awr_core::ensure_public_text(work_key)?;
    crate::public_source_context(
        store,
        hard_context_selected(store, project, work_key, branch, input),
    )
}
fn hard_context_selected(
    store: &Store,
    project: Id,
    work_key: &str,
    branch: Option<Id>,
    input: &RuleScopeInput,
) -> Result<HardContext> {
    let project = store.project(project)?;
    crate::branch::branch_binding(store, &project, branch)?;
    let work = store.work_item(project.id, work_key)?;
    let selection = select_rules(store, &project, Some(&work), input)?;
    let mut issues = Vec::new();
    if work.source.freshness != Freshness::Fresh {
        issues.push("work source is not fresh".into());
    }
    if work.item.status == WorkStatus::Unknown {
        issues.push("work status is unknown".into());
    }
    if work.item.acceptance.is_empty() || work.item.acceptance.iter().any(|a| a.trim().is_empty()) {
        issues.push("acceptance criteria are missing or blank".into());
    }
    if work.item.next_action.trim().is_empty() {
        issues.push("next action is missing".into());
    }
    if work.item.status == WorkStatus::Blocked
        && work
            .item
            .blocker
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
    {
        issues.push("blocked work has no blocker reason".into());
    }
    let unresolved = selection
        .unknown
        .into_iter()
        .filter(|r| r.rule.item.severity == Some(Severity::Hard) || r.rule.item.severity.is_none())
        .collect::<Vec<_>>();
    let mut ids = BTreeSet::from([work.source.id]);
    for rule in &selection.hard {
        ids.insert(rule.source.id);
    }
    for rule in &unresolved {
        ids.insert(rule.rule.source.id);
    }
    let sources = store.sources(project.id)?;
    // Only an explicit minimal profile may go without a rules source; the same basis completeness and bootstrap use, so the
    // rendered gaps never contradict `rules_complete`. Configured rules stay mandatory below, whatever the profile.
    if !sources.iter().any(|s| s.domain == "rules") && !awr_source::minimal_context(&sources) {
        issues.push("rules source is missing".into());
    }
    for source in sources.iter().filter(|s| s.domain == "rules") {
        ids.insert(source.id);
        if source.freshness != Freshness::Fresh {
            issues.push(format!("rules source {} is not fresh", source.id));
        }
    }
    let source_revisions = sources
        .into_iter()
        .filter(|s| ids.contains(&s.id))
        .map(SourceVersion::from)
        .collect();
    let result = HardContext {
        project_id: project.id,
        project_revision: project.project_revision,
        branch_id: branch,
        work: HardWork {
            meta: work.item.meta,
            status: work.item.status,
            raw_status: work.item.raw_status,
            acceptance: work.item.acceptance,
            blocker: work.item.blocker,
            next_action: work.item.next_action,
        },
        rules: selection.hard.into_iter().map(|r| r.item).collect(),
        scope: selection.context,
        paths_origin: selection.paths_origin,
        source_revisions,
        complete: issues.is_empty() && unresolved.is_empty(),
        unresolved,
        issues,
    };
    let actual = store.project(project.id)?.project_revision;
    if actual != project.project_revision {
        return Err(Error::RevisionConflict {
            expected: project.project_revision,
            actual,
        });
    }
    Ok(result)
}
