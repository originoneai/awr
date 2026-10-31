//! Recorded work extends the live claims of its session when the project's policy asks for it, in the
//! revision and the receipt of the work itself; a claim that ran out is never revived.
#[allow(dead_code)]
mod support;
use awr_core::*;
use serde_json::json;
use support::Fixture;

const HOUR: u64 = 3_600_000;

fn work(f: &Fixture, key: &str) -> WorkItem {
    WorkItem {
        spec_refs: vec![],
        definition: None,
        ordinary_completion: None,
        archived: false,
        meta: f.meta(key),
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
        acceptance: vec![],
        tags: vec![],
        paths: vec![],
    }
}

fn fixture(keys: &[&str]) -> Fixture {
    let mut f = Fixture::new();
    let items = keys.iter().map(|key| work(&f, key)).collect();
    f.commit(ProjectionBatch {
        work_items: items,
        ..Default::default()
    });
    f
}

fn revision(f: &Fixture) -> Revision {
    f.store.project(f.project.id).unwrap().project_revision
}

/// A session on `key` holding a claim of `ttl` milliseconds (none: no expiry).
fn held(f: &mut Fixture, key: &str, agent: &str, ttl: Option<u64>) -> (Session, Claim) {
    let expected = revision(f);
    let (started, _) = f
        .store
        .start_session(
            f.project.id,
            expected,
            SessionDraft {
                work_item_key: Some(key.into()),
                agent_id: agent.into(),
                provider: "fixture".into(),
                model: "test".into(),
                branch_id: None,
                claim: true,
                claim_ttl_ms: ttl,
            },
        )
        .unwrap();
    (started.session, started.claim.unwrap())
}

/// Saves a checkpoint of `session`; returns the event and the revision the save was started at.
fn checkpoint(f: &mut Fixture, session: Id, renewal: Option<ClaimRenewal>) -> (Event, Revision) {
    let expected = revision(f);
    let started = f
        .store
        .begin_checkpoint_save(
            f.project.id,
            expected,
            session,
            CheckpointDraft {
                context_hash: "b".repeat(64),
                digest: "Saved progress".into(),
                next_action: "Go on".into(),
                open_loops: vec![],
                changed_entities: vec![],
            },
        )
        .unwrap();
    let (_, event) = f
        .store
        .finish_checkpoint_save_renewing(
            f.project.id,
            started.project_revision,
            started.id,
            renewal,
        )
        .unwrap();
    (event, started.project_revision)
}

fn claims(f: &Fixture, session: Id) -> Vec<Claim> {
    f.store.session_claims(f.project.id, session).unwrap()
}

fn sql(f: &Fixture, statement: &str, params: impl rusqlite::Params) {
    rusqlite::Connection::open(f.root.join("state.db"))
        .unwrap()
        .execute(statement, params)
        .unwrap();
}

#[test]
fn a_checkpoint_extends_the_live_claim_of_its_session_in_its_own_revision_and_says_so() {
    let mut f = fixture(&["W"]);
    let (session, claim) = held(&mut f, "W", "executor", Some(600_000));
    let before = claim.expires_at.unwrap();
    let (event, started) = checkpoint(&mut f, session.id, Some(ClaimRenewal { ttl_ms: HOUR }));
    let after = claims(&f, session.id)[0].expires_at.unwrap();
    let ceiling = now_millis().unwrap() + HOUR as i64;
    assert!(after > before && after <= ceiling, "{before} {after}");
    assert!(after >= before + HOUR as i64 - 600_000);
    // One write, one revision, one receipt: a caller chaining on the revision it was given is not surprised.
    assert_eq!(event.event_type, "checkpoint.created");
    assert_eq!(event.project_revision, started + 1);
    assert_eq!(revision(&f), started + 1);
    let receipt = &event.payload["claim_renewal"];
    assert_eq!(receipt["ttl_ms"], HOUR);
    assert_eq!(receipt["renewed"].as_array().unwrap().len(), 1);
    assert_eq!(receipt["renewed"][0]["claim_id"], json!(claim.id));
    assert_eq!(
        receipt["renewed"][0]["work_item_id"],
        json!(claim.work_item_id)
    );
    assert_eq!(receipt["renewed"][0]["previous_expires_at"], before);
    assert_eq!(receipt["renewed"][0]["expires_at"], after);
    assert_eq!(receipt["expired_claim_ids"], json!([]));
    let held = &claims(&f, session.id)[0];
    assert_eq!(held.status, "active");
    assert_eq!(held.revision, claim.revision + 1);
    assert_eq!(held.acquired_at, claim.acquired_at);
}

#[test]
fn nothing_is_renewed_unless_asked_and_nothing_is_said_when_nothing_changes() {
    let mut f = fixture(&["W"]);
    let (session, claim) = held(&mut f, "W", "executor", Some(600_000));
    // Not asked: the expiry stays and the receipt is the one it always was.
    let (event, _) = checkpoint(&mut f, session.id, None);
    assert_eq!(claims(&f, session.id)[0].expires_at, claim.expires_at);
    assert!(event.payload.get("claim_renewal").is_none());
    // Asked for less than the claim already has: it is never shortened, and there is nothing to report.
    let (event, _) = checkpoint(&mut f, session.id, Some(ClaimRenewal { ttl_ms: 60_000 }));
    assert_eq!(claims(&f, session.id)[0].expires_at, claim.expires_at);
    assert_eq!(claims(&f, session.id)[0].revision, claim.revision);
    assert!(event.payload.get("claim_renewal").is_none());
}

