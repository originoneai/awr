//! Synthetic source-file regressions for Team's internal and display identities.
use awr_source::*;
use awr_team::{DraftChange, DraftDefinitionState, DraftOpKind, TaskDraft, WorkstreamBundle};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/team-mcp/publish-prep")
}

fn ledger(name: &str) -> Vec<u8> {
    fs::read(root().join(name)).unwrap()
}

fn prepare(bytes: &[u8]) -> awr_core::Result<WorkstreamBundle> {
    prepare_publish_from_ledger_bytes(
        &SoleSourceLocation::server_directory(root(), "ledger.yaml").unwrap(),
        &root(),
        bytes,
        "demo-project",
        &PublishPrepOptions::default(),
    )?
    .bundle()
}

fn draft() -> TaskDraft {
    serde_json::from_value(json!({
        "work_id":"internal-api", "external_key":"API-1", "title":"Define the interface",
        "goals":["delivery"], "scope_paths":["openapi.yaml"],
        "acceptance":["OpenAPI is reviewed"], "required_dependencies":[],
        "completion_policy":"independent_review", "definition_state":"enabled", "workstream":"api"
    }))
    .unwrap()
}

fn change(op: DraftOpKind, before: Option<TaskDraft>, after: TaskDraft) -> DraftChange {
    DraftChange { op, before, after }
}

#[test]
fn source_file_roundtrip_preserves_created_edited_and_retired_identities() {
    let mut bytes = ledger("distinct-identities.yaml");
    let original = prepare(&bytes).unwrap();
    assert_eq!(
        original.contracts[0].contract.work_id.as_str(),
        "internal-api"
    );
    assert_eq!(original.contracts[0].contract.external_key, "API-1");
    assert_eq!(
        original.contracts[1].contract.required_dependencies,
        ["internal-api"]
    );

    let mut created = draft();
    created.work_id = "internal-new".into();
    created.external_key = "API-NEW".into();
    created.required_dependencies = vec!["internal-api".into()];
    created.dependency_acceptance = Some(
        serde_json::from_value(json!({"internal-api":"agent_reviewed_caller_asserted_reconciled"}))
            .unwrap(),
    );
    created.split_from = Some("internal-api".into());
    bytes = apply_planning_changes_to_ledger(
        &bytes,
        &[change(DraftOpKind::CreateTask, None, created.clone())],
    )
    .unwrap()
    .after_bytes;
    let expected = prepare(&bytes).unwrap();
    let contract = &expected.contracts[2].contract;
    assert_eq!(contract.work_id.as_str(), "internal-new");
    assert_eq!(contract.external_key, "API-NEW");
    assert_eq!(contract.required_dependencies, ["internal-api"]);
    assert!(contract.dependency_acceptance.contains_key("internal-api"));

    for op in [
        DraftOpKind::EditFields,
        DraftOpKind::Split,
        DraftOpKind::Archive,
        DraftOpKind::Cancel,
    ] {
        let mut after = created.clone();
        after.title = "Reviewed new title".into();
        after.split_children = vec!["internal-child".into()];
        after.definition_state = match op {
            DraftOpKind::Archive => DraftDefinitionState::Archived,
            DraftOpKind::Cancel => DraftDefinitionState::Cancelled,
            _ => DraftDefinitionState::Enabled,
        };
        bytes = apply_planning_changes_to_ledger(
            &bytes,
            &[change(op, Some(created.clone()), after.clone())],
        )
        .unwrap()
        .after_bytes;
        let written: Value = serde_yaml_ng::from_slice(&bytes).unwrap();
        assert_eq!(written["work_items"][2]["id"], "API-NEW");
        assert_eq!(written["work_items"][2]["work_id"], "internal-new");
        assert_eq!(written["work_items"][2]["split_from"], "internal-api");
        assert_eq!(
            written["work_items"][2]["split_children"],
            json!(["internal-child"])
        );
        assert_eq!(prepare(&bytes).unwrap(), expected);
        created = after;
    }
}

