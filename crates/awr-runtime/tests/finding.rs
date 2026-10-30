//! Findings against a real indexed project: severity follows from the adopted acceptance tiers and
//! the hard rules of the project and from what a finding shows, only blocking findings hold the
//! completion of the work, deferred records block nothing, and a review round that sends work
//! back counts against its rework budget.
use awr_core::*;
use awr_runtime::{admit_work, finding_work};
use awr_source::{Manifest, index_project};
use awr_store::Store;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    fs,
    path::{Path, PathBuf},
};

const ACC: &str = "Saving failure keeps the user's input";
const OTHER_ACC: &str = "Saving reports its progress";
const OUTCOME: &str = "Users keep their input when saving fails";

struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn definition(pin: &str, tier: &str, size: Option<&str>) -> Value {
    let mut definition = json!({
        "version": 1, "purpose": "implementation", "tier": tier,
        "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": pin}],
        "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft", "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge", "by": "agent-a"},
        "responsibility": {"name": "storage-module", "kind": "delivery"},
        "deliverable": "Local save with a failure fallback",
        "evidence_method": "Targeted tests compared before and after the change",
        "failure_recovery": "A failed save keeps the previous data",
        "examples": [
            {"criterion": ACC, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
            {"criterion": ACC, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"},
            {"criterion": OTHER_ACC, "kind": "positive", "given": "a slow disk", "when": "the user saves", "then": "progress is shown"},
            {"criterion": OTHER_ACC, "kind": "negative", "given": "a slow disk", "when": "the user saves", "then": "no progress is ever shown"}
        ],
        "discrimination": {"method": "fail_before_pass_after"}
    });
    if let Some(size) = size {
        definition["size"] = json!(size);
        // A tight rework budget: one return is allowed, a second spends it.
        definition["budget"] = json!({"rework_cycles": 1});
    }
    definition
}

fn pin(root: &Path) -> String {
    format!(
        "sha256:{:x}",
        sha2::Sha256::digest(fs::read(root.join("decisions/scope.md")).unwrap())
    )
}

fn write_ledger(root: &Path, size: Option<&str>) {
    let work = |key: &str, tier: &str, size: Option<&str>| {
        json!({"id": key, "title": format!("Save safely {key}"), "status": "planned", "goal": "G",
            "acceptance": [ACC, OTHER_ACC], "paths": ["crates/storage/src"], "next_action": "Do the work",
            "definition": definition(&pin(root), tier, size)})
    };
    let ledger = json!({
        "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
        "work_items": [work("W", "required", size), work("V", "later", None)]
    });
    fs::write(
        root.join("work.yaml"),
        serde_json::to_vec_pretty(&ledger).unwrap(),
    )
    .unwrap();
}

fn code(error: Error) -> String {
    match error {
        Error::Admission { code, .. } => code.to_string(),
        other => panic!("expected an admission refusal, got {other:?}"),
    }
}

impl Fixture {
    fn new() -> Self {
        Self::sized(None)
    }

