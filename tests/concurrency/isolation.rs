#[path = "../../crates/awr-store/tests/support/mod.rs"]
mod support;

use awr_core::*;
use awr_store::{BranchFilter, EventQuery, ReconcileAction, Store};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use support::Fixture;

fn fixture() -> Fixture {
    let mut f = Fixture::new();
    fs::write(f.root.join("fixture-owner"), "awr-isolation-test").unwrap();
    let work = |key: &str| WorkItem {
        spec_refs: vec![],
        ordinary_completion: None,
        archived: false,
        meta: f.meta(key),
        title: format!("Review {key} report"),
        kind: None,
        owner: Some("source-team".into()),
        required: true,
        raw_status: "ready".into(),
        status: WorkStatus::Ready,
        priority: None,
        milestone: None,
        score: None,
        evidence_level: None,
        summary: "Source report facts".into(),
        next_action: "Review the draft".into(),
        blocker: None,
        acceptance: vec!["Deliver the reviewed report".into()],
        tags: vec![],
        paths: vec![],
    };
    let batch = ProjectionBatch {
        work_items: vec![work("W"), work("X"), work("Y")],
        ..Default::default()
    };
    f.commit(batch);
    f
}
fn revision(f: &Fixture) -> Revision {
    f.store.project(f.project.id).unwrap().project_revision
}
fn draft(agent: &str, branch: Option<Id>, claim: bool, ttl: Option<u64>) -> SessionDraft {
    SessionDraft {
        work_item_key: Some("W".into()),
        agent_id: agent.into(),
        provider: "fixture".into(),
        model: "metadata-test".into(),
        branch_id: branch,
        claim,
        claim_ttl_ms: ttl,
    }
}
fn start(
    f: &mut Fixture,
    agent: &str,
    branch: Option<Id>,
    claim: bool,
    ttl: Option<u64>,
) -> SessionStarted {
    f.store
        .start_session(f.project.id, revision(f), draft(agent, branch, claim, ttl))
        .unwrap()
        .0
}
fn start_work(
    f: &mut Fixture,
    key: &str,
    agent: &str,
    branch: Option<Id>,
    ttl: Option<u64>,
) -> SessionStarted {
    let mut request = draft(agent, branch, true, ttl);
    request.work_item_key = Some(key.into());
    f.store
        .start_session(f.project.id, revision(f), request)
        .unwrap()
        .0
}
fn branch(f: &mut Fixture, name: &str) -> Id {
    f.store
        .create_branch(
            f.project.id,
            revision(f),
            BranchDraft {
                name: name.into(),
                parent_branch_id: None,
                git_binding: None,
                actor: "reviewer".into(),
                reason: "Explore an independent report review".into(),
            },
        )
        .unwrap()
        .0
        .id
}
fn checkpoint(f: &mut Fixture, session: Id, digest: &str) -> Checkpoint {
    f.store
        .create_checkpoint(
            f.project.id,
            revision(f),
            session,
            CheckpointDraft {
                context_hash: "a".repeat(64),
                digest: digest.into(),
                next_action: "Finish this report review".into(),
                open_loops: vec![],
                changed_entities: vec![],
            },
        )
        .unwrap()
        .0
}
fn end(f: &mut Fixture, session: Id) {
    f.store
        .end_session(f.project.id, revision(f), session, SessionOutcome::Ended)
        .unwrap();
}
fn state(f: &Fixture) -> Value {
    let sessions = f.store.sessions(f.project.id, false, 1000).unwrap();
    let claims: Vec<_> = sessions
        .iter()
        .flat_map(|s| f.store.session_claims(f.project.id, s.id).unwrap())
        .collect();
    json!({"revision":revision(f), "sessions":sessions, "claims":claims,
        "events":f.store.events_since(f.project.id,0,1000).unwrap(),
        "work":f.store.work_item(f.project.id,"W").unwrap(),
        "branches":f.store.branches(f.project.id,None,0,1000).unwrap()})
}
fn wait_expired(claim: &Claim) {
    let deadline = claim.expires_at.unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    while now_millis().unwrap() <= deadline {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn passed(id: &str) {
    println!("AWR_ISOLATION_CASE {id}");
}

struct Contender {
    child: Child,
    lines: mpsc::Receiver<String>,
}
impl Contender {
    fn new(f: &Fixture, agent: &str, branch: Option<Id>) -> Self {
        let path = f.root.join(format!("contender-{}.json", Id::new()));
        fs::write(
            &path,
            serde_json::to_vec(&json!({"project":f.project.id,"agent":agent,"branch":branch}))
                .unwrap(),
        )
        .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "fixture_contender",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("AWR_ISOLATION_SPEC", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if send.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let mut result = Self { child, lines };
        assert_eq!(result.receive("AWR_ISOLATION_READY "), "ready");
        assert!(result.child.try_wait().unwrap().is_none());
        result
    }
    fn receive(&mut self, prefix: &str) -> String {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let line = self
                .lines
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .expect("live contender did not respond");
            if let Some((_, result)) = line.split_once(prefix) {
                return result.to_owned();
            }
        }
    }
    fn send(&mut self, revision: Revision) {
        assert!(self.child.try_wait().unwrap().is_none());
        let input = self.child.stdin.as_mut().unwrap();
        writeln!(input, "{revision}").unwrap();
        input.flush().unwrap();
    }
    fn result(&mut self) -> Value {
        let result = serde_json::from_str(&self.receive("AWR_ISOLATION_RESULT ")).unwrap();
        assert!(self.child.try_wait().unwrap().is_none());
        result
    }
}
impl Drop for Contender {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "Private live OS process fixture; parent cases supply its temporary project"]
fn fixture_contender() {
    let path = PathBuf::from(
        std::env::var_os("AWR_ISOLATION_SPEC").expect("requires fixture specification"),
    );
    let root = path.parent().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("fixture-owner")).unwrap(),
        "awr-isolation-test"
    );
    let specification: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let project = serde_json::from_value(specification["project"].clone()).unwrap();
    let branch = serde_json::from_value(specification["branch"].clone()).unwrap();
    let mut store = Store::open_existing(&root.join("state.db")).unwrap();
    assert_eq!(
        store.project(project).unwrap().root,
        root.canonicalize().unwrap()
    );
    println!("AWR_ISOLATION_READY ready");
    std::io::stdout().flush().unwrap();
    for line in std::io::stdin().lock().lines() {
        let expected: Revision = line.unwrap().parse().unwrap();
        let result = store.start_session(
            project,
            expected,
            draft(specification["agent"].as_str().unwrap(), branch, true, None),
        );
        let value = match result {
            Ok((started, event)) => json!({"ok":true,"started":started,"event":event}),
            Err(error) => json!({"ok":false,"code":error.code()}),
        };
        println!("AWR_ISOLATION_RESULT {value}");
        std::io::stdout().flush().unwrap();
    }
}

