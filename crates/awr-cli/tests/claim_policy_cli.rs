//! The project's claim policy at the command line: the lifetime of a claim whose request gave none,
//! `--no-expiry`, the renewal that recorded work earns, and what `doctor` and `capabilities` say about it.
use awr_core::*;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const POLICY: &str = "version = 1\n\n[claims]\ndefault_ttl = \"4h\"\nrenew_on_activity = true\n";
const WORK: &str = "work_items:\n- id: W\n  title: Deliver the report\n  status: in_progress\n  next_action: Draft the report\n  acceptance: [Review the report]\n- id: OTHER\n  title: Other work\n  status: ready\n  next_action: Separate work\n  acceptance: [Other work is done]\n";
const RULES: &str =
    "# Common {#common severity=hard scope=project value=*}\n\nKeep exact ground truth.\n";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// A project with the given policy file written after `init` created `.awr`.
    fn new(policy: Option<&str>) -> Self {
        let f = Self {
            root: std::env::temp_dir().join(format!("awr-claim-policy-cli-{}", Id::new())),
        };
        fs::create_dir(&f.root).unwrap();
        fs::write(f.root.join("work.yaml"), WORK).unwrap();
        fs::write(f.root.join("rules.md"), RULES).unwrap();
        fs::write(
            f.root.join("goal.md"),
            "# Deliver the report\n\nKeep the work resumable.\n",
        )
        .unwrap();
        fs::write(f.root.join("sources.toml"), "[project]\nname='Claim policy'\nexternal_key='claim-policy'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n").unwrap();
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        if let Some(text) = policy {
            fs::write(f.root.join(".awr/policy.toml"), text).unwrap();
        }
        f
    }
    fn run_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_awr"));
        command
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .env_remove("AWR_DEFAULT_CLAIM_TTL");
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().unwrap()
    }
    fn run(&self, args: &[&str]) -> Output {
        self.run_with(args, &[])
    }
    fn ok(&self, args: &[&str]) -> Value {
        self.ok_with(args, &[])
    }
    fn ok_with(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let out = self.run_with(args, env);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn refused(&self, out: &Output, code: &str) -> String {
        assert!(
            !out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], code, "{error}");
        error["message"].as_str().unwrap_or_default().to_string()
    }
    fn revision(&self) -> String {
        self.ok(&["proposal", "list"])["project_revision"].to_string()
    }
    /// A session on `work` that asks for a claim with the given extra arguments.
    fn start_with(&self, work: &str, agent: &str, extra: &[&str], env: &[(&str, &str)]) -> Value {
        let revision = self.revision();
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
            "--claim",
        ];
        args.extend(extra);
        args.extend(["--expected-revision", &revision]);
        self.ok_with(&args, env)
    }
    fn start(&self, work: &str, agent: &str, extra: &[&str]) -> Value {
        self.start_with(work, agent, extra, &[])
    }
    fn sql(&self, statement: &str, params: impl rusqlite::Params) {
        rusqlite::Connection::open(self.root.join(".awr/state.db"))
            .unwrap()
            .execute(statement, params)
            .unwrap();
    }
    fn claim(&self, claim: &str) -> (String, Option<i64>) {
        rusqlite::Connection::open(self.root.join(".awr/state.db"))
            .unwrap()
            .query_row(
                "SELECT status,expires_at FROM claims WHERE id=?1",
                [claim],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }
    fn expiry(&self, claim: &str) -> Option<i64> {
        self.claim(claim).1
    }
    /// Make the claim run out `ms` from now.
    fn runs_out_in(&self, claim: &str, ms: i64) {
        self.sql(
            "UPDATE claims SET expires_at=?1 WHERE id=?2",
            rusqlite::params![now_millis().unwrap() + ms, claim],
        );
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
    fn checkpoint(&self, session: &str) -> Value {
        // The hash is only recorded as the caller's claim of what it last read; it is not verified.
        self.ok(&[
            "session",
            "checkpoint",
            "--session",
            session,
            "--context-hash",
            &"a".repeat(64),
            "--digest",
            "Progress so far",
            "--next-action",
            "Go on",
            "--expected-revision",
            &self.revision(),
        ])
    }
    fn act(&self, action: &str, session: &str, extra: &[&str]) -> Output {
        let revision = self.revision();
        let mut args = vec![
            "work",
            action,
            "W",
            "--session",
            session,
            "--reason",
            "Recorded work",
            "--expected-revision",
            &revision,
        ];
        args.extend(extra);
        self.run(&args)
    }
    fn acted(&self, action: &str, session: &str, extra: &[&str]) -> Value {
        let out = self.act(action, session, extra);
        assert!(
            out.status.success(),
            "{action}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// `actual` is about `from_now` milliseconds from now (the command took some time to run).
fn about(actual: Option<i64>, from_now: i64) -> bool {
    actual.is_some_and(|at| (at - now_millis().unwrap() - from_now).abs() < 2 * MINUTE)
}

#[test]
fn a_policy_gives_a_claim_that_asked_for_no_lifetime_the_default_one() {
    let f = Fixture::new(Some(POLICY));
    let started = f.start("W", "worker-a", &[]);
    let claim = started["claim"]["id"].as_str().unwrap();
    assert!(
        about(started["claim"]["expires_at"].as_i64(), 4 * HOUR),
        "{started}"
    );
    assert_eq!(f.expiry(claim), started["claim"]["expires_at"].as_i64());
    // `work claim` is the same request by another door.
    let idle = f.ok(&[
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
        "--expected-revision",
        &f.revision(),
    ]);
    let claimed = f.ok(&[
        "work",
        "claim",
        "OTHER",
        "--session",
        idle["session"]["id"].as_str().unwrap(),
        "--expected-revision",
        &f.revision(),
    ]);
    assert!(
        about(claimed["claim"]["expires_at"].as_i64(), 4 * HOUR),
        "{claimed}"
    );
}

#[test]
fn a_lifetime_in_the_request_is_unchanged_and_no_expiry_is_asked_for_by_name() {
    let f = Fixture::new(Some(POLICY));
    let explicit = f.start("W", "worker-a", &["--ttl-ms", "600000"]);
    assert!(
        about(explicit["claim"]["expires_at"].as_i64(), 10 * MINUTE),
        "{explicit}"
    );
    let f = Fixture::new(Some(POLICY));
    let unlimited = f.start("W", "worker-a", &["--no-expiry"]);
    assert!(unlimited["claim"]["expires_at"].is_null(), "{unlimited}");
    let claim = unlimited["claim"]["id"].as_str().unwrap();
    assert_eq!(f.expiry(claim), None);
    // Both at once is a usage error, and no-expiry without a claim is one too.
    let f = Fixture::new(Some(POLICY));
    let revision = f.revision();
    for extra in [
        &["--claim", "--ttl-ms", "600000", "--no-expiry"][..],
        &["--no-expiry"][..],
    ] {
        let mut args = vec![
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "a",
            "--provider",
            "p",
            "--model",
            "m",
            "--expected-revision",
            &revision,
        ];
        args.extend(extra);
        let out = f.run(&args);
        assert_eq!(out.status.code(), Some(2), "{extra:?}");
    }
    // `work claim` has the same pair of options.
    let idle = f.ok(&[
        "session",
        "start",
        "--work",
        "OTHER",
        "--agent",
        "b",
        "--provider",
        "p",
        "--model",
        "m",
        "--expected-revision",
        &revision,
    ]);
    let session = idle["session"]["id"].as_str().unwrap();
    let revision = f.revision();
    let both = f.run(&[
        "work",
        "claim",
        "OTHER",
        "--session",
        session,
        "--ttl-ms",
        "600000",
        "--no-expiry",
        "--expected-revision",
        &revision,
    ]);
    assert_eq!(both.status.code(), Some(2));
    let claimed = f.ok(&[
        "work",
        "claim",
        "OTHER",
        "--session",
        session,
        "--no-expiry",
        "--expected-revision",
        &revision,
    ]);
    assert!(claimed["claim"]["expires_at"].is_null());
}

#[test]
fn without_a_policy_nothing_changes_and_the_variable_alone_is_a_policy() {
    let f = Fixture::new(None);
    let plain = f.start("W", "worker-a", &[]);
    assert!(plain["claim"]["expires_at"].is_null(), "{plain}");
    let f = Fixture::new(None);
    let from_variable = f.start_with("W", "worker-a", &[], &[("AWR_DEFAULT_CLAIM_TTL", "30m")]);
    assert!(
        about(from_variable["claim"]["expires_at"].as_i64(), 30 * MINUTE),
        "{from_variable}"
    );
    // The variable wins over the file, and `none` turns the default off.
    let f = Fixture::new(Some(POLICY));
    let off = f.start_with("W", "worker-a", &[], &[("AWR_DEFAULT_CLAIM_TTL", "none")]);
    assert!(off["claim"]["expires_at"].is_null(), "{off}");
    // A value that is not a duration is refused by name, never echoed.
    let f = Fixture::new(None);
    let revision = f.revision();
    let out = f.run_with(
        &[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "a",
            "--provider",
            "p",
            "--model",
            "m",
            "--claim",
            "--expected-revision",
            &revision,
        ],
        &[("AWR_DEFAULT_CLAIM_TTL", "S3CRET")],
    );
    let message = f.refused(&out, "InvalidInput");
    assert!(
        message.contains("AWR_DEFAULT_CLAIM_TTL") && !message.contains("S3CRET"),
        "{message}"
    );
}

#[test]
fn a_policy_this_build_cannot_read_stops_only_the_request_that_asks_for_the_default() {
    let f = Fixture::new(Some("version = 2\n"));
    let revision = f.revision();
    let start = |extra: &[&str]| {
        let mut args = vec![
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "a",
            "--provider",
            "p",
            "--model",
            "m",
            "--expected-revision",
            &revision,
        ];
        args.extend(extra);
        f.run(&args)
    };
    let message = f.refused(&start(&["--claim"]), "Unsupported");
    assert!(message.contains("newer awr"), "{message}");
    // A session that asks for no claim does not consult the policy at all.
    let plain = start(&[]);
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    // An explicit lifetime, or none, works whatever state the file is in.
    let f = Fixture::new(Some("version = 2\n"));
    assert!(f.start("W", "a", &["--ttl-ms", "600000"])["claim"]["id"].is_string());
    let f = Fixture::new(Some("version = 2\n"));
    assert!(f.start("W", "a", &["--no-expiry"])["claim"]["id"].is_string());
}

#[test]
fn resume_that_acquires_gets_the_default_and_resume_that_inherits_changes_nothing() {
    let f = Fixture::new(Some(POLICY));
    let old = f.start("W", "sender", &["--ttl-ms", "3600000"]);
    let sid = old["session"]["id"].as_str().unwrap();
    f.checkpoint(sid);
    let resume = |from: &str, extra: &[&str]| {
        let revision = f.revision();
        let mut args = vec![
            "session",
            "resume",
            "--from-session",
            from,
            "--agent",
            "receiver",
            "--provider",
            "target",
            "--model",
            "target",
            "--expected-revision",
            &revision,
        ];
        args.extend(extra);
        f.ok(&args)
    };
    // Inheriting keeps the expiry the predecessor's claim has; the policy is not applied to it.
    let held = now_millis().unwrap() + HOUR;
    f.sql(
        "UPDATE claims SET expires_at=?1 WHERE id=?2",
        rusqlite::params![held, old["claim"]["id"].as_str().unwrap()],
    );
    let inherited = resume(sid, &[]);
    assert_eq!(
        inherited["resumed"]["claim"]["expires_at"], held,
        "{inherited}"
    );
    // Acquiring is a new request for a claim, and a request that gives no lifetime gets the default.
    let next = inherited["resumed"]["session"]["id"].as_str().unwrap();
    let acquired = resume(next, &["--claim"]);
    assert!(
        about(
            acquired["resumed"]["claim"]["expires_at"].as_i64(),
            4 * HOUR
        ),
        "{acquired}"
    );
    let next = acquired["resumed"]["session"]["id"].as_str().unwrap();
    let unlimited = resume(next, &["--claim", "--no-expiry"]);
    assert!(
        unlimited["resumed"]["claim"]["expires_at"].is_null(),
        "{unlimited}"
    );
}

#[test]
fn a_checkpoint_extends_the_live_claim_in_its_own_revision_and_the_event_says_so() {
    let f = Fixture::new(Some(POLICY));
    let started = f.start("W", "worker-a", &["--ttl-ms", "600000"]);
    let sid = started["session"]["id"].as_str().unwrap();
    let claim = started["claim"]["id"].as_str().unwrap();
    let before = f.expiry(claim).unwrap();
    let saved = f.checkpoint(sid);
    assert!(about(f.expiry(claim), 4 * HOUR));
    assert!(f.expiry(claim).unwrap() > before);
    // The revision the reply carries is the live one, so the next write needs no extra read.
    assert_eq!(saved["project_revision"].to_string(), f.revision());
    let receipts = f.events("checkpoint.created");
    let receipt = &receipts.last().unwrap()["payload"]["claim_renewal"];
    assert_eq!(receipt["renewed"][0]["claim_id"], claim, "{receipt}");
    assert_eq!(receipt["renewed"][0]["previous_expires_at"], before);
    assert_eq!(receipt["ttl_ms"], 4 * HOUR);
}

#[test]
fn progress_block_and_unblock_extend_the_claim_and_the_reply_revision_stays_live() {
    let f = Fixture::new(Some(POLICY));
    let started = f.start("W", "worker-a", &["--ttl-ms", "600000"]);
    let sid = started["session"]["id"].as_str().unwrap();
    let claim = started["claim"]["id"].as_str().unwrap();
    for (action, extra, event) in [
        (
            "progress",
            &["--next-action", "Draft it"][..],
            "work.progressed",
        ),
        (
            "block",
            &["--blocker", "Waiting for input"][..],
            "work.blocked",
        ),
        ("unblock", &["--next-action", "Go on"][..], "work.unblocked"),
    ] {
        f.runs_out_in(claim, 10 * MINUTE);
        let reply = f.acted(action, sid, extra);
        assert!(about(f.expiry(claim), 4 * HOUR), "{action}: {reply}");
        assert_eq!(
            reply["project_revision"].to_string(),
            f.revision(),
            "{action}"
        );
        let receipts = f.events(event);
        let receipt = &receipts.last().unwrap()["payload"]["claim_renewal"];
        assert_eq!(
            receipt["renewed"][0]["claim_id"], claim,
            "{action}: {receipt}"
        );
    }
}

#[test]
fn renewal_can_be_switched_off_and_a_project_without_a_policy_renews_nothing() {
    let off = "version = 1\n[claims]\ndefault_ttl = \"4h\"\nrenew_on_activity = false\n";
    for policy in [Some(off), None] {
        let f = Fixture::new(policy);
        let started = f.start("W", "worker-a", &["--ttl-ms", "600000"]);
        let sid = started["session"]["id"].as_str().unwrap();
        let claim = started["claim"]["id"].as_str().unwrap();
        let before = f.expiry(claim);
        f.checkpoint(sid);
        f.acted("progress", sid, &["--next-action", "Draft it"]);
        assert_eq!(f.expiry(claim), before, "{policy:?}");
        assert!(
            f.events("checkpoint.created").last().unwrap()["payload"]
                .get("claim_renewal")
                .is_none()
        );
    }
}

#[test]
fn a_claim_that_ran_out_is_not_revived_by_work_and_the_work_that_needs_it_is_refused() {
    let f = Fixture::new(Some(POLICY));
    let started = f.start("W", "worker-a", &["--ttl-ms", "600000"]);
    let sid = started["session"]["id"].as_str().unwrap();
    let claim = started["claim"]["id"].as_str().unwrap();
    let ended = now_millis().unwrap() - 1;
    f.sql(
        "UPDATE claims SET expires_at=?1 WHERE id=?2",
        rusqlite::params![ended, claim],
    );
    // A recorded progress needs a claim that is live; it is refused, and the claim stays out.
    let refused = f.act("progress", sid, &["--next-action", "Draft it"]);
    assert!(!refused.status.success());
    assert_eq!(f.expiry(claim), Some(ended));
    // A checkpoint is recorded all the same; it says that the claim ran out and does not revive it.
    f.checkpoint(sid);
    assert_eq!(f.claim(claim), ("expired".to_string(), Some(ended)));
    let receipts = f.events("checkpoint.created");
    let receipt = &receipts.last().unwrap()["payload"]["claim_renewal"];
    assert_eq!(receipt["renewed"], serde_json::json!([]));
    assert_eq!(receipt["expired_claim_ids"], serde_json::json!([claim]));
}

#[test]
fn work_can_be_claimed_again_after_a_claim_ran_out_and_the_old_session_cannot_extend_the_new_claim()
{
    let f = Fixture::new(Some(POLICY));
    let old = f.start("W", "worker-a", &["--ttl-ms", "600000"]);
    let old_session = old["session"]["id"].as_str().unwrap();
    let old_claim = old["claim"]["id"].as_str().unwrap();
    f.sql(
        "UPDATE claims SET expires_at=?1 WHERE id=?2",
        rusqlite::params![now_millis().unwrap() - 1, old_claim],
    );
    // The work is free again, and the new claim takes the project's default lifetime.
    let new = f.start("W", "worker-b", &[]);
    let new_claim = new["claim"]["id"].as_str().unwrap();
    assert!(
        about(new["claim"]["expires_at"].as_i64(), 4 * HOUR),
        "{new}"
    );
    assert_eq!(f.claim(old_claim).0, "expired");
    // What the old session does afterwards neither revives its claim nor touches the one that replaced it.
    f.runs_out_in(new_claim, 10 * MINUTE);
    let held = f.expiry(new_claim);
    f.checkpoint(old_session);
    assert_eq!(f.claim(old_claim).0, "expired");
    assert_eq!(f.expiry(new_claim), held);
    assert_eq!(f.claim(new_claim).0, "active");
}

#[test]
fn a_policy_that_cannot_be_read_does_not_fail_the_work_it_would_have_renewed() {
    let f = Fixture::new(None);
    let started = f.start("W", "worker-a", &["--ttl-ms", "600000"]);
    let sid = started["session"]["id"].as_str().unwrap();
    let claim = started["claim"]["id"].as_str().unwrap();
    let before = f.expiry(claim);
    fs::write(f.root.join(".awr/policy.toml"), "version = 2\n").unwrap();
    f.checkpoint(sid);
    f.acted("progress", sid, &["--next-action", "Draft it"]);
    assert_eq!(f.expiry(claim), before);
}

#[test]
fn doctor_warns_of_a_claim_that_has_had_no_expiry_for_a_day_and_of_a_policy_it_cannot_read() {
    let f = Fixture::new(None);
    let old = f.start("W", "worker-a", &[]);
    let young = f.start("OTHER", "worker-b", &[]);
    let (old_claim, young_claim) = (
        old["claim"]["id"].as_str().unwrap(),
        young["claim"]["id"].as_str().unwrap(),
    );
    f.sql(
        "UPDATE claims SET acquired_at=?1 WHERE id=?2",
        rusqlite::params![now_millis().unwrap() - 25 * HOUR, old_claim],
    );
    // The report is read whatever the exit code says; the exit code is part of what is asserted.
    let report = |f: &Fixture| -> (Output, Value) {
        let out = f.run(&["doctor"]);
        let value = serde_json::from_slice(&out.stdout).unwrap();
        (out, value)
    };
    let findings = |f: &Fixture, code: &str| -> Vec<Value> {
        report(f).1["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|finding| finding["code"] == code)
            .cloned()
            .collect()
    };
    let warned = findings(&f, "claim_without_expiry");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0]["object_id"], old_claim);
    assert_eq!(warned[0]["severity"], "warning");
    assert!(warned[0]["repair"].is_null());
    let message = warned[0]["message"].as_str().unwrap();
    assert!(
        message.contains("policy.toml") || message.contains("AWR_DEFAULT_CLAIM_TTL"),
        "{message}"
    );
    assert!(warned.iter().all(|w| w["object_id"] != young_claim));
    // It advises: a project that was healthy for the exit code stays so.
    let (out, value) = report(&f);
    assert!(out.status.success(), "{value}");
    assert_eq!(value["ok"], true);
    // A claim with an expiry is not that, however old, and neither is one that was released.
    f.sql(
        "UPDATE claims SET expires_at=?1 WHERE id=?2",
        rusqlite::params![now_millis().unwrap() + HOUR, old_claim],
    );
    assert!(findings(&f, "claim_without_expiry").is_empty());

    // A policy this build cannot read is reported as a fault of the project.
    assert!(findings(&f, "claim_policy_unreadable").is_empty());
    fs::write(f.root.join(".awr/policy.toml"), "version = 2\n").unwrap();
    let unreadable = findings(&f, "claim_policy_unreadable");
    assert_eq!(unreadable.len(), 1, "{unreadable:?}");
    assert_eq!(unreadable[0]["severity"], "error");
    assert!(!report(&f).0.status.success());
    assert!(
        unreadable[0]["message"]
            .as_str()
            .unwrap()
            .contains("newer awr")
    );
}

#[test]
fn capabilities_expose_the_operating_baseline_to_a_host_that_requires_it() {
    let f = Fixture::new(None);
    let report = f.ok(&["capabilities", "--require", "operating_baseline"]);
    let entry = report["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "operating_baseline")
        .unwrap_or_else(|| panic!("{report}"));
    assert_eq!(entry["available"], true);
    let features = entry["limitations"].to_string();
    for name in [
        "policy_file_v1",
        "default_claim_ttl",
        "renew_on_activity",
        "explicit_no_expiry",
    ] {
        assert!(features.contains(name), "{name}: {features}");
    }
}
