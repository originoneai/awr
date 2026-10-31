//! DEC-041 at the store: what is read for the history is the project's own, as the program stored it.
#[allow(dead_code)]
mod support;
use awr_core::workstream_usage::*;
use awr_core::*;
use awr_store::SourceRegistration;
use serde_json::json;
use support::Fixture;

fn work_in(source: &Source, project_key: &str, key: &str) -> WorkItem {
    WorkItem {
        spec_refs: vec![],
        definition: None,
        ordinary_completion: None,
        archived: false,
        meta: ProjectionMeta {
            id: Id::new(),
            external_key: key.into(),
            revision: 1,
            source_ref: SourceRef {
                source_id: source.id,
                locator: source.locator.clone(),
                source_revision: source.revision + 1,
                source_fingerprint: "snapshot-1".into(),
                pointer: Some(format!("/{project_key}/{key}")),
                start_line: None,
                end_line: None,
                section_fingerprint: None,
            },
        },
        title: format!("Work {key}"),
        kind: None,
        owner: None,
        required: true,
        raw_status: "in_progress".into(),
        status: WorkStatus::InProgress,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: "Current source facts".into(),
        next_action: "Go on".into(),
        blocker: None,
        acceptance: vec!["Review the report".into()],
        tags: vec![],
        paths: vec![],
    }
}

fn fixture(keys: &[&str]) -> Fixture {
    let mut f = Fixture::new();
    let items = keys
        .iter()
        .map(|key| work_in(&f.source, "a", key))
        .collect();
    f.commit(ProjectionBatch {
        work_items: items,
        ..Default::default()
    });
    f
}

fn event(
    f: &Fixture,
    project: Id,
    work: Id,
    kind: &str,
    payload: serde_json::Value,
    revision: u64,
    at: i64,
) {
    rusqlite::Connection::open(f.root.join("state.db"))
        .unwrap()
        .execute(
            "INSERT INTO events(id,project_id,work_item_id,event_type,importance,summary,payload_json,project_revision,created_at)
             VALUES(?1,?2,?3,?4,'normal','synthetic',?5,?6,?7)",
            rusqlite::params![
                Id::new().to_string(),
                project.to_string(),
                work.to_string(),
                kind,
                payload.to_string(),
                revision as i64,
                at
            ],
        )
        .unwrap();
}

fn id_of(f: &Fixture, key: &str) -> Id {
    f.store.work_item(f.project.id, key).unwrap().item.meta.id
}

fn ask(f: &Fixture, project: Id) -> OutcomeHistory {
    let input = f.store.outcome_history_input(project, None).unwrap();
    aggregate_outcome_history(
        &input,
        &HistoryRequest {
            as_of_ms: now_millis().unwrap() + 1_000,
            cohort_by: CohortBy::None,
            work_keys: None,
        },
    )
    .unwrap()
}

fn money(micros: u64) -> UsageMoney {
    UsageMoney {
        currency: "USD".into(),
        micros,
    }
}

fn receipt(project: Id, id: &str, work: &str, cost: UsageCost) -> UsageReceipt {
    UsageReceipt {
        receipt_id: id.into(),
        project_id: project.to_string(),
        provider_namespace: "account".into(),
        provider: "provider".into(),
        call_id: format!("call-{id}"),
        model: "model".into(),
        session_id: "session".into(),
        occurred_at_ms: 10,
        channel: UsageChannel::Model,
        attribution: UsageAttribution {
            work_id: work.into(),
            execution_id: format!("exec-{id}"),
            workstream_id: None,
        },
        tokens: None,
        cost,
    }
}

