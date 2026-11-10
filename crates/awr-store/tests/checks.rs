mod support;
use awr_core::*;
use serde_json::json;
use support::Fixture;

fn fixture() -> (Fixture, Id, Revision) {
    let mut f = Fixture::new();
    let work = WorkItem {
        spec_refs: vec![],
        definition: None,
        ordinary_completion: None,
        archived: false,
        meta: f.meta("W"),
        title: "Check a draft".into(),
        kind: None,
        owner: None,
        required: true,
        raw_status: "in_progress".into(),
        status: WorkStatus::InProgress,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: "Retain draft".into(),
        next_action: "Run checks".into(),
        blocker: None,
        acceptance: vec!["The draft is retained".into()],
        tags: vec![],
        paths: vec![],
    };
    f.commit(ProjectionBatch {
        work_items: vec![work],
        ..Default::default()
    });
    let r = f.store.project(f.project.id).unwrap().project_revision;
    let (started, event) = f
        .store
        .start_session(
            f.project.id,
            r,
            SessionDraft {
                work_item_key: Some("W".into()),
                agent_id: "checker".into(),
                provider: "fixture".into(),
                model: "local".into(),
                branch_id: None,
                claim: true,
                claim_ttl_ms: None,
            },
        )
        .unwrap();
    (f, started.session.id, event.project_revision)
}
fn payload() -> CheckPayload {
    serde_json::from_value(json!({"version":1,"example":"draft","criterion_sha256":"a".repeat(64),"phase":"baseline","outcome":"failed","reason":null,"log_dir":".awr/artifacts/checks/W/1-baseline-draft","exit_code":1,"example_oracle":format!("sha256:{}","a".repeat(64)),"oracle_files":[{"path":"test.rs","sha256":"a".repeat(64),"size":10}],"run_sha256":"a".repeat(64),"run_preview":"draft-check","source_sha":"a".repeat(40),"tree_state":"clean","stdout_sha256":"a".repeat(64),"stderr_sha256":"b".repeat(64),"started_at":1,"finished_at":2})).unwrap()
}
#[test]
fn check_records_one_typed_event_and_rejects_a_stale_revision_without_appending() {
    let (mut f, session, r) = fixture();
    let request = RecordCheckRequest {
        work: "W".into(),
        session,
        expected_revision: r,
        payload: payload(),
    };
    f.store
        .require_check(f.project.id, r, "W", session, CheckPhase::Baseline)
        .unwrap();
    let event = f.store.record_check(f.project.id, &request).unwrap();
    assert_eq!(event.project_revision, r + 1);
    assert_eq!(event.session_id, Some(session));
    assert_eq!(
        f.store.check_events(f.project.id, "W", None).unwrap().len(),
        1
    );
    assert!(matches!(
        f.store.record_check(f.project.id, &request),
        Err(Error::RevisionConflict { .. })
    ));
    assert_eq!(
        f.store.check_events(f.project.id, "W", None).unwrap().len(),
        1
    );
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        r + 1
    );
}
#[test]
fn check_cannot_be_forged_by_generic_append_or_a_session_not_bound_to_work() {
    let (mut f, _, r) = fixture();
    let mut draft = EventDraft::new("work.check", "Unverified check claim");
    draft.payload = serde_json::to_value(payload()).unwrap();
    assert!(matches!(
        f.store.append_event(f.project.id, r, draft),
        Err(Error::InvalidInput(_))
    ));
    let (other, event) = f
        .store
        .start_session(
            f.project.id,
            r,
            SessionDraft {
                work_item_key: None,
                agent_id: "observer".into(),
                provider: "fixture".into(),
                model: "local".into(),
                branch_id: None,
                claim: false,
                claim_ttl_ms: None,
            },
        )
        .unwrap();
    assert!(matches!(
        f.store.record_check(
            f.project.id,
            &RecordCheckRequest {
                work: "W".into(),
                session: other.session.id,
                expected_revision: event.project_revision,
                payload: payload()
            }
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(
        f.store
            .check_events(f.project.id, "W", None)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn check_event_contract_rejects_long_previews_missing_fields_and_oversized_payloads() {
    let valid = serde_json::to_value(payload()).unwrap();
    assert!(is_domain_event_type(CHECK_EVENT));
    assert!(checked_event_payload(CHECK_EVENT, "normal", "Recorded check", &valid).is_ok());
    let mut invalid = valid.clone();
    invalid["run_preview"] = json!("x".repeat(257));
    assert!(checked_event_payload(CHECK_EVENT, "normal", "Recorded check", &invalid).is_err());
    let mut invalid = valid.clone();
    invalid.as_object_mut().unwrap().remove("criterion_sha256");
    assert!(checked_event_payload(CHECK_EVENT, "normal", "Recorded check", &invalid).is_err());
    let mut invalid = valid;
    invalid["reason"] = json!("x".repeat(CHECK_PAYLOAD_CAP));
    assert!(checked_event_payload(CHECK_EVENT, "normal", "Recorded check", &invalid).is_err());
}
