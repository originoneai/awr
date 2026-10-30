//! Gaps through the real CLI: a gap is routed to the layer it affects and puts in question only the
//! judgments that read that layer, on that work, while the rest of the project keeps going.
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

fn governed(extra: &str) -> String {
    format!("mode='governed_enforce'\nscope='all'\n{extra}")
}

impl Project {
    fn new(policy: &str) -> Self {
        Self::build(Some(policy))
    }

    fn build(policy: Option<&str>) -> Self {
        let section = policy.map_or(String::new(), |policy| {
            format!("[sources.options.admission_policy]\nversion=1\n{policy}")
        });
        let p = Self(std::env::temp_dir().join(format!("awr-work-gap-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(
            p.0.join("mapping.toml"),
            format!("[project]\nname='Gap fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n{section}[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"),
        )
        .unwrap();
        fs::write(
            p.0.join("decisions/scope.md"),
            "---\n{\"id\":\"scope\",\"status\":\"accepted\",\"affected_keys\":[\"*\"]}\n---\n# Scope\n\n## Decision\nThe first release is local only.\n",
        )
        .unwrap();
        p.ledger(&["C1", "C2"], &[]);
        p.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
        p
    }

    fn pin(&self) -> String {
        format!(
            "sha256:{:x}",
            sha2::Sha256::digest(fs::read(self.0.join("decisions/scope.md")).unwrap())
        )
    }

    fn child(&self, parent: &str, path: &str) -> Value {
        json!({
            "version": 1, "purpose": "implementation", "tier": "required", "parent": parent,
            "intent": [{"role": "user_decision", "decision": "scope", "source_fingerprint": self.pin()}],
            "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft", "by": "agent-a"}],
            "granularity": {"judgment": "step_of_parent", "reason": "One step of the parent", "by": "agent-a"},
            "responsibility": {"name": "storage-module", "kind": "delivery"},
            "deliverable": format!("Work under {path}"),
            "evidence_method": "Targeted tests compared before and after the change",
            "failure_recovery": "A failed save keeps the previous data",
            "examples": [
                {"criterion": ACC, "kind": "positive", "given": "storage is read-only", "when": "the user saves", "then": "the input is still shown"},
                {"criterion": ACC, "kind": "negative", "given": "storage is read-only", "when": "the user saves", "then": "success is never reported"}
            ],
            "discrimination": {"method": "fail_before_pass_after"}
        })
    }

    /// P is the parent; `children` become its children (C3 deliberately sits outside P's paths);
    /// `extra_deps` gives C1 dependencies on other work.
    fn ledger(&self, children: &[&str], extra_deps: &[&str]) {
        let placed: Vec<(&str, &str)> = children
            .iter()
            .map(|key| {
                (
                    *key,
                    if *key == "C3" {
                        "crates/other/src"
                    } else {
                        "crates/storage/src"
                    },
                )
            })
            .collect();
        self.ledger_full(&["crates/storage"], &placed, extra_deps);
    }