#[test]
fn case_two_live_contenders_share_one_revision_and_one_claim_owner() {
    let f = fixture();
    let before = revision(&f);
    let mut writers = [
        Contender::new(&f, "first-reviewer", None),
        Contender::new(&f, "second-reviewer", None),
    ];
    assert_ne!(writers[0].child.id(), writers[1].child.id());
    for writer in &mut writers {
        writer.send(before);
    }
    let results = [writers[0].result(), writers[1].result()];
    assert_eq!(results.iter().filter(|v| v["ok"] == true).count(), 1);
    let loser = results
        .iter()
        .position(|v| v["code"] == "RevisionConflict")
        .unwrap();
    assert_eq!(revision(&f), before + 1);
    assert_eq!(f.store.sessions(f.project.id, false, 100).unwrap().len(), 1);
    let stable = state(&f);
    writers[loser].send(revision(&f));
    assert_eq!(writers[loser].result()["code"], "ClaimConflict");
    assert_eq!(state(&f), stable);
    passed("claim_compare_revision_processes");
    passed("claim_single_owner_processes");
}

#[test]
fn case_expired_claim_race_keeps_one_new_owner_and_original_receipts() {
    let mut f = fixture();
    let expired = start(&mut f, "original-reviewer", None, true, Some(1))
        .claim
        .unwrap();
    wait_expired(&expired);
    let mut writers = [
        Contender::new(&f, "next-reviewer", None),
        Contender::new(&f, "other-reviewer", None),
    ];
    let before = revision(&f);
    for writer in &mut writers {
        writer.send(before);
    }
    let results = [writers[0].result(), writers[1].result()];
    let winner = results.iter().find(|v| v["ok"] == true).unwrap();
    assert_eq!(results.iter().filter(|v| v["ok"] == true).count(), 1);
    assert_eq!(
        winner["event"]["payload"]["expired_claim_ids"],
        json!([expired.id])
    );
    let loser = results
        .iter()
        .position(|v| v["code"] == "RevisionConflict")
        .unwrap();
    assert_eq!(
        f.store.claim(f.project.id, expired.id).unwrap().status,
        "expired"
    );
    let current = state(&f);
    writers[loser].send(revision(&f));
    assert_eq!(writers[loser].result()["code"], "ClaimConflict");
    assert_eq!(state(&f), current);
    assert_eq!(f.store.sessions(f.project.id, false, 100).unwrap().len(), 2);
    passed("claim_expired_race_processes");
}

