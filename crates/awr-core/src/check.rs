//! Check contracts and pure oracle hashes. Execution and persistence live in their own crates.
use crate::{Error, Id, Result, Revision};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CHECK_EVENT: &str = "work.check";
pub const CHECK_PAYLOAD_CAP: usize = 16 * 1024;
pub const CHECK_FILE_CAP: u64 = 4 * 1024 * 1024;
pub const MUTATION_REPORT_CAP: usize = 4 * 1024 * 1024;
pub const MUTATION_ENTRY_CAP: usize = 5000;
pub const MUTATION_SURVIVOR_CAP: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutantStatus {
    Killed,
    Survived,
    Equivalent,
    Timeout,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutant {
    pub id: String,
    pub file: String,
    pub line: u32,
    pub status: MutantStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub killed_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationReport {
    pub version: u32,
    pub source_sha: String,
    pub tool: String,
    pub mutants: Vec<Mutant>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationCounts {
    pub killed: u32,
    pub survived: u32,
    pub equivalent: u32,
    pub timeout: u32,
    pub skipped: u32,
}
impl MutationCounts {
    pub fn passed(self) -> bool {
        self.survived == 0 && self.timeout == 0
    }
    fn total(self) -> u64 {
        [
            self.killed,
            self.survived,
            self.equivalent,
            self.timeout,
            self.skipped,
        ]
        .into_iter()
        .map(u64::from)
        .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationSurvivor {
    pub id: String,
    pub file: String,
    pub line: u32,
}
impl MutationSurvivor {
    fn valid(&self) -> bool {
        report_label(&self.id, 128) && report_label(&self.file, 512) && self.line > 0
    }
}

fn report_label(value: &str, cap: usize) -> bool {
    !value.trim().is_empty() && value.len() <= cap && !value.chars().any(char::is_control)
}

impl MutationReport {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MUTATION_REPORT_CAP {
            return Err(Error::InvalidInput("mutation report exceeds 4 MiB".into()));
        }
        let report: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::InvalidInput(format!("invalid mutation report: {e}")))?;
        if report.version != 1
            || !crate::is_source_sha(&report.source_sha)
            || !report_label(&report.tool, 256)
            || report.mutants.len() > MUTATION_ENTRY_CAP
        {
            return Err(Error::InvalidInput("mutation report requires version 1, source_sha, a tool label and at most 5000 mutants".into()));
        }
        let mut ids = std::collections::BTreeSet::new();
        for mutant in &report.mutants {
            if !report_label(&mutant.id, 128)
                || !report_label(&mutant.file, 512)
                || mutant.line == 0
                || !ids.insert(&mutant.id)
            {
                return Err(Error::InvalidInput(format!(
                    "mutation entry {} needs a unique id (128 bytes), file (512 bytes) and positive line",
                    mutant.id
                )));
            }
            if mutant.status == MutantStatus::Equivalent
                && mutant
                    .note
                    .as_ref()
                    .is_none_or(|note| note.trim().is_empty())
            {
                return Err(Error::InvalidInput(format!(
                    "equivalent mutation entry {} requires a nonblank note",
                    mutant.id
                )));
            }
        }
        Ok(report)
    }

    pub fn counts(&self) -> MutationCounts {
        let mut counts = MutationCounts::default();
        for mutant in &self.mutants {
            match mutant.status {
                MutantStatus::Killed => counts.killed += 1,
                MutantStatus::Survived => counts.survived += 1,
                MutantStatus::Equivalent => counts.equivalent += 1,
                MutantStatus::Timeout => counts.timeout += 1,
                MutantStatus::Skipped => counts.skipped += 1,
            }
        }
        counts
    }

    pub fn survivors(&self) -> Vec<MutationSurvivor> {
        self.mutants
            .iter()
            .filter(|m| m.status == MutantStatus::Survived)
            .take(MUTATION_SURVIVOR_CAP)
            .map(|m| MutationSurvivor {
                id: m.id.clone(),
                file: m.file.clone(),
                line: m.line,
            })
            .collect()
    }
}

// Oracle identity: file sizes and execution observations never change the declared oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

pub fn example_oracle(id: &str, run: &[String], files: &[OracleFile]) -> Result<String> {
    let files: Vec<_> = files
        .iter()
        .map(|file| json!({"path": file.path, "sha256": file.sha256}))
        .collect();
    oracle_hash(
        "awr-example-oracle-v1\n",
        &json!({"id": id, "run": run, "files": files}),
    )
}

/// Callers supply only required examples with a check binding; order is by stable example ID.
pub fn oracle_fingerprint(oracles: &[(String, String)]) -> Result<String> {
    let mut sorted: Vec<_> = oracles.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    oracle_hash(
        "awr-oracle-v1\n",
        &json!(sorted.iter().map(|v| &v.1).collect::<Vec<_>>()),
    )
}

fn oracle_hash(domain: &str, value: &Value) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    // serde_json's object map is ordered, matching the existing canonical JSON hashes.
    hash.update(serde_json::to_vec(value)?);
    Ok(format!("sha256:{:x}", hash.finalize()))
}

// Run observations. Later check gates and discrimination rules can build on these types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckPhase {
    Baseline,
    Acceptance,
    Mutation,
}
impl CheckPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Acceptance => "acceptance",
            Self::Mutation => "mutation",
        }
    }
    pub fn met(self, outcome: CheckOutcome) -> bool {
        outcome
            == if self == Self::Baseline {
                CheckOutcome::Failed
            } else {
                CheckOutcome::Passed
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    Errored,
    Unknown,
}
impl CheckOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Errored => "errored",
            Self::Unknown => "unknown",
        }
    }
}

