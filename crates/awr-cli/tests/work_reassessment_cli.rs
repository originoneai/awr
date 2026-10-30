//! Reassessment through the real CLI: a governed work whose own records show a method failing
//! again and again is held until a current decision answers those facts, and nothing else is.
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
        let p = Self(std::env::temp_dir().join(format!("awr-work-reassessment-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(
            p.0.join("mapping.toml"),
            format!("[project]\nname='Reassessment fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[sources.options.admission_policy]\nversion=1\nmode='{mode}'\nscope='all'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"),
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

    /// An approved admission decision attributed to a declared agent.
    fn admit(&self, work: &str) {
        let fingerprint = self.ok(&["work", "assess", work])["contract_fingerprint"].clone();
        let request = json!({"work": work, "agent": "lead", "expected_revision": self.revision(), "request_key": format!("admit-{work}"),
            "decision": "approved", "contract_fingerprint": fingerprint,
            "judgment": {"contribution": "Advances the saving outcome", "necessity": "The draft is lost today",
                         "alternatives": "Reuse of the existing buffer was checked", "deliverable": "A fallback path",
                         "exit": "Stop when the fallback is verified", "responsibility": "storage-module"}});
        let path = self.0.join(format!("admit-{work}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        self.ok(&["work", "admit", "--input", path.to_str().unwrap()]);
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

#[test]
fn a_work_whose_records_show_repeated_failures_is_held_until_a_current_decision() {
    let p = Project::new("governed_enforce");
    p.admit("W");
    let session = p.session("agent-w", "W");
    // Work with no trouble gains no key.
    assert!(
        p.ok(&["work", "show", "W"])["work"]
            .get("reassessment")
            .is_none()
    );
    let failed: Vec<String> = (1..=3)
        .map(|n| p.fail(&session, &format!("check-{n}")))
        .collect();
    let block = p.block("W");
    assert_eq!(block["state"], "required", "{block}");
    assert_eq!(block["triggers"][0]["trigger"], "repeated_failed_method");
    let mut references: Vec<String> = block["triggers"][0]["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect();
    let mut expected = failed.clone();
    references.sort();
    expected.sort();
    assert_eq!(references, expected);
    // The failures are classified and keep a way back to their evidence.
    let kinds: Vec<&str> = block["deviations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["failed_method"; 3]);
    assert!(
        block["deviations"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("exited with code"),
        "{block}"
    );
    // Nothing else is held: the sibling work shows nothing and is not touched.
    assert!(
        p.ok(&["work", "show", "V"])["work"]
            .get("reassessment")
            .is_none()
    );
    // prepare says what is missing.
    let prepared = p.ok(&["work", "prepare", "W", "--budget", "12000"]);
    assert_eq!(prepared["reassessment"]["state"], "required", "{prepared}");
    assert_eq!(
        prepared["next_action"],
        "record_reassessment_before_claiming"
    );
    // A new attempt is refused, whatever it runs.
    let (launched, refusal) = p.launch(&session, "check-4");
    assert!(!launched, "{refusal}");
    assert_eq!(refusal["code"], "AdmissionRequired", "{refusal}");
    assert_eq!(refusal["details"]["reason"], "reassessment_required");
    // The decision binds these facts and answers with the state it leaves.
    let (decided, receipt) = p.reassess("decide-1", "continue", json!({}));
    assert!(decided, "{receipt}");
    assert_eq!(receipt["already_recorded"], false);
    assert_eq!(receipt["reassessment"]["state"], "current");
    assert_eq!(receipt["execution_started"], false);
    assert_eq!(
        receipt["event"]["payload"]["decider_agent"], "lead",
        "{receipt}"
    );
    let (replayed, replay) = p.reassess("decide-1", "continue", json!({}));
    assert!(replayed, "{replay}");
    assert_eq!(replay["already_recorded"], true);
    assert_eq!(replay["event"]["id"], receipt["event"]["id"]);
    // The same key with other content is refused.
    let (changed, conflict) = p.reassess("decide-1", "replan", json!({}));
    assert!(!changed, "{conflict}");
    assert_eq!(conflict["code"], "SourceConflict", "{conflict}");
    // The work can go on, and a further failure makes the decision stale.
    let fourth = p.fail(&session, "check-4");
    let block = p.block("W");
    assert_eq!(block["state"], "stale", "{block}");
    assert!(
        block["diagnostics"][0]["detail"]
            .as_str()
            .unwrap()
            .contains(&format!("fact_added:{fourth}")),
        "{block}"
    );
    let (launched, refusal) = p.launch(&session, "check-5");
    assert!(!launched, "{refusal}");
    assert_eq!(refusal["details"]["reason"], "reassessment_stale");
}

#[test]
fn a_decision_needs_the_facts_it_answers() {
    let p = Project::new("governed_enforce");
    p.admit("W");
    let session = p.session("agent-w", "W");
    let early = json!({"work": "W", "agent": "lead", "expected_revision": p.revision(),
        "request_key": "early", "decision": "continue", "facts_fingerprint": format!("sha256:{}", "a".repeat(64)),
        "judgment": {"reason": "Looked", "next_step": "Go on", "evidence_considered": "Nothing"}});
    let path = p.0.join("early.json");
    fs::write(&path, serde_json::to_vec(&early).unwrap()).unwrap();
    let (recorded, value) = p.run(&["work", "reassess", "--input", path.to_str().unwrap()]);
    assert!(!recorded, "{value}");
    assert_eq!(value["code"], "InvalidInput", "{value}");
    for n in 1..=3 {
        p.fail(&session, &format!("check-{n}"));
    }
    // Facts other than the current ones are a conflict, not a decision.
    let (recorded, value) = p.reassess(
        "wrong",
        "continue",
        json!({"facts_fingerprint": format!("sha256:{}", "b".repeat(64))}),
    );
    assert!(!recorded, "{value}");
    assert_eq!(value["code"], "SourceConflict", "{value}");
    // Only records of this work may be grouped under a cause.
    let (recorded, value) = p.reassess(
        "foreign",
        "alter_method",
        json!({"root_cause": {"label": "The fixture is read-only", "references": [Id::new().to_string()]}}),
    );
    assert!(!recorded, "{value}");
    assert_eq!(value["code"], "InvalidInput", "{value}");
    // An attributed grouping of real records is kept with the decision.
    let references = p.block("W")["current"]["references"].clone();
    let (recorded, value) = p.reassess(
        "grouped",
        "alter_method",
        json!({"root_cause": {"label": "The fixture is read-only", "references": references}}),
    );
    assert!(recorded, "{value}");
    assert_eq!(
        value["event"]["payload"]["root_cause"]["label"],
        "The fixture is read-only"
    );
    // Work in a project that only observes is told, never held.
    let observing = Project::new("legacy_observe");
    observing.admit("W");
    let session = observing.session("agent-w", "W");
    for n in 1..=3 {
        observing.fail(&session, &format!("check-{n}"));
    }
    assert_eq!(observing.block("W")["state"], "required");
    let (launched, value) = observing.launch(&session, "check-4");
    assert!(launched, "observing never holds anything: {value}");
}

#[test]
fn the_command_is_a_capability_of_admission() {
    let p = Project::new("governed_enforce");
    let capabilities = p.ok(&["capabilities"]).to_string();
    assert!(capabilities.contains("work reassess"), "{capabilities}");
}