#[test]
fn case_parallel_branches_cannot_bypass_work_ownership() {
    let mut f = fixture();
    let a = branch(&mut f, "review-a");
    let b = branch(&mut f, "review-b");
    let source_work = json!(f.store.work_item(f.project.id, "W").unwrap().item);
    let mut writers = [
        Contender::new(&f, "reviewer-a", Some(a)),
        Contender::new(&f, "reviewer-b", Some(b)),
    ];
    let before = revision(&f);
    for writer in &mut writers {
        writer.send(before);
    }
    let mut results = [writers[0].result(), writers[1].result()];
    let loser = results
        .iter()
        .position(|v| v["code"] == "RevisionConflict")
        .unwrap();
    writers[loser].send(revision(&f));
    results[loser] = writers[loser].result();
    assert_eq!(results[loser]["code"], "ClaimConflict");
    let winner = 1 - loser;
    let result = &results[winner];
    let expected = [a, b][winner];
    assert_eq!(result["ok"], true);
    assert_eq!(result["started"]["session"]["branch_id"], json!(expected));
    assert_eq!(result["started"]["claim"]["branch_id"], json!(expected));
    assert_eq!(result["event"]["branch_id"], json!(expected));
    let claim: Id = serde_json::from_value(result["started"]["claim"]["id"].clone()).unwrap();
    assert!(
        f.store
            .claim(f.project.id, claim)
            .unwrap()
            .active_at(now_millis().unwrap())
    );
    assert_eq!(f.store.sessions(f.project.id, false, 100).unwrap().len(), 1);
    assert_eq!(revision(&f), before + 1);
    assert_eq!(
        json!(f.store.work_item(f.project.id, "W").unwrap().item),
        source_work
    );
    passed("branch_parallel_sessions_claims");
}

#[test]
fn case_future_and_indefinite_claims_cannot_be_expired_or_stolen() {
    for ttl in [None, Some(60_000)] {
        let mut f = fixture();
        let owner = start(&mut f, "owner", None, true, ttl);
        let claim = owner.claim.unwrap();
        let before = state(&f);
        assert!(matches!(
            f.store.reconcile(
                f.project.id,
                revision(&f),
                ReconcileAction::ExpireClaim { claim_id: claim.id },
                "Inspect a selected claim"
            ),
            Err(Error::InvalidTransition(_))
        ));
        assert!(matches!(
            f.store.start_session(
                f.project.id,
                revision(&f),
                draft("contender", None, true, None)
            ),
            Err(Error::ClaimConflict(_))
        ));
        assert_eq!(state(&f), before);
    }
    passed("claim_protect_unexpired_and_indefinite");
}

#[test]
fn case_selected_expiry_and_wrong_owner_release_preserve_other_claims() {
    let mut f = fixture();
    let a = branch(&mut f, "review-a");
    let b = branch(&mut f, "review-b");
    let main = start(&mut f, "main-owner", None, true, None);
    let expired = start_work(&mut f, "X", "a-owner", Some(a), Some(1));
    let live = start_work(&mut f, "Y", "b-owner", Some(b), Some(60_000));
    let before = state(&f);
    assert!(matches!(
        f.store.release_claim(
            f.project.id,
            revision(&f),
            live.session.id,
            main.claim.as_ref().unwrap().id
        ),
        Err(Error::ClaimConflict(_))
    ));
    assert_eq!(state(&f), before);
    let foreign_root = f.root.join("other-project");
    fs::create_dir(&foreign_root).unwrap();
    let foreign = f
        .store
        .register_project(&foreign_root, "other", "Other review")
        .unwrap();
    assert!(matches!(
        f.store.release_claim(
            foreign.id,
            foreign.project_revision,
            main.session.id,
            main.claim.as_ref().unwrap().id
        ),
        Err(Error::NotFound(_))
    ));
    assert_eq!(state(&f), before);
    let claim = expired.claim.unwrap();
    wait_expired(&claim);
    let receipt = f
        .store
        .reconcile(
            f.project.id,
            revision(&f),
            ReconcileAction::ExpireClaim { claim_id: claim.id },
            "Reclaim the expired report reservation",
        )
        .unwrap();
    assert_eq!(receipt.event.branch_id, Some(a));
    assert_eq!(receipt.event.session_id, Some(expired.session.id));
    assert_eq!(
        f.store.claim(f.project.id, claim.id).unwrap().status,
        "expired"
    );
    for original in [main.claim.unwrap(), live.claim.unwrap()] {
        assert_eq!(
            json!(f.store.claim(f.project.id, original.id).unwrap()),
            json!(original)
        );
    }
    passed("claim_expiry_scope");
    passed("claim_release_owner");
}

