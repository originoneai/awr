use awr_context::{CompletenessRequest, RuleScopeInput, check_completeness, inspect_completeness};
use awr_core::*;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use std::{fs, path::PathBuf};

const WORK: &str = "work_items:\n- id: W\n  title: Implement feature\n  status: in_progress\n  next_action: Continue implementation\n  acceptance: [Deliver the feature]\n  depends_on: [DEP]\n- id: DEP\n  title: Required input\n  status: blocked\n  blocker: Await source input\n  next_action: Request missing source\n  acceptance: [Source is provided]\n";
const RULES: &str =
    "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve source facts.\n";
const MANIFEST: &str = "[project]\nname='Completeness'\nexternal_key='completeness'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n";
struct Fixture {
    root: PathBuf,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-completeness-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::create_dir(root.join("decisions")).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(root.join("rules.md"), RULES).unwrap();
        fs::write(root.join(".awr/project.toml"), MANIFEST).unwrap();
        let store = Store::open(&root.join(".awr/state.db")).unwrap();
        Self { root, store }
    }
    fn check(&mut self) -> awr_context::ContextCompleteness {
        check_completeness(&mut self.store, &self.root, &request("W")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn request(key: &str) -> CompletenessRequest {
    CompletenessRequest {
        work_item_key: key.into(),
        branch_id: None,
        scope: RuleScopeInput {
            agent_id: Some("executor".into()),
            ..Default::default()
        },
        source_sha: None,
    }
}

#[test]
fn complete_context_retains_pending_dependencies_and_evidence_gaps_without_promoting_work() {
    let mut f = Fixture::new();
    let first = f.check();
    assert!(first.complete);
    assert_eq!(first.status, "CONTEXT COMPLETE");
    assert!(
        first.source_fresh
            && first.work_item_found
            && first.acceptance_complete
            && first.rules_complete
            && first.dependencies_complete
            && first.decision_context_complete
    );
    assert_eq!(first.unresolved_required_dependencies, ["DEP"]);
    assert!(first.evidence_gaps.iter().any(|g| g.code == "no_evidence"));
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&f.check()).unwrap()
    );
    let snapshot = inspect_completeness(&f.store, first.project_id, &request("W"), None).unwrap();
    assert!(!snapshot.source_fresh && !snapshot.complete && !snapshot.source_refresh_performed);
    assert_eq!(snapshot.status, "CONTEXT INCOMPLETE");
    assert!(
        snapshot
            .issues
            .iter()
            .any(|i| i.code == "source_refresh_missing")
    );
    assert_eq!(fs::read_to_string(f.root.join("work.yaml")).unwrap(), WORK);
    assert_eq!(
        f.store
            .work_item(first.project_id, "W")
            .unwrap()
            .item
            .status,
        WorkStatus::InProgress
    );
    let refresh = index_project(
        &mut f.store,
        &f.root,
        &Manifest::load(&f.root).unwrap(),
        false,
    )
    .unwrap();
    f.store
        .record_evidence(
            first.project_id,
            refresh.project_revision,
            EvidenceDraft {
                external_key: "REPORT".into(),
                work_item_key: Some("W".into()),
                evidence_type: "report".into(),
                level: EvidenceLevel::Implemented,
                summary: "Implementation reported; validation pending".into(),
                locator: "missing-report.json".into(),
                sha256: None,
                source_sha: None,
                command: None,
                scope: vec!["W".into()],
                branch_id: None,
                verified_at: None,
            },
        )
        .unwrap();
    assert!(matches!(
        inspect_completeness(&f.store, first.project_id, &request("W"), Some(&refresh)),
        Err(Error::RevisionConflict { .. })
    ));
    let with_evidence = f.check();
    assert!(with_evidence.complete);
    assert!(
        with_evidence
            .evidence_gaps
            .iter()
            .any(|g| g.code == "missing_evidence_bindings")
    );
    assert!(
        with_evidence
            .evidence_gaps
            .iter()
            .any(|g| g.code == "evidence_currency_unknown")
    );
    assert_eq!(
        f.store
            .work_item(first.project_id, "W")
            .unwrap()
            .item
            .status,
        WorkStatus::InProgress
    );
}

