//! Gaps: something that is missing or has changed in the intent, the acceptance, the design, the
//! plan or the work definition of a work. A gap is routed to the layer it affects and says how it
//! can be closed, because a fact that must be looked up, an experiment that must run, a choice
//! that only someone authorized may make and an environment that must change are different
//! things, and repairing a plan proves none of them. An open gap puts in question only the
//! judgments that depend on its layer, on that work; it keeps every failure on record, opens no
//! task and holds no unrelated work.
//!
//! This module holds the request, the pure fold of recorded gap events and the rule that decides
//! whether a gap may be closed. The store gathers the facts (whether an experiment really ran,
//! who may decide) and persists the events.
use crate::admission::bounded;
use crate::{Error, Id, Result, ensure_public_data};
use serde::{Deserialize, Serialize};

pub const GAP_EVENT: &str = "work.gap";

const MAX_LIST: usize = 32;

/// The layer a gap belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapRoute {
    /// What the user asked for.
    Intent,
    /// The behavior and the acceptance criteria that say the work is done.
    Acceptance,
    /// How it will be built. AWR keeps no judgment on design, so nothing is put in question.
    Design,
    /// How the work is cut into children and ordered.
    Plan,
    /// The declared definition of the work: purpose, examples, responsibility.
    WorkDefinition,
}

/// How a gap can be closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapResolution {
    /// A fact that exists and has to be found, with the sources that were consulted.
    FactLookup,
    /// A question only a run can answer. The run must have happened; repairing the plan around
    /// it does not count.
    FeasibilityExperiment,
    /// A choice that is not the agent's to make.
    AuthorizedChoice,
    /// Something outside the work has to change, and someone has to observe that it did.
    EnvironmentWait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapAction {
    Open,
    Resolve,
    Withdraw,
}

/// The judgments an open gap of a route puts in question, for the work it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapJudgment {
    /// The work's admission decision.
    Admission,
    /// The containment decision on the work's children.
    Containment,
}

impl GapRoute {
    /// Only judgments that read the layer are put in question; a gap in the design touches none,
    /// and a gap in the plan does not touch the work's own admission.
    pub fn puts_in_question(self) -> &'static [GapJudgment] {
        match self {
            Self::Intent => &[GapJudgment::Admission, GapJudgment::Containment],
            Self::Acceptance | Self::WorkDefinition => &[GapJudgment::Admission],
            Self::Plan => &[GapJudgment::Containment],
            Self::Design => &[],
        }
    }
}

/// What a gap request says apart from who sends it; this is what an event keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapInput {
    pub work: String,
    pub gap_key: String,
    pub action: Option<GapAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<GapRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<GapResolution>,
    /// What is missing or has changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The operation key under which the experiment will be registered as an execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<String>,
    /// Agents who may make an authorized choice; the policy's supervisors when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authority: Vec<String>,
    /// What the work waits for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    /// What was found, chosen or concluded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// The sources a fact lookup consulted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// What was observed that ended an environment wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
    /// Why a gap is withdrawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One request to open, resolve or withdraw a gap of a work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapRequest {
    pub work: String,
    /// An active session, or a declaring agent when no session is given (attribution only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub expected_revision: u64,
    /// Stable idempotency key: the same key with the same content returns the original receipt.
    pub request_key: String,
    /// Names the gap within the work and branch.
    pub gap_key: String,
    pub action: GapAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<GapRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<GapResolution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authority: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn name_ok(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= 256
        && text == text.trim()
        && !text.chars().any(char::is_control)
}