#[test]
fn usage_is_read_with_its_corrections_applied_for_a_work_named_by_key_or_by_id() {
    let mut f = fixture(&["W", "V"]);
    let project = f.project.id;
    let v = id_of(&f, "V").to_string();
    f.store
        .ingest_usage_receipt(
            project,
            "i1",
            &receipt(project, "r1", "W", UsageCost::Actual(money(100))),
        )
        .unwrap();
    f.store
        .ingest_usage_receipt(
            project,
            "i2",
            &receipt(project, "r2", &v, UsageCost::Unknown),
        )
        .unwrap();
    f.store
        .record_usage_correction(
            project,
            "c1",
            &UsageCorrection {
                correction_id: "corr-1".into(),
                project_id: project.to_string(),
                target_receipt_id: "r1".into(),
                corrected_at_ms: 20,
                reason: "Restated by the provider".into(),
                prior_cost: UsageCost::Actual(money(100)),
                new_cost: UsageCost::Actual(money(150)),
                actor: "fixture".into(),
            },
        )
        .unwrap();
    let report = ask(&f, project);
    let cost = &report.cohorts[0].cost;
    assert_eq!(cost.actual_micros["USD"], 150, "the correction is applied");
    assert_eq!((cost.unknown_cost_calls, cost.unique_calls), (1, 2));
    assert_eq!((cost.works_with_usage, cost.works_without_usage), (2, 0));
    assert_eq!(report.coverage.works_with_usage, (2, 2));
}

#[test]
fn a_receipt_the_program_cannot_read_is_counted_with_what_it_does_not_say() {
    let f = fixture(&["W", "V"]);
    let project = f.project.id;
    // A completion whose payload is not the shape of one, and a cancellation that names no version.
    event(
        &f,
        project,
        id_of(&f, "W"),
        "work.completed",
        json!({"work_action": 5, "target_revision": 7}),
        100,
        1_000,
    );
    event(
        &f,
        project,
        id_of(&f, "V"),
        "work.cancelled",
        json!({}),
        101,
        1_100,
    );
    let report = ask(&f, project);
    let signals = &report.cohorts[0].signals;
    assert_eq!(signals.completion_by_evidence_level["not_recorded"], 1);
    assert_eq!(
        (
            signals.completion_reported,
            signals.completion_standard_unknown
        ),
        (0, 1)
    );
    assert_eq!(signals.cancelled, 1);
    assert_eq!(
        (
            report.sample.receipts.used,
            report.sample.receipts.without_version
        ),
        (2, 1)
    );
}

#[test]
fn another_project_in_the_same_database_is_never_read() {
    let mut f = fixture(&["W"]);
    let a = f.project.id;
    // Project B has a work, a session with an execution, a receipt and a usage receipt of its own.
    let root = f.root.join("other");
    std::fs::create_dir(&root).unwrap();
    let b = f.store.register_project(&root, "other", "Other").unwrap();
    let source = f
        .store
        .register_source(
            b.id,
            &SourceRegistration {
                domain: "ledger",
                role: "primary",
                locator: "file:///other/ledger.yaml",
                format: "yaml",
                adapter: "yaml-ledger-v1",
            },
        )
        .unwrap();
    let work_b = work_in(&source, "b", "W");
    f.store
        .commit_source_projection(
            &source,
            "snapshot-1",
            ProjectionBatch {
                work_items: vec![work_b.clone()],
                ..Default::default()
            },
        )
        .unwrap();
    let revision = f.store.project(b.id).unwrap().project_revision;
    let (started, _) = f
        .store
        .start_session(
            b.id,
            revision,
            SessionDraft {
                work_item_key: Some("W".into()),
                agent_id: "other-agent".into(),
                provider: "other-provider".into(),
                model: "other-model".into(),
                branch_id: None,
                claim: false,
                claim_ttl_ms: None,
            },
        )
        .unwrap();
    event(
        &f,
        b.id,
        work_b.meta.id,
        "work.completed",
        json!({"target_revision": 1}),
        50,
        1_000,
    );
    f.store
        .ingest_usage_receipt(
            b.id,
            "i1",
            &receipt(b.id, "rb", "W", UsageCost::Actual(money(999))),
        )
        .unwrap();

    // A sees none of it, though B's work has the same key; B sees its own.
    let report = ask(&f, a);
    assert_eq!(
        f.store
            .outcome_history_input(a, None)
            .unwrap()
            .sessions
            .len(),
        0
    );
    assert_eq!(report.sample.receipts.read, 0);
    assert_eq!(report.sample.usage.read, 0);
    assert!(report.cohorts[0].cost.actual_micros.is_empty());
    assert_eq!(report.cohorts[0].signals.open, 1);
    let own = ask(&f, b.id);
    assert_eq!(own.sample.receipts.used, 1);
    assert_eq!(own.cohorts[0].cost.actual_micros["USD"], 999);
    assert_eq!(
        f.store.outcome_history_input(b.id, None).unwrap().sessions[0].session_id,
        started.session.id
    );
}
