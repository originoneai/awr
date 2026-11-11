//! A work definition through the real CLI: reported by `work show` and `work prepare` only when it
//! is declared, checked against the current goals and decisions, and bound into the management
//! contract fingerprint without disturbing work that declares none.
use awr_core::Id;
use serde_json::{Value, json};
use sha2::Digest;
use std::{fs, path::PathBuf, process::Command};

const ACC: &str = "Saving failure keeps the user's input";
const OUTCOME: &str = "Users keep their input when saving fails";

fn governance_help() -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_awr"))
        .env("AWR_LOCAL_WEB", "0")
        .args(["work", "help", "governance"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

struct GapHint<'a> {
    codes: Vec<&'a str>,
    path: &'a str,
    value: Value,
    note: &'a str,
}

fn gap_hints(text: &str) -> Vec<GapHint<'_>> {
    text.lines()
        .filter_map(|line| {
            let (codes, rest) = line.split_once(" | ")?;
            let (yaml, note) = rest.split_once(" | ")?;
            let (path, shape) = yaml.split_once(": ")?;
            if !codes
                .bytes()
                .all(|c| c.is_ascii_lowercase() || b"_,".contains(&c))
            {
                return None;
            }
            Some(GapHint {
                codes: codes.split(',').collect(),
                path,
                value: serde_yaml_ng::from_str(shape).expect("a one-line YAML value"),
                note,
            })
        })
        .collect()
}

// Read emitted codes from the checker itself, including conditional reference/graph gaps.
// A new diagnostic is checked without copying the checker's list into this test.
fn checker_gap_codes(source: &str) -> std::collections::BTreeSet<&str> {
    let quoted: Vec<_> = source.split('"').skip(1).collect();
    quoted
        .chunks_exact(2)
        .filter(|pair| {
            pair[0].contains('_')
                && pair[0].bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
                && matches!(
                    pair[1].split_whitespace().collect::<String>().as_str(),
                    "," | ",format!("
                )
        })
        .map(|pair| pair[0])
        .collect()
}

fn uncovered_codes<'a>(
    codes: impl IntoIterator<Item = &'a str>,
    hints: &[GapHint<'_>],
) -> Vec<&'a str> {
    codes
        .into_iter()
        .filter(|code| !hints.iter().any(|h| h.codes.contains(code)))
        .collect()
}