impl GapRequest {
    /// What a request key binds: the request without its revision guard and the key itself.
    pub fn input(&self) -> GapInput {
        GapInput {
            work: self.work.clone(),
            gap_key: self.gap_key.clone(),
            action: Some(self.action),
            route: self.route,
            resolution: self.resolution,
            subject: self.subject.clone(),
            experiment: self.experiment.clone(),
            authority: self.authority.clone(),
            waiting_for: self.waiting_for.clone(),
            outcome: self.outcome.clone(),
            sources: self.sources.clone(),
            observed: self.observed.clone(),
            reason: self.reason.clone(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |why: &str| Err(Error::InvalidInput(format!("gap request {why}")));
        if self.request_key.trim().is_empty()
            || self.request_key.len() > 512
            || self.request_key.chars().any(char::is_control)
        {
            return invalid("requires a bounded stable request_key");
        }
        if self.work.trim().is_empty() || self.work.len() > 512 {
            return invalid("requires the work key");
        }
        if !name_ok(&self.gap_key) {
            return invalid("requires a bounded gap_key that names the gap within the work");
        }
        match (&self.session, &self.agent) {
            (Some(_), None) => {}
            (None, Some(agent)) if name_ok(agent) => {}
            _ => {
                return invalid(
                    "names exactly one of an active session or a bounded declaring agent",
                );
            }
        }
        let text = |value: &Option<String>| value.as_deref().is_some_and(bounded);
        let blank = |value: &Option<String>| value.is_none();
        match self.action {
            GapAction::Open => {
                let (Some(_), Some(resolution), true) =
                    (self.route, self.resolution, text(&self.subject))
                else {
                    return invalid("to open a gap needs a route, a resolution and a subject");
                };
                if !(blank(&self.outcome)
                    && blank(&self.observed)
                    && blank(&self.reason)
                    && self.sources.is_empty())
                {
                    return invalid("to open a gap says nothing about how it was closed");
                }
                // Each way of closing a gap needs its own anchor, and no other.
                let anchors = (
                    self.experiment.is_some(),
                    !self.authority.is_empty(),
                    self.waiting_for.is_some(),
                );
                let fits = match resolution {
                    GapResolution::FactLookup => anchors == (false, false, false),
                    GapResolution::FeasibilityExperiment => {
                        anchors == (true, false, false)
                            && self.experiment.as_deref().is_some_and(|k| {
                                !k.trim().is_empty()
                                    && k.len() <= 512
                                    && !k.chars().any(char::is_control)
                            })
                    }
                    GapResolution::AuthorizedChoice => {
                        !anchors.0
                            && !anchors.2
                            && self.authority.len() <= MAX_LIST
                            && self.authority.iter().all(|a| name_ok(a))
                    }
                    GapResolution::EnvironmentWait => {
                        anchors == (false, false, true) && text(&self.waiting_for)
                    }
                };
                if !fits {
                    return invalid(
                        "carries the anchor of its resolution and no other: an experiment names its operation key, an authorized choice may name its authority, an environment wait says what it waits for",
                    );
                }
            }
            GapAction::Resolve => {
                if !text(&self.outcome) {
                    return invalid("to resolve a gap needs the outcome");
                }
                if self.route.is_some()
                    || self.resolution.is_some()
                    || self.subject.is_some()
                    || self.experiment.is_some()
                    || !self.authority.is_empty()
                    || self.waiting_for.is_some()
                    || self.reason.is_some()
                {
                    return invalid("to resolve a gap restates nothing about how it was opened");
                }
                if self.sources.len() > MAX_LIST || !self.sources.iter().all(|s| bounded(s)) {
                    return invalid("has unbounded sources");
                }
                if self.observed.as_deref().is_some_and(|o| !bounded(o)) {
                    return invalid("has an unbounded observation");
                }
            }
            GapAction::Withdraw => {
                if !text(&self.reason) {
                    return invalid("to withdraw a gap needs the reason");
                }
                if self.route.is_some()
                    || self.resolution.is_some()
                    || self.subject.is_some()
                    || self.experiment.is_some()
                    || !self.authority.is_empty()
                    || self.waiting_for.is_some()
                    || self.outcome.is_some()
                    || !self.sources.is_empty()
                    || self.observed.is_some()
                {
                    return invalid("to withdraw a gap says only why");
                }
            }
        }
        ensure_public_data(self)
    }
}

/// A gap as the recorded events leave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gap {
    pub key: String,
    pub route: GapRoute,
    pub resolution: GapResolution,
    pub subject: String,
    /// open, resolved or withdrawn.
    pub state: &'static str,
    pub opened_event: Id,
    pub opened_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experiment: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authority: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_event: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<String>,
}

/// One recorded gap event, in journal order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedGap {
    pub event: Id,
    pub decider_agent: String,
    pub input: GapInput,
}

