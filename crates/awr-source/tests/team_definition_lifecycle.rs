//! Synthetic source roundtrips; source progress never establishes completion.
use awr_source::{
    PublishPrepOptions, SoleSourceLocation, apply_planning_changes_to_ledger,
    prepare_publish_from_ledger_bytes,
};
use awr_team::{
    DraftChange, DraftDefinitionState, DraftOpKind, TASK_DEFINITIONS_FILE, TaskDefinitions,
};
use serde_json::{Value, json};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/team-mcp/publish-prep")
}

fn ledger() -> Value {
    serde_yaml_ng::from_slice(&std::fs::read(root().join("ledger.yaml")).unwrap()).unwrap()
}

fn prepare(doc: &Value) -> awr_core::Result<awr_source::TeamPublishPackage> {
    prepare_publish_from_ledger_bytes(
        &SoleSourceLocation::server_directory(root(), "ledger.yaml").unwrap(),
        &root(),
        &serde_json::to_vec(doc).unwrap(),
        "demo-project",
        &PublishPrepOptions::default(),
    )
}

fn definitions(doc: &Value) -> TaskDefinitions {
    let package = prepare(doc).unwrap();
    serde_json::from_slice(
        &package
            .files
            .iter()
            .find(|f| f.path == TASK_DEFINITIONS_FILE)
            .unwrap()
            .bytes,
    )
    .unwrap()
}

#[test]
fn source_definition_states_and_titles_are_explicit_and_completion_is_only_a_note() {
    let doc = ledger();
    let package = prepare(&doc).unwrap();
    let metadata = definitions(&doc);
    assert_eq!(
        metadata.works["API-1"].definition_state,
        DraftDefinitionState::Enabled
    );
    assert_eq!(metadata.works["API-1"].title, "Define the interface");
    assert!(
        package
            .source_status_notes
            .iter()
            .any(|n| n.raw_status == "completed" && n.meaning == "source_only")
    );
    for state in ["draft", "enabled", "archived", "cancelled"] {
        let mut changed = doc.clone();
        changed["work_items"][0]["definition_state"] = json!(state);
        assert_eq!(
            definitions(&changed).works["API-1"]
                .definition_state
                .as_str(),
            state
        );
        // Progress notes remain independent even when they disagree with definition state.
        assert_eq!(
            prepare(&changed).unwrap().source_status_notes[0].raw_status,
            "completed"
        );
        changed["work_items"][0]
            .as_object_mut()
            .unwrap()
            .remove("definition_state");
        changed["work_items"][0]["status"] = json!(state);
        assert_eq!(
            definitions(&changed).works["API-1"]
                .definition_state
                .as_str(),
            state
        );
    }
    for value in [
        json!("completed"),
        json!("unknown"),
        json!(null),
        json!(false),
        json!(0),
    ] {
        let mut invalid = doc.clone();
        invalid["work_items"][0]["definition_state"] = value;
        assert!(
            prepare(&invalid)
                .unwrap_err()
                .to_string()
                .contains("definition_state")
        );
    }
}

#[test]
fn approved_create_and_edit_encode_draft_and_enabled_as_distinct_recoverable_source() {
    let base = ledger();
    let draft = |state| {
        serde_json::from_value(json!({
        "work_id":"NEW", "external_key":"NEW", "title":"Approved title", "goals":["delivery"],
        "scope_paths":["sdk"], "acceptance":["verified"], "required_dependencies":[],
        "completion_policy":"independent_review", "definition_state":state, "workstream":"client"
    })).unwrap()
    };
    let create = |state| {
        apply_planning_changes_to_ledger(
            &serde_json::to_vec(&base).unwrap(),
            &[DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: draft(state),
            }],
        )
        .unwrap()
    };
    let created = create("draft");
    assert_ne!(created.after_bytes, create("enabled").after_bytes);
    let mut prior = draft("draft");
    let mut bytes = created.after_bytes;
    for state in [
        DraftDefinitionState::Enabled,
        DraftDefinitionState::Archived,
        DraftDefinitionState::Cancelled,
        DraftDefinitionState::Draft,
    ] {
        let mut after = prior.clone();
        after.definition_state = state;
        after.title = format!("Approved {}", state.as_str());
        bytes = apply_planning_changes_to_ledger(
            &bytes,
            &[DraftChange {
                op: match state {
                    DraftDefinitionState::Archived => DraftOpKind::Archive,
                    DraftDefinitionState::Cancelled => DraftOpKind::Cancel,
                    _ => DraftOpKind::EditFields,
                },
                before: Some(prior),
                after: after.clone(),
            }],
        )
        .unwrap()
        .after_bytes;
        let doc: Value = serde_yaml_ng::from_slice(&bytes).unwrap();
        let recovered = definitions(&doc);
        assert_eq!(recovered.works["NEW"].definition_state, state);
        assert_eq!(recovered.works["NEW"].title, after.title);
        prior = after;
    }
}

