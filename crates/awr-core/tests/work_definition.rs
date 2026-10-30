//! Positive and negative examples for the work definition model and its deterministic checker.
//! Each test name states the assertion; the examples come from the contract, not from the code.
use awr_core::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const ACC: &str = "Saving failure keeps the user's input";
const OUTCOME: &str = "Users keep their input when saving fails";
const PIN: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEWER_PIN: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn implementation() -> Value {
    json!({
        "version": 1,
        "purpose": "implementation",
        "tier": "required",
        "intent": [{"role": "user_decision", "decision": "D", "source_fingerprint": PIN,
                    "note": "The first release is local only"}],
        "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The failure path keeps the draft",
                         "by": "agent-a"}],
        "granularity": {"judgment": "independent_result", "reason": "One result a reviewer can judge",
                        "by": "agent-a"},
        "responsibility": {"name": "storage-module", "kind": "delivery"},
        "deliverable": "Local save with a failure fallback",
        "evidence_method": "Targeted tests compared before and after the change",
        "failure_recovery": "A failed save keeps the previous data and a retry is idempotent",
        "examples": [
            {"criterion": ACC, "kind": "positive", "given": "the storage is read-only",
             "when": "the user saves", "then": "the input is still shown and the failure is reported"},
            {"criterion": ACC, "kind": "negative", "given": "the storage is read-only",
             "when": "the user saves", "then": "success is never reported"}
        ],
        "discrimination": {"method": "fail_before_pass_after"}
    })
}

fn parse(value: &Value) -> WorkDefinition {
    serde_json::from_value(value.clone()).expect("a definition that matches the schema")
}

struct World {
    acceptance: Vec<String>,
    paths: Vec<String>,
    dependencies: Vec<String>,
    spec_refs: Vec<SpecificationReference>,
    dispositions: BTreeMap<(String, String), RequirementDisposition>,
    goals: BTreeMap<String, Vec<String>>,
    decisions: BTreeMap<String, String>,
    works: BTreeMap<String, DefinitionWork>,
}

impl World {
    fn new() -> Self {
        let mut world = Self {
            acceptance: vec![ACC.into()],
            paths: vec!["crates/storage/src".into()],
            dependencies: vec![],
            spec_refs: vec![],
            dispositions: BTreeMap::new(),
            goals: BTreeMap::from([("G".into(), vec![OUTCOME.into()])]),
            decisions: BTreeMap::from([("D".into(), PIN.into())]),
            works: BTreeMap::new(),
        };
        world.set("W", "api", WorkStatus::Ready, Some(implementation()));
        world
    }

    fn set(&mut self, key: &str, stream: &str, status: WorkStatus, definition: Option<Value>) {
        self.works.insert(
            key.into(),
            DefinitionWork {
                workstream: Some(stream.into()),
                status,
                definition: definition.map(|value| parse(&value)),
            },
        );
    }

    fn change(&mut self, edit: impl FnOnce(&mut Value)) {
        let mut value = implementation();
        edit(&mut value);
        self.set("W", "api", WorkStatus::Ready, Some(value));
    }

    fn report_for(&self, work: &str) -> DefinitionReport {
        check_work_definition(&DefinitionFacts {
            work,
            acceptance: &self.acceptance,
            paths: &self.paths,
            dependencies: &self.dependencies,
            spec_refs: &self.spec_refs,
            spec_dispositions: &self.dispositions,
            goals: &self.goals,
            decisions: &self.decisions,
            works: &self.works,
        })
    }

    fn report(&self) -> DefinitionReport {
        self.report_for("W")
    }
}

fn codes(report: &DefinitionReport) -> Vec<&'static str> {
    report.gaps.iter().map(|gap| gap.code).collect()
}

fn state<'a>(report: &'a DefinitionReport, obligation: &str) -> &'a str {
    report
        .obligations
        .iter()
        .find(|o| o.obligation == obligation)
        .unwrap_or_else(|| panic!("no obligation {obligation}"))
        .state
}

// ---------------------------------------------------------------- the model

#[test]
fn a_definition_that_meets_every_obligation_is_complete() {
    let report = World::new().report();
    assert_eq!(codes(&report), Vec::<&str>::new());
    assert_eq!(report.state, "complete");
    assert!(report.obligations.iter().all(|o| o.state != "missing"));
    assert_eq!(report.effective_tier, Some(DeliveryTier::Required));
}