fn filled_hint(value: &Value, pin: &str) -> Value {
    match value {
        Value::String(text) => json!(
            text.replace("<sha256>", pin.strip_prefix("sha256:").unwrap())
                .replace("<decision>", "scope")
                .replace("<goal>", "G")
                .replace("<outcome>", OUTCOME)
                .replace("<criterion>", ACC)
                .replace("<path>", "crates/storage/src")
                .replace("<id>", "result")
                .replace(['<', '>'], "")
        ),
        Value::Array(values) => Value::Array(values.iter().map(|v| filled_hint(v, pin)).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), filled_hint(v, pin)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn set_hint(work: &mut Value, path: &str, value: Value) {
    let path = path.replace("[0]", ".0");
    let mut parts = path.split('.').peekable();
    let mut target = work;
    while let Some(part) = parts.next() {
        if let Ok(index) = part.parse::<usize>() {
            target = &mut target[index];
        } else {
            target = &mut target[part];
        }
        if parts.peek().is_none() {
            *target = value;
            break;
        }
    }
}

fn parse_hint_work(work: Value) -> awr_core::Result<awr_core::ProjectionBatch> {
    use awr_source::{Manifest, ParseContext, SourceAdapter, SourceSnapshot, YamlLedgerAdapter};
    let manifest = Manifest::parse(
        "[project]\nname='Hint fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n",
    )?;
    let source = awr_core::Source {
        id: Id::new(),
        project_id: Id::new(),
        domain: "ledger".into(),
        role: "primary".into(),
        locator: "file:///fixture/work.yaml".into(),
        format: "yaml".into(),
        adapter: "yaml-ledger-v1".into(),
        revision: 1,
        fingerprint: String::new(),
        freshness: awr_core::Freshness::Fresh,
        config: json!({}),
    };
    let bytes = serde_json::to_vec(&json!({"work_items": [work]})).unwrap();
    let snapshot = SourceSnapshot {
        content_review: None,
        locator: source.locator.clone(),
        fingerprint: awr_source::fingerprint(&bytes),
        bytes,
    };
    YamlLedgerAdapter.parse(
        &snapshot,
        &ParseContext {
            source: &source,
            existing_ids: Default::default(),
        },
        &manifest.sources[0],
    )
}

#[test]
fn gap_hints_ac1_every_checker_code_has_a_field_and_yaml_value() {
    let text = governance_help();
    let hints = gap_hints(&text);
    let source = include_str!("../../awr-core/src/work_definition.rs");
    let source = source.split_once("fn intent_problems").unwrap().1;
    let codes = checker_gap_codes(source);
    assert!(
        codes.len() >= 30,
        "checker extraction lost diagnostics: {codes:?}"
    );
    assert_eq!(
        uncovered_codes(codes.iter().copied(), &hints),
        Vec::<&str>::new()
    );
    for code in codes {
        let rows: Vec<_> = hints.iter().filter(|h| h.codes.contains(&code)).collect();
        assert_eq!(rows.len(), 1, "one repair row for {code}");
        assert!(!rows[0].path.is_empty());
        assert!(!rows[0].note.is_empty() && rows[0].note.len() <= 100);
    }
}

#[test]
fn gap_hints_ac1_new_checker_code_without_a_row_is_detected() {
    let text = governance_help();
    let hints = gap_hints(&text);
    let changed_checker = "run.gap(\"new_definition_gap\", \"A new obligation\");";
    assert_eq!(
        uncovered_codes(checker_gap_codes(changed_checker), &hints),
        ["new_definition_gap"]
    );
}

#[test]
fn gap_hints_ac2_every_field_and_value_is_accepted_by_the_source_parser() {
    let p = Project::new();
    let text = governance_help();
    let hints = gap_hints(&text);
    assert!(!hints.is_empty(), "missing gap field hints");
    for hint in hints {
        let mut work = json!({"id":"W", "title":"Save safely", "status":"planned", "acceptance":[ACC], "definition": p.definition("Local save")});
        set_hint(&mut work, hint.path, filled_hint(&hint.value, &p.pin()));
        parse_hint_work(work).unwrap_or_else(|e| panic!("{}: {e}", hint.path));
    }
}

#[test]
fn gap_hints_ac2_unknown_field_in_a_hint_is_rejected() {
    let p = Project::new();
    let text = governance_help();
    let hints = gap_hints(&text);
    let hint = hints
        .iter()
        .find(|h| h.codes.contains(&"responsibility_missing"))
        .expect("responsibility hint");
    let mut work = json!({"id":"W", "title":"Save safely", "status":"planned", "definition":p.definition("Local save")});
    set_hint(
        &mut work,
        &hint.path.replace("responsibility", "responsibilty"),
        filled_hint(&hint.value, &p.pin()),
    );
    assert!(
        parse_hint_work(work).is_err(),
        "an invented field must fail parsing"
    );
}

fn definition_from_hints(p: &Project) -> Value {
    let text = governance_help();
    let yaml = text
        .split_once("```yaml\n")
        .unwrap()
        .1
        .split_once("```")
        .unwrap()
        .0;
    let mut work: Value = serde_yaml_ng::from_str(yaml).unwrap();
    work["paths"] = json!([]);
    let hints = gap_hints(&text);
    for _ in 0..20 {
        p.ledger_with(
            &work["definition"],
            "Review the contract",
            json!({"paths": work["paths"]}),
        );
        p.reindex();
        let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
        let codes = gap_codes(&report);
        if codes.is_empty() {
            return work;
        }
        for code in codes {
            let hint = hints
                .iter()
                .find(|h| h.codes.contains(&code.as_str()))
                .unwrap_or_else(|| panic!("no field hint for {code}"));
            let mut value = filled_hint(&hint.value, &p.pin());
            if hint.path == "definition.examples" {
                assert!(hint.note.contains("negative"), "explain both example kinds");
                let mut negative = value[0].clone();
                negative["kind"] = json!("negative");
                negative["then"] = json!("Success is withheld");
                value.as_array_mut().unwrap().push(negative);
            }
            set_hint(&mut work, hint.path, value);
        }
    }
    panic!("the hints did not clear the definition gaps");
}

#[test]
fn gap_hints_ac3_filling_reported_fields_clears_work_show_gaps() {
    let p = Project::new();
    let work = definition_from_hints(&p);
    assert_eq!(
        work["definition"]["intent"][0]["source_fingerprint"],
        p.pin()
    );
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(report["state"], "complete");
    assert!(gap_codes(&report).is_empty());
}

#[test]
fn gap_hints_ac3_omitting_one_field_reports_exactly_its_mapped_gap() {
    let p = Project::new();
    let complete = definition_from_hints(&p);
    let text = governance_help();
    let hints = gap_hints(&text);
    for (field, code) in [
        ("granularity", "granularity_missing"),
        ("responsibility", "responsibility_missing"),
        ("deliverable", "deliverable_missing"),
        ("evidence_method", "evidence_method_missing"),
        ("failure_recovery", "failure_recovery_missing"),
        ("discrimination", "discrimination_missing"),
    ] {
        let mut sparse = complete["definition"].clone();
        sparse.as_object_mut().unwrap().remove(field);
        p.ledger(&sparse, "Repair the missing field");
        p.reindex();
        let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
        assert_eq!(gap_codes(&report), [code], "omitted {field}: {report}");
        assert_eq!(
            hints.iter().find(|h| h.codes.contains(&code)).unwrap().path,
            format!("definition.{field}")
        );
    }
}

fn author_from_help(include_front_matter: bool) -> (Project, Value, Vec<String>) {
    let text = governance_help();
    let hints = gap_hints(&text);
    let decision_template = text
        .split_once("```markdown\n")
        .expect("help must show the registered decision file format")
        .1
        .split_once("```")
        .unwrap()
        .0;
    assert!(decision_template.lines().count() <= 4);
    assert!(text.contains("registered decisions/ source"));
    assert!(text.contains("shasum -a 256 <decision file>"));
    let mut decision = decision_template
        .replace("<decision>", "scope")
        .replace("<decision text>", "The first release is local only.");
    if !include_front_matter {
        let delimiter = decision.lines().next().unwrap();
        decision = decision.splitn(3, delimiter).last().unwrap().to_owned();
    }
    let p = Project::uninitialized();
    assert_eq!(fs::read_dir(p.0.join("decisions")).unwrap().count(), 0);
    fs::write(p.0.join("decisions/scope.md"), &decision).unwrap();
    let digest = format!("{:x}", sha2::Sha256::digest(decision.as_bytes()));
    let intent = hints
        .iter()
        .find(|h| h.codes.contains(&"intent_missing"))
        .unwrap();
    let pin = intent.value[0]["source_fingerprint"]
        .as_str()
        .unwrap()
        .replace("<sha256>", &digest);
    let examples = hints
        .iter()
        .find(|h| h.codes.contains(&"examples_missing"))
        .unwrap();
    assert!(
        examples.note.starts_with("Each acceptance criterion"),
        "{}",
        examples.note
    );
    let kinds: Vec<awr_core::ExampleKind> = examples
        .note
        .split_whitespace()
        .filter_map(|word| {
            serde_json::from_value(json!(word.trim_matches(|c: char| !c.is_ascii_alphabetic())))
                .ok()
        })
        .collect();
    assert_eq!(
        kinds.len(),
        2,
        "explain both example kinds: {}",
        examples.note
    );
    let acceptance = vec![
        ACC.to_string(),
        "A successful save shows confirmation".to_string(),
    ];
    let yaml = text
        .split_once("```yaml\n")
        .unwrap()
        .1
        .split_once("```")
        .unwrap()
        .0;
    let mut work: Value = serde_yaml_ng::from_str(yaml).unwrap();
    work["paths"] = json!([]);
    let mut applied = std::collections::BTreeSet::new();
    let write = |work: &Value| {
        p.ledger_with(
            &work["definition"],
            "Review the contract",
            json!({"paths":work["paths"], "acceptance":acceptance}),
        )
    };
    write(&work);
    p.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
    for _ in 0..20 {
        let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
        let mut changed = false;
        for code in gap_codes(&report) {
            let hint = hints
                .iter()
                .find(|h| h.codes.contains(&code.as_str()))
                .unwrap_or_else(|| panic!("no field hint for {code}"));
            if !applied.insert(hint.path) {
                continue;
            }
            let value = if hint.path == examples.path {
                let mut values = Vec::new();
                for criterion in &acceptance {
                    for kind in &kinds {
                        let mut example = filled_hint(&hint.value[0], &pin);
                        example["criterion"] = json!(criterion);
                        example["kind"] = json!(kind);
                        values.push(example);
                    }
                }
                Value::Array(values)
            } else {
                filled_hint(&hint.value, &pin)
            };
            set_hint(&mut work, hint.path, value);
            changed = true;
        }
        if !changed {
            return (p, work, acceptance);
        }
        write(&work);
        p.reindex();
    }
    panic!("the printed field instructions did not converge");
}

#[test]
fn gap_hints_r1_ac3_help_alone_registers_decision_and_covers_each_criterion() {
    let (p, _, acceptance) = author_from_help(true);
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(report["state"], "complete", "{report}");
    assert!(gap_codes(&report).is_empty(), "{report}");
    for criterion in acceptance {
        for kind in ["positive", "negative"] {
            assert_eq!(
                report["examples"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|e| e["criterion"] == criterion && e["kind"] == kind)
                    .count(),
                1,
                "{report}"
            );
        }
    }
}

#[test]
fn gap_hints_r1_ac3_missing_decision_front_matter_keeps_reference_gap() {
    let (p, _, _) = author_from_help(false);
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(
        gap_codes(&report),
        ["intent_ref_not_found", "inputs_unversioned"]
    );
    let text = governance_help();
    let hints = gap_hints(&text);
    let hint = hints
        .iter()
        .find(|h| h.codes.contains(&"intent_ref_not_found"))
        .unwrap();
    assert_eq!(hint.path, "definition.intent");
    assert!(hint.note.contains("front matter"), "{}", hint.note);
}

#[test]
fn gap_hints_r1_ac3_one_example_per_criterion_is_insufficient() {
    let (p, mut work, acceptance) = author_from_help(true);
    work["definition"]["examples"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["kind"] == "positive");
    p.ledger_with(
        &work["definition"],
        "Add the counterexamples",
        json!({"paths":work["paths"], "acceptance":acceptance}),
    );
    p.reindex();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(gap_codes(&report), ["examples_missing", "examples_missing"]);
}

#[test]
fn gap_hints_ac5_help_leaves_definition_and_admission_reports_unchanged() {
    let p = Project::new();
    let before = p.ok(&["work", "show", "W"]);
    let text = governance_help();
    assert!(text.contains("shasum -a 256 <decision file>"));
    assert!(text.contains("sourced_fact") && text.contains("Assumptions are not adopted facts"));
    let after = p.ok(&["work", "show", "W"]);
    for field in ["definition", "admission"] {
        assert_eq!(before["work"][field], after["work"][field]);
    }
}

#[test]
fn gap_hints_ac5_assumptions_still_leave_intent_and_inputs_unmet() {
    let p = Project::new();
    let mut definition = p.definition("Local save");
    definition["intent"][0]["role"] = json!("assumption");
    p.ledger(&definition, "Resolve the assumption");
    p.reindex();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(gap_codes(&report), ["intent_missing", "inputs_unversioned"]);
}

struct Project(PathBuf);
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Project {
    fn uninitialized() -> Self {
        let p = Self(std::env::temp_dir().join(format!("awr-work-definition-{}", Id::new())));
        fs::create_dir_all(p.0.join("decisions")).unwrap();
        fs::write(
            p.0.join("mapping.toml"),
            "[project]\nname='Definition fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n",
        )
        .unwrap();
        p
    }

    fn new() -> Self {
        let p = Self::uninitialized();
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
fn show_and_prepare_list_checked_and_unchecked_examples() {
    let p = Project::new();
    p.ok(&["capabilities", "--require", "definition.example_checks"]);
    let mut definition = p.definition("Local save with a failure fallback");
    for (index, example) in definition["examples"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        example["id"] = json!(format!("save-{index}"));
        example["check"] = json!({
            "run": ["nonexistent-storage-check", "--case", index.to_string()],
            "files": ["nonexistent-tests/save.rs"]
        });
    }
    p.ledger(&definition, "Review the fallback");
    p.reindex();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(report["state"], "complete");
    assert_eq!(
        report["examples"],
        json!([
            {"id": "save-0", "kind": "positive", "criterion": ACC, "has_check": true},
            {"id": "save-1", "kind": "negative", "criterion": ACC, "has_check": true}
        ])
    );
    let prepared = p.ok(&["work", "prepare", "W", "--budget", "12000"]);
    assert_eq!(prepared["definition"], report);

    definition["examples"][1]
        .as_object_mut()
        .unwrap()
        .remove("check");
    p.ledger(&definition, "Review the fallback");
    p.reindex();
    let report = p.ok(&["work", "show", "W"])["work"]["definition"].clone();
    assert_eq!(report["state"], "complete");
    assert_eq!(report["examples"][0]["has_check"], true);
    assert_eq!(report["examples"][1]["has_check"], false);
    assert_eq!(report["notes"][0]["code"], "example_check_absent");
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
