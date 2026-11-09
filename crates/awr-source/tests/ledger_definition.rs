//! The optional `definition` of a YAML work item: projected when valid, refused with a precise
//! diagnostic that never echoes the rejected content, unsupported in Markdown ledgers.
use awr_core::{DiscriminationMethod, Freshness, Id, Source, WorkPurpose};
use awr_source::{
    Manifest, MarkdownLedgerAdapter, ParseContext, SourceAdapter, SourceSnapshot,
    YamlLedgerAdapter, fingerprint,
};
use std::collections::BTreeMap;

fn parse(text: &str, adapter: &str) -> awr_core::Result<awr_core::ProjectionBatch> {
    let manifest = Manifest::parse(&format!(
        "[project]\nname='Definition fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='{adapter}'\n"
    ))?;
    let source = Source {
        id: Id::new(),
        project_id: Id::new(),
        domain: "ledger".into(),
        role: "primary".into(),
        locator: "file:///fixture/work.yaml".into(),
        format: "yaml".into(),
        adapter: adapter.into(),
        revision: 1,
        fingerprint: String::new(),
        freshness: Freshness::Fresh,
        config: serde_json::json!({}),
    };
    let snapshot = SourceSnapshot {
        content_review: None,
        locator: source.locator.clone(),
        bytes: text.as_bytes().into(),
        fingerprint: fingerprint(text.as_bytes()),
    };
    let context = ParseContext {
        source: &source,
        existing_ids: BTreeMap::new(),
    };
    if adapter == "yaml-ledger-v1" {
        YamlLedgerAdapter.parse(&snapshot, &context, &manifest.sources[0])
    } else {
        MarkdownLedgerAdapter.parse(&snapshot, &context, &manifest.sources[0])
    }
}

fn ledger(definition: &str) -> String {
    format!(
        "work_items:\n  - id: W\n    title: Save safely\n    status: planned\n    acceptance: [Keep the input]\n{definition}"
    )
}

const VALID: &str = "    definition:\n      version: 1\n      purpose: implementation\n      tier: required\n      deliverable: Local save\n      discrimination:\n        method: diff_mutation\n";

fn example(kind: &str) -> serde_json::Value {
    serde_json::json!({"criterion": "Keep the input", "kind": kind,
        "given": "storage refuses a write", "when": "a draft is saved", "then": "the draft remains visible"})
}

fn with_examples(examples: serde_json::Value) -> String {
    ledger(&format!("{VALID}      examples: {examples}\n"))
}

#[test]
fn examples_with_ids_and_checks_are_projected_without_opening_commands_or_files() {
    let mut examples = serde_json::json!([example("positive"), example("negative")]);
    for (index, item) in examples.as_array_mut().unwrap().iter_mut().enumerate() {
        item["id"] = serde_json::json!(format!("save-{index}"));
        item["check"] = serde_json::json!({"run": ["nonexistent-storage-check", "--case", index.to_string()], "files": ["nonexistent-tests/save.rs"]});
    }
    let batch = parse(&with_examples(examples.clone()), "yaml-ledger-v1").unwrap();
    let definition = batch.work_items[0].definition.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(definition).unwrap()["examples"],
        examples
    );
}

#[test]
fn an_example_check_parent_path_is_rejected_with_the_rule_and_field() {
    let mut item = example("positive");
    item["id"] = serde_json::json!("save-input");
    item["check"] = serde_json::json!({"run": ["storage-check"], "files": ["../PRIVATE-PATH-77"]});
    let report = parse(&with_examples(serde_json::json!([item])), "yaml-ledger-v1")
        .unwrap_err()
        .report();
    assert_eq!(report.code, "InvalidInput");
    assert!(
        report.message.contains("examples[0].check.files"),
        "{}",
        report.message
    );
    assert!(!report.message.contains("PRIVATE-PATH-77"));
    let details = report.details.unwrap();
    assert_eq!(details["rule"], "ledger.definition");
    assert_eq!(details["location"]["pointer"], "/work_items/0/definition");
}

#[test]
fn legacy_yaml_examples_do_not_acquire_ids_or_checks() {
    let examples = serde_json::json!([example("positive"), example("negative")]);
    let batch = parse(&with_examples(examples.clone()), "yaml-ledger-v1").unwrap();
    let definition = batch.work_items[0].definition.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(definition).unwrap()["examples"],
        examples
    );
    for item in serde_json::to_value(definition).unwrap()["examples"]
        .as_array()
        .unwrap()
    {
        assert!(item.get("id").is_none());
        assert!(item.get("check").is_none());
    }
}

