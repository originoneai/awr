//! The earliest material gap of governed work, told as one bounded instruction by `work prepare`
//! in its action view, through the real CLI.
use awr_core::Id;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

const ACC: &str = "Saving failure keeps the user's input";
const OUTCOME: &str = "Users keep their input when saving fails";

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Project {
    fn new(mode: &str) -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-work-guidance-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(
            p.0.join("mapping.toml"),
            format!("[project]\nname='Guidance fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[sources.options.admission_policy]\nversion=1\nmode='{mode}'\nscope='all'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"),
        )
        .unwrap();
        fs::write(
            p.0.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        let pin = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(fs::read(p.0.join("decisions/scope.md")).unwrap())
        );
        let work = |key: &str| {
            json!({"id": key, "title": format!("Save safely {key}"), "status": "planned", "goal": "G",
                "acceptance": [ACC], "paths": ["crates/storage/src"], "tags": [], "next_action": "Do the work",
                "definition": {
                    "version": 1, "purpose": "implementation", "tier": "required",
                    "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": pin}],
                    "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft", "by": "agent-a"}],
                    "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge", "by": "agent-a"},
                    "responsibility": {"name": "storage-module", "kind": "delivery"},
                    "deliverable": "Local save with a failure fallback",
                    "evidence_method": "Targeted tests compared before and after the change",
                    "failure_recovery": "A failed save keeps the previous data",
                    "examples": [
                        {"criterion": ACC, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
                        {"criterion": ACC, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"}
                    ],
                    "discrimination": {"method": "fail_before_pass_after"}}})
        };
        let ledger = json!({
            "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
            "work_items": [work("W"), work("V")]
        });
        fs::write(
            p.0.join("work.yaml"),
            serde_json::to_vec_pretty(&ledger).unwrap(),
        )
        .unwrap();
        p.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        p
    }

    fn run(&self, args: &[&str]) -> (bool, Value) {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .env("AWR_EXECUTION_TEST_ROOT", &self.0)
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

    fn revision(&self) -> u64 {
        self.ok(&["status"])["project_revision"].as_u64().unwrap()
    }

    /// A session bound to `work` that holds no claim: enough to run commands for it.
    fn session(&self, agent: &str, work: &str) -> String {
        let revision = self.revision().to_string();
        self.ok(&[
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
            "--expected-revision",
            &revision,
        ])["session"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Starts a managed run that fails: the AWR binary is given an option it does not have, so it
    /// exits with a usage error on every platform.
    fn launch(&self, session: &str, key: &str) -> (bool, Value) {
        self.run(&[
            "execution",
            "run",
            "--session",
            session,
            "--key",
            key,
            "--purpose",
            "Run the storage checks",
            "--",
            env!("CARGO_BIN_EXE_awr"),
            "--no-such-option",
        ])
    }

    /// A whole failed attempt: the run is launched and its failure recorded before this returns.
    fn fail(&self, session: &str, key: &str) -> String {
        let (launched, value) = self.launch(session, key);
        assert!(launched, "{value}");
        let id = value["execution"]["id"].as_str().unwrap().to_string();
        let start = Instant::now();
        loop {
            let shown = self.ok(&["execution", "show", &id])["execution"].clone();
            if shown["state"] == "failed" {
                return id;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "execution timed out: {shown}"
            );
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn block(&self, work: &str) -> Value {
        self.ok(&["work", "show", work])["work"]["reassessment"].clone()
    }

    fn reassess(&self, key: &str, decision: &str, extra: Value) -> (bool, Value) {
        let fingerprint = self.block("W")["current"]["facts_fingerprint"].clone();
        let mut request = json!({"work": "W", "agent": "lead", "expected_revision": self.revision(),
            "request_key": key, "decision": decision, "facts_fingerprint": fingerprint,
            "judgment": {"reason": "The failures came from the read-only fixture",
                         "next_step": "Run the same check on a writable fixture",
                         "evidence_considered": "The output of the failed runs"}});
        for (k, v) in extra.as_object().unwrap() {
            request[k] = v.clone();
        }
        let path = self.0.join(format!("reassess-{key}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        self.run(&["work", "reassess", "--input", path.to_str().unwrap()])
    }
}

impl Project {}

impl Project {
    /// The one instruction `work prepare` gives in its action view.
    fn guidance(&self, work: &str) -> Value {
        let prepared = self.ok(&[
            "work",
            "prepare",
            work,
            "--budget",
            "12000",
            "--response-view",
            "action",
        ]);
        prepared["guidance"].clone()
    }

    fn decide(&self, key: &str, work: &str, decision: &str, extra: Value) -> (bool, Value) {
        let fingerprint = self.ok(&["work", "assess", work])["contract_fingerprint"].clone();
        let mut request = json!({"work": work, "agent": "lead", "expected_revision": self.revision(),
            "request_key": key, "decision": decision, "contract_fingerprint": fingerprint});
        for (k, v) in extra.as_object().unwrap() {
            request[k] = v.clone();
        }
        let path = self.0.join(format!("admit-{key}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        self.run(&["work", "admit", "--input", path.to_str().unwrap()])
    }

    fn judgment() -> Value {
        json!({"contribution": "Advances the saving outcome", "necessity": "The draft is lost today",
               "alternatives": "Reuse of the existing buffer was checked", "deliverable": "A fallback path",
               "exit": "Stop when the fallback is verified", "responsibility": "storage-module"})
    }

    fn finding(&self, key: &str, extra: Value) -> Value {
        let mut request = json!({"work": "W", "agent": "reviewer", "expected_revision": self.revision(),
            "request_key": key, "finding_key": key, "action": "record"});
        for (k, v) in extra.as_object().unwrap() {
            request[k] = v.clone();
        }
        let path = self.0.join(format!("finding-{key}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        self.ok(&["work", "finding", "--input", path.to_str().unwrap()])
    }
}

#[test]
fn each_gap_of_governed_work_is_one_instruction_and_unknown_denied_and_stale_differ() {
    let p = Project::new("governed_enforce");
    // Nothing is admitted yet: the instruction is to admit, in one call.
    let guidance = p.guidance("W");
    assert_eq!(
        guidance["when"], "Governed work has no admission decision",
        "{guidance}"
    );
    assert!(
        guidance["next_action"]
            .as_str()
            .unwrap()
            .contains("work admit")
    );
    assert!(serde_json::to_vec(&guidance).unwrap().len() <= 1024);
    // A denial is explained with its reason, apart from every other state.
    let (denied, value) = p.decide(
        "deny",
        "W",
        "denied",
        json!({"reason": "The examples restate the acceptance instead of testing it"}),
    );
    assert!(denied, "{value}");
    let denied = p.guidance("W");
    assert_eq!(
        denied["when"], "The latest decision denied admission",
        "{denied}"
    );
    assert!(
        denied["basis"]
            .as_str()
            .unwrap()
            .contains("restate the acceptance")
    );
    // An approval clears it: whatever instruction remains is no longer about admission.
    let (approved, value) = p.decide(
        "approve",
        "W",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(approved, "{value}");
    let after = p.guidance("W");
    assert!(
        !after["basis"].as_str().unwrap().starts_with("admission_"),
        "{after}"
    );
    // The scope moves: the approval is stale, which is neither a denial nor an unknown state.
    let path = p.0.join("work.yaml");
    let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    ledger["work_items"][0]["paths"] = json!(["crates/storage/src/more"]);
    fs::write(&path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
    p.ok(&["source", "reindex"]);
    let stale = p.guidance("W");
    assert_eq!(
        stale["when"], "An approved admission no longer matches the work",
        "{stale}"
    );
    assert!(
        stale["next_action"]
            .as_str()
            .unwrap()
            .contains("supervisor")
    );
    assert_ne!(stale["when"], denied["when"]);
    // The other work is untouched by all of it.
    assert_eq!(
        p.guidance("V")["when"],
        "Governed work has no admission decision"
    );
}

#[test]
fn a_work_with_nothing_governed_to_do_gets_the_ordinary_instruction() {
    let p = Project::new("legacy_observe");
    // Observing holds nothing, so there is no governance instruction either.
    let guidance = p.guidance("W");
    assert_ne!(
        guidance["when"], "Governed work has no admission decision",
        "{guidance}"
    );
}

#[test]
fn repeated_failures_and_blocking_findings_are_told_as_their_own_instructions() {
    let p = Project::new("governed_enforce");
    let (approved, value) = p.decide(
        "approve",
        "W",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(approved, "{value}");
    let session = p.session("agent-w", "W");
    for n in 1..=3 {
        p.fail(&session, &format!("check-{n}"));
    }
    let guidance = p.guidance("W");
    assert_eq!(
        guidance["when"],
        "The records of this work show a method failing again or an outcome nobody knows",
        "{guidance}"
    );
    assert!(
        guidance["next_action"]
            .as_str()
            .unwrap()
            .contains("work reassess")
    );
    let (decided, value) = p.reassess("continue", "continue", json!({}));
    assert!(decided, "{value}");
    // A blocking finding holds completion, not readiness: the work is ready and the instruction says so.
    p.finding(
        "bug",
        json!({"round": 1, "summary": "Saving offline loses the draft", "criterion": ACC,
               "counterexample": {"given": "the storage is read-only", "when": "the user saves", "then": "the draft is gone"}}),
    );
    let guidance = p.guidance("W");
    assert_eq!(
        guidance["when"], "Blocking findings hold the completion of this work",
        "{guidance}"
    );
    assert!(guidance["basis"].as_str().unwrap().contains("bug"));
}

#[test]
fn a_judgment_made_for_one_session_is_current_for_the_next_and_nothing_is_asked_twice() {
    let p = Project::new("governed_enforce");
    let (approved, value) = p.decide(
        "approve",
        "W",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(approved, "{value}");
    let claim = |agent: &str| {
        let revision = p.revision().to_string();
        p.run(&[
            "session",
            "start",
            "--work",
            "W",
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
    };
    // The first session claims the work under the decision and ends without finishing it.
    let (claimed, first) = claim("agent-one");
    assert!(claimed, "{first}");
    let first_session = first["session"]["id"].as_str().unwrap().to_string();
    let revision = p.revision().to_string();
    p.ok(&[
        "session",
        "end",
        "--session",
        &first_session,
        "--outcome",
        "incomplete",
        "--expected-revision",
        &revision,
    ]);
    // A later session, of another agent, needs no new decision and is told nothing about admission.
    let (claimed, second) = claim("agent-two");
    assert!(claimed, "the decision is still current: {second}");
    let after = p.guidance("W");
    assert!(
        !after["basis"].as_str().unwrap().starts_with("admission_"),
        "{after}"
    );
    // The decision is an event of the work, not of a session: it was never asked for again.
    let history = p.ok(&["work", "history", "W", "--event-type", "work.admission"]);
    assert_eq!(history["events"].as_array().unwrap().len(), 1, "{history}");
}
