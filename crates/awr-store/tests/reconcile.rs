mod support;

use awr_core::{
    CheckpointDraft, Error, EventDraft, Id, ProjectionBatch, SessionDraft, SessionOutcome,
    WorkItem, WorkStatus,
};
use awr_store::{BranchFilter, EventQuery, ReconcileAction, Store};
use rusqlite::{Connection, params};
use support::Fixture;

fn add_work(fixture: &mut Fixture, key: &str) -> Id {
    let item = WorkItem {
        spec_refs: vec![],
        ordinary_completion: None,
        archived: false,
        meta: fixture.meta(key),
        title: key.into(),
        kind: Some("feature".into()),
        owner: None,
        required: true,
        raw_status: "planned".into(),
        status: WorkStatus::Planned,
        priority: Some("P0".into()),
        milestone: Some("M5".into()),
        score: None,
        evidence_level: None,
        summary: String::new(),
        next_action: "Continue".into(),
        blocker: None,
        acceptance: vec!["Works".into()],
        tags: Vec::new(),
        paths: Vec::new(),
    };
    let id = item.meta.id;
    fixture.commit(ProjectionBatch {
        work_items: vec![item],
        ..ProjectionBatch::default()
    });
    id
}

fn start_session(
    fixture: &mut Fixture,
    key: Option<&str>,
    claim: bool,
) -> awr_core::SessionStarted {
    let revision = fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision;
    fixture
        .store
        .start_session(
            fixture.project.id,
            revision,
            SessionDraft {
                work_item_key: key.map(str::to_string),
                agent_id: "doctor-test".into(),
                provider: "local".into(),
                model: "fixture".into(),
                branch_id: None,
                claim,
                claim_ttl_ms: claim.then_some(60_000),
            },
        )
        .unwrap()
        .0
}

fn checkpoint_draft() -> CheckpointDraft {
    CheckpointDraft {
        context_hash: "a".repeat(64),
        digest: "Progress before interruption".into(),
        next_action: "Resume safely".into(),
        open_loops: vec!["One open loop".into()],
        changed_entities: Vec::new(),
    }
}

