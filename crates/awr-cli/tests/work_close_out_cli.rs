use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const C1: &str = "The report contains the requested analysis";
const C2: &str = "The report is reviewed and delivered";
const WORK: &str = "work_items:\n- id: W\n  title: Deliver the report\n  status: in_progress\n  owner: business-coordinator\n  acceptance: [The report contains the requested analysis, The report is reviewed and delivered]\n- id: OTHER\n  title: Another piece of work\n  status: ready\n";

struct Fixture {
    root: PathBuf,
    session: String,
    claim: String,
}

impl Fixture {
    fn new() -> Self {
        let mut f = Self {
            root: std::env::temp_dir().join(format!("awr-close-out-{}", Id::new())),
            session: String::new(),
            claim: String::new(),
        };
        fs::create_dir(&f.root).unwrap();
        fs::write(f.root.join("work.yaml"), WORK).unwrap();
        fs::write(f.root.join("sources.toml"),"[project]\nname='Close out'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n").unwrap();
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        let s = f.start("W");
        f.session = s["session"]["id"].as_str().unwrap().into();
        f.claim = s["claim"]["id"].as_str().unwrap().into();
        f
    }
    fn start(&self, work: &str) -> Value {
        self.ok(&[
            "session",
            "start",
            "--work",
            work,
            "--agent",
            "worker-a",
            "--provider",
            "fixture",
            "--model",
            "local",
            "--claim",
            "--expected-revision",
            &self.revision(),
        ])
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
        Self::success(&self.run(args))
    }
    fn success(r: &Output) -> Value {
        assert!(
            r.status.success(),
            "{} {}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn error(r: &Output, code: &str) {
        assert!(
            !r.status.success(),
            "{}",
            String::from_utf8_lossy(&r.stdout)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stderr).unwrap()["code"],
            code,
            "{} {}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
    }
    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }
    fn store(&self) -> (Store, Id) {
        let s = Store::open_existing(&self.root.join(".awr/state.db")).unwrap();
        let p = s
            .project_by_root(&self.root.canonicalize().unwrap())
            .unwrap()
            .id;
        (s, p)
    }
    fn work_status(&self, work: &str) -> Value {
        self.ok(&["object", "show", "work", work, "--cached", "--full"])["object"]["status"].clone()
    }
    fn session_status(&self) -> Value {
        self.ok(&["session", "show", &self.session])["session"]["status"].clone()
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
    /// A claim of this session on other work, as a handoff or a resume can leave it.
    fn hold_other_claim(&self) {
        let db = rusqlite::Connection::open(self.root.join(".awr/state.db")).unwrap();
        db.execute(
            "INSERT INTO claims(id,project_id,work_item_id,session_id,agent_id,branch_id,status,acquired_at,expires_at,released_at,revision) \
             SELECT ?1, project_id, id, ?2, 'worker-a', NULL, 'active', ?3, NULL, NULL, 1 FROM work_items WHERE external_key='OTHER'",
            rusqlite::params![Id::new().to_string(), self.session, now_millis().unwrap()],
        )
        .unwrap();
    }
    fn record_evidence(&self) {
        let report = json!({"version":1,"work_item":"W","source_sha":SHA,"command":"verify report and delivery","scope":["W"],"verified_at":now_millis().unwrap(),"checks":[{"name":"content and delivery","passed":true,"details":"Checked the analysis and delivery receipt","criteria":[C1,C2]}]});
        let bytes = serde_json::to_vec_pretty(&report).unwrap();
        let locator = format!("report-{}.json", Id::new());
        fs::write(self.root.join(&locator), &bytes).unwrap();
        let draft = EvidenceDraft {
            external_key: "E".into(),
            work_item_key: Some("W".into()),
            evidence_type: "completion_report".into(),
            level: EvidenceLevel::LocallyVerified,
            summary: "Checked report".into(),
            locator,
            sha256: Some(
                awr_source::fingerprint(&bytes)
                    .trim_start_matches("sha256:")
                    .into(),
            ),
            source_sha: Some(SHA.into()),
            command: Some("verify report and delivery".into()),
            scope: vec!["W".into()],
            branch_id: None,
            verified_at: report["verified_at"].as_i64(),
        };
        let (mut s, p) = self.store();
        let r = s.project(p).unwrap().project_revision;
        s.record_evidence(p, r, draft).unwrap();
    }
    fn complete(&self, extra: &[&str]) -> Output {
        let input = json!({"version":1,"source_sha":SHA,"acceptance":[{"criterion":C1,"evidence":["E"]},{"criterion":C2,"evidence":["E"]}]});
        let path = self.root.join("completion.json");
        fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
        let revision = self.revision();
        let mut args = vec![
            "work",
            "complete",
            "W",
            "--session",
            &self.session,
            "--reason",
            "Reviewed the evidence for every acceptance criterion",
            "--input",
            path.to_str().unwrap(),
            "--expected-revision",
            &revision,
        ];
        args.extend_from_slice(extra);
        self.run(&args)
    }
}

#[test]
fn a_completion_tells_what_to_do_with_the_session_in_under_sixty_tokens() {
    let f = Fixture::new();
    f.record_evidence();
    let done = Fixture::success(&f.complete(&[]));
    assert_eq!(done["event"]["event_type"], "work.completed");
    let close_out = &done["close_out"];
    for field in ["condition", "basis", "next", "recheck"] {
        assert!(close_out[field].as_str().is_some_and(|t| !t.is_empty()));
    }
    assert!(close_out["next"].as_str().unwrap().contains("session end"));
    assert!(awr_context::token_count(&close_out.to_string()) <= 60);
    // Nothing else was done for the caller: the session is still active and not ended.
    assert_eq!(f.session_status(), "active");
    assert!(done.get("session_end").is_none());
    assert!(f.events("session.ended").is_empty());
}

#[test]
fn a_cancellation_tells_it_too() {
    let f = Fixture::new();
    let cancelled = f.ok(&[
        "work",
        "cancel",
        "W",
        "--session",
        &f.session,
        "--reason",
        "No longer needed",
        "--expected-revision",
        &f.revision(),
    ]);
    assert_eq!(cancelled["event"]["event_type"], "work.cancelled");
    assert!(
        cancelled["close_out"]["basis"]
            .as_str()
            .unwrap()
            .contains("cancelled")
    );
    assert!(awr_context::token_count(&cancelled["close_out"].to_string()) <= 60);
}

#[test]
fn the_summary_view_keeps_the_instruction() {
    let f = Fixture::new();
    f.record_evidence();
    let done = f.complete(&["--response-view", "summary"]);
    let done = Fixture::success(&done);
    assert_eq!(done["response_view"]["view"], "summary");
    assert!(done["close_out"]["next"].as_str().is_some());
}

#[test]
fn ending_the_session_with_the_completion_is_one_request() {
    let f = Fixture::new();
    f.record_evidence();
    let done = Fixture::success(&f.complete(&["--end-session"]));
    assert_eq!(done["event"]["event_type"], "work.completed");
    assert_eq!(f.work_status("W"), "completed");
    assert_eq!(f.session_status(), "ended");
    // No instruction is left to follow, and the receipt names both events.
    assert!(done.get("close_out").is_none());
    let end = &done["session_end"];
    assert_eq!(end["status"], "ended");
    assert_eq!(end["session_id"], f.session.as_str());
    assert_eq!(end["completion_event_id"], done["event"]["id"]);
    let ended = f.events("session.ended");
    assert_eq!(ended.len(), 1);
    assert_eq!(end["event_id"], ended[0]["id"]);
    assert_eq!(
        ended[0]["payload"]["completion_event_id"],
        done["event"]["id"]
    );
    // The claim of the work was released by the completion; the end releases nothing more.
    assert_eq!(
        done["event"]["payload"]["released_claim_ids"],
        json!([f.claim])
    );
    assert_eq!(end["closed_claim_ids"], json!([]));
    // A repeated request neither completes nor ends anything again: the session is not active any more.
    Fixture::error(&f.complete(&["--end-session"]), "InvalidInput");
    assert_eq!(f.events("session.ended").len(), 1);
}

#[test]
fn without_the_flag_the_session_stays_and_the_call_is_the_old_one() {
    let f = Fixture::new();
    f.record_evidence();
    let done = Fixture::success(&f.complete(&[]));
    assert!(done.get("session_end").is_none());
    assert_eq!(f.session_status(), "active");
    // The session can still be ended by itself, as before.
    f.ok(&[
        "session",
        "end",
        "--session",
        &f.session,
        "--outcome",
        "ended",
        "--expected-revision",
        &f.revision(),
    ]);
    assert_eq!(f.session_status(), "ended");
}

#[test]
fn another_claim_of_the_session_refuses_the_end_and_changes_nothing() {
    let f = Fixture::new();
    f.record_evidence();
    f.hold_other_claim();
    let before = f.revision();
    let refused = f.complete(&["--end-session"]);
    Fixture::error(&refused, "ClaimConflict");
    let detail = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(detail.contains("OTHER"), "{detail}");
    assert_eq!(f.revision(), before, "nothing was written");
    assert_eq!(f.work_status("W"), "in_progress");
    assert_eq!(f.session_status(), "active");
    // Completed without the flag, no instruction is told while the other claim is held.
    let done = Fixture::success(&f.complete(&[]));
    assert!(done.get("close_out").is_none());
}