#[test]
fn case_invalid_ttl_and_failed_reclaim_receipts_leave_no_partial_ownership() {
    let mut f = fixture();
    for ttl in [0, u64::MAX] {
        let before = state(&f);
        assert!(matches!(
            f.store.start_session(
                f.project.id,
                revision(&f),
                draft("invalid-ttl", None, true, Some(ttl))
            ),
            Err(Error::InvalidInput(_))
        ));
        assert_eq!(state(&f), before);
    }
    let first = start(&mut f, "original-reviewer", None, true, Some(1));
    wait_expired(first.claim.as_ref().unwrap());
    let before = state(&f);
    let db = rusqlite::Connection::open(f.root.join("state.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_fixture_session BEFORE INSERT ON events WHEN NEW.event_type='session.started' BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END;").unwrap();
    assert!(matches!(
        f.store.start_session(
            f.project.id,
            revision(&f),
            draft("new-reviewer", None, true, None)
        ),
        Err(Error::Storage(_))
    ));
    assert_eq!(state(&f), before);
    db.execute_batch("DROP TRIGGER reject_fixture_session")
        .unwrap();
    let successor = start(&mut f, "new-reviewer", None, true, None);
    assert_eq!(
        f.store
            .claim(f.project.id, first.claim.unwrap().id)
            .unwrap()
            .status,
        "expired"
    );
    assert!(successor.claim.unwrap().active_at(now_millis().unwrap()));
    passed("claim_ttl_invalid_rolls_back");
    passed("claim_event_failure_rollback");
}

#[test]
fn case_public_events_reject_forged_receipts_and_conflicting_identity() {
    let mut f = fixture();
    let a = branch(&mut f, "review-a");
    let b = branch(&mut f, "review-b");
    let owner = start(&mut f, "owner", Some(a), false, None);
    let baseline = state(&f);
    for kind in [
        "claim.expired",
        "work.completed",
        "session.resumed",
        "branch.closed",
    ] {
        let event = EventDraft::new(kind, "A fabricated domain receipt");
        assert!(matches!(
            f.store.append_event(f.project.id, revision(&f), event),
            Err(Error::InvalidInput(_))
        ));
    }
    for conflict in ["branch", "work", "session"] {
        let mut event = EventDraft::new("report.observed", "Report review observation");
        event.session_id = Some(owner.session.id);
        match conflict {
            "branch" => event.branch_id = Some(b),
            "work" => event.work_item_id = Some(f.store.work_identity(f.project.id, "X").unwrap()),
            _ => event.session_id = Some(Id::new()),
        }
        assert!(
            f.store
                .append_event(f.project.id, revision(&f), event)
                .is_err()
        );
    }
    let mut explicit_main = EventDraft::new("report.observed", "Explicit main review");
    explicit_main.session_id = Some(owner.session.id);
    assert!(matches!(
        f.store
            .append_event_in_branch(f.project.id, revision(&f) - 1, explicit_main.clone()),
        Err(Error::RevisionConflict { .. })
    ));
    assert!(matches!(
        f.store
            .append_event_in_branch(f.project.id, revision(&f), explicit_main),
        Err(Error::InvalidInput(_))
    ));
    let mut returned = f.store.events_since(f.project.id, 0, 1000).unwrap();
    returned[0].summary = "Caller changed its own copy".into();
    assert_eq!(state(&f), baseline);
    let mut event = EventDraft::new("report.observed", "Bound review observation");
    event.session_id = Some(owner.session.id);
    let bound = f
        .store
        .append_event(f.project.id, revision(&f), event)
        .unwrap();
    assert_eq!(bound.branch_id, Some(a));
    assert_eq!(bound.work_item_id, owner.session.work_item_id);
    let mut explicit_branch = EventDraft::new("report.observed", "Explicit branch review");
    explicit_branch.session_id = Some(owner.session.id);
    explicit_branch.branch_id = Some(a);
    let selected = f
        .store
        .append_event_in_branch(f.project.id, revision(&f), explicit_branch)
        .unwrap();
    assert_eq!(selected.branch_id, Some(a));
    assert_eq!(selected.work_item_id, owner.session.work_item_id);
    let foreign_root = f.root.join("other-project");
    fs::create_dir(&foreign_root).unwrap();
    let foreign = f
        .store
        .register_project(&foreign_root, "other", "Other review")
        .unwrap();
    let stable = state(&f);
    let mut event = EventDraft::new("report.observed", "Wrong project");
    event.session_id = Some(owner.session.id);
    assert!(matches!(
        f.store
            .append_event(foreign.id, foreign.project_revision, event),
        Err(Error::NotFound(_))
    ));
    assert_eq!(state(&f), stable);
    passed("event_reserved_and_bound_provenance");
}

#[test]
fn case_branch_pagination_default_switch_and_session_selection_keep_identity() {
    let mut f = fixture();
    let a = branch(&mut f, "review-a");
    let b = branch(&mut f, "review-b");
    let main = start(&mut f, "main-owner", None, true, None);
    let in_a = start_work(&mut f, "X", "a-owner", Some(a), None);
    let in_b = start_work(&mut f, "Y", "b-owner", Some(b), None);
    let cp = checkpoint(&mut f, in_a.session.id, "Review A checkpoint");
    let sessions = [main, in_a, in_b];
    for owner in &sessions {
        for _ in 0..3 {
            let mut e = EventDraft::new("report.observed", "Review progress");
            e.session_id = Some(owner.session.id);
            f.store.append_event(f.project.id, revision(&f), e).unwrap();
        }
    }
    let history = json!(f.store.events_since(f.project.id, 0, 1000).unwrap());
    let ownership:Vec<_>=sessions.iter().map(|s|json!({"session":f.store.session(f.project.id,s.session.id).unwrap(),"claims":f.store.session_claims(f.project.id,s.session.id).unwrap()})).collect();
    f.store
        .switch_branch(
            f.project.id,
            revision(&f),
            Some(b),
            "reviewer",
            "Continue on review B",
        )
        .unwrap();
    for (owner, before) in sessions.iter().zip(ownership) {
        assert_eq!(
            json!({"session":f.store.session(f.project.id,owner.session.id).unwrap(),"claims":f.store.session_claims(f.project.id,owner.session.id).unwrap()}),
            before
        );
        let mut query = EventQuery {
            branch: BranchFilter::exact(owner.session.branch_id),
            session_id: Some(owner.session.id),
            event_type: Some("report.observed".into()),
            limit: 1,
            ..Default::default()
        };
        let mut ids = Vec::new();
        loop {
            let page = f.store.query_events(f.project.id, &query).unwrap();
            for event in page.events {
                assert_eq!(event.branch_id, owner.session.branch_id);
                assert_eq!(event.session_id, Some(owner.session.id));
                ids.push(event.id);
            }
            if let Some(cursor) = page.next_cursor {
                query.cursor = Some(cursor);
            } else {
                break;
            }
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3);
        assert_eq!(
            f.store
                .select_active_session(f.project.id, None, None, None, owner.session.branch_id)
                .unwrap()
                .id,
            owner.session.id
        );
    }
    assert_eq!(
        f.store
            .latest_checkpoint(f.project.id, sessions[1].session.id)
            .unwrap()
            .unwrap()
            .id,
        cp.id
    );
    let previous = history.as_array().unwrap();
    assert_eq!(
        json!(f.store.events_since(f.project.id, 0, 1000).unwrap())
            .as_array()
            .unwrap()[..previous.len()],
        previous[..]
    );
    let observer = start(&mut f, "main-observer", None, false, None);
    assert!(matches!(
        f.store
            .select_active_session(f.project.id, None, None, None, None),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        f.store
            .select_active_session(
                f.project.id,
                Some(observer.session.id),
                None,
                Some("main-observer"),
                None
            )
            .unwrap()
            .id,
        observer.session.id
    );
    assert!(matches!(
        f.store.select_active_session(
            f.project.id,
            Some(observer.session.id),
            None,
            Some("another-agent"),
            None
        ),
        Err(Error::NotFound(_))
    ));
    assert_eq!(
        f.store
            .select_active_session(f.project.id, None, None, None, Some(b))
            .unwrap()
            .id,
        sessions[2].session.id
    );
    passed("event_branch_query_isolation");
    passed("branch_switch_retains_ownership");
    passed("branch_session_selection");
}

#[test]
fn case_handoff_and_resume_keep_branch_checkpoint_and_live_claim_bindings() {
    let mut f = fixture();
    let a = branch(&mut f, "review-a");
    let b = branch(&mut f, "review-b");
    let from = start(&mut f, "sender", Some(a), true, Some(60_000));
    let to = start(&mut f, "receiver", Some(a), false, None);
    let other = start_work(&mut f, "X", "other-reviewer", Some(b), None);
    let cp = checkpoint(&mut f, from.session.id, "Source review checkpoint");
    let before = state(&f);
    assert!(matches!(
        f.store.handoff(
            f.project.id,
            revision(&f),
            from.session.id,
            Some(other.session.id),
            None
        ),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(state(&f), before);
    let (handoff, _) = f
        .store
        .handoff(
            f.project.id,
            revision(&f),
            from.session.id,
            Some(to.session.id),
            None,
        )
        .unwrap();
    let claim = handoff.transferred_claim.unwrap();
    assert_eq!(claim.branch_id, Some(a));
    assert_eq!(claim.session_id, to.session.id);
    assert_eq!(claim.expires_at, from.claim.unwrap().expires_at);
    assert_eq!(handoff.checkpoint.session_id, from.session.id);
    assert_eq!(
        f.store
            .incoming_handoff(f.project.id, to.session.id)
            .unwrap()
            .unwrap()
            .id,
        cp.id
    );
    assert!(
        f.store
            .claim(f.project.id, other.claim.as_ref().unwrap().id)
            .unwrap()
            .active_at(now_millis().unwrap())
    );
    let resume = SessionResumeDraft {
        from_session_id: to.session.id,
        checkpoint_id: Some(cp.id),
        agent_id: "successor".into(),
        provider: "fixture".into(),
        model: "metadata-test".into(),
        claim: ResumeClaim::Inherit,
        claim_ttl_ms: None,
        prepared_context_hash: "b".repeat(64),
    };
    let before = state(&f);
    assert!(matches!(
        f.store
            .resume_session(f.project.id, revision(&f), resume.clone()),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(state(&f), before);
    f.store
        .switch_branch(
            f.project.id,
            revision(&f),
            Some(a),
            "reviewer",
            "Resume the original review branch",
        )
        .unwrap();
    let (resumed, event) = f
        .store
        .resume_session(f.project.id, revision(&f), resume.clone())
        .unwrap();
    assert_eq!(resumed.session.branch_id, Some(a));
    assert_eq!(resumed.claim.unwrap().expires_at, claim.expires_at);
    assert_eq!(resumed.checkpoint.unwrap().session_id, from.session.id);
    assert_eq!(event.branch_id, Some(a));
    let before = state(&f);
    assert!(matches!(
        f.store.resume_session(f.project.id, revision(&f), resume),
        Err(Error::InvalidTransition(_))
    ));
    assert_eq!(state(&f), before);
    assert_eq!(
        json!(
            f.store
                .claim(f.project.id, other.claim.as_ref().unwrap().id)
                .unwrap()
        ),
        json!(other.claim.unwrap())
    );
    passed("branch_handoff_binding");
    passed("branch_resume_binding");
}

#[test]
fn case_end_and_explicit_interruption_release_only_selected_owners() {
    let mut f = fixture();
    let a = branch(&mut f, "review-a");
    let b = branch(&mut f, "review-b");
    let main = start(&mut f, "main-owner", None, true, None);
    let first = start_work(&mut f, "X", "a-owner", Some(a), None);
    let other = start_work(&mut f, "Y", "b-owner", Some(b), None);
    end(&mut f, first.session.id);
    let before = json!(
        f.store
            .claim(f.project.id, other.claim.as_ref().unwrap().id)
            .unwrap()
    );
    let receipt = f
        .store
        .reconcile(
            f.project.id,
            revision(&f),
            ReconcileAction::InterruptSession {
                session_id: main.session.id,
            },
            "Owner explicitly ended the interrupted review",
        )
        .unwrap();
    assert_eq!(receipt.event.branch_id, None);
    assert_eq!(receipt.event.session_id, Some(main.session.id));
    for owner in [first, main] {
        assert_eq!(
            f.store
                .claim(f.project.id, owner.claim.unwrap().id)
                .unwrap()
                .status,
            "released"
        );
    }
    assert_eq!(
        json!(
            f.store
                .claim(f.project.id, other.claim.unwrap().id)
                .unwrap()
        ),
        before
    );
    assert_eq!(
        f.store
            .session(f.project.id, other.session.id)
            .unwrap()
            .status,
        "active"
    );
    passed("branch_cleanup_scope");
}

#[test]
fn case_closed_branch_rejects_new_domain_mutations_without_rewriting_history() {
    let mut f = fixture();
    let a = branch(&mut f, "review-a");
    let owner = start(&mut f, "reviewer", Some(a), false, None);
    end(&mut f, owner.session.id);
    f.store
        .close_branch(
            f.project.id,
            a,
            revision(&f),
            CloseBranchDraft {
                input: CloseBranchInput {
                    version: 1,
                    outcome: BranchCloseOutcome::Abandoned,
                    summary: "Review approach was abandoned".into(),
                    merge: None,
                    open_loops: vec![],
                },
                target_branch_id: None,
                actor: "reviewer".into(),
                reason: "Close the completed investigation".into(),
                source_versions: vec![f.source.clone().into()],
                merge: None,
            },
        )
        .unwrap();
    let before = state(&f);
    assert!(matches!(
        f.store.start_session(
            f.project.id,
            revision(&f),
            draft("new-reviewer", Some(a), true, None)
        ),
        Err(Error::NotFound(_))
    ));
    assert!(
        f.store
            .acquire_claim(f.project.id, revision(&f), owner.session.id, None)
            .is_err()
    );
    let mut event = EventDraft::new("report.observed", "Closed review observation");
    event.branch_id = Some(a);
    assert!(matches!(
        f.store.append_event(f.project.id, revision(&f), event),
        Err(Error::NotFound(_))
    ));
    assert_eq!(state(&f), before);
    passed("branch_closed_mutation_rejection");
}

#[test]
fn case_checkpoint_and_evidence_selection_preserve_exact_branch_provenance() {
    let mut f = fixture();
    let a = branch(&mut f, "review-a");
    let b = branch(&mut f, "review-b");
    let mut checkpoints = Vec::new();
    for (name, branch) in [("main", None), ("review-a", Some(a)), ("review-b", Some(b))] {
        let owner = start(&mut f, name, branch, false, None);
        let cp = checkpoint(&mut f, owner.session.id, name);
        end(&mut f, owner.session.id);
        checkpoints.push((branch, cp));
        f.store
            .record_evidence(
                f.project.id,
                revision(&f),
                EvidenceDraft {
                    external_key: name.into(),
                    work_item_key: Some("W".into()),
                    evidence_type: "review_report".into(),
                    level: EvidenceLevel::LocallyVerified,
                    summary: "Synthetic report binding for component verification".into(),
                    locator: format!("reports/{name}.json"),
                    sha256: Some("a".repeat(64)),
                    source_sha: Some("b".repeat(40)),
                    command: Some("review-report".into()),
                    scope: vec!["W".into()],
                    branch_id: branch,
                    verified_at: Some(now_millis().unwrap()),
                },
            )
            .unwrap();
    }
    let work = f.store.work_identity(f.project.id, "W").unwrap();
    for (branch, expected) in checkpoints {
        assert_eq!(
            f.store
                .latest_work_checkpoint(f.project.id, work, branch)
                .unwrap()
                .unwrap()
                .id,
            expected.id
        );
        let records = f
            .store
            .evidence_for_work(f.project.id, "W", Some(&"b".repeat(40)), branch)
            .unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(
            records
                .iter()
                .filter(|e| e.currency == EvidenceCurrency::Current)
                .count(),
            1
        );
        for record in records {
            assert_eq!(
                record.currency,
                if record.evidence.item.branch_id == branch {
                    EvidenceCurrency::Current
                } else {
                    EvidenceCurrency::Historical
                }
            );
        }
    }
    assert_eq!(
        f.store
            .work_item(f.project.id, "W")
            .unwrap()
            .item
            .owner
            .as_deref(),
        Some("source-team")
    );
    passed("branch_checkpoint_and_evidence_scoping");
}
