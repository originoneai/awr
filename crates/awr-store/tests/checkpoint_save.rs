#[allow(dead_code)]
mod support;
use awr_core::*;
use awr_store::Store;
use support::Fixture;

fn draft(next: &str) -> CheckpointDraft {
    CheckpointDraft {
        context_hash: "b".repeat(64),
        digest: "Saved progress".into(),
        next_action: next.into(),
        open_loops: vec!["Keep the unfinished work".into()],
        changed_entities: vec![],
    }
}
#[test]
fn completion_receipt_failure_rolls_back_checkpoint_and_pointer_but_keeps_attempt() {
    let mut f = Fixture::new();
    let revision = f.store.project(f.project.id).unwrap().project_revision;
    let (session, event) = f
        .store
        .start_session(
            f.project.id,
            revision,
            SessionDraft {
                work_item_key: None,
                agent_id: "executor".into(),
                provider: "fixture".into(),
                model: "test".into(),
                branch_id: None,
                claim: false,
                claim_ttl_ms: None,
            },
        )
        .unwrap();
    let sid = session.session.id;
    let (first, event) = f
        .store
        .create_checkpoint(
            f.project.id,
            event.project_revision,
            sid,
            draft("Last known complete checkpoint"),
        )
        .unwrap();
    assert!(
        f.store
            .checkpoint_saved_delta(f.project.id, first.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.store
            .checkpoint_save_metadata(f.project.id, first.id)
            .unwrap()["delta_recorded"],
        false
    );
    let started = f
        .store
        .begin_checkpoint_save(
            f.project.id,
            event.project_revision,
            sid,
            draft("New saved checkpoint"),
        )
        .unwrap();
    let fixture_sql = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    fixture_sql.execute_batch("CREATE TRIGGER fail_checkpoint_receipt BEFORE INSERT ON events WHEN NEW.event_type='checkpoint.created' BEGIN SELECT RAISE(ABORT,'simulated completion receipt failure'); END;").unwrap();
    assert!(matches!(
        f.store
            .finish_checkpoint_save(f.project.id, started.project_revision, started.id),
        Err(Error::Storage(_))
    ));
    let reopened = Store::open_readonly(&f.root.join("state.db")).unwrap();
    assert_eq!(
        reopened.project(f.project.id).unwrap().project_revision,
        started.project_revision
    );
    assert_eq!(
        reopened
            .latest_checkpoint(f.project.id, sid)
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    assert_eq!(
        fixture_sql
            .query_row("SELECT count(*) FROM checkpoints", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let attempts = reopened.checkpoint_attempts(f.project.id, sid, 20).unwrap();
    assert_eq!(attempts.incomplete_count, 1);
    assert_eq!(attempts.attempts[0].status, "pending_or_interrupted");
    drop(reopened);
    fixture_sql
        .execute_batch("DROP TRIGGER fail_checkpoint_receipt;")
        .unwrap();
    let (saved, complete) = f
        .store
        .finish_checkpoint_save(f.project.id, started.project_revision, started.id)
        .unwrap();
    assert_eq!(saved.next_action, "New saved checkpoint");
    assert_eq!(saved.project_revision, started.project_revision - 1);
    let delta = f
        .store
        .checkpoint_saved_delta(f.project.id, saved.id)
        .unwrap()
        .unwrap();
    assert_eq!(delta.baseline_checkpoint_id, Some(first.id));
    assert_eq!(delta.through_revision, saved.project_revision);
    let attempts = f.store.checkpoint_attempts(f.project.id, sid, 20).unwrap();
    assert_eq!(attempts.incomplete_count, 0);
    assert_eq!(attempts.attempts[0].checkpoint_id, Some(saved.id));
    assert!(matches!(
        f.store
            .finish_checkpoint_save(f.project.id, complete.project_revision, started.id),
        Err(Error::RevisionConflict { .. })
    ));
    assert_eq!(
        fixture_sql
            .query_row("SELECT count(*) FROM checkpoints", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(f.store.doctor().unwrap().ok);
}

#[test]
fn invalid_drafts_and_forged_attempt_receipts_do_not_change_state() {
    let mut f = Fixture::new();
    let before = f.store.project(f.project.id).unwrap().project_revision;
    let mut invalid = draft("Next");
    invalid.context_hash = "invalid".into();
    assert!(matches!(
        f.store
            .begin_checkpoint_save(f.project.id, before, Id::new(), invalid),
        Err(Error::InvalidInput(_))
    ));
    for event_type in [
        "checkpoint.started",
        "checkpoint.created",
        "checkpoint.failed",
    ] {
        let forged = EventDraft::new(event_type, "Untrusted caller receipt");
        assert!(matches!(
            f.store.append_event(f.project.id, before, forged),
            Err(Error::InvalidInput(_))
        ));
    }
    assert_eq!(
        f.store.project(f.project.id).unwrap().project_revision,
        before
    );
}

#[test]
fn invalid_checkpoint_fields_are_identified_without_creating_an_attempt() {
    let mut f = Fixture::new();
    let before = f.store.project(f.project.id).unwrap().project_revision;
    for (field, value, expected) in [
        (
            "context_hash",
            format!("sha256:{}", "a".repeat(64)),
            "without a sha256: prefix",
        ),
        ("context_hash", "a".repeat(63), "64 hexadecimal characters"),
        (
            "context_hash",
            "not-a-context-digest".into(),
            "64 hexadecimal characters",
        ),
        (
            "digest",
            " \t".into(),
            "digest (--digest) must not be blank",
        ),
        (
            "next_action",
            "\n".into(),
            "next_action (--next-action) must not be blank",
        ),
        (
            "open_loops",
            " ".into(),
            "open_loops (--open-loop) entry 2 must not be blank",
        ),
        (
            "changed_entities",
            "\t".into(),
            "changed_entities (--changed-entity) entry 2 must not be blank",
        ),
    ] {
        let mut input = draft("Next action");
        match field {
            "context_hash" => input.context_hash = value,
            "digest" => input.digest = value,
            "next_action" => input.next_action = value,
            "open_loops" => input.open_loops.push(value),
            "changed_entities" => input.changed_entities = vec!["work/W".into(), value],
            _ => unreachable!(),
        }
        for error in [
            f.store
                .create_checkpoint(f.project.id, before, Id::new(), input.clone())
                .unwrap_err(),
            f.store
                .begin_checkpoint_save(f.project.id, before, Id::new(), input)
                .unwrap_err(),
        ] {
            let Error::InvalidInput(message) = error else {
                panic!("Unexpected error: {error}")
            };
            assert!(message.contains(expected), "{field}: {message}");
            assert!(!message.contains("not-a-context-digest"));
        }
        assert_eq!(
            f.store.project(f.project.id).unwrap().project_revision,
            before
        );
    }
    let conn = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM checkpoints", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM events WHERE event_type LIKE 'checkpoint.%'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
