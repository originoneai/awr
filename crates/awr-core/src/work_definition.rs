//! A proportionate, source-owned definition of what one work item is for.
//!
//! The definition only records references, roles, attribution and short bounded text. Existing
//! Goals, success criteria, acceptance, specification references, decisions and dependencies stay
//! the authority for their own facts. Checking is deterministic and never judges whether a
//! contribution claim, granularity judgment or example is semantically adequate.
use crate::{
    Error, RequirementDisposition, Result, SpecificationReference, WorkStatus, ensure_public_data,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const WORK_DEFINITION_VERSION: u32 = 1;
const MAX_ENTRIES: usize = 64;
const MAX_TEXT: usize = 4096;
const MAX_NAME: usize = 512;
const MAX_BYTES: usize = 64 * 1024;

/// What the work is for. Distinct from the existing work `kind`, which keeps its meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPurpose {
    Implementation,
    Research,
    Document,
    Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryTier {
    Required,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentRole {
    UserDecision,
    SourcedFact,
    Assumption,
    Suggestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    IndependentResult,
    StepOfParent,
    SharedBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponsibilityKind {
    Design,
    Delivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExampleKind {
    Positive,
    Negative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscriminationMethod {
    FailBeforePassAfter,
    DiffMutation,
    JustifiedAlternative,
}

/// One piece of adopted or unadopted intent. User decisions and sourced facts must point at a
/// registered decision and pin the exact bytes adopted; assumptions and suggestions never do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentEntry {
    pub role: IntentRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adoption {
    pub basis: String,
    pub by: String,
}

/// An attributed claim that this work advances one exact success criterion of a goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub goal: String,
    pub outcome: String,
    pub claim: String,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GranularityJudgment {
    pub judgment: Granularity,
    pub reason: String,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Responsibility {
    pub name: String,
    pub kind: ResponsibilityKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Example {
    pub criterion: String,
    pub kind: ExampleKind,
    pub given: String,
    pub when: String,
    pub then: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discrimination {
    pub method: DiscriminationMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// How large the work is expected to be; the budgets default from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkSize {
    Small,
    Medium,
    Large,
}

pub const MAX_REWORK_BUDGET: u32 = 20;
pub const MAX_EFFORT_BUDGET_MINUTES: u64 = 100_000;

impl WorkSize {
    /// The rework cycles and the active-effort minutes a work of this size gets when it declares
    /// no budget of its own. Larger work declares its own rework budget: the figure given here is
    /// the standard one, which applies only until it does, and the definition reports a gap.
    pub fn defaults(self) -> (u32, u64) {
        match self {
            Self::Small => (2, 60),
            Self::Medium => (3, 240),
            Self::Large => (3, 960),
        }
    }
}

/// How many times the work may be returned and how long it may be worked on before someone has
/// to decide again. A limit left out falls back to the default of the size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rework_cycles: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_effort_minutes: Option<u64>,
}

/// The budgets in force for a work, with where each came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveBudget {
    pub size: WorkSize,
    /// `declared` or `default` (a work that declares no size is medium).
    pub size_basis: &'static str,
    pub rework_cycles: u32,
    /// `declared` or `size_default`.
    pub rework_basis: &'static str,
    pub active_effort_minutes: u64,
    pub effort_basis: &'static str,
}

/// What a work may spend before a decision is needed: its declared limits, else the defaults of
/// its size. Work without a definition is a medium work.
pub fn effective_budget(definition: Option<&WorkDefinition>) -> EffectiveBudget {
    let declared_size = definition.and_then(|d| d.size);
    let size = declared_size.unwrap_or(WorkSize::Medium);
    let (rework_default, effort_default) = size.defaults();
    let budget = definition.and_then(|d| d.budget.as_ref());
    let rework = budget.and_then(|b| b.rework_cycles);
    let effort = budget.and_then(|b| b.active_effort_minutes);
    EffectiveBudget {
        size,
        size_basis: if declared_size.is_some() {
            "declared"
        } else {
            "default"
        },
        rework_cycles: rework.unwrap_or(rework_default),
        rework_basis: if rework.is_some() {
            "declared"
        } else {
            "size_default"
        },
        active_effort_minutes: effort.unwrap_or(effort_default),
        effort_basis: if effort.is_some() {
            "declared"
        } else {
            "size_default"
        },
    }
}

/// A side line of work spawned from another: it must say when it rejoins and when it stops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SideLine {
    /// What has to be true for the work to hand its result back to the line it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_condition: Option<String>,
    /// When to stop: an effort or observation boundary, or a result that no longer matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_condition: Option<String>,
}

/// Distant work kept as an outline: the constraints already known and when to elaborate it.
/// An outline is not near-term executable, so it is never admitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outline {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkDefinition {
    pub version: u32,
    pub purpose: WorkPurpose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<DeliveryTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intent: Vec<IntentEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_gaps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adoption: Option<Adoption>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contributes: Vec<Contribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<GranularityJudgment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responsibility: Option<Responsibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliverable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_recovery: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discrimination: Option<Discrimination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_line: Option<SideLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<Outline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<WorkSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<WorkBudget>,
}

fn text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_TEXT
        && !value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}

fn name(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_NAME
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn fingerprint(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn opt_text(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(text)
}

impl WorkDefinition {
    /// Structural checks only: shape, bounds and attribution. Whether references resolve is the
    /// checker's job because it needs the rest of the project.
    pub fn validate(&self) -> Result<()> {
        let invalid = |why: &str| Error::InvalidInput(format!("work definition {why}"));
        if self.version != WORK_DEFINITION_VERSION {
            return Err(invalid("must declare version 1"));
        }
        if self.intent.len() > MAX_ENTRIES
            || self.open_gaps.len() > MAX_ENTRIES
            || self.contributes.len() > MAX_ENTRIES
            || self.examples.len() > MAX_ENTRIES
        {
            return Err(invalid("lists hold at most 64 entries"));
        }
        for entry in &self.intent {
            let adopted = matches!(
                entry.role,
                IntentRole::UserDecision | IntentRole::SourcedFact
            );
            let pinned = entry.decision.as_deref().is_some_and(name)
                && entry.source_fingerprint.as_deref().is_some_and(fingerprint);
            let unpinned = entry.decision.is_none() && entry.source_fingerprint.is_none();
            let well_formed = if adopted { pinned } else { pinned || unpinned };
            if !opt_text(&entry.note) || !well_formed {
                return Err(invalid(
                    "intent entries need a note within bounds and, for user decisions and sourced facts, a decision key with a sha256 source fingerprint",
                ));
            }
        }
        if self.open_gaps.iter().any(|gap| !text(gap))
            || self
                .adoption
                .as_ref()
                .is_some_and(|a| !text(&a.basis) || !name(&a.by))
        {
            return Err(invalid(
                "open gaps and the adoption basis need bounded text and an attributed author",
            ));
        }
        if self
            .contributes
            .iter()
            .any(|c| !name(&c.goal) || !text(&c.outcome) || !text(&c.claim) || !name(&c.by))
        {
            return Err(invalid(
                "contributions need a goal key, the exact outcome, a claim and an author",
            ));
        }
        if self
            .granularity
            .as_ref()
            .is_some_and(|g| !text(&g.reason) || !name(&g.by))
        {
            return Err(invalid(
                "a granularity judgment needs a reason and an author",
            ));
        }
        if self.responsibility.as_ref().is_some_and(|r| !name(&r.name))
            || [&self.parent, &self.inherits]
                .into_iter()
                .any(|key| key.as_deref().is_some_and(|k| !name(k)))
            || !opt_text(&self.deliverable)
            || !opt_text(&self.evidence_method)
            || !opt_text(&self.failure_recovery)
            || self
                .discrimination
                .as_ref()
                .is_some_and(|d| !opt_text(&d.note))
        {
            return Err(invalid(
                "names and descriptive text must be nonblank and bounded",
            ));
        }
        if self
            .examples
            .iter()
            .any(|e| !text(&e.criterion) || !text(&e.given) || !text(&e.when) || !text(&e.then))
        {
            return Err(invalid(
                "examples need the exact criterion plus given, when and then text",
            ));
        }
        if let Some(side_line) = &self.side_line
            && !(opt_text(&side_line.return_condition) && opt_text(&side_line.stop_condition))
        {
            return Err(invalid("side line conditions must be nonblank and bounded"));
        }
        if let Some(outline) = &self.outline
            && (outline.constraints.len() > MAX_ENTRIES
                || outline.constraints.iter().any(|c| !text(c))
                || !opt_text(&outline.trigger))
        {
            return Err(invalid(
                "an outline needs bounded constraints and a bounded elaboration trigger",
            ));
        }
        if let Some(budget) = &self.budget
            && (budget.rework_cycles.is_some_and(|n| n > MAX_REWORK_BUDGET)
                || budget
                    .active_effort_minutes
                    .is_some_and(|n| n == 0 || n > MAX_EFFORT_BUDGET_MINUTES))
        {
            return Err(invalid(
                "a budget allows at most 20 rework cycles and between 1 and 100000 minutes of active effort",
            ));
        }
        if serde_json::to_vec(self)?.len() > MAX_BYTES {
            return Err(invalid("must stay within 64 KiB"));
        }
        ensure_public_data(self)
    }
}

/// Another work as the checker sees it.
#[derive(Debug, Clone)]
pub struct DefinitionWork {
    pub workstream: Option<String>,
    pub status: WorkStatus,
    pub definition: Option<WorkDefinition>,
}

/// Everything the checker may read. The runtime fills it from current sources; the checker holds
/// no state and performs no I/O.
pub struct DefinitionFacts<'a> {
    pub work: &'a str,
    pub acceptance: &'a [String],
    pub paths: &'a [String],
    /// Keys of the works this one depends on: its declared prerequisites.
    pub dependencies: &'a [String],
    pub spec_refs: &'a [SpecificationReference],
    /// (specification key, item id) to its declared disposition.
    pub spec_dispositions: &'a BTreeMap<(String, String), RequirementDisposition>,
    /// Visible goal key to its success criteria.
    pub goals: &'a BTreeMap<String, Vec<String>>,
    /// Visible decision key to the sha256 fingerprint of its current source bytes.
    pub decisions: &'a BTreeMap<String, String>,
    /// Every work, including the checked one, by external key.
    pub works: &'a BTreeMap<String, DefinitionWork>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObligationStatus {
    pub obligation: &'static str,
    /// satisfied, missing, unknown, stale or not_applicable
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefinitionGap {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefinitionReport {
    pub work: String,
    /// not_declared, complete or gaps
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<WorkPurpose>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_tier: Option<DeliveryTier>,
    pub obligations: Vec<ObligationStatus>,
    pub gaps: Vec<DefinitionGap>,
    /// Informational diagnostics that never turn a complete definition into one with gaps.
    pub notes: Vec<DefinitionGap>,
    pub basis: &'static str,
}

const BASIS: &str = "Declared references, roles, versions and obligations only; semantic adequacy of claims, judgments and examples remains an attributed review judgment.";

fn required_for(purpose: WorkPurpose, obligation: &str) -> bool {
    match purpose {
        WorkPurpose::Implementation => true,
        WorkPurpose::Research | WorkPurpose::Document => matches!(
            obligation,
            "purpose" | "intent" | "contribution" | "granularity" | "deliverable"
        ),
        WorkPurpose::Decision => {
            matches!(
                obligation,
                "purpose" | "intent" | "contribution" | "deliverable"
            )
        }
    }
}

struct Run {
    obligations: Vec<ObligationStatus>,
    gaps: Vec<DefinitionGap>,
    /// Outline work owes only its purpose and the outcome it contributes to.
    outline: bool,
}

impl Run {
    fn owes(&self, purpose: WorkPurpose, obligation: &str) -> bool {
        required_for(purpose, obligation)
            && (!self.outline || matches!(obligation, "purpose" | "contribution"))
    }

    fn gap(&mut self, code: &'static str, detail: impl Into<String>) {
        self.gaps.push(DefinitionGap {
            code,
            detail: detail.into(),
        });
    }

    fn settle(
        &mut self,
        obligation: &'static str,
        purpose: WorkPurpose,
        ok: bool,
        provided: bool,
        source: Option<String>,
    ) {
        let state = if ok {
            "satisfied"
        } else if self.owes(purpose, obligation) {
            "missing"
        } else if provided {
            "unknown"
        } else {
            "not_applicable"
        };
        self.obligations.push(ObligationStatus {
            obligation,
            state,
            source,
        });
    }
}

/// Reference problems of the adopted intent entries (user decisions and sourced facts).
fn intent_problems(
    definition: &WorkDefinition,
    facts: &DefinitionFacts<'_>,
) -> Vec<(&'static str, String)> {
    let mut problems = Vec::new();
    for entry in definition
        .intent
        .iter()
        .filter(|e| matches!(e.role, IntentRole::UserDecision | IntentRole::SourcedFact))
    {
        let (Some(key), Some(pin)) = (&entry.decision, &entry.source_fingerprint) else {
            continue;
        };
        match facts.decisions.get(key) {
            None => problems.push(("intent_ref_not_found", format!("decision {key} is not registered or not visible"))),
            Some(current) if !current.eq_ignore_ascii_case(pin) => problems.push((
                "intent_ref_stale",
                format!("decision {key} changed after it was adopted; review it and pin the current fingerprint"),
            )),
            Some(_) => {}
        }
    }
    problems
}

fn adopted_entries(definition: &WorkDefinition) -> usize {
    definition
        .intent
        .iter()
        .filter(|e| matches!(e.role, IntentRole::UserDecision | IntentRole::SourcedFact))
        .count()
}

fn contribution_problems(
    definition: &WorkDefinition,
    facts: &DefinitionFacts<'_>,
) -> Vec<(&'static str, String)> {
    let mut problems = Vec::new();
    for c in &definition.contributes {
        match facts.goals.get(&c.goal) {
            None => problems.push((
                "contribution_goal_not_found",
                format!("goal {} is not declared or not visible", c.goal),
            )),
            Some(criteria) if !criteria.iter().any(|s| s == &c.outcome) => problems.push((
                "contribution_outcome_unknown",
                format!(
                    "the outcome is not one of the current success criteria of goal {}",
                    c.goal
                ),
            )),
            Some(_) => {}
        }
    }
    problems
}

/// Does `parent` chain lead back to `work`? Bounded by the number of works, so malformed or
/// cyclic data cannot loop.
fn parent_cycle(work: &str, facts: &DefinitionFacts<'_>) -> bool {
    let mut seen = BTreeSet::new();
    let mut at = facts
        .works
        .get(work)
        .and_then(|w| w.definition.as_ref())
        .and_then(|d| d.parent.clone());
    while let Some(key) = at {
        if key == work {
            return true;
        }
        if !seen.insert(key.clone()) {
            return false; // a cycle that does not include this work is reported on its own members
        }
        at = facts
            .works
            .get(&key)
            .and_then(|w| w.definition.as_ref())
            .and_then(|d| d.parent.clone());
    }
    false
}

/// Per-criterion tier: the work's own tier first, then the specification dispositions its
/// references declare for exactly that criterion.
fn criterion_tier(
    criterion: &str,
    definition: &WorkDefinition,
    facts: &DefinitionFacts<'_>,
) -> DeliveryTier {
    if definition.tier == Some(DeliveryTier::Later) {
        return DeliveryTier::Later;
    }
    let dispositions: Vec<RequirementDisposition> = facts
        .spec_refs
        .iter()
        .filter(|r| r.criterion.as_deref() == Some(criterion))
        .filter_map(|r| {
            facts
                .spec_dispositions
                .get(&(r.specification.clone(), r.item.clone()))
                .copied()
        })
        .collect();
    if !dispositions.is_empty()
        && dispositions
            .iter()
            .all(|d| *d == RequirementDisposition::Future)
    {
        DeliveryTier::Later
    } else {
        DeliveryTier::Required
    }
}

pub fn check_work_definition(facts: &DefinitionFacts<'_>) -> DefinitionReport {
    let Some(me) = facts
        .works
        .get(facts.work)
        .and_then(|w| w.definition.as_ref())
    else {
        return DefinitionReport {
            work: facts.work.into(),
            state: "not_declared",
            purpose: None,
            effective_tier: None,
            obligations: vec![],
            gaps: vec![],
            notes: vec![],
            basis: BASIS,
        };
    };
    let purpose = me.purpose;
    let mut run = Run {
        obligations: vec![],
        gaps: vec![],
        outline: me.outline.is_some(),
    };
    let mut notes = Vec::new();

    // The inherited definition, when it is declared and exists.
    let inherited = me.inherits.as_ref().map(|key| {
        (
            key.clone(),
            facts.works.get(key).and_then(|w| w.definition.as_ref()),
        )
    });
    if let Some((key, None)) = &inherited {
        run.gap(
            "inherits_not_found",
            format!("work {key} declares no definition to inherit"),
        );
    }
    let parent_def = inherited
        .as_ref()
        .and_then(|(k, d)| d.map(|d| (k.clone(), d)));

    run.settle("purpose", purpose, true, true, None);

    // Intent: adopted entries must resolve and be current; inheritance only fills an empty intent.
    let problems = intent_problems(me, facts);
    for (code, detail) in &problems {
        run.gap(code, detail.clone());
    }
    let own_adopted = adopted_entries(me);
    let (intent_ok, intent_source) = if own_adopted > 0 {
        (problems.is_empty(), None)
    } else if let Some((key, parent)) = &parent_def {
        let ok = adopted_entries(parent) > 0 && intent_problems(parent, facts).is_empty();
        (ok, ok.then(|| format!("inherited:{key}")))
    } else {
        (false, None)
    };
    if !intent_ok && own_adopted == 0 && run.owes(purpose, "intent") {
        run.gap(
            "intent_missing",
            "no user decision or sourced fact pins the adopted intent; assumptions and suggestions are not adopted facts",
        );
    }
    let intent_inherited = intent_source.is_some();
    run.settle(
        "intent",
        purpose,
        intent_ok,
        !me.intent.is_empty(),
        intent_source,
    );

    // Contribution to an exact success criterion.
    let problems = contribution_problems(me, facts);
    for (code, detail) in &problems {
        run.gap(code, detail.clone());
    }
    let (contribution_ok, contribution_source) = if !me.contributes.is_empty() {
        (problems.is_empty(), None)
    } else if let Some((key, parent)) = &parent_def {
        let ok = !parent.contributes.is_empty() && contribution_problems(parent, facts).is_empty();
        (ok, ok.then(|| format!("inherited:{key}")))
    } else {
        (false, None)
    };
    if !contribution_ok && me.contributes.is_empty() && run.owes(purpose, "contribution") {
        run.gap(
            "contribution_missing",
            "the work does not say which goal outcome it advances",
        );
    }
    run.settle(
        "contribution",
        purpose,
        contribution_ok,
        !me.contributes.is_empty(),
        contribution_source,
    );

    // Granularity judgment.
    if me.granularity.is_none() && run.owes(purpose, "granularity") {
        run.gap(
            "granularity_missing",
            "no attributed judgment says why this is one independently judgeable result",
        );
    }
    if me
        .granularity
        .as_ref()
        .is_some_and(|g| g.judgment == Granularity::StepOfParent)
        && me.parent.is_none()
    {
        run.gap(
            "step_without_parent",
            "the work is declared a step of a parent but names none",
        );
    }
    run.settle(
        "granularity",
        purpose,
        me.granularity.is_some(),
        me.granularity.is_some(),
        None,
    );

    // Parent graph.
    let mut parent_ok = None;
    if let Some(parent) = &me.parent {
        let mut ok = true;
        if parent == facts.work {
            run.gap("parent_self", "a work cannot be its own parent");
            ok = false;
        } else if let Some(other) = facts.works.get(parent) {
            let mine = facts
                .works
                .get(facts.work)
                .and_then(|w| w.workstream.as_ref());
            if mine.is_some() && other.workstream.is_some() && mine != other.workstream.as_ref() {
                run.gap("parent_cross_workstream", format!("parent {parent} belongs to another workstream; use a dependency for cross-workstream relations"));
                ok = false;
            }
            if parent_cycle(facts.work, facts) {
                run.gap(
                    "parent_cycle",
                    format!("the parent chain from {parent} leads back to this work"),
                );
                ok = false;
            }
        } else {
            run.gap(
                "parent_not_found",
                format!("parent {parent} is not a known work"),
            );
            ok = false;
        }
        parent_ok = Some(ok);
    }
    run.obligations.push(ObligationStatus {
        obligation: "parent",
        state: match parent_ok {
            Some(true) => "satisfied",
            Some(false) => "unknown",
            None => "not_applicable",
        },
        source: None,
    });

    // Named design or delivery responsibility. A bare link elsewhere cannot stand in for it.
    let (responsibility_ok, responsibility_source) = if me.responsibility.is_some() {
        (true, None)
    } else if let Some((key, parent)) = &parent_def {
        let ok = parent.responsibility.is_some();
        (ok, ok.then(|| format!("inherited:{key}")))
    } else {
        (false, None)
    };
    if !responsibility_ok && run.owes(purpose, "responsibility") {
        run.gap("responsibility_missing", "no named design or delivery responsibility carries this work; a link by identifier alone does not provide one");
    }
    run.settle(
        "responsibility",
        purpose,
        responsibility_ok,
        me.responsibility.is_some(),
        responsibility_source,
    );

    // Versioned inputs and allowed scope.
    let inputs_ok = !facts.spec_refs.is_empty()
        || (own_adopted > 0 && intent_problems(me, facts).is_empty())
        || intent_inherited;
    if !inputs_ok && run.owes(purpose, "inputs") {
        run.gap(
            "inputs_unversioned",
            "neither specification references nor pinned decisions fix the input versions",
        );
    }
    run.settle("inputs", purpose, inputs_ok, false, None);
    let scope_ok = !facts.paths.is_empty();
    if !scope_ok && run.owes(purpose, "scope") {
        run.gap("scope_missing", "the work declares no allowed paths");
    }
    run.settle("scope", purpose, scope_ok, false, None);

    // Prerequisites are the work's own dependency links. Having none is a legitimate answer, so
    // this row informs and never opens a gap; the readiness rules still hold unfinished ones.
    run.obligations.push(ObligationStatus {
        obligation: "prerequisites",
        state: if facts.dependencies.is_empty() {
            "not_applicable"
        } else {
            "satisfied"
        },
        source: (!facts.dependencies.is_empty()).then(|| "dependencies".to_string()),
    });

    // Deliverable, observable assertions, evidence method and failure or recovery conditions.
    let pick = |own: &Option<String>,
                field: fn(&WorkDefinition) -> &Option<String>|
     -> (bool, Option<String>) {
        if own.is_some() {
            (true, None)
        } else if let Some((key, parent)) = &parent_def {
            let ok = field(parent).is_some();
            (ok, ok.then(|| format!("inherited:{key}")))
        } else {
            (false, None)
        }
    };
    let ok = me.deliverable.is_some();
    if !ok && run.owes(purpose, "deliverable") {
        run.gap(
            "deliverable_missing",
            "the expected deliverable is not stated",
        );
    }
    run.settle("deliverable", purpose, ok, false, None);
    // The observable assertions are the work's own acceptance criteria; examples refine them.
    let ok = !facts.acceptance.is_empty();
    if !ok && run.owes(purpose, "assertions") {
        run.gap(
            "acceptance_missing",
            "the work states no acceptance criterion, so nothing observable can be asserted",
        );
    }
    run.settle(
        "assertions",
        purpose,
        ok,
        false,
        ok.then(|| "acceptance".to_string()),
    );
    let (ok, source) = pick(&me.evidence_method, |d| &d.evidence_method);
    if !ok && run.owes(purpose, "evidence_method") {
        run.gap(
            "evidence_method_missing",
            "no method says how the result will be shown",
        );
    }
    run.settle("evidence_method", purpose, ok, false, source);
    let (ok, source) = pick(&me.failure_recovery, |d| &d.failure_recovery);
    if !ok && run.owes(purpose, "failure_recovery") {
        run.gap(
            "failure_recovery_missing",
            "failure and recovery conditions are not stated",
        );
    }
    run.settle("failure_recovery", purpose, ok, false, source);

    // Tier, examples and the discrimination method.
    let tiers: Vec<(&String, DeliveryTier)> = facts
        .acceptance
        .iter()
        .map(|c| (c, criterion_tier(c, me, facts)))
        .collect();
    let required_criteria: Vec<&String> = tiers
        .iter()
        .filter(|(_, tier)| *tier == DeliveryTier::Required)
        .map(|(c, _)| *c)
        .collect();
    let effective_tier = if required_criteria.is_empty() && !tiers.is_empty() {
        DeliveryTier::Later
    } else {
        DeliveryTier::Required
    };
    if me.tier == Some(DeliveryTier::Required)
        && !facts.spec_refs.is_empty()
        && facts.spec_refs.iter().all(|r| {
            facts
                .spec_dispositions
                .get(&(r.specification.clone(), r.item.clone()))
                == Some(&RequirementDisposition::Future)
        })
    {
        run.gap("tier_conflicts_with_specification", "the work is marked required while every specification item it references is declared future");
    }
    run.obligations.push(ObligationStatus {
        obligation: "tier",
        state: if me.tier.is_some() || tiers.iter().any(|(_, t)| *t == DeliveryTier::Later) {
            "satisfied"
        } else {
            "not_applicable"
        },
        source: None,
    });
    for example in &me.examples {
        if !facts.acceptance.iter().any(|c| c == &example.criterion) {
            run.gap(
                "example_criterion_unknown",
                "an example names text that is not a current acceptance criterion",
            );
        }
    }
    let examples_needed = run.owes(purpose, "examples") && !required_criteria.is_empty();
    let mut examples_ok = true;
    for criterion in &required_criteria {
        let of = |kind| {
            me.examples
                .iter()
                .any(|e| &&e.criterion == criterion && e.kind == kind)
        };
        if !(of(ExampleKind::Positive) && of(ExampleKind::Negative)) {
            examples_ok = false;
            if examples_needed {
                run.gap(
                    "examples_missing",
                    format!(
                        "required criterion lacks a positive and a negative example: {criterion}"
                    ),
                );
            }
        }
    }
    run.obligations.push(ObligationStatus {
        obligation: "examples",
        state: if !examples_needed {
            "not_applicable"
        } else if examples_ok {
            "satisfied"
        } else {
            "missing"
        },
        source: None,
    });
    let discrimination_ok = me.discrimination.is_some()
        || parent_def
            .as_ref()
            .is_some_and(|(_, d)| d.discrimination.is_some());
    if examples_needed && !discrimination_ok {
        run.gap(
            "discrimination_missing",
            "no method says how the tests will show they detect a wrong implementation",
        );
    }
    run.obligations.push(ObligationStatus {
        obligation: "discrimination",
        state: if !examples_needed {
            "not_applicable"
        } else if discrimination_ok {
            "satisfied"
        } else {
            "missing"
        },
        source: (me.discrimination.is_none() && discrimination_ok)
            .then(|| parent_def.as_ref().map(|(k, _)| format!("inherited:{k}")))
            .flatten(),
    });

    // A side line says how it rejoins the line it came from and when it stops.
    if let Some(side_line) = &me.side_line {
        let missing = [
            (
                me.parent.is_none(),
                "side_line_without_parent",
                "a side line names the line it returns to as its parent",
            ),
            (
                side_line.return_condition.is_none(),
                "side_line_return_missing",
                "no condition says when the result returns to the line it came from",
            ),
            (
                side_line.stop_condition.is_none(),
                "side_line_stop_missing",
                "no condition says when to stop",
            ),
        ];
        for (absent, code, detail) in missing {
            if absent {
                run.gap(code, detail);
            }
        }
        run.obligations.push(ObligationStatus {
            obligation: "side_line",
            state: if missing.iter().any(|(absent, ..)| *absent) {
                "missing"
            } else {
                "satisfied"
            },
            source: None,
        });
    }

    // Larger work has no default rework budget; it declares its own.
    if me.size == Some(WorkSize::Large)
        && me.budget.as_ref().and_then(|b| b.rework_cycles).is_none()
    {
        run.gap(
            "large_work_budget_missing",
            "a large work declares its own rework budget; the figure of a standard work applies only until it does",
        );
    }

    // Outline work keeps what is known and when to elaborate it; it is not near-term executable.
    if let Some(outline) = &me.outline {
        if outline.trigger.is_none() {
            run.gap(
                "outline_trigger_missing",
                "no trigger says when this outline is elaborated",
            );
        }
        run.gap(
            "outline_not_elaborated",
            "outline work is not near-term executable; elaborate it into a full definition before admission",
        );
        run.obligations.push(ObligationStatus {
            obligation: "outline",
            state: if outline.trigger.is_some() {
                "satisfied"
            } else {
                "missing"
            },
            source: None,
        });
    }

    // Finished children never prove the parent's own outcome.
    let children: Vec<&DefinitionWork> = facts
        .works
        .iter()
        .filter(|(key, w)| {
            key.as_str() != facts.work
                && w.definition.as_ref().and_then(|d| d.parent.as_deref()) == Some(facts.work)
        })
        .map(|(_, w)| w)
        .collect();
    let own_status = facts.works.get(facts.work).map(|w| w.status);
    if !children.is_empty()
        && children.iter().all(|w| w.status == WorkStatus::Completed)
        && own_status != Some(WorkStatus::Completed)
    {
        notes.push(DefinitionGap {
            code: "parent_outcome_unproven",
            detail: "every child is completed, yet the parent's own acceptance still needs its own evidence".into(),
        });
    }

    DefinitionReport {
        work: facts.work.into(),
        state: if run.gaps.is_empty() {
            "complete"
        } else {
            "gaps"
        },
        purpose: Some(purpose),
        effective_tier: Some(effective_tier),
        obligations: run.obligations,
        gaps: run.gaps,
        notes,
        basis: BASIS,
    }
}