/// Why a baseline failed. Missing classifications retain the historical failure semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineClassification {
    Passed,
    FailedAsExpected,
    FailedUnrunnable,
    FailedUnclassified,
    Legacy,
}
/// Collect known features from captured output; this performs no I/O or extra execution.
#[derive(Default)]
pub struct BaselineClassifier {
    assertion_failed: bool,
    unrunnable: bool,
}
impl BaselineClassifier {
    pub fn observe(&mut self, output: &str) {
        for line in output.lines().map(str::trim) {
            let lower = line.to_ascii_lowercase();
            self.unrunnable |= [
                "ModuleNotFoundError",
                "ImportError",
                "SyntaxError",
                "error[E",
                "command not found",
                "not recognized as an internal or external command",
                "error: could not compile",
            ]
            .iter()
            .any(|marker| line.contains(marker));
            // A missing input file inside an assertion is a behavior failure. Only launcher
            // diagnostics identify a missing command or entry-point file here.
            self.unrunnable |= lower.contains("no such file or directory")
                && [
                    "sh:",
                    "bash:",
                    "zsh:",
                    "dash:",
                    "/bin/sh:",
                    "/bin/bash:",
                    "env:",
                    "/usr/bin/env:",
                ]
                .iter()
                .any(|prefix| lower.starts_with(prefix));
            self.unrunnable |=
                lower.contains("no such file or directory") && lower.contains(": can't open file ");
            let pytest = line
                .strip_prefix('E')
                .or_else(|| line.strip_prefix('>'))
                .filter(|rest| rest.starts_with(char::is_whitespace))
                .is_some_and(|rest| rest.trim_start().starts_with("assert "));
            self.assertion_failed |= line.starts_with("FAIL:")
                || line.contains("AssertionError")
                || (line.contains("assertion ") && line.contains(" failed"))
                || line.starts_with("test result: FAILED")
                || pytest;
        }
    }

