//! Check contracts and pure oracle hashes. Execution and persistence live in their own crates.
use crate::{Error, Id, Result, Revision};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CHECK_EVENT: &str = "work.check";
pub const CHECK_PAYLOAD_CAP: usize = 16 * 1024;
pub const CHECK_FILE_CAP: u64 = 4 * 1024 * 1024;

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
    pub mutants: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub survivors: Option<Vec<String>>,
}
impl CheckPayload {
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
#[serde(default, deny_unknown_fields)]
pub struct CheckGatePolicy {
    pub mode: CheckGateMode,
    pub scope: Option<crate::AdmissionScope>,
    pub small_work: SmallWorkChecks,
    pub require_clean_tree: bool,
    pub separate_approver: bool,
}
impl Default for CheckGatePolicy {
    fn default() -> Self {
        Self {
            mode: CheckGateMode::Off,
            scope: None,
            small_work: SmallWorkChecks::Reason,
            require_clean_tree: true,
            separate_approver: false,
        }
    }
}
impl CheckGatePolicy {
    pub fn validate(&self) -> Result<()> {
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
    let mut unmet = |example: Option<&str>, criterion: Option<&str>, reason: &str| {
        result.unmet.push(UnmetCheck {
            example: example.map(String::from),
            criterion: criterion.map(String::from),
            reason: reason.into(),
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
                    unmet(id, Some(criterion), "mutation_missing")
                }
                Some(DiscriminationMethod::JustifiedAlternative) => {
                    unmet(id, Some(criterion), "alternative_unreviewed")
                }
                _ => {
                    let baselines: Vec<_> = check_events
                        .iter()
                        .filter(matches)
                        .filter(|e| e.phase == CheckPhase::Baseline)
                        .collect();
                    if !baselines.iter().any(|e| e.outcome == CheckOutcome::Failed) {
                        unmet(
                            id,
                            Some(criterion),
                            if baselines.iter().any(|e| e.outcome == CheckOutcome::Passed) {
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
    result.would_refuse = !result.unmet.is_empty();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
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