#[test]
fn missing_or_unknown_hard_facts_and_invalid_dependency_graph_are_explicit_and_recoverable() {
    let mut f = Fixture::new();
    assert!(f.check().complete);
    let broken = WORK
        .replace("status: in_progress", "status: vendor_stage")
        .replace("acceptance: [Deliver the feature]", "acceptance: []")
        .replace("next_action: Continue implementation", "next_action: ''")
        .replace("depends_on: [DEP]", "depends_on: [DEP, MISSING]")
        .replace(
            "acceptance: [Source is provided]",
            "acceptance: [Source is provided]\n  depends_on: [W]",
        );
    fs::write(f.root.join("work.yaml"), &broken).unwrap();
    fs::write(
        f.root.join("rules.md"),
        "# Unclassified rule\n\nA possibly hard obligation.\n",
    )
    .unwrap();
    fs::write(
        f.root.join("decisions/adr.md"),
        "# Decision\n\nStatus: accepted\n\n## Decision\n\nUse one shared source.\n",
    )
    .unwrap();
    let report = f.check();
    assert!(!report.complete);
    assert_eq!(report.status, "CONTEXT INCOMPLETE");
    assert!(
        !report.acceptance_complete
            && !report.work_state_complete
            && !report.rules_complete
            && !report.dependencies_complete
            && !report.decision_context_complete
    );
    for code in [
        "acceptance_missing_or_stale",
        "work_state_incomplete",
        "hard_rule_unresolved",
        "required_dependency_missing",
        "dependency_cycle",
        "decision_unresolved",
    ] {
        assert!(
            report.issues.iter().any(|i| i.code == code),
            "missing {code}: {:?}",
            report.issues
        );
    }
    assert_eq!(
        fs::read_to_string(f.root.join("work.yaml")).unwrap(),
        broken
    );
    fs::write(f.root.join("work.yaml"), WORK).unwrap();
    fs::write(f.root.join("rules.md"), RULES).unwrap();
    fs::write(f.root.join("decisions/adr.md"),"---\naffected_keys: [W]\n---\n\n# Decision\n\nStatus: accepted\n\n## Decision\n\nUse one shared source.\n").unwrap();
    assert!(f.check().complete);
}

#[test]
fn failed_new_mappings_stale_work_and_missing_work_never_become_current_context() {
    let mut f = Fixture::new();
    let first = f.check();
    assert!(first.complete);
    // No file has ever been indexed from this directory, so no cached Source row can prove availability.
    fs::write(f.root.join(".awr/project.toml"),format!("{MANIFEST}\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='missing-decisions'\nadapter='markdown-directory-v1'\n")).unwrap();
    let failed = f.check();
    assert!(!failed.source_fresh && !failed.decision_context_complete && !failed.complete);
    assert!(failed.dependencies_complete); // Independent ledger/edge authority is still current.
    assert!(
        failed
            .issues
            .iter()
            .any(|i| i.code == "source_refresh_failed")
    );
    fs::write(f.root.join(".awr/project.toml"), MANIFEST).unwrap();
    fs::remove_file(f.root.join("work.yaml")).unwrap();
    let stale = f.check();
    assert!(stale.work_item_found);
    assert!(
        !stale.source_fresh
            && !stale.acceptance_complete
            && !stale.work_state_complete
            && !stale.dependencies_complete
    );
    assert!(
        stale
            .source_versions
            .iter()
            .any(|s| s.freshness == Freshness::Unavailable)
    );
    let missing = check_completeness(&mut f.store, &f.root, &request("ABSENT")).unwrap();
    assert!(!missing.work_item_found && !missing.complete);
    assert!(
        missing
            .issues
            .iter()
            .any(|i| i.code == "work_item_not_found")
    );
    fs::write(f.root.join("work.yaml"), WORK).unwrap();
    assert!(f.check().complete);
}

