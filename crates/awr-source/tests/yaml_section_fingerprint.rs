use awr_core::{Freshness, Id, ProjectionBatch, Source};
use awr_source::{
    Manifest, ParseContext, SourceAdapter, SourceSnapshot, YamlLedgerAdapter, fingerprint,
};
use std::collections::BTreeMap;

const BASE: &str = "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n";

fn parse(text: &str) -> awr_core::Result<ProjectionBatch> {
    let manifest = Manifest::parse(
        "[project]\nname='Section fingerprint fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'",
    )?;
    let source = Source {
        id: Id::new(),
        project_id: Id::new(),
        domain: "ledger".into(),
        role: "primary".into(),
        locator: "file:///fixture/work.yaml".into(),
        format: "yaml".into(),
        adapter: "yaml-ledger-v1".into(),
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
    YamlLedgerAdapter.parse(&snapshot, &context, &manifest.sources[0])
}

fn section_fp(batch: &ProjectionBatch) -> Option<String> {
    batch.work_items[0]
        .meta
        .source_ref
        .section_fingerprint
        .clone()
}

#[test]
fn work_items_carry_a_section_fingerprint_distinct_from_the_file_fingerprint() {
    let batch = parse(BASE).unwrap();
    let fp = section_fp(&batch).expect("work item must carry a section fingerprint");
    assert!(fp.starts_with("sha256:"));
    let source_ref = &batch.work_items[0].meta.source_ref;
    assert_ne!(
        fp, source_ref.source_fingerprint,
        "section fingerprint must not fall back to the file-level fingerprint"
    );
}

#[test]
fn goals_and_plans_remain_unfingerprinted() {
    let batch = parse(
        "goals:\n- id: G\n  title: Ship v1\nmilestones:\n- id: M\n  title: Beta\nwork_items:\n- id: W\n  title: Work\n",
    )
    .unwrap();
    assert!(section_fp(&batch).is_some());
    assert!(batch.goals[0].meta.source_ref.section_fingerprint.is_none());
    assert!(batch.plans[0].meta.source_ref.section_fingerprint.is_none());
}

#[test]
fn yaml_relayout_keeps_the_section_fingerprint() {
    let original = parse(BASE).unwrap();
    // Same logical record: keys reordered, quotes and whitespace changed.
    let relaid = parse(
        "work_items:\n  -\n      acceptance:\n        - \"Report renders\"\n        - 'Reader receives it'\n      tags:\n        - report\n\n      id: W\n      status: planned\n      title: \"Ship the report\"\n",
    )
    .unwrap();
    assert_ne!(
        original.work_items[0].meta.source_ref.source_fingerprint,
        relaid.work_items[0].meta.source_ref.source_fingerprint,
        "the file-level fingerprint must still change with the layout"
    );
    assert_eq!(section_fp(&original), section_fp(&relaid));
}

#[test]
fn acceptance_or_identity_edits_change_the_section_fingerprint() {
    let base = section_fp(&parse(BASE).unwrap());
    let edited_criterion = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  acceptance:\n    - Report renders cleanly\n    - Reader receives it\n  tags: [report]\n",
    )
    .unwrap();
    let added_criterion = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  acceptance:\n    - Report renders\n    - Reader receives it\n    - Archived within a week\n  tags: [report]\n",
    )
    .unwrap();
    let retitled = parse(
        "work_items:\n- id: W\n  title: Ship the quarterly report\n  status: planned\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n",
    )
    .unwrap();
    assert_ne!(base, section_fp(&edited_criterion));
    assert_ne!(base, section_fp(&added_criterion));
    assert_ne!(base, section_fp(&retitled));
}