#[test]
fn an_example_check_rejects_unknown_yaml_fields_without_echoing_them() {
    let mut item = example("positive");
    item["id"] = serde_json::json!("save-input");
    item["check"] = serde_json::json!({"run": ["storage-check"], "files": ["tests/save.rs"], "unexpected": "PRIVATE-NOTE-77"});
    let report = parse(&with_examples(serde_json::json!([item])), "yaml-ledger-v1")
        .unwrap_err()
        .report();
    assert_eq!(report.details.unwrap()["rule"], "ledger.definition");
    assert!(!report.message.contains("PRIVATE-NOTE-77"));
}

#[test]
fn a_valid_definition_is_projected_and_an_absent_one_leaves_the_work_unchanged() {
    let batch = parse(&ledger(VALID), "yaml-ledger-v1").unwrap();
    let definition = batch.work_items[0].definition.as_ref().unwrap();
    assert_eq!(definition.purpose, WorkPurpose::Implementation);
    assert_eq!(definition.deliverable.as_deref(), Some("Local save"));
    assert_eq!(
        definition.discrimination.as_ref().unwrap().method,
        DiscriminationMethod::DiffMutation
    );
    let plain = parse(&ledger(""), "yaml-ledger-v1").unwrap();
    assert!(plain.work_items[0].definition.is_none());
    // An explicit null is the same as no definition.
    let null = parse(&ledger("    definition: null\n"), "yaml-ledger-v1").unwrap();
    assert!(null.work_items[0].definition.is_none());
}

#[test]
fn an_invalid_definition_names_the_rule_and_does_not_echo_what_was_rejected() {
    for (what, body) in [
        (
            "an unknown field",
            "      version: 1\n      purpose: research\n      mystery: PRIVATE-NOTE-77\n",
        ),
        (
            "an unknown purpose",
            "      version: 1\n      purpose: PRIVATE-NOTE-77\n",
        ),
        ("a missing purpose", "      version: 1\n"),
        (
            "a wrong version",
            "      version: 9\n      purpose: research\n",
        ),
        (
            "an unpinned decision",
            "      version: 1\n      purpose: research\n      intent:\n        - role: user_decision\n          note: PRIVATE-NOTE-77\n",
        ),
        (
            "a blank author",
            "      version: 1\n      purpose: research\n      contributes:\n        - goal: G\n          outcome: PRIVATE-NOTE-77\n          claim: c\n          by: ' '\n",
        ),
    ] {
        let report = parse(
            &ledger(&format!("    definition:\n{body}")),
            "yaml-ledger-v1",
        )
        .unwrap_err()
        .report();
        assert_eq!(report.code, "InvalidInput", "{what}");
        let details = report.details.unwrap();
        assert_eq!(details["rule"], "ledger.definition", "{what}");
        assert_eq!(
            details["location"]["pointer"], "/work_items/0/definition",
            "{what}"
        );
        let repair = details["repair"].as_str().unwrap();
        assert!(
            !report.message.contains("PRIVATE-NOTE-77") && !repair.contains("PRIVATE-NOTE-77"),
            "{what}"
        );
    }
}

#[test]
fn a_markdown_ledger_refuses_any_definition_with_the_same_rule() {
    let list = |metadata: &str| {
        format!(
            "# Work\n\n- [ ] Save safely <!-- awr:id=\"W\" --> <!-- awr:acceptance=[\"Keep the input\"] -->{metadata}\n"
        )
    };
    let plain = parse(&list(""), "markdown-ledger-v1").unwrap();
    assert!(plain.work_items[0].definition.is_none());
    for value in [r#"{"version":1,"purpose":"research"}"#, "null", "[]"] {
        let report = parse(
            &list(&format!(" <!-- awr:definition={value} -->")),
            "markdown-ledger-v1",
        )
        .unwrap_err()
        .report();
        assert_eq!(report.code, "InvalidInput", "{value}");
        let details = report.details.unwrap();
        assert_eq!(details["rule"], "ledger.definition", "{value}");
        assert!(
            details["repair"].as_str().unwrap().contains("YAML"),
            "{value}"
        );
    }
}

#[test]
fn changing_the_definition_changes_the_work_section_fingerprint() {
    let first = parse(&ledger(VALID), "yaml-ledger-v1").unwrap();
    let edited = parse(
        &ledger(&VALID.replace("Local save", "Local save with retry")),
        "yaml-ledger-v1",
    )
    .unwrap();
    let plain = parse(&ledger(""), "yaml-ledger-v1").unwrap();
    let section = |batch: &awr_core::ProjectionBatch| {
        batch.work_items[0]
            .meta
            .source_ref
            .section_fingerprint
            .clone()
            .unwrap()
    };
    assert_ne!(section(&first), section(&edited));
    assert_ne!(section(&first), section(&plain));
}
