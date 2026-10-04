//! Task planning suggestions and controlled draft changes (AWR-TMCP-021).
//!
//! Pure rules only: no PG, MCP, or filesystem IO. Suggestions are advisory and
//! never claimable or executable. Draft candidates carry exact digests; approve
//! and publish are separate actions bound to the current digest. Ordinary
//! planning may self-approve under an explicit project policy, but that policy
//! must not downgrade independent delivery-review requirements.

use crate::canonical::contract_hash;
use crate::error::{TeamError, TeamResult};
use crate::permission::{Action, AuthorityScope, ResourceRef, authorize_action};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const PLANNING_CODEC: &str = "awr-team-planning-v1";
pub const PLANNING_CODEC_V2: &str = "awr-team-planning-v2";

/// Suggestions never become claimable work and never enlarge the formal work
/// denominator or mutate live deps/acceptance.
pub const SUGGESTION_CLAIMABLE: bool = false;
pub const SUGGESTION_ADDS_FORMAL_WORK: bool = false;
pub const SUGGESTION_MUTATES_LIVE_DEPS: bool = false;
pub const SUGGESTION_MUTATES_LIVE_ACCEPTANCE: bool = false;
pub const SUGGESTION_API_WRITABLE_BY_READER: bool = false;

/// Hard-delete of planning history and forging completion via draft status are
/// forbidden. Archive/cancel are the only retirement paths.
pub const HARD_DELETE_HISTORY_ALLOWED: bool = false;
pub const FORGE_COMPLETION_VIA_STATUS_ALLOWED: bool = false;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionState {
    Open,
    AcceptedIntoDraft,
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningSuggestion {
    pub codec: String,
    pub suggestion_id: String,
    pub project_id: String,
    pub author_person_id: String,
    pub author_actor_id: String,
    pub rationale: String,
    pub version: u32,
    /// Source/baseline digest the author observed. Expired baselines refuse.
    pub baseline_digest: String,
    pub baseline_epoch: String,
    pub affected_work_keys: Vec<String>,
    /// Informational only — never applied by suggestion APIs.
    pub proposed_notes: Value,
    pub state: SuggestionState,
}

impl PlanningSuggestion {
    pub fn validate(&self) -> TeamResult<()> {
        if self.codec != PLANNING_CODEC {
            return Err(TeamError::InvalidInput("unsupported planning codec".into()));
        }
        if self.suggestion_id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || self.author_person_id.trim().is_empty()
            || self.author_actor_id.trim().is_empty()
        {
            return Err(TeamError::InvalidInput(
                "suggestion identity/author fields required".into(),
            ));
        }
        if self.rationale.trim().is_empty() || self.rationale.len() > 8_192 {
            return Err(TeamError::InvalidInput(
                "suggestion rationale required (1..8192 chars)".into(),
            ));
        }
        if self.version == 0 {
            return Err(TeamError::InvalidInput(
                "suggestion version must be >= 1".into(),
            ));
        }
        if self.baseline_digest.trim().is_empty() || self.baseline_epoch.trim().is_empty() {
            return Err(TeamError::InvalidInput(
                "suggestion baseline digest/epoch required".into(),
            ));
        }
        if self.affected_work_keys.len() > 256 {
            return Err(TeamError::InvalidInput(
                "too many affected work keys".into(),
            ));
        }
        Ok(())
    }

    pub fn claimable(&self) -> bool {
        SUGGESTION_CLAIMABLE
    }

    pub fn adds_formal_work(&self) -> bool {
        SUGGESTION_ADDS_FORMAL_WORK
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftDefinitionState {
    Draft,
    Enabled,
    Archived,
    Cancelled,
}

impl DraftDefinitionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Enabled => "enabled",
            Self::Archived => "archived",
            Self::Cancelled => "cancelled",
        }
    }

    /// Source/status fields must never imply PG completion.
    pub fn forges_completion(self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftOpKind {
    CreateTask,
    EditFields,
    Split,
    Cancel,
    Archive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDraft {
    /// Preserved original task identity. Never rewritten by draft edits.
    pub work_id: String,
    pub external_key: String,
    pub title: String,
    pub goals: Vec<String>,
    pub scope_paths: Vec<String>,
    pub acceptance: Vec<String>,
    pub required_dependencies: Vec<String>,
    pub completion_policy: String,
    /// V2 only. Omission retains the source policy; a present map is an explicit
    /// reviewed replacement, never an implicit acceptance or permission grant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::contract::present_modes"
    )]
    pub dependency_acceptance: Option<BTreeMap<String, crate::DependencyAcceptanceMode>>,
    pub definition_state: DraftDefinitionState,
    /// Owning workstream external key. Required for CreateTask writeback so
    /// publish prep can bind the new task before authoritative source mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workstream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_from: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub split_children: Vec<String>,
}

impl TaskDraft {
    pub fn validate(&self) -> TeamResult<()> {
        if self.work_id.trim().is_empty() || self.external_key.trim().is_empty() {
            return Err(TeamError::InvalidInput(
                "draft work_id and external_key required".into(),
            ));
        }
        if self.title.trim().is_empty() || self.title.len() > 512 {
            return Err(TeamError::InvalidInput(
                "draft title required (1..512 chars)".into(),
            ));
        }
        if self.acceptance.is_empty() {
            return Err(TeamError::InvalidInput("draft acceptance required".into()));
        }
        if self.completion_policy.trim().is_empty() {
            return Err(TeamError::InvalidInput(
                "draft completion_policy required".into(),
            ));
        }
        if let Some(modes) = &self.dependency_acceptance {
            if modes.is_empty()
                || self
                    .required_dependencies
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != self.required_dependencies.len()
                || modes
                    .keys()
                    .any(|key| key == &self.work_id || !self.required_dependencies.contains(key))
            {
                return Err(TeamError::InvalidInput(
                    "dependency_acceptance requires a nonempty map and unique existing required predecessors".into(),
                ));
            }
        }
        if FORGE_COMPLETION_VIA_STATUS_ALLOWED {
            return Err(TeamError::InvalidInput(
                "forge completion via status must remain forbidden".into(),
            ));
        }
        if self.definition_state.forges_completion() {
            return Err(TeamError::InvalidInput(
                "draft definition_state cannot forge completion".into(),
            ));
        }
        // Refuse inventing a "done"/"completed" completion policy synonym.
        let policy = self.completion_policy.to_ascii_lowercase();
        if matches!(
            policy.as_str(),
            "done" | "completed" | "complete" | "finished" | "source_done"
        ) {
            return Err(TeamError::InvalidInput(
                "completion_policy cannot forge completion via status synonyms".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftChange {
    pub op: DraftOpKind,
    pub before: Option<TaskDraft>,
    pub after: TaskDraft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Drafting,
    Approved,
    Published,
    Superseded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrdinaryPlanningSelfApprovePolicy {
    /// When true, a planner who also holds `planning.approve` may approve their
    /// own ordinary planning candidate. Independent delivery review is never
    /// softened by this flag.
    pub allow_self_approve_ordinary: bool,
    /// Project's required delivery completion policy. Must stay at least as
    /// strong as independent_review when that was already required.
    pub delivery_completion_policy: String,
}

impl OrdinaryPlanningSelfApprovePolicy {
    pub fn ordinary_default() -> Self {
        Self {
            allow_self_approve_ordinary: true,
            delivery_completion_policy: "independent_review".into(),
        }
    }

    pub fn validate_no_delivery_downgrade(&self, prior_policy: &str) -> TeamResult<()> {
        ensure_independent_review_not_downgraded(prior_policy, &self.delivery_completion_policy)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningApproval {
    pub approval_id: String,
    pub candidate_digest: String,
    pub approver_person_id: String,
    pub approver_actor_id: String,
    pub self_approved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningCandidate {
    pub codec: String,
    pub candidate_id: String,
    pub project_id: String,
    pub author_person_id: String,
    pub author_actor_id: String,
    pub baseline_digest: String,
    pub baseline_epoch: String,
    /// Increments on every edit; old approvals cannot bind a new revision.
    pub draft_revision: u32,
    pub changes: Vec<DraftChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestion_ids: Vec<String>,
    pub state: CandidateState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<PlanningApproval>,
    /// Allowed relative spec roots for scope_paths (project-bounded).
    pub allowed_spec_roots: Vec<String>,
    /// Project goal keys permitted for drafts (no cross-project goals).
    pub project_goal_keys: Vec<String>,
}

impl PlanningCandidate {
    pub fn validate_structure(&self) -> TeamResult<()> {
        if self.codec != planning_codec_for_changes(&self.changes) {
            return Err(TeamError::InvalidInput("unsupported planning codec".into()));
        }
        if self.candidate_id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || self.author_person_id.trim().is_empty()
            || self.author_actor_id.trim().is_empty()
        {
            return Err(TeamError::InvalidInput(
                "candidate identity/author fields required".into(),
            ));
        }
        if self.draft_revision == 0 {
            return Err(TeamError::InvalidInput(
                "draft_revision must be >= 1".into(),
            ));
        }
        if self.baseline_digest.trim().is_empty() || self.baseline_epoch.trim().is_empty() {
            return Err(TeamError::InvalidInput(
                "candidate baseline digest/epoch required".into(),
            ));
        }
        if self.changes.is_empty() || self.changes.len() > 1_024 {
            return Err(TeamError::InvalidInput(
                "candidate requires 1..1024 draft changes".into(),
            ));
        }
        for change in &self.changes {
            if let Some(before) = &change.before {
                before.validate()?;
                ensure_independent_review_not_downgraded(
                    &before.completion_policy,
                    &change.after.completion_policy,
                )?;
                if before.work_id != change.after.work_id {
                    return Err(TeamError::InvalidInput(
                        "draft edits must preserve original work_id".into(),
                    ));
                }
            }
            change.after.validate()?;
            match change.op {
                DraftOpKind::CreateTask => {
                    if change.before.is_some() {
                        return Err(TeamError::InvalidInput(
                            "create_task must not carry before state".into(),
                        ));
                    }
                }
                DraftOpKind::EditFields | DraftOpKind::Cancel | DraftOpKind::Archive => {
                    if change.before.is_none() {
                        return Err(TeamError::InvalidInput(format!(
                            "{:?} requires before state",
                            change.op
                        )));
                    }
                }
                DraftOpKind::Split => {
                    if change.after.split_children.is_empty() {
                        return Err(TeamError::InvalidInput(
                            "split requires child work ids".into(),
                        ));
                    }
                }
            }
        }
        if HARD_DELETE_HISTORY_ALLOWED {
            return Err(TeamError::InvalidInput(
                "hard-delete of planning history must remain forbidden".into(),
            ));
        }
        Ok(())
    }

    pub fn digest_material(&self) -> TeamResult<Value> {
        self.validate_structure()?;
        Ok(json!({
            "codec": self.codec,
            "candidate_id": self.candidate_id,
            "project_id": self.project_id,
            "baseline_digest": self.baseline_digest,
            "baseline_epoch": self.baseline_epoch,
            "draft_revision": self.draft_revision,
            "suggestion_ids": self.suggestion_ids,
            "changes": self.changes,
            "allowed_spec_roots": self.allowed_spec_roots,
            "project_goal_keys": self.project_goal_keys,
        }))
    }

    pub fn candidate_digest(&self) -> TeamResult<String> {
        contract_hash(&self.digest_material()?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FieldDiff {
    pub work_key: String,
    pub field: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffectedTaskImpact {
    pub work_id: String,
    pub external_key: String,
    pub work_runtime_state: String,
    pub execution_state: String,
    pub has_active_claim: bool,
    pub review_requirement: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateDiff {
    pub candidate_digest: String,
    pub draft_revision: u32,
    pub field_diffs: Vec<FieldDiff>,
    pub affected_tasks: Vec<AffectedTaskImpact>,
    pub review_requirements: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineView<'a> {
    pub digest: &'a str,
    pub epoch: &'a str,
    pub current: bool,
    pub known_work_ids: BTreeSet<&'a str>,
    pub known_external_keys: BTreeSet<&'a str>,
}

/// Refuse cycles, dangling deps, cross-project goals, out-of-bound spec paths,
/// and expired baselines for a planning candidate.
pub fn validate_candidate(
    candidate: &PlanningCandidate,
    baseline: &BaselineView<'_>,
) -> TeamResult<()> {
    candidate.validate_structure()?;
    if !baseline.current
        || baseline.digest != candidate.baseline_digest
        || baseline.epoch != candidate.baseline_epoch
    {
        return Err(TeamError::InvalidInput(
            "expired or mismatched planning baseline".into(),
        ));
    }

    let mut nodes: BTreeSet<String> = baseline
        .known_work_ids
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let mut edges: Vec<(String, String)> = Vec::new();

    for change in &candidate.changes {
        let draft = &change.after;
        // Preserve identity: edits must not invent a new id for an existing key
        // that already maps elsewhere.
        if matches!(change.op, DraftOpKind::CreateTask) {
            if baseline.known_work_ids.contains(draft.work_id.as_str())
                || baseline
                    .known_external_keys
                    .contains(draft.external_key.as_str())
            {
                return Err(TeamError::InvalidInput(
                    "create_task cannot reuse existing work identity".into(),
                ));
            }
        } else if let Some(before) = &change.before {
            if before.work_id != draft.work_id {
                return Err(TeamError::InvalidInput(
                    "draft must preserve original task id".into(),
                ));
            }
        }
        nodes.insert(draft.work_id.clone());
        for child in &draft.split_children {
            nodes.insert(child.clone());
        }

        for goal in &draft.goals {
            if !candidate.project_goal_keys.iter().any(|g| g == goal) {
                return Err(TeamError::InvalidInput(format!(
                    "cross-project or unknown goal refused: {goal}"
                )));
            }
        }

        for path in &draft.scope_paths {
            validate_spec_path_in_bounds(path, &candidate.allowed_spec_roots)?;
        }

        for dep in &draft.required_dependencies {
            edges.push((draft.work_id.clone(), dep.clone()));
        }
    }

    // Dangling deps: every required dependency must resolve in baseline∪drafts.
    for (from, to) in &edges {
        if !nodes.contains(to) {
            return Err(TeamError::InvalidInput(format!(
                "dangling dependency: {from} -> {to}"
            )));
        }
        if from == to {
            return Err(TeamError::InvalidInput(format!(
                "dependency cycle (self): {from}"
            )));
        }
    }

    detect_cycle(&nodes.iter().cloned().collect::<Vec<_>>(), &edges)?;
    Ok(())
}

fn validate_spec_path_in_bounds(path: &str, roots: &[String]) -> TeamResult<()> {
    if path.trim().is_empty() {
        return Err(TeamError::InvalidInput("empty scope path".into()));
    }
    if path.contains('\0') || path.contains('\\') || path.starts_with('/') || path.starts_with("./")
    {
        return Err(TeamError::InvalidInput(format!(
            "out-of-bound spec path: {path}"
        )));
    }
    if path
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(TeamError::InvalidInput(format!(
            "out-of-bound spec path: {path}"
        )));
    }
    if roots.is_empty() {
        // No declared roots ⇒ only refuse absolute/traversal forms above.
        return Ok(());
    }
    let ok = roots
        .iter()
        .any(|root| path == root.as_str() || path.starts_with(&format!("{root}/")) || root == ".");
    if !ok {
        return Err(TeamError::InvalidInput(format!(
            "out-of-bound spec path: {path}"
        )));
    }
    Ok(())
}

fn detect_cycle(nodes: &[String], edges: &[(String, String)]) -> TeamResult<()> {
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for n in nodes {
        adj.entry(n.as_str()).or_default();
    }
    for (from, to) in edges {
        adj.entry(from.as_str()).or_default().push(to.as_str());
        adj.entry(to.as_str()).or_default();
    }
    let mut indeg: BTreeMap<&str, usize> = adj.keys().map(|k| (*k, 0usize)).collect();
    for outs in adj.values() {
        for t in outs {
            *indeg.entry(*t).or_default() += 1;
        }
    }
    let mut q: VecDeque<&str> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut seen = 0usize;
    while let Some(n) = q.pop_front() {
        seen += 1;
        for t in adj.get(n).into_iter().flatten() {
            let d = indeg.get_mut(t).unwrap();
            *d -= 1;
            if *d == 0 {
                q.push_back(t);
            }
        }
    }
    if seen != adj.len() {
        return Err(TeamError::InvalidInput(
            "dependency cycle in planning candidate".into(),
        ));
    }
    Ok(())
}

/// Build exact field diffs and surface review requirements for a candidate.
pub fn build_candidate_diff(
    candidate: &PlanningCandidate,
    impacts: Vec<AffectedTaskImpact>,
) -> TeamResult<CandidateDiff> {
    candidate.validate_structure()?;
    let mut field_diffs = Vec::new();
    let mut review_requirements = BTreeSet::new();
    for change in &candidate.changes {
        let key = change.after.external_key.clone();
        let pairs: &[(&str, Option<Value>, Option<Value>)] = &[
            (
                "title",
                change.before.as_ref().map(|b| json!(b.title)),
                Some(json!(change.after.title)),
            ),
            (
                "goals",
                change.before.as_ref().map(|b| json!(b.goals)),
                Some(json!(change.after.goals)),
            ),
            (
                "scope_paths",
                change.before.as_ref().map(|b| json!(b.scope_paths)),
                Some(json!(change.after.scope_paths)),
            ),
            (
                "acceptance",
                change.before.as_ref().map(|b| json!(b.acceptance)),
                Some(json!(change.after.acceptance)),
            ),
            (
                "required_dependencies",
                change
                    .before
                    .as_ref()
                    .map(|b| json!(b.required_dependencies)),
                Some(json!(change.after.required_dependencies)),
            ),
            (
                "definition_state",
                change
                    .before
                    .as_ref()
                    .map(|b| json!(b.definition_state.as_str())),
                Some(json!(change.after.definition_state.as_str())),
            ),
            (
                "completion_policy",
                change.before.as_ref().map(|b| json!(b.completion_policy)),
                Some(json!(change.after.completion_policy)),
            ),
        ];
        for (field, before, after) in pairs {
            if before != after {
                field_diffs.push(FieldDiff {
                    work_key: key.clone(),
                    field: (*field).into(),
                    before: before.clone(),
                    after: after.clone(),
                });
            }
        }
        let before_modes = change
            .before
            .as_ref()
            .and_then(|b| b.dependency_acceptance.as_ref());
        let after_modes = change.after.dependency_acceptance.as_ref();
        // None means retain, rather than remove, an existing policy.
        if after_modes.is_some() && before_modes != after_modes {
            field_diffs.push(FieldDiff {
                work_key: key,
                field: "dependency_acceptance".into(),
                before: before_modes.map(|m| json!(m)),
                after: after_modes.map(|m| json!(m)),
            });
            review_requirements.insert("explicit_dependency_assurance_review".into());
        }
        review_requirements.insert(change.after.completion_policy.clone());
        if change.after.definition_state == DraftDefinitionState::Cancelled
            || change.after.definition_state == DraftDefinitionState::Archived
        {
            review_requirements.insert("impact_review_for_retirement".into());
        }
    }
    for impact in &impacts {
        review_requirements.insert(impact.review_requirement.clone());
        if impact.has_active_claim || impact.execution_state != "none" {
            review_requirements.insert("in_flight_work_impact_review".into());
        }
    }
    Ok(CandidateDiff {
        candidate_digest: candidate.candidate_digest()?,
        draft_revision: candidate.draft_revision,
        field_diffs,
        affected_tasks: impacts,
        review_requirements: review_requirements.into_iter().collect(),
    })
}

/// Independent delivery-review policy must never be weakened by planning
/// self-approve policy.
pub fn ensure_independent_review_not_downgraded(
    prior_policy: &str,
    next_policy: &str,
) -> TeamResult<()> {
    let prior = prior_policy.trim().to_ascii_lowercase();
    let next = next_policy.trim().to_ascii_lowercase();
    if matches!(
        prior.as_str(),
        "independent_review" | "independent-review" | "trusted_execution_and_review"
    ) && !matches!(
        next.as_str(),
        "independent_review" | "independent-review" | "trusted_execution_and_review"
    ) {
        return Err(TeamError::PermissionDenied(
            "independent delivery-review policy must not be downgraded by planning self-approve"
                .into(),
        ));
    }
    Ok(())
}

/// Authorize a planning action at a domain entry already gated by TMCP-011.
pub fn authorize_planning_action(
    scope: &AuthorityScope,
    action: Action,
    resource: &ResourceRef,
    now_unix_ms: u64,
) -> TeamResult<()> {
    match action {
        Action::PlanningPropose
        | Action::PlanningEditDraft
        | Action::PlanningApprove
        | Action::PlanningPublish => authorize_action(scope, action, resource, now_unix_ms),
        _ => Err(TeamError::PermissionDenied(format!(
            "not a planning action: {}",
            action.as_str()
        ))),
    }
}

/// A request may repeat the authenticated actor's id, but it cannot name a
/// different person. Planning records use the actor from the credential.
pub fn attested_actor_person(
    authenticated_actor: &str,
    asserted: Option<&str>,
) -> TeamResult<String> {
    let actor = authenticated_actor.trim();
    if actor.is_empty() {
        return Err(TeamError::InvalidInput(
            "authenticated actor is empty".into(),
        ));
    }
    if let Some(asserted) = asserted.map(str::trim).filter(|value| !value.is_empty()) {
        if asserted != actor {
            return Err(TeamError::PermissionDenied(
                "asserted person id does not match the authenticated actor".into(),
            ));
        }
    }
    Ok(actor.to_owned())
}

/// Readers cannot write via suggestion APIs.
pub fn refuse_reader_suggestion_write(scope: &AuthorityScope) -> TeamResult<()> {
    if !scope.allowed_actions.contains(&Action::PlanningPropose) {
        return Err(TeamError::PermissionDenied(
            "readers cannot write via suggestion APIs".into(),
        ));
    }
    if SUGGESTION_API_WRITABLE_BY_READER {
        return Err(TeamError::PermissionDenied(
            "suggestion API must remain non-writable for readers".into(),
        ));
    }
    Ok(())
}

/// Approve is bound to the *current* candidate digest. Edited drafts cannot
/// reuse an old approval. Self-approve is allowed only under explicit policy
/// and never downgrades independent delivery review.
pub fn authorize_planning_approve(
    scope: &AuthorityScope,
    resource: &ResourceRef,
    now_unix_ms: u64,
    candidate: &PlanningCandidate,
    presented_digest: &str,
    policy: &OrdinaryPlanningSelfApprovePolicy,
    prior_delivery_policy: &str,
) -> TeamResult<bool> {
    authorize_planning_action(scope, Action::PlanningApprove, resource, now_unix_ms)?;
    if candidate.state != CandidateState::Drafting {
        return Err(TeamError::InvalidInput(
            "approve requires a drafting candidate".into(),
        ));
    }
    policy.validate_no_delivery_downgrade(prior_delivery_policy)?;
    let current = candidate.candidate_digest()?;
    if current != presented_digest {
        return Err(TeamError::InvalidInput(
            "approval digest does not match current candidate".into(),
        ));
    }
    if let Some(existing) = &candidate.approval {
        if existing.candidate_digest != current {
            return Err(TeamError::InvalidInput(
                "edited drafts cannot reuse old approvals".into(),
            ));
        }
    }
    let self_approve = scope.person_id == candidate.author_person_id
        || scope.client_id == candidate.author_actor_id
        || scope.person_id == candidate.author_actor_id;
    if self_approve && !policy.allow_self_approve_ordinary {
        return Err(TeamError::PermissionDenied(
            "self-approve of ordinary planning disabled by project policy".into(),
        ));
    }
    Ok(self_approve)
}

/// Publish is a separate permissioned action, also digest-bound.
pub fn authorize_planning_publish(
    scope: &AuthorityScope,
    resource: &ResourceRef,
    now_unix_ms: u64,
    candidate: &PlanningCandidate,
    presented_digest: &str,
) -> TeamResult<()> {
    authorize_planning_action(scope, Action::PlanningPublish, resource, now_unix_ms)?;
    if candidate.state != CandidateState::Approved {
        return Err(TeamError::InvalidInput(
            "publish requires an approved planning candidate".into(),
        ));
    }
    let current = candidate.candidate_digest()?;
    if current != presented_digest {
        return Err(TeamError::InvalidInput(
            "publish digest does not match current candidate".into(),
        ));
    }
    let approval = candidate.approval.as_ref().ok_or_else(|| {
        TeamError::InvalidInput("publish requires approval bound to current digest".into())
    })?;
    if approval.candidate_digest != current {
        return Err(TeamError::InvalidInput(
            "edited drafts cannot reuse old approvals for publish".into(),
        ));
    }
    Ok(())
}

/// Apply an edit that bumps revision and clears any prior approval binding.
pub fn edit_candidate(
    mut candidate: PlanningCandidate,
    changes: Vec<DraftChange>,
) -> TeamResult<PlanningCandidate> {
    if matches!(
        candidate.state,
        CandidateState::Published | CandidateState::Superseded
    ) {
        return Err(TeamError::InvalidInput(
            "cannot edit published or superseded candidate".into(),
        ));
    }
    candidate.codec = planning_codec_for_changes(&changes).into();
    candidate.changes = changes;
    candidate.draft_revision = candidate.draft_revision.saturating_add(1).max(1);
    candidate.approval = None;
    candidate.state = CandidateState::Drafting;
    candidate.validate_structure()?;
    Ok(candidate)
}

/// Deterministic version selection lets stored V1 candidates keep their exact
/// original digest without a schema migration or a reinterpretation of policy.
pub fn planning_codec_for_changes(changes: &[DraftChange]) -> &'static str {
    if changes.iter().any(|c| {
        c.after.dependency_acceptance.is_some()
            || c.before
                .as_ref()
                .is_some_and(|b| b.dependency_acceptance.is_some())
    }) {
        PLANNING_CODEC_V2
    } else {
        PLANNING_CODEC
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{AuthorityScope, RoleTemplate, authority_from_template};

    fn draft(id: &str, deps: &[&str]) -> TaskDraft {
        TaskDraft {
            work_id: id.into(),
            external_key: id.into(),
            title: format!("Task {id}"),
            goals: vec!["delivery".into()],
            scope_paths: vec!["specs/api.md".into()],
            acceptance: vec!["tests pass".into()],
            required_dependencies: deps.iter().map(|s| (*s).into()).collect(),
            completion_policy: "independent_review".into(),
            dependency_acceptance: None,
            definition_state: DraftDefinitionState::Draft,
            workstream: None,
            split_from: None,
            split_children: vec![],
        }
    }

    fn candidate(changes: Vec<DraftChange>) -> PlanningCandidate {
        PlanningCandidate {
            codec: PLANNING_CODEC.into(),
            candidate_id: "cand-1".into(),
            project_id: "project-a".into(),
            author_person_id: "person-maint".into(),
            author_actor_id: "actor-maint".into(),
            baseline_digest: "sha256:baseline".into(),
            baseline_epoch: "1".into(),
            draft_revision: 1,
            changes,
            suggestion_ids: vec![],
            state: CandidateState::Drafting,
            approval: None,
            allowed_spec_roots: vec!["specs".into()],
            project_goal_keys: vec!["delivery".into()],
        }
    }

    fn baseline<'a>(ids: &'a [&'a str]) -> BaselineView<'a> {
        BaselineView {
            digest: "sha256:baseline",
            epoch: "1",
            current: true,
            known_work_ids: ids.iter().copied().collect(),
            known_external_keys: ids.iter().copied().collect(),
        }
    }

    fn dependency_modes() -> BTreeMap<String, crate::DependencyAcceptanceMode> {
        BTreeMap::from([(
            "B".into(),
            crate::DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
        )])
    }

    #[test]
    fn legacy_candidate_wire_and_digest_material_remain_unchanged() {
        let legacy = json!({
            "codec": "awr-team-planning-v1",
            "candidate_id": "cand-1",
            "project_id": "project-a",
            "author_person_id": "person-maint",
            "author_actor_id": "actor-maint",
            "baseline_digest": "sha256:baseline",
            "baseline_epoch": "1",
            "draft_revision": 1,
            "changes": [{
                "op": "create_task",
                "before": null,
                "after": {
                    "work_id": "A", "external_key": "A", "title": "Task A",
                    "goals": ["delivery"], "scope_paths": ["specs/api.md"],
                    "acceptance": ["tests pass"], "required_dependencies": [],
                    "completion_policy": "independent_review", "definition_state": "draft"
                }
            }],
            "state": "drafting",
            "allowed_spec_roots": ["specs"],
            "project_goal_keys": ["delivery"]
        });
        let loaded: PlanningCandidate = serde_json::from_value(legacy.clone()).unwrap();
        loaded.validate_structure().unwrap();
        assert_eq!(serde_json::to_value(&loaded).unwrap(), legacy);
        let material = loaded.digest_material().unwrap();
        assert!(
            material["changes"][0]["after"]
                .get("dependency_acceptance")
                .is_none()
        );
        assert_eq!(
            material,
            json!({
                "codec": legacy["codec"],
                "candidate_id": legacy["candidate_id"],
                "project_id": legacy["project_id"],
                "baseline_digest": legacy["baseline_digest"],
                "baseline_epoch": legacy["baseline_epoch"],
                "draft_revision": legacy["draft_revision"],
                "suggestion_ids": [],
                "changes": legacy["changes"],
                "allowed_spec_roots": legacy["allowed_spec_roots"],
                "project_goal_keys": legacy["project_goal_keys"]
            })
        );
    }

    #[test]
    fn explicit_dependency_policy_requires_v2_preview_and_new_approval() {
        let changes = vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &["B"])),
            after: draft("A", &["B"]),
        }];
        let mut approved = candidate(changes.clone());
        let legacy_digest = approved.candidate_digest().unwrap();
        approved.state = CandidateState::Approved;
        approved.approval = Some(PlanningApproval {
            approval_id: "ap-legacy".into(),
            candidate_digest: legacy_digest.clone(),
            approver_person_id: "person-maint".into(),
            approver_actor_id: "actor-maint".into(),
            self_approved: true,
        });
        let mut changes = changes;
        changes[0].after.dependency_acceptance = Some(dependency_modes());
        let mut edited = edit_candidate(approved, changes).unwrap();
        assert_eq!(edited.codec, PLANNING_CODEC_V2);
        assert_eq!(edited.draft_revision, 2);
        assert!(edited.approval.is_none());
        assert_eq!(edited.state, CandidateState::Drafting);
        assert_ne!(edited.candidate_digest().unwrap(), legacy_digest);
        validate_candidate(&edited, &baseline(&["A", "B"])).unwrap();
        let diff = build_candidate_diff(&edited, vec![]).unwrap();
        assert!(diff.field_diffs.iter().any(|d| {
            d.field == "dependency_acceptance"
                && d.before.is_none()
                && d.after == Some(json!(dependency_modes()))
        }));
        assert!(
            diff.review_requirements
                .iter()
                .any(|r| r == "explicit_dependency_assurance_review")
        );
        edited.codec = PLANNING_CODEC.into();
        assert!(
            edited.validate_structure().is_err(),
            "V1 must reject explicit V2 policy fields"
        );
    }

    #[test]
    fn dependency_policy_rejects_null_duplicates_unknown_modes_and_invalid_predecessors() {
        let base = json!(draft("A", &["B"]));
        for raw in [
            Value::Null,
            json!({"B": "unknown_assurance"}),
            json!("invalid"),
        ] {
            let mut value = base.clone();
            value["dependency_acceptance"] = raw;
            assert!(serde_json::from_value::<TaskDraft>(value).is_err());
        }
        let duplicate = format!(
            "{},\"dependency_acceptance\":{{\"B\":\"agent_reviewed_caller_asserted_reconciled\",\"B\":\"agent_reviewed_caller_asserted_reconciled\"}}}}",
            serde_json::to_string(&base).unwrap().trim_end_matches('}')
        );
        assert!(
            serde_json::from_str::<TaskDraft>(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        for raw in [
            json!({}),
            json!({"MISSING": "agent_reviewed_caller_asserted_reconciled"}),
            json!({"A": "agent_reviewed_caller_asserted_reconciled"}),
        ] {
            let mut value = base.clone();
            value["dependency_acceptance"] = raw;
            assert!(
                serde_json::from_value::<TaskDraft>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut repeated = draft("A", &["B", "B"]);
        repeated.dependency_acceptance = Some(dependency_modes());
        assert!(repeated.validate().is_err());
        let mut dangling = draft("A", &["B"]);
        dangling.dependency_acceptance = Some(dependency_modes());
        let mut c = candidate(vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: dangling,
        }]);
        c.codec = PLANNING_CODEC_V2.into();
        assert!(
            validate_candidate(&c, &baseline(&[]))
                .unwrap_err()
                .to_string()
                .contains("dangling")
        );
    }

    #[test]
    fn candidate_cannot_downgrade_existing_independent_review() {
        let mut after = draft("A", &[]);
        after.completion_policy = "caller_managed_execution_and_agent_review".into();
        let c = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after,
        }]);
        assert!(
            c.validate_structure()
                .unwrap_err()
                .to_string()
                .contains("downgrade")
        );
    }

    #[test]
    fn suggestion_is_not_claimable_and_does_not_add_formal_work() {
        let s = PlanningSuggestion {
            codec: PLANNING_CODEC.into(),
            suggestion_id: "sug-1".into(),
            project_id: "project-a".into(),
            author_person_id: "dev".into(),
            author_actor_id: "dev-agent".into(),
            rationale: "missing dependency on shared types".into(),
            version: 1,
            baseline_digest: "sha256:b".into(),
            baseline_epoch: "3".into(),
            affected_work_keys: vec!["A".into()],
            proposed_notes: json!({"note":"add dep on SHARED-1"}),
            state: SuggestionState::Open,
        };
        s.validate().unwrap();
        assert!(!s.claimable());
        assert!(!s.adds_formal_work());
        assert!(!SUGGESTION_MUTATES_LIVE_DEPS);
        assert!(!SUGGESTION_MUTATES_LIVE_ACCEPTANCE);
        assert!(!SUGGESTION_API_WRITABLE_BY_READER);
    }

    #[test]
    fn reader_cannot_write_suggestions() {
        let reader = authority_from_template(RoleTemplate::Reader, "t", "p", "reader", "cli");
        assert!(refuse_reader_suggestion_write(&reader).is_err());
        let developer = authority_from_template(RoleTemplate::Developer, "t", "p", "dev", "cli");
        refuse_reader_suggestion_write(&developer).unwrap();
    }

    #[test]
    fn draft_preserves_work_id_and_forbids_status_forge() {
        let before = draft("A", &[]);
        let mut after = before.clone();
        after.title = "Renamed".into();
        after.definition_state = DraftDefinitionState::Archived;
        let c = candidate(vec![DraftChange {
            op: DraftOpKind::Archive,
            before: Some(before),
            after,
        }]);
        c.validate_structure().unwrap();
        assert!(!HARD_DELETE_HISTORY_ALLOWED);
        assert!(!FORGE_COMPLETION_VIA_STATUS_ALLOWED);

        let mut bad = draft("A", &[]);
        bad.completion_policy = "done".into();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn rejects_cycle_dangling_cross_project_oob_and_expired_baseline() {
        let base = baseline(&["A", "B"]);
        // Cycle A->B->A
        let cycle = candidate(vec![
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("A", &[])),
                after: draft("A", &["B"]),
            },
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("B", &[])),
                after: draft("B", &["A"]),
            },
        ]);
        assert!(
            validate_candidate(&cycle, &base)
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );

        // Dangling
        let dangling = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after: draft("A", &["MISSING"]),
        }]);
        assert!(
            validate_candidate(&dangling, &base)
                .unwrap_err()
                .to_string()
                .contains("dangling")
        );

        // Cross-project goal
        let mut cross = draft("A", &[]);
        cross.goals = vec!["other-project-goal".into()];
        let cross_c = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after: cross,
        }]);
        assert!(
            validate_candidate(&cross_c, &base)
                .unwrap_err()
                .to_string()
                .contains("goal")
        );

        // Out-of-bound path
        let mut oob = draft("A", &[]);
        oob.scope_paths = vec!["../secrets/key.pem".into()];
        let oob_c = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after: oob,
        }]);
        assert!(
            validate_candidate(&oob_c, &base)
                .unwrap_err()
                .to_string()
                .contains("out-of-bound")
        );

        // Expired baseline
        let mut expired = baseline(&["A"]);
        expired.current = false;
        let ok_edit = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after: {
                let mut d = draft("A", &[]);
                d.title = "x".into();
                d
            },
        }]);
        assert!(
            validate_candidate(&ok_edit, &expired)
                .unwrap_err()
                .to_string()
                .contains("baseline")
        );
    }

    #[test]
    fn approve_and_publish_bind_digest_and_edit_invalidates_approval() {
        let mut c = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after: {
                let mut d = draft("A", &[]);
                d.title = "Updated".into();
                d
            },
        }]);
        let digest = c.candidate_digest().unwrap();
        let maint = authority_from_template(
            RoleTemplate::Maintainer,
            "tenant-a",
            "project-a",
            "person-maint",
            "actor-maint",
        );
        let resource = ResourceRef {
            tenant_id: "tenant-a".into(),
            project_id: "project-a".into(),
            workstream_id: None,
            work_id: None,
        };
        let policy = OrdinaryPlanningSelfApprovePolicy::ordinary_default();
        let self_approved = authorize_planning_approve(
            &maint,
            &resource,
            1,
            &c,
            &digest,
            &policy,
            "independent_review",
        )
        .unwrap();
        assert!(self_approved);
        c.approval = Some(PlanningApproval {
            approval_id: "ap-1".into(),
            candidate_digest: digest.clone(),
            approver_person_id: "person-maint".into(),
            approver_actor_id: "actor-maint".into(),
            self_approved: true,
        });
        c.state = CandidateState::Approved;
        authorize_planning_publish(&maint, &resource, 1, &c, &digest).unwrap();

        // Edit clears approval and bumps revision.
        let edited = edit_candidate(
            c.clone(),
            vec![DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("A", &[])),
                after: {
                    let mut d = draft("A", &[]);
                    d.title = "Again".into();
                    d
                },
            }],
        )
        .unwrap();
        assert!(edited.approval.is_none());
        assert_eq!(edited.state, CandidateState::Drafting);
        assert_ne!(edited.candidate_digest().unwrap(), digest);
        assert!(
            authorize_planning_publish(&maint, &resource, 1, &edited, &digest).is_err(),
            "old digest must not publish after edit"
        );
    }

    #[test]
    fn self_approve_cannot_downgrade_independent_delivery_review() {
        let policy = OrdinaryPlanningSelfApprovePolicy {
            allow_self_approve_ordinary: true,
            delivery_completion_policy: "author_may_complete".into(),
        };
        assert!(
            policy
                .validate_no_delivery_downgrade("independent_review")
                .is_err()
        );
        ensure_independent_review_not_downgraded("independent_review", "independent_review")
            .unwrap();
    }

    #[test]
    fn candidate_diff_shows_exact_fields_and_impacts() {
        let c = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after: {
                let mut d = draft("A", &["B"]);
                d.title = "New title".into();
                d
            },
        }]);
        let base = baseline(&["A", "B"]);
        validate_candidate(&c, &base).unwrap();
        let diff = build_candidate_diff(
            &c,
            vec![AffectedTaskImpact {
                work_id: "A".into(),
                external_key: "A".into(),
                work_runtime_state: "ready".into(),
                execution_state: "running".into(),
                has_active_claim: true,
                review_requirement: "independent_review".into(),
            }],
        )
        .unwrap();
        assert!(
            diff.field_diffs
                .iter()
                .any(|d| d.field == "title" && d.after == Some(json!("New title")))
        );
        assert!(
            diff.field_diffs
                .iter()
                .any(|d| d.field == "required_dependencies")
        );
        assert_eq!(diff.affected_tasks.len(), 1);
        assert!(
            diff.review_requirements
                .iter()
                .any(|r| r == "in_flight_work_impact_review")
        );
    }

    #[test]
    fn developer_may_propose_but_not_edit_or_publish() {
        let developer = authority_from_template(RoleTemplate::Developer, "t", "p", "dev", "cli");
        let resource = ResourceRef {
            tenant_id: "t".into(),
            project_id: "p".into(),
            workstream_id: None,
            work_id: None,
        };
        authorize_planning_action(&developer, Action::PlanningPropose, &resource, 1).unwrap();
        assert!(
            authorize_planning_action(&developer, Action::PlanningEditDraft, &resource, 1).is_err()
        );
        assert!(
            authorize_planning_action(&developer, Action::PlanningPublish, &resource, 1).is_err()
        );
    }

    fn maintainer_scope() -> (AuthorityScope, ResourceRef) {
        let scope = authority_from_template(
            RoleTemplate::Maintainer,
            "tenant-a",
            "project-a",
            "person-maint",
            "actor-maint",
        );
        let resource = ResourceRef {
            tenant_id: "tenant-a".into(),
            project_id: "project-a".into(),
            workstream_id: None,
            work_id: None,
        };
        (scope, resource)
    }

    #[test]
    fn attested_person_rejects_substitution_that_would_clear_self_approve() {
        assert_eq!(
            attested_actor_person("person-maint", None).unwrap(),
            "person-maint"
        );
        assert_eq!(
            attested_actor_person("person-maint", Some(" person-maint ")).unwrap(),
            "person-maint"
        );
        let forged = attested_actor_person("person-maint", Some("other-person")).unwrap_err();
        assert!(forged.to_string().contains("does not match"), "{forged}");

        let c = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after: draft("A", &[]),
        }]);
        let digest = c.candidate_digest().unwrap();
        let (real, resource) = maintainer_scope();
        let mut banned = OrdinaryPlanningSelfApprovePolicy::ordinary_default();
        banned.allow_self_approve_ordinary = false;
        let denied = authorize_planning_approve(
            &real,
            &resource,
            1,
            &c,
            &digest,
            &banned,
            "independent_review",
        )
        .unwrap_err();
        assert!(denied.to_string().contains("self-approve"), "{denied}");
    }

    #[test]
    fn approve_and_publish_refuse_when_candidate_is_already_published() {
        let mut c = candidate(vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some(draft("A", &[])),
            after: draft("A", &[]),
        }]);
        let digest = c.candidate_digest().unwrap();
        c.state = CandidateState::Published;
        c.approval = Some(PlanningApproval {
            approval_id: "ap-1".into(),
            candidate_digest: digest.clone(),
            approver_person_id: "person-maint".into(),
            approver_actor_id: "actor-maint".into(),
            self_approved: true,
        });
        let (scope, resource) = maintainer_scope();
        let policy = OrdinaryPlanningSelfApprovePolicy::ordinary_default();
        assert!(
            authorize_planning_approve(
                &scope,
                &resource,
                1,
                &c,
                &digest,
                &policy,
                "independent_review",
            )
            .is_err()
        );
        assert!(authorize_planning_publish(&scope, &resource, 1, &c, &digest).is_err());
    }
}