#[test]
fn equal_identity_create_preserves_existing_writeback_fingerprint() {
    let mut task = draft();
    task.work_id = "API-NEW".into();
    task.external_key = "API-NEW".into();
    let patch = apply_planning_changes_to_ledger(
        &ledger("ledger.yaml"),
        &[change(DraftOpKind::CreateTask, None, task)],
    )
    .unwrap();
    // Pin the existing definition-state writer's output so optional identity
    // support does not change a pending equal-ID intent's source fingerprint.
    assert_eq!(
        patch.after_fingerprint,
        "sha256:15a42ff34ff0ce8bcd5e96fb773d4cd0c5c7f3ee66f696ab61600700bc53c711"
    );
    let written: Value = serde_yaml_ng::from_slice(&patch.after_bytes).unwrap();
    assert!(written["work_items"][2].get("work_id").is_none());
    let bundle = prepare(&patch.after_bytes).unwrap();
    assert_eq!(bundle.contracts[2].contract.work_id.as_str(), "API-NEW");
}

#[test]
fn legacy_omission_and_explicit_equal_identity_have_identical_contracts() {
    let bytes = ledger("ledger.yaml");
    let legacy = prepare(&bytes).unwrap();
    assert_eq!(legacy.contracts[0].contract.work_id.as_str(), "API-1");
    let mut doc: Value = serde_yaml_ng::from_slice(&bytes).unwrap();
    for row in doc["work_items"].as_array_mut().unwrap() {
        row["work_id"] = row["id"].clone();
    }
    assert_eq!(prepare(&serde_json::to_vec(&doc).unwrap()).unwrap(), legacy);
    let mut before = draft();
    before.work_id = "API-1".into();
    let mut after = before.clone();
    after.title = "Edited legacy title".into();
    let patch = apply_planning_changes_to_ledger(
        &bytes,
        &[change(DraftOpKind::EditFields, Some(before), after)],
    )
    .unwrap();
    assert_eq!(prepare(&patch.after_bytes).unwrap(), legacy);
}

#[test]
fn explicit_invalid_and_duplicate_identities_are_rejected_by_both_paths() {
    for (field, value) in [
        ("work_id", Value::Null),
        ("work_id", json!("")),
        ("work_id", json!(" ")),
        ("work_id", json!(7)),
        ("work_id", json!("bad\nidentity")),
        ("work_id", json!("x".repeat(129))),
        ("work_id", json!("internal-client")),
        ("id", json!("CLIENT-1")),
    ] {
        let mut doc: Value =
            serde_yaml_ng::from_slice(&ledger("distinct-identities.yaml")).unwrap();
        doc["work_items"][0][field] = value;
        let bytes = serde_json::to_vec(&doc).unwrap();
        assert!(prepare(&bytes).is_err(), "publish accepted invalid {field}");
        assert!(
            apply_planning_changes_to_ledger(&bytes, &[]).is_err(),
            "writeback accepted invalid {field}"
        );
    }
    // An explicit internal ID also conflicts with a legacy row's effective ID.
    let mut doc: Value = serde_yaml_ng::from_slice(&ledger("ledger.yaml")).unwrap();
    doc["work_items"][0]["work_id"] = json!("CLIENT-1");
    assert!(prepare(&serde_json::to_vec(&doc).unwrap()).is_err());

    let ambiguous = String::from_utf8(ledger("distinct-identities.yaml"))
        .unwrap()
        .replace(
            "work_id: internal-api",
            "work_id: null\n    work_id: internal-api",
        );
    assert!(prepare(ambiguous.as_bytes()).is_err());
    assert!(apply_planning_changes_to_ledger(ambiguous.as_bytes(), &[]).is_err());
}

