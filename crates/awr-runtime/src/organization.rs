//! Source-backed project organization. This diagnoses work; it never writes source or invents intent.
use crate::read::read_registered_file;
use awr_core::*;
use awr_store::Store;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationState {
    NotInitialized,
    SourceUnreadable,
    NeedsOrganization,
    Ready,
    Blocked,
    AwaitingVerification,
    Completed,
    CompletedUnderPolicy,
    ClosedWithoutCompletion,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizationGap {
    pub code: String,
    pub target: String,
    pub detail: String,
    pub source_refs: Vec<SourceRef>,
    pub action: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizationAction {
    pub id: &'static str,
    pub instruction: &'static str,
    pub required_fields: Vec<&'static str>,
    pub done_when: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizationReport {
    pub schema_version: u32,
    pub state: OrganizationState,
    pub context_profile: &'static str,
    pub project_revision: Option<Revision>,
    pub business_execution_ready: bool,
    pub executable_work: Vec<String>,
    pub executable_work_total: usize,
    #[serde(skip)]
    pub(crate) executable_keys: BTreeSet<String>,
    #[serde(skip)]
    pub(crate) structured_keys: BTreeSet<String>,
    pub sources: Vec<serde_json::Value>,
    pub goals: Vec<serde_json::Value>,
    pub goal_total: usize,
    pub gaps: Vec<OrganizationGap>,
    pub gap_total: usize,
    pub historical_gap_total: usize,
    pub truncated: bool,
    pub actions: Vec<OrganizationAction>,
    pub minimal_structure: &'static str,
    pub semantic_review: &'static str,
    pub completion_basis: &'static str,
    pub source_sha: Option<String>,
    pub source_completed: usize,
    pub verified_completed: usize,
    pub completion_not_checked: usize,
    pub completion_check_blocked: usize,
    pub completion_verification_failed: usize,
    pub user_confirmed_completed: usize,
    pub business_checked_completed: usize,
    pub ordinary_work_policies: Vec<serde_json::Value>,
    pub source_cancelled: usize,
    pub source_archived: usize,
    pub recheck: Vec<String>,
    pub next_action: String,
}

const MINIMUM: &str = "Reuse authoritative material. A small project needs one source-declared goal and work items with id, title, status, goal, acceptance and next_action. Goals and work_items may share one YAML ledger. Add plans/milestones only when needed; do not create separate plan/rule files just to satisfy intake.";
const SEMANTIC: &str = "AWR checks declared structure, references and evidence bindings. The Coding Agent must compare user intent, README, implementation and development history, record its reasoning and sources, and keep uncertain goals as draft/candidate/needs_confirmation. Ask the user only for intent that cannot be established. An active/confirmed goal is a source assertion, not proof that AWR understood or approved the business intent.";

impl OrganizationReport {
    pub fn intake_preview(
        sources: Vec<serde_json::Value>,
        missing: &[String],
        ambiguous: &[String],
    ) -> Self {
        let mut report = Self::unavailable(
            false,
            "Source inventory is a preview; no business readiness or completion is inferred.",
        );
        report.actions.clear();
        report.sources = sources;
        for domain in missing {
            report.gap(&format!("{domain}_missing"), domain, "No authoritative source was identified. Review existing material before adopting a proposed intake source.", vec![], if domain == "goal" { "goals" } else { "work" });
        }
        for domain in ambiguous {
            report.gap("authority_ambiguous", domain, "Several authority candidates exist. Select the actual authority in the draft mapping; do not combine conflicting goals or progress silently.", vec![], "sources");
        }
        if !report.actions.iter().any(|a| a.id == "sources") {
            report.actions.push(action_for("sources"));
        }
        report.finish();
        report
    }
    pub fn unavailable(initialized: bool, reason: &str) -> Self {
        let mut report = Self::new();
        report.state = if initialized {
            OrganizationState::SourceUnreadable
        } else {
            OrganizationState::NotInitialized
        };
        report.gap(
            if initialized {
                "source_unreadable"
            } else {
                "not_initialized"
            },
            "project",
            reason,
            vec![],
            "sources",
        );
        report.finish();
        report
    }

    fn new() -> Self {
        Self {
            schema_version: 1,
            state: OrganizationState::NeedsOrganization,
            context_profile: "standard",
            project_revision: None,
            business_execution_ready: false,
            executable_work: vec![],
            executable_work_total: 0,
            executable_keys: BTreeSet::new(),
            structured_keys: BTreeSet::new(),
            sources: vec![],
            goals: vec![],
            goal_total: 0,
            gaps: vec![],
            gap_total: 0,
            historical_gap_total: 0,
            truncated: false,
            actions: vec![],
            minimal_structure: MINIMUM,
            semantic_review: SEMANTIC,
            completion_basis: "Source status only; completion reports have not been verified for an explicit source SHA. This is not a release or business-acceptance decision.",
            source_sha: None,
            source_completed: 0,
            verified_completed: 0,
            completion_not_checked: 0,
            completion_check_blocked: 0,
            completion_verification_failed: 0,
            user_confirmed_completed: 0,
            business_checked_completed: 0,
            ordinary_work_policies: vec![],
            source_cancelled: 0,
            source_archived: 0,
            recheck: vec![
                "awr".into(),
                "intake".into(),
                "inspect".into(),
                "--json".into(),
            ],
            next_action: String::new(),
        }
    }

    fn gap(
        &mut self,
        code: &str,
        target: &str,
        detail: &str,
        refs: Vec<SourceRef>,
        action: &'static str,
    ) {
        self.gap_total += 1;
        // Count all findings, but bound the returned diagnostic sample. Readiness never uses this sample.
        if self.gaps.len() < 100 {
            self.gaps.push(OrganizationGap {
                code: code.into(),
                target: target.into(),
                detail: safe_diagnostic(detail),
                source_refs: refs,
                action,
            });
        }
        if !self.actions.iter().any(|a| a.id == action) {
            self.actions.push(action_for(action));
        }
    }

    fn finish(&mut self) {
        self.truncated = self.gap_total > self.gaps.len()
            || self.executable_work_total > self.executable_work.len()
            || self.goal_total > self.goals.len();
        self.actions.sort_by_key(|a| match a.id {
            "sources" => 0,
            "goals" => 1,
            "work" => 2,
            "dependencies" => 3,
            "evidence" => 4,
            _ => 5,
        });
        if !self.actions.is_empty() {
            self.actions.push(action_for("recheck"));
        }
        self.next_action = match self.state {
            OrganizationState::NotInitialized => "Preview awr init, reuse existing sources, then accept the reviewed mapping. Initialization establishes sources, not business readiness.",
            OrganizationState::SourceUnreadable => "Repair the source/manifest/cache error first; do not replace unreadable authority with an empty ledger. Reindex and run awr intake inspect.",
            OrganizationState::NeedsOrganization => "Follow actions in order; organize known facts and keep uncertain intent explicit. Recheck with awr intake inspect after editing the authoritative sources.",
            OrganizationState::Ready => "Select an executable_work item, inspect its work context and claim ownership before execution. Findings on other items remain open.",
            OrganizationState::Blocked => "The work structure is present, but prerequisites or blockers prevent execution. Resolve the cited dependencies and recheck.",
            OrganizationState::AwaitingVerification => "Source tasks are marked completed, but project completion is unverified. Supply current acceptance reports and an explicit source SHA to awr intake inspect --source-sha <sha>.",
            OrganizationState::Completed => "All non-cancelled ledger work has matching passing acceptance reports for the explicit source SHA. Check the project's separate release/business-acceptance gates before delivery claims.",
            OrganizationState::CompletedUnderPolicy => "The current work scope is complete under its explicit policies. User confirmations and business checks are separate from engineering verification; inspect each confirmation's actor, basis, artifacts and policy version.",
            OrganizationState::ClosedWithoutCompletion => "The ledger contains cancelled work or cancelled required scope; this is not a completed project. Clarify remaining scope before selecting new work.",
        }.into();
    }

    pub fn rendered(&self) -> String {
        let mut text = format!(
            "Project organization: {:?}\nBusiness execution ready: {}\nNext: {}\n",
            self.state, self.business_execution_ready, self.next_action
        );
        for gap in &self.gaps {
            text.push_str(&format!(
                "- {} [{}]: {}\n",
                gap.code, gap.target, gap.detail
            ));
            for source in &gap.source_refs {
                text.push_str(&format!(
                    "  Source: {}{} @r{}\n",
                    source.locator,
                    source.pointer.as_deref().unwrap_or_default(),
                    source.source_revision
                ));
            }
        }
        for (i, action) in self.actions.iter().enumerate() {
            text.push_str(&format!(
                "{}. {}\n   Done when: {}\n",
                i + 1,
                action.instruction,
                action.done_when
            ));
        }
        if self.truncated {
            text.push_str(&format!(
                "Diagnostic sample: {} of {}; fix these and recheck for remaining findings.\n",
                self.gaps.len(),
                self.gap_total
            ));
        }
        text.push_str(&format!(
            "{}\n{}\n",
            self.minimal_structure, self.semantic_review
        ));
        text
    }
}

fn action_for(id: &'static str) -> OrganizationAction {
    let (instruction, fields, done_when) = match id {
        "sources" => (
            "Inspect the cited files and source errors. Select the authoritative goal/ledger mapping; preserve existing sources. For a new project, review awr init before accepting it. Repair unreadable sources in place and run awr source reindex.",
            vec!["source path", "adapter", "authority role"],
            "Sources can be indexed without errors; missing, empty and unreadable sources are distinguished.",
        ),
        "goals" => (
            "Use user intent and the listed source material to write the smallest goal description and success criteria. Record the source/reasoning in its summary. Keep uncertain intent draft/candidate/needs_confirmation; use active/confirmed only when supported. Reuse existing goals instead of inventing new ones.",
            vec![
                "id",
                "title",
                "status",
                "summary or success_criteria",
                "provenance",
            ],
            "Each executable task links to a clear source-declared active/confirmed goal; unresolved intent remains visible.",
        ),
        "work" => (
            "Compare README, existing code, development history and the goal. Turn the next concrete delivery into work items; link each task using goal/goals, add acceptance and next_action, and preserve known progress with evidence. A generic intake task is organization work; replace/split proposals into concrete business tasks before execution.",
            vec![
                "id",
                "title",
                "status",
                "goal or goals",
                "acceptance",
                "next_action",
            ],
            "At least one non-intake task has a resolved goal, nonempty acceptance and a concrete next action. Add plans/milestones only for actual coordination needs.",
        ),
        "dependencies" => (
            "Inspect cited required dependencies and blockers. Repair missing links/cycles and declare actual prerequisites; do not mark prerequisites completed just to unlock work.",
            vec!["depends_on", "blocker", "next_action"],
            "The selected work has no unresolved required prerequisite or active blocker.",
        ),
        "evidence" => (
            "Review the original completion claim and its actual reports. Bind every acceptance criterion to a passing report for the intended source SHA; source status and evidence level alone do not prove completion. Recheck with an explicit source SHA.",
            vec![
                "work_item",
                "source_sha",
                "command",
                "scope",
                "verified_at",
                "checks[].criteria",
                "report digest",
            ],
            "Each criterion is covered by a current, hash-verified report at the required evidence level; no business or release gate is inferred.",
        ),
        _ => (
            "Run awr intake inspect --json after source edits. Follow remaining actions until the intended work appears in executable_work; review its context before claiming it. Do not treat draft generation as completion.",
            vec![],
            "The fresh diagnosis resolves the selected work's structural gaps. Semantic uncertainties are resolved or explicitly deferred to unaffected work.",
        ),
    };
    OrganizationAction {
        id,
        instruction,
        required_fields: fields,
        done_when,
    }
}

fn unresolved_goal_detail(
    link_key: &str,
    goals: &BTreeMap<String, &Projected<Goal>>,
    work_open: bool,
) -> String {
    if let Some(goal) = goals.get(link_key) {
        if work_open && finished_goal(&goal.item) && declared_goal(goal) {
            return format!(
                "Goal {link_key} is already finished ({}) while this work remains open. Link an open goal or complete the work.",
                goal.item.status
            );
        }
        return format!(
            "Goal {link_key} is present but not confirmed: {}. Record source-backed confirmation before linking open work to it.",
            undeclared_goal_reason(goal)
        );
    }
    let mut detail =
        format!("Goal {link_key} is not a primary goal key. Links must match goals[].key exactly.");
    let matches = matching_goal_keys(link_key, goals.keys().map(String::as_str));
    if matches.len() == 1 {
        detail.push_str(&format!(
            " The value matches goal key {}; use that exact goals[].key.",
            matches[0]
        ));
    } else if matches.len() > 1 {
        detail.push_str(" The value matches more than one goal key; choose the exact goals[].key.");
    }
    detail
}

fn undeclared_goal_reason(goal: &Projected<Goal>) -> String {
    let mut reasons = Vec::new();
    if goal.source.freshness != Freshness::Fresh {
        reasons.push(format!("source is {:?}", goal.source.freshness));
    }
    if goal.item.title.trim().is_empty() {
        reasons.push("title is empty".into());
    }
    let status = goal.item.status.to_ascii_lowercase();
    if !matches!(
        status.as_str(),
        "active" | "confirmed" | "approved" | "in_progress" | "completed" | "done"
    ) {
        reasons.push(format!("status {} is not confirmed", goal.item.status));
    }
    if goal
        .source
        .locator
        .replace('\\', "/")
        .contains(".awr/intake/")
        && goal.item.title == "Establish a verified project baseline"
    {
        reasons.push("it is the legacy intake placeholder".into());
    }
    if goal.item.summary.trim().is_empty()
        && validate_criteria(&goal.item.success_criteria).is_err()
    {
        reasons.push("summary and success criteria are missing".into());
    }
    if reasons.is_empty() {
        reasons.push("it is not source-declared".into());
    }
    reasons.join("; ")
}

fn matching_goal_keys<'a>(value: &str, keys: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }
    keys.filter(|key| {
        let anchor = key
            .rsplit_once('#')
            .map(|(_, anchor)| anchor)
            .unwrap_or(key);
        *key == value
            || anchor == value
            || key.strip_suffix(value).is_some_and(|prefix| {
                prefix.is_empty() || prefix.ends_with('/') || prefix.ends_with('#')
            })
    })
    .collect()
}

fn declared_goal(goal: &Projected<Goal>) -> bool {
    goal.source.freshness == Freshness::Fresh
        && !goal.item.title.trim().is_empty()
        && matches!(goal.item.status.to_ascii_lowercase().as_str(), "active" | "confirmed" | "approved" | "in_progress" | "completed" | "done")
        // Legacy intake generated an active placeholder. It is still organization work.
        && !(goal.source.locator.replace('\\', "/").contains(".awr/intake/")
            && goal.item.title == "Establish a verified project baseline")
        && (!goal.item.summary.trim().is_empty() || validate_criteria(&goal.item.success_criteria).is_ok())
}

fn inactive_goal(goal: &Goal) -> bool {
    matches!(
        goal.status.to_ascii_lowercase().as_str(),
        "cancelled" | "canceled" | "archived" | "retired"
    )
}
fn finished_goal(goal: &Goal) -> bool {
    matches!(
        goal.status.to_ascii_lowercase().as_str(),
        "completed" | "done"
    )
}

/// Call after source refresh/verification. No authoritative files or runtime state are changed.
pub fn inspect_organization(
    store: &Store,
    project: &Project,
    branch: Option<Id>,
    source_sha: Option<&str>,
    refresh_ok: bool,
    works: &[Projected<WorkItem>],
    readiness: &ReadyReport,
) -> Result<OrganizationReport> {
    if source_sha.is_some_and(|sha| !is_source_sha(sha)) {
        return Err(Error::InvalidInput("source SHA must be a full hash".into()));
    }
    let mut result = OrganizationReport::new();
    result.project_revision = Some(project.project_revision);
    result.source_sha = source_sha.map(str::to_owned);
    result.recheck.extend([
        "--branch".into(),
        branch
            .map(|b| b.to_string())
            .unwrap_or_else(|| "main".into()),
    ]);
    if let Some(sha) = source_sha {
        result.recheck.extend(["--source-sha".into(), sha.into()]);
    }
    let sources = store.sources(project.id)?;
    let minimal = awr_source::minimal_context(&sources);
    for source in &sources {
        if let Some(policy) = OrdinaryWorkPolicy::from_config(&source.config)? {
            result.ordinary_work_policies.push(serde_json::json!({"source_id": source.id, "policy": policy, "fingerprint": policy.fingerprint()?}));
        }
    }
    result.context_profile = if minimal { "minimal" } else { "standard" };
    let rules_defined = minimal || sources.iter().any(|s| s.domain == "rules");
    if !rules_defined {
        result.gap("rules_source_missing", "project", "The preserved standard context profile requires a rules source. Provide the real source, or explicitly select context_profile = minimal in the manifest for a project without one; configured rules remain mandatory.", vec![], "sources");
    }
    result.sources = sources.iter().map(|s| serde_json::json!({"domain":s.domain,"role":s.role,"locator":s.locator,"revision":s.revision,"freshness":s.freshness})).collect();
    let sources_ok = refresh_ok && sources.iter().all(|s| s.freshness == Freshness::Fresh);
    if !sources_ok {
        result.gap("source_unreadable", "project", "Source refresh/verification is incomplete; retained projections are not current authority. Inspect source_issues.", vec![], "sources");
    }
    let goals = store.goals(project.id)?;
    let goals: BTreeMap<_, _> = goals
        .iter()
        .filter(|g| g.source.role == "primary")
        .map(|g| (g.item.meta.external_key.clone(), g))
        .collect();
    result.goal_total = goals.len();
    result.goals = goals.iter().take(100).map(|(key,g)| serde_json::json!({"key":key,"title":g.item.title,"status":g.item.status,"source_declared":declared_goal(g),"confirmation_basis":"source assertion; semantic judgment belongs to the Agent/user","source_ref":g.item.meta.source_ref})).collect();
    if goals.is_empty() {
        result.gap("goal_missing", "project", "No primary goal is projected. Reuse an existing goal source or add goals to the primary YAML ledger.", vec![], "goals");
    }
    for (key, goal) in &goals {
        if !inactive_goal(&goal.item) && !declared_goal(goal) {
            result.gap("goal_unconfirmed", key, "Goal is a draft, unknown, stale, empty or an intake placeholder. Preserve uncertain intent and record its source before declaring it active.", vec![goal.item.meta.source_ref.clone()], "goals");
        }
    }
    if works.is_empty() {
        result.gap(if sources.iter().any(|s| s.domain == "ledger") { "ledger_empty" } else { "ledger_missing" }, "project", "No work is projected. This is not evidence that all project work is complete; establish the current scope and its next delivery.", vec![], "work");
    }
    let links = store.work_goal_links(project.id)?;
    let mut by_work: BTreeMap<&str, Vec<&Edge>> = BTreeMap::new();
    for link in &links {
        by_work.entry(&link.from_key).or_default().push(link);
    }
    let mut unresolved_goal_scope = goals
        .values()
        .any(|g| !inactive_goal(&g.item) && !declared_goal(g));
    let noncancelled: BTreeSet<_> = works
        .iter()
        .filter(|w| w.item.status != WorkStatus::Cancelled)
        .filter(|w| !w.item.archived)
        .map(|w| &w.item.meta.external_key)
        .collect();
    for (key, goal) in &goals {
        if declared_goal(goal)
            && !links
                .iter()
                .any(|link| &link.to_key == key && noncancelled.contains(&link.from_key))
        {
            unresolved_goal_scope = true;
            result.gap("goal_work_missing", key, "This goal has no non-cancelled work association. Define its remaining delivery or record a source-backed scope decision before claiming project completion.", vec![goal.item.meta.source_ref.clone()], "work");
        }
    }
    let readiness: BTreeMap<_, _> = readiness
        .ready
        .iter()
        .chain(&readiness.blocked)
        .map(|r| (r.work.item.meta.external_key.as_str(), r))
        .collect();
    let mut unfinished = 0;
    let mut structured_unfinished = 0;
    let mut required_cancelled = false;
    let mut verification_budget = 16 * 1024 * 1024;
    // Current work gets the diagnostic budget first; history cannot crowd it out.
    let mut ordered: Vec<_> = works.iter().collect();
    ordered.sort_by_key(|w| w.item.status == WorkStatus::Completed);
    for projected in ordered {
        let work = &projected.item;
        let key = &work.meta.external_key;
        let previous_gaps = result.gap_total;
        if work.archived {
            result.source_archived += 1;
            continue;
        }
        if work.status == WorkStatus::Cancelled {
            result.source_cancelled += 1;
            required_cancelled |= work.required;
            continue;
        }
        let mut structured =
            sources_ok && rules_defined && projected.source.freshness == Freshness::Fresh;
        let mut missing = vec![];
        if !minimal
            && work.status != WorkStatus::Completed
            && work
                .milestone
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
        {
            missing.push("milestone (standard context profile)");
        }
        if work.title.trim().is_empty() {
            missing.push("title");
        }
        if work.status == WorkStatus::Unknown {
            missing.push("recognized status");
            result.gap("work_status_unmapped", key, "The source status has no confirmed interpretation. Configure sources.options.status_map in .awr/project.toml, retaining the original ledger vocabulary, then reindex. For first intake use init --status-map SOURCE=CANONICAL. Do not infer completion from an unknown value.", vec![work.meta.source_ref.clone()], "sources");
        }
        if validate_criteria(&work.acceptance).is_err() {
            missing.push("nonempty unique acceptance");
        }
        if work.status != WorkStatus::Completed && work.next_action.trim().is_empty() {
            missing.push("next_action");
        }
        if !missing.is_empty() {
            structured = false;
            result.gap(
                "work_structure_incomplete",
                key,
                &format!("Missing or invalid: {}", missing.join(", ")),
                vec![work.meta.source_ref.clone()],
                "work",
            );
        }
        let work_links = by_work.get(key.as_str());
        if work_links.is_none_or(|v| v.is_empty()) {
            structured = false;
            result.gap("work_goal_missing", key, "Link this work to its goal using goal/goals in YAML or a goal column in the Markdown table. Do not infer a business relationship from filenames.", vec![work.meta.source_ref.clone()], "work");
        } else {
            let work_open = work.status != WorkStatus::Completed;
            for link in work_links.into_iter().flatten() {
                let resolved = goals
                    .get(&link.to_key)
                    .is_some_and(|g| declared_goal(g) && (!work_open || !finished_goal(&g.item)));
                if !resolved {
                    structured = false;
                    result.gap(
                        "work_goal_unresolved",
                        key,
                        &unresolved_goal_detail(&link.to_key, &goals, work_open),
                        vec![link.source_ref.clone()],
                        "goals",
                    );
                }
            }
        }
        if work.kind.as_deref() == Some("intake") && work.status != WorkStatus::Completed {
            structured = false;
            result.gap("intake_work_only", key, "This task organizes the project. Complete the organization and create concrete delivery work; an intake task is not business execution readiness.", vec![work.meta.source_ref.clone()], "work");
        }
        if let Some(milestone) = &work.milestone {
            match store.plan(project.id, milestone) {
                Ok(plan) if plan.source.freshness == Freshness::Fresh => (),
                Ok(_) | Err(Error::NotFound(_)) => {
                    structured = false;
                    result.gap("plan_reference_unresolved", key, &format!("Declared milestone {milestone} is missing or stale; reuse its actual plan source or correct the reference."), vec![work.meta.source_ref.clone()], "work");
                }
                Err(error) => return Err(error),
            }
        }
        if work.status == WorkStatus::Completed {
            result.source_completed += 1;
            if work.ordinary_completion.is_some() {
                match crate::assess_ordinary_completion(store, project.id, projected) {
                    Ok(()) if structured => match work.ordinary_completion.as_ref().unwrap().kind {
                        OrdinaryCompletionKind::UserConfirmation => {
                            result.user_confirmed_completed += 1
                        }
                        OrdinaryCompletionKind::BusinessCheck => {
                            result.business_checked_completed += 1
                        }
                    },
                    Ok(()) => result.gap(
                        "ordinary_confirmation_unresolved",
                        key,
                        "Current structural gaps prevent counting this confirmation.",
                        vec![work.meta.source_ref.clone()],
                        "work",
                    ),
                    Err(error) => result.gap(
                        "ordinary_confirmation_unresolved",
                        key,
                        &error.report().message,
                        vec![work.meta.source_ref.clone()],
                        "work",
                    ),
                }
                result.historical_gap_total += result.gap_total - previous_gaps;
                continue;
            }
            if structured && source_sha.is_some() {
                match verify_completed(
                    store,
                    project.id,
                    work,
                    branch,
                    source_sha.unwrap(),
                    &mut verification_budget,
                ) {
                    Ok(()) => result.verified_completed += 1,
                    Err(error) => {
                        result.completion_verification_failed += 1;
                        result.gap(
                            "completion_verification_failed",
                            key,
                            &error.report().message,
                            vec![work.meta.source_ref.clone()],
                            "evidence",
                        );
                    }
                }
            } else if source_sha.is_none() {
                result.completion_not_checked += 1;
                result.gap("completion_not_checked", key, "Not checked in this query: no explicit source SHA was supplied. This is neither a verification failure nor proof of completion. Inspect the original report and its recorded source version; never fill historical evidence with current HEAD.", vec![work.meta.source_ref.clone()], "evidence");
            } else {
                result.completion_check_blocked += 1;
                result.gap("completion_check_blocked", key, "Verification was requested but structural gaps prevent checking this completion. Resolve the cited gaps and recheck the explicit source version.", vec![work.meta.source_ref.clone()], "evidence");
            }
            result.historical_gap_total += result.gap_total - previous_gaps;
            continue;
        }
        unfinished += 1;
        if structured {
            structured_unfinished += 1;
            result.structured_keys.insert(key.clone());
        }
        let mut blocked = false;
        if let Some(ready) = readiness.get(key.as_str()) {
            for diagnostic in &ready.diagnostics {
                if matches!(
                    diagnostic.code.as_str(),
                    "active_claim" | "status_not_selectable"
                ) && matches!(
                    work.status,
                    WorkStatus::Planned
                        | WorkStatus::Ready
                        | WorkStatus::Claimed
                        | WorkStatus::InProgress
                ) {
                    continue;
                }
                blocked = true;
                result.gap(
                    &diagnostic.code,
                    key,
                    &diagnostic.detail,
                    vec![work.meta.source_ref.clone()],
                    "dependencies",
                );
            }
        } else {
            blocked = true;
        }
        if structured && !blocked {
            result.executable_keys.insert(key.clone());
            result.executable_work_total += 1;
            if result.executable_work.len() < 100 {
                result.executable_work.push(key.clone());
            }
        }
    }
    let concrete_work = works.iter().any(|w| {
        !w.item.archived
            && w.item.kind.as_deref() != Some("intake")
            && w.item.status != WorkStatus::Cancelled
    });
    if !works.is_empty()
        && !concrete_work
        && result.source_cancelled + result.source_archived != works.len()
    {
        result.gap("business_work_missing", "project", "Only intake work is present. Organizing a baseline does not establish the business delivery scope; add the actual delivery work before claiming project completion.", vec![], "work");
    }
    result.business_execution_ready = sources_ok && result.executable_work_total > 0;
    result.state = if !sources_ok {
        OrganizationState::SourceUnreadable
    } else if works.is_empty() {
        OrganizationState::NeedsOrganization
    } else if unfinished == 0 && (result.source_completed == 0 || required_cancelled) {
        OrganizationState::ClosedWithoutCompletion
    } else if unfinished == 0 && (unresolved_goal_scope || !concrete_work) {
        OrganizationState::NeedsOrganization
    } else if unfinished == 0 && result.verified_completed == result.source_completed {
        OrganizationState::Completed
    } else if unfinished == 0
        && result.verified_completed
            + result.user_confirmed_completed
            + result.business_checked_completed
            == result.source_completed
    {
        OrganizationState::CompletedUnderPolicy
    } else if unfinished == 0 {
        OrganizationState::AwaitingVerification
    } else if result.business_execution_ready {
        OrganizationState::Ready
    } else if structured_unfinished > 0 {
        OrganizationState::Blocked
    } else {
        OrganizationState::NeedsOrganization
    };
    if source_sha.is_some() {
        result.completion_basis = "Only verified_completed counts hash-verified passing reports covering all current task acceptance for the explicit source SHA. Source-declared completion remains separate. This does not certify release or real-client business acceptance.";
    }
    if !result.ordinary_work_policies.is_empty() {
        result.completion_basis = "User confirmations and business checks require matching current policy, acceptance and applied host receipts. They never count as verified_completed. Other work keeps the strict engineering policy; changing configuration cannot promote old source declarations.";
    }
    result.finish();
    let actual = store.project(project.id)?.project_revision;
    if actual != project.project_revision {
        return Err(Error::RevisionConflict {
            expected: project.project_revision,
            actual,
        });
    }
    ensure_public_data(&result)?;
    Ok(result)
}

fn verify_completed(
    store: &Store,
    project: Id,
    work: &WorkItem,
    branch: Option<Id>,
    sha: &str,
    budget: &mut usize,
) -> Result<()> {
    store.check_completion_dependencies(project, &work.meta.external_key, branch)?;
    let minimum = work
        .evidence_level
        .map(verification_rank)
        .unwrap_or(Some(2))
        .ok_or_else(|| Error::EvidenceMissing("unknown required evidence level".into()))?
        .max(2);
    let binding = store
        .current_completion_binding(project, &work.meta.external_key, branch)?
        .filter(|binding| binding.source_sha == sha);
    if let Some(binding) = &binding {
        store.check_completion_binding(project, work, branch, binding)?;
    }
    let evidence = store.evidence_for_work(project, &work.meta.external_key, Some(sha), branch)?;
    let mut covered = BTreeSet::new();
    let mut reports = BTreeMap::new();
    for assessment in evidence {
        let evidence = &assessment.evidence.item;
        if binding.as_ref().is_some_and(|binding| {
            !binding
                .evidence
                .iter()
                .any(|selected| selected.id == evidence.id)
        }) {
            // Rejected attempts remain queryable evidence; they are not retroactively
            // added to the successful completion's explicit proof requirements.
            continue;
        }
        if assessment.currency != EvidenceCurrency::Current
            || !assessment.missing_bindings.is_empty()
            || verification_rank(evidence.level).is_none_or(|r| r < minimum)
        {
            continue;
        }
        if *budget == 0 {
            return Err(Error::EvidenceMissing(
                "report verification budget exhausted; verify a smaller work scope separately"
                    .into(),
            ));
        }
        let bytes = read_registered_file(
            store,
            project,
            &evidence.locator,
            None,
            evidence.sha256.as_deref(),
            (*budget).min(1024 * 1024) as u64,
        )?;
        *budget = budget.saturating_sub(bytes.len());
        let report: CompletionReport = serde_json::from_slice(&bytes)
            .map_err(|_| Error::EvidenceMissing("invalid completion report".into()))?;
        report.validate(
            evidence,
            &work.meta.external_key,
            &work.acceptance,
            now_millis()?,
        )?;
        for criterion in &work.acceptance {
            if report.covers(criterion) {
                covered.insert(criterion);
            }
        }
        reports.insert(evidence.id, report);
    }
    if let Some(binding) = &binding {
        // Recheck every selected record, including extra required evidence. A new
        // passing report cannot replace a damaged or missing selected report.
        for evidence in &binding.evidence {
            if !reports.contains_key(&evidence.id) {
                return Err(Error::EvidenceMissing(format!(
                    "selected evidence {} no longer meets the current verification requirements",
                    evidence.external_key
                )));
            }
        }
        for mapping in &binding.acceptance {
            for id in &mapping.evidence {
                if !reports
                    .get(id)
                    .is_some_and(|r| r.covers(&mapping.criterion))
                {
                    return Err(Error::EvidenceMissing(format!(
                        "selected evidence {id} does not verify acceptance: {}",
                        mapping.criterion
                    )));
                }
            }
        }
    }
    if work.acceptance.iter().any(|c| !covered.contains(c)) {
        return Err(Error::EvidenceMissing(
            "current verified reports do not cover every acceptance criterion".into(),
        ));
    }
    Ok(())
}
