//! The explicit clean-up of what completed or cancelled work left behind: a plan that changes nothing,
//! and the same plan accepted by its fingerprint.
use awr_core::*;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const WORK: &str = "work_items:\n- id: W\n  title: Deliver the report\n  status: in_progress\n  acceptance: [The report is delivered]\n- id: DONE\n  title: Another delivery\n  status: in_progress\n  acceptance: [The other delivery is made]\n- id: OPEN\n  title: Work that goes on\n  status: in_progress\n  acceptance: [The work is done]\n- id: FREE\n  title: Work nobody holds\n  status: in_progress\n  acceptance: [The free work is done]\n";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            root: std::env::temp_dir().join(format!("awr-finished-work-{}", Id::new())),
        };
        fs::create_dir(&f.root).unwrap();
        fs::write(f.root.join("work.yaml"), WORK).unwrap();
        fs::write(f.root.join("sources.toml"), "[project]\nname='Finished work'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
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
    fn refused(&self, args: &[&str], code: &str) {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} succeeded");
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], code, "{error}");
    }
    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }
    fn start(&self, work: &str, agent: &str, claim: bool) -> (String, Option<String>) {
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
        let started = self.ok(&args);
        (
            started["session"]["id"].as_str().unwrap().into(),
            started["claim"]["id"].as_str().map(str::to_string),
        )
    }
    /// The source says the work is finished, and a refresh has already put it into the projection
    /// without reconciling: the claim it left is backlog.
    fn finish(&self, work: &str, status: &str) {
        let before = format!("- id: {work}\n  title: ");
        let text = fs::read_to_string(self.root.join("work.yaml")).unwrap();
        let start = text.find(&before).unwrap();
        let line = text[start..].find("  status: ").unwrap() + start;
        let end = text[line..].find('\n').unwrap() + line;
        fs::write(
            self.root.join("work.yaml"),
            format!("{}  status: {status}{}", &text[..line], &text[end..]),
        )
        .unwrap();
        self.sql(
            "UPDATE work_items SET status=?1 WHERE external_key=?2",
            rusqlite::params![status, work],
        );
        self.ok(&["status"]);
    }
    fn sql(&self, statement: &str, params: impl rusqlite::Params) {
        let db = rusqlite::Connection::open(self.root.join(".awr/state.db")).unwrap();
        db.execute(statement, params).unwrap();
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
    fn plan(&self) -> Value {
        self.ok(&["doctor", "repair", "finished-work"])
    }
    fn accept(&self, plan: &Value, reason: &str) -> Value {
        self.ok(&[
            "doctor",
            "repair",
            "finished-work",
            "--accept",
            "--expected-preview",
            plan["preview_fingerprint"].as_str().unwrap(),
            "--expected-revision",
            &plan["project_revision"].to_string(),
            "--reason",
            reason,
        ])
    }
}

/// W has a claimed session, DONE an idle one, OPEN a claimed one; W and DONE are finished.
fn backlog() -> (Fixture, [String; 3], String) {
    let f = Fixture::new();
    let (on_w, claim) = f.start("W", "worker-a", true);
    let (on_done, _) = f.start("DONE", "worker-b", false);
    let (on_open, _) = f.start("OPEN", "worker-c", true);
    f.finish("W", "completed");
    f.finish("DONE", "cancelled");
    (f, [on_w, on_done, on_open], claim.unwrap())
}

