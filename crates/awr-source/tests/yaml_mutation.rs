use awr_core::*;
use awr_source::{Manifest, index_project, prepare_yaml_mutation};
use awr_store::Store;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl Fixture {
    fn new(text: &str) -> Self {
        Self::with_options(text, "")
    }
    fn with_options(text: &str, options: &str) -> Self {
        let root = std::env::temp_dir().join(format!("awr-yaml-mutation-{}", Id::new()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ledger.yaml"), text).unwrap();
        let manifest:Manifest=Manifest::parse(&format!("[project]\nname='Mutation source'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='ledger.yaml'\nadapter='yaml-ledger-v1'\n{options}")).unwrap();
        fs::create_dir(root.join(".awr")).unwrap();
        fs::write(
            root.join(".awr/project.toml"),
            toml::to_string(&manifest).unwrap(),
        )
        .unwrap();
        let mut store = Store::open(&root.join("state.db")).unwrap();
        let report = index_project(&mut store, &root, &manifest, false).unwrap();
        assert!(report.ok, "{:?}", report.issues);
        Self {
            root,
            store,
            project: report.project_id,
        }
    }
    fn plan(
        &self,
        kind: EntityKind,
        key: &str,
        changes: Value,
    ) -> Result<awr_source::PreparedYamlMutation> {
        let target = self.store.mutation_target(self.project, kind, key)?;
        let patch = MutationPatch {
            version: 1,
            host_edit: None,
            work_action: None,
            target: MutationTarget {
                kind,
                meta: serde_json::from_value(target.item)?,
            },
            source_config: target.source.config.clone(),
            intent: "Update exactly this source record".into(),
            changes,
        };
        let proposal = MutationProposal {
            id: Id::new(),
            project_id: self.project,
            work_item_id: None,
            source_id: target.source.id,
            base_fingerprint: target.source.fingerprint.clone(),
            expected_revision: target.project_revision,
            mutation_type: "update_fields".into(),
            patch: serde_json::to_value(patch)?,
            status: ProposalStatus::Approved,
            created_by_session: None,
            revision: 1,
        };
        prepare_yaml_mutation(
            &self.root,
            &target.source,
            &proposal,
            self.store.projection_ids(&target.source)?,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn parsed(text: &str) -> Value {
    serde_json::to_value(serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text).unwrap()).unwrap()
}

#[test]
fn mapped_yaml_writer_retains_original_keys_and_other_records() {
    let text = "# retained\nwork_items:\n- ticket: W\n  name: Report\n  phase: Pending\n  next: Before\n- ticket: OTHER\n  name: Unrelated\n  phase: Pending\n";
    let f = Fixture::with_options(
        text,
        "[sources.options.field_map]\nid='ticket'\ntitle='name'\nstatus='phase'\nnext_action='next'\n[sources.options.status_map]\nPending='planned'\nComplete='completed'\n",
    );
    let plan = f
        .plan(EntityKind::WorkItem, "W", json!({"next_action":"After"}))
        .unwrap();
    let after = parsed(plan.after.text().unwrap());
    assert_eq!(
        plan.after.text().unwrap(),
        text.replace("next: Before", "next: After")
    );
    assert_eq!(after["work_items"][0]["next"], "After");
    assert!(after["work_items"][0].get("next_action").is_none());
    assert_eq!(after["work_items"][0]["phase"], "Pending");
    assert_eq!(after["work_items"][1], parsed(text)["work_items"][1]);
    assert_eq!(
        fs::read_to_string(f.root.join("ledger.yaml")).unwrap(),
        text
    );
    assert!(matches!(
        f.plan(EntityKind::WorkItem, "W", json!({"status":"completed"})),
        Err(Error::MutationUnsupported(_))
    ));
}

#[test]
fn one_field_keeps_all_other_bytes_comments_key_order_quotes_and_line_endings() {
    for newline in ["\n", "\r\n"] {
        let text = [
            "# 项目",
            "work_items:",
            "- id: W  # stable",
            "  title: 'Keep my title'",
            "  status: ready",
            "  next_action: \"Before\" # keep inline",
            "  summary: >-",
            "    Preserve this folded",
            "    description exactly.",
            "  tags: [first, 'second']",
            "# trailing comment",
            "- id: OTHER",
            "  title: Do not touch",
            "",
        ]
        .join(newline);
        let f = Fixture::new(&text);
        let plan = f
            .plan(
                EntityKind::WorkItem,
                "W",
                json!({"next_action":"新的动作 🐾"}),
            )
            .unwrap();
        assert_eq!(
            plan.after.text().unwrap(),
            text.replace("\"Before\"", "\"新的动作 🐾\"")
        );
        assert_eq!(
            fs::read(f.root.join("ledger.yaml")).unwrap(),
            text.as_bytes()
        );
    }
}

#[test]
fn quoted_fields_keep_quote_style_and_escape_new_content_without_touching_neighbors() {
    for (old, new, encoded) in [
        ("'Before'", "Don't normalize", "'Don''t normalize'"),
        ("\"Before\"", "Quoted \"value\"", "\"Quoted \\\"value\\\"\""),
        ("Before", "false", "\"false\""),
    ] {
        let text = format!(
            "work_items: [{{id: W, status: ready, next_action: {old}, title: 'Keep'}}] # comment\n"
        );
        let f = Fixture::new(&text);
        let plan = f
            .plan(EntityKind::WorkItem, "W", json!({"next_action":new}))
            .unwrap();
        assert_eq!(plan.after.text().unwrap(), text.replace(old, encoded));
    }
}

#[test]
fn new_field_and_empty_scalar_preserve_existing_inline_and_trailing_comments() {
    for text in [
        "work_items:\n- id: W\n  status: ready # state note\n# outside\n- id: OTHER\n  status: ready\n",
        "work_items: [{id: W, status: ready}, {id: OTHER, status: ready}] # outside\n",
        "work_items:\n  W:\n    status: ready # state note",
        "work_items:\n- id: W\n  status: ready\n  summary: # keep empty comment\n  next_action: Before\n",
    ] {
        let f = Fixture::new(text);
        let plan = f
            .plan(EntityKind::WorkItem, "W", json!({"summary":"New summary"}))
            .unwrap();
        let output = plan.after.text().unwrap();
        assert!(output.contains("status: ready"));
        for comment in ["# state note", "# outside", "# keep empty comment"] {
            if text.contains(comment) {
                assert!(output.contains(comment));
            }
        }
        let mut expected = parsed(text);
        if expected["work_items"].is_array() {
            expected["work_items"][0]["summary"] = json!("New summary");
        } else {
            expected["work_items"]["W"]["summary"] = json!("New summary");
        }
        assert_eq!(parsed(output), expected);
    }
}

#[test]
fn block_field_edits_keep_literal_or_folded_style_header_comment_and_neighbor_bytes() {
    for newline in ["\n", "\r\n"] {
        for style in ["|-", ">-", "|2-", ">2-"] {
            let prefix = format!(
                "work_items:{newline}- id: W{newline}  status: ready{newline}  next_action: "
            );
            let suffix = format!("  title: 'Untouched'{newline}# outside{newline}");
            let text =
                format!("{prefix}{style} # keep header{newline}    Old line{newline}{suffix}");
            let f = Fixture::new(&text);
            for value in [
                "First line\nsecond line",
                "Paragraph one\n\nparagraph two\n",
                "Keep a final blank\n\n",
            ] {
                let plan = f
                    .plan(EntityKind::WorkItem, "W", json!({"next_action":value}))
                    .unwrap();
                let out = plan.after.text().unwrap();
                assert!(out.starts_with(&format!("{prefix}{}", &style[..1])));
                assert!(out.contains("# keep header"));
                assert!(out.ends_with(&suffix));
                assert_eq!(parsed(out)["work_items"][0]["next_action"], value);
                if newline == "\r\n" {
                    assert!(!out.replace("\r\n", "").contains('\n'));
                }
            }
        }
    }
}

#[test]
fn multiline_plain_fields_are_preserved_or_edited_at_the_exact_span() {
    let text = "work_items:\n- id: W\n  status: ready\n  title: A plain title continued\n    on another line\n  next_action: Before\n";
    let f = Fixture::new(text);
    assert_eq!(
        f.plan(EntityKind::WorkItem, "W", json!({"next_action":"After"}))
            .unwrap()
            .after
            .text()
            .unwrap(),
        text.replace("Before", "After")
    );
    assert_eq!(
        f.plan(EntityKind::WorkItem, "W", json!({"title":"Changed"}))
            .unwrap()
            .after
            .text()
            .unwrap(),
        text.replace("A plain title continued\n    on another line", "Changed")
    );
    assert_eq!(
        fs::read_to_string(f.root.join("ledger.yaml")).unwrap(),
        text
    );
}

#[test]
fn wrapped_plain_edits_preserve_trailing_comments_and_newline_style() {
    for newline in ["\n", "\r\n"] {
        let text = "work_items:\n- id: W\n  status: ready\n  title: A plain title\n    continued here # keep this comment\n  next_action: Before\n".replace('\n', newline);
        let f = Fixture::new(&text);
        let expected = text.replace(
            &format!("A plain title{newline}    continued here"),
            "Changed",
        );
        assert_eq!(
            f.plan(EntityKind::WorkItem, "W", json!({"title":"Changed"}))
                .unwrap()
                .after
                .text()
                .unwrap(),
            expected
        );
        assert_eq!(
            fs::read_to_string(f.root.join("ledger.yaml")).unwrap(),
            text
        );
    }
}

#[test]
fn comment_bearing_collection_and_anchor_edits_fail_before_writing() {
    for text in [
        "work_items:\n- id: W\n  status: ready\n  tags:\n  - old # a tag comment\n  next_action: Before\n",
        "work_items:\n- id: W\n  status: ready\n  tags: &tags [old]\nextra: *tags\n",
    ] {
        let f = Fixture::new(text);
        assert!(matches!(
            f.plan(EntityKind::WorkItem, "W", json!({"tags":["new"]})),
            Err(Error::MutationUnsupported(_))
        ));
        assert_eq!(
            fs::read_to_string(f.root.join("ledger.yaml")).unwrap(),
            text
        );
    }
}

#[test]
fn exact_record_spans_preserve_other_bytes_across_block_flow_unicode_and_crlf() {
    for text in [
        "# 前面的注释\nwork_items:\n- id: BEFORE\n  title: 中文和 emoji 🐾\n  status: ready\n- id: W\n  title: Report\n  status: in_progress\n  next_action: Before\n# Keep this trailing comment\n- id: AFTER\n  status: ready\n",
        "work_items: [{id: W, status: ready, next_action: 'Before, again'}, {id: AFTER, status: ready}] # 外面\n",
        "work_items:\r\n  W:\r\n    title: Report\r\n    status: ready\r\n    next_action: >\r\n      First line\r\n      second line\r\n  AFTER:\r\n    status: ready\r\n",
    ] {
        let f = Fixture::new(text);
        let plan=f.plan(EntityKind::WorkItem,"W",json!({"next_action":"完成后继续 🐾\nsecond line","summary":"Keep\u{85}special\u{2028}breaks"})).unwrap();
        let before = parsed(text);
        let after = parsed(plan.after.text().unwrap());
        let pointer = if before["work_items"].is_array() {
            if before["work_items"][0]["id"] == "W" {
                "/work_items/0"
            } else {
                "/work_items/1"
            }
        } else {
            "/work_items/W"
        };
        let mut expected = before.clone();
        let object = expected.pointer_mut(pointer).unwrap();
        object["next_action"] = json!("完成后继续 🐾\nsecond line");
        object["summary"] = json!("Keep\u{85}special\u{2028}breaks");
        assert_eq!(after, expected);
        assert_eq!(
            fs::read_to_string(f.root.join("ledger.yaml")).unwrap(),
            text
        );
        if text.contains("# Keep") {
            assert!(
                plan.after
                    .text()
                    .unwrap()
                    .contains("\n# Keep this trailing comment\n- id: AFTER\n  status: ready\n")
            );
        }
        if text.contains("\r\n") {
            assert!(
                plan.after
                    .text()
                    .unwrap()
                    .ends_with("\r\n  AFTER:\r\n    status: ready\r\n")
            );
        }
        assert_ne!(plan.plan.before_fingerprint, plan.plan.after_fingerprint);
        plan.plan.validate().unwrap();
    }
}

#[test]
fn goals_milestones_and_structured_evidence_keep_identity_and_pointer() {
    let f = Fixture::new(
        "goals:\n  G/~1:\n    title: Goal\n    status: active\nmilestones:\n- id: M\n  name: Delivery\n  status: planned\nwork_items:\n- id: W\n  status: ready\n  evidence:\n  - locator: reports/a.json\n    summary: Before\n",
    );
    for (kind, key, changes, pointer) in [
        (
            EntityKind::Goal,
            "G/~1",
            json!({"summary":"A clearer goal"}),
            "/goals/G~1~01/summary",
        ),
        (
            EntityKind::Plan,
            "M",
            json!({"status":"active"}),
            "/milestones/0/status",
        ),
        (
            EntityKind::Evidence,
            "W/evidence/reports/a.json",
            json!({"summary":"A retained source reference"}),
            "/work_items/0/evidence/0/summary",
        ),
    ] {
        let plan = f.plan(kind, key, changes.clone()).unwrap();
        assert_eq!(
            parsed(plan.after.text().unwrap()).pointer(pointer).unwrap(),
            changes.as_object().unwrap().values().next().unwrap()
        );
    }
}

#[test]
fn unsafe_shapes_guarded_fields_and_ignored_fields_remain_manual() {
    let f = Fixture::new("work_items:\n- id: W\n  status: ready\n  evidence: [reports/a.json]\n");
    for change in [
        json!({"status":"completed"}),
        json!({"owner":"different"}),
        json!({"verification":{"evidence_level":"released"}}),
        json!({"unknown":"ignored"}),
    ] {
        assert!(matches!(
            f.plan(EntityKind::WorkItem, "W", change),
            Err(Error::MutationUnsupported(_))
        ));
    }
    assert!(matches!(
        f.plan(
            EntityKind::Evidence,
            "W/evidence/reports/a.json",
            json!({"summary":"New"})
        ),
        Err(Error::MutationUnsupported(_))
    ));
    let f = Fixture::new(
        "work_items:\n- &target\n  id: W\n  status: ready\n  next_action: Before\nextra: *target\n",
    );
    assert!(matches!(
        f.plan(EntityKind::WorkItem, "W", json!({"next_action":"New"})),
        Err(Error::MutationUnsupported(_))
    ));
}

#[test]
fn drift_noop_and_invalid_field_types_never_prepare_an_apply_plan() {
    let f = Fixture::new("work_items:\n- id: W\n  status: ready\n  next_action: Before\n");
    assert!(matches!(
        f.plan(EntityKind::WorkItem, "W", json!({"next_action":"Before"})),
        Err(Error::InvalidInput(_))
    ));
    let invalid = f
        .plan(EntityKind::WorkItem, "W", json!({"next_action":123}))
        .err()
        .expect("invalid field must not prepare a mutation")
        .report();
    assert_eq!(invalid.code, "InvalidInput");
    let details = invalid.details.unwrap();
    assert_eq!(details["rule"], "ledger.string");
    assert_eq!(details["location"]["pointer"], "/work_items/0/next_action");
    fs::write(
        f.root.join("ledger.yaml"),
        "work_items: [{id: W, status: ready, next_action: Changed}]\n",
    )
    .unwrap();
    assert!(matches!(
        f.plan(EntityKind::WorkItem, "W", json!({"next_action":"New"})),
        Err(Error::SourceConflict(_))
    ));
}

#[test]
fn a_plain_scalar_with_a_trailing_comment_keeps_the_comment_on_rewrite() {
    // Deliberate design: field edits preserve same-line comments. A plain scalar
    // whose authored value contains " #" truncates at the comment at index time, so
    // the comment tail survives later rewrites. Authors must quote such values.
    let text = "work_items:\n- id: W\n  status: ready\n  title: Keep\n  next_action: Before #322 review tail\n";
    let f = Fixture::new(text);
    let plan = f
        .plan(EntityKind::WorkItem, "W", json!({"next_action":"After"}))
        .unwrap();
    assert_eq!(
        plan.after.text().unwrap(),
        "work_items:\n- id: W\n  status: ready\n  title: Keep\n  next_action: After #322 review tail\n"
    );
}

#[test]
fn multibyte_block_scalar_positions_stay_exact_for_later_field_edits() {
    // Upstream yaml-rust2 advanced mark.index by BYTES inside the block-scalar
    // content fast path while every other path counts CHARACTERS, so markers
    // after a multi-byte block scalar drifted and byte-level edits were refused
    // with "missing YAML mapping colon" (or would mislocate fields). The
    // vendored copy restores character accounting; these ledgers must edit
    // exactly the target field and leave every other byte intact.
    for text in [
        "work_items:\n- id: W\n  status: ready\n  summary: |\n    第一行说明\n    第二行说明\n  next_action: Before\n- id: AFTER\n  status: ready\n",
        "work_items:\n- id: W\n  status: ready\n  summary: >\n    中文备注 🐾 带上 emoji\n    第二行也在块标量里\n  owner: alice\n  next_action: Before\n",
        "work_items:\r\n- id: W\r\n  status: ready\r\n  summary: |\r\n    中文内容\r\n    第二行\r\n  next_action: Before\r\n- id: AFTER\r\n  status: ready\r\n",
    ] {
        let f = Fixture::new(text);
        let plan = f
            .plan(EntityKind::WorkItem, "W", json!({"next_action":"After"}))
            .unwrap();
        let after = plan.after.text().unwrap();
        assert_eq!(
            parsed(after).pointer("/work_items/0/next_action"),
            Some(&json!("After")),
            "target field must change: {text}"
        );
        let mut expected = parsed(text);
        expected["work_items"][0]["next_action"] = json!("After");
        assert_eq!(parsed(after), expected, "unrelated records must survive");
        assert!(
            after.contains("第一行说明")
                || after.contains("中文备注")
                || after.contains("中文内容"),
            "block scalar content bytes must survive verbatim"
        );
        assert_ne!(plan.plan.before_fingerprint, plan.plan.after_fingerprint);
        plan.plan.validate().unwrap();
    }
    // Editing a field BEFORE the block scalar must keep working too, and the
    // block scalar bytes must be preserved exactly.
    let text = "work_items:\n- id: W\n  status: ready\n  next_action: Before\n  summary: |\n    第一行说明\n    第二行说明\n";
    let f = Fixture::new(text);
    let plan = f
        .plan(EntityKind::WorkItem, "W", json!({"next_action":"After"}))
        .unwrap();
    assert_eq!(
        plan.after.text().unwrap(),
        text.replace("next_action: Before", "next_action: After")
    );
    plan.plan.validate().unwrap();
}
