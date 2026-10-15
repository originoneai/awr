use crate::completeness::{CompletenessFacts, assess_completeness};
use crate::*;
use awr_core::*;
use awr_store::{Store, WorkstreamRead, WorkstreamReadSelection};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRequest {
    pub work_item_key: Option<String>,
    pub session_id: Option<Id>,
    /// Prepare explicit work for an agent before a session exists; do not borrow an active session.
    #[serde(default, skip_serializing_if = "is_false")]
    pub detached: bool,
    pub agent_id: Option<String>,
    /// None uses the legacy project default, or the explicit session's branch/main
    /// for workstream-enabled projects. Some reads that branch without switching.
    /// Use compile_branch_context for a name or an explicit main baseline.
    pub branch_id: Option<Id>,
    pub goal_keys: Vec<String>,
    pub paths: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub source_sha: Option<String>,
    pub intent: String,
    pub token_budget: usize,
    pub delta_baseline: DeltaBaseline,
}
impl Default for ContextRequest {
    fn default() -> Self {
        Self {
            work_item_key: None,
            session_id: None,
            detached: false,
            agent_id: None,
            branch_id: None,
            goal_keys: vec![],
            paths: None,
            tags: None,
            source_sha: None,
            intent: "work".into(),
            token_budget: 5000,
            delta_baseline: DeltaBaseline::Auto,
        }
    }
}
fn is_false(value: &bool) -> bool {
    !value
}
#[derive(Debug, Clone, Serialize)]
pub struct ContextOmission {
    pub kind: &'static str,
    pub reference: String,
    pub count: usize,
    pub reason: &'static str,
}
#[derive(Debug, Clone, Serialize)]
pub struct WorkContextReport {
    pub level: &'static str,
    pub selection_basis: &'static str,
    pub goal_selection_basis: &'static str,
    pub session_id: Option<Id>,
    pub checkpoint_id: Option<Id>,
    pub delta_after_revision: Option<Revision>,
    /// Absent when a work item cannot be resolved; no execution pack/hash is fabricated.
    pub work_context: Option<BudgetedContext>,
    pub completeness: ContextCompleteness,
    pub omitted_refs: Vec<ContextOmission>,
    pub diagnostic_text: Option<String>,
}
impl WorkContextReport {
    pub fn rendered_context(&self) -> &str {
        self.work_context
            .as_ref()
            .map(|p| p.rendered_context.as_str())
            .or(self.diagnostic_text.as_deref())
            .unwrap_or("")
    }
}
pub(crate) struct Selection {
    pub work: Option<Projected<WorkItem>>,
    pub session: Option<Session>,
    pub basis: &'static str,
}
pub(crate) fn select_work_scoped(
    store: &Store,
    project: &Project,
    branch: Option<Id>,
    request: &ContextRequest,
    read: Option<&WorkstreamRead>,
) -> Result<Selection> {
    if request.detached && (request.session_id.is_some() || request.work_item_key.is_none()) {
        return Err(Error::InvalidInput(
            "detached context requires explicit work and no session ID".into(),
        ));
    }
    let mut work = match request.work_item_key.as_deref() {
        Some(key) => match read.map_or_else(
            || store.work_item(project.id, key),
            |read| read.work_item(key),
        ) {
            Ok(w) => Some(w),
            Err(Error::NotFound(_)) => None,
            Err(e) => return Err(e),
        },
        None => None,
    };
    // An explicitly absent work key must not silently fall back to another task/session.
    if request.work_item_key.is_some() && work.is_none() {
        return Ok(Selection {
            work: None,
            session: None,
            basis: "explicit_work_missing",
        });
    }
    let session = if request.detached {
        None
    } else if let Some(id) = request.session_id {
        let session = read.map_or_else(
            || store.session(project.id, id),
            |read| read.context_session(id),
        )?;
        if session.branch_id != branch
            || request
                .agent_id
                .as_ref()
                .is_some_and(|a| a != &session.agent_id)
            || work
                .as_ref()
                .is_some_and(|w| session.work_item_id != Some(w.item.meta.id))
        {
            return Err(Error::InvalidInput(
                "context session does not match the selected work, agent or branch".into(),
            ));
        }
        Some(session)
    } else {
        let active = if let Some(read) = read {
            read.select_active_session(
                work.as_ref().map(|w| w.item.meta.id),
                request.agent_id.as_deref(),
                branch,
            )
        } else {
            store.select_active_session(
                project.id,
                None,
                work.as_ref().map(|w| w.item.meta.id),
                request.agent_id.as_deref(),
                branch,
            )
        };
        match active {
            Ok(s) => Some(s),
            Err(Error::NotFound(_)) => None,
            Err(e) => return Err(e),
        }
    };
    let mut basis = if request.detached {
        "explicit_work_detached_session"
    } else if work.is_some() {
        "explicit_work"
    } else {
        "none"
    };
    if work.is_none() {
        if let Some(id) = session.as_ref().and_then(|s| s.work_item_id) {
            work = match read.map_or_else(
                || store.work_item_by_id(project.id, id),
                |read| read.work_item(&id.to_string()),
            ) {
                Ok(w) => Some(w),
                Err(Error::NotFound(_)) => None,
                Err(e) => return Err(e),
            };
            basis = if work.is_some() {
                "session"
            } else {
                "session_work_retired"
            };
        } else {
            let mut candidates = read
                .map_or_else(|| store.work_items(project.id), |read| read.work_items())?
                .into_iter()
                .filter(|w| matches!(w.item.status, WorkStatus::Claimed | WorkStatus::InProgress))
                .collect::<Vec<_>>();
            if candidates.len() > 1 {
                return Err(Error::InvalidInput(format!(
                    "multiple current work items; specify --work: {}",
                    candidates
                        .iter()
                        .map(|w| w.item.meta.external_key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            work = candidates.pop();
            if work.is_some() {
                basis = "source_current";
            }
        }
    }
    if work.as_ref().is_some_and(|w| {
        session
            .as_ref()
            .is_some_and(|s| s.work_item_id != Some(w.item.meta.id))
    }) {
        return Err(Error::InvalidInput("selected session is not bound to the resolved work; specify --work to select its matching session".into()));
    }
    Ok(Selection {
        work,
        session,
        basis,
    })
}
fn entity(meta: &ProjectionMeta, kind: &str) -> SelectedEntity {
    SelectedEntity {
        kind: kind.into(),
        id: meta.id,
        revision: meta.revision,
    }
}
fn chunk(
    key: impl Into<String>,
    section: ContextSection,
    text: impl Into<String>,
    entities: Vec<SelectedEntity>,
) -> ContextChunk {
    ContextChunk {
        key: key.into(),
        section,
        text: text.into(),
        entities,
    }
}
fn optional(priority: u16, recency: Revision, chunk: ContextChunk) -> RankedChunk {
    RankedChunk {
        priority,
        recency,
        chunk,
    }
}
fn add_issue(
    report: &mut ContextCompleteness,
    field: &'static str,
    code: &'static str,
    reference: impl Into<String>,
    reason: impl Into<String>,
) {
    report.issues.push(CompletenessIssue {
        field,
        code,
        reference: reference.into(),
        reason: reason.into(),
    });
    report.complete = false;
    report.status = "CONTEXT INCOMPLETE";
}
fn scope(request: &ContextRequest, session: Option<&Session>) -> RuleScopeInput {
    RuleScopeInput {
        agent_id: request
            .agent_id
            .clone()
            .or_else(|| session.map(|s| s.agent_id.clone())),
        paths: request.paths.clone(),
        tags: request.tags.clone(),
    }
}

fn goal_needs_confirmation(goal: &Projected<Goal>) -> bool {
    goal.source.freshness != Freshness::Fresh
        || goal.item.status.trim().is_empty()
        || matches!(
            goal.item.status.trim().to_ascii_lowercase().as_str(),
            "unknown" | "draft" | "candidate" | "pending" | "needs_confirmation"
        )
        || (goal
            .source
            .locator
            .replace('\\', "/")
            .contains(".awr/intake/")
            && goal.item.title == "Establish a verified project baseline")
        || goal.item.title.trim().is_empty()
}

fn project_goal_notice(goal: &Projected<Goal>) -> CompletenessIssue {
    CompletenessIssue {
        field: "project_goal",
        code: "project_goal_unconfirmed",
        reference: goal.item.meta.external_key.clone(),
        reason: format!(
            "Project goal {} ({}) is unconfirmed and is not linked to this work. It does not make this work's context incomplete. Confirm or retire it with awr intake inspect.",
            goal.item.title, goal.item.status
        ),
    }
}

fn goals(
    store: &Store,
    project: &Project,
    sources: &[Source],
    keys: &[String],
) -> Result<Vec<Projected<Goal>>> {
    let mut goals = Vec::new();
    if !keys.is_empty() {
        for key in keys.iter().collect::<BTreeSet<_>>() {
            match store.goal(project.id, key) {
                Ok(goal) => goals.push(goal),
                Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
    } else {
        // Project-level goal sources are explicitly primary. Supporting/unrelated bodies are not read.
        for source in sources
            .iter()
            .filter(|s| matches!(s.domain.as_str(), "goal" | "ledger") && s.role == "primary")
        {
            for payload in store.source_projection_payloads(source, EntityKind::Goal)? {
                let goal: Goal = serde_json::from_value(payload)?;
                if ![
                    "completed",
                    "done",
                    "cancelled",
                    "canceled",
                    "archived",
                    "retired",
                ]
                .contains(&goal.status.to_ascii_lowercase().as_str())
                {
                    goals.push(Projected {
                        item: goal,
                        source: source.clone(),
                        project_revision: project.project_revision,
                    });
                }
            }
        }
    }
    goals.sort_by(|a, b| a.item.meta.external_key.cmp(&b.item.meta.external_key));
    Ok(goals)
}
fn source_state(state: Option<&awr_store::SourceState>) -> String {
    state
        .map(|s| {
            format!(
                "r{} {} {:?} active={}",
                s.revision, s.fingerprint, s.freshness, s.active
            )
        })
        .unwrap_or_else(|| "unavailable/absent; see before_known".into())
}

/// The deterministic L1 pipeline: refresh -> work -> dependencies -> rules -> decisions -> delta
/// -> evidence/completeness -> whole-fact budget -> stable rendered text/hash. No LLM is called.
pub fn compile_context(
    store: &mut Store,
    root: &Path,
    request: &ContextRequest,
) -> Result<WorkContextReport> {
    awr_core::ensure_public_data(request)?;
    compile_context_selected(store, root, request, None, None)
}

/// Read a named branch overlay without changing project defaults or any runtime ownership.
pub fn compile_branch_context(
    store: &mut Store,
    root: &Path,
    reference: &str,
    request: &ContextRequest,
) -> Result<WorkContextReport> {
    awr_core::ensure_public_data(&(reference, request))?;
    if request.branch_id.is_some() {
        return Err(Error::InvalidInput("use only one branch selector".into()));
    }
    crate::branch::require_fork_request(&request.delta_baseline)?;
    let mut request = request.clone();
    request.delta_baseline = DeltaBaseline::BranchFork;
    compile_context_selected(store, root, &request, Some(reference), None)
}

/// Access is loaded by a trusted host, never deserialized from request data.
/// Selection narrows those grants and does not confer execution permission.
pub fn compile_workstream_context(
    store: &mut Store,
    root: &Path,
    request: &ContextRequest,
    access: &WorkstreamAccess,
    selection: &WorkstreamReadSelection,
) -> Result<WorkContextReport> {
    let mut request = request.clone();
    if selection
        .work_item_key
        .as_ref()
        .zip(request.work_item_key.as_ref())
        .is_some_and(|(a, b)| a != b)
        || selection
            .session_id
            .zip(request.session_id)
            .is_some_and(|(a, b)| a != b)
    {
        return Err(WorkstreamError::BindingMismatch.into());
    }
    request.work_item_key = request.work_item_key.or(selection.work_item_key.clone());
    request.session_id = request.session_id.or(selection.session_id);
    awr_core::ensure_public_data(&request)?;
    compile_context_selected(store, root, &request, None, Some((access, selection)))
}

fn compile_context_selected(
    store: &mut Store,
    root: &Path,
    request: &ContextRequest,
    reference: Option<&str>,
    access: Option<(&WorkstreamAccess, &WorkstreamReadSelection)>,
) -> Result<WorkContextReport> {
    if request.token_budget == 0
        || request.token_budget > 100000
        || request.intent.trim().is_empty()
        || request
            .work_item_key
            .as_ref()
            .is_some_and(|s| s.trim().is_empty())
        || request
            .agent_id
            .as_ref()
            .is_some_and(|s| s.trim().is_empty())
        || request
            .goal_keys
            .iter()
            .chain(request.paths.iter().flatten())
            .chain(request.tags.iter().flatten())
            .any(|s| s.trim().is_empty())
    {
        return Err(Error::InvalidInput(
            "context needs nonblank request fields and a 1..100000 token budget".into(),
        ));
    }
    let root = root.canonicalize()?;
    let snapshot = awr_source::refresh_snapshot(store, &root)?;
    let refresh = snapshot.refresh;
    let store = &snapshot.store;
    let project = store.project(refresh.project_id)?;
    let read = scoped_read(store, &project, request, access)?;
    if read.is_some() && (!refresh.ok || refresh.pending > 0) {
        return Err(Error::ContextIncomplete(
            "Source refresh failed; the workstream boundary cannot be proven current".into(),
        ));
    }
    let branch = match (&read, reference) {
        (Some(read), Some(reference)) => read.context_branch(reference)?,
        (Some(read), None) => {
            if let Some(id) = request.branch_id {
                read.context_branch(&id.to_string())?
            } else if let Some(id) = request.session_id {
                read.context_session(id)?.branch_id
            } else {
                None
            }
        }
        (None, Some(reference)) => store.resolve_branch(project.id, reference)?,
        (None, None) => request.branch_id.or(project.current_branch_id),
    };
    let branch_context = crate::branch::branch_binding(store, &project, branch)?;
    let mut sources = store.sources(project.id)?;
    let selection = select_work_scoped(store, &project, branch, request, read.as_ref())?;
    if read.is_some() && selection.work.is_none() {
        return Err(Error::ContextIncomplete(
            "No current task in the selected workstream; specify a work item".into(),
        ));
    }
    let mut goal_basis = if !request.goal_keys.is_empty() {
        "explicit_goal_keys"
    } else if read.is_some() {
        "workstream_goal_references"
    } else {
        "project_primary_goal_sources"
    };
    let key = selection
        .work
        .as_ref()
        .map(|w| w.item.meta.external_key.as_str())
        .or(request.work_item_key.as_deref())
        .unwrap_or("(unresolved work)");
    let Some(work) = selection.work.as_ref() else {
        let mut completeness = assess_completeness(CompletenessFacts {
            project: &project,
            branch,
            work_key: key,
            work: None,
            hard: None,
            related: None,
            sources: &sources,
            refresh: Some(&refresh),
        })?;
        completeness.branch_context = Some(branch_context);
        completeness.goal_context_complete = Some(false);
        let text = format!(
            "CONTEXT INCOMPLETE\nProject: {} [{}] r{}\nRequested work: {}\nNo unambiguous active work projection was resolved. Run awr intake inspect --json for source-backed organization actions, then specify a current --work key.\n",
            project.external_key, project.id, project.project_revision, key
        );
        let count = token_count(&text);
        if count > request.token_budget {
            return Err(Error::BudgetExceeded {
                required: count,
                budget: request.token_budget,
            });
        }
        let actual = store.project(project.id)?.project_revision;
        if actual != project.project_revision {
            return Err(Error::RevisionConflict {
                expected: project.project_revision,
                actual,
            });
        }
        return crate::public_context(Ok(WorkContextReport {
            level: "L1",
            selection_basis: selection.basis,
            goal_selection_basis: goal_basis,
            session_id: selection.session.map(|s| s.id),
            checkpoint_id: None,
            delta_after_revision: None,
            work_context: None,
            completeness,
            omitted_refs: vec![],
            diagnostic_text: Some(text),
        }));
    };
    let scope = scope(request, selection.session.as_ref());
    let hard = hard_context(store, project.id, key, branch, &scope)?;
    let rules = select_rules(store, &project, Some(work), &scope)?;
    let related = if let Some(read) = &read {
        related_work_in_workstream(
            read,
            key,
            branch,
            request.source_sha.as_deref(),
            scope.paths.as_deref(),
        )?
    } else {
        let mut related =
            crate::related::RelatedSelection::dependencies(store, project.id, key, branch)?;
        related.decisions(store, scope.paths.as_deref())?;
        related.branch_evidence(store, request.source_sha.as_deref())?;
        related.finish(store)?
    };
    let linked_goal_keys = if request.goal_keys.is_empty() {
        if let Some(read) = &read {
            read.goal_links_for(key)?
        } else {
            store
                .work_goal_links(project.id)?
                .into_iter()
                .filter(|edge| edge.from_key == key)
                .map(|edge| edge.to_key)
                .collect()
        }
    } else {
        Vec::new()
    };
    if !linked_goal_keys.is_empty() {
        goal_basis = "work_goal_links";
    }
    let selection_keys = if !request.goal_keys.is_empty() {
        request.goal_keys.clone()
    } else {
        linked_goal_keys.clone()
    };
    let selected_goals = if let Some(read) = &read {
        if request.goal_keys.is_empty() && !linked_goal_keys.is_empty() {
            let mut goals = read.goals()?;
            goals.retain(|goal| {
                linked_goal_keys
                    .iter()
                    .any(|link| link == &goal.item.meta.external_key)
            });
            goals
        } else {
            scoped_goals(read, &request.goal_keys)?
        }
    } else {
        goals(store, &project, &sources, &selection_keys)?
    };
    if let Some(read) = &read {
        let mut ids = BTreeSet::from([work.source.id]);
        ids.extend(hard.source_revisions.iter().map(|s| s.id));
        ids.extend(related.source_revisions.iter().map(|s| s.id));
        ids.extend(selected_goals.iter().map(|g| g.source.id));
        ids.extend(rules.soft.iter().chain(&rules.info).map(|r| r.source.id));
        ids.extend(rules.unknown.iter().map(|r| r.rule.source.id));
        if let Some(source) = read.authority_source()? {
            ids.insert(source.id);
        }
        sources.retain(|s| ids.contains(&s.id) || s.domain == "rules");
    }
    let delta_request = DeltaRequest {
        baseline: request.delta_baseline.clone(),
        session_id: selection.session.as_ref().map(|s| s.id),
        ..Default::default()
    };
    let delta = if let Some(read) = &read {
        let facts = delta_facts(&selected_goals, &rules, &related);
        crate::delta::recent_delta_in_workstream(store, read, key, branch, &delta_request, &facts)?
    } else {
        recent_delta(store, project.id, key, branch, &delta_request)?
    };
    let mut completeness = assess_completeness(CompletenessFacts {
        project: &project,
        branch,
        work_key: key,
        work: Some(work),
        hard: Some(&hard),
        related: Some(&related),
        sources: &sources,
        refresh: Some(&refresh),
    })?;
    completeness.branch_context = Some(branch_context.clone());
    if read
        .as_ref()
        .is_some_and(|read| read.workstream().state != WorkstreamState::Active)
    {
        completeness.work_state_complete = false;
        add_issue(
            &mut completeness,
            "work_state_complete",
            "workstream_inactive",
            key,
            "Workstream is paused or archived; inspect current policy before continuing execution",
        );
    }
    if read.is_some() && !delta.gaps.is_empty() {
        add_issue(
            &mut completeness,
            "recent_delta",
            "scoped_delta_incomplete",
            key,
            delta.gaps.join("; "),
        );
    }
    let mut goal_complete = !selected_goals.is_empty();
    if selected_goals.is_empty() {
        add_issue(
            &mut completeness,
            "goal_context_complete",
            "goal_context_missing",
            key,
            "No current primary project goal was found. Run awr intake inspect --json; reuse goal material or add goals to the primary YAML ledger, link work with goal/goals, and recheck. Keep uncertain intent draft rather than inventing a goal",
        );
    }
    for requested in selection_keys.iter().collect::<BTreeSet<_>>() {
        if !selected_goals
            .iter()
            .any(|g| &g.item.meta.external_key == requested)
        {
            goal_complete = false;
            add_issue(
                &mut completeness,
                "goal_context_complete",
                "goal_not_found",
                requested.clone(),
                if request.goal_keys.is_empty() {
                    "Linked goal has no current source projection. Use the exact goals[].key."
                } else {
                    "Explicit goal has no current source projection"
                },
            );
        }
    }
    let mut required = hard_chunks(&hard)?;
    if read.is_some() {
        for unknown in &hard.unresolved {
            required.push(chunk(
                format!("unresolved-rule:{}", unknown.rule.item.meta.id),
                ContextSection::Rules,
                format!(
                    "Applicability/severity unresolved: {}\nScope: {}\n{}",
                    unknown.reasons.join("; "),
                    serde_json::to_string(&unknown.rule.item.scope)?,
                    unknown.rule.item.text
                ),
                vec![entity(&unknown.rule.item.meta, "rule")],
            ));
        }
    }

    if let Some(policy) = OrdinaryWorkPolicy::from_config(&work.source.config)? {
        if policy.work_items.iter().any(|w| w == key) {
            required.push(chunk("ordinary-work-policy", ContextSection::Metadata,
                format!("Ordinary work policy (explicit source configuration): {}\nPolicy fingerprint: {}\nOnly a protected ordinary confirmation may record user approval or a business check; it is never engineering verification. An Agent cannot change this policy through a work edit. Recorded source confirmation: {}", serde_json::to_string(&policy)?, policy.fingerprint()?, serde_json::to_string(&work.item.ordinary_completion)?),
                vec![entity(&work.item.meta, "work_item")]));
        }
    }
    if awr_source::minimal_context(&sources) {
        required.push(chunk("context-profile", ContextSection::Metadata, "Context profile: minimal. Separate plan/rule sources are optional; every configured hard rule still applies. Check project organization before business execution.", vec![]));
    }
    // Every execution on this work/branch is mandatory, including completed results and
    // unverified nonterminal records. Budget overflow is explicit, never silent omission.
    let executions = if let Some(read) = &read {
        read.context_executions(key, branch)?
    } else {
        store.executions(project.id, Some(work.item.meta.id))?
    };
    for execution in executions.iter().filter(|e| e.branch_id == branch) {
        required.push(chunk(
            format!("execution:{}", execution.id),
            ContextSection::Executions,
            execution.continuity_text()?,
            vec![SelectedEntity {
                kind: "execution".into(),
                id: execution.id,
                revision: execution.revision,
            }],
        ));
    }
    required.push(chunk(
        "branch-context", ContextSection::Metadata,
        format!("Work branch: {} | revision: {:?} | fork: {} | parent: {}\nGit ref at creation: {}; recorded commit: {}\nSource basis: {}\nRuntime scope: {}",
            branch_context.name, branch_context.branch_revision, branch_context.fork_project_revision,
            branch_context.parent_branch_id.map(|id| id.to_string()).unwrap_or_else(|| "main".into()),
            branch_context.git_ref.as_deref().unwrap_or("unbound"),
            branch_context.git_binding.as_ref().map(|g| g.commit_sha.as_str()).unwrap_or("unverified"),
            branch_context.source_basis, branch_context.runtime_scope),
        branch_context.branch_id.map(|id| vec![SelectedEntity { kind: "branch".into(), id, revision: branch_context.branch_revision.unwrap_or(0) }]).unwrap_or_default(),
    ));
    let mut optional_chunks = Vec::new();
    let mut omitted_refs = Vec::new();
    required.push(chunk(
        "identity",
        ContextSection::Metadata,
        format!(
            "Intent: {}\nSelection: {}\nSession: {}\nPhase: {}\nTitle: {}",
            request.intent,
            selection.basis,
            selection
                .session
                .as_ref()
                .map(|s| s.id.to_string())
                .unwrap_or_else(|| "none".into()),
            work.item.milestone.as_deref().unwrap_or("unknown"),
            work.item.title
        ),
        vec![],
    ));
    for goal in &selected_goals {
        if goal.source.freshness != Freshness::Fresh
            || goal.item.status.trim().is_empty()
            || matches!(
                goal.item.status.trim().to_ascii_lowercase().as_str(),
                "unknown" | "draft" | "candidate" | "pending" | "needs_confirmation"
            )
            || (goal
                .source
                .locator
                .replace('\\', "/")
                .contains(".awr/intake/")
                && goal.item.title == "Establish a verified project baseline")
            || goal.item.title.trim().is_empty()
        {
            goal_complete = false;
            add_issue(
                &mut completeness,
                "goal_context_complete",
                "goal_state_incomplete",
                goal.item.meta.external_key.clone(),
                "Goal title/status must be declared and its source current; drafts and intake placeholders do not establish business intent. Run awr intake inspect --json and record source-backed goal confirmation before business execution",
            );
        }
        required.push(chunk(
            format!("goal:{}", goal.item.meta.external_key),
            ContextSection::Goal,
            format!(
                "{}\nStatus: {}\nSuccess criteria:\n{}",
                goal.item.title,
                goal.item.status,
                goal.item.success_criteria.join("\n")
            ),
            vec![entity(&goal.item.meta, "goal")],
        ));
        if !goal.item.summary.trim().is_empty() {
            optional_chunks.push(optional(
                10,
                goal.item.meta.revision,
                chunk(
                    format!("goal-body:{}", goal.item.meta.external_key),
                    ContextSection::Goal,
                    goal.item.summary.clone(),
                    vec![entity(&goal.item.meta, "goal")],
                ),
            ));
        }
    }
    if !selection_keys.is_empty() {
        let universe = if let Some(read) = &read {
            read.goals()?
        } else {
            goals(store, &project, &sources, &[])?
        };
        let selected_keys = selected_goals
            .iter()
            .map(|goal| goal.item.meta.external_key.as_str())
            .collect::<BTreeSet<_>>();
        for goal in universe {
            if selected_keys.contains(goal.item.meta.external_key.as_str())
                || !goal_needs_confirmation(&goal)
            {
                continue;
            }
            completeness
                .project_notices
                .push(project_goal_notice(&goal));
        }
        completeness.project_notices.sort();
        completeness.project_notices.dedup();
    }
    completeness.goal_context_complete = Some(goal_complete);
    if !goal_complete {
        completeness.complete = false;
        completeness.status = "CONTEXT INCOMPLETE";
    }
    if !work.item.summary.trim().is_empty() {
        optional_chunks.push(optional(
            20,
            work.item.meta.revision,
            chunk(
                "work-summary",
                ContextSection::Work,
                work.item.summary.clone(),
                vec![entity(&work.item.meta, "work_item")],
            ),
        ));
    }
    for dependency in &related.unavailable_dependencies {
        required.push(chunk(format!("unavailable-dependency:{}",dependency.edge_id),ContextSection::Dependencies,
            format!("Required dependency {} declared by {} is unavailable in this workstream. Obtain an authorized delivery; do not treat it as satisfied.",dependency.edge_id,dependency.from_work_key),vec![]));
    }
    for dependency in &related.unresolved_dependencies {
        required.push(chunk(format!("required:{}",dependency.meta.external_key),ContextSection::Dependencies,format!("{}\nStatus: {} (raw {})\nFreshness: {:?}\nBlocker present: {}\nBlocker:\n{}\nNext Action:\n{}",dependency.title,serde_json::to_string(&dependency.status)?,serde_json::to_string(&dependency.raw_status)?,dependency.freshness,dependency.blocker.is_some(),dependency.blocker.as_deref().unwrap_or(""),dependency.next_action),vec![entity(&dependency.meta,"work_item")]));
    }
    for dependency in &related.resolved_dependencies {
        optional_chunks.push(optional(
            40,
            dependency.meta.revision,
            chunk(
                format!("resolved:{}", dependency.meta.external_key),
                ContextSection::Dependencies,
                format!(
                    "{}\nStatus: {}\nRaw status: {}",
                    dependency.title,
                    serde_json::to_string(&dependency.status)?,
                    serde_json::to_string(&dependency.raw_status)?
                ),
                vec![entity(&dependency.meta, "work_item")],
            ),
        ));
    }
    required.push(chunk(
        "dependency-summary",
        ContextSection::Dependencies,
        format!(
            "Required dependencies: {} unresolved, {} resolved; {} missing, {} cycle members.",
            related.unresolved_dependencies.len(),
            related.resolved_dependencies.len(),
            related.missing_dependencies.len(),
            related.dependency_cycles.len()
        ),
        vec![],
    ));
    for rule in rules.soft.into_iter().chain(rules.info) {
        optional_chunks.push(optional(
            if rule.item.severity == Some(Severity::Soft) {
                50
            } else {
                70
            },
            rule.item.meta.revision,
            chunk(
                rule.item.meta.external_key.clone(),
                ContextSection::Rules,
                format!(
                    "Severity: {}\nScope: {}\n{}",
                    serde_json::to_string(&rule.item.severity)?,
                    serde_json::to_string(&rule.item.scope)?,
                    rule.item.text
                ),
                vec![entity(&rule.item.meta, "rule")],
            ),
        ));
    }
    for rule in rules.unknown.iter().filter(|r| {
        r.rule.item.severity == Some(Severity::Soft) || r.rule.item.severity == Some(Severity::Info)
    }) {
        omitted_refs.push(ContextOmission {
            kind: "rule",
            reference: rule.rule.item.meta.id.to_string(),
            count: 1,
            reason: "optional rule applicability is unresolved",
        });
    }
    for decision in &related.accepted_decisions {
        required.push(chunk(
            format!("decision:{}", decision.meta.external_key),
            ContextSection::Decisions,
            format!(
                "{}\nStatus: {} (raw {})\nAffected work: {}\nPaths: {}\nDecision:\n{}",
                decision.title,
                serde_json::to_string(&decision.status)?,
                serde_json::to_string(&decision.raw_status)?,
                serde_json::to_string(&decision.affected_keys)?,
                serde_json::to_string(&decision.paths)?,
                decision.statement
            ),
            vec![entity(&decision.meta, "decision")],
        ));
    }
    let delta_text = if read.is_some() {
        format!(
            "Scoped changes after revision {} ({}). Checkpoint: {}. Important events: {}; omitted by event limit: {}.",
            delta.after_revision,
            delta.baseline_origin,
            delta
                .checkpoint_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".into()),
            delta.events.important_event_count,
            delta.events.omitted_important_events
        )
    } else {
        format!(
            "After project revision {} ({}) through {}. Checkpoint: {}. Important events: {}; omitted by event limit: {}.",
            delta.after_revision,
            delta.baseline_origin,
            delta.events.project_revision,
            delta
                .checkpoint_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".into()),
            delta.events.important_event_count,
            delta.events.omitted_important_events
        )
    };
    required.push(chunk(
        "delta-baseline",
        ContextSection::Delta,
        delta_text,
        vec![],
    ));
    if let Some(id) = delta.checkpoint_id {
        let cp = match &read {
            Some(read) => read.context_checkpoint(id, key, branch)?,
            None => store.checkpoint(project.id, id)?,
        };
        required.push(chunk("checkpoint",ContextSection::Delta,format!("Checkpoint context hash: {}\nCheckpoint Next Action:\n{}\nCheckpoint Open Loops:\n{}",cp.context_hash,cp.next_action,cp.open_loops.join("\n")),vec![SelectedEntity{kind:"checkpoint".into(),id,revision:cp.revision}]));
        optional_chunks.push(optional(
            25,
            cp.project_revision,
            chunk(
                "checkpoint-digest",
                ContextSection::Delta,
                cp.digest,
                vec![SelectedEntity {
                    kind: "checkpoint".into(),
                    id: cp.id,
                    revision: cp.revision,
                }],
            ),
        ));
    }
    for source in &delta.events.source_changes {
        if read.is_some() {
            required.push(chunk(format!("source:{}",source.source_id),ContextSection::Delta,
                format!("Scoped source changes: {} identities; {} details omitted; {} events lack entity history.",source.changed_entity_count,source.omitted_entities,source.legacy_events),vec![]));
        } else {
            required.push(chunk(format!("source:{}",source.source_id),ContextSection::Delta,format!("{} source events; latest {}. Before known: {}\nBefore: {}\nAfter: {}\nChanged identities: {}; omitted identities: {}; legacy events without entity history: {}.\nHistory events: {} .. {} (r{}..r{})",source.event_count,source.latest_operation,source.before_known,source_state(source.before.as_ref()),source_state(source.after.as_ref()),source.changed_entity_count,source.omitted_entities,source.legacy_events,source.first_event.id,source.last_event.id,source.first_event.project_revision,source.last_event.project_revision),vec![]));
        }
        for changed in &source.changed_entities {
            optional_chunks.push(optional(
                35,
                changed.last_event.project_revision,
                chunk(
                    format!("change:{}:{}", source.source_id, changed.id),
                    ContextSection::Delta,
                    format!(
                        "{} {}: {} {:?} -> {:?}, {} changes; event {}",
                        changed.kind,
                        changed.external_key,
                        changed.latest_action,
                        changed.before_revision,
                        changed.after_revision,
                        changed.occurrences,
                        changed.last_event.id
                    ),
                    vec![SelectedEntity {
                        kind: format!("delta:{}", changed.kind),
                        id: changed.id,
                        revision: changed
                            .after_revision
                            .or(changed.before_revision)
                            .unwrap_or(0),
                    }],
                ),
            ));
        }
        if source.omitted_entities > 0 {
            omitted_refs.push(ContextOmission {
                kind: "source_entity_changes",
                reference: source.source_id.to_string(),
                count: source.omitted_entities,
                reason: "per-source delta detail limit; use immutable source events",
            });
        }
    }
    for event in &delta.events.important_events {
        let chunk = chunk(
            format!("event:{}", event.event.id),
            ContextSection::Delta,
            format!(
                "{} {} at r{}: {}",
                event.importance, event.event_type, event.event.project_revision, event.summary
            ),
            vec![SelectedEntity {
                kind: "event".into(),
                id: event.event.id,
                revision: event.event.project_revision,
            }],
        );
        if event.importance == "critical" {
            required.push(chunk);
        } else {
            optional_chunks.push(optional(30, event.event.project_revision, chunk));
        }
    }
    if delta.events.omitted_important_events > 0 {
        omitted_refs.push(ContextOmission {
            kind: "important_events",
            reference: format!("after_revision:{}", delta.after_revision),
            count: delta.events.omitted_important_events,
            reason: "delta event limit; drill down through event history",
        });
    }
    let folded = delta
        .events
        .process_history
        .iter()
        .map(|g| {
            format!(
                "{} {}: {} at/before baseline, {} after",
                g.event_type, g.importance, g.before_or_at_baseline, g.after_baseline
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !folded.is_empty() {
        optional_chunks.push(optional(
            60,
            if read.is_some() {
                delta
                    .events
                    .important_events
                    .iter()
                    .map(|e| e.event.project_revision)
                    .max()
                    .unwrap_or(delta.after_revision)
            } else {
                project.project_revision
            },
            chunk(
                "folded-process-history",
                ContextSection::Delta,
                folded,
                vec![],
            ),
        ));
    }
    for gap in &delta.gaps {
        required.push(chunk(
            format!("delta-gap:{}", required.len()),
            ContextSection::Delta,
            gap.clone(),
            vec![],
        ));
    }
    for evidence in &related.evidence {
        optional_chunks.push(optional(
            55,
            evidence.revision,
            chunk(
                format!("evidence:{}", evidence.external_key),
                ContextSection::Evidence,
                format!(
                    "{}\nLevel: {:?}; currency: {:?}\nReport: {}\nSource SHA: {}\nScope: {}",
                    evidence.summary,
                    evidence.level,
                    evidence.currency,
                    evidence.locator,
                    evidence.source_sha.as_deref().unwrap_or("unknown"),
                    evidence.scope.join(", ")
                ),
                vec![SelectedEntity {
                    kind: "evidence".into(),
                    id: evidence.id,
                    revision: evidence.revision,
                }],
            ),
        ));
    }
    let evidence_text = if related.evidence_gaps.is_empty() {
        "No metadata evidence gaps found; verification commands were not executed by compilation."
            .into()
    } else {
        related
            .evidence_gaps
            .iter()
            .map(|g| format!("{} [{}]: {}", g.code, g.reference, g.reason))
            .collect::<Vec<_>>()
            .join("\n")
    };
    required.push(chunk(
        "evidence-gaps",
        ContextSection::Evidence,
        evidence_text,
        vec![],
    ));
    completeness.issues.sort();
    completeness.issues.dedup();
    let mut completeness_text = format!(
        "{}\n{}\n{}",
        completeness.status,
        completeness
            .issues
            .iter()
            .map(|i| format!("{} [{}]: {}", i.code, i.reference, i.reason))
            .collect::<Vec<_>>()
            .join("\n"),
        completeness.assessment_scope
    );
    if !completeness.project_notices.is_empty() {
        completeness_text.push_str(
            "\nProject notices (not linked to this work; they do not make this work incomplete):\n",
        );
        completeness_text.push_str(
            &completeness
                .project_notices
                .iter()
                .map(|notice| format!("{} [{}]: {}", notice.code, notice.reference, notice.reason))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    required.push(chunk(
        "completeness",
        ContextSection::Metadata,
        completeness_text,
        vec![],
    ));
    let identity = ContextIdentity {
        project_id: project.id,
        project_key: project.external_key.clone(),
        project_revision: project.project_revision,
        work_item_id: work.item.meta.id,
        work_item_key: key.into(),
        work_item_revision: work.item.meta.revision,
        branch_id: branch,
        source_versions: completeness.source_versions.clone(),
    };
    let mut normalized_request = request.clone();
    normalized_request.goal_keys.sort();
    normalized_request.goal_keys.dedup();
    for values in [&mut normalized_request.paths, &mut normalized_request.tags]
        .into_iter()
        .flatten()
    {
        values.sort();
        values.dedup();
    }
    let completeness_binding = if read.is_some() {
        serde_json::json!({
            "complete":completeness.complete,"source_fresh":completeness.source_fresh,"work_state_complete":completeness.work_state_complete,
            "acceptance_complete":completeness.acceptance_complete,"rules_complete":completeness.rules_complete,
            "dependencies_complete":completeness.dependencies_complete,"decision_context_complete":completeness.decision_context_complete,
            "goal_context_complete":completeness.goal_context_complete,"unresolved_dependencies":completeness.unresolved_required_dependencies,
            "evidence_gaps":completeness.evidence_gaps,"issues":completeness.issues
        })
    } else {
        serde_json::to_value(&completeness)?
    };
    let mut binding = serde_json::json!({"request":normalized_request,"effective_rule_scope":hard.scope,"selection":selection.basis,"goal_selection":goal_basis,"session":selection.session.as_ref().map(|s|s.id),"delta_baseline":delta.after_revision,"checkpoint":delta.checkpoint_id,"completeness":completeness_binding,"omitted_refs":omitted_refs});
    let budget = if let Some(read) = &read {
        binding["source_policies"] = serde_json::to_value(sources.iter().map(|s|serde_json::json!({"id":s.id,"adapter":s.adapter,"format":s.format,"role":s.role,"config":s.config})).collect::<Vec<_>>())?;
        let scope = WorkstreamContextIdentity {
            version: 1,
            workstream_id: read.workstream().id,
            authority_version: read.workstream().authority_version,
            ownership_revision: read.ownership(key)?.revision,
            reader_binding: read.scope_binding().into(),
        };
        budget_workstream_context(
            &identity,
            &scope,
            &binding,
            &required,
            &optional_chunks,
            request.token_budget,
        )?
    } else {
        budget_context(
            &identity,
            &binding,
            &required,
            &optional_chunks,
            request.token_budget,
        )?
    };
    crate::budget::validate_reviewed_budget(store, &budget, &required, &optional_chunks)
        .map_err(crate::context_output_error)?;
    let actual = store.project(project.id)?.project_revision;
    if actual != project.project_revision {
        return Err(Error::RevisionConflict {
            expected: project.project_revision,
            actual,
        });
    }
    let report = WorkContextReport {
        level: "L1",
        selection_basis: selection.basis,
        goal_selection_basis: goal_basis,
        session_id: selection.session.map(|s| s.id),
        checkpoint_id: delta.checkpoint_id,
        delta_after_revision: Some(delta.after_revision),
        work_context: Some(budget),
        completeness,
        omitted_refs,
        diagnostic_text: None,
    };
    let mut checked = serde_json::to_value(&report)?;
    // The exact rendered bytes were checked above with their per-chunk provenance.
    checked["work_context"]
        .as_object_mut()
        .unwrap()
        .remove("rendered_context");
    store
        .ensure_source_output(&checked)
        .map_err(crate::context_output_error)?;
    Ok(report)
}

pub(crate) fn scoped_read(
    store: &Store,
    project: &Project,
    request: &ContextRequest,
    trusted: Option<(&WorkstreamAccess, &WorkstreamReadSelection)>,
) -> Result<Option<WorkstreamRead>> {
    if trusted.is_none() && !store.workstreams_enabled(project.id)? {
        return Ok(None);
    }
    let local;
    let (access, mut selection) = if let Some((access, selection)) = trusted {
        (access, selection.clone())
    } else {
        let catalog = store.workstream_catalog(project.id)?;
        local = WorkstreamAccess {
            project_id: project.id.to_string(),
            subject: "local-project-owner".into(),
            grants: catalog
                .workstreams
                .iter()
                .map(|s| WorkstreamGrant {
                    workstream_id: s.id,
                    authority_version: s.authority_version,
                    read: true,
                    write: false,
                    manage: false,
                })
                .collect(),
        };
        (&local, WorkstreamReadSelection::default())
    };
    selection.work_item_key = request.work_item_key.clone();
    selection.session_id = request.session_id;
    Ok(Some(store.read_workstream(
        project.id,
        access,
        &selection,
        256 * 1024 * 1024,
    )?))
}
pub(crate) fn scoped_goals(read: &WorkstreamRead, keys: &[String]) -> Result<Vec<Projected<Goal>>> {
    let mut goals = read.goals()?;
    if !keys.is_empty() {
        if keys
            .iter()
            .any(|key| !goals.iter().any(|g| g.item.meta.external_key == *key))
        {
            return Err(WorkstreamError::AccessDenied.into());
        }
        goals.retain(|g| keys.contains(&g.item.meta.external_key));
    } else {
        goals.retain(|g| {
            ![
                "completed",
                "done",
                "cancelled",
                "canceled",
                "archived",
                "retired",
            ]
            .contains(&g.item.status.to_ascii_lowercase().as_str())
        });
    }
    Ok(goals)
}
pub(crate) fn delta_facts(
    goals: &[Projected<Goal>],
    rules: &RuleSelection,
    related: &RelatedWorkContext,
) -> Vec<(String, Id)> {
    let mut facts = Vec::new();
    facts.extend(goals.iter().map(|g| ("goals".into(), g.item.meta.id)));
    facts.extend(
        rules
            .hard
            .iter()
            .chain(&rules.soft)
            .chain(&rules.info)
            .map(|r| ("rules".into(), r.item.meta.id)),
    );
    facts.extend(
        rules
            .unknown
            .iter()
            .map(|r| ("rules".into(), r.rule.item.meta.id)),
    );
    facts.extend(
        related
            .accepted_decisions
            .iter()
            .map(|d| ("decisions".into(), d.meta.id)),
    );
    facts.extend(
        related
            .uncertain_decisions
            .iter()
            .map(|d| ("decisions".into(), d.meta.id)),
    );
    facts.extend(related.evidence.iter().map(|e| ("evidence".into(), e.id)));
    facts
}