#[test]
fn completeness_unannotated_rule_explains_the_heading_suffix_and_recovers() {
    let mut f = Fixture::new();
    let heading = "# Preserve source facts";
    fs::write(
        f.root.join("rules.md"),
        format!("{heading}\n\nKeep source facts.\n"),
    )
    .unwrap();
    let report = f.check();
    assert!(!report.rules_complete);
    let reason = &report
        .issues
        .iter()
        .find(|i| i.code == "hard_rule_unresolved")
        .unwrap()
        .reason;
    assert!(reason.contains("end of the heading line"), "{reason}");
    let annotation = "{severity=hard scope=project value=*}";
    assert!(reason.contains(annotation), "{reason}");
    fs::write(
        f.root.join("rules.md"),
        format!("{heading} {annotation}\n\nKeep source facts.\n"),
    )
    .unwrap();
    assert!(f.check().rules_complete);
}

#[test]
fn completeness_rule_annotation_in_the_body_does_not_classify_the_heading() {
    let mut f = Fixture::new();
    fs::write(
        f.root.join("rules.md"),
        "# Preserve source facts\n\n{severity=hard scope=project value=*}\n",
    )
    .unwrap();
    let report = f.check();
    assert!(!report.rules_complete);
    let reason = &report
        .issues
        .iter()
        .find(|i| i.code == "hard_rule_unresolved")
        .unwrap()
        .reason;
    assert!(reason.contains("severity unresolved"), "{reason}");
    assert!(reason.contains("end of the heading line"), "{reason}");
}

#[test]
fn ledger_evidence_reference_inherits_registered_verification_bindings() {
    // work complete writes bound report locators into the work record's
    // evidence list. Projecting that reference must not mint a second, unbound
    // shadow row: it inherits the registered record's bindings, so completeness
    // diagnostics show neither missing_evidence_bindings nor
    // evidence_level_unverified for the completed work.
    let mut f = Fixture::new();
    let refresh = index_project(
        &mut f.store,
        &f.root,
        &Manifest::load(&f.root).unwrap(),
        false,
    )
    .unwrap();
    let report_bytes = br#"{"version":1,"work_item":"W","checks":[]}"#;
    fs::write(f.root.join("report.json"), report_bytes).unwrap();
    use sha2::{Digest, Sha256};
    let sha256 = Sha256::digest(report_bytes);
    let sha256 = hex(&sha256);
    let source_sha = "1".repeat(40);
    f.store
        .record_evidence(
            refresh.project_id,
            refresh.project_revision,
            EvidenceDraft {
                external_key: "W-REPORT".into(),
                work_item_key: Some("W".into()),
                evidence_type: "verification_report".into(),
                level: EvidenceLevel::LocallyVerified,
                summary: "Registered verification of report.json".into(),
                locator: "report.json".into(),
                sha256: Some(sha256.clone()),
                source_sha: Some(source_sha.clone()),
                command: Some("cargo test verification".into()),
                scope: vec!["W".into()],
                branch_id: None,
                verified_at: Some(1_794_000_000_000),
            },
        )
        .unwrap();
    // The completed record references the same locator the caller registered.
    fs::write(
        f.root.join("work.yaml"),
        "work_items:\n- id: W\n  title: Implement feature\n  status: completed\n  next_action: Done\n  acceptance: [Deliver the feature]\n  evidence: [report.json]\n- id: DEP\n  title: Required input\n  status: ready\n  next_action: Continue\n  acceptance: [Source is provided]\n",
    )
    .unwrap();
    let report = f.check();
    for code in ["missing_evidence_bindings", "evidence_level_unverified"] {
        assert!(
            !report.evidence_gaps.iter().any(|g| g.code == code),
            "unexpected {code}: {:?}",
            report.evidence_gaps
        );
    }
    let projected = f
        .store
        .evidence(refresh.project_id, "W/evidence/report.json")
        .unwrap();
    assert_eq!(projected.item.locator, "report.json");
    assert!(f.root.join("report.json").is_file());
    assert_eq!(projected.item.sha256.as_deref(), Some(sha256.as_str()));
    assert_eq!(
        projected.item.source_sha.as_deref(),
        Some(source_sha.as_str())
    );
    assert_eq!(
        projected.item.command.as_deref(),
        Some("cargo test verification")
    );
    assert_eq!(projected.item.verified_at, Some(1_794_000_000_000));
    assert_eq!(projected.item.level, EvidenceLevel::LocallyVerified);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