    fn sized(size: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!("awr-finding-runtime-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::create_dir_all(root.join("decisions")).unwrap();
        fs::write(
            root.join(".awr/project.toml"),
            "[project]\nname = \"Finding runtime fixture\"\nexternal_key = \"finding-runtime-fixture\"\n\n[[sources]]\ndomain = \"ledger\"\nrole = \"primary\"\npath = \"work.yaml\"\nadapter = \"yaml-ledger-v1\"\n\n[sources.options.admission_policy]\nversion = 1\nmode = \"governed_enforce\"\nscope = \"all\"\n\n[[sources]]\ndomain = \"rules\"\nrole = \"primary\"\npath = \"rules.md\"\nadapter = \"markdown-rules-v1\"\n\n[sources.options]\nkey_prefix = \"rules\"\n\n[[sources]]\ndomain = \"decisions\"\nrole = \"supporting\"\npath = \"decisions\"\nadapter = \"markdown-directory-v1\"\n",
        )
        .unwrap();
        fs::write(
            root.join("rules.md"),
            "# Data integrity {#data-integrity severity=hard scope=project value=*}\n\nNever lose the only copy of the user's data.\n\n# Naming {#naming severity=soft scope=project value=*}\n\nPrefer short names for helpers.\n",
        )
        .unwrap();
        fs::write(
            root.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        write_ledger(&root, size);
        let root = root.canonicalize().unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let indexed =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(indexed.ok, "{indexed:?}");
        Self {
            root,
            store,
            project: indexed.project_id,
        }
    }

    fn revision(&self) -> u64 {
        self.store.project(self.project).unwrap().project_revision
    }

    fn session(&mut self, agent: &str, work: Option<&str>) -> Id {
        let revision = self.revision();
        self.store
            .start_session(
                self.project,
                revision,
                SessionDraft {
                    work_item_key: work.map(String::from),
                    agent_id: agent.into(),
                    provider: "test".into(),
                    model: "m".into(),
                    branch_id: None,
                    claim: false,
                    claim_ttl_ms: None,
                },
            )
            .unwrap()
            .0
            .session
            .id
    }

    fn admit(&mut self, work: &str, reviewer: Id) {
        let request = AdmissionRequest {
            work: work.into(),
            session: Some(reviewer),
            agent: None,
            expected_revision: self.revision(),
            request_key: format!("admit-{work}"),
            decision: AdmissionDecision::Approved,
            contract_fingerprint: self
                .store
                .work_contract(self.project, work)
                .unwrap()
                .fingerprint,
            judgment: Some(AdmissionJudgment {
                contribution: "Advances the saving outcome".into(),
                necessity: "The draft is lost today".into(),
                alternatives: "Reuse of the existing buffer was checked".into(),
                deliverable: "A fallback path".into(),
                exit: "Stop when the fallback is verified".into(),
                responsibility: "storage-module".into(),
            }),
            proposal_event: None,
            reason: None,
        };
        admit_work(&mut self.store, &self.root.clone(), &request).unwrap();
    }

    fn request(
        &self,
        work: &str,
        key: &str,
        finding: &str,
        action: FindingAction,
    ) -> FindingRequest {
        FindingRequest {
            work: work.into(),
            session: None,
            agent: Some("reviewer".into()),
            expected_revision: self.revision(),
            request_key: format!("{key}-{action:?}"),
            finding_key: finding.into(),
            action,
            round: None,
            summary: None,
            criterion: None,
            rule: None,
            counterexample: None,
            risk: None,
            resolution: None,
        }
    }

    fn record(
        &mut self,
        work: &str,
        finding: &str,
        round: u32,
        edit: impl FnOnce(&mut FindingRequest),
    ) -> Result<Value> {
        let mut request = self.request(work, finding, finding, FindingAction::Record);
        request.round = Some(round);
        request.summary = Some("Saving offline loses the draft".into());
        edit(&mut request);
        finding_work(&mut self.store, &self.root.clone(), &request)
    }

    fn resolve(&mut self, work: &str, finding: &str) -> Result<Value> {
        let mut request = self.request(work, finding, finding, FindingAction::Resolve);
        request.resolution = Some("The fallback keeps the draft".into());
        finding_work(&mut self.store, &self.root.clone(), &request)
    }

    fn status(&self, work: &str) -> FindingStatus {
        self.store.finding_status(self.project, work, None).unwrap()
    }

    fn severity(&self, work: &str, finding: &str) -> String {
        self.status(work)
            .findings
            .iter()
            .find(|f| f.key == finding)
            .unwrap_or_else(|| panic!("no finding {finding}"))
            .severity
            .clone()
    }
}

fn example() -> Counterexample {
    Counterexample {
        given: "the storage is read-only".into(),
        when: "the user saves".into(),
        then: "the draft is gone".into(),
    }
}

#[test]
fn severity_follows_the_adopted_tier_the_hard_rules_and_what_the_finding_shows() {
    let mut f = Fixture::new();
    // A required criterion with a counterexample blocks; without one it is deferred.
    f.record("W", "f1", 1, |r| {
        r.criterion = Some(ACC.into());
        r.counterexample = Some(example());
    })
    .unwrap();
    f.record("W", "f2", 1, |r| r.criterion = Some(ACC.into()))
        .unwrap();
    // A hard rule with a counterexample blocks; a soft rule does not.
    f.record("W", "f3", 1, |r| {
        r.rule = Some("rules#data-integrity".into());
        r.counterexample = Some(example());
    })
    .unwrap();
    f.record("W", "f4", 1, |r| {
        r.rule = Some("rules#naming".into());
        r.counterexample = Some(example());
    })
    .unwrap();
    // A second-round matter of style is deferred.
    f.record("W", "f5", 2, |r| {
        r.summary = Some("Prefer shorter helper names".into())
    })
    .unwrap();
    // A stated data-loss risk blocks without a criterion, a rule or a counterexample.
    f.record("W", "f6", 1, |r| {
        r.risk = Some(Risk {
            kind: RiskKind::DataLoss,
            reasoning: "A failed save deletes the only copy of the draft".into(),
        })
    })
    .unwrap();
    let blocking = ["f1", "f3", "f6"];
    for key in ["f1", "f2", "f3", "f4", "f5", "f6"] {
        let expected = if blocking.contains(&key) {
            "blocking"
        } else {
            "non_blocking"
        };
        assert_eq!(f.severity("W", key), expected, "{key}");
    }
    let status = f.status("W");
    assert_eq!(status.open_blocking, ["f1", "f3", "f6"]);
    assert_eq!(status.open_deferred, ["f2", "f4", "f5"]);
    // The deferred ones are linked records: they keep what they were measured against.
    let deferred = status.findings.iter().find(|x| x.key == "f4").unwrap();
    assert_eq!(
        (deferred.disposition, deferred.rule.as_deref()),
        ("deferred", Some("rules#naming"))
    );
    // Against a work whose adopted tier is later, even a counterexample does not block.
    f.record("V", "later", 1, |r| {
        r.criterion = Some(ACC.into());
        r.counterexample = Some(example());
    })
    .unwrap();
    let later = f.status("V").findings.remove(0);
    assert_eq!(later.severity, "non_blocking");
    assert!(later.basis.contains("later-tier"), "{}", later.basis);
    // What a finding names has to exist.
    let error = f
        .record("W", "ghost-criterion", 1, |r| {
            r.criterion = Some("Saving is fast".into())
        })
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    let error = f
        .record("W", "ghost-rule", 1, |r| {
            r.rule = Some("no-such-rule".into())
        })
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
}

#[test]
fn only_a_blocking_finding_holds_completion_and_deferred_records_block_nothing() {
    let mut f = Fixture::new();
    f.check_completion("W").unwrap();
    f.record("W", "style", 2, |_| {}).unwrap();
    f.record("W", "weak", 1, |r| r.criterion = Some(ACC.into()))
        .unwrap();
    f.check_completion("W")
        .expect("deferred records never hold completion");
    f.record("W", "blocker-1", 1, |r| {
        r.criterion = Some(ACC.into());
        r.counterexample = Some(example());
    })
    .unwrap();
    f.record("W", "blocker-2", 1, |r| {
        r.risk = Some(Risk {
            kind: RiskKind::Security,
            reasoning: "The draft is readable by other users".into(),
        })
    })
    .unwrap();
    let error = f.check_completion("W").unwrap_err();
    assert!(
        matches!(&error, Error::DependencyBlocked(why) if why.contains("blocker-1") && why.contains("blocker-2")),
        "{error:?}"
    );
    // Work that nobody found fault with is not held by the fault found in another.
    f.check_completion("V").unwrap();
    // Closing the blocking ones releases it, and the deferred ones stay open and listed.
    f.resolve("W", "blocker-1").unwrap();
    assert!(f.check_completion("W").is_err());
    let receipt = f.resolve("W", "blocker-2").unwrap();
    assert_eq!(receipt["findings"]["open_blocking"], json!([]));
    f.check_completion("W").unwrap();
    assert_eq!(f.status("W").open_deferred, ["style", "weak"]);
}

impl Fixture {
    fn check_completion(&self, work: &str) -> Result<()> {
        self.store
            .check_completion_findings(self.project, work, None)
    }
}

#[test]
fn review_rounds_that_send_work_back_count_against_its_rework_budget_once_each() {
    let mut f = Fixture::sized(Some("small"));
    let reviewer = f.session("reviewer", None);
    f.admit("W", reviewer);
    let used = |f: &Fixture| {
        f.store
            .reassessment_status(f.project, "W", None)
            .unwrap()
            .budget
            .expect("the budget is reported")
    };
    // Deferred findings never send work back.
    f.record("W", "style", 1, |_| {}).unwrap();
    f.record("W", "weak", 1, |r| r.criterion = Some(ACC.into()))
        .unwrap();
    assert_eq!(used(&f).usage.rework.used, 0);
    // Several blocking findings of one round are one return, which a budget of one allows.
    for key in ["a", "b"] {
        f.record("W", key, 1, |r| {
            r.criterion = Some(ACC.into());
            r.counterexample = Some(example());
        })
        .unwrap();
    }
    let budget = used(&f);
    assert_eq!(
        (budget.usage.rework.used, budget.usage.rework.counted_from),
        (1, "recorded")
    );
    assert_eq!(budget.state, "within");
    // The next round is a second return, and the budget is spent.
    f.record("W", "c", 2, |r| {
        r.criterion = Some(OTHER_ACC.into());
        r.counterexample = Some(example());
    })
    .unwrap();
    let budget = used(&f);
    assert_eq!((budget.usage.rework.used, budget.state), (2, "required"));
    let reviewer_session = f.session("agent-w", Some("W"));
    let refused = f
        .store
        .register_execution(
            f.project,
            f.revision(),
            reviewer_session,
            ExecutionIntent {
                operation_key: "op-1".into(),
                purpose: "Run the storage checks".into(),
                executor: ExecutorKind::ManagedLocal,
                command: vec!["check".into()],
                cwd: f.root.to_string_lossy().into(),
                external_reference: None,
            },
        )
        .unwrap_err();
    assert_eq!(code(refused), "reassessment_budget_required");
}

#[test]
fn history_is_kept_requests_replay_and_a_finding_key_names_one_finding_for_good() {
    let mut f = Fixture::new();
    let first = f
        .record("W", "f1", 1, |r| {
            r.criterion = Some(ACC.into());
            r.counterexample = Some(example());
        })
        .unwrap();
    assert_eq!(first["already_recorded"], false);
    assert_eq!(first["completion_claimed"], false);
    assert_eq!(
        first["findings"]["findings"][0]["disposition"],
        "returns_work"
    );
    // The same request replays; another request under the key does not.
    let mut again = f.request("W", "f1", "f1", FindingAction::Record);
    again.round = Some(1);
    again.summary = Some("Saving offline loses the draft".into());
    again.criterion = Some(ACC.into());
    again.counterexample = Some(example());
    let replay = finding_work(&mut f.store, &f.root.clone(), &again).unwrap();
    assert_eq!(replay["already_recorded"], true);
    assert_eq!(replay["event"]["id"], first["event"]["id"]);
    again.round = Some(2);
    let error = finding_work(&mut f.store, &f.root.clone(), &again).unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    // A used key cannot be recorded again, a resolved finding cannot be resolved again.
    let error = f
        .record("W", "f1", 1, |r| r.request_key = "other".into())
        .unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
    f.resolve("W", "f1").unwrap();
    let mut again = f.request("W", "late", "f1", FindingAction::Resolve);
    again.resolution = Some("late".into());
    let error = finding_work(&mut f.store, &f.root.clone(), &again).unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    let error = f.resolve("W", "ghost").unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    // The resolved finding stays listed with how it was dealt with.
    let status = f.status("W");
    assert_eq!(status.findings[0].state, "resolved");
    assert_eq!(
        status.findings[0].resolution.as_deref(),
        Some("The fallback keeps the draft")
    );
    assert_eq!(status.findings[0].resolved_by.as_deref(), Some("reviewer"));
    // A stale write is refused.
    let mut late = f.request("W", "stale", "f9", FindingAction::Record);
    late.round = Some(1);
    late.summary = Some("A matter of style".into());
    late.expected_revision -= 1;
    let error = finding_work(&mut f.store, &f.root.clone(), &late).unwrap_err();
    assert!(matches!(error, Error::RevisionConflict { .. }), "{error:?}");
}

#[test]
fn a_session_records_under_its_own_agent_only_while_it_is_active_and_only_governed_work_is_judged()
{
    let mut f = Fixture::new();
    let gone = f.session("agent-gone", None);
    let revision = f.revision();
    f.store
        .end_session(f.project, revision, gone, SessionOutcome::Ended)
        .unwrap();
    let mut by_session = f.request("W", "by-session", "f1", FindingAction::Record);
    by_session.agent = None;
    by_session.session = Some(gone);
    by_session.round = Some(1);
    by_session.summary = Some("A matter of style".into());
    let error = finding_work(&mut f.store, &f.root.clone(), &by_session).unwrap_err();
    assert!(matches!(error, Error::InvalidTransition(_)), "{error:?}");
    let active = f.session("agent-active", None);
    by_session.session = Some(active);
    by_session.expected_revision = f.revision();
    let receipt = finding_work(&mut f.store, &f.root.clone(), &by_session).unwrap();
    assert_eq!(receipt["event"]["payload"]["decider_agent"], "agent-active");
    assert_eq!(receipt["event"]["session_id"], json!(active));
    // Without a policy there is nothing to judge a finding against.
    let manifest = fs::read_to_string(f.root.join(".awr/project.toml")).unwrap();
    let plain = manifest.replace(
        "[sources.options.admission_policy]\nversion = 1\nmode = \"governed_enforce\"\nscope = \"all\"\n\n",
        "",
    );
    assert_ne!(manifest, plain);
    fs::write(f.root.join(".awr/project.toml"), plain).unwrap();
    let indexed = index_project(
        &mut f.store,
        &f.root,
        &Manifest::load(&f.root).unwrap(),
        false,
    )
    .unwrap();
    assert!(indexed.ok);
    let error = f.record("W", "f2", 1, |_| {}).unwrap_err();
    assert_eq!(code(error), "admission_not_governed");
    assert!(
        f.store
            .finding_block(f.project, "W", None)
            .unwrap()
            .is_none()
    );
}