#[test]
fn the_plan_shows_what_finished_work_left_behind_and_changes_nothing() {
    let (f, [on_w, on_done, on_open], claim) = backlog();
    let events = f.ok(&["event", "history", "--limit", "1000"])["events"]
        .as_array()
        .unwrap()
        .len();
    let plan = f.plan();
    assert_eq!(plan["preview"], true);
    assert_eq!(plan["read_only"], true);
    let sessions: Vec<&str> = plan["plan"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(sessions.len(), 2, "{plan}");
    assert!(sessions.contains(&on_w.as_str()) && sessions.contains(&on_done.as_str()));
    assert!(!sessions.contains(&on_open.as_str()));
    let held = plan["plan"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == json!(on_w))
        .unwrap();
    assert_eq!(held["claim_ids"], json!([claim]));
    assert_eq!(held["work_item_key"], "W");
    assert_eq!(held["work_status"], "completed");
    assert!(
        plan["preview_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    // Nothing was written.
    assert_eq!(f.session_status(&on_w), "active");
    assert_eq!(f.claim_status(&claim), "active");
    assert_eq!(
        f.ok(&["event", "history", "--limit", "1000"])["events"]
            .as_array()
            .unwrap()
            .len(),
        events
    );
    // The same facts give the same plan.
    assert_eq!(f.plan()["preview_fingerprint"], plan["preview_fingerprint"]);
}

#[test]
fn accepting_the_plan_closes_the_sessions_and_releases_the_claims_and_changes_no_work() {
    let (f, [on_w, on_done, on_open], claim) = backlog();
    let source = fs::read(f.root.join("work.yaml")).unwrap();
    let plan = f.plan();
    let applied = f.accept(&plan, "Clean up the backlog of finished work");
    assert_eq!(applied["repair_applied"], true);
    assert_eq!(applied["applied"]["released_claim_ids"], json!([claim]));
    assert_eq!(applied["applied"]["skipped"], json!([]));
    assert_eq!(f.session_status(&on_w), "interrupted");
    assert_eq!(f.session_status(&on_done), "interrupted");
    assert_eq!(f.session_status(&on_open), "active");
    assert_eq!(f.claim_status(&claim), "released");
    // Each receipt says why and who asked, and what the source said.
    let receipts = f.events("session.interrupted");
    assert_eq!(receipts.len(), 2);
    for receipt in &receipts {
        let payload = &receipt["payload"];
        assert_eq!(payload["reason"], "source_terminal");
        assert_eq!(payload["note"], "Clean up the backlog of finished work");
        assert!(
            payload["source_fingerprint"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
    }
    // The statuses of the works and the source are untouched; nothing is called ended.
    assert_eq!(f.ok(&["work", "show", "W"])["work"]["status"], "completed");
    assert_eq!(
        f.ok(&["work", "show", "DONE"])["work"]["status"],
        "cancelled"
    );
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), source);
    assert!(f.events("session.ended").is_empty());
    // Nothing is left to do.
    let again = f.plan();
    assert_eq!(again["plan"]["sessions"], json!([]));
    assert_eq!(again["plan"]["claims"], json!([]));
}

#[test]
fn a_claim_of_a_session_that_goes_on_is_released_alone() {
    let (f, [on_w, _, _], claim) = backlog();
    // The same session also holds a claim on work that goes on.
    let other = Id::new().to_string();
    f.sql(
        "INSERT INTO claims(id,project_id,work_item_id,session_id,agent_id,branch_id,status,acquired_at,expires_at,released_at,revision) \
         SELECT ?1, project_id, id, ?2, 'worker-a', NULL, 'active', ?3, NULL, NULL, 1 FROM work_items WHERE external_key='FREE'",
        rusqlite::params![other, on_w, now_millis().unwrap()],
    );
    let plan = f.plan();
    assert_eq!(plan["plan"]["claims"][0]["claim_id"], json!(claim));
    assert!(
        !plan["plan"]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session_id"] == json!(on_w))
    );
    f.accept(&plan, "Release the claim on finished work");
    assert_eq!(f.claim_status(&claim), "released");
    assert_eq!(f.claim_status(&other), "active");
    assert_eq!(f.session_status(&on_w), "active");
    assert_eq!(
        f.events("claim.released").last().unwrap()["payload"]["note"],
        "Release the claim on finished work"
    );
}

#[test]
fn what_may_still_be_at_work_is_listed_and_left_alone() {
    let (f, [on_w, on_done, _], claim) = backlog();
    f.sql(
        "INSERT INTO events(id,project_id,project_revision,event_type,summary,importance,payload_json,session_id,created_at) \
         SELECT ?1, id, project_revision, 'checkpoint.started', 'Started a save', 'normal', '{}', ?2, ?3 FROM projects",
        rusqlite::params![Id::new().to_string(), on_w, now_millis().unwrap()],
    );
    let plan = f.plan();
    assert_eq!(
        plan["plan"]["skipped"][0]["reason"],
        "checkpoint_save_in_flight"
    );
    assert_eq!(plan["plan"]["skipped"][0]["object_id"], json!(on_w));
    assert_eq!(plan["plan"]["sessions"].as_array().unwrap().len(), 1);
    f.accept(&plan, "Clean up what is safe");
    assert_eq!(f.session_status(&on_w), "active");
    assert_eq!(f.claim_status(&claim), "active");
    assert_eq!(f.session_status(&on_done), "interrupted");
}

#[test]
fn a_plan_that_changed_a_revision_that_moved_and_a_missing_reason_are_refused() {
    let (f, [on_w, _, _], claim) = backlog();
    let plan = f.plan();
    let fingerprint = plan["preview_fingerprint"].as_str().unwrap();
    let revision = plan["project_revision"].to_string();
    // The fingerprint of another plan.
    f.refused(
        &[
            "doctor",
            "repair",
            "finished-work",
            "--accept",
            "--expected-preview",
            "sha256:0000",
            "--expected-revision",
            &revision,
            "--reason",
            "Clean up",
        ],
        "MutationConflict",
    );
    // A revision that is not the live one.
    f.refused(
        &[
            "doctor",
            "repair",
            "finished-work",
            "--accept",
            "--expected-preview",
            fingerprint,
            "--expected-revision",
            "1",
            "--reason",
            "Clean up",
        ],
        "RevisionConflict",
    );
    // No reason, and the options of --accept without it.
    f.refused(
        &[
            "doctor",
            "repair",
            "finished-work",
            "--accept",
            "--expected-preview",
            fingerprint,
            "--expected-revision",
            &revision,
        ],
        "InvalidInput",
    );
    f.refused(
        &[
            "doctor",
            "repair",
            "finished-work",
            "--expected-preview",
            fingerprint,
        ],
        "InvalidInput",
    );
    // Nothing was changed by any of them.
    assert_eq!(f.session_status(&on_w), "active");
    assert_eq!(f.claim_status(&claim), "active");
    assert!(f.events("session.interrupted").is_empty());
}