#[test]
fn creates_cannot_duplicate_either_identity_and_edits_cannot_change_them() {
    let bytes = ledger("distinct-identities.yaml");
    for (internal, external) in [
        ("internal-api", "NEW"),
        ("new", "API-1"),
        ("bad\nidentity", "NEW"),
    ] {
        let mut after = draft();
        after.work_id = internal.into();
        after.external_key = external.into();
        assert!(
            apply_planning_changes_to_ledger(
                &bytes,
                &[change(DraftOpKind::CreateTask, None, after)]
            )
            .is_err()
        );
    }
    for (internal, external) in [
        ("new", "API-1"),
        ("internal-api", "NEW"),
        ("internal-client", "API-1"),
    ] {
        let mut after = draft();
        after.work_id = internal.into();
        after.external_key = external.into();
        for before in [None, Some(draft())] {
            assert!(
                apply_planning_changes_to_ledger(
                    &bytes,
                    &[change(DraftOpKind::EditFields, before, after.clone())]
                )
                .is_err()
            );
        }
    }
    let mut forged = draft();
    forged.work_id = "unobserved-prior".into();
    assert!(
        apply_planning_changes_to_ledger(
            &bytes,
            &[change(DraftOpKind::EditFields, Some(forged), draft())]
        )
        .is_err()
    );
    for (internal, external) in [("new-internal", "OTHER"), ("other-internal", "NEW")] {
        let mut first = draft();
        first.work_id = "new-internal".into();
        first.external_key = "NEW".into();
        let mut second = first.clone();
        second.work_id = internal.into();
        second.external_key = external.into();
        assert!(
            apply_planning_changes_to_ledger(
                &bytes,
                &[
                    change(DraftOpKind::CreateTask, None, first),
                    change(DraftOpKind::CreateTask, None, second),
                ]
            )
            .is_err()
        );
    }
}

#[test]
fn identity_namespaces_do_not_alias_and_dependencies_remain_internal() {
    let mut doc: Value = serde_yaml_ng::from_slice(&ledger("distinct-identities.yaml")).unwrap();
    // The provider's display key may equal a different task's internal ID.
    doc["work_items"][1]["work_id"] = json!("API-1");
    doc["work_items"][1]["workstream"] = json!("api");
    // The legacy dependency alias has the same internal-ID semantics, and
    // writeback's canonical depends_on must retain its meaning.
    doc["work_items"][1]
        .as_object_mut()
        .unwrap()
        .remove("depends_on");
    doc["work_items"][1]["dependencies"] = json!(["internal-api"]);
    doc["work_items"][1]["dependency_acceptance"] =
        json!({"internal-api":"agent_reviewed_caller_asserted_reconciled"});
    let bytes = serde_json::to_vec(&doc).unwrap();
    let expected = prepare(&bytes).unwrap();
    let mut before = draft();
    before.work_id = "API-1".into();
    before.external_key = "CLIENT-1".into();
    before.required_dependencies = vec!["internal-api".into()];
    before.dependency_acceptance = Some(
        serde_json::from_value(json!({"internal-api":"agent_reviewed_caller_asserted_reconciled"}))
            .unwrap(),
    );
    before.scope_paths = vec!["sdk/".into()];
    before.acceptance = vec!["SDK smoke test passes".into()];
    let mut after = before.clone();
    after.title = "Edited consumer".into();
    let patch = apply_planning_changes_to_ledger(
        &bytes,
        &[change(DraftOpKind::EditFields, Some(before), after)],
    )
    .unwrap();
    let written: Value = serde_yaml_ng::from_slice(&patch.after_bytes).unwrap();
    assert_eq!(written["work_items"][0]["title"], "Define the interface");
    assert_eq!(written["work_items"][1]["title"], "Edited consumer");
    assert_eq!(prepare(&patch.after_bytes).unwrap(), expected);
    // Display keys are not translated into dependencies or policy-map keys.
    doc["work_items"][1]["depends_on"] = json!(["API-1"]);
    doc["work_items"][1]["dependency_acceptance"] =
        json!({"API-1":"agent_reviewed_caller_asserted_reconciled"});
    assert!(prepare(&serde_json::to_vec(&doc).unwrap()).is_err());
}
