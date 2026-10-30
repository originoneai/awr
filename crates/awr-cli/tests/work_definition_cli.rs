//! A work definition through the real CLI: reported by `work show` and `work prepare` only when it
//! is declared, checked against the current goals and decisions, and bound into the management
//! contract fingerprint without disturbing work that declares none.
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
    fn new() -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-work-definition-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(
            p.0.join("mapping.toml"),
            "[project]\nname='Definition fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n",
        )
        .unwrap();
        p.decision("The first release is local only.");
        p.ledger(
            &p.definition("Local save with a failure fallback"),
            "Write the fallback",
        );
        p.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        p
    }

    fn decision(&self, body: &str) {
        self.decision_with_status("accepted", body);
    }

    fn decision_with_status(&self, status: &str, body: &str) {
        let metadata = json!({"id":"scope","status":status,"affected_keys":["*"]});
        fs::write(
            self.0.join("decisions/scope.md"),
            format!(
                "---\n{}\n---\n# Scope\n\n## Decision\n{body}\n",
                serde_json::to_string_pretty(&metadata).unwrap()
            ),
        )
        .unwrap();
    }

    /// An accepted specification with one item of the given disposition, and the reference that
    /// binds that item to exactly the acceptance criterion of work W.
    fn roadmap(&self, disposition: &str) -> Value {
        let metadata = json!({"id": "roadmap", "status": "accepted", "affected_keys": ["*"],
            "specification": {"version": "1", "items": [{"id": "R1", "disposition": disposition}]}});
        let path = self.0.join("decisions/roadmap.md");
        fs::write(
            &path,
            format!(
                "---\n{}\n---\n# Roadmap\n\n## Decision\nUse the explicit requirements.\n",
                serde_json::to_string_pretty(&metadata).unwrap()
            ),
        )
        .unwrap();
        json!([{"specification": "roadmap", "item": "R1", "criterion": ACC,
                "source_fingerprint": format!("sha256:{:x}", sha2::Sha256::digest(fs::read(&path).unwrap()))}])
    }

    fn pin(&self) -> String {
        format!(
            "sha256:{:x}",
            sha2::Sha256::digest(fs::read(self.0.join("decisions/scope.md")).unwrap())
        )
    }

    fn definition(&self, deliverable: &str) -> Value {
        json!({
            "version": 1, "purpose": "implementation", "tier": "required",
            "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": self.pin()}],
            "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft", "by": "agent-a"}],
            "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge", "by": "agent-a"},
            "responsibility": {"name": "storage-module", "kind": "delivery"},
            "deliverable": deliverable,
            "evidence_method": "Targeted tests compared before and after the change",
            "failure_recovery": "A failed save keeps the previous data",
            "examples": [
                {"criterion": ACC, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
                {"criterion": ACC, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"}
            ],
            "discrimination": {"method": "fail_before_pass_after"}
        })
    }

    fn ledger(&self, definition: &Value, next_action: &str) {
        self.ledger_with(definition, next_action, json!({}));
    }

    /// `extra` holds more fields of work W, such as `depends_on` or `spec_refs`.
    fn ledger_with(&self, definition: &Value, next_action: &str, extra: Value) {
        let mut work = json!({
            "id": "W", "title": "Save safely", "status": "planned", "goal": "G",
            "acceptance": [ACC], "paths": ["crates/storage/src"], "next_action": next_action,
            "definition": definition
        });
        work.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let ledger = json!({
            "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
            "work_items": [
                work,
                {"id": "L", "title": "Legacy work", "status": "planned", "goal": "G",
                 "acceptance": ["It still works"], "next_action": "Keep going"}
            ]
        });
        fs::write(
            self.0.join("work.yaml"),
            serde_json::to_vec_pretty(&ledger).unwrap(),
        )
        .unwrap();
    }

    fn reindex(&self) {
        self.ok(&["source", "reindex"]);
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .args(["--project", self.0.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

fn gap_codes(report: &Value) -> Vec<String> {
    report["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|gap| gap["code"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn show_and_prepare_report_a_declared_definition_and_stay_silent_without_one() {
    let p = Project::new();
    let shown = p.ok(&["work", "show", "W"]);
    let report = &shown["work"]["definition"];
    assert_eq!(report["state"], "complete", "{report}");
    assert_eq!(report["purpose"], "implementation");
    assert!(gap_codes(report).is_empty(), "{report}");
    let prepared = p.ok(&["work", "prepare", "W", "--budget", "12000"]);
    assert_eq!(prepared["definition"]["state"], "complete");
    assert_eq!(prepared["definition"], *report);

    // Work without a definition is reported exactly as before: no new key anywhere.
    let legacy = p.ok(&["work", "show", "L"]);
    assert!(legacy["work"].get("definition").is_none(), "{legacy}");
    let prepared = p.ok(&["work", "prepare", "L", "--budget", "12000"]);
    assert!(prepared.get("definition").is_none());
}

#[test]
fn a_missing_obligation_is_an_explicit_gap_not_a_silent_pass() {
    let p = Project::new();
    let mut sparse = p.definition("Local save");
    for field in [
        "responsibility",
        "examples",
        "discrimination",
        "evidence_method",
    ] {
        sparse.as_object_mut().unwrap().remove(field);
    }
    p.ledger(&sparse, "Write the fallback");
    p.reindex();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(report["state"], "gaps");
    let codes = gap_codes(&report);
    for code in [
        "responsibility_missing",
        "examples_missing",
        "discrimination_missing",
        "evidence_method_missing",
    ] {
        assert!(codes.contains(&code.to_string()), "{code} in {codes:?}");
    }
}

#[test]
fn editing_an_adopted_decision_makes_the_pinned_intent_stale() {
    let p = Project::new();
    p.decision("The first release is local only and syncs later.");
    p.reindex();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert!(
        gap_codes(&report).contains(&"intent_ref_stale".to_string()),
        "{report}"
    );
    // Pinning the current bytes again restores the adoption.
    p.ledger(
        &p.definition("Local save with a failure fallback"),
        "Write the fallback",
    );
    p.reindex();
    assert_eq!(
        p.ok(&["work", "show", "W"])["work"]["definition"]["state"],
        "complete"
    );
}

#[test]
fn the_management_contract_follows_the_definition_but_not_progress() {
    let p = Project::new();
    let fingerprint = |work: &str| p.ok(&["work", "assess", work])["contract_fingerprint"].clone();
    let (declared, plain) = (fingerprint("W"), fingerprint("L"));

    // Progress and next-action edits keep the binding.
    p.ledger(
        &p.definition("Local save with a failure fallback"),
        "A different next step",
    );
    p.reindex();
    assert_eq!(fingerprint("W"), declared);
    // A definition edit does not.
    p.ledger(
        &p.definition("Local save with retry"),
        "A different next step",
    );
    p.reindex();
    assert_ne!(fingerprint("W"), declared);
    // Work that declares no definition is untouched by all of it.
    assert_eq!(fingerprint("L"), plain);
}

#[test]
fn a_decision_that_is_not_accepted_cannot_carry_adopted_intent() {
    let p = Project::new();
    p.decision_with_status("proposed", "The first release is local only.");
    p.ledger(
        &p.definition("Local save with a failure fallback"),
        "Write the fallback",
    );
    p.reindex();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert!(
        gap_codes(&report).contains(&"intent_ref_not_found".to_string()),
        "{report}"
    );
    p.decision_with_status("accepted", "The first release is local only.");
    p.ledger(
        &p.definition("Local save with a failure fallback"),
        "Write the fallback",
    );
    p.reindex();
    assert_eq!(
        p.ok(&["work", "show", "W"])["work"]["definition"]["state"],
        "complete"
    );
}

fn obligation_state(report: &Value, name: &str) -> String {
    report["obligations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["obligation"] == name)
        .unwrap_or_else(|| panic!("no obligation {name} in {report}"))["state"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn prerequisites_and_assertions_are_read_from_the_work_itself() {
    let p = Project::new();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(obligation_state(&report, "prerequisites"), "not_applicable");
    assert_eq!(obligation_state(&report, "assertions"), "satisfied");

    p.ledger_with(
        &p.definition("Local save with a failure fallback"),
        "Write the fallback",
        json!({"depends_on": ["L"]}),
    );
    p.reindex();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(obligation_state(&report, "prerequisites"), "satisfied");
    assert_eq!(report["state"], "complete", "{report}");
}

#[test]
fn a_specification_disposition_sets_the_tier_of_the_criterion_it_is_bound_to() {
    let p = Project::new();
    let mut sparse = p.definition("Local save with a failure fallback");
    for field in ["examples", "discrimination", "tier"] {
        sparse.as_object_mut().unwrap().remove(field);
    }
    let show = |p: &Project| p.ok(&["work", "show", "W"])["work"]["definition"].clone();

    // A required criterion without examples is an explicit gap.
    p.ledger(&sparse, "Write the fallback");
    p.reindex();
    assert!(gap_codes(&show(&p)).contains(&"examples_missing".to_string()));

    // A future specification item bound to exactly that criterion defers it.
    let refs = p.roadmap("future");
    p.ledger_with(&sparse, "Write the fallback", json!({"spec_refs": refs}));
    p.reindex();
    let report = show(&p);
    let codes = gap_codes(&report);
    assert!(!codes.contains(&"examples_missing".to_string()), "{report}");
    assert!(
        !codes.contains(&"discrimination_missing".to_string()),
        "{report}"
    );
    assert_eq!(report["effective_tier"], "later");

    // The same item declared required puts the criterion back in the current increment.
    let refs = p.roadmap("required");
    p.ledger_with(&sparse, "Write the fallback", json!({"spec_refs": refs}));
    p.reindex();
    let report = show(&p);
    assert!(
        gap_codes(&report).contains(&"examples_missing".to_string()),
        "{report}"
    );
    assert_eq!(report["effective_tier"], "required");

    // A work marked required cannot rest only on future items.
    let refs = p.roadmap("future");
    let mut marked = sparse.clone();
    marked["tier"] = json!("required");
    p.ledger_with(&marked, "Write the fallback", json!({"spec_refs": refs}));
    p.reindex();
    assert!(gap_codes(&show(&p)).contains(&"tier_conflicts_with_specification".to_string()));
}
