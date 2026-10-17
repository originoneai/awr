#[path = "../../awr-store/tests/support/mod.rs"]
mod support;
use awr_context::{RuleScopeInput, hard_context, select_rules};
use awr_core::*;
use awr_store::SourceRegistration;
use support::Fixture;

fn fixture(path: &str, unknown: bool) -> Fixture {
    let mut f = Fixture::new();
    let work = WorkItem {
        spec_refs: vec![],
        ordinary_completion: None,
        archived: false,
        meta: f.meta("W"),
        title: "Hard facts".into(),
        kind: None,
        owner: None,
        required: true,
        raw_status: "blocked".into(),
        status: WorkStatus::Blocked,
        priority: None,
        milestone: Some("M4".into()),
        score: None,
        evidence_level: None,
        summary: "Optional summary".into(),
        next_action: "Preserve next action\nexactly, including punctuation.".into(),
        blocker: Some("Await source conflict resolution\nwithout overwriting.".into()),
        acceptance: vec![
            "Exact criterion one\nwith two lines".into(),
            format!(
                "{}CRITERION_TAIL",
                "Required acceptance clause. ".repeat(2000)
            ),
        ],
        tags: vec!["rust".into()],
        paths: vec![path.into()],
    };
    f.commit(ProjectionBatch {
        work_items: vec![work],
        ..Default::default()
    });
    let source = f
        .store
        .register_source(
            f.project.id,
            &SourceRegistration {
                domain: "rules",
                role: "primary",
                locator: "file:///fixture/rules.md",
                format: "markdown",
                adapter: "markdown-rules-v1",
            },
        )
        .unwrap();
    let mut rules = Vec::new();
    for (key, kind, value, severity) in [
        ("project", ScopeKind::Project, "*", Severity::Hard),
        ("path", ScopeKind::Path, "src/**", Severity::Hard),
        ("tag", ScopeKind::Tag, "rust", Severity::Hard),
        ("work", ScopeKind::WorkItem, "W", Severity::Hard),
        ("agent", ScopeKind::Agent, "executor", Severity::Hard),
        ("other", ScopeKind::Path, "docs/**", Severity::Hard),
        ("soft", ScopeKind::Project, "*", Severity::Soft),
        ("info", ScopeKind::Project, "*", Severity::Info),
    ] {
        let mut meta = f.meta(key);
        meta.source_ref = SourceRef {
            source_id: source.id,
            locator: source.locator.clone(),
            source_revision: 1,
            source_fingerprint: "rules-snapshot".into(),
            pointer: Some(format!("#{key}")),
            start_line: Some(1),
            end_line: Some(3),
            section_fingerprint: Some(format!("section-{key}")),
        };
        rules.push(Rule {
            meta,
            text: format!("Hard {key}\nPreserve this exact obligation, including its wording."),
            severity: Some(severity),
            scope: Some(Scope {
                kind,
                value: value.into(),
            }),
            unresolved: vec![],
        });
    }
    if unknown {
        let mut rule = rules[0].clone();
        rule.meta.id = Id::new();
        rule.meta.external_key = "unknown".into();
        rule.severity = None;
        rule.scope = None;
        rule.unresolved = vec!["missing metadata".into()];
        rules.push(rule);
    }
    f.store
        .commit_source_projection(
            &source,
            "rules-snapshot",
            ProjectionBatch {
                rules,
                ..Default::default()
            },
        )
        .unwrap();
    f
}
fn scope() -> RuleScopeInput {
    RuleScopeInput {
        agent_id: Some("executor".into()),
        ..Default::default()
    }
}