/// Folds the recorded events of a work into its gaps, in the order they were opened. History is
/// never rewritten: a closed gap stays, with who closed it and how.
pub fn fold_gaps(records: &[RecordedGap]) -> Vec<Gap> {
    let mut gaps: Vec<Gap> = Vec::new();
    for record in records {
        let input = &record.input;
        match input.action {
            Some(GapAction::Open) => {
                if gaps.iter().any(|g| g.key == input.gap_key) {
                    continue;
                }
                let (Some(route), Some(resolution), Some(subject)) =
                    (input.route, input.resolution, input.subject.clone())
                else {
                    continue;
                };
                gaps.push(Gap {
                    key: input.gap_key.clone(),
                    route,
                    resolution,
                    subject,
                    state: "open",
                    opened_event: record.event,
                    opened_by: record.decider_agent.clone(),
                    experiment: input.experiment.clone(),
                    authority: input.authority.clone(),
                    waiting_for: input.waiting_for.clone(),
                    outcome: None,
                    sources: vec![],
                    observed: None,
                    reason: None,
                    closed_event: None,
                    closed_by: None,
                });
            }
            Some(action @ (GapAction::Resolve | GapAction::Withdraw)) => {
                if let Some(gap) = gaps
                    .iter_mut()
                    .find(|g| g.key == input.gap_key && g.state == "open")
                {
                    gap.state = if action == GapAction::Resolve {
                        "resolved"
                    } else {
                        "withdrawn"
                    };
                    gap.outcome = input.outcome.clone();
                    gap.sources = input.sources.clone();
                    gap.observed = input.observed.clone();
                    gap.reason = input.reason.clone();
                    gap.closed_event = Some(record.event);
                    gap.closed_by = Some(record.decider_agent.clone());
                }
            }
            None => {}
        }
    }
    gaps
}

/// A judgment that open gaps put in question, with the gaps that do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suspension {
    pub judgment: GapJudgment,
    pub gaps: Vec<String>,
}

/// Which judgments the open gaps of a work put in question. Nothing else is touched.
pub fn suspensions(gaps: &[Gap]) -> Vec<Suspension> {
    let mut found: Vec<Suspension> = Vec::new();
    for gap in gaps.iter().filter(|g| g.state == "open") {
        for judgment in gap.route.puts_in_question() {
            match found.iter_mut().find(|s| s.judgment == *judgment) {
                Some(existing) => existing.gaps.push(gap.key.clone()),
                None => found.push(Suspension {
                    judgment: *judgment,
                    gaps: vec![gap.key.clone()],
                }),
            }
        }
    }
    found
}

/// What the store established about the world when a gap is closed.
pub struct ClosingFacts<'a> {
    /// Whether an execution with the gap's experiment key was registered after the gap was
    /// opened and has reached an end, successful or not.
    pub experiment_ran: bool,
    /// The agent who closes the gap.
    pub decider: &'a str,
    /// The agents the policy names as supervisors.
    pub supervisors: &'a [String],
}

/// Whether `request` may close `gap`: the rule behind "repairing a plan does not prove its
/// experiment ran". Returns the diagnostic code and detail of the first rule broken.
pub fn check_closing(
    gap: &Gap,
    request: &GapRequest,
    facts: &ClosingFacts<'_>,
) -> std::result::Result<(), (&'static str, String)> {
    if request.action != GapAction::Resolve {
        return Ok(());
    }
    match gap.resolution {
        GapResolution::FactLookup => {
            if request.sources.is_empty() {
                return Err((
                    "gap_sources_missing",
                    "a fact lookup is closed by naming the sources that were consulted".into(),
                ));
            }
        }
        GapResolution::FeasibilityExperiment => {
            if !facts.experiment_ran {
                return Err((
                    "gap_experiment_not_run",
                    format!(
                        "no execution with the operation key {} has ended since this gap was opened; repairing the plan does not prove the experiment ran, and a failed run counts as a run",
                        gap.experiment.as_deref().unwrap_or("of the experiment")
                    ),
                ));
            }
        }
        GapResolution::AuthorizedChoice => {
            let allowed: &[String] = if gap.authority.is_empty() {
                facts.supervisors
            } else {
                &gap.authority
            };
            if !allowed.iter().any(|a| a == facts.decider) {
                return Err((
                    "gap_not_authorized",
                    format!(
                        "{} is not among the agents authorized to make this choice",
                        facts.decider
                    ),
                ));
            }
        }
        GapResolution::EnvironmentWait => {
            if request.observed.is_none() {
                return Err((
                    "gap_observation_missing",
                    "an environment wait is closed by saying what was observed that ended it"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

const BASIS: &str = "A gap is routed to the layer it affects and closed the way its kind requires. The program keeps the record, checks that a run really ran and that the right agent chose, and puts in question only the judgments that read the gap's layer, on that work; it judges no design and opens no task.";

/// The gaps of one work, with what the open ones put in question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GapStatus {
    pub work: String,
    pub gaps: Vec<Gap>,
    /// Keys of the gaps that are still open.
    pub open: Vec<String>,
    pub suspensions: Vec<Suspension>,
    pub basis: &'static str,
}

impl GapStatus {
    pub fn of(work: &str, gaps: Vec<Gap>) -> Self {
        Self {
            work: work.into(),
            open: gaps
                .iter()
                .filter(|g| g.state == "open")
                .map(|g| g.key.clone())
                .collect(),
            suspensions: suspensions(&gaps),
            gaps,
            basis: BASIS,
        }
    }
}
