//! A claim on work the source now calls completed or cancelled is released, and a session left
//! without a claim is closed as interrupted, when the sources are refreshed by a command that writes
//! or refreshes; whatever may still be at work is left alone and reported.
use awr_core::*;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const WORK: &str = "work_items:\n- id: W\n  title: Deliver the report\n  status: in_progress\n  owner: business-coordinator\n  acceptance: [The report is delivered]\n- id: OTHER\n  title: Another piece of work\n  status: in_progress\n  acceptance: [Another piece is delivered]\n";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            root: std::env::temp_dir().join(format!("awr-source-terminal-{}", Id::new())),
        };
        fs::create_dir(&f.root).unwrap();
        fs::write(f.root.join("work.yaml"), WORK).unwrap();
        fs::write(f.root.join("sources.toml"), "[project]\nname='Source terminal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .env("AWR_EXECUTION_TEST_ROOT", &self.root)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }
    /// A session on `work` that holds a claim; returns (session id, claim id).
    fn start(&self, work: &str, agent: &str) -> (String, String) {
        let started = self.start_with(work, agent, true);
        (
            started["session"]["id"].as_str().unwrap().into(),
            started["claim"]["id"].as_str().unwrap().into(),
        )
    }
    fn start_with(&self, work: &str, agent: &str, claim: bool) -> Value {
        let mut args = vec![
            "session",
            "start",
            "--work",
            work,
            "--agent",
            agent,
            "--provider",
            "fixture",
            "--model",
            "local",
        ];
        if claim {
            args.push("--claim");
        }
        let revision = self.revision();
        args.extend(["--expected-revision", &revision]);
        self.ok(&args)
    }
    /// The source says W (or OTHER) has this status; nothing is told to AWR.
    fn say(&self, work: &str, status: &str) {
        let before = format!("- id: {work}\n  title: ");
        let text = fs::read_to_string(self.root.join("work.yaml")).unwrap();
        let start = text.find(&before).unwrap();
        let line = text[start..].find("  status: ").unwrap() + start;
        let end = text[line..].find('\n').unwrap() + line;
        let edited = format!("{}  status: {status}{}", &text[..line], &text[end..]);
        fs::write(self.root.join("work.yaml"), edited).unwrap();
    }
    fn session_status(&self, session: &str) -> String {
        self.ok(&["session", "show", session])["session"]["status"]
            .as_str()
            .unwrap()
            .into()
    }
    fn claim_status(&self, claim: &str) -> String {
        let db = rusqlite::Connection::open(self.root.join(".awr/state.db")).unwrap();
        db.query_row("SELECT status FROM claims WHERE id=?1", [claim], |r| {
            r.get(0)
        })
        .unwrap()
    }
    fn events(&self, kind: &str) -> Vec<Value> {
        self.ok(&["event", "history", "--limit", "1000"])["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == kind)
            .map(|e| {
                self.ok(&["event", "show", e["id"].as_str().unwrap(), "--full"])["event"].clone()
            })
            .collect()
    }
    fn sql(&self, statement: &str, params: impl rusqlite::Params) {
        let db = rusqlite::Connection::open(self.root.join(".awr/state.db")).unwrap();
        db.execute(statement, params).unwrap();
    }
    fn source_bytes(&self) -> Vec<u8> {
        fs::read(self.root.join("work.yaml")).unwrap()
    }
}

#[test]
fn a_claim_on_work_the_source_calls_completed_is_released_and_its_session_interrupted() {
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    f.say("W", "completed");
    let source = f.source_bytes();
    let status = f.ok(&["status"]);
    let note = &status["reconciliation"];
    assert_eq!(note["reason"], "source_terminal");
    assert_eq!(note["released_claim_ids"], json!([claim]));
    assert_eq!(note["interrupted_session_ids"], json!([session]));
    assert_eq!(note["skipped"], json!([]));
    assert_eq!(f.session_status(&session), "interrupted");
    assert_eq!(f.claim_status(&claim), "released");
    // The receipt says why, and what the source said when it did.
    let events = f.events("session.interrupted");
    assert_eq!(events.len(), 1);
    let payload = &events[0]["payload"];
    assert_eq!(payload["reason"], "source_terminal");
    assert_eq!(payload["status"], "interrupted");
    assert_eq!(payload["released_claim_ids"], json!([claim]));
    assert!(payload["source_revision"].as_u64().unwrap() >= 1);
    assert!(
        payload["source_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    // The source and the status of the work are untouched, and a normal completion is not claimed.
    assert_eq!(f.source_bytes(), source);
    assert_eq!(f.ok(&["work", "show", "W"])["work"]["status"], "completed");
    assert!(f.events("session.ended").is_empty());
    // Asking again changes nothing and says nothing.
    let again = f.ok(&["status"]);
    assert!(again.get("reconciliation").is_none(), "{again}");
    assert_eq!(f.events("session.interrupted").len(), 1);
    // The reply's revision is the live one.
    assert_eq!(
        status["project_revision"],
        json!(f.revision().parse::<u64>().unwrap())
    );
}

#[test]
fn a_cancelled_work_is_the_same_and_the_work_can_be_claimed_again_when_it_is_reopened() {
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    f.say("W", "cancelled");
    let note = f.ok(&["ready"])["reconciliation"].clone();
    assert_eq!(note["released_claim_ids"], json!([claim]));
    assert_eq!(f.session_status(&session), "interrupted");
    // The owner reopens the work in the source: a new session may claim it.
    f.say("W", "in_progress");
    f.ok(&["source", "reindex"]);
    let (second, _) = f.start("W", "worker-b");
    assert_eq!(f.session_status(&second), "active");
}

#[test]
fn source_reindex_session_start_and_work_claim_reconcile_too() {
    // source reindex: the reply carries the note
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    f.say("W", "completed");
    let reindexed = f.ok(&["source", "reindex"]);
    assert_eq!(
        reindexed["reconciliation"]["released_claim_ids"],
        json!([claim])
    );
    assert_eq!(f.session_status(&session), "interrupted");

    // session start: the refresh moves the revision, so the write is refused as it always was, and
    // what the source finished is released first; asking again with the revision read now succeeds
    let f = Fixture::new();
    let (first, claim) = f.start("W", "worker-a");
    f.say("W", "completed");
    let stale = f.revision();
    let refused = f.run(&[
        "session",
        "start",
        "--work",
        "OTHER",
        "--agent",
        "worker-b",
        "--provider",
        "fixture",
        "--model",
        "local",
        "--claim",
        "--expected-revision",
        &stale,
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("RevisionConflict"));
    assert_eq!(f.session_status(&first), "interrupted");
    assert_eq!(f.claim_status(&claim), "released");
    let second = f.start_with("OTHER", "worker-b", true);
    assert_eq!(second["session"]["status"], "active");

    // work claim: the refresh reconciles before the claim is judged
    let f = Fixture::new();
    let (first, claim) = f.start("W", "worker-a");
    let idle = f.start_with("OTHER", "worker-b", false);
    f.say("W", "completed");
    let stale = f.revision();
    let refused = f.run(&[
        "work",
        "claim",
        "OTHER",
        "--session",
        idle["session"]["id"].as_str().unwrap(),
        "--expected-revision",
        &stale,
    ]);
    assert!(!refused.status.success());
    assert_eq!(f.session_status(&first), "interrupted");
    assert_eq!(f.claim_status(&claim), "released");
    let claimed = f.ok(&[
        "work",
        "claim",
        "OTHER",
        "--session",
        idle["session"]["id"].as_str().unwrap(),
        "--expected-revision",
        &f.revision(),
    ]);
    assert_eq!(claimed["ok"], true);
}

/// W is claimed and the source calls it completed; `stale` is the revision read before the refresh.
fn finished_under_a_claim() -> (Fixture, String, String, String) {
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    f.say("W", "completed");
    let stale = f.revision();
    (f, session, claim, stale)
}

#[test]
fn event_append_reconciles_before_the_moved_revision_refuses_it() {
    let (f, session, claim, stale) = finished_under_a_claim();
    let append = |revision: &str| {
        f.run(&[
            "event",
            "append",
            "--type",
            "review.observed",
            "--summary",
            "Read the delivered report",
            "--work",
            "W",
            "--expected-revision",
            revision,
        ])
    };
    let refused = append(&stale);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("RevisionConflict"));
    assert_eq!(f.claim_status(&claim), "released");
    assert_eq!(f.session_status(&session), "interrupted");
    let again = append(&f.revision());
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn evidence_add_reconciles_before_the_moved_revision_refuses_it() {
    let (f, session, claim, stale) = finished_under_a_claim();
    // The revision is judged before the input is read, so the input does not need to be complete.
    fs::write(f.root.join("evidence.json"), "{}").unwrap();
    let refused = f.run(&[
        "evidence",
        "add",
        "--input",
        "evidence.json",
        "--expected-revision",
        &stale,
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("RevisionConflict"));
    assert_eq!(f.claim_status(&claim), "released");
    assert_eq!(f.session_status(&session), "interrupted");
}

#[test]
fn a_work_whose_source_is_not_fresh_keeps_its_claim_until_the_source_is() {
    // The status the projection holds is not something to release a claim on while the source it came
    // from is unconfirmed; the store decides that, whichever command did the refreshing.
    let f = Fixture::new();
    let (_, claim) = f.start("W", "worker-a");
    let root = f.root.canonicalize().unwrap();
    let mut store = awr_store::Store::open(&root.join(".awr/state.db")).unwrap();
    let project = store.project_by_root(&root).unwrap().id;
    let watched = store.works_with_open_claims(project).unwrap();
    assert_eq!(watched.len(), 1);
    f.say("W", "completed");
    let manifest = awr_source::Manifest::load(&root).unwrap();
    assert!(
        awr_source::index_project(&mut store, &root, &manifest, false)
            .unwrap()
            .ok
    );
    for freshness in ["stale", "unavailable"] {
        f.sql("UPDATE sources SET freshness=?1", [freshness]);
        let report = store.reconcile_finished_work(project, &watched).unwrap();
        assert!(report.is_empty(), "{freshness}: {report:?}");
        assert_eq!(f.claim_status(&claim), "active", "{freshness}");
    }
    f.sql("UPDATE sources SET freshness='fresh'", []);
    let report = store.reconcile_finished_work(project, &watched).unwrap();
    assert_eq!(report.released_claim_ids.len(), 1, "{report:?}");
    assert_eq!(f.claim_status(&claim), "released");
}

#[test]
fn a_session_that_still_holds_a_claim_on_other_work_goes_on() {
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    // As a handoff or a resume can leave it: the same session also holds a claim on other work.
    let other = Id::new().to_string();
    f.sql(
        "INSERT INTO claims(id,project_id,work_item_id,session_id,agent_id,branch_id,status,acquired_at,expires_at,released_at,revision) \
         SELECT ?1, project_id, id, ?2, 'worker-a', NULL, 'active', ?3, NULL, NULL, 1 FROM work_items WHERE external_key='OTHER'",
        rusqlite::params![other, session, now_millis().unwrap()],
    );
    f.say("W", "completed");
    let note = f.ok(&["status"])["reconciliation"].clone();
    assert_eq!(note["released_claim_ids"], json!([claim]));
    assert_eq!(note["interrupted_session_ids"], json!([]));
    assert_eq!(f.session_status(&session), "active");
    assert_eq!(f.claim_status(&claim), "released");
    assert_eq!(f.claim_status(&other), "active");
    let released = f.events("claim.released");
    assert_eq!(
        released.last().unwrap()["payload"]["reason"],
        "source_terminal"
    );
    assert!(f.events("session.interrupted").is_empty());
}

#[test]
fn what_may_still_be_at_work_is_left_alone_and_reported() {
    // A checkpoint save that has not finished.
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    f.sql(
        "INSERT INTO events(id,project_id,project_revision,event_type,summary,importance,payload_json,session_id,created_at) \
         SELECT ?1, id, project_revision, 'checkpoint.started', 'Started a save', 'normal', '{}', ?2, ?3 FROM projects",
        rusqlite::params![Id::new().to_string(), session, now_millis().unwrap()],
    );
    f.say("W", "completed");
    let note = f.ok(&["status"])["reconciliation"].clone();
    assert_eq!(note["released_claim_ids"], json!([]));
    assert_eq!(note["skipped"][0]["reason"], "checkpoint_save_in_flight");
    assert_eq!(note["skipped"][0]["object_id"], json!(session));
    assert_eq!(note["skipped"][0]["work_item_key"], "W");
    assert_eq!(f.session_status(&session), "active");
    assert_eq!(f.claim_status(&claim), "active");

    // A change waiting to be applied.
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    f.sql(
        "INSERT INTO mutation_proposals(id,project_id,work_item_id,source_id,base_fingerprint,expected_revision,mutation_type,patch_json,status,created_by_session,revision,created_at) \
         SELECT ?1, w.project_id, w.id, s.id, 'sha256:x', 0, 'work_action', '{}', 'approved', ?2, 1, ?3 FROM work_items w, sources s WHERE w.external_key='W' AND s.domain='ledger'",
        rusqlite::params![Id::new().to_string(), session, now_millis().unwrap()],
    );
    f.say("W", "completed");
    let note = f.ok(&["status"])["reconciliation"].clone();
    assert_eq!(note["skipped"][0]["reason"], "change_waiting_to_be_applied");
    assert_eq!(f.session_status(&session), "active");
    assert_eq!(f.claim_status(&claim), "active");

    // An execution that has not ended.
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    let execution = f.ok(&[
        "execution",
        "register",
        "--session",
        &session,
        "--key",
        "ci-build",
        "--purpose",
        "Track the build",
        "--reference",
        "ci://build/1",
    ]);
    assert_eq!(execution["execution"]["state"], "registered");
    f.say("W", "completed");
    let note = f.ok(&["status"])["reconciliation"].clone();
    assert_eq!(note["skipped"][0]["reason"], "execution_not_ended");
    assert_eq!(f.session_status(&session), "active");
    assert_eq!(f.claim_status(&claim), "active");
}

#[test]
fn a_claim_on_another_branch_is_left_alone() {
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    let branch = f.ok(&[
        "branch",
        "create",
        "review",
        "--actor",
        "fixture",
        "--reason",
        "Review the report",
        "--expected-revision",
        &f.revision(),
    ]);
    let branch_id = branch["branch"]["id"].as_str().unwrap().to_string();
    f.sql(
        "UPDATE claims SET branch_id=?1 WHERE id=?2",
        rusqlite::params![branch_id, claim],
    );
    f.say("W", "completed");
    let note = f.ok(&["status"])["reconciliation"].clone();
    assert_eq!(note["skipped"][0]["reason"], "claim_on_another_branch");
    assert_eq!(f.claim_status(&claim), "active");
    assert_eq!(f.session_status(&session), "active");
}

#[test]
fn a_claim_that_stood_on_finished_work_before_the_refresh_is_not_swept_up() {
    // The reconciliation answers a change the refresh brought; the backlog has its own explicit path.
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    f.say("W", "completed");
    // The projection already calls the work finished when the refresh comes: the claim is backlog.
    f.sql(
        "UPDATE work_items SET status='completed' WHERE external_key='W'",
        [],
    );
    let status = f.ok(&["status"]);
    assert!(status.get("reconciliation").is_none(), "{status}");
    assert_eq!(f.claim_status(&claim), "active");
    assert_eq!(f.session_status(&session), "active");
}

#[test]
fn work_that_is_not_finished_keeps_its_claim_whatever_the_refresh_says() {
    let f = Fixture::new();
    let (session, claim) = f.start("W", "worker-a");
    f.say("W", "blocked");
    let status = f.ok(&["status"]);
    assert!(status.get("reconciliation").is_none(), "{status}");
    assert_eq!(f.claim_status(&claim), "active");
    assert_eq!(f.session_status(&session), "active");
}