    /// `children` are `(key, path)`; `parent_paths` are the paths the parent grants.
    fn ledger_full(&self, parent_paths: &[&str], children: &[(&str, &str)], extra_deps: &[&str]) {
        let mut works = vec![
            json!({"id": "P", "title": "Keep user input", "status": "planned", "goal": "G",
                   "acceptance": ["Every save keeps the input"], "paths": parent_paths,
                   "next_action": "Split it", "tags": []}),
            json!({"id": "X", "title": "Existing buffer work", "status": "completed", "goal": "G",
                   "acceptance": ["The buffer exists"], "paths": [], "tags": [], "next_action": "Nothing"}),
        ];
        for (key, path) in children {
            let mut work = json!({"id": key, "title": format!("Child {key}"), "status": "planned", "goal": "G",
                "acceptance": [ACC], "paths": [path], "tags": [], "next_action": "Do the work",
                "definition": self.child("P", path)});
            if *key == "C1" && !extra_deps.is_empty() {
                work["depends_on"] = json!(extra_deps);
            }
            works.push(work);
        }
        let ledger = json!({
            "goals": [{"id": "G", "title": "Safe saving", "status": "active", "success_criteria": [OUTCOME]}],
            "work_items": works
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

    fn revision(&self) -> u64 {
        self.ok(&["status"])["project_revision"].as_u64().unwrap()
    }

    /// The change-set fingerprint of the parent as `work show P` reports it.
    fn change_set(&self) -> String {
        self.ok(&["work", "show", "P"])["work"]["containment"]["current"]["change_set"]
            .as_str()
            .unwrap_or_else(|| panic!("no containment block"))
            .to_string()
    }

    fn judgment() -> Value {
        json!({"contained_because": "Both children serve the saving outcome and add no new surface",
               "reuse_checked": "The existing draft buffer is reused instead of a new store",
               "out_of_scope_excluded": "Syncing is left to a later change"})
    }

    /// `who` names the decider: `{"agent": ...}` or `{"session": ...}`.
    fn contain_by(&self, who: Value, key: &str, decision: &str, extra: Value) -> (bool, Value) {
        let mut request = json!({"parent": "P", "expected_revision": self.revision(),
            "request_key": key, "decision": decision, "change_set_fingerprint": self.change_set()});
        for (k, v) in who
            .as_object()
            .unwrap()
            .iter()
            .chain(extra.as_object().unwrap())
        {
            request[k] = v.clone();
        }
        self.record(key, &request)
    }

    fn contain_as(&self, agent: &str, key: &str, decision: &str, extra: Value) -> (bool, Value) {
        self.contain_by(json!({"agent": agent}), key, decision, extra)
    }

    fn record(&self, key: &str, request: &Value) -> (bool, Value) {
        let path = self.0.join(format!("contain-{key}.json"));
        fs::write(&path, serde_json::to_vec(request).unwrap()).unwrap();
        self.run(&["work", "contain", "--input", path.to_str().unwrap()])
    }

    fn approve_set(&self, agent: &str, key: &str) -> Value {
        let (success, value) = self.contain_as(
            agent,
            key,
            "approved",
            json!({"judgment": Self::judgment()}),
        );
        assert!(success, "{value}");
        value
    }

    /// An admission decision for a child, attributed to a declared agent.
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

    fn claim(&self, work: &str) -> (bool, Value) {
        let revision = self.revision().to_string();
        self.run(&[
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
        ])
    }
}

impl Project {
    fn gap(&self, work: &str, key: &str, gap: &str, action: &str, extra: Value) -> (bool, Value) {
        let mut request = json!({"work": work, "agent": "lead", "expected_revision": self.revision(),
            "request_key": key, "gap_key": gap, "action": action});
        for (k, v) in extra.as_object().unwrap() {
            request[k] = v.clone();
        }
        let path = self.0.join(format!("gap-{key}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        self.run(&["work", "gap", "--input", path.to_str().unwrap()])
    }

    fn open(&self, work: &str, gap: &str, route: &str, resolution: &str, extra: Value) -> Value {
        let mut details = json!({"route": route, "resolution": resolution,
            "subject": "The second criterion does not say what happens offline"});
        for (k, v) in extra.as_object().unwrap() {
            details[k] = v.clone();
        }
        let (opened, value) = self.gap(work, &format!("open-{gap}"), gap, "open", details);
        assert!(opened, "{value}");
        value
    }

    fn resolve_by_lookup(&self, work: &str, gap: &str) -> Value {
        let (resolved, value) = self.gap(
            work,
            &format!("resolve-{gap}"),
            gap,
            "resolve",
            json!({"outcome": "Offline saves are queued and replayed", "sources": ["docs/offline.md"]}),
        );
        assert!(resolved, "{value}");
        value
    }

    /// The readiness diagnostics of a work as `work prepare` reports them.
    fn codes(&self, work: &str) -> Vec<String> {
        self.prepared(work)["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["code"].as_str().unwrap().to_string())
            .collect()
    }

    fn prepared(&self, work: &str) -> Value {
        self.ok(&["work", "prepare", work, "--budget", "12000"])
    }
}

#[test]
fn a_gap_in_the_plan_of_a_parent_puts_its_children_in_question_and_resolving_it_lifts_that() {
    let p = Project::new(&governed(""));
    p.admit("C1");
    p.admit("C2");
    p.approve_set("lead", "approve");
    assert!(p.codes("C1").is_empty(), "{:?}", p.codes("C1"));
    // A gap in the design touches no judgment AWR keeps.
    p.open("P", "design-1", "design", "fact_lookup", json!({}));
    assert!(p.codes("C1").is_empty());
    // A gap in the plan puts the containment of the parent's children in question ...
    let opened = p.open("P", "plan-1", "plan", "fact_lookup", json!({}));
    assert_eq!(opened["already_recorded"], false);
    assert_eq!(opened["gaps"]["open"], json!(["design-1", "plan-1"]));
    for child in ["C1", "C2"] {
        assert_eq!(p.codes(child), ["containment_suspended"], "{child}");
    }
    let prepared = p.prepared("C1");
    assert_eq!(prepared["next_action"], "resolve_open_gaps_before_claiming");
    let refusal = p.claim("C1");
    assert!(!refusal.0, "{}", refusal.1);
    assert_eq!(refusal.1["code"], "AdmissionRequired", "{}", refusal.1);
    assert_eq!(refusal.1["details"]["reason"], "containment_suspended");
    assert!(
        refusal.1["details"]["diagnostics"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("plan-1")
    );
    // The parent shows its gaps, and what they put in question names only the plan gap.
    let shown = p.ok(&["work", "show", "P"]);
    assert_eq!(shown["work"]["gaps"]["open"], json!(["design-1", "plan-1"]));
    assert_eq!(
        shown["work"]["gaps"]["suspensions"],
        json!([{"judgment": "containment", "gaps": ["plan-1"]}])
    );
    // Closing it lifts the suspension: the containment decision was never touched, so nothing is
    // decided again. The gap in the design is still open and still holds nothing.
    let resolved = p.resolve_by_lookup("P", "plan-1");
    assert_eq!(resolved["gaps"]["open"], json!(["design-1"]));
    assert!(p.codes("C1").is_empty());
    assert!(p.claim("C1").0);
    // Everything stays on record.
    let history = p.ok(&["work", "history", "P", "--event-type", "work.gap"]);
    assert_eq!(history["events"].as_array().unwrap().len(), 3, "{history}");
}

#[test]
fn a_gap_in_the_acceptance_of_a_child_puts_only_its_own_admission_in_question() {
    let p = Project::new(&governed(""));
    p.admit("C1");
    p.admit("C2");
    p.approve_set("lead", "approve");
    p.open("C1", "accept-1", "acceptance", "fact_lookup", json!({}));
    assert_eq!(p.codes("C1"), ["admission_suspended"]);
    assert!(p.codes("C2").is_empty(), "a sibling is not held");
    assert!(p.codes("P").iter().all(|c| !c.ends_with("_suspended")));
    let refusal = p.claim("C1");
    assert!(!refusal.0, "{}", refusal.1);
    assert_eq!(refusal.1["details"]["reason"], "admission_suspended");
    assert!(p.claim("C2").0);
    p.resolve_by_lookup("C1", "accept-1");
    assert!(p.claim("C1").0);
}

#[test]
fn the_command_replays_names_what_it_refuses_and_is_a_capability_of_admission() {
    let p = Project::new(&governed(""));
    let first = p.open("C1", "g1", "acceptance", "fact_lookup", json!({}));
    let (replayed, replay) = p.gap(
        "C1",
        "open-g1",
        "g1",
        "open",
        json!({"route": "acceptance", "resolution": "fact_lookup",
               "subject": "The second criterion does not say what happens offline"}),
    );
    assert!(replayed, "{replay}");
    assert_eq!(replay["already_recorded"], true);
    assert_eq!(replay["event"]["id"], first["event"]["id"]);
    // An experiment is not closed by saying so.
    p.open(
        "C1",
        "offline",
        "plan",
        "feasibility_experiment",
        json!({"experiment": "probe-offline-save"}),
    );
    let (resolved, refusal) = p.gap(
        "C1",
        "resolve-offline",
        "offline",
        "resolve",
        json!({"outcome": "It is feasible"}),
    );
    assert!(!resolved, "{refusal}");
    assert_eq!(refusal["code"], "AdmissionRequired", "{refusal}");
    assert_eq!(refusal["details"]["reason"], "gap_experiment_not_run");
    // A choice nobody is authorized to make cannot even be opened.
    let (opened, refusal) = p.gap(
        "C1",
        "open-choice",
        "choice",
        "open",
        json!({"route": "intent", "resolution": "authorized_choice", "subject": "Which storage?"}),
    );
    assert!(!opened, "{refusal}");
    assert_eq!(refusal["details"]["reason"], "gap_authority_unknown");
    // Withdrawing keeps the gap listed.
    let (withdrawn, value) = p.gap(
        "C1",
        "withdraw-g1",
        "g1",
        "withdraw",
        json!({"reason": "Rewritten by the user"}),
    );
    assert!(withdrawn, "{value}");
    let shown = p.ok(&["work", "show", "C1"]);
    let states: Vec<&str> = shown["work"]["gaps"]["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["withdrawn", "open"], "{shown}");
    let prepared = p.prepared("C1");
    assert_eq!(prepared["gaps"]["open"], json!(["offline"]));
    let capabilities = p.ok(&["capabilities"]).to_string();
    assert!(capabilities.contains("work gap"), "{capabilities}");
}