#[test]
fn inspection_is_read_only_and_reports_runtime_metadata_without_opening_files() {
    let mut fixture = Fixture::new();
    let work_id = add_work(&mut fixture, "W-1");
    let started = start_session(&mut fixture, Some("W-1"), true);
    let revision = fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision;
    let attempt = fixture
        .store
        .begin_checkpoint_save(
            fixture.project.id,
            revision,
            started.session.id,
            checkpoint_draft(),
        )
        .unwrap();

    let missing_checkpoint = Id::new();
    let invalid_branch = Id::new();
    let artifact = Id::new();
    let proposal = Id::new();
    let edge = Id::new();
    let database = fixture.root.join("state.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    connection
        .execute(
            "UPDATE claims SET expires_at=0 WHERE id=?1",
            [started.claim.as_ref().unwrap().id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE work_items SET raw_status='completed',status='completed' WHERE id=?1",
            [work_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE sessions SET last_checkpoint_id=?1 WHERE id=?2",
            params![
                missing_checkpoint.to_string(),
                started.session.id.to_string()
            ],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE sources SET freshness='stale' WHERE id=?1",
            [fixture.source.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision)
             VALUES(?1,?2,'invalid-current',0,'abandoned',1)",
            params![invalid_branch.to_string(), fixture.project.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE projects SET current_branch_id=?1 WHERE id=?2",
            params![invalid_branch.to_string(), fixture.project.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO artifacts(id,project_id,artifact_type,locator,sha256,size,mime,source_event_id,revision)
             VALUES(?1,?2,'report','missing.txt',?3,1,'text/plain',NULL,1)",
            params![artifact.to_string(), fixture.project.id.to_string(), "b".repeat(64)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mutation_proposals(id,project_id,work_item_id,source_id,base_fingerprint,expected_revision,mutation_type,patch_json,status,revision,created_at)
             VALUES(?1,?2,?3,?4,'snapshot-1',?5,'status','{}','ready',1,1)",
            params![
                proposal.to_string(),
                fixture.project.id.to_string(),
                work_id.to_string(),
                fixture.source.id.to_string(),
                revision as i64
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO edges(id,project_id,from_kind,from_key,relation,to_kind,to_key,required,source_id,source_ref_json,source_revision,revision,active)
             VALUES(?1,?2,'work_item','W-1','depends_on','work_item','MISSING',1,?3,'{}',1,1,1)",
            params![
                edge.to_string(),
                fixture.project.id.to_string(),
                fixture.source.id.to_string()
            ],
        )
        .unwrap();
    let before: (i64, i64, i64, String, String) = connection
        .query_row(
            "SELECT p.project_revision,
                    (SELECT count(*) FROM events WHERE project_id=p.id),
                    (SELECT count(*) FROM checkpoints),
                    (SELECT status FROM sessions WHERE id=?2),
                    (SELECT status FROM claims WHERE id=?3)
             FROM projects p WHERE p.id=?1",
            params![
                fixture.project.id.to_string(),
                started.session.id.to_string(),
                started.claim.as_ref().unwrap().id.to_string()
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    drop(connection);

    let report = fixture
        .store
        .inspect_runtime(fixture.project.id, 1)
        .unwrap();
    assert_eq!(report.project_revision, attempt.project_revision);
    assert_eq!(report.active_session_count, 1);
    for code in [
        "source_stale",
        "orphan_session",
        "expired_claim",
        "invalid_claim",
        "incomplete_checkpoint",
        "invalid_checkpoint_pointer",
        "orphan_artifact",
        "invalid_branch",
        "pending_mutation",
        "missing_dependency",
    ] {
        assert!(
            report.findings.iter().any(|finding| finding.code == code),
            "missing finding {code}: {:#?}",
            report.findings
        );
    }
    let incomplete = report
        .findings
        .iter()
        .find(|finding| finding.code == "incomplete_checkpoint")
        .unwrap();
    assert!(incomplete.message.contains("may still be in flight"));
    assert_eq!(
        incomplete.repair,
        Some(ReconcileAction::AbandonCheckpoint {
            attempt_id: attempt.id
        })
    );
    assert!(
        report
            .findings
            .iter()
            .all(|finding| finding.code != "active_session")
    );

    let connection = Connection::open(&database).unwrap();
    let after: (i64, i64, i64, String, String) = connection
        .query_row(
            "SELECT p.project_revision,
                    (SELECT count(*) FROM events WHERE project_id=p.id),
                    (SELECT count(*) FROM checkpoints),
                    (SELECT status FROM sessions WHERE id=?2),
                    (SELECT status FROM claims WHERE id=?3)
             FROM projects p WHERE p.id=?1",
            params![
                fixture.project.id.to_string(),
                started.session.id.to_string(),
                started.claim.as_ref().unwrap().id.to_string()
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        fixture.store.artifacts(fixture.project.id).unwrap().len(),
        1
    );
}

#[test]
fn explicit_repairs_are_scoped_append_only_and_abandoned_attempt_cannot_finish() {
    let mut fixture = Fixture::new();
    let work_id = add_work(&mut fixture, "W-1");
    let started = start_session(&mut fixture, Some("W-1"), true);
    let claim_id = started.claim.as_ref().unwrap().id;
    let connection = Connection::open(fixture.root.join("state.db")).unwrap();
    connection
        .execute(
            "UPDATE claims SET expires_at=0 WHERE id=?1",
            [claim_id.to_string()],
        )
        .unwrap();
    drop(connection);

    let revision = fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision;
    let expired = fixture
        .store
        .reconcile(
            fixture.project.id,
            revision,
            ReconcileAction::ExpireClaim { claim_id },
            "Observed expired lease",
        )
        .unwrap();
    assert_eq!(expired.event.event_type, "claim.expired");
    assert_eq!(expired.event.session_id, Some(started.session.id));
    assert_eq!(expired.event.work_item_id, Some(work_id));
    assert_eq!(expired.project_revision, revision + 1);
    assert_eq!(
        fixture
            .store
            .claim(fixture.project.id, claim_id)
            .unwrap()
            .status,
        "expired"
    );
    let page = fixture
        .store
        .query_events(
            fixture.project.id,
            &EventQuery {
                session_id: Some(started.session.id),
                event_type: Some("claim.expired".into()),
                ..EventQuery::default()
            },
        )
        .unwrap();
    assert_eq!(page.events.len(), 1);

    assert!(matches!(
        fixture.store.reconcile(
            fixture.project.id,
            expired.project_revision,
            ReconcileAction::ExpireClaim { claim_id },
            "Duplicate repair",
        ),
        Err(Error::InvalidTransition(_))
    ));
    assert_eq!(
        fixture
            .store
            .project(fixture.project.id)
            .unwrap()
            .project_revision,
        expired.project_revision
    );

    let revision = expired.project_revision;
    let (claim, _) = fixture
        .store
        .acquire_claim(fixture.project.id, revision, started.session.id, None)
        .unwrap();
    let interrupted = fixture
        .store
        .reconcile(
            fixture.project.id,
            revision + 1,
            ReconcileAction::InterruptSession {
                session_id: started.session.id,
            },
            "Operator confirmed process exit",
        )
        .unwrap();
    assert_eq!(interrupted.event.event_type, "session.interrupted");
    assert_eq!(interrupted.event.session_id, Some(started.session.id));
    assert_eq!(
        fixture
            .store
            .session(fixture.project.id, started.session.id)
            .unwrap()
            .status,
        "interrupted"
    );
    assert_eq!(
        fixture
            .store
            .claim(fixture.project.id, claim.id)
            .unwrap()
            .status,
        "released"
    );
    let page = fixture
        .store
        .query_events(
            fixture.project.id,
            &EventQuery {
                session_id: Some(started.session.id),
                event_type: Some("session.interrupted".into()),
                ..EventQuery::default()
            },
        )
        .unwrap();
    assert_eq!(page.events.len(), 1);

    let checkpoint_session = start_session(&mut fixture, Some("W-1"), false);
    let revision = fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision;
    let attempt = fixture
        .store
        .begin_checkpoint_save(
            fixture.project.id,
            revision,
            checkpoint_session.session.id,
            checkpoint_draft(),
        )
        .unwrap();
    let abandoned = fixture
        .store
        .reconcile(
            fixture.project.id,
            attempt.project_revision,
            ReconcileAction::AbandonCheckpoint {
                attempt_id: attempt.id,
            },
            "Interrupted during durable save",
        )
        .unwrap();
    assert_eq!(abandoned.event.event_type, "checkpoint.abandoned");
    assert_eq!(
        abandoned.event.session_id,
        Some(checkpoint_session.session.id)
    );
    let attempts = fixture
        .store
        .checkpoint_attempts(fixture.project.id, checkpoint_session.session.id, 10)
        .unwrap();
    assert_eq!(attempts.incomplete_count, 0);
    assert_eq!(attempts.attempts[0].status, "abandoned");
    assert!(matches!(
        fixture.store.finish_checkpoint_save(
            fixture.project.id,
            abandoned.project_revision,
            attempt.id
        ),
        Err(Error::InvalidTransition(message)) if message.contains("abandoned")
    ));
    assert_eq!(
        fixture
            .store
            .project(fixture.project.id)
            .unwrap()
            .project_revision,
        abandoned.project_revision
    );

    let branch_id = Id::new();
    let connection = Connection::open(fixture.root.join("state.db")).unwrap();
    connection
        .execute(
            "INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision)
             VALUES(?1,?2,'abandoned-current',0,'abandoned',1)",
            params![branch_id.to_string(), fixture.project.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE projects SET current_branch_id=?1 WHERE id=?2",
            params![branch_id.to_string(), fixture.project.id.to_string()],
        )
        .unwrap();
    drop(connection);
    let cleared = fixture
        .store
        .reconcile(
            fixture.project.id,
            abandoned.project_revision,
            ReconcileAction::ClearInvalidBranch { branch_id },
            "Current branch was already abandoned",
        )
        .unwrap();
    assert_eq!(cleared.event.event_type, "branch.current_cleared");
    assert_eq!(cleared.event.branch_id, Some(branch_id));
    assert_eq!(
        fixture
            .store
            .project(fixture.project.id)
            .unwrap()
            .current_branch_id,
        None
    );
    let connection = Connection::open(fixture.root.join("state.db")).unwrap();
    let branch_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM branches WHERE id=?1",
            [branch_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(branch_count, 1);
    let page = fixture
        .store
        .query_events(
            fixture.project.id,
            &EventQuery {
                branch: BranchFilter::Branch(branch_id),
                event_type: Some("branch.current_cleared".into()),
                ..EventQuery::default()
            },
        )
        .unwrap();
    assert_eq!(page.events.len(), 1);
}

#[test]
fn stale_invalid_and_event_insert_failures_do_not_advance_state() {
    let mut fixture = Fixture::new();
    add_work(&mut fixture, "W-1");
    let started = start_session(&mut fixture, Some("W-1"), true);
    let claim_id = started.claim.as_ref().unwrap().id;
    let database = fixture.root.join("state.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE claims SET expires_at=0 WHERE id=?1",
            [claim_id.to_string()],
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_reconcile_event BEFORE INSERT ON events
             WHEN NEW.event_type='claim.expired'
             BEGIN SELECT RAISE(ABORT,'test receipt failure'); END;",
        )
        .unwrap();
    drop(connection);
    let revision = fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision;

    assert!(matches!(
        fixture.store.reconcile(
            fixture.project.id,
            revision - 1,
            ReconcileAction::ExpireClaim { claim_id },
            "stale caller",
        ),
        Err(Error::RevisionConflict { .. })
    ));
    assert!(matches!(
        fixture.store.reconcile(
            fixture.project.id,
            revision,
            ReconcileAction::ExpireClaim { claim_id },
            "   ",
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        fixture.store.reconcile(
            fixture.project.id,
            revision,
            ReconcileAction::ExpireClaim { claim_id },
            "Exercise transaction rollback",
        ),
        Err(Error::Storage(message)) if message.contains("test receipt failure")
    ));
    assert_eq!(
        fixture
            .store
            .project(fixture.project.id)
            .unwrap()
            .project_revision,
        revision
    );
    let claim = fixture.store.claim(fixture.project.id, claim_id).unwrap();
    assert_eq!(claim.status, "active");
    assert_eq!(claim.revision, 1);

    let connection = Connection::open(&database).unwrap();
    let receipt_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM events WHERE project_id=?1 AND event_type='claim.expired'",
            [fixture.project.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt_count, 0);
}

#[test]
fn historical_claims_and_branch_parents_are_not_judged_by_current_executability() {
    let mut fixture = Fixture::new();
    let work_id = add_work(&mut fixture, "W-1");
    let parent = Id::new();
    let child = Id::new();
    let database = fixture.root.join("state.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO branches(id,project_id,name,fork_project_revision,status,revision)
             VALUES(?1,?2,'parent',0,'active',1)",
            params![parent.to_string(), fixture.project.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO branches(id,project_id,name,parent_branch_id,fork_project_revision,status,revision)
             VALUES(?1,?2,'child',?3,0,'active',1)",
            params![
                child.to_string(),
                fixture.project.id.to_string(),
                parent.to_string()
            ],
        )
        .unwrap();
    drop(connection);
    let revision = fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision;
    let (started, event) = fixture
        .store
        .start_session(
            fixture.project.id,
            revision,
            SessionDraft {
                work_item_key: Some("W-1".into()),
                agent_id: "history".into(),
                provider: "local".into(),
                model: "fixture".into(),
                branch_id: Some(child),
                claim: true,
                claim_ttl_ms: None,
            },
        )
        .unwrap();
    let claim_id = started.claim.as_ref().unwrap().id;
    fixture
        .store
        .end_session(
            fixture.project.id,
            event.project_revision,
            started.session.id,
            SessionOutcome::Ended,
        )
        .unwrap();
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE work_items SET raw_status='completed',status='completed',active=0 WHERE id=?1",
            [work_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE branches SET status='abandoned' WHERE id=?1",
            [parent.to_string()],
        )
        .unwrap();
    drop(connection);

    let report = fixture
        .store
        .inspect_runtime(fixture.project.id, i64::MAX)
        .unwrap();
    assert!(report.findings.iter().all(|finding| {
        !(finding.code == "invalid_claim" && finding.object_id == claim_id.to_string())
    }));
    assert!(report.findings.iter().all(|finding| {
        !(finding.code == "invalid_branch" && finding.object_id == child.to_string())
    }));

    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE projects SET current_branch_id=?1 WHERE id=?2",
            params![child.to_string(), fixture.project.id.to_string()],
        )
        .unwrap();
    drop(connection);
    let revision = fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision;
    assert!(matches!(
        fixture.store.reconcile(
            fixture.project.id,
            revision,
            ReconcileAction::ClearInvalidBranch { branch_id: child },
            "Parent was closed",
        ),
        Err(Error::InvalidTransition(_))
    ));
    assert_eq!(
        fixture
            .store
            .project(fixture.project.id)
            .unwrap()
            .current_branch_id,
        Some(child)
    );
}

#[test]
fn normal_active_session_is_informational_and_does_not_claim_process_liveness() {
    let mut fixture = Fixture::new();
    add_work(&mut fixture, "W-1");
    let started = start_session(&mut fixture, Some("W-1"), false);
    let report = fixture
        .store
        .inspect_runtime(fixture.project.id, 1)
        .unwrap();
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.object_id == started.session.id.to_string())
        .unwrap();
    assert_eq!(finding.code, "active_session");
    assert_eq!(finding.severity, "info");
    assert!(
        finding
            .message
            .contains("process liveness was not inspected")
    );
}

#[test]
fn open_existing_neither_creates_nor_upgrades_databases() {
    let fixture = Fixture::new();
    let missing = fixture.root.join("missing.db");
    assert!(matches!(
        Store::open_existing(&missing),
        Err(Error::NotFound(_))
    ));
    assert!(!missing.exists());

    let database = fixture.root.join("state.db");
    let connection = Connection::open(&database).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);
    assert!(matches!(
        Store::open_existing(&database),
        Err(Error::Storage(_))
    ));
    let connection = Connection::open(&database).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
}

#[test]
fn raw_append_cannot_forge_reconciliation_receipts() {
    let mut fixture = Fixture::new();
    let revision = fixture
        .store
        .project(fixture.project.id)
        .unwrap()
        .project_revision;
    for event_type in [
        "claim.expired",
        "session.interrupted",
        "checkpoint.abandoned",
        "branch.current_cleared",
    ] {
        let result = fixture.store.append_event(
            fixture.project.id,
            revision,
            EventDraft::new(event_type, "forged repair"),
        );
        assert!(matches!(result, Err(Error::InvalidInput(_))));
        assert_eq!(
            fixture
                .store
                .project(fixture.project.id)
                .unwrap()
                .project_revision,
            revision
        );
    }
}