#[test]
fn definition_writeback_binds_prior_source_state_with_legacy_status_mapping() {
    let task: awr_team::TaskDraft = serde_json::from_value(json!({
        "work_id":"NEW", "external_key":"NEW", "title":"Original title", "goals":["delivery"],
        "scope_paths":["sdk"], "acceptance":["verified"], "required_dependencies":[],
        "completion_policy":"independent_review", "definition_state":"enabled", "workstream":"client"
    }))
    .unwrap();
    for (fields, expected) in [
        (
            json!({"definition_state":"draft", "status":"planned"}),
            DraftDefinitionState::Draft,
        ),
        (
            json!({"definition_state":"enabled", "status":"archived"}),
            DraftDefinitionState::Enabled,
        ),
        (
            json!({"definition_state":"archived", "status":"planned"}),
            DraftDefinitionState::Archived,
        ),
        (
            json!({"definition_state":"cancelled", "status":"completed"}),
            DraftDefinitionState::Cancelled,
        ),
        (json!({"status":"draft"}), DraftDefinitionState::Draft),
        (json!({"status":"archived"}), DraftDefinitionState::Archived),
        (
            json!({"status":"cancelled"}),
            DraftDefinitionState::Cancelled,
        ),
        (json!({"status":"planned"}), DraftDefinitionState::Enabled),
        (json!({"status":"done"}), DraftDefinitionState::Enabled),
        (json!({"status":"completed"}), DraftDefinitionState::Enabled),
        (json!({}), DraftDefinitionState::Enabled),
    ] {
        let mut doc = json!({"work_items":[{"id":"NEW", "title":"Original title"}]});
        doc["work_items"][0]
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let bytes = serde_json::to_vec(&doc).unwrap();
        let mut before = task.clone();
        before.definition_state = if expected == DraftDefinitionState::Enabled {
            DraftDefinitionState::Archived
        } else {
            DraftDefinitionState::Enabled
        };
        let mut after = before.clone();
        after.title = "Corrected title".into();
        let stale = apply_planning_changes_to_ledger(
            &bytes,
            &[DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(before),
                after,
            }],
        );
        assert!(
            matches!(stale, Err(awr_core::Error::SourceConflict(_))),
            "a title edit must not overwrite newer source state {fields}: {stale:?}"
        );

        let mut before = task.clone();
        before.definition_state = expected;
        let mut after = before.clone();
        after.title = "Corrected title".into();
        let patch = apply_planning_changes_to_ledger(
            &bytes,
            &[DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(before),
                after,
            }],
        )
        .unwrap();
        let restored: Value = serde_yaml_ng::from_slice(&patch.after_bytes).unwrap();
        assert_eq!(
            restored["work_items"][0]["definition_state"],
            expected.as_str()
        );
        assert_eq!(restored["work_items"][0]["title"], "Corrected title");
    }

    for invalid in [json!(null), json!("completed")] {
        let bytes = serde_json::to_vec(&json!({
            "work_items":[{"id":"NEW", "title":"Original title", "definition_state":invalid}]
        }))
        .unwrap();
        let mut after = task.clone();
        after.title = "Corrected title".into();
        assert!(matches!(
            apply_planning_changes_to_ledger(
                &bytes,
                &[DraftChange {
                    op: DraftOpKind::EditFields,
                    before: Some(task.clone()),
                    after,
                }],
            ),
            Err(awr_core::Error::InvalidInput(_))
        ));
    }
}