    pub fn classify(self, exit_code: Option<i32>, reason: Option<&str>) -> BaselineClassification {
        use BaselineClassification::*;
        if matches!(reason, Some("timeout" | "spawn_failed")) {
            FailedUnrunnable
        } else if exit_code == Some(0) && reason.is_none() {
            Passed
        } else if exit_code.is_none() || reason.is_some() {
            FailedUnclassified
        } else if self.unrunnable {
            FailedUnrunnable
        } else if self.assertion_failed {
            FailedAsExpected
        } else {
            FailedUnclassified
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckTreeState {
    Clean,
    Dirty,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckPayload {
    pub version: u32,
    pub example: String,
    pub criterion_sha256: String,
    pub phase: CheckPhase,
    pub outcome: CheckOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<BaselineClassification>,
    pub reason: Option<String>,
    pub log_dir: String,
    pub exit_code: Option<i32>,
    pub example_oracle: Option<String>,
    pub oracle_files: Vec<OracleFile>,
    pub run_sha256: String,
    pub run_preview: String,
    pub source_sha: Option<String>,
    pub tree_state: CheckTreeState,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub started_at: i64,
    pub finished_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutants: Option<MutationCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub survivors: Option<Vec<MutationSurvivor>>,
}
impl CheckPayload {
    pub fn baseline_classification(&self) -> Option<BaselineClassification> {
        (self.phase == CheckPhase::Baseline).then_some(
            self.classification
                .unwrap_or(BaselineClassification::Legacy),
        )
    }

    pub fn met(&self) -> bool {
        self.phase.met(self.outcome)
            && (self.phase != CheckPhase::Baseline
                || matches!(
                    self.baseline_classification(),
                    Some(BaselineClassification::FailedAsExpected | BaselineClassification::Legacy)
                ))
    }

    pub fn validate(&self) -> Result<()> {
        let hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let relative = |s: &str| {
            !s.is_empty()
                && !s.contains(['\\', ':'])
                && s.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
        };
        let example_id = (1..=32).contains(&self.example.len())
            && self
                .example
                .starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && self
                .example
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if self.version != 1
            || !example_id
            || !hash(&self.criterion_sha256)
            || !hash(&self.run_sha256)
            || !hash(&self.stdout_sha256)
            || !hash(&self.stderr_sha256)
            || self.run_preview.len() > 256
            || self.started_at < 0
            || self.finished_at < self.started_at
            || !self.log_dir.starts_with(".awr/artifacts/checks/")
            || !relative(&self.log_dir)
            || self
                .source_sha
                .as_ref()
                .is_some_and(|s| !crate::is_source_sha(s))
            || self
                .example_oracle
                .as_ref()
                .is_some_and(|s| !s.strip_prefix("sha256:").is_some_and(hash))
            || self.oracle_files.iter().any(|f| {
                f.path.starts_with(['/', '\\'])
                    || f.path.split(['/', '\\']).any(|p| p == "..")
                    || f.path.trim().is_empty()
                    || !hash(&f.sha256)
                    || f.size > CHECK_FILE_CAP
            })
            || serde_json::to_vec(self)?.len() > CHECK_PAYLOAD_CAP
            || self
                .mutants
                .is_some_and(|c| c.total() > MUTATION_ENTRY_CAP as u64)
            || self.survivors.as_ref().is_some_and(|s| {
                s.len() > MUTATION_SURVIVOR_CAP
                    || s.iter().any(|m| !m.valid())
                    || self.mutants.is_none_or(|c| {
                        s.len() as u32 != c.survived.min(MUTATION_SURVIVOR_CAP as u32)
                    })
            })
            || (self.mutants.is_some()
                && (self.phase != CheckPhase::Mutation || self.survivors.is_none()))
            || (self.phase == CheckPhase::Mutation
                && self.outcome == CheckOutcome::Passed
                && self.mutants.is_some_and(|c| !c.passed()))
            || self.classification.is_some_and(|classification| {
                self.phase != CheckPhase::Baseline
                    || match classification {
                        BaselineClassification::Passed => self.outcome != CheckOutcome::Passed,
                        BaselineClassification::FailedAsExpected => {
                            self.outcome != CheckOutcome::Failed
                                || self.exit_code.is_none_or(|code| code == 0)
                                || self.reason.is_some()
                        }
                        BaselineClassification::FailedUnrunnable
                        | BaselineClassification::FailedUnclassified => {
                            self.outcome == CheckOutcome::Passed
                        }
                        BaselineClassification::Legacy => false,
                    }
            })
        {
            return Err(Error::InvalidInput(
                "invalid or oversized work.check observation".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RecordCheckRequest {
    pub work: String,
    pub session: Id,
    pub expected_revision: Revision,
    pub payload: CheckPayload,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckGateMode {
    #[default]
    Off,
    Observe,
    Enforce,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmallWorkChecks {
    #[default]
    Reason,
    Checks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationCheck {
    pub run: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckGatePolicy {
    pub mode: CheckGateMode,
    pub scope: Option<crate::AdmissionScope>,
    pub small_work: SmallWorkChecks,
    pub require_clean_tree: bool,
    pub separate_approver: bool,
    pub mutation: Option<MutationCheck>,
}
impl Default for CheckGatePolicy {
    fn default() -> Self {
        Self {
            mode: CheckGateMode::Off,
            scope: None,
            small_work: SmallWorkChecks::Reason,
            require_clean_tree: true,
            separate_approver: false,
            mutation: None,
        }
    }
}
impl CheckGatePolicy {
    pub fn validate(&self) -> Result<()> {
        if let Some(mutation) = &self.mutation
            && (!(1..=64).contains(&mutation.run.len())
                || mutation.run[0].trim().is_empty()
                || mutation
                    .run
                    .iter()
                    .any(|a| a.len() > 1024 || a.contains('\0')))
        {
            return Err(Error::InvalidInput(".awr/gates.toml: checks.mutation.run requires 1..64 arguments of at most 1024 bytes and a nonblank executable".into()));
        }
        // Reuse the source-owned scope contract and its bounded tag validation.
        crate::AdmissionPolicy {
            version: 1,
            mode: if self.mode == CheckGateMode::Enforce {
                crate::AdmissionMode::GovernedEnforce
            } else {
                crate::AdmissionMode::LegacyObserve
            },
            scope: self.scope.clone(),
            independent_review_tags: vec![],
            supervisors: vec![],
        }.validate().map_err(|_| Error::InvalidInput(
            ".awr/gates.toml: checks enforce requires scope; tag scopes need 1..32 bounded tags".into()
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnmetCheck {
    pub example: Option<String>,
    pub criterion: Option<String>,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub survivors: Vec<MutationSurvivor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckGate {
    pub applies: bool,
    pub mode: CheckGateMode,
    pub unmet: Vec<UnmetCheck>,
    pub would_refuse: bool,
}

/// Current source facts, gathered by the store. Evaluation performs no I/O.
pub struct CheckGateDefinition<'a> {
    pub definition: Option<&'a crate::WorkDefinition>,
    pub tags: &'a [String],
    pub required_criteria: &'a [String],
    pub oracles: &'a [(String, Option<String>)],
    pub oracle_fingerprint: Option<&'a str>,
    pub session: Option<Id>,
    pub check_waiver: Option<&'a str>,
}

pub struct CheckGateRequest {
    pub work: String,
    pub branch: Option<Id>,
    pub session: Option<Id>,
    pub source_sha: String,
    pub check_waiver: Option<String>,
}

pub fn evaluate_check_gate(
    facts: &CheckGateDefinition<'_>,
    admission: Option<&crate::RecordedAdmission>,
    check_events: &[CheckPayload],
    source_sha: &str,
    policy: &CheckGatePolicy,
) -> Result<CheckGate> {
    use crate::{AdmissionDecision, AdmissionScope, DiscriminationMethod, WorkPurpose, WorkSize};
    policy.validate()?;
    if let Some(reason) = facts.check_waiver
        && (policy.small_work != SmallWorkChecks::Reason
            || facts.definition.and_then(|d| d.size) != Some(WorkSize::Small)
            || reason.trim().is_empty()
            || reason.len() > 4096
            || reason.chars().any(char::is_control))
    {
        return Err(Error::InvalidInput(
            "check_waiver requires small work, small_work = reason and one nonempty bounded line"
                .into(),
        ));
    }
    let scoped = match &policy.scope {
        Some(AdmissionScope::All) | None => true,
        Some(AdmissionScope::Tags(tags)) => tags.iter().any(|t| facts.tags.contains(t)),
    };
    let applies = policy.mode != CheckGateMode::Off
        && scoped
        && facts
            .definition
            .is_some_and(|d| d.purpose == WorkPurpose::Implementation)
        && !facts.required_criteria.is_empty();
    let mut result = CheckGate {
        applies,
        mode: policy.mode,
        unmet: vec![],
        would_refuse: false,
    };
    if !applies || facts.check_waiver.is_some() {
        return Ok(result);
    }
    let definition = facts.definition.expect("applicable gate has a definition");
    let approved = admission.filter(|a| a.decision == AdmissionDecision::Approved);
    let oracle_current = facts.oracle_fingerprint.is_some()
        && approved.and_then(|a| a.bound.oracle_fingerprint.as_deref()) == facts.oracle_fingerprint;
    let mut survivor_details = std::collections::BTreeMap::new();
    let mut unmet = |example: Option<&str>, criterion: Option<&str>, reason: &str| {
        result.unmet.push(UnmetCheck {
            example: example.map(String::from),
            criterion: criterion.map(String::from),
            reason: reason.into(),
            survivors: vec![],
        });
    };
    for criterion in facts.required_criteria {
        let examples: Vec<_> = definition
            .examples
            .iter()
            .filter(|e| &e.criterion == criterion)
            .collect();
        if examples.is_empty() {
            unmet(None, Some(criterion), "check_unbound");
        }
        for example in examples {
            let id = example.id.as_deref();
            if example.check.is_none() || id.is_none() {
                unmet(id, Some(criterion), "check_unbound");
                continue;
            }
            let oracle = facts
                .oracles
                .iter()
                .find(|(key, _)| Some(key.as_str()) == id)
                .and_then(|(_, hash)| hash.as_deref());
            if !oracle_current || oracle.is_none() {
                unmet(id, Some(criterion), "oracle_changed");
            }
            let criterion_hash = format!("{:x}", Sha256::digest(criterion.as_bytes()));
            let matches = |event: &&CheckPayload| {
                Some(event.example.as_str()) == id
                    && event.criterion_sha256 == criterion_hash
                    && oracle.is_some()
                    && event.example_oracle.as_deref() == oracle
            };
            let acceptances: Vec<_> = check_events
                .iter()
                .rev()
                .filter(matches)
                .filter(|event| {
                    event.phase == CheckPhase::Acceptance
                        && event
                            .source_sha
                            .as_ref()
                            .is_some_and(|sha| sha.eq_ignore_ascii_case(source_sha))
                })
                .collect();
            // A passing run of this exact commit and oracle remains a valid observation.
            let acceptance = acceptances
                .iter()
                .copied()
                .find(|event| {
                    event.outcome == CheckOutcome::Passed
                        && (!policy.require_clean_tree || event.tree_state == CheckTreeState::Clean)
                })
                .or_else(|| {
                    acceptances
                        .iter()
                        .copied()
                        .find(|event| event.outcome == CheckOutcome::Passed)
                })
                .or_else(|| acceptances.first().copied());
            match acceptance {
                None => unmet(id, Some(criterion), "acceptance_missing"),
                Some(event) => {
                    if event.outcome != CheckOutcome::Passed {
                        unmet(id, Some(criterion), "acceptance_failed");
                    }
                    if policy.require_clean_tree && event.tree_state != CheckTreeState::Clean {
                        unmet(id, Some(criterion), "acceptance_dirty_tree");
                    }
                }
            }
            match definition.discrimination.as_ref().map(|d| d.method) {
                Some(DiscriminationMethod::DiffMutation) => {
                    let mutations: Vec<_> = check_events
                        .iter()
                        .rev()
                        .filter(matches)
                        .filter(|event| {
                            event.phase == CheckPhase::Mutation
                                && event
                                    .source_sha
                                    .as_ref()
                                    .is_some_and(|sha| sha.eq_ignore_ascii_case(source_sha))
                        })
                        .collect();
                    let passed = |event: &&CheckPayload| {
                        event.outcome == CheckOutcome::Passed
                            && event.mutants.is_some_and(MutationCounts::passed)
                            && (!policy.require_clean_tree
                                || event.tree_state == CheckTreeState::Clean)
                    };
                    if !mutations.iter().any(passed) {
                        match mutations.first() {
                            None => unmet(id, Some(criterion), "mutation_missing"),
                            Some(event) => {
                                let counts = event.mutants.unwrap_or_default();
                                if counts.survived > 0 {
                                    unmet(id, Some(criterion), "mutants_survived");
                                    survivor_details.insert(
                                        event.example.clone(),
                                        event.survivors.clone().unwrap_or_default(),
                                    );
                                }
                                if counts.timeout > 0 || event.reason.as_deref() == Some("timeout")
                                {
                                    unmet(id, Some(criterion), "mutation_timeout");
                                }
                                if policy.require_clean_tree
                                    && event.tree_state != CheckTreeState::Clean
                                {
                                    unmet(id, Some(criterion), "mutation_dirty_tree");
                                }
                                if counts.passed()
                                    && event.reason.as_deref() != Some("timeout")
                                    && (event.outcome != CheckOutcome::Passed
                                        || event.mutants.is_none())
                                {
                                    unmet(id, Some(criterion), "mutation_failed");
                                }
                            }
                        }
                    }
                }
                Some(DiscriminationMethod::JustifiedAlternative) => {
                    if definition
                        .discrimination
                        .as_ref()
                        .and_then(|d| d.note.as_deref())
                        .is_none_or(|note| note.trim().is_empty())
                    {
                        unmet(id, Some(criterion), "discrimination_note_missing");
                    }
                    if approved.is_none_or(|a| {
                        a.proposer_session.is_none()
                            || a.decider_session.is_none()
                            || a.proposer_session == a.decider_session
                    }) {
                        unmet(id, Some(criterion), "alternative_unreviewed");
                    }
                }
                _ => {
                    let baselines: Vec<_> = check_events
                        .iter()
                        .filter(matches)
                        .filter(|e| e.phase == CheckPhase::Baseline)
                        .collect();
                    if !baselines.iter().any(|e| e.met()) {
                        unmet(
                            id,
                            Some(criterion),
                            if baselines.iter().any(|e| {
                                e.classification == Some(BaselineClassification::FailedUnrunnable)
                            }) {
                                crate::BASELINE_UNRUNNABLE
                            } else if baselines.iter().any(|e| {
                                e.classification == Some(BaselineClassification::FailedUnclassified)
                            }) {
                                crate::BASELINE_UNCLASSIFIED
                            } else if baselines.iter().any(|e| e.outcome == CheckOutcome::Passed) {
                                "baseline_passed"
                            } else {
                                "baseline_missing"
                            },
                        );
                    }
                }
            }
        }
    }
    if policy.separate_approver
        && facts.session.is_some()
        && approved.and_then(|a| a.decider_session) == facts.session
    {
        unmet(None, None, "approver_is_implementer");
    }
    for item in &mut result.unmet {
        if item.reason == "mutants_survived" {
            item.survivors = item
                .example
                .as_ref()
                .and_then(|id| survivor_details.get(id))
                .cloned()
                .unwrap_or_default();
        }
    }
    result.would_refuse = !result.unmet.is_empty();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_baseline_classifier_requires_known_behavior_features() {
        use BaselineClassification::*;
        for (output, expected) in [
            ("FAIL: test_save (SaveTests)", FailedAsExpected),
            ("AssertionError: draft lost", FailedAsExpected),
            ("E       assert saved == draft", FailedAsExpected),
            (">       assert saved", FailedAsExpected),
            ("assertion `left == right` failed", FailedAsExpected),
            ("test result: FAILED. 1 failed", FailedAsExpected),
            ("", FailedUnclassified),
            ("assert is enabled", FailedUnclassified),
            ("assertion helpers loaded", FailedUnclassified),
        ] {
            let mut classifier = BaselineClassifier::default();
            classifier.observe(output);
            assert_eq!(classifier.classify(Some(1), None), expected, "{output}");
        }
    }

    #[test]
    fn check_baseline_classifier_prioritizes_unrunnable_output_across_streams() {
        use BaselineClassification::*;
        for output in [
            "ModuleNotFoundError: tests.test_save",
            "ImportError: save",
            "SyntaxError: unexpected token",
            "sh: missing-check: command not found",
            "sh: ./missing-check: No such file or directory",
            "python3: can't open file 'missing.py': [Errno 2] No such file or directory",
            "error[E0425]: missing name",
            "error: could not compile `storage`",
        ] {
            for first in [true, false] {
                let mut classifier = BaselineClassifier::default();
                for line in if first {
                    [output, "test result: FAILED"]
                } else {
                    ["test result: FAILED", output]
                } {
                    classifier.observe(line);
                }
                assert_eq!(
                    classifier.classify(Some(1), None),
                    FailedUnrunnable,
                    "{output}"
                );
            }
        }
        let mut classifier = BaselineClassifier::default();
        classifier.observe("AssertionError: No such file or directory for the user's input");
        assert_eq!(classifier.classify(Some(1), None), FailedAsExpected);
    }

    #[test]
    fn check_baseline_classifier_uses_exit_and_execution_failure_without_guessing() {
        use BaselineClassification::*;
        for (exit, reason, expected) in [
            (Some(0), None, Passed),
            (None, Some("timeout"), FailedUnrunnable),
            (None, Some("spawn_failed"), FailedUnrunnable),
            (None, Some("terminated_by_signal"), FailedUnclassified),
            (
                Some(1),
                Some("oracle_changed_during_run"),
                FailedUnclassified,
            ),
        ] {
            let mut classifier = BaselineClassifier::default();
            classifier.observe("AssertionError: pending behavior");
            assert_eq!(classifier.classify(exit, reason), expected);
        }
    }

    #[test]
    fn check_oracle_hashes_the_versioned_canonical_document_without_file_sizes() {
        let mut files = vec![OracleFile {
            path: "tests/draft.rs".into(),
            sha256: "a".repeat(64),
            size: 42,
        }];
        let argv = vec!["draft-test".into(), "--exact".into()];
        let value = json!({"id":"draft", "run":argv, "files":[{"path":"tests/draft.rs","sha256":"a".repeat(64)}]});
        let expected = format!(
            "sha256:{:x}",
            Sha256::digest(format!("awr-example-oracle-v1\n{value}"))
        );
        assert_eq!(example_oracle("draft", &argv, &files).unwrap(), expected);
        files[0].size = 43;
        assert_eq!(example_oracle("draft", &argv, &files).unwrap(), expected);
        files[0].sha256 = "b".repeat(64);
        assert_ne!(example_oracle("draft", &argv, &files).unwrap(), expected);
        assert_ne!(example_oracle("other", &argv, &files).unwrap(), expected);
    }
    #[test]
    fn check_fingerprint_orders_by_id_and_keeps_individual_oracles_independent() {
        let a = vec![("z".into(), "hash-z".into()), ("a".into(), "hash-a".into())];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(
            oracle_fingerprint(&a).unwrap(),
            oracle_fingerprint(&b).unwrap()
        );
        b[0].1 = "changed".into();
        assert_ne!(
            oracle_fingerprint(&a).unwrap(),
            oracle_fingerprint(&b).unwrap()
        );
    }
    #[test]
    fn check_timeout_and_errors_do_not_meet_baseline_or_acceptance() {
        assert!(CheckPhase::Baseline.met(CheckOutcome::Failed));
        assert!(CheckPhase::Acceptance.met(CheckOutcome::Passed));
        for phase in [
            CheckPhase::Baseline,
            CheckPhase::Acceptance,
            CheckPhase::Mutation,
        ] {
            assert!(!phase.met(CheckOutcome::Unknown));
            assert!(!phase.met(CheckOutcome::Errored));
        }
    }

    fn mutation_document() -> Value {
        json!({"version":1,"source_sha":"a".repeat(40),"tool":"synthetic-report 1",
            "mutants":[{"id":"m1","file":"src/save.rs","line":42,"status":"killed","killed_by":["keeps-input"]}]})
    }

    fn mutation_report(document: Value) -> Result<MutationReport> {
        MutationReport::parse(&serde_json::to_vec(&document).unwrap())
    }

    #[test]
    fn check_mutation_report_counts_all_five_statuses_and_caps_survivors() {
        let mut doc = mutation_document();
        let mutants = doc["mutants"].as_array_mut().unwrap();
        for (index, status) in ["survived", "equivalent", "timeout", "skipped"]
            .iter()
            .enumerate()
        {
            mutants.push(json!({"id":format!("m-{}",index),"file":"src/save.rs","line":12,"status":status,"note":"Guard excludes this branch"}));
        }
        let report = mutation_report(doc.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(report.counts()).unwrap(),
            json!({"killed":1,"survived":1,"equivalent":1,"timeout":1,"skipped":1})
        );
        assert!(!report.counts().passed());
        for index in 0..25 {
            doc["mutants"].as_array_mut().unwrap().push(json!({"id":format!("live-{index}"),"file":"src/save.rs","line":index+1,"status":"survived"}));
        }
        let report = mutation_report(doc).unwrap();
        assert_eq!(report.counts().survived, 26);
        assert_eq!(report.survivors().len(), 20);
    }

    #[test]
    fn check_mutation_equivalent_requires_a_nonblank_note_and_identifies_the_entry() {
        for note in [Value::Null, json!(""), json!(" \n ")] {
            let mut doc = mutation_document();
            doc["mutants"][0]["status"] = json!("equivalent");
            doc["mutants"][0]["note"] = note;
            assert!(mutation_report(doc).unwrap_err().to_string().contains("m1"));
        }
        let mut doc = mutation_document();
        doc["mutants"][0]["status"] = json!("equivalent");
        doc["mutants"][0]["note"] = json!("The guard excludes this branch");
        assert_eq!(mutation_report(doc).unwrap().counts().equivalent, 1);
    }

    #[test]
    fn check_mutation_report_enforces_byte_entry_version_and_identity_bounds() {
        let mut doc = mutation_document();
        doc["mutants"] = json!((0..5000).map(|i| json!({"id":format!("m{i}"),"file":"src/save.rs","line":1,"status":"killed"})).collect::<Vec<_>>());
        assert_eq!(mutation_report(doc.clone()).unwrap().counts().killed, 5000);
        doc["mutants"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"extra","file":"src/save.rs","line":1,"status":"killed"}));
        assert!(mutation_report(doc).is_err());
        let bytes = serde_json::to_vec(&mutation_document()).unwrap();
        let mut capped = bytes.clone();
        capped.resize(4 * 1024 * 1024, b' ');
        assert!(MutationReport::parse(&capped).is_ok());
        capped.push(b' ');
        assert!(MutationReport::parse(&capped).is_err());
        for (pointer, value) in [
            ("/version", json!(2)),
            ("/source_sha", json!("unknown")),
            ("/tool", json!("")),
            ("/mutants/0/status", json!("unknown")),
            ("/mutants/0/line", json!(0)),
            ("/mutants/0/file", json!("")),
            ("/mutants/0/id", json!("")),
        ] {
            let mut doc = mutation_document();
            *doc.pointer_mut(pointer).unwrap() = value;
            assert!(mutation_report(doc).is_err(), "{pointer}");
        }
        let mut doc = mutation_document();
        let duplicate = doc["mutants"][0].clone();
        doc["mutants"].as_array_mut().unwrap().push(duplicate);
        assert!(mutation_report(doc).is_err());
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::*;

    fn definition() -> WorkDefinition {
        serde_json::from_value(
            json!({"version":1,"purpose":"implementation","tier":"required",
            "size":"medium","discrimination":{"method":"fail_before_pass_after"},
            "examples":[{"id":"retains-draft","criterion":"Retain the draft","kind":"positive",
            "given":"a draft","when":"saving","then":"the draft remains",
            "check":{"run":["draft-check"],"files":["oracle.txt"]}}]}),
        )
        .unwrap()
    }
    fn observation(phase: CheckPhase, outcome: CheckOutcome) -> CheckPayload {
        CheckPayload {
            version: 1,
            example: "retains-draft".into(),
            criterion_sha256: format!("{:x}", Sha256::digest(b"Retain the draft")),
            phase,
            outcome,
            classification: None,
            reason: None,
            log_dir: ".awr/artifacts/checks/run/retains-draft".into(),
            exit_code: Some(0),
            example_oracle: Some("oracle".into()),
            oracle_files: vec![],
            run_sha256: "a".repeat(64),
            run_preview: "draft-check".into(),
            source_sha: Some("b".repeat(40)),
            tree_state: CheckTreeState::Clean,
            stdout_sha256: "c".repeat(64),
            stderr_sha256: "d".repeat(64),
            started_at: 0,
            finished_at: 1,
            mutants: None,
            survivors: None,
        }
    }
    fn approved(session: Id) -> RecordedAdmission {
        RecordedAdmission {
            event: Id::new(),
            decision: AdmissionDecision::Approved,
            reason: None,
            proposer_session: None,
            decider_session: Some(session),
            bound: AdmissionBinding {
                contract_fingerprint: "contract".into(),
                definition_fingerprint: None,
                examples_fingerprint: None,
                oracle_fingerprint: Some("fingerprint".into()),
                policy_fingerprint: "policy".into(),
                purpose: Some(WorkPurpose::Implementation),
                tier: Some(DeliveryTier::Required),
            },
        }
    }
    fn evaluate(
        d: &WorkDefinition,
        events: &[CheckPayload],
        policy: &CheckGatePolicy,
        same_approver: bool,
    ) -> CheckGate {
        let approver = Id::new();
        let session = if same_approver { approver } else { Id::new() };
        evaluate_check_gate(
            &CheckGateDefinition {
                definition: Some(d),
                tags: &["checked".into()],
                required_criteria: &["Retain the draft".into()],
                oracles: &[("retains-draft".into(), Some("oracle".into()))],
                oracle_fingerprint: Some("fingerprint"),
                session: Some(session),
                check_waiver: None,
            },
            Some(&approved(approver)),
            events,
            &"b".repeat(40),
            policy,
        )
        .unwrap()
    }
    fn policy() -> CheckGatePolicy {
        CheckGatePolicy {
            mode: CheckGateMode::Enforce,
            scope: Some(AdmissionScope::All),
            ..Default::default()
        }
    }
    fn passing() -> Vec<CheckPayload> {
        vec![
            observation(CheckPhase::Baseline, CheckOutcome::Failed),
            observation(CheckPhase::Acceptance, CheckOutcome::Passed),
        ]
    }
    fn reasons(gate: &CheckGate) -> Vec<&str> {
        gate.unmet.iter().map(|u| u.reason.as_str()).collect()
    }

    #[test]
    fn check_gate_baseline_classification_accepts_only_behavior_failures() {
        for (classification, reason) in [
            ("failed_as_expected", None),
            ("failed_unrunnable", Some("baseline_unrunnable")),
            ("failed_unclassified", Some("baseline_unclassified")),
        ] {
            let mut events = passing();
            let mut payload = serde_json::to_value(&events[0]).unwrap();
            payload["classification"] = json!(classification);
            events[0] = serde_json::from_value(payload).unwrap();
            let gate = evaluate(&definition(), &events, &policy(), false);
            assert_eq!(
                gate.would_refuse,
                reason.is_some(),
                "{classification}: {gate:?}"
            );
            if let Some(reason) = reason {
                assert!(reasons(&gate).contains(&reason), "{gate:?}");
            }
        }
    }

    #[test]
    fn check_gate_legacy_failed_baseline_preserves_approved_admission() {
        let mut events = passing();
        let mut payload = serde_json::to_value(&events[0]).unwrap();
        payload.as_object_mut().unwrap().remove("classification");
        events[0] = serde_json::from_value(payload).unwrap();
        assert_eq!(
            events[0].baseline_classification(),
            Some(BaselineClassification::Legacy)
        );
        assert!(!evaluate(&definition(), &events, &policy(), false).would_refuse);
        events[0].outcome = CheckOutcome::Unknown;
        assert!(evaluate(&definition(), &events, &policy(), false).would_refuse);
    }

    #[test]
    fn check_baseline_classification_is_optional_and_rejects_inconsistent_payloads() {
        let mut payload = observation(CheckPhase::Baseline, CheckOutcome::Failed);
        payload.example_oracle = Some(format!("sha256:{}", "a".repeat(64)));
        payload.exit_code = Some(1);
        payload.classification = Some(BaselineClassification::FailedAsExpected);
        assert!(payload.validate().is_ok());
        assert!(payload.met());
        payload.outcome = CheckOutcome::Passed;
        assert!(payload.validate().is_err());
        assert!(!payload.met());
        payload.classification = None;
        payload.phase = CheckPhase::Acceptance;
        assert!(payload.validate().is_ok());
        assert!(payload.met());
        payload.classification = Some(BaselineClassification::Passed);
        assert!(payload.validate().is_err());
    }

    #[test]
    fn check_gate_acceptance_outcome_and_current_oracle_are_required() {
        assert!(!evaluate(&definition(), &passing(), &policy(), false).would_refuse);
        for outcome in [
            CheckOutcome::Failed,
            CheckOutcome::Errored,
            CheckOutcome::Unknown,
        ] {
            let mut events = passing();
            events[1].outcome = outcome;
            assert!(
                reasons(&evaluate(&definition(), &events, &policy(), false))
                    .contains(&"acceptance_failed")
            );
        }
        let mut events = passing();
        events[1].example_oracle = Some("earlier-oracle".into());
        assert!(
            reasons(&evaluate(&definition(), &events, &policy(), false))
                .contains(&"acceptance_missing")
        );
        events[1].example_oracle = Some("oracle".into());
        events[1].criterion_sha256 = "changed criterion".into();
        assert!(
            reasons(&evaluate(&definition(), &events, &policy(), false))
                .contains(&"acceptance_missing")
        );
    }
    #[test]
    fn check_gate_a_valid_run_at_the_exact_commit_remains_sufficient() {
        let mut events = passing();
        events.push(observation(CheckPhase::Acceptance, CheckOutcome::Unknown));
        let mut dirty = observation(CheckPhase::Acceptance, CheckOutcome::Passed);
        dirty.tree_state = CheckTreeState::Dirty;
        events.push(dirty);
        assert!(!evaluate(&definition(), &events, &policy(), false).would_refuse);
    }

    #[test]
    fn check_gate_baseline_failure_is_specific_to_example_and_oracle() {
        let mut events = passing();
        events[0].outcome = CheckOutcome::Passed;
        assert!(
            reasons(&evaluate(&definition(), &events, &policy(), false))
                .contains(&"baseline_passed")
        );
        for outcome in [CheckOutcome::Errored, CheckOutcome::Unknown] {
            events[0].outcome = outcome;
            assert!(
                reasons(&evaluate(&definition(), &events, &policy(), false))
                    .contains(&"baseline_missing")
            );
        }
        events[0].outcome = CheckOutcome::Failed;
        events[0].example = "other-example".into();
        assert!(
            reasons(&evaluate(&definition(), &events, &policy(), false))
                .contains(&"baseline_missing")
        );
        events[0].example = "retains-draft".into();
        events[0].example_oracle = Some("other-oracle".into());
        assert!(
            reasons(&evaluate(&definition(), &events, &policy(), false))
                .contains(&"baseline_missing")
        );
    }
    #[test]
    fn check_gate_unbound_and_absent_examples_are_explicit() {
        let mut d = definition();
        d.examples[0].check = None;
        assert_eq!(
            reasons(&evaluate(&d, &passing(), &policy(), false)),
            ["check_unbound"]
        );
        d.examples.clear();
        assert_eq!(
            reasons(&evaluate(&d, &passing(), &policy(), false)),
            ["check_unbound"]
        );
    }
    #[test]
    fn check_gate_deferred_discrimination_methods_remain_unmet() {
        for (method, reason) in [
            (DiscriminationMethod::DiffMutation, "mutation_missing"),
            (
                DiscriminationMethod::JustifiedAlternative,
                "alternative_unreviewed",
            ),
        ] {
            let mut d = definition();
            d.discrimination = Some(Discrimination {
                method,
                note: Some("A stated alternative".into()),
            });
            assert_eq!(
                reasons(&evaluate(&d, &passing(), &policy(), false)),
                [reason]
            );
        }
    }

    #[test]
    fn check_gate_mutation_requires_current_source_oracle_and_successful_report() {
        let mut d = definition();
        d.discrimination = Some(Discrimination {
            method: DiscriminationMethod::DiffMutation,
            note: None,
        });
        let mut events = vec![observation(CheckPhase::Acceptance, CheckOutcome::Passed)];
        let mut mutation = observation(CheckPhase::Mutation, CheckOutcome::Passed);
        mutation.mutants = Some(MutationCounts {
            killed: 2,
            ..Default::default()
        });
        mutation.survivors = Some(vec![]);
        events.push(mutation.clone());
        assert!(!evaluate(&d, &events, &policy(), false).would_refuse);
        for field in ["source", "oracle", "counts", "outcome", "tree"] {
            let mut changed = mutation.clone();
            match field {
                "source" => changed.source_sha = Some("e".repeat(40)),
                "oracle" => changed.example_oracle = Some("earlier-oracle".into()),
                "counts" => changed.mutants = None,
                "outcome" => changed.outcome = CheckOutcome::Errored,
                "tree" => changed.tree_state = CheckTreeState::Dirty,
                _ => unreachable!(),
            }
            events[1] = changed;
            assert!(
                evaluate(&d, &events, &policy(), false).would_refuse,
                "{field}"
            );
        }
        let mut failed = mutation.clone();
        failed.outcome = CheckOutcome::Failed;
        failed.mutants = Some(MutationCounts {
            survived: 1,
            timeout: 1,
            ..Default::default()
        });
        failed.survivors = Some(vec![MutationSurvivor {
            id: "m1".into(),
            file: "src/save.rs".into(),
            line: 42,
        }]);
        events[1] = failed;
        let gate = evaluate(&d, &events, &policy(), false);
        assert_eq!(reasons(&gate), ["mutants_survived", "mutation_timeout"]);
        assert_eq!(gate.unmet[0].survivors[0].line, 42);
        // A valid observation of the same source and oracle remains sufficient.
        events.push(mutation);
        assert!(!evaluate(&d, &events, &policy(), false).would_refuse);
    }

    #[test]
    fn check_legacy_mutation_observations_remain_readable_without_report_proof() {
        let mut legacy = observation(CheckPhase::Mutation, CheckOutcome::Passed);
        legacy.example_oracle = Some(format!("sha256:{}", "e".repeat(64)));
        legacy.validate().unwrap();
        let mut d = definition();
        d.discrimination = Some(Discrimination {
            method: DiscriminationMethod::DiffMutation,
            note: None,
        });
        legacy.example_oracle = Some("oracle".into());
        let events = [
            observation(CheckPhase::Acceptance, CheckOutcome::Passed),
            legacy,
        ];
        assert_eq!(
            reasons(&evaluate(&d, &events, &policy(), false)),
            ["mutation_failed"]
        );
    }

    #[test]
    fn check_gate_alternative_requires_note_and_distinct_proposer_and_reviewer() {
        let mut d = definition();
        d.discrimination = Some(Discrimination {
            method: DiscriminationMethod::JustifiedAlternative,
            note: Some("Review the injected storage failure".into()),
        });
        let mut admission = approved(Id::new());
        admission.proposer_session = Some(Id::new());
        let run = |d: &WorkDefinition, admission: &RecordedAdmission| {
            evaluate_check_gate(
                &CheckGateDefinition {
                    definition: Some(d),
                    tags: &[],
                    required_criteria: &["Retain the draft".into()],
                    oracles: &[("retains-draft".into(), Some("oracle".into()))],
                    oracle_fingerprint: Some("fingerprint"),
                    session: None,
                    check_waiver: None,
                },
                Some(admission),
                &[observation(CheckPhase::Acceptance, CheckOutcome::Passed)],
                &"b".repeat(40),
                &policy(),
            )
            .unwrap()
        };
        assert!(!run(&d, &admission).would_refuse);
        admission.proposer_session = admission.decider_session;
        assert_eq!(reasons(&run(&d, &admission)), ["alternative_unreviewed"]);
        admission.proposer_session = Some(Id::new());
        d.discrimination.as_mut().unwrap().note = None;
        assert_eq!(
            reasons(&run(&d, &admission)),
            ["discrimination_note_missing"]
        );
    }
    #[test]
    fn check_gate_separate_approver_uses_actual_completion_session() {
        let p = CheckGatePolicy {
            separate_approver: true,
            ..policy()
        };
        assert!(!evaluate(&definition(), &passing(), &p, false).would_refuse);
        assert_eq!(
            reasons(&evaluate(&definition(), &passing(), &p, true)),
            ["approver_is_implementer"]
        );
    }
    #[test]
    fn check_gate_scope_and_nonimplementation_work_have_no_obligations() {
        let p = CheckGatePolicy {
            scope: Some(AdmissionScope::Tags(vec!["other".into()])),
            ..policy()
        };
        assert!(!evaluate(&definition(), &[], &p, false).applies);
        for purpose in [
            WorkPurpose::Document,
            WorkPurpose::Research,
            WorkPurpose::Decision,
        ] {
            let mut d = definition();
            d.purpose = purpose;
            let gate = evaluate(&d, &[], &policy(), false);
            assert!(!gate.applies);
            assert!(!gate.would_refuse);
            assert!(gate.unmet.is_empty());
        }
    }
    #[test]
    fn check_gate_observe_keeps_unmet_reasons_and_off_is_inactive() {
        let gate = evaluate(
            &definition(),
            &[],
            &CheckGatePolicy {
                mode: CheckGateMode::Observe,
                ..policy()
            },
            false,
        );
        assert!(gate.would_refuse);
        assert_eq!(gate.mode, CheckGateMode::Observe);
        assert!(!evaluate(&definition(), &[], &CheckGatePolicy::default(), false).applies);
    }
    #[test]
    fn check_gate_missing_or_changed_admission_fingerprint_is_unmet() {
        let d = definition();
        let oracles = vec![("retains-draft".into(), Some("oracle".into()))];
        for fingerprint in [None, Some("changed")] {
            let gate = evaluate_check_gate(
                &CheckGateDefinition {
                    definition: Some(&d),
                    tags: &[],
                    required_criteria: &["Retain the draft".into()],
                    oracles: &oracles,
                    oracle_fingerprint: fingerprint,
                    session: None,
                    check_waiver: None,
                },
                Some(&approved(Id::new())),
                &passing(),
                &"b".repeat(40),
                &policy(),
            )
            .unwrap();
            assert!(reasons(&gate).contains(&"oracle_changed"));
        }
    }
    #[test]
    fn check_gate_unknown_tree_is_unmet_unless_explicitly_allowed() {
        let mut events = passing();
        events[1].tree_state = CheckTreeState::Unknown;
        assert!(
            reasons(&evaluate(&definition(), &events, &policy(), false))
                .contains(&"acceptance_dirty_tree")
        );
        assert!(
            !evaluate(
                &definition(),
                &events,
                &CheckGatePolicy {
                    require_clean_tree: false,
                    ..policy()
                },
                false
            )
            .would_refuse
        );
    }
}