#[test]
fn a_minimal_definition_validates_and_unknown_or_invalid_values_do_not() {
    let minimal = json!({"version": 1, "purpose": "research"});
    serde_json::from_value::<WorkDefinition>(minimal)
        .unwrap()
        .validate()
        .unwrap();
    parse(&implementation()).validate().unwrap();
    for (what, edit) in [
        ("an unknown field", json!({"mystery": true})),
        ("an unknown purpose", json!({"purpose": "wishful"})),
        ("an unknown tier", json!({"tier": "maybe"})),
        ("an unknown role", json!({"intent": [{"role": "hunch"}]})),
    ] {
        let mut value = implementation();
        value
            .as_object_mut()
            .unwrap()
            .extend(edit.as_object().unwrap().clone());
        assert!(
            serde_json::from_value::<WorkDefinition>(value).is_err(),
            "{what}"
        );
    }
    for (what, edit) in [
        ("a wrong version", json!({"version": 2})),
        (
            "a blank author",
            json!({"contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "c", "by": " "}]}),
        ),
        (
            "a blank claim",
            json!({"contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "", "by": "a"}]}),
        ),
        (
            "a missing judgment author",
            json!({"granularity": {"judgment": "independent_result", "reason": "r", "by": ""}}),
        ),
        ("a control character", json!({"deliverable": "bell\u{7}"})),
        (
            "an oversized text",
            json!({"deliverable": "x".repeat(4097)}),
        ),
    ] {
        let mut value = implementation();
        value
            .as_object_mut()
            .unwrap()
            .extend(edit.as_object().unwrap().clone());
        let definition = parse(&value);
        assert!(definition.validate().is_err(), "{what}");
    }
}

#[test]
fn every_list_is_bounded() {
    let intent = json!({"role": "assumption", "note": "Users save rarely"});
    let contribution = json!({"goal": "G", "outcome": OUTCOME, "claim": "c", "by": "agent-a"});
    let example =
        json!({"criterion": ACC, "kind": "positive", "given": "g", "when": "w", "then": "t"});
    for (field, entry) in [
        ("open_gaps", json!("a gap")),
        ("intent", intent),
        ("contributes", contribution),
        ("examples", example),
    ] {
        let mut value = implementation();
        value[field] = json!(vec![entry.clone(); 65]);
        assert!(parse(&value).validate().is_err(), "{field} with 65 entries");
        value[field] = json!(vec![entry; 64]);
        parse(&value)
            .validate()
            .unwrap_or_else(|e| panic!("{field} with 64 entries: {e}"));
    }
}

#[test]
fn adopted_roles_need_a_pinned_decision_while_unadopted_roles_do_not() {
    for (what, entry, ok) in [
        (
            "a user decision without a pin",
            json!({"role": "user_decision", "decision": "D"}),
            false,
        ),
        (
            "a sourced fact without any reference",
            json!({"role": "sourced_fact"}),
            false,
        ),
        (
            "a user decision with a malformed fingerprint",
            json!({"role": "user_decision", "decision": "D", "source_fingerprint": "sha256:abc"}),
            false,
        ),
        (
            "an assumption with only a pin half",
            json!({"role": "assumption", "decision": "D"}),
            false,
        ),
        (
            "an assumption in prose",
            json!({"role": "assumption", "note": "Users probably save rarely"}),
            true,
        ),
        (
            "a suggestion in prose",
            json!({"role": "suggestion", "note": "Consider an autosave"}),
            true,
        ),
        (
            "a sourced fact with a pin",
            json!({"role": "sourced_fact", "decision": "D", "source_fingerprint": PIN}),
            true,
        ),
    ] {
        let mut value = implementation();
        value["intent"] = json!([entry]);
        assert_eq!(parse(&value).validate().is_ok(), ok, "{what}");
    }
}

#[test]
fn secret_like_text_is_refused() {
    let mut value = implementation();
    value["deliverable"] = json!("password: hunter2hunter2");
    assert!(parse(&value).validate().is_err());
}

// ---------------------------------------------------------------- contribution and granularity

#[test]
fn a_contribution_names_a_visible_goal_and_one_of_its_exact_success_criteria() {
    assert_eq!(codes(&World::new().report()), Vec::<&str>::new());
    let mut missing_goal = World::new();
    missing_goal.goals.clear();
    assert!(codes(&missing_goal.report()).contains(&"contribution_goal_not_found"));
    let mut reworded = World::new();
    reworded
        .goals
        .insert("G".into(), vec!["Users keep their data".into()]);
    let report = reworded.report();
    assert!(codes(&report).contains(&"contribution_outcome_unknown"));
    assert_eq!(state(&report, "contribution"), "missing");
}

#[test]
fn a_work_without_any_contribution_is_untraced_and_a_step_needs_its_parent() {
    let mut world = World::new();
    world.change(|v| {
        v.as_object_mut().unwrap().remove("contributes");
        v["granularity"]["judgment"] = json!("step_of_parent");
    });
    let report = world.report();
    assert!(codes(&report).contains(&"contribution_missing"));
    assert!(codes(&report).contains(&"step_without_parent"));
}

// ---------------------------------------------------------------- parent and child

#[test]
fn a_parent_must_exist_be_another_work_in_the_same_workstream_and_not_loop_back() {
    let research = json!({"version": 1, "purpose": "research"});
    let mut ok = World::new();
    ok.set("P", "api", WorkStatus::Planned, Some(research.clone()));
    ok.change(|v| v["parent"] = json!("P"));
    assert_eq!(state(&ok.report(), "parent"), "satisfied");

    let mut absent = World::new();
    absent.change(|v| v["parent"] = json!("NOPE"));
    assert!(codes(&absent.report()).contains(&"parent_not_found"));
    // An inconsistent relation is never reported as established.
    assert_eq!(state(&absent.report(), "parent"), "unknown");

    let mut itself = World::new();
    itself.change(|v| v["parent"] = json!("W"));
    assert!(codes(&itself.report()).contains(&"parent_self"));
    assert_eq!(state(&itself.report(), "parent"), "unknown");

    let mut other_stream = World::new();
    other_stream.set("P", "ops", WorkStatus::Planned, Some(research.clone()));
    other_stream.change(|v| v["parent"] = json!("P"));
    assert!(codes(&other_stream.report()).contains(&"parent_cross_workstream"));
    assert_eq!(state(&other_stream.report(), "parent"), "unknown");

    let mut looped = World::new();
    looped.set(
        "P",
        "api",
        WorkStatus::Planned,
        Some(json!({"version": 1, "purpose": "research", "parent": "W"})),
    );
    looped.change(|v| v["parent"] = json!("P"));
    assert!(codes(&looped.report()).contains(&"parent_cycle"));
    assert_eq!(state(&looped.report(), "parent"), "unknown");
}

#[test]
fn finished_children_never_prove_the_parents_own_outcome() {
    let research = json!({"version": 1, "purpose": "research"});
    let child = json!({"version": 1, "purpose": "research", "parent": "P"});
    let mut world = World::new();
    world.set("P", "api", WorkStatus::InProgress, Some(research.clone()));
    world.set("C1", "api", WorkStatus::Completed, Some(child.clone()));
    world.set("C2", "api", WorkStatus::Completed, Some(child.clone()));
    let report = world.report_for("P");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.code == "parent_outcome_unproven")
    );
    // A note never turns the parent's own definition into one with gaps or into a finished one.
    assert!(
        !report
            .gaps
            .iter()
            .any(|g| g.code == "parent_outcome_unproven")
    );
    // One unfinished child, or a finished parent, removes the note.
    world.set("C2", "api", WorkStatus::InProgress, Some(child));
    assert!(world.report_for("P").notes.is_empty());
    world.set(
        "C2",
        "api",
        WorkStatus::Completed,
        world.works["C2"]
            .definition
            .as_ref()
            .map(|d| serde_json::to_value(d).unwrap()),
    );
    world.set("P", "api", WorkStatus::Completed, Some(research));
    assert!(world.report_for("P").notes.is_empty());
}

// ---------------------------------------------------------------- intent

#[test]
fn intent_roles_are_told_apart_and_adopted_references_must_be_current() {
    let mut stale = World::new();
    stale.decisions.insert("D".into(), NEWER_PIN.into());
    let report = stale.report();
    assert!(codes(&report).contains(&"intent_ref_stale"));
    assert_eq!(state(&report, "intent"), "missing");

    let mut unknown = World::new();
    unknown.decisions.clear();
    assert!(codes(&unknown.report()).contains(&"intent_ref_not_found"));

    // Assumptions and suggestions are kept but are never adopted facts.
    let mut only_unadopted = World::new();
    only_unadopted.change(|v| {
        v["intent"] = json!([
            {"role": "assumption", "note": "Users save rarely"},
            {"role": "suggestion", "note": "Consider autosave"}
        ]);
    });
    let report = only_unadopted.report();
    assert!(codes(&report).contains(&"intent_missing"));

    // All four roles together are accepted and the pinned one carries the obligation.
    let mut all_roles = World::new();
    all_roles.change(|v| {
        v["intent"] = json!([
            {"role": "user_decision", "decision": "D", "source_fingerprint": PIN},
            {"role": "sourced_fact", "decision": "D", "source_fingerprint": PIN},
            {"role": "assumption", "note": "Users save rarely"},
            {"role": "suggestion", "note": "Consider autosave"}
        ]);
        v["open_gaps"] = json!(["Offline behavior is undecided"]);
        v["adoption"] =
            json!({"basis": "Scope confirmed in the planning thread", "by": "person-1"});
    });
    let report = all_roles.report();
    assert_eq!(codes(&report), Vec::<&str>::new());
    assert_eq!(state(&report, "intent"), "satisfied");
}

// ---------------------------------------------------------------- obligations by purpose

#[test]
fn an_implementation_needs_a_named_responsibility_that_no_bare_link_replaces() {
    let mut world = World::new();
    world.change(|v| {
        v.as_object_mut().unwrap().remove("responsibility");
    });
    let report = world.report();
    assert!(codes(&report).contains(&"responsibility_missing"));
    assert_eq!(state(&report, "responsibility"), "missing");
}

#[test]
fn each_missing_implementation_obligation_is_reported_under_its_own_code() {
    for (field, code) in [
        ("deliverable", "deliverable_missing"),
        ("evidence_method", "evidence_method_missing"),
        ("failure_recovery", "failure_recovery_missing"),
        ("granularity", "granularity_missing"),
    ] {
        let mut world = World::new();
        world.change(|v| {
            v.as_object_mut().unwrap().remove(field);
        });
        assert!(codes(&world.report()).contains(&code), "{field}");
    }
    let mut no_scope = World::new();
    no_scope.paths.clear();
    assert!(codes(&no_scope.report()).contains(&"scope_missing"));
    let mut unversioned = World::new();
    unversioned.change(|v| {
        v.as_object_mut().unwrap().remove("intent");
    });
    let codes_found = codes(&unversioned.report());
    assert!(codes_found.contains(&"inputs_unversioned"));
    assert!(codes_found.contains(&"intent_missing"));
}

#[test]
fn research_and_documents_only_owe_what_their_purpose_needs() {
    for purpose in ["research", "document"] {
        let mut world = World::new();
        world.change(|v| {
            *v = json!({
                "version": 1, "purpose": purpose,
                "intent": v["intent"].clone(), "contributes": v["contributes"].clone(),
                "granularity": v["granularity"].clone(), "deliverable": "An answer with its limits"
            });
        });
        let report = world.report();
        assert_eq!(codes(&report), Vec::<&str>::new(), "{purpose}");
        assert_eq!(state(&report, "assertions"), "satisfied", "{purpose}");
        for obligation in [
            "responsibility",
            "examples",
            "discrimination",
            "evidence_method",
            "failure_recovery",
        ] {
            assert_eq!(
                state(&report, obligation),
                "not_applicable",
                "{purpose} {obligation}"
            );
        }
    }
    // The same sparse definition is not enough once the work is an implementation.
    let mut sparse = World::new();
    sparse.change(|v| *v = json!({"version": 1, "purpose": "implementation"}));
    let report = sparse.report();
    assert_eq!(report.state, "gaps");
    for code in [
        "responsibility_missing",
        "examples_missing",
        "discrimination_missing",
        "deliverable_missing",
        "intent_missing",
    ] {
        assert!(codes(&report).contains(&code), "{code}");
    }
}

#[test]
fn observable_assertions_are_the_acceptance_criteria_and_an_implementation_needs_one() {
    let mut world = World::new();
    let report = world.report();
    assert_eq!(state(&report, "assertions"), "satisfied");
    let row = report
        .obligations
        .iter()
        .find(|o| o.obligation == "assertions")
        .unwrap();
    assert_eq!(row.source.as_deref(), Some("acceptance"));

    world.acceptance.clear();
    let report = world.report();
    assert!(codes(&report).contains(&"acceptance_missing"));
    assert_eq!(state(&report, "assertions"), "missing");

    // Research states what it will deliver and owes no acceptance criterion of its own.
    world.change(|v| {
        *v = json!({
            "version": 1, "purpose": "research", "intent": v["intent"].clone(),
            "contributes": v["contributes"].clone(), "granularity": v["granularity"].clone(),
            "deliverable": "An answer with its limits"
        });
    });
    let report = world.report();
    assert!(!codes(&report).contains(&"acceptance_missing"));
    assert_eq!(state(&report, "assertions"), "not_applicable");
}

#[test]
fn prerequisites_come_from_dependency_links_and_having_none_is_never_a_gap() {
    let mut world = World::new();
    let alone = world.report();
    assert_eq!(state(&alone, "prerequisites"), "not_applicable");
    assert_eq!(alone.state, "complete");

    world.dependencies = vec!["P".into()];
    let report = world.report();
    assert_eq!(state(&report, "prerequisites"), "satisfied");
    let row = report
        .obligations
        .iter()
        .find(|o| o.obligation == "prerequisites")
        .unwrap();
    assert_eq!(row.source.as_deref(), Some("dependencies"));
    assert_eq!(report.state, "complete");
}

#[test]
fn research_still_owes_a_contribution_and_adopted_intent() {
    let mut world = World::new();
    world.change(|v| {
        *v = json!({"version": 1, "purpose": "research", "deliverable": "An answer with its limits",
                    "granularity": v["granularity"].clone()});
    });
    let found = codes(&world.report());
    assert!(found.contains(&"contribution_missing"), "{found:?}");
    assert!(found.contains(&"intent_missing"), "{found:?}");
}

#[test]
fn a_decision_needs_adopted_intent_and_a_contribution_but_no_implementation_detail() {
    let mut world = World::new();
    world.change(|v| {
        *v = json!({
            "version": 1, "purpose": "decision",
            "intent": v["intent"].clone(), "contributes": v["contributes"].clone(),
            "deliverable": "A recorded choice", "open_gaps": ["Cost is unknown"]
        });
    });
    assert_eq!(codes(&world.report()), Vec::<&str>::new());
}

// ---------------------------------------------------------------- inheritance

#[test]
fn a_child_inherits_current_obligations_and_loses_them_when_the_parent_goes_stale() {
    let child = json!({
        "version": 1, "purpose": "implementation", "inherits": "W",
        "contributes": [{"goal": "G", "outcome": OUTCOME, "claim": "The child covers the retry path", "by": "agent-b"}],
        "granularity": {"judgment": "independent_result", "reason": "Separate retry result", "by": "agent-b"},
        "deliverable": "Retry handling",
        "examples": [
            {"criterion": ACC, "kind": "positive", "given": "g", "when": "w", "then": "t"},
            {"criterion": ACC, "kind": "negative", "given": "g", "when": "w", "then": "t2"}
        ]
    });
    let mut world = World::new();
    world.set("C", "api", WorkStatus::Ready, Some(child.clone()));
    let report = world.report_for("C");
    assert_eq!(codes(&report), Vec::<&str>::new());
    let intent = report
        .obligations
        .iter()
        .find(|o| o.obligation == "intent")
        .unwrap();
    assert_eq!(
        (intent.state, intent.source.as_deref()),
        ("satisfied", Some("inherited:W"))
    );
    let responsibility = report
        .obligations
        .iter()
        .find(|o| o.obligation == "responsibility")
        .unwrap();
    assert_eq!(responsibility.source.as_deref(), Some("inherited:W"));

    // The inherited intent only counts while the parent's references are current.
    world.decisions.insert("D".into(), NEWER_PIN.into());
    let report = world.report_for("C");
    assert!(codes(&report).contains(&"intent_missing"));
    assert_eq!(state(&report, "intent"), "missing");

    let mut orphan = World::new();
    orphan.set(
        "C",
        "api",
        WorkStatus::Ready,
        Some(json!({"version": 1, "purpose": "implementation", "inherits": "NOPE"})),
    );
    assert!(codes(&orphan.report_for("C")).contains(&"inherits_not_found"));
}

// ---------------------------------------------------------------- tiers, examples, discrimination

#[test]
fn later_work_never_owes_examples_or_a_discrimination_method() {
    let mut world = World::new();
    world.change(|v| {
        v["tier"] = json!("later");
        v.as_object_mut().unwrap().remove("examples");
        v.as_object_mut().unwrap().remove("discrimination");
    });
    let report = world.report();
    assert_eq!(codes(&report), Vec::<&str>::new());
    assert_eq!(report.effective_tier, Some(DeliveryTier::Later));
    assert_eq!(state(&report, "examples"), "not_applicable");
    assert_eq!(state(&report, "discrimination"), "not_applicable");
}

#[test]
fn every_required_criterion_needs_a_positive_and_a_negative_example_and_a_discrimination_method() {
    let mut only_positive = World::new();
    only_positive.change(|v| {
        v["examples"].as_array_mut().unwrap().pop();
    });
    let report = only_positive.report();
    assert!(codes(&report).contains(&"examples_missing"));
    assert_eq!(state(&report, "examples"), "missing");

    let mut prose_only = World::new();
    prose_only.change(|v| {
        v.as_object_mut().unwrap().remove("examples");
    });
    assert!(codes(&prose_only.report()).contains(&"examples_missing"));

    let mut no_method = World::new();
    no_method.change(|v| {
        v.as_object_mut().unwrap().remove("discrimination");
    });
    assert!(codes(&no_method.report()).contains(&"discrimination_missing"));

    let mut elsewhere = World::new();
    elsewhere.change(|v| v["examples"][0]["criterion"] = json!("Some other criterion"));
    let report = elsewhere.report();
    assert!(codes(&report).contains(&"example_criterion_unknown"));
    assert!(codes(&report).contains(&"examples_missing"));

    let mut second = World::new();
    second.acceptance.push("A second criterion".into());
    let report = second.report();
    let detail = &report
        .gaps
        .iter()
        .find(|g| g.code == "examples_missing")
        .unwrap()
        .detail;
    assert!(detail.contains("A second criterion") && !detail.contains(ACC));
}

#[test]
fn a_future_specification_item_makes_its_criterion_later_and_a_required_tier_must_agree() {
    let reference = SpecificationReference {
        specification: "spec".into(),
        item: "EXPORT".into(),
        source_fingerprint: PIN.into(),
        criterion: Some(ACC.into()),
    };
    let mut world = World::new();
    world.spec_refs = vec![reference];
    world.dispositions.insert(
        ("spec".into(), "EXPORT".into()),
        RequirementDisposition::Future,
    );
    // Without an explicit tier the future disposition decides: nothing is owed for this increment.
    world.change(|v| {
        v.as_object_mut().unwrap().remove("tier");
        v.as_object_mut().unwrap().remove("examples");
        v.as_object_mut().unwrap().remove("discrimination");
    });
    let report = world.report();
    assert_eq!(report.effective_tier, Some(DeliveryTier::Later));
    assert_eq!(codes(&report), Vec::<&str>::new());
    // An explicit required tier contradicts a specification that defers every item it references.
    world.change(|v| {
        v["tier"] = json!("required");
    });
    assert!(codes(&world.report()).contains(&"tier_conflicts_with_specification"));
    // Required items keep their obligation.
    world.dispositions.insert(
        ("spec".into(), "EXPORT".into()),
        RequirementDisposition::Required,
    );
    world.change(|v| {
        v.as_object_mut().unwrap().remove("examples");
    });
    assert!(codes(&world.report()).contains(&"examples_missing"));

    // A criterion bound to one deferred and one required item is still owed in this increment.
    let reference = |item: &str| SpecificationReference {
        specification: "spec".into(),
        item: item.into(),
        source_fingerprint: PIN.into(),
        criterion: Some(ACC.into()),
    };
    let mut mixed = World::new();
    mixed.spec_refs = vec![reference("EXPORT"), reference("SAVE")];
    mixed.dispositions.insert(
        ("spec".into(), "EXPORT".into()),
        RequirementDisposition::Future,
    );
    mixed.dispositions.insert(
        ("spec".into(), "SAVE".into()),
        RequirementDisposition::Required,
    );
    mixed.change(|v| {
        for field in ["tier", "examples", "discrimination"] {
            v.as_object_mut().unwrap().remove(field);
        }
    });
    let report = mixed.report();
    assert_eq!(report.effective_tier, Some(DeliveryTier::Required));
    assert!(codes(&report).contains(&"examples_missing"));
}

// ---------------------------------------------------------------- compatibility

#[test]
fn a_work_without_a_definition_reports_only_that_nothing_is_declared() {
    let mut world = World::new();
    world.set("W", "api", WorkStatus::Ready, None);
    let report = world.report();
    assert_eq!(report.state, "not_declared");
    assert!(report.obligations.is_empty() && report.gaps.is_empty() && report.notes.is_empty());
    assert_eq!(report.purpose, None);
}

#[test]
fn stored_work_without_a_definition_still_decodes_and_never_gains_the_field() {
    // Payloads written before the field existed carry no `definition` key.
    let mut stored = serde_json::to_value(sample_work(None)).unwrap();
    assert!(
        stored.get("definition").is_none(),
        "an absent definition is not serialized"
    );
    stored.as_object_mut().unwrap().remove("definition");
    let decoded: WorkItem = serde_json::from_value(stored.clone()).unwrap();
    assert!(decoded.definition.is_none());
    assert_eq!(serde_json::to_value(&decoded).unwrap(), stored);
    // A declared definition round-trips.
    let with = sample_work(Some(parse(&implementation())));
    let again: WorkItem = serde_json::from_value(serde_json::to_value(&with).unwrap()).unwrap();
    assert_eq!(again.definition, with.definition);
}

fn sample_work(definition: Option<WorkDefinition>) -> WorkItem {
    WorkItem {
        archived: false,
        ordinary_completion: None,
        meta: ProjectionMeta {
            id: Id::new(),
            external_key: "W".into(),
            revision: 1,
            source_ref: SourceRef {
                source_id: Id::new(),
                locator: "file:///work.yaml".into(),
                source_revision: 1,
                source_fingerprint: PIN.into(),
                pointer: None,
                start_line: None,
                end_line: None,
                section_fingerprint: None,
            },
        },
        title: "Work".into(),
        kind: None,
        owner: None,
        required: true,
        raw_status: "ready".into(),
        status: WorkStatus::Ready,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: String::new(),
        next_action: String::new(),
        blocker: None,
        acceptance: vec![ACC.into()],
        tags: vec![],
        paths: vec![],
        spec_refs: vec![],
        definition,
    }
}

// ---------------------------------------------------------------- side lines and outlines

#[test]
fn a_side_line_names_the_line_it_returns_to_and_says_when_it_returns_and_stops() {
    let research = json!({"version": 1, "purpose": "research"});
    let mut world = World::new();
    world.set("P", "api", WorkStatus::Planned, Some(research));
    world.change(|v| {
        v["parent"] = json!("P");
        v["side_line"] = json!({"return_condition": "The failure modes are listed and reviewed",
                                "stop_condition": "Two days of effort or a decision to drop the screen"});
    });
    let report = world.report();
    assert_eq!(codes(&report), Vec::<&str>::new(), "{report:?}");
    assert_eq!(state(&report, "side_line"), "satisfied");

    // Each missing piece is its own explicit gap.
    for (edit, code) in [
        (
            (|v: &mut Value| v["side_line"]["return_condition"] = Value::Null) as fn(&mut Value),
            "side_line_return_missing",
        ),
        (
            |v| v["side_line"]["stop_condition"] = Value::Null,
            "side_line_stop_missing",
        ),
        (|v| v["side_line"] = json!({}), "side_line_return_missing"),
    ] {
        let mut gap = World::new();
        gap.set(
            "P",
            "api",
            WorkStatus::Planned,
            Some(json!({"version": 1, "purpose": "research"})),
        );
        gap.change(|v| {
            v["parent"] = json!("P");
            v["side_line"] = json!({"return_condition": "Reviewed", "stop_condition": "Two days"});
            edit(v);
        });
        let report = gap.report();
        assert!(
            codes(&report).contains(&code),
            "{code}: {:?}",
            codes(&report)
        );
        assert_eq!(state(&report, "side_line"), "missing");
    }
    // Without a parent there is no line to return to.
    let mut orphan = World::new();
    orphan.change(|v| {
        v["side_line"] = json!({"return_condition": "Reviewed", "stop_condition": "Two days"})
    });
    assert!(codes(&orphan.report()).contains(&"side_line_without_parent"));
    // Work that is not a side line carries no such row.
    assert!(
        World::new()
            .report()
            .obligations
            .iter()
            .all(|o| o.obligation != "side_line")
    );
    // Blank or oversized conditions are refused when the definition is read.
    let mut blank = implementation();
    blank["side_line"] = json!({"return_condition": " ", "stop_condition": "Two days"});
    assert!(parse(&blank).validate().is_err());
}

#[test]
fn an_outline_keeps_what_is_known_and_a_trigger_and_is_never_executable() {
    // Distant work owes its purpose, the outcome it contributes to and a trigger, nothing else.
    let mut sparse = World::new();
    sparse.change(|v| {
        *v = json!({"version": 1, "purpose": "implementation",
                    "contributes": v["contributes"].clone(),
                    "outline": {"constraints": ["No new storage layer"], "trigger": "When the API contract is accepted"}});
    });
    let report = sparse.report();
    assert_eq!(codes(&report), ["outline_not_elaborated"], "{report:?}");
    assert_eq!(state(&report, "outline"), "satisfied");
    for obligation in [
        "responsibility",
        "examples",
        "discrimination",
        "evidence_method",
        "failure_recovery",
        "intent",
    ] {
        assert_ne!(
            state(&report, obligation),
            "missing",
            "{obligation} is not owed by an outline"
        );
    }
    // The trigger is what makes an outline elaborable; without one it is an explicit gap.
    let mut no_trigger = World::new();
    no_trigger.change(|v| {
        *v = json!({"version": 1, "purpose": "implementation", "contributes": v["contributes"].clone(),
                    "outline": {"constraints": ["No new storage layer"]}});
    });
    let found = codes(&no_trigger.report());
    assert!(
        found.contains(&"outline_trigger_missing") && found.contains(&"outline_not_elaborated"),
        "{found:?}"
    );
    // An outline still says which outcome it advances.
    let mut untraced = World::new();
    untraced.change(|v| {
        *v = json!({"version": 1, "purpose": "implementation", "outline": {"trigger": "When the API is accepted"}});
    });
    assert!(codes(&untraced.report()).contains(&"contribution_missing"));
    // Elaborating it, that is dropping the outline, restores the full obligations of the purpose.
    let full = World::new().report();
    assert_eq!(codes(&full), Vec::<&str>::new());
    // Bounds.
    let mut too_many = implementation();
    too_many["outline"] = json!({"constraints": (0..65).map(|i| format!("constraint {i}")).collect::<Vec<_>>(), "trigger": "Later"});
    assert!(parse(&too_many).validate().is_err());
    let mut blank = implementation();
    blank["outline"] = json!({"constraints": [" "], "trigger": "Later"});
    assert!(parse(&blank).validate().is_err());
}

#[test]
fn a_large_work_declares_its_own_rework_budget_and_other_sizes_owe_nothing() {
    let report = |edit: fn(&mut Value)| {
        let mut world = World::new();
        world.change(edit);
        world.report()
    };
    let large = report(|v| v["size"] = json!("large"));
    assert!(
        codes(&large).contains(&"large_work_budget_missing"),
        "{large:?}"
    );
    // Declaring the effort alone is not declaring the rework budget.
    let effort_only = report(|v| {
        v["size"] = json!("large");
        v["budget"] = json!({"active_effort_minutes": 2000});
    });
    assert!(codes(&effort_only).contains(&"large_work_budget_missing"));
    let declared = report(|v| {
        v["size"] = json!("large");
        v["budget"] = json!({"rework_cycles": 4});
    });
    assert!(
        !codes(&declared).contains(&"large_work_budget_missing"),
        "{declared:?}"
    );
    for size in ["small", "medium"] {
        let value = json!(size);
        let mut world = World::new();
        world.change(|v| v["size"] = value.clone());
        assert!(
            !codes(&world.report()).contains(&"large_work_budget_missing"),
            "{size}"
        );
    }
    assert!(!codes(&World::new().report()).contains(&"large_work_budget_missing"));
}