#[test]
fn lifecycle_writes_keep_the_section_fingerprint() {
    let base = section_fp(&parse(BASE).unwrap());
    // What completion rewrites in the source record.
    let completed = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: completed\n  evidence:\n    - reports/w-report.json\n  verification:\n    evidence_level: locally_verified\n  evidence_level: locally_verified\n  next_action: Done\n  blocker: was flaky\n  summary: finished ahead of plan\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n",
    )
    .unwrap();
    assert_eq!(base, section_fp(&completed));
    // The progress action rewrites summary + next_action (there is no
    // "progress" column), so a summary edit alone must not invalidate.
    let progressed = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: in_progress\n  summary: halfway there\n  next_action: render the report\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n",
    )
    .unwrap();
    assert_eq!(base, section_fp(&progressed));
    // Projection metadata refreshed by a reindex is excluded as well.
    let reindexed = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  source_ref: work.yaml#L1\n  revision: 7\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n",
    )
    .unwrap();
    assert_eq!(base, section_fp(&reindexed));
}

#[test]
fn unknown_fields_change_the_section_fingerprint() {
    // `reviewer` is outside the ledger FIELDS vocabulary (ledger_mapping.rs),
    // so this proves schema-external keys are hashed — not just known non-E
    // fields, which the writable-set test below covers.
    let base = section_fp(&parse(BASE).unwrap());
    let unknown = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n  reviewer: alice\n",
    )
    .unwrap();
    assert_ne!(base, section_fp(&unknown));
}

#[test]
fn writable_set_edits_change_the_section_fingerprint() {
    let base = section_fp(&parse(BASE).unwrap());
    // title/priority/tags/deliverables are generic writable fields outside the
    // exclusion set, so editing any of them must change the fingerprint.
    let reprioritized = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  priority: high\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n",
    )
    .unwrap();
    let retagged = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report, urgent]\n",
    )
    .unwrap();
    let delivered = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n  deliverables:\n    - report.pdf\n",
    )
    .unwrap();
    let reassigned = parse(
        "work_items:\n- id: W\n  title: Ship the report\n  status: planned\n  acceptance:\n    - Report renders\n    - Reader receives it\n  tags: [report]\n  owner: kf\n",
    )
    .unwrap();
    assert_ne!(base, section_fp(&reprioritized));
    assert_ne!(base, section_fp(&retagged));
    assert_ne!(base, section_fp(&delivered));
    assert_ne!(base, section_fp(&reassigned));
}

#[test]
fn specification_bindings_change_only_the_owning_work_section() {
    let reference = serde_json::json!({
        "specification":"export-spec", "item":"EXPORT", "approved_version":"1",
        "source_fingerprint":format!("sha256:{}", "a".repeat(64)),
        "criterion":"Export the reviewed format"
    });
    let mut ledger = serde_json::json!({"work_items":[
        {"id":"W","status":"planned","acceptance":["Export the reviewed format"],"spec_refs":[reference]},
        {"id":"OTHER","status":"planned","acceptance":["Export the reviewed format"],"spec_refs":[reference]}
    ]});
    let read = |value: &serde_json::Value| parse(&value.to_string()).unwrap();
    let base = read(&ledger);
    assert_eq!(base.work_items[0].spec_refs[0].approved_version, "1");

    ledger["work_items"][1]["spec_refs"][0]["approved_version"] = "2".into();
    let sibling = read(&ledger);
    assert_eq!(section_fp(&base), section_fp(&sibling));
    assert_ne!(
        base.work_items[1].meta.source_ref.section_fingerprint,
        sibling.work_items[1].meta.source_ref.section_fingerprint
    );

    ledger["work_items"][0]["spec_refs"][0]["approved_version"] = "2".into();
    ledger["work_items"][0]["spec_refs"][0]["source_fingerprint"] =
        format!("sha256:{}", "b".repeat(64)).into();
    let rebound = read(&ledger);
    assert_eq!(
        base.work_items[0].acceptance,
        rebound.work_items[0].acceptance
    );
    assert_ne!(section_fp(&base), section_fp(&rebound));

    ledger["work_items"][0]["status"] = "in_progress".into();
    ledger["work_items"][0]["next_action"] = "Review the export".into();
    assert_eq!(section_fp(&rebound), section_fp(&read(&ledger)));
}
