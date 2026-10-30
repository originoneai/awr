//! Findings through the real CLI: a finding's severity is derived, only a blocking finding holds the
//! completion of its work, and a deferred record is listed and blocks nothing.
use awr_core::Id;
use serde_json::{Value, json};
use sha2::Digest;
use std::{fs, path::PathBuf, process::Command};

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
        let p = Self(std::env::temp_dir().join(format!("awr-work-finding-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(
            p.0.join("mapping.toml"),
            format!("[project]\nname='Finding fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[sources.options.admission_policy]\nversion=1\nmode='{mode}'\nscope='all'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"),
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
}

impl Project {
    fn finding(&self, key: &str, finding: &str, action: &str, extra: Value) -> (bool, Value) {
        let mut request = json!({"work": "W", "agent": "reviewer", "expected_revision": self.revision(),
            "request_key": key, "finding_key": finding, "action": action});
        for (k, v) in extra.as_object().unwrap() {
            request[k] = v.clone();
        }
        let path = self.0.join(format!("finding-{key}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        self.run(&["work", "finding", "--input", path.to_str().unwrap()])
    }

    fn record(&self, finding: &str, extra: Value) -> Value {
        let (recorded, value) =
            self.finding(&format!("record-{finding}"), finding, "record", extra);
        assert!(recorded, "{value}");
        value
    }

    /// Makes work W small with a tight rework budget: one return is allowed, a second spends it.
    fn make_small(&self) {
        let path = self.0.join("work.yaml");
        let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        ledger["work_items"][0]["definition"]["size"] = json!("small");
        ledger["work_items"][0]["definition"]["budget"] = json!({"rework_cycles": 1});
        fs::write(&path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
        self.ok(&["source", "reindex"]);
    }

    fn claimed_session(&self, work: &str) -> String {
        let revision = self.revision().to_string();
        self.ok(&[
            "session",
            "start",
            "--work",
            work,
            "--agent",
            "agent-w",
            "--provider",
            "test",
            "--model",
            "m",
            "--claim",
            "--expected-revision",
            &revision,
        ])["session"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Registers a report that verifies the criterion, and returns what completion needs.
    fn evidence(&self) -> PathBuf {
        let sha = "a".repeat(40);
        let verified_at = awr_core::now_millis().unwrap();
        let report = self.0.join("report.json");
        fs::write(
            &report,
            serde_json::to_vec(&json!({"version": 1, "work_item": "W", "source_sha": sha,
                "command": "cargo test", "scope": ["W"], "verified_at": verified_at,
                "checks": [{"name": "saving", "passed": true, "criteria": [ACC], "details": "The tests pass"}]}))
            .unwrap(),
        )
        .unwrap();
        let evidence = json!({"work_item_key": "W", "external_key": "unit-1", "evidence_type": "unit_test",
            "level": "locally_verified", "summary": "The tests pass", "locator": report.to_str().unwrap(),
            "sha256": format!("{:x}", sha2::Sha256::digest(fs::read(&report).unwrap())),
            "source_sha": sha, "command": "cargo test", "scope": ["W"], "verified_at": verified_at});
        let path = self.0.join("evidence.json");
        fs::write(&path, serde_json::to_vec(&evidence).unwrap()).unwrap();
        let revision = self.revision().to_string();
        self.ok(&[
            "evidence",
            "add",
            "--input",
            path.to_str().unwrap(),
            "--expected-revision",
            &revision,
        ]);
        let completion = json!({"version": 1, "source_sha": sha, "minimum_level": "locally_verified",
            "required_evidence": ["unit-1"], "acceptance": [{"criterion": ACC, "evidence": ["unit-1"]}]});
        let path = self.0.join("completion.json");
        fs::write(&path, serde_json::to_vec(&completion).unwrap()).unwrap();
        path
    }

    fn complete(&self, session: &str, input: &std::path::Path) -> (bool, Value) {
        let revision = self.revision().to_string();
        self.run(&[
            "work",
            "complete",
            "W",
            "--session",
            session,
            "--reason",
            "The tests pass",
            "--input",
            input.to_str().unwrap(),
            "--expected-revision",
            &revision,
        ])
    }
}

fn counterexample() -> Value {
    json!({"given": "the storage is read-only", "when": "the user saves", "then": "the draft is gone"})
}

#[test]
fn a_blocking_finding_holds_the_completion_of_its_work_and_a_deferred_record_does_not() {
    let p = Project::new("governed_enforce");
    p.admit("W");
    let session = p.claimed_session("W");
    let revision = p.revision().to_string();
    p.ok(&[
        "work",
        "progress",
        "W",
        "--session",
        &session,
        "--reason",
        "Starting the work",
        "--next-action",
        "Write the fallback",
        "--expected-revision",
        &revision,
    ]);
    let completion = p.evidence();
    // Work with nothing found shows no block.
    assert!(
        p.ok(&["work", "show", "W"])["work"]
            .get("findings")
            .is_none()
    );
    // A second-round matter of style is deferred; a required criterion with a counterexample blocks.
    let style = p.record(
        "style",
        json!({"round": 2, "summary": "Prefer shorter helper names"}),
    );
    assert_eq!(
        style["findings"]["findings"][0]["severity"], "non_blocking",
        "{style}"
    );
    assert_eq!(style["findings"]["findings"][0]["disposition"], "deferred");
    let bug = p.record(
        "bug",
        json!({"round": 1, "summary": "Saving offline loses the draft", "criterion": ACC,
               "counterexample": counterexample()}),
    );
    assert_eq!(bug["findings"]["open_blocking"], json!(["bug"]), "{bug}");
    assert_eq!(bug["completion_claimed"], false);
    let block = p.ok(&["work", "show", "W"])["work"]["findings"].clone();
    assert_eq!(block["open_blocking"], json!(["bug"]), "{block}");
    assert_eq!(block["open_deferred"], json!(["style"]));
    assert!(
        block["findings"][1]["basis"]
            .as_str()
            .unwrap()
            .contains("required criterion")
    );
    assert_eq!(
        p.ok(&["work", "prepare", "W", "--budget", "12000"])["findings"]["open_blocking"],
        json!(["bug"])
    );
    // The blocking finding returns the work: it cannot be completed while the finding is open.
    let (completed, refusal) = p.complete(&session, &completion);
    assert!(!completed, "{refusal}");
    assert_eq!(refusal["code"], "DependencyBlocked", "{refusal}");
    assert!(
        refusal["message"].as_str().unwrap().contains("bug"),
        "{refusal}"
    );
    assert_eq!(
        p.ok(&["work", "show", "W"])["work"]["status"],
        "in_progress"
    );
    // Dealing with it releases the work; the deferred record stays open, listed and harmless.
    let (resolved, value) = p.finding(
        "resolve-bug",
        "bug",
        "resolve",
        json!({"resolution": "The fallback keeps the draft"}),
    );
    assert!(resolved, "{value}");
    assert_eq!(value["findings"]["open_blocking"], json!([]));
    let (completed, value) = p.complete(&session, &completion);
    assert!(completed, "{value}");
    let shown = p.ok(&["work", "show", "W"]);
    assert_eq!(shown["work"]["status"], "completed", "{shown}");
    assert_eq!(shown["work"]["findings"]["open_deferred"], json!(["style"]));
    // A replay returns the original receipt, and a claim of the criterion that is not one is refused.
    let (replayed, replay) = p.finding(
        "record-style",
        "style",
        "record",
        json!({"round": 2, "summary": "Prefer shorter helper names"}),
    );
    assert!(replayed, "{replay}");
    assert_eq!(replay["already_recorded"], true);
    let (recorded, refusal) = p.finding(
        "ghost",
        "ghost",
        "record",
        json!({"round": 1, "summary": "Saving is slow", "criterion": "Saving is fast"}),
    );
    assert!(!recorded, "{refusal}");
    assert_eq!(refusal["code"], "InvalidInput", "{refusal}");
}

#[test]
fn the_command_is_a_capability_of_admission() {
    let p = Project::new("governed_enforce");
    assert!(p.ok(&["capabilities"]).to_string().contains("work finding"));
}

#[test]
fn a_return_of_the_work_is_counted_once_whether_a_review_round_or_a_reopening_makes_it() {
    let p = Project::new("governed_enforce");
    p.make_small();
    p.admit("W");
    let session = p.claimed_session("W");
    let revision = p.revision().to_string();
    p.ok(&[
        "work",
        "progress",
        "W",
        "--session",
        &session,
        "--reason",
        "Starting the work",
        "--next-action",
        "Write the fallback",
        "--expected-revision",
        &revision,
    ]);
    let completion = p.evidence();
    // Round 1 finds blocking issues in unfinished work: the first return, within a budget of one.
    p.record(
        "bug",
        json!({"round": 1, "summary": "Saving offline loses the draft", "criterion": ACC,
               "counterexample": counterexample()}),
    );
    assert!(
        p.ok(&["work", "show", "W"])["work"]
            .get("reassessment")
            .is_none(),
        "one return is within the budget and shows nothing"
    );
    let (resolved, value) = p.finding(
        "resolve-bug",
        "bug",
        "resolve",
        json!({"resolution": "The fallback keeps the draft"}),
    );
    assert!(resolved, "{value}");
    let (completed, value) = p.complete(&session, &completion);
    assert!(completed, "{value}");
    // Round 2 finds blocking issues in completed work: the reopening that answers it is the return.
    p.record(
        "late",
        json!({"round": 2, "summary": "Saving offline still loses the draft", "criterion": ACC,
               "counterexample": counterexample()}),
    );
    let revision = p.revision().to_string();
    p.ok(&[
        "work",
        "reopen",
        "W",
        "--session",
        &session,
        "--reason",
        "A blocking finding returned the work",
        "--next-action",
        "Fix the fallback",
        "--expected-revision",
        &revision,
    ]);
    let budget = p.ok(&["work", "show", "W"])["work"]["reassessment"]["budget"].clone();
    assert_eq!(budget["use"]["rework"]["used"], 2, "{budget}");
    assert_eq!(budget["use"]["rework"]["allowed"], 1);
    assert_eq!(budget["use"]["rework"]["counted_from"], "recorded");
    assert_eq!(budget["state"], "required");
}
