use awr_core::{Error, Freshness, Id, Source, WorkStatus};
use awr_source::{
    Manifest, MarkdownLedgerAdapter, ParseContext, SourceAdapter, SourceSnapshot,
    YamlLedgerAdapter, fingerprint,
};
use std::collections::BTreeMap;

fn parse(text: &str, adapter: &str, options: &str) -> awr_core::Result<awr_core::ProjectionBatch> {
    let manifest = Manifest::parse(&format!(
        "[project]\nname='Mapping fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='{adapter}'\n{options}"
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

#[test]
fn yaml_preserves_original_status_and_actual_relationship_pointers() {
    let text = "work_items:\n- ticket: W\n  name: 保留既有任务\n  phase: Pending\n  done_when: [Produce the report]\n  next: Write report\n  links: [DEP]\n  objective: G\n";
    let options = "[sources.options.field_map]\nid='ticket'\ntitle='name'\nstatus='phase'\nacceptance='done_when'\nnext_action='next'\ndepends_on='links'\ngoal='objective'\n[sources.options.status_map]\nPending='planned'\ncomplete='completed'\n";
    let batch = parse(text, "yaml-ledger-v1", options).unwrap();
    let work = &batch.work_items[0];
    assert_eq!(work.meta.external_key, "W");
    assert_eq!(work.raw_status, "Pending");
    assert_eq!(work.status, WorkStatus::Planned);
    assert_eq!(work.title, "保留既有任务");
    assert_eq!(work.acceptance, ["Produce the report"]);
    assert_eq!(work.next_action, "Write report");
    assert_eq!(
        batch.edges[0].source_ref.pointer.as_deref(),
        Some("/work_items/0/links/0")
    );
    assert_eq!(
        batch.edges[1].source_ref.pointer.as_deref(),
        Some("/work_items/0/objective")
    );
    let unknown = parse(
        "work_items: [{id: W, status: pending}]",
        "yaml-ledger-v1",
        "",
    )
    .unwrap();
    assert_eq!(unknown.work_items[0].status, WorkStatus::Unknown);
    assert!(!unknown.warnings.is_empty());
}

#[test]
fn markdown_original_chinese_columns_and_custom_names_are_retained() {
    let text = "# 权威台账\n\n| ID | 工作项 | 状态 | Owner 角色 | 完成硬门槛 | 当前证据 / 下一动作 |\n|---|---|---|---|---|---|\n| W | 生成报告 | pending | Delivery | 读者收到报告 | 整理报告 |\n| D | 历史工作 | complete | Delivery | 报告正确 | 复核历史证据 |\n";
    let options = "[sources.options.status_map]\npending='planned'\ncomplete='completed'\n";
    let b = parse(text, "markdown-ledger-v1", options).unwrap();
    assert_eq!(b.work_items.len(), 2);
    assert_eq!(b.work_items[0].status, WorkStatus::Planned);
    assert_eq!(b.work_items[0].owner.as_deref(), Some("Delivery"));
    assert_eq!(b.work_items[0].next_action, "整理报告");
    assert_eq!(b.work_items[0].acceptance, ["读者收到报告"]);
    assert_eq!(b.work_items[0].raw_status, "pending");
    assert_eq!(b.work_items[1].status, WorkStatus::Completed);
    assert!(b.evidence.is_empty());
    assert_eq!(b.work_items[0].meta.source_ref.start_line, Some(5));
    let custom = text.replace("工作项", "业务事项").replace("状态", "阶段");
    let b = parse(
        &custom,
        "markdown-ledger-v1",
        &format!("{options}[sources.options.field_map]\ntitle='业务事项'\nstatus='阶段'\n"),
    )
    .unwrap();
    assert_eq!(b.work_items[0].title, "生成报告");
    assert_eq!(b.work_items[0].status, WorkStatus::Planned);
}

#[test]
fn mapping_conflicts_and_invalid_semantics_are_rejected() {
    for options in [
        "[sources.options.status_map]\ncompleted='planned'\n",
        "[sources.options.status_map]\npending='almost_ready'\n",
        "[sources.options.status_map]\nPending='planned'\npending='ready'\n",
        "[sources.options.field_map]\ntitle='same'\nstatus='same'\n",
        "[sources.options.field_map]\nstatus='evidence'\n",
        "[sources.options.field_map]\nverification='checks'\n",
    ] {
        assert!(
            parse("work_items: []", "yaml-ledger-v1", options).is_err(),
            "{options}"
        );
    }
    assert!(matches!(
        parse(
            "work_items: [{id: W, status: ready, phase: pending}]",
            "yaml-ledger-v1",
            "[sources.options.field_map]\nstatus='phase'\n"
        ),
        Err(Error::SourceConflict(_))
    ));
    assert!(matches!(
        parse(
            "| ID | 工作项 | 事项 | 状态 |\n|---|---|---|---|\n| W | One | Two | ready |\n",
            "markdown-ledger-v1",
            "[sources.options.field_map]\ntitle='事项'\n"
        ),
        Err(Error::SourceConflict(_))
    ));
}

#[test]
fn diagnostics_locate_mapped_chinese_fields_and_list_members_without_source_content() {
    let text = "work_items:\n  '工/作~1':\n    id: '工/作~1'\n    标题: [错误类型]\n";
    let report = parse(
        text,
        "yaml-ledger-v1",
        "[sources.options.field_map]\ntitle='标题'\n",
    )
    .unwrap_err()
    .report();
    assert_eq!(report.code, "InvalidInput");
    let d = report.details.unwrap();
    assert_eq!(d["rule"], "ledger.string");
    assert_eq!(d["location"]["locator"], "file:///fixture/work.yaml");
    assert_eq!(d["location"]["pointer"], "/work_items/工~1作~01/标题");
    assert_eq!(d["location"]["line"], 4);
    assert_eq!(d["location"]["column"], 9);
    assert!(!report.message.contains("错误类型"));
    assert!(d["repair"].as_str().unwrap().contains("string"));
    let report = parse(
        "work_items:\n- id: W\n  acceptance: [正确, 42]\n",
        "yaml-ledger-v1",
        "",
    )
    .unwrap_err()
    .report();
    let d = report.details.unwrap();
    assert_eq!(d["location"]["pointer"], "/work_items/0/acceptance/1");
    assert_eq!(d["location"]["line"], 3);
}

fn evidence_ledger(entry: &str) -> String {
    format!("work_items:\n- id: W\n  title: Reviewed work\n  evidence:\n    {entry}\n")
}

#[test]
fn an_unquoted_colon_in_an_evidence_entry_is_diagnosed_as_missing_quotes() {
    // YAML reads `- Report.java: text` as a mapping with one key, so the entry has no locator although the author wrote one.
    let report = parse(
        &evidence_ledger("- Report.java: refresh() parses both columns"),
        "yaml-ledger-v1",
        "",
    )
    .unwrap_err()
    .report();
    assert_eq!(report.code, "InvalidInput");
    let d = report.details.unwrap();
    assert_eq!(d["rule"], "ledger.evidence_locator");
    assert_eq!(d["location"]["pointer"], "/work_items/0/evidence/0");
    assert_eq!(d["location"]["line"], 5);
    assert!(report.message.contains("mapping"));
    let repair = d["repair"].as_str().unwrap();
    assert!(repair.contains("Quote the whole entry"));
    assert!(repair.contains("locator"));
    // The rejected text is not echoed back.
    assert!(!report.message.contains("Report.java") && !repair.contains("Report.java"));
    assert!(!report.message.contains("refresh()") && !repair.contains("refresh()"));

    // A mapping that really lacks a locator keeps the plain repair.
    for entry in ["- summary: reviewed", "- locator: ''", "- {}"] {
        let report = parse(&evidence_ledger(entry), "yaml-ledger-v1", "")
            .unwrap_err()
            .report();
        let d = report.details.unwrap();
        assert_eq!(d["rule"], "ledger.evidence_locator", "{entry}");
        assert!(
            d["repair"].as_str().unwrap().contains("report path or URI"),
            "{entry}"
        );
    }

    // The spellings that work keep their whole text as the locator.
    for (entry, locator) in [
        (
            r#"- "Report.java: refresh() parses both columns""#,
            "Report.java: refresh() parses both columns",
        ),
        (
            "- 'Report.java: refresh() parses both columns'",
            "Report.java: refresh() parses both columns",
        ),
        (
            "- locator: \"Report.java: refresh()\"\n      summary: parses both columns",
            "Report.java: refresh()",
        ),
        (
            "- Report.java：refresh() parses both columns",
            "Report.java：refresh() parses both columns",
        ),
        (
            r##"- "src/Report.java #12 refresh()""##,
            "src/Report.java #12 refresh()",
        ),
    ] {
        let batch = parse(&evidence_ledger(entry), "yaml-ledger-v1", "").unwrap();
        assert_eq!(batch.evidence.len(), 1, "{entry}");
        assert_eq!(batch.evidence[0].locator, locator, "{entry}");
    }
}

#[test]
fn syntax_diagnostics_and_valid_long_chinese_yaml_are_not_confused() {
    let report = parse(
        "work_items:\n- id: W\n  title: [broken\n",
        "yaml-ledger-v1",
        "",
    )
    .unwrap_err()
    .report();
    let d = report.details.unwrap();
    assert_eq!(d["rule"], "yaml.syntax");
    assert!(d["location"]["line"].as_u64().unwrap() >= 3);
    assert!(d["location"]["column"].as_u64().unwrap() >= 1);
    assert!(d["location"]["pointer"].is_null());
    let body = format!(
        "{}\n路径：目录/含 空格/资料.yaml",
        "这是完整中文说明，不能为了通过解析而删掉事实。".repeat(200)
    );
    let text = format!(
        "work_items:\n- id: W\n  title: 正常工作\n  summary: {}\n  paths: ['资料/含 空格.yaml']\n",
        serde_json::to_string(&body).unwrap()
    );
    let result = parse(&text, "yaml-ledger-v1", "").unwrap();
    assert_eq!(result.work_items[0].summary, body);
    assert_eq!(result.work_items[0].paths, ["资料/含 空格.yaml"]);
}

#[test]
fn aliases_and_complex_yaml_keep_honest_locations() {
    for (field, pointer) in [
        ("required_for_v1: quoted", "/work_items/0/required_for_v1"),
        (
            "dependencies: [{key: 42}]",
            "/work_items/0/dependencies/0/key",
        ),
        ("deliverables: [42]", "/work_items/0/deliverables/0"),
    ] {
        let report = parse(
            &format!("work_items:\n- id: W\n  {field}\n"),
            "yaml-ledger-v1",
            "",
        )
        .unwrap_err()
        .report();
        let d = report.details.unwrap();
        assert_eq!(d["location"]["pointer"], pointer);
        assert_eq!(d["location"]["line"], 3);
    }
    let report = parse(
        "bad: &bad [value]\nwork_items:\n- id: W\n  title: *bad\n",
        "yaml-ledger-v1",
        "",
    )
    .unwrap_err()
    .report();
    let d = report.details.unwrap();
    assert_eq!(d["location"]["pointer"], "/work_items/0/title");
    assert!(d["location"]["line"].is_null());
    assert!(d["location"]["column"].is_null());
}
