//! Controlled decomposition through the real CLI: the children of a parent are admitted only
//! while a decision on exactly this set is current. Splitting, widening, reusing, rejecting and
//! drifting all stay visible, and history and denominators are never rewritten.
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

    /// A project that declares no admission policy at all.
    fn without_policy() -> Self {
        Self::build(None)
    }

    fn build(policy: Option<&str>) -> Self {
        let section = policy.map_or(String::new(), |policy| {
            format!("[sources.options.admission_policy]\nversion=1\n{policy}")
        });
        let p = Self(std::env::temp_dir().join(format!("awr-work-containment-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(
            p.0.join("mapping.toml"),
            format!("[project]\nname='Containment fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n{section}[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"),
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

    fn reindex(&self) {
        self.ok(&["source", "reindex"]);
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

    /// A session bound to no work: enough to decide.
    fn reviewer(&self, agent: &str) -> String {
        let revision = self.revision().to_string();
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

fn refused(result: (bool, Value), code: &str) -> Value {
    let (success, value) = result;
    assert!(!success, "expected a refusal: {value}");
    assert_eq!(value["code"], "AdmissionRequired", "{value}");
    assert_eq!(value["details"]["reason"], code, "{value}");
    value
}

fn detail_of(value: &Value) -> String {
    value["details"]["diagnostics"][0]["detail"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn governed_children_wait_for_a_current_decision_on_exactly_their_set() {
    let p = Project::new(&governed(""));
    p.admit("C1");
    // The child is admitted, yet its parent's set of children has no decision.
    refused(p.claim("C1"), "containment_missing");
    let (proposed, value) = p.contain_as(
        "lead",
        "propose",
        "proposed",
        json!({"judgment": Project::judgment()}),
    );
    assert!(proposed, "{value}");
    refused(p.claim("C1"), "containment_pending");
    p.approve_set("lead", "approve");
    let (claimed, started) = p.claim("C1");
    assert!(claimed, "{started}");
}

#[test]
fn a_child_outside_the_paths_of_its_parent_is_unchecked_and_cannot_be_approved() {
    let p = Project::new(&governed(""));
    p.ledger(&["C1", "C2", "C3"], &[]);
    p.reindex();
    let shown = p.ok(&["work", "show", "P"]);
    let detail = shown["work"]["containment"]["diagnostics"].to_string();
    assert!(
        detail.contains("containment_unchecked") && detail.contains("child_outside_parent_scope"),
        "{detail}"
    );
    assert!(detail.contains("C3"), "{detail}");
    let (approved, value) = p.contain_as(
        "lead",
        "widen",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(!approved);
    assert_eq!(
        value["details"]["reason"], "containment_unchecked",
        "{value}"
    );
    // Taking the child back inside the parent lets the set be decided.
    p.ledger(&["C1", "C2"], &[]);
    p.reindex();
    p.approve_set("lead", "split");
}

#[test]
fn adding_changing_or_removing_a_child_makes_the_approval_stale_and_names_it() {
    let p = Project::new(&governed(""));
    p.admit("C1");
    p.approve_set("lead", "approve");
    // A new child appears: the set that was judged no longer covers the work.
    p.ledger(&["C1", "C2", "C4"], &[]);
    p.reindex();
    let shown = p.ok(&["work", "show", "P"]);
    assert_eq!(shown["work"]["containment"]["state"], "stale", "{shown}");
    let detail = shown["work"]["containment"]["diagnostics"][0]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(detail.contains("child_added:C4"), "{detail}");
    // Removing a child cannot shrink the set unseen either: the removal is named.
    p.ledger(&["C1"], &[]);
    p.reindex();
    let shown = p.ok(&["work", "show", "P"]);
    let detail = shown["work"]["containment"]["diagnostics"][0]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("child_removed:C2") && detail.contains("child_removed"),
        "{detail}"
    );
    refused(p.claim("C1"), "containment_stale");
    // History is kept: the earlier approval is still on record.
    let history = p.ok(&["work", "history", "P", "--event-type", "work.containment"]);
    assert_eq!(history["events"].as_array().unwrap().len(), 1, "{history}");
    // Deciding the current set restores it, and that is a second event, not a rewrite.
    p.approve_set("lead", "approve-2");
    let history = p.ok(&["work", "history", "P", "--event-type", "work.containment"]);
    assert_eq!(history["events"].as_array().unwrap().len(), 2, "{history}");
}

#[test]
fn a_rejected_set_blocks_its_children_and_a_later_approval_replaces_it() {
    let p = Project::new(&governed(""));
    p.admit("C1");
    let (denied, value) = p.contain_as(
        "lead",
        "reject",
        "denied",
        json!({"reason": "The second child widens the parent"}),
    );
    assert!(denied, "{value}");
    let error = refused(p.claim("C1"), "containment_denied");
    assert!(detail_of(&error).contains("widens the parent"));
    p.approve_set("lead", "approve");
    assert!(p.claim("C1").0);
}

#[test]
fn a_dependency_on_existing_work_is_part_of_what_is_bound() {
    let p = Project::new(&governed(""));
    p.approve_set("lead", "approve");
    // Reusing existing work is a dependency, not a new child; changing it changes the bound set.
    p.ledger(&["C1", "C2"], &["X"]);
    p.reindex();
    let shown = p.ok(&["work", "show", "P"]);
    assert_eq!(shown["work"]["containment"]["state"], "stale", "{shown}");
    assert!(
        shown["work"]["containment"]["diagnostics"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("dependencies")
    );
}

#[test]
fn only_listed_supervisors_may_approve_when_the_policy_names_some() {
    let p = Project::new(&governed("supervisors=['lead']\n"));
    let (approved, value) = p.contain_as(
        "intern",
        "by-intern",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(!approved);
    assert_eq!(
        value["details"]["reason"], "containment_not_authorized",
        "{value}"
    );
    p.approve_set("lead", "by-lead");
    p.admit("C1");
    assert!(p.claim("C1").0);
}

#[test]
fn a_request_key_replays_the_original_outcome_and_never_a_different_request() {
    let p = Project::new(&governed(""));
    let first = p.approve_set("lead", "once");
    assert_eq!(first["already_recorded"], false);
    let replay = p.contain_as(
        "lead",
        "once",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(replay.0, "{}", replay.1);
    assert_eq!(replay.1["already_recorded"], true);
    assert_eq!(replay.1["event"]["id"], first["event"]["id"]);
    // Even a request that differs only by its decision is refused.
    let proposal = p.contain_as(
        "lead",
        "once",
        "proposed",
        json!({"judgment": Project::judgment()}),
    );
    assert!(!proposal.0, "{}", proposal.1);
    assert_eq!(proposal.1["code"], "SourceConflict", "{}", proposal.1);
}

#[test]
fn a_decision_reviewed_against_an_older_set_is_rejected() {
    let p = Project::new(&governed(""));
    let reviewed = p.change_set();
    p.ledger(&["C1", "C2", "C4"], &[]);
    p.reindex();
    let (approved, value) = p.contain_as(
        "lead",
        "late",
        "approved",
        json!({"judgment": Project::judgment(), "change_set_fingerprint": reviewed}),
    );
    assert!(!approved);
    assert_eq!(value["code"], "SourceConflict", "{value}");
}

#[test]
fn work_without_children_or_without_a_policy_shows_no_containment() {
    let p = Project::new(&governed(""));
    let shown = p.ok(&["work", "show", "C1"]);
    assert!(shown["work"].get("containment").is_none(), "{shown}");
    // Observing reports the state of the set and blocks nothing.
    let plain = Project::new("mode='legacy_observe'\n");
    let shown = plain.ok(&["work", "show", "P"]);
    assert_eq!(
        shown["work"]["containment"]["state"], "missing",
        "observe still reports"
    );
    assert!(plain.claim("C1").0, "observe never blocks");
}

#[test]
fn a_path_that_only_shares_a_prefix_with_the_parent_is_outside_it() {
    let p = Project::new(&governed(""));
    p.ledger_full(
        &["crates/storage"],
        &[
            ("C1", "crates/storage/src"),
            ("C5", "crates/storage-extra/src"),
        ],
        &[],
    );
    p.reindex();
    let shown = p.ok(&["work", "show", "P"]);
    let detail = shown["work"]["containment"]["diagnostics"].to_string();
    assert!(
        detail.contains("child_outside_parent_scope") && detail.contains("C5"),
        "{detail}"
    );
    assert!(
        !detail.contains("C1 touches"),
        "a path inside the parent is fine: {detail}"
    );
}

#[test]
fn a_parent_without_paths_grants_no_scope_to_compare_against() {
    let p = Project::new(&governed(""));
    p.ledger_full(
        &[],
        &[("C1", "crates/anywhere/src"), ("C2", "crates/else/src")],
        &[],
    );
    p.reindex();
    assert_eq!(
        p.ok(&["work", "show", "P"])["work"]["containment"]["state"],
        "missing"
    );
    p.approve_set("lead", "approve");
    assert_eq!(
        p.ok(&["work", "show", "P"])["work"]["containment"]["state"],
        "approved"
    );
}

#[test]
fn changing_what_a_child_covers_makes_the_approval_stale_but_other_work_does_not() {
    let p = Project::new(&governed(""));
    p.approve_set("lead", "approve");
    let path = p.0.join("work.yaml");
    let edit = |change: &dyn Fn(&mut Value)| {
        let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        change(&mut ledger);
        fs::write(&path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
        p.reindex();
        p.ok(&["work", "show", "P"])["work"]["containment"]["state"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let at = |ledger: &mut Value, key: &str| -> usize {
        ledger["work_items"]
            .as_array()
            .unwrap()
            .iter()
            .position(|w| w["id"] == key)
            .unwrap()
    };
    // Work that is not a child of P is not part of what was judged.
    assert_eq!(
        edit(&|ledger| {
            let x = at(ledger, "X");
            ledger["work_items"][x]["acceptance"] = json!(["The buffer exists and is documented"]);
        }),
        "approved"
    );
    // Nor is a new dependency of work that is not a child.
    assert_eq!(
        edit(&|ledger| {
            ledger["work_items"].as_array_mut().unwrap().push(json!({
                "id": "Y", "title": "Unrelated follow-up", "status": "planned", "goal": "G",
                "acceptance": ["It follows the buffer"], "paths": [], "tags": [],
                "next_action": "Later", "depends_on": ["X"]
            }));
        }),
        "approved"
    );
    // A child's own paths, acceptance or dependencies are.
    assert_eq!(
        edit(&|ledger| {
            let c = at(ledger, "C2");
            ledger["work_items"][c]["paths"] = json!(["crates/storage/src/more"]);
        }),
        "stale"
    );
}

#[test]
fn a_project_without_a_policy_shows_no_containment_and_prepare_points_at_the_decision() {
    let plain = Project::without_policy();
    let shown = plain.ok(&["work", "show", "P"]);
    assert!(shown["work"].get("containment").is_none(), "{shown}");
    let p = Project::new(&governed(""));
    p.admit("C1");
    // The parent's prepare reports the set; the child's only blocker is the missing decision.
    let parent = p.ok(&["work", "prepare", "P", "--budget", "12000"]);
    assert_eq!(parent["containment"]["state"], "missing", "{parent}");
    let child = p.ok(&["work", "prepare", "C1", "--budget", "12000"]);
    assert_eq!(
        child["next_action"], "resolve_admission_before_claiming",
        "{child}"
    );
}

#[test]
fn a_proposal_is_answered_only_while_it_is_the_latest_and_describes_the_current_set() {
    let p = Project::new(&governed(""));
    let propose = |key: &str| {
        let (proposed, value) = p.contain_as(
            "lead",
            key,
            "proposed",
            json!({"judgment": Project::judgment()}),
        );
        assert!(proposed, "{value}");
        value["event"]["id"].clone()
    };
    let first = propose("first");
    // The children moved since it was proposed, so it no longer describes them.
    p.ledger(&["C1", "C2", "C4"], &[]);
    p.reindex();
    let (answered, value) = p.contain_as(
        "lead",
        "answer-first",
        "approved",
        json!({"proposal_event": first}),
    );
    assert!(!answered, "{value}");
    assert_eq!(value["details"]["reason"], "containment_stale", "{value}");
    // A newer proposal replaces it.
    let second = propose("second");
    let (answered, value) = p.contain_as(
        "lead",
        "answer-old",
        "approved",
        json!({"proposal_event": first}),
    );
    assert!(!answered, "{value}");
    assert_eq!(value["code"], "SourceConflict", "{value}");
    // The latest proposal can be answered, and its judgment is the one on record.
    let (approved, value) = p.contain_as(
        "lead",
        "answer-second",
        "approved",
        json!({"proposal_event": second}),
    );
    assert!(approved, "{value}");
    assert_eq!(value["containment"]["state"], "approved", "{value}");
    assert_eq!(value["event"]["payload"]["proposal_event"], second);
    assert_eq!(
        value["event"]["payload"]["judgment"]["reuse_checked"],
        Project::judgment()["reuse_checked"]
    );
    // An approval is not a proposal.
    let (answered, value) = p.contain_as(
        "lead",
        "answer-approval",
        "approved",
        json!({"proposal_event": value["event"]["id"]}),
    );
    assert!(!answered, "{value}");
    assert_eq!(value["code"], "InvalidInput", "{value}");
}

#[test]
fn archiving_a_child_removes_it_from_the_set_and_says_so() {
    let p = Project::new(&governed(""));
    p.approve_set("lead", "approve");
    let path = p.0.join("work.yaml");
    let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let c2 = ledger["work_items"]
        .as_array()
        .unwrap()
        .iter()
        .position(|w| w["id"] == "C2")
        .unwrap();
    ledger["work_items"][c2]["archived"] = json!(true);
    fs::write(&path, serde_json::to_vec_pretty(&ledger).unwrap()).unwrap();
    p.reindex();
    let shown = p.ok(&["work", "show", "P"]);
    assert_eq!(shown["work"]["containment"]["state"], "stale", "{shown}");
    assert_eq!(shown["work"]["containment"]["children"], json!(["C1"]));
    assert!(
        detail_of_block(&shown).contains("child_removed:C2"),
        "{shown}"
    );
}

fn detail_of_block(shown: &Value) -> String {
    shown["work"]["containment"]["diagnostics"][0]["detail"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_session_decides_as_its_own_agent_and_only_while_it_is_active() {
    let p = Project::new(&governed("supervisors=['lead']\n"));
    // The agent of the session decides who is authorized, not a name in the request.
    let intern = p.reviewer("intern");
    let (approved, value) = p.contain_by(
        json!({"session": intern}),
        "by-intern",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(!approved, "{value}");
    assert_eq!(
        value["details"]["reason"], "containment_not_authorized",
        "{value}"
    );
    let lead = p.reviewer("lead");
    let (approved, value) = p.contain_by(
        json!({"session": lead}),
        "by-lead",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(approved, "{value}");
    assert_eq!(value["event"]["payload"]["decider_agent"], "lead");
    assert_eq!(value["event"]["session_id"], json!(lead), "{value}");
    // A session that has ended decides nothing more.
    let revision = p.revision().to_string();
    p.ok(&[
        "session",
        "end",
        "--session",
        &lead,
        "--outcome",
        "ended",
        "--expected-revision",
        &revision,
    ]);
    let (approved, value) = p.contain_by(
        json!({"session": lead}),
        "after-end",
        "approved",
        json!({"judgment": Project::judgment()}),
    );
    assert!(!approved, "{value}");
    assert_eq!(value["code"], "InvalidTransition", "{value}");
}

#[test]
fn containment_is_recorded_only_for_a_governed_parent_that_has_children() {
    // The reviewed set is never reached in either case, so any well-formed fingerprint will do.
    let guess = format!("sha256:{}", "a".repeat(64));
    let decide = |p: &Project, parent: &str, key: &str| {
        let request = json!({"parent": parent, "agent": "lead", "expected_revision": p.revision(),
            "request_key": key, "decision": "approved", "change_set_fingerprint": guess,
            "judgment": Project::judgment()});
        p.record(key, &request)
    };
    let plain = Project::without_policy();
    let (recorded, value) = decide(&plain, "P", "ungoverned");
    assert!(!recorded, "{value}");
    assert_eq!(
        value["details"]["reason"], "admission_not_governed",
        "{value}"
    );
    let governed = Project::new(&governed(""));
    let (recorded, value) = decide(&governed, "C1", "no-children");
    assert!(!recorded, "{value}");
    assert_eq!(value["code"], "InvalidInput", "{value}");
}

#[test]
fn a_decision_computed_against_an_older_revision_is_a_stale_write() {
    let p = Project::new(&governed(""));
    let (recorded, value) = p.contain_as(
        "lead",
        "late",
        "approved",
        json!({"judgment": Project::judgment(), "expected_revision": p.revision() - 1}),
    );
    assert!(!recorded, "{value}");
    assert_eq!(value["code"], "RevisionConflict", "{value}");
    let shown = p.ok(&["work", "show", "P"]);
    assert_eq!(shown["work"]["containment"]["state"], "missing", "{shown}");
}