#[test]
fn hard_context_preserves_exact_acceptance_state_rules_and_provenance() {
    let f = fixture("src/main.rs", false);
    let work = f.store.work_item(f.project.id, "W").unwrap();
    let hard = hard_context(&f.store, f.project.id, "W", None, &scope()).unwrap();
    assert!(hard.complete);
    assert_eq!(hard.rules.len(), 5);
    assert_eq!(hard.work.acceptance, work.item.acceptance);
    assert_eq!(hard.work.next_action, work.item.next_action);
    assert_eq!(hard.work.blocker, work.item.blocker);
    assert_eq!(hard.work.raw_status, "blocked");
    assert_eq!(hard.work.status, WorkStatus::Blocked);
    assert_eq!(
        serde_json::to_value(&hard.work.meta).unwrap(),
        serde_json::to_value(&work.item.meta).unwrap()
    );
    for rule in &hard.rules {
        let original = f.store.rule(f.project.id, &rule.meta.external_key).unwrap();
        assert_eq!(
            serde_json::to_value(rule).unwrap(),
            serde_json::to_value(original.item).unwrap()
        );
    }
    let serialized = serde_json::to_string(&hard).unwrap();
    assert!(serialized.len() > 50000);
    assert!(serialized.contains("CRITERION_TAIL"));
    let again = hard_context(&f.store, f.project.id, "W", None, &scope()).unwrap();
    assert_eq!(serialized, serde_json::to_string(&again).unwrap());
    let project = f.store.project(f.project.id).unwrap();
    let selected = select_rules(&f.store, &project, Some(&work), &scope()).unwrap();
    assert_eq!(selected.soft.len(), 1);
    assert_eq!(selected.info.len(), 1);
    assert_eq!(selected.not_applicable.len(), 1);
    assert!(selected.unknown.is_empty());
    assert!(matches!(
        hard_context(&f.store, f.project.id, "W", Some(Id::new()), &scope()),
        Err(Error::NotFound(_))
    ));
    let identity = awr_context::ContextIdentity {
        project_id: project.id,
        project_key: project.external_key,
        project_revision: project.project_revision,
        work_item_id: hard.work.meta.id,
        work_item_key: "W".into(),
        work_item_revision: hard.work.meta.revision,
        branch_id: None,
        source_versions: hard.source_revisions.clone(),
    };
    let chunks = awr_context::hard_chunks(&hard).unwrap();
    assert!(matches!(
        awr_context::budget_context(&identity, &serde_json::json!({}), &chunks, &[], 1000),
        Err(Error::BudgetExceeded { .. })
    ));
    let pack = awr_context::budget_context(&identity, &serde_json::json!({}), &chunks, &[], 30000)
        .unwrap();
    for text in hard
        .work
        .acceptance
        .iter()
        .chain([&hard.work.next_action, hard.work.blocker.as_ref().unwrap()])
        .chain(hard.rules.iter().map(|r| &r.text))
    {
        assert!(pack.rendered_context.contains(text));
    }
    assert!(pack.rendered_context.contains("CRITERION_TAIL"));
    assert_eq!(pack.selected_entities.len(), 6);
    assert!(pack.selected_chunks.iter().all(|c| c.required));
}

#[test]
fn broad_paths_and_missing_metadata_are_unknown_until_scope_is_explicit() {
    let f = fixture("src/", true);
    let hard = hard_context(&f.store, f.project.id, "W", None, &scope()).unwrap();
    assert!(!hard.complete);
    assert_eq!(hard.paths_origin, "unknown_broad_scope");
    assert!(
        hard.unresolved
            .iter()
            .any(|r| r.rule.item.meta.external_key == "path")
    );
    assert!(
        hard.unresolved
            .iter()
            .any(|r| r.rule.item.meta.external_key == "unknown")
    );
    let input = RuleScopeInput {
        agent_id: Some("executor".into()),
        paths: Some(vec!["src/main.rs".into()]),
        tags: Some(vec![]),
    };
    let hard = hard_context(&f.store, f.project.id, "W", None, &input).unwrap();
    assert_eq!(hard.rules.len(), 5);
    assert_eq!(hard.paths_origin, "caller");
    assert_eq!(hard.unresolved.len(), 1);
    assert!(!hard.complete);
    assert!(hard.scope.tags.as_ref().unwrap().contains(&"rust".into()));
    let invalid = RuleScopeInput {
        agent_id: Some("".into()),
        ..Default::default()
    };
    assert!(matches!(
        hard_context(&f.store, f.project.id, "W", None, &invalid),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn stale_rule_sources_never_produce_complete_hard_context() {
    let mut f = fixture("src/main.rs", false);
    let source = f.store.rules(f.project.id).unwrap()[0].source.clone();
    f.store
        .mark_source_freshness(&source, Freshness::Unavailable)
        .unwrap();
    let hard = hard_context(&f.store, f.project.id, "W", None, &scope()).unwrap();
    assert!(!hard.complete);
    assert!(!hard.unresolved.is_empty());
    assert!(
        hard.source_revisions
            .iter()
            .any(|s| s.freshness == Freshness::Unavailable)
    );
    assert!(hard.issues.iter().any(|s| s.contains("not fresh")));
}