#[test]
fn a_claim_without_an_expiry_or_already_released_is_left_as_it_is() {
    let mut f = fixture(&["W", "V"]);
    let (open, unlimited) = held(&mut f, "W", "executor-a", None);
    let (event, _) = checkpoint(&mut f, open.id, Some(ClaimRenewal { ttl_ms: HOUR }));
    assert_eq!(claims(&f, open.id)[0].expires_at, None);
    assert_eq!(claims(&f, open.id)[0].revision, unlimited.revision);
    assert!(event.payload.get("claim_renewal").is_none());

    let (other, claim) = held(&mut f, "V", "executor-b", Some(600_000));
    let expected = revision(&f);
    f.store
        .release_claim(f.project.id, expected, other.id, claim.id)
        .unwrap();
    let (event, _) = checkpoint(&mut f, other.id, Some(ClaimRenewal { ttl_ms: HOUR }));
    let released = &claims(&f, other.id)[0];
    assert_eq!(released.status, "released");
    assert_eq!(released.expires_at, claim.expires_at);
    assert!(event.payload.get("claim_renewal").is_none());
}

#[test]
fn a_claim_that_ran_out_is_not_revived_and_the_receipt_says_it_ran_out() {
    let mut f = fixture(&["W"]);
    let (session, claim) = held(&mut f, "W", "executor", Some(600_000));
    let ended = now_millis().unwrap() - 1;
    sql(
        &f,
        "UPDATE claims SET expires_at=?1 WHERE id=?2",
        rusqlite::params![ended, claim.id.to_string()],
    );
    let (event, _) = checkpoint(&mut f, session.id, Some(ClaimRenewal { ttl_ms: HOUR }));
    let after = &claims(&f, session.id)[0];
    assert_eq!(after.status, "expired");
    assert_eq!(after.expires_at, Some(ended));
    assert!(!after.active_at(now_millis().unwrap()));
    let receipt = &event.payload["claim_renewal"];
    assert_eq!(receipt["renewed"], json!([]));
    assert_eq!(receipt["expired_claim_ids"], json!([claim.id]));
    // The work was recorded all the same, and the session goes on.
    assert_eq!(event.event_type, "checkpoint.created");
    assert_eq!(
        f.store.session(f.project.id, session.id).unwrap().status,
        "active"
    );
    // A later checkpoint finds nothing left to report.
    let (event, _) = checkpoint(&mut f, session.id, Some(ClaimRenewal { ttl_ms: HOUR }));
    assert!(event.payload.get("claim_renewal").is_none());
}

#[test]
fn only_the_claims_of_the_session_that_did_the_work_are_renewed() {
    let mut f = fixture(&["W", "V"]);
    let (first, first_claim) = held(&mut f, "W", "executor-a", Some(600_000));
    let (second, second_claim) = held(&mut f, "V", "executor-b", Some(600_000));
    checkpoint(&mut f, first.id, Some(ClaimRenewal { ttl_ms: HOUR }));
    assert!(claims(&f, first.id)[0].expires_at > first_claim.expires_at);
    assert_eq!(claims(&f, second.id)[0].expires_at, second_claim.expires_at);
    assert_eq!(claims(&f, second.id)[0].revision, second_claim.revision);
}

#[test]
fn a_claim_taken_over_after_it_ran_out_is_not_extended_by_the_old_session() {
    let mut f = fixture(&["W"]);
    let (old, claim) = held(&mut f, "W", "executor-a", Some(600_000));
    sql(
        &f,
        "UPDATE claims SET expires_at=?1 WHERE id=?2",
        rusqlite::params![now_millis().unwrap() - 1, claim.id.to_string()],
    );
    // Another session takes the work over; that marks the old claim expired.
    let (new, taken) = held(&mut f, "W", "executor-b", Some(600_000));
    assert_eq!(claims(&f, old.id)[0].status, "expired");
    let (event, _) = checkpoint(&mut f, old.id, Some(ClaimRenewal { ttl_ms: HOUR }));
    assert!(event.payload.get("claim_renewal").is_none());
    assert_eq!(claims(&f, old.id)[0].status, "expired");
    assert_eq!(claims(&f, new.id)[0].expires_at, taken.expires_at);
    assert_eq!(claims(&f, new.id)[0].status, "active");
}

#[test]
fn a_save_that_cannot_complete_renews_nothing() {
    let mut f = fixture(&["W"]);
    let (session, claim) = held(&mut f, "W", "executor", Some(600_000));
    let expected = revision(&f);
    let started = f
        .store
        .begin_checkpoint_save(
            f.project.id,
            expected,
            session.id,
            CheckpointDraft {
                context_hash: "b".repeat(64),
                digest: "Saved progress".into(),
                next_action: "Go on".into(),
                open_loops: vec![],
                changed_entities: vec![],
            },
        )
        .unwrap();
    // Anything written in between invalidates the attempt, and the claim must not have moved with it.
    let expected = revision(&f);
    f.store
        .append_event(
            f.project.id,
            expected,
            EventDraft::new("review.observed", "Something else was written"),
        )
        .unwrap();
    assert!(
        f.store
            .finish_checkpoint_save_renewing(
                f.project.id,
                started.project_revision,
                started.id,
                Some(ClaimRenewal { ttl_ms: HOUR }),
            )
            .is_err()
    );
    assert_eq!(claims(&f, session.id)[0].expires_at, claim.expires_at);
    assert_eq!(claims(&f, session.id)[0].revision, claim.revision);
}
