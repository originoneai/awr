//! `check_completion_binding` validates the exact binding the store recorded.
//!
//! The runtime must pass that binding through unchanged: dropping or narrowing a
//! binding because one of its records is not current would skip these checks, so
//! the four outcomes below are pinned at the store boundary.
mod support;
use awr_core::*;
use support::Fixture;

const SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "The report contains the requested analysis";

fn work(f: &Fixture) -> WorkItem {
    WorkItem {
        ordinary_completion: None,
        archived: false,
        meta: f.meta("W"),
        title: "W".into(),
        kind: None,
        owner: None,
        required: true,
        raw_status: "planned".into(),
        status: WorkStatus::Planned,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: Some(EvidenceLevel::Designed),
        summary: "".into(),
        next_action: "build".into(),
        blocker: None,
        acceptance: vec![C.into()],
        tags: vec![],
        paths: vec![],
    }
}

fn draft(key: &str) -> EvidenceDraft {
    EvidenceDraft {
        external_key: key.into(),
        work_item_key: Some("W".into()),
        evidence_type: "completion_report".into(),
        level: EvidenceLevel::LocallyVerified,
        summary: "Checked report".into(),
        locator: "reports/check.json".into(),
        sha256: Some("a".repeat(64)),
        source_sha: Some(SHA.into()),
        command: Some("verify report and delivery".into()),
        scope: vec!["W".into()],
        branch_id: None,
        verified_at: Some(100),
    }
}

fn record(f: &mut Fixture, key: &str) -> Evidence {
    let revision = f.store.project(f.project.id).unwrap().project_revision;
    f.store
        .record_evidence(f.project.id, revision, draft(key))
        .unwrap()
        .0
}

fn binding(evidence: Vec<Evidence>, criteria: &[&str]) -> CompletionBinding {
    let ids = evidence.iter().map(|e| e.id).collect::<Vec<_>>();
    CompletionBinding {
        version: 1,
        source_sha: SHA.into(),
        minimum_level: EvidenceLevel::LocallyVerified,
        acceptance: criteria
            .iter()
            .map(|criterion| AcceptanceEvidenceBinding {
                criterion: (*criterion).into(),
                evidence: ids.clone(),
            })
            .collect(),
        evidence,
    }
}

fn fixture_with_work() -> (Fixture, WorkItem) {
    let mut f = Fixture::new();
    let item = work(&f);
    f.commit(ProjectionBatch {
        work_items: vec![item.clone()],
        ..Default::default()
    });
    (f, item)
}

fn missing_message(error: Error) -> String {
    match error {
        Error::EvidenceMissing(message) => message,
        other => panic!("expected EvidenceMissing, got {other:?}"),
    }
}

#[test]
fn a_mainline_record_is_not_current_for_a_branch_request() {
    let (mut f, item) = fixture_with_work();
    let evidence = record(&mut f, "E");
    let binding = binding(vec![evidence], &[C]);
    let message = missing_message(
        f.store
            .check_completion_binding(f.project.id, &item, Some(Id::new()), &binding)
            .unwrap_err(),
    );
    println!("fixture 1 EvidenceMissing: {message}");
    assert!(message.contains("not current"), "{message}");
}

#[test]
fn bound_criteria_must_match_the_source_acceptance() {
    let (mut f, item) = fixture_with_work();
    let evidence = record(&mut f, "E");
    let binding = binding(vec![evidence], &["A criterion the source does not declare"]);
    let message = missing_message(
        f.store
            .check_completion_binding(f.project.id, &item, None, &binding)
            .unwrap_err(),
    );
    println!("fixture 2 EvidenceMissing: {message}");
    assert!(message.contains("bound acceptance"), "{message}");
}

#[test]
fn a_record_changed_after_binding_is_rejected() {
    let (mut f, item) = fixture_with_work();
    let mut evidence = record(&mut f, "E");
    evidence.summary = "Checked report, edited after the binding".into();
    let binding = binding(vec![evidence], &[C]);
    let message = missing_message(
        f.store
            .check_completion_binding(f.project.id, &item, None, &binding)
            .unwrap_err(),
    );
    println!("fixture 3 EvidenceMissing: {message}");
    assert!(message.contains("changed after binding"), "{message}");
}

#[test]
fn a_fully_current_binding_is_accepted() {
    let (mut f, item) = fixture_with_work();
    let evidence = record(&mut f, "E");
    let binding = binding(vec![evidence], &[C]);
    // Guards against over-rejection: the four refusals above are only meaningful if a
    // binding whose records are current on the requested branch still passes.
    f.store
        .check_completion_binding(f.project.id, &item, None, &binding)
        .expect("a fully current binding must be accepted");
}

#[test]
fn a_report_reference_collision_is_a_source_conflict() {
    let (mut f, item) = fixture_with_work();
    let evidence = record(&mut f, "E");
    let collision = format!("{}/evidence/{}", item.meta.external_key, evidence.locator);
    record(&mut f, &collision);
    let binding = binding(vec![evidence], &[C]);
    let error = f
        .store
        .check_completion_binding(f.project.id, &item, None, &binding)
        .unwrap_err();
    println!("fixture 4 SourceConflict: {error:?}");
    assert!(matches!(error, Error::SourceConflict(_)), "{error:?}");
}
