use awr_core::{EntityKind, Freshness, Id, ProjectionBatch, Source};
use awr_source::{
    Manifest, ParseContext, SourceAdapter, SourceSnapshot, YamlLedgerAdapter,
    YamlWorkstreamLedgerAdapter, fingerprint,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const DISTINCT: &str =
    include_str!("../../../tests/fixtures/team-mcp/publish-prep/distinct-identities.yaml");

fn parse(text: &str, scoped: bool, options: &str) -> awr_core::Result<ProjectionBatch> {
    let adapter = if scoped {
        "yaml-workstream-ledger-v1"
    } else {
        "yaml-ledger-v1"
    };
    let manifest = Manifest::parse(&format!(
        "[project]\nname='Identity fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='ledger.yaml'\nadapter='{adapter}'\n{options}"
    ))?;
    let source = Source {
        id: Id::new(),
        project_id: Id::new(),
        domain: "ledger".into(),
        role: "primary".into(),
        locator: "file:///fixture/ledger.yaml".into(),
        format: "yaml".into(),
        adapter: adapter.into(),
        revision: 1,
        fingerprint: String::new(),
        freshness: Freshness::Fresh,
        config: json!({}),
    };
    let snapshot = SourceSnapshot {
        content_review: None,
        locator: source.locator.clone(),
        bytes: text.as_bytes().into(),
        fingerprint: fingerprint(text.as_bytes()),
    };
    let context = ParseContext {
        source: &source,
        existing_ids: BTreeMap::from([((EntityKind::WorkItem, "API-1".into()), source.id)]),
    };
    let batch = if scoped {
        YamlWorkstreamLedgerAdapter.parse(&snapshot, &context, &manifest.sources[0])?
    } else {
        YamlLedgerAdapter.parse(&snapshot, &context, &manifest.sources[0])?
    };
    if let Some(api) = batch
        .work_items
        .iter()
        .find(|w| w.meta.external_key == "API-1")
    {
        assert_eq!(
            api.meta.id, source.id,
            "local identity must retain the display-key binding"
        );
    }
    Ok(batch)
}

fn dependency(batch: &ProjectionBatch) -> &awr_core::Edge {
    batch
        .edges
        .iter()
        .find(|edge| edge.relation == "depends_on")
        .unwrap()
}

#[test]
fn both_yaml_adapters_resolve_the_team_fixture_by_internal_identity() {
    let mut doc: Value = serde_yaml_ng::from_str(DISTINCT).unwrap();
    for scoped in [true, false] {
        if !scoped {
            doc.as_object_mut().unwrap().remove("workstreams");
        }
        let batch = parse(&serde_json::to_string(&doc).unwrap(), scoped, "").unwrap();
        let edge = dependency(&batch);
        assert_eq!(edge.from_key, "CLIENT-1");
        assert_eq!(edge.to_key, "API-1");
        assert_eq!(
            edge.source_ref.pointer.as_deref(),
            Some("/work_items/1/depends_on/0")
        );
        assert_eq!(batch.work_items[0].meta.external_key, "API-1");
        if scoped {
            let projection = batch.workstream_projection.as_ref().unwrap();
            assert!(
                projection
                    .ownership
                    .iter()
                    .any(|binding| binding.work_item_id == batch.work_items[0].meta.id.to_string())
            );
        }
    }
}

#[test]
fn dependency_internal_identity_wins_over_another_rows_display_key() {
    let batch = parse(
        &json!({"work_items":[
            {"id":"API-1","work_id":"internal-api"},
            {"id":"internal-api","work_id":"internal-shadow","depends_on":["internal-api"]}
        ]})
        .to_string(),
        false,
        "",
    )
    .unwrap();
    assert_eq!(dependency(&batch).from_key, "internal-api");
    assert_eq!(dependency(&batch).to_key, "API-1");
}

#[test]
fn mapped_and_keyed_records_keep_dependency_aliases_and_original_pointers() {
    let text = "work_items:\n  API-1:\n    ticket: API-1\n    identity: internal-api\n  CLIENT-1:\n    ticket: CLIENT-1\n    identity: internal-client\n    links: [{key: internal-api, required: false}]\n";
    let batch = parse(
        text,
        false,
        "[sources.options.field_map]\nid='ticket'\nwork_id='identity'\ndepends_on='links'\n",
    )
    .unwrap();
    assert_eq!(dependency(&batch).to_key, "API-1");
    assert!(!dependency(&batch).required);
    assert_eq!(
        dependency(&batch).source_ref.pointer.as_deref(),
        Some("/work_items/CLIENT-1/links/0")
    );
    let alias = parse("work_items: [{id: API-1, work_id: internal-api}, {id: CLIENT-1, dependencies: [{id: internal-api}]}]", false, "").unwrap();
    assert_eq!(dependency(&alias).to_key, "API-1");
    assert_eq!(
        dependency(&alias).source_ref.pointer.as_deref(),
        Some("/work_items/1/dependencies/0")
    );
}

#[test]
fn explicit_invalid_or_duplicate_internal_ids_are_source_errors() {
    for value in [
        Value::Null,
        json!(7),
        json!(""),
        json!(" "),
        json!("bad\nidentity"),
        json!("x".repeat(129)),
    ] {
        let report = parse(
            &json!({"work_items":[{"id":"API-1","work_id":value}]}).to_string(),
            false,
            "",
        )
        .unwrap_err()
        .report();
        assert_eq!(
            report.details.unwrap()["location"]["pointer"],
            "/work_items/0/work_id"
        );
    }
    for first in [
        json!({"id":"API-1","work_id":"internal-api"}),
        json!({"id":"internal-api"}),
    ] {
        assert!(
            parse(
                &json!({"work_items":[first,{"id":"CLIENT-1","work_id":"internal-api"}]})
                    .to_string(),
                false,
                ""
            )
            .is_err()
        );
    }
    let report = parse(
        "work_items:\n- id: API-1\n  identity: null\n",
        false,
        "[sources.options.field_map]\nwork_id='identity'\n",
    )
    .unwrap_err()
    .report();
    let details = report.details.unwrap();
    assert_eq!(details["location"]["pointer"], "/work_items/0/identity");
    assert_eq!(details["location"]["line"], 3);
}

#[test]
fn display_keys_cannot_silently_satisfy_internal_dependencies() {
    let report = parse(
        "work_items: [{id: API-1, work_id: internal-api}, {id: CLIENT-1, depends_on: [API-1]}]",
        false,
        "",
    )
    .unwrap_err()
    .report();
    assert_eq!(
        report.details.unwrap()["location"]["pointer"],
        "/work_items/1/depends_on/0"
    );
}

#[test]
fn missing_targets_and_legacy_dependencies_keep_their_original_meaning() {
    for identity in ["", "work_id: API-1,"] {
        let text = format!(
            "work_items: [{{id: API-1, {identity} status: completed}}, {{id: CLIENT-1, depends_on: [API-1, MISSING]}}]"
        );
        let batch = parse(&text, false, "").unwrap();
        let targets: Vec<_> = batch
            .edges
            .iter()
            .map(|edge| edge.to_key.as_str())
            .collect();
        assert_eq!(targets, ["API-1", "MISSING"]);
    }
}
