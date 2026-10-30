//! Governed admission through the real CLI: a claim, an execution registration and a start need a
//! current approved decision; the source's own status, a request, or a stale approval never
//! stands in for one; recovery paths stay open. Each test states one assertion.
use awr_core::Id;
use serde_json::{Value, json};
use sha2::Digest;
use std::{fs, path::PathBuf, process::Command};

const ACC: &str = "Saving failure keeps the user's input";
const OUTCOME: &str = "Users keep their input when saving fails";
const ALL: &str = "scope='all'\nindependent_review_tags=['security']\n";

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Project {
    fn new(policy: &str) -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-work-admission-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(
            p.0.join("mapping.toml"),
            format!("[project]\nname='Admission fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[sources.options.admission_policy]\nversion=1\n{policy}[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"),
        )
        .unwrap();
        fs::write(
            p.0.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        p.ledger(ACC, "planned");
        p.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        p
    }

    fn pin(&self) -> String {
        format!(
            "sha256:{:x}",
            sha2::Sha256::digest(fs::read(self.0.join("decisions/scope.md")).unwrap())
        )
    }

    fn intent(&self) -> Value {
        json!([{"role": "user_decision", "decision": "scope", "source_fingerprint": self.pin()}])
    }

    fn implementation(&self, criterion: &str) -> Value {
        json!({
            "version": 1, "purpose": "implementation", "tier": "required",
            "intent": self.intent(),
            "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft", "by": "agent-a"}],
            "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge", "by": "agent-a"},
            "responsibility": {"name": "storage-module", "kind": "delivery"},
            "deliverable": "Local save with a failure fallback",
            "evidence_method": "Targeted tests compared before and after the change",
            "failure_recovery": "A failed save keeps the previous data",
            "examples": [
                {"criterion": criterion, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
                {"criterion": criterion, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"}
            ],
            "discrimination": {"method": "fail_before_pass_after"}
        })
    }

    fn research(&self) -> Value {
        json!({
            "version": 1, "purpose": "research",
            "intent": self.intent(),
            "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "Maps the failure modes", "by": "agent-a"}],
            "granularity": {"judgment": "independent_result", "reason": "One answer a reviewer can judge", "by": "agent-a"},
            "deliverable": "A list of failure modes with their limits"
        })
    }

    /// W is governed implementation work, R research, S security-tagged implementation and N has
    /// no definition at all. `criterion` is W's acceptance text, so editing it moves the scope.
    fn ledger(&self, criterion: &str, status: &str) {
        let work = |id: &str, tags: Value, definition: Option<Value>| {
            let mut value = json!({"id": id, "title": format!("Work {id}"), "status": status, "goal": "G",
                "acceptance": [if id == "R" { "The failure modes are listed" } else { criterion }],
                "paths": ["crates/storage/src"], "tags": tags, "next_action": "Do the work"});
            if let Some(definition) = definition {
                value["definition"] = definition;
            }
            value
        };
        let ledger = json!({
            "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
            "work_items": [
                work("W", json!([]), Some(self.implementation(criterion))),
                work("R", json!([]), Some(self.research())),
                work("S", json!(["security"]), Some(self.implementation(criterion))),
                work("N", json!([]), None),
                {"id": "O", "title": "Outline work O", "status": "planned", "goal": "G",
                 "acceptance": ["The later work is done"], "paths": [], "tags": [], "next_action": "Wait for the trigger",
                 "definition": {"version": 1, "purpose": "implementation",
                     "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The later outcome", "by": "agent-a"}],
                     "outline": {"constraints": ["No new storage layer"], "trigger": "When W is completed"}}},
                {"id": "X", "title": "Finished work X", "status": "completed", "goal": "G",
                 "acceptance": ["It is done"], "paths": [], "tags": [], "next_action": "Nothing"},
                {"id": "T", "title": "Draft work T", "status": "draft", "goal": "G",
                 "acceptance": ["It is drafted"], "paths": [], "tags": [], "next_action": "Draft it"}
            ]
        });
        fs::write(
            self.0.join("work.yaml"),
            serde_json::to_vec_pretty(&ledger).unwrap(),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> (bool, Value) {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .unwrap();
        let text = if output.stdout.is_empty() {
            &output.stderr
        } else {
            &output.stdout
        };
        let value = serde_json::from_slice(text).unwrap_or_else(|_| {
            panic!(
                "{args:?}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.success(), value)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (success, value) = self.run(args);
        assert!(success, "{args:?}: {value}");
        value
    }

    fn reindex(&self) {
        self.ok(&["source", "reindex"]);
    }

    fn revision(&self) -> String {
        self.ok(&["status"])["project_revision"].to_string()
    }

    fn fingerprint(&self, work: &str) -> String {
        self.ok(&["work", "assess", work])["contract_fingerprint"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A session bound to no work: enough to propose or decide.
    fn reviewer(&self, agent: &str) -> String {
        let revision = self.revision();
        self.ok(&[
            "session",
            "start",
            "--agent",
            agent,
            "--provider",
            "test",
            "--model",
            "m",
            "--expected-revision",
            &revision,
        ])["session"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn claim(&self, work: &str, agent: &str) -> (bool, Value) {
        let revision = self.revision();
        self.run(&[
            "session",
            "start",
            "--work",
            work,
            "--agent",
            agent,
            "--provider",
            "test",
            "--model",
            "m",
            "--claim",
            "--expected-revision",
            &revision,
        ])
    }

    fn judgment() -> Value {
        json!({"contribution": "Advances the saving outcome", "necessity": "The draft is lost today",
               "alternatives": "Reuse of the existing buffer was checked", "deliverable": "A fallback path",
               "exit": "Stop when the fallback is verified", "responsibility": "storage-module"})
    }

    fn admit(
        &self,
        work: &str,
        session: &str,
        key: &str,
        decision: &str,
        extra: Value,
    ) -> (bool, Value) {
        let mut request = json!({"work": work, "session": session, "expected_revision": self.revision().parse::<u64>().unwrap(),
            "request_key": key, "decision": decision, "contract_fingerprint": self.fingerprint(work)});
        for (k, v) in extra.as_object().unwrap() {
            request[k] = v.clone();
        }
        let path = self.0.join(format!("admit-{key}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        self.run(&["work", "admit", "--input", path.to_str().unwrap()])
    }

    /// A decision attributed to a declaring agent instead of a session.
    fn admit_as_agent(
        &self,
        work: &str,
        agent: &str,
        key: &str,
        decision: &str,
        extra: Value,
    ) -> (bool, Value) {
        let mut request = json!({"work": work, "agent": agent, "expected_revision": self.revision().parse::<u64>().unwrap(),
            "request_key": key, "decision": decision, "contract_fingerprint": self.fingerprint(work)});
        for (k, v) in extra.as_object().unwrap() {
            request[k] = v.clone();
        }
        let path = self.0.join(format!("admit-{key}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        self.run(&["work", "admit", "--input", path.to_str().unwrap()])
    }

    fn approve(&self, work: &str, session: &str, key: &str) -> Value {
        let (success, value) = self.admit(
            work,
            session,
            key,
            "approved",
            json!({"judgment": Self::judgment()}),
        );
        assert!(success, "{value}");
        value
    }
}

fn gap_codes(report: &Value) -> Vec<String> {
    report["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["code"].as_str().unwrap().to_string())
        .collect()
}

fn reasons(error: &Value) -> Vec<String> {
    error["details"]["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("no admission diagnostics in {error}"))
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect()
}

fn refused(result: (bool, Value), code: &str) -> Value {
    let (success, value) = result;
    assert!(!success, "expected a refusal: {value}");
    assert_eq!(value["code"], "AdmissionRequired", "{value}");
    assert_eq!(value["details"]["reason"], code, "{value}");
    value
}

#[test]
fn governed_work_is_claimed_only_with_a_current_approved_decision() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    refused(p.claim("W", "agent-w"), "admission_missing");
    // The source calling the work ready or in progress never grants admission.
    p.ledger(ACC, "ready");
    p.reindex();
    refused(p.claim("W", "agent-w"), "admission_missing");
    p.ledger(ACC, "in_progress");
    p.reindex();
    refused(p.claim("W", "agent-w"), "admission_missing");
    p.ledger(ACC, "planned");
    p.reindex();

    let reviewer = p.reviewer("reviewer");
    // A request is recorded and still is not a decision.
    let proposed = p.admit(
        "W",
        &reviewer,
        "propose-1",
        "proposed",
        json!({"judgment": Project::judgment()}),
    );
    assert!(proposed.0, "{}", proposed.1);
    refused(p.claim("W", "agent-w"), "admission_pending");

    p.approve("W", &reviewer, "approve-1");
    let (claimed, value) = p.claim("W", "agent-w");
    assert!(claimed, "{value}");
    assert_eq!(value["claim"]["status"], "active");
}

#[test]
fn an_approval_goes_stale_when_the_scope_it_judged_moves() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    p.approve("W", &reviewer, "approve-1");
    // Changing the acceptance text changes the contract (and the examples that name it).
    p.ledger("Saving failure keeps all of the user's input", "planned");
    p.reindex();
    let error = refused(p.claim("W", "agent-w"), "admission_stale");
    assert!(reasons(&error).contains(&"admission_stale".to_string()));
    let detail = error["details"]["diagnostics"][0]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(detail.contains("contract"), "{detail}");
    // Deciding again on the current scope restores it.
    p.approve("W", &reviewer, "approve-2");
    assert!(p.claim("W", "agent-w").0);
}

#[test]
fn a_denial_blocks_with_its_own_reason_and_a_later_approval_replaces_it() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    let (denied, value) = p.admit(
        "W",
        &reviewer,
        "deny-1",
        "denied",
        json!({"reason": "Not needed for this increment"}),
    );
    assert!(denied, "{value}");
    let error = refused(p.claim("W", "agent-w"), "admission_denied");
    assert!(
        error["details"]["diagnostics"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("Not needed for this increment")
    );
    p.approve("W", &reviewer, "approve-1");
    assert!(p.claim("W", "agent-w").0);
}

#[test]
fn observe_reports_the_missing_decision_without_blocking_anything() {
    let p = Project::new(&format!("mode='legacy_observe'\n{ALL}"));
    let shown = p.ok(&["work", "show", "W"]);
    assert_eq!(shown["work"]["admission"]["state"], "missing", "{shown}");
    assert_eq!(shown["work"]["admission"]["enforced"], false);
    let (claimed, value) = p.claim("W", "agent-w");
    assert!(claimed, "observe must not block: {value}");
}

#[test]
fn work_outside_the_tag_scope_and_projects_without_a_policy_are_untouched() {
    let p = Project::new("mode='governed_enforce'\nscope={tags=['governed']}\n");
    let shown = p.ok(&["work", "show", "W"]);
    assert!(shown["work"].get("admission").is_none(), "{shown}");
    let prepared = p.ok(&["work", "prepare", "W", "--budget", "12000"]);
    assert!(prepared.get("admission").is_none());
    assert!(
        p.claim("W", "agent-w").0,
        "ungoverned work is claimed as before"
    );
}

#[test]
fn work_that_needs_independent_review_cannot_be_approved_by_its_own_proposer() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let author = p.reviewer("author");
    let reviewer = p.reviewer("reviewer");
    // An inline approval has no distinct proposer.
    let inline = p.admit(
        "S",
        &author,
        "inline",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    refused(inline, "admission_review_not_independent");
    let (proposed, value) = p.admit(
        "S",
        &author,
        "propose",
        "proposed",
        json!({"judgment": Project::judgment()}),
    );
    assert!(proposed, "{value}");
    let proposal = value["event"]["id"].as_str().unwrap().to_string();
    // The proposer cannot answer their own proposal ...
    refused(
        p.admit(
            "S",
            &author,
            "self",
            "approved",
            json!({"proposal_event": proposal}),
        ),
        "admission_review_not_independent",
    );
    // ... another session can, and the program proves only that the sessions differ.
    let (approved, value) = p.admit(
        "S",
        &reviewer,
        "other",
        "approved",
        json!({"proposal_event": proposal}),
    );
    assert!(approved, "{value}");
    assert!(p.claim("S", "agent-s").0);
    // Work without a review tag may decide for itself.
    p.approve("W", &author, "w-inline");
    assert!(p.claim("W", "agent-w").0);
}

#[test]
fn entry_obligations_follow_the_declared_purpose() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    // Research may investigate an unknown: its purpose owes no examples or responsibility.
    p.approve("R", &reviewer, "research");
    assert!(p.claim("R", "agent-r").0);
    // Work without a definition is judged as implementation, so it cannot be approved.
    let (approved, value) = p.admit(
        "N",
        &reviewer,
        "plain",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(!approved);
    assert_eq!(
        value["details"]["reason"], "admission_obligations_unmet",
        "{value}"
    );
    assert!(
        value["details"]["diagnostics"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("definition_not_declared")
    );
    refused(p.claim("N", "agent-n"), "admission_missing");
}

#[test]
fn a_request_key_replays_the_original_outcome_and_never_a_different_request() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    let first = p.approve("W", &reviewer, "once");
    assert_eq!(first["already_recorded"], false);
    let replay = p.admit(
        "W",
        &reviewer,
        "once",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(replay.0, "{}", replay.1);
    assert_eq!(replay.1["already_recorded"], true);
    assert_eq!(
        replay.1["event"]["id"], first["event"]["id"],
        "one decision, not two"
    );
    // The same key with other content is refused instead of silently reused.
    let other = p.admit(
        "W",
        &reviewer,
        "once",
        "denied",
        json!({"reason": "Changed my mind"}),
    );
    assert!(!other.0);
    assert_eq!(other.1["code"], "SourceConflict", "{}", other.1);
    // Even when only the decision differs: a proposal and an inline approval carry the same judgment.
    let proposal = p.admit(
        "W",
        &reviewer,
        "once",
        "proposed",
        json!({"judgment": Project::judgment()}),
    );
    assert!(!proposal.0, "{}", proposal.1);
    assert_eq!(proposal.1["code"], "SourceConflict", "{}", proposal.1);
    let history = p.ok(&["work", "history", "W", "--event-type", "work.admission"]);
    assert_eq!(history["events"].as_array().unwrap().len(), 1);
}

#[test]
fn a_decision_reviewed_against_an_old_contract_is_rejected() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    let reviewed = p.fingerprint("W");
    p.ledger("Saving failure keeps all of the user's input", "planned");
    p.reindex();
    let (approved, value) = p.admit(
        "W",
        &reviewer,
        "late",
        "approved",
        json!({"judgment": Project::judgment(), "contract_fingerprint": reviewed}),
    );
    assert!(!approved);
    assert_eq!(value["code"], "SourceConflict", "{value}");
}

#[test]
fn prepare_reports_the_admission_state_and_points_at_the_decision() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let prepared = p.ok(&["work", "prepare", "W", "--budget", "12000"]);
    assert_eq!(prepared["admission"]["state"], "missing", "{prepared}");
    assert_eq!(prepared["next_action"], "resolve_admission_before_claiming");
    assert_eq!(prepared["admission"]["facts"]["admitted"], false);
    // Finished work cannot be claimed, so it owes no admission reason.
    let done = p.ok(&["work", "show", "X"]);
    let codes: Vec<&str> = done["work"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(
        codes.iter().all(|code| !code.starts_with("admission_")),
        "{codes:?}"
    );
    // A draft has its own blocker besides the missing decision, so the guidance stays the general one.
    let draft = p.ok(&["work", "prepare", "T", "--budget", "12000"]);
    assert_eq!(draft["admission"]["state"], "missing", "{draft}");
    assert_eq!(draft["next_action"], "inspect_readiness_and_current_claim");
    let reviewer = p.reviewer("reviewer");
    p.approve("W", &reviewer, "approve-1");
    let prepared = p.ok(&["work", "prepare", "W", "--budget", "12000"]);
    assert_eq!(prepared["admission"]["state"], "approved");
    assert_eq!(prepared["admission"]["facts"]["admitted"], true);
    assert_eq!(prepared["admission"]["facts"]["completed"], false);
    assert_ne!(prepared["next_action"], "resolve_admission_before_claiming");
}

#[test]
fn recovery_paths_stay_open_after_admission_lapses() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    p.approve("W", &reviewer, "approve-1");
    let (claimed, started) = p.claim("W", "agent-w");
    assert!(claimed, "{started}");
    let session = started["session"]["id"].as_str().unwrap().to_string();
    let claim = started["claim"]["id"].as_str().unwrap().to_string();
    // The scope moves, so the approval is stale. Reading, releasing and ending still work ...
    p.ledger("Saving failure keeps all of the user's input", "planned");
    p.reindex();
    assert!(p.run(&["work", "show", "W"]).0);
    let revision = p.revision();
    let (released, value) = p.run(&[
        "work",
        "release",
        "W",
        "--session",
        &session,
        "--claim",
        &claim,
        "--expected-revision",
        &revision,
    ]);
    assert!(released, "{value}");
    let revision = p.revision();
    let (ended, value) = p.run(&[
        "session",
        "end",
        "--session",
        &session,
        "--expected-revision",
        &revision,
    ]);
    assert!(ended, "{value}");
    // ... and only a new claim needs the decision again.
    refused(p.claim("W", "agent-other"), "admission_stale");
}

#[test]
fn a_resumed_session_inherits_a_claim_only_while_the_admission_is_current() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    p.approve("W", &reviewer, "approve-1");
    let (claimed, started) = p.claim("W", "agent-w");
    assert!(claimed, "{started}");
    let session = started["session"]["id"].as_str().unwrap().to_string();
    p.ledger("Saving failure keeps all of the user's input", "planned");
    p.reindex();
    let resume = |extra: &[&str]| {
        let revision = p.revision();
        let mut args = vec![
            "session",
            "resume",
            "--from-session",
            &session,
            "--agent",
            "agent-b",
            "--provider",
            "test",
            "--model",
            "m",
            "--expected-revision",
            &revision,
        ];
        args.extend_from_slice(extra);
        p.run(&args)
    };
    // Inheriting the predecessor's claim would hand over a claim the admission no longer backs ...
    refused(resume(&[]), "admission_stale");
    // ... while resuming to recover, without a claim, stays possible.
    let (resumed, value) = resume(&["--no-claim"]);
    assert!(resumed, "{value}");
    assert!(value["claim"].is_null(), "{value}");
}

#[test]
fn only_the_latest_proposal_can_be_answered_and_only_while_it_describes_the_current_scope() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let author = p.reviewer("author");
    let reviewer = p.reviewer("reviewer");
    let propose = |key: &str| {
        let (ok, value) = p.admit(
            "S",
            &author,
            key,
            "proposed",
            json!({"judgment": Project::judgment()}),
        );
        assert!(ok, "{value}");
        value["event"]["id"].as_str().unwrap().to_string()
    };
    let first = propose("p1");
    let second = propose("p2");
    // A newer proposal exists, so the older one is no longer what the decider should be answering.
    let (answered, value) = p.admit(
        "S",
        &reviewer,
        "old",
        "approved",
        json!({"proposal_event": first}),
    );
    assert!(!answered);
    assert_eq!(value["code"], "SourceConflict", "{value}");
    // The scope moves after the latest proposal: answering it is refused as stale, not silently approved.
    p.ledger("Saving failure keeps all of the user's input", "planned");
    p.reindex();
    refused(
        p.admit(
            "S",
            &reviewer,
            "late",
            "approved",
            json!({"proposal_event": second}),
        ),
        "admission_stale",
    );
    assert!(p.claim("S", "agent-s").0 == false, "nothing was admitted");
}

#[test]
fn a_markdown_ledger_cannot_declare_a_policy_because_definitions_are_yaml_only() {
    let root = std::env::temp_dir().join(format!("awr-work-admission-md-{}", Id::new()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("PLAN.md"), "# Plan\n").unwrap();
    fs::write(
        root.join("mapping.toml"),
        "[project]\nname='Markdown fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='PLAN.md'\nadapter='markdown-ledger-v1'\n[sources.options.admission_policy]\nversion=1\nmode='legacy_observe'\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .args([
            "--project",
            root.to_str().unwrap(),
            "--json",
            "init",
            "--manifest",
            "mapping.toml",
            "--accept",
        ])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&root);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("YAML ledger"), "{text}");
}

#[test]
fn a_handoff_transfers_a_claim_only_while_the_admission_is_current() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    p.approve("W", &reviewer, "approve-1");
    let (claimed, started) = p.claim("W", "agent-w");
    assert!(claimed, "{started}");
    let sender = started["session"]["id"].as_str().unwrap().to_string();
    // The receiver is bound to the work without a claim, which needs no admission.
    let revision = p.revision();
    let receiver = p.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "agent-r",
        "--provider",
        "test",
        "--model",
        "m",
        "--expected-revision",
        &revision,
    ])["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let prepared = p.ok(&["work", "prepare", "W", "--budget", "12000"]);
    let hash = prepared["context"]["work_context"]["context_hash"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = p.revision();
    p.ok(&[
        "session",
        "checkpoint",
        "--session",
        &sender,
        "--work",
        "W",
        "--context-hash",
        &hash,
        "--digest",
        "Half done",
        "--next-action",
        "Finish the fallback",
        "--expected-project-revision",
        &revision,
    ]);
    p.ledger("Saving failure keeps all of the user's input", "planned");
    p.reindex();
    let handoff = |to: &[&str]| {
        let revision = p.revision();
        let mut args = vec![
            "work",
            "handoff",
            "W",
            "--session",
            &sender,
            "--expected-revision",
            &revision,
        ];
        args.extend_from_slice(to);
        p.run(&args)
    };
    // The transfer would give the receiver a claim that no current decision backs ...
    refused(handoff(&["--to-session", &receiver]), "admission_stale");
    // ... while handing the work off unassigned releases it and keeps the checkpoint.
    let (released, value) = handoff(&[]);
    assert!(released, "{value}");
}

#[test]
fn changing_only_a_reviewed_example_makes_the_approval_stale_and_names_the_examples() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let reviewer = p.reviewer("reviewer");
    p.approve("W", &reviewer, "approve-1");
    // The criterion, the goal and the policy stay as they were; one example's expected result moves.
    let path = p.0.join("work.yaml");
    let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let work = ledger["work_items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|w| w["id"] == "W")
        .unwrap();
    work["definition"]["examples"][0]["then"] =
        json!("the input is still shown and the failure is reported");
    fs::write(&path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
    p.reindex();
    let error = refused(p.claim("W", "agent-w"), "admission_stale");
    let detail = error["details"]["diagnostics"][0]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(detail.contains("examples"), "{detail}");
    // A fresh judgment on the changed examples admits the work again.
    p.approve("W", &reviewer, "approve-2");
    assert!(p.claim("W", "agent-w").0);
}

#[test]
fn a_declared_agent_can_decide_for_work_that_needs_no_independent_review() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    // No session is needed: the decision is attributed to the declared agent, like a checkpoint.
    let (approved, value) = p.admit_as_agent(
        "W",
        "claude-run",
        "agent-1",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(approved, "{value}");
    assert_eq!(value["event"]["payload"]["decider_agent"], "claude-run");
    assert!(value["event"]["session_id"].is_null(), "{value}");
    assert!(p.claim("W", "agent-w").0);
    // A replay with the same agent returns the original receipt.
    let replay = p.admit_as_agent(
        "W",
        "claude-run",
        "agent-1",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert_eq!(replay.1["already_recorded"], true, "{}", replay.1);
    // Exactly one of a session or an agent names who decides.
    let both = p.admit_as_agent(
        "W",
        "claude-run",
        "both",
        "approved",
        json!({"judgment": Project::judgment(), "session": Id::new()}),
    );
    assert!(!both.0);
    assert_eq!(both.1["code"], "InvalidInput", "{}", both.1);
}

#[test]
fn work_that_needs_independent_review_is_never_proposed_or_approved_by_an_agent_alone() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    // Only sessions can be shown to differ, so a declared agent cannot start or finish the review.
    refused(
        p.admit_as_agent(
            "S",
            "claude-run",
            "p-agent",
            "proposed",
            json!({"judgment": Project::judgment()}),
        ),
        "admission_review_not_independent",
    );
    refused(
        p.admit_as_agent(
            "S",
            "claude-run",
            "a-agent",
            "approved",
            json!({"judgment": Project::judgment()}),
        ),
        "admission_review_not_independent",
    );
    let author = p.reviewer("author");
    let (proposed, value) = p.admit(
        "S",
        &author,
        "propose",
        "proposed",
        json!({"judgment": Project::judgment()}),
    );
    assert!(proposed, "{value}");
    let proposal = value["event"]["id"].as_str().unwrap().to_string();
    refused(
        p.admit_as_agent(
            "S",
            "claude-run",
            "answer-agent",
            "approved",
            json!({"proposal_event": proposal}),
        ),
        "admission_review_not_independent",
    );
}

#[test]
fn an_agent_declared_decision_applies_to_the_current_branch_and_only_to_it() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let revision = p.revision();
    let created = p.ok(&[
        "branch",
        "create",
        "feature",
        "--actor",
        "claude-run",
        "--reason",
        "Try the change",
        "--expected-revision",
        &revision,
    ]);
    let branch = created["branch"]["id"].as_str().unwrap().to_string();
    let revision = p.revision();
    p.ok(&[
        "branch",
        "switch",
        "feature",
        "--actor",
        "claude-run",
        "--reason",
        "Work there",
        "--expected-revision",
        &revision,
    ]);
    let (approved, value) = p.admit_as_agent(
        "W",
        "claude-run",
        "on-feature",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(approved, "{value}");
    assert_eq!(value["event"]["branch_id"], json!(branch), "{value}");
    // The decision is read on that branch; the baseline branch has none of its own.
    let on_branch = p.ok(&["work", "show", "W", "--branch", "feature"]);
    assert_eq!(
        on_branch["work"]["admission"]["state"], "approved",
        "{on_branch}"
    );
    let on_main = p.ok(&["work", "show", "W", "--branch", "main"]);
    assert_eq!(
        on_main["work"]["admission"]["state"], "missing",
        "{on_main}"
    );
}

#[test]
fn outline_work_is_kept_with_its_trigger_but_never_admitted_until_it_is_elaborated() {
    let p = Project::new(&format!("mode='governed_enforce'\n{ALL}"));
    let shown = p.ok(&["work", "show", "O"]);
    let codes = gap_codes(&shown["work"]["definition"]);
    assert_eq!(codes, ["outline_not_elaborated"], "{shown}");
    // The outline cannot be approved, whoever asks, and says why.
    let (approved, value) = p.admit_as_agent(
        "O",
        "claude-run",
        "outline",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(!approved);
    assert_eq!(
        value["details"]["reason"], "admission_obligations_unmet",
        "{value}"
    );
    assert!(
        value["details"]["diagnostics"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("outline_not_elaborated")
    );
    refused(p.claim("O", "agent-o"), "admission_missing");
}
