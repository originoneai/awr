use awr_core::*;
use awr_store::{BranchFilter, EventQuery, Store};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
const WORK: &str = "work_items:\n- id: W\n  title: Deliver branch-aware work\n  status: in_progress\n  owner: business-owner\n  next_action: Continue the selected work\n  acceptance: [Preserve work and branch ownership]\n";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let f = Self(std::env::temp_dir().join(format!("awr-branch-{}", Id::new())));
        fs::create_dir_all(f.0.join("decisions")).unwrap();
        fs::write(f.0.join("work.yaml"), WORK).unwrap();
        fs::write(f.0.join("rules.md"),"# Authority {#authority severity=hard scope=project value=*}\n\nPreserve shared source facts exactly.\n").unwrap();
        fs::write(
            f.0.join("goal.md"),
            "# Deliver durable work\n\nRetain the requested work and its results.\n",
        )
        .unwrap();
        fs::write(f.0.join("decisions/adr.md"),"---\naffected_keys: [W]\n---\n\n# Store work history\n\nStatus: accepted\n\n## Decision\n\nRecord work history separately from source facts.\n").unwrap();
        fs::write(f.0.join("sources.toml"),"[project]\nname='Branch fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n").unwrap();
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.0)
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
            "{}",
            String::from_utf8_lossy(&r.stderr)
        );
    }
    fn revision(&self) -> String {
        self.ok(&["branch", "list"])["project_revision"].to_string()
    }
    fn create(&self, name: &str, extra: &[&str]) -> Output {
        let rev = self.revision();
        let mut args = vec![
            "branch",
            "create",
            name,
            "--actor",
            "developer",
            "--reason",
            "Explore the selected work",
            "--expected-revision",
            &rev,
        ];
        args.extend_from_slice(extra);
        self.run(&args)
    }
    fn switch(&self, name: &str) -> Output {
        self.run(&[
            "branch",
            "switch",
            name,
            "--actor",
            "developer",
            "--reason",
            "Continue on the selected work branch",
            "--expected-revision",
            &self.revision(),
        ])
    }
    fn start(&self, agent: &str, claim: bool) -> Value {
        let rev = self.revision();
        let mut args = vec![
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            agent,
            "--provider",
            "fixture",
            "--model",
            "local",
            "--expected-revision",
            &rev,
        ];
        if claim {
            args.push("--claim");
        }
        self.ok(&args)
    }
    fn store(&self) -> (Store, Id) {
        let s = Store::open_existing(&self.0.join(".awr/state.db")).unwrap();
        let p = s
            .project_by_root(&self.0.canonicalize().unwrap())
            .unwrap()
            .id;
        (s, p)
    }
    fn event(&self, session: &str, summary: &str) -> Event {
        let (mut s, p) = self.store();
        let mut e = EventDraft::new("work.observed", summary);
        e.importance = "critical".into();
        e.session_id = Some(session.parse().unwrap());
        let r = s.project(p).unwrap().project_revision;
        s.append_event(p, r, e).unwrap()
    }
    fn evidence(&self, key: &str, branch: Option<Option<Id>>) -> Value {
        let mut draft = json!({"external_key":key,"work_item_key":"W","evidence_type":"report","level":"implemented","summary":format!("Evidence for {key}"),"locator":"report.json","source_sha":"a".repeat(40),"scope":["W"]});
        if let Some(branch) = branch {
            draft["branch_id"] = json!(branch);
        }
        let path = self.0.join("evidence.json");
        fs::write(&path, serde_json::to_vec(&draft).unwrap()).unwrap();
        self.ok(&[
            "evidence",
            "add",
            "--input",
            path.to_str().unwrap(),
            "--expected-revision",
            &self.revision(),
        ])
    }
    fn git(&self, args: &[&str]) -> String {
        let r = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        String::from_utf8(r.stdout).unwrap().trim().into()
    }
    fn init_git(&self) {
        self.git(&["init", "-b", "main"]);
        self.git(&["config", "user.name", "Fixture"]);
        self.git(&["config", "user.email", "fixture@example.invalid"]);
        self.git(&["config", "commit.gpgsign", "false"]);
        self.git(&["add", "work.yaml"]);
        self.git(&["commit", "-m", "Fixture source"]);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn completion_preflight_round_trips_without_rewriting_evidence_or_losing_its_branch() {
    for selection in [
        "current_main",
        "current_branch",
        "named_branch",
        "branch_id",
        "explicit_main",
    ] {
        let f = Fixture::new();
        let branch = Fixture::success(&f.create("analysis", &[]));
        let bid = branch["branch"]["id"].as_str().unwrap();
        Fixture::success(&f.create("other", &[]));
        if matches!(selection, "current_branch" | "explicit_main") {
            Fixture::success(&f.switch("analysis"));
        }
        let expected_branch = if matches!(selection, "current_main" | "explicit_main") {
            Value::Null
        } else {
            json!(bid)
        };
        let sha = "a".repeat(40);
        let report = json!({"version":1,"work_item":"W","source_sha":sha,
            "command":"inspect fixture","scope":["W"],"verified_at":1,
            "checks":[{"name":"review","passed":true,"details":"fixture result",
                "criteria":["Preserve work and branch ownership"]}]});
        fs::write(f.0.join("report.json"), report.to_string()).unwrap();
        let revision = f.revision();
        let source = fs::read(f.0.join("work.yaml")).unwrap();
        let mut args = vec![
            "work",
            "prepare-completion",
            "W",
            "--report",
            "report.json",
            "--evidence-key",
            "E",
            "--source-sha",
            &sha,
        ];
        match selection {
            "named_branch" => args.extend_from_slice(&["--branch", "analysis"]),
            "branch_id" => args.extend_from_slice(&["--branch", bid]),
            "explicit_main" => args.extend_from_slice(&["--branch", "main"]),
            _ => {}
        }
        let prepared = f.ok(&args);
        assert_eq!(prepared["verification_executed"], false);
        assert_eq!(prepared["completion_claimed"], false);
        assert_eq!(f.revision(), revision);
        assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), source);

        // Change the default after preflight: the unchanged draft must still
        // record on the branch against which the report was validated.
        Fixture::success(&f.switch("other"));
        fs::write(f.0.join("evidence.json"), prepared["evidence"].to_string()).unwrap();
        let recorded = f.ok(&[
            "evidence",
            "add",
            "--input",
            f.0.join("evidence.json").to_str().unwrap(),
            "--expected-revision",
            &f.revision(),
        ]);
        let draft = &prepared["evidence"];
        assert!(draft.get("work").is_none() && draft.get("branch").is_none());
        assert_eq!(draft["work_item_key"], "W");
        assert_eq!(draft.get("branch_id"), Some(&expected_branch));
        assert_eq!(recorded["evidence"]["branch_id"], expected_branch);
        let (store, project) = f.store();
        assert_eq!(
            recorded["evidence"]["work_item_id"],
            json!(store.work_item(project, "W").unwrap().item.meta.id)
        );
        drop(store);

        Fixture::success(&f.switch(if expected_branch.is_null() {
            "main"
        } else {
            "analysis"
        }));
        let session = f.start("executor", true);
        fs::write(
            f.0.join("completion.json"),
            prepared["completion"].to_string(),
        )
        .unwrap();
        let completed = f.ok(&[
            "work",
            "complete",
            "W",
            "--session",
            session["session"]["id"].as_str().unwrap(),
            "--reason",
            "Deliver the verified branch result",
            "--input",
            f.0.join("completion.json").to_str().unwrap(),
            "--expected-revision",
            &f.revision(),
        ]);
        assert_eq!(completed["write_outcome"], "applied", "{selection}");
        assert_eq!(
            f.ok(&["work", "show", "W"])["work"]["status"],
            "completed",
            "{selection}"
        );
    }
}

#[test]
fn named_context_combines_current_shared_sources_with_fork_delta_without_switching() {
    let f = Fixture::new();
    f.init_git();
    let main = f.start("executor", true);
    let ms = main["session"]["id"].as_str().unwrap();
    f.event(ms, "MAIN_RUNTIME_SENTINEL");
    f.evidence("main-report", None);
    let a = Fixture::success(&f.create("analysis", &["--git-ref", "main"]));
    let aid = a["branch"]["id"].as_str().unwrap();
    Fixture::success(&f.switch("analysis"));
    let session = f.start("executor", false);
    let sid = session["session"]["id"].as_str().unwrap();
    f.event(sid, "Branch finding before checkpoint");
    f.evidence("analysis-report", None);
    let pack = f.ok(&["context", "compile", "--session", sid]);
    let cp = f.ok(&[
        "session",
        "checkpoint",
        "--session",
        sid,
        "--context-hash",
        pack["work_context"]["context_hash"].as_str().unwrap(),
        "--digest",
        "Read the branch inputs",
        "--next-action",
        "Review the updated branch input",
        "--open-loop",
        "Retain this branch open loop",
        "--expected-revision",
        &f.revision(),
    ]);
    f.event(sid, "Branch finding after checkpoint");
    let child = Fixture::success(&f.create("child", &[]));
    Fixture::success(&f.switch("child"));
    let sibling_session = f.start("executor", false);
    let ss = sibling_session["session"]["id"].as_str().unwrap();
    f.event(ss, "CHILD_RUNTIME_SENTINEL");
    f.evidence("child-report", None);
    f.event(ms, "MAIN_LATE_SENTINEL");
    let fresh_work = WORK.replace(
        "Continue the selected work",
        "Use the latest shared source after fork",
    );
    fs::write(f.0.join("work.yaml"), &fresh_work).unwrap();
    fs::write(f.0.join("rules.md"), "# Authority {#authority severity=hard scope=project value=*}\n\nLatest shared hard rule after fork.\n").unwrap();
    let git_before = f.git(&["status", "--porcelain=v1"]);
    let source_before = fs::read(f.0.join("work.yaml")).unwrap();
    let sessions_before = [ms, sid, ss].map(|id| f.ok(&["session", "show", id])["session"].clone());
    let report = f.ok(&["branch", "context", "analysis", "--work", "W"]);
    assert_eq!(report["session_id"], sid);
    assert_eq!(report["checkpoint_id"], cp["checkpoint"]["id"]);
    assert_eq!(
        report["delta_after_revision"],
        a["branch"]["fork_project_revision"]
    );
    assert_eq!(report["completeness"]["complete"], true);
    let binding = &report["completeness"]["branch_context"];
    assert_eq!(binding["branch_id"], aid);
    assert_eq!(binding["branch_revision"], a["branch"]["revision"]);
    assert_eq!(binding["git_binding"], a["git_binding"]);
    let text = report["work_context"]["rendered_context"].as_str().unwrap();
    for fact in [
        "Use the latest shared source after fork",
        "Latest shared hard rule after fork.",
        "Branch finding before checkpoint",
        "Branch finding after checkpoint",
        "Retain this branch open loop",
        "analysis-report",
        "Preserve work and branch ownership",
    ] {
        assert!(text.contains(fact), "missing {fact}");
    }
    let all = report.to_string();
    for excluded in [
        "MAIN_RUNTIME_SENTINEL",
        "MAIN_LATE_SENTINEL",
        "CHILD_RUNTIME_SENTINEL",
        "main-report",
        "child-report",
    ] {
        assert!(
            !all.contains(excluded),
            "foreign runtime record: {excluded}"
        );
    }
    assert_eq!(
        f.ok(&["branch", "show"])["branch_id"],
        child["branch"]["id"]
    );
    assert_eq!(
        sessions_before,
        [ms, sid, ss].map(|id| f.ok(&["session", "show", id])["session"].clone())
    );
    assert_eq!(source_before, fs::read(f.0.join("work.yaml")).unwrap());
    assert_eq!(git_before, f.git(&["status", "--porcelain=v1"]));
    assert_eq!(report, f.ok(&["branch", "context", aid, "--work", "W"]));
    // This child has a parent, but inherits shared source facts only, never parent runtime.
    let child_context = f.ok(&["branch", "context", "child", "--work", "W"]);
    assert_eq!(
        child_context["completeness"]["branch_context"]["parent_branch_id"],
        aid
    );
    assert!(!child_context.to_string().contains("Branch finding"));
    assert!(!child_context.to_string().contains("analysis-report"));
    assert!(
        child_context
            .to_string()
            .contains("Latest shared hard rule after fork.")
    );
}

#[test]
fn named_main_and_branch_context_hashes_bind_branch_and_latest_source_versions() {
    let f = Fixture::new();
    f.init_git();
    let main = f.start("main-agent", false);
    let ms = main["session"]["id"].as_str().unwrap();
    f.event(ms, "Main-only finding");
    let a = Fixture::success(&f.create("experiment", &["--git-ref", "main"]));
    let aid = a["branch"]["id"].as_str().unwrap();
    Fixture::success(&f.switch("experiment"));
    let before = f.ok(&["branch", "context", "experiment", "--work", "W"]);
    assert!(before["session_id"].is_null()); // Never borrow the main session.
    assert!(!before.to_string().contains("Main-only finding"));
    let main_pack = f.ok(&["branch", "context", "main", "--work", "W"]);
    assert_eq!(main_pack["session_id"], ms);
    assert!(main_pack["completeness"]["branch_id"].is_null());
    assert!(main_pack.to_string().contains("Main-only finding"));
    assert_ne!(
        main_pack["work_context"]["context_hash"],
        before["work_context"]["context_hash"]
    );
    assert_eq!(f.ok(&["branch", "show"])["branch_id"], aid);
    fs::write(
        f.0.join("work.yaml"),
        WORK.replace("Continue the selected work", "Updated source action"),
    )
    .unwrap();
    let after = f.ok(&["branch", "context", "experiment", "--work", "W"]);
    assert_ne!(
        before["completeness"]["source_versions"],
        after["completeness"]["source_versions"]
    );
    assert_ne!(
        before["work_context"]["context_hash"],
        after["work_context"]["context_hash"]
    );
    assert_eq!(
        after["work_context"]["identity"]["source_versions"],
        after["completeness"]["source_versions"]
    );
    assert_eq!(
        before["completeness"]["branch_context"],
        after["completeness"]["branch_context"]
    );
    assert_eq!(
        after,
        f.ok(&["branch", "context", "experiment", "--work", "W"])
    );
    // The immutable Git observation does not make a later dirty checkout look like fork source.
    assert_eq!(
        after["completeness"]["branch_context"]["git_binding"],
        a["git_binding"]
    );
}

#[test]
fn branch_delta_is_bounded_and_keeps_shared_changes_separate_from_exact_branch_history() {
    let f = Fixture::new();
    let main = f.start("main", false);
    let ms = main["session"]["id"].as_str().unwrap();
    f.event(ms, "FOREIGN_EVENT_BEFORE_FORK");
    let a = Fixture::success(&f.create("delta", &[]));
    Fixture::success(&f.switch("delta"));
    let session = f.start("executor", false);
    let sid = session["session"]["id"].as_str().unwrap();
    for text in [
        "First branch observation",
        "Second branch observation",
        "Third branch observation",
    ] {
        f.event(sid, text);
    }
    Fixture::success(&f.switch("main"));
    f.event(ms, "FOREIGN_EVENT_AFTER_FORK");
    fs::write(
        f.0.join("work.yaml"),
        WORK.replace(
            "Continue the selected work",
            "Shared source observed from main",
        ),
    )
    .unwrap();
    let report = f.ok(&[
        "branch",
        "delta",
        "delta",
        "--work",
        "W",
        "--event-limit",
        "1",
        "--entity-limit",
        "1",
    ]);
    let delta = &report["delta"];
    assert_eq!(delta["baseline_origin"], "branch_fork");
    assert_eq!(
        delta["after_revision"],
        a["branch"]["fork_project_revision"]
    );
    assert_eq!(
        delta["fork_project_revision"],
        a["branch"]["fork_project_revision"]
    );
    assert_eq!(delta["events"]["important_event_count"], 3);
    assert_eq!(delta["events"]["omitted_important_events"], 2);
    assert_eq!(
        delta["events"]["important_events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        delta["events"]["important_events"][0]["branch_id"],
        a["branch"]["id"]
    );
    assert_eq!(
        delta["events"]["source_changes"].as_array().unwrap().len(),
        1
    );
    assert!(!report.to_string().contains("FOREIGN_EVENT"));
    assert!(f.ok(&["branch", "show"])["branch_id"].is_null());
    Fixture::error(
        &f.run(&[
            "branch",
            "delta",
            "delta",
            "--work",
            "W",
            "--event-limit",
            "0",
        ]),
        "InvalidInput",
    );
}

#[test]
fn branch_context_rejects_foreign_sessions_checkpoints_and_pre_fork_windows() {
    let f = Fixture::new();
    let main = f.start("main", false);
    let ms = main["session"]["id"].as_str().unwrap();
    let pack = f.ok(&["context", "compile", "--session", ms]);
    let cp = f.ok(&[
        "session",
        "checkpoint",
        "--session",
        ms,
        "--context-hash",
        pack["work_context"]["context_hash"].as_str().unwrap(),
        "--digest",
        "Main progress",
        "--next-action",
        "MAIN_CHECKPOINT_PRIVATE_NEXT",
        "--expected-revision",
        &f.revision(),
    ]);
    let a = Fixture::success(&f.create("isolated", &[]));
    let aid = a["branch"]["id"].as_str().unwrap();
    let before = f.revision();
    Fixture::error(
        &f.run(&["branch", "context", "isolated", "--session", ms]),
        "InvalidInput",
    );
    Fixture::error(
        &f.run(&["branch", "delta", "isolated", "--session", ms]),
        "InvalidInput",
    );
    Fixture::error(
        &f.run(&[
            "context",
            "compile",
            "--branch",
            aid,
            "--work",
            "W",
            "--checkpoint",
            cp["checkpoint"]["id"].as_str().unwrap(),
        ]),
        "InvalidInput",
    );
    Fixture::error(
        &f.run(&[
            "context",
            "compile",
            "--branch",
            aid,
            "--work",
            "W",
            "--after-revision",
            "0",
        ]),
        "InvalidInput",
    );
    Fixture::error(
        &f.run(&[
            "context",
            "compile",
            "--branch",
            aid,
            "--work",
            "W",
            "--after-revision",
            "999999",
        ]),
        "InvalidInput",
    );
    Fixture::error(
        &f.run(&["branch", "context", "absent", "--work", "W"]),
        "NotFound",
    );
    let other = Fixture::new();
    let foreign = Fixture::success(&other.create("other-project", &[]));
    Fixture::error(
        &f.run(&[
            "branch",
            "context",
            foreign["branch"]["id"].as_str().unwrap(),
            "--work",
            "W",
        ]),
        "NotFound",
    );
    assert_eq!(before, f.revision());
    let allowed = f.ok(&["context", "compile", "--branch", aid, "--work", "W"]);
    assert_eq!(
        allowed["delta_after_revision"],
        a["branch"]["fork_project_revision"]
    );
    assert!(!allowed.to_string().contains("MAIN_CHECKPOINT_PRIVATE_NEXT"));
    assert!(f.ok(&["branch", "show"])["branch_id"].is_null());
}

#[test]
fn named_branch_context_preserves_incomplete_sources_and_refuses_hard_fact_overflow() {
    let f = Fixture::new();
    Fixture::success(&f.create("inspect", &[]));
    fs::remove_file(f.0.join("rules.md")).unwrap();
    let failure = f.run(&["branch", "context", "inspect", "--work", "W"]);
    Fixture::error(&failure, "ContextIncomplete");
    let diagnostic: Value = serde_json::from_slice(&failure.stdout).unwrap();
    assert_eq!(diagnostic["completeness"]["source_fresh"], false);
    assert_eq!(
        diagnostic["completeness"]["branch_context"]["name"],
        "inspect"
    );
    let delta = f.run(&["branch", "delta", "inspect", "--work", "W"]);
    Fixture::error(&delta, "SourceStale");
    assert_eq!(
        serde_json::from_slice::<Value>(&delta.stdout).unwrap()["source_refresh_ok"],
        false
    );
    let missing = f.run(&["branch", "context", "inspect", "--work", "MISSING"]);
    Fixture::error(&missing, "ContextIncomplete");
    let missing: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert!(missing["work_context"].is_null());
    assert_eq!(missing["completeness"]["branch_context"]["name"], "inspect");
    fs::write(
        f.0.join("rules.md"),
        format!(
            "# Authority {{severity=hard scope=project value=*}}\n\n{}REQUIRED_TAIL\n",
            "Required source fact. ".repeat(10000)
        ),
    )
    .unwrap();
    let overflow = f.run(&["branch", "context", "inspect", "--work", "W"]);
    Fixture::error(&overflow, "BudgetExceeded");
    assert!(overflow.stdout.is_empty());
    assert!(
        fs::read_to_string(f.0.join("rules.md"))
            .unwrap()
            .contains("REQUIRED_TAIL")
    );
    assert!(f.ok(&["branch", "show"])["branch_id"].is_null());
}

#[test]
fn closed_or_corrupt_branch_metadata_cannot_compile_a_current_execution_pack() {
    let f = Fixture::new();
    let created = Fixture::success(&f.create("closed", &[]));
    let id = created["branch"]["id"].as_str().unwrap();
    let db = rusqlite::Connection::open(f.0.join(".awr/state.db")).unwrap();
    db.execute("UPDATE branches SET status='abandoned' WHERE id=?1", [id])
        .unwrap();
    Fixture::error(
        &f.run(&["branch", "context", "closed", "--work", "W"]),
        "InvalidTransition",
    );
    Fixture::error(
        &f.run(&["branch", "delta", "closed", "--work", "W"]),
        "InvalidTransition",
    );
    db.execute(
        "UPDATE branches SET status='active',fork_project_revision=999999 WHERE id=?1",
        [id],
    )
    .unwrap();
    assert!(
        !f.run(&["branch", "context", "closed", "--work", "W"])
            .status
            .success()
    );
    assert!(f.ok(&["branch", "show", "main"])["branch_id"].is_null());
}

#[test]
fn creating_branch_records_observed_git_commit_without_changing_checkout_sources_or_selection() {
    let f = Fixture::new();
    f.init_git();
    let sha = f.git(&["rev-parse", "HEAD"]);
    fs::write(
        f.0.join("work.yaml"),
        WORK.replace("Continue the selected work", "Uncommitted user work"),
    )
    .unwrap();
    let source = fs::read(f.0.join("work.yaml")).unwrap();
    let git_before = f.git(&["status", "--porcelain"]);
    let rev = f.revision().parse::<u64>().unwrap();
    let created = Fixture::success(&f.create("experiment", &["--git-ref", "main"]));
    assert_eq!(created["branch"]["fork_project_revision"], rev);
    assert_eq!(created["branch"]["parent_branch_id"], Value::Null);
    assert_eq!(created["branch"]["git_ref"], "main");
    assert_eq!(created["git_binding"]["commit_sha"], sha);
    assert_eq!(created["git_binding"]["resolved_ref"], "refs/heads/main");
    assert_eq!(created["current_branch_id"], Value::Null);
    assert_eq!(created["event"]["event_type"], "branch.created");
    assert_eq!(created["event"]["branch_id"], created["branch"]["id"]);
    assert_eq!(created["project_revision"], rev + 1);
    assert_eq!(f.git(&["status", "--porcelain"]), git_before);
    assert_eq!(f.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), source);
    let shown = f.ok(&["branch", "show", "experiment"]);
    assert_eq!(shown["record"]["git_binding"], created["git_binding"]);
    assert_eq!(shown["record"]["creation_event_id"], created["event"]["id"]);
    f.git(&["add", "work.yaml"]);
    f.git(&["commit", "-m", "Move Git ref after observation"]);
    assert_ne!(f.git(&["rev-parse", "HEAD"]), sha);
    assert_eq!(
        f.ok(&["branch", "show", "experiment"])["record"]["git_binding"]["commit_sha"],
        sha
    );
}

#[test]
fn branching_from_current_or_explicit_main_preserves_lineage_and_bounded_read_queries() {
    let f = Fixture::new();
    let a = Fixture::success(&f.create("a", &[]));
    assert!(a["git_binding"].is_null());
    Fixture::success(&f.switch("a"));
    let fork = f.revision();
    let b = Fixture::success(&f.create("b", &[]));
    assert_eq!(b["branch"]["parent_branch_id"], a["branch"]["id"]);
    assert_eq!(b["branch"]["fork_project_revision"].to_string(), fork);
    let c = Fixture::success(&f.create("c", &["--parent", "main"]));
    assert!(c["branch"]["parent_branch_id"].is_null());
    let rev = f.revision();
    let first = f.ok(&["branch", "list", "--limit", "2"]);
    assert_eq!(first["branches"].as_array().unwrap().len(), 2);
    assert_eq!(first["total"], 3);
    assert_eq!(first["has_more"], true);
    let last = f.ok(&["branch", "list", "--offset", "2", "--limit", "2"]);
    assert_eq!(last["branches"][0]["name"], "c");
    assert_eq!(last["has_more"], false);
    assert_eq!(f.ok(&["branch", "show"])["branch_id"], a["branch"]["id"]);
    assert_eq!(f.revision(), rev);
    Fixture::success(&f.switch("main"));
    assert!(f.ok(&["branch", "show"])["record"].is_null());
    Fixture::error(&f.switch("main"), "InvalidTransition");
    for args in [
        &["branch", "list", "--limit", "0"][..],
        &["branch", "list", "--status", "unknown"][..],
        &["branch", "show", "missing"][..],
    ] {
        assert!(!f.run(args).status.success());
    }
}

#[test]
fn creation_checks_revision_before_git_access_and_failure_preserves_database_and_sources() {
    let f = Fixture::new();
    let source = fs::read(f.0.join("work.yaml")).unwrap();
    let r = f.revision();
    Fixture::error(
        &f.run(&[
            "branch",
            "create",
            "stale",
            "--actor",
            "developer",
            "--reason",
            "Review",
            "--git-ref",
            "missing",
            "--expected-revision",
            "0",
        ]),
        "RevisionConflict",
    );
    Fixture::error(&f.create("no-repo", &["--git-ref", "HEAD"]), "InvalidInput");
    f.init_git();
    Fixture::error(
        &f.create("bad-ref", &["--git-ref", "missing-ref"]),
        "InvalidInput",
    );
    Fixture::error(&f.create("main", &[]), "InvalidInput");
    Fixture::error(&f.create(" ", &[]), "InvalidInput");
    Fixture::error(
        &f.create("bad-parent", &["--parent", "missing"]),
        "NotFound",
    );
    assert_eq!(f.revision(), r);
    assert_eq!(f.ok(&["branch", "list"])["total"], 0);
    assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), source);
}

#[test]
fn git_binding_handles_detached_commits_tags_and_ignores_unrelated_git_environment() {
    let f = Fixture::new();
    f.init_git();
    let sha = f.git(&["rev-parse", "HEAD"]);
    f.git(&["tag", "-a", "candidate", "-m", "Candidate"]);
    let tag = Fixture::success(&f.create("tag-work", &["--git-ref", "candidate"]));
    assert_eq!(tag["git_binding"]["commit_sha"], sha);
    assert_eq!(tag["git_binding"]["resolved_ref"], "refs/tags/candidate");
    f.git(&["checkout", "--detach", "HEAD"]);
    let detached = Fixture::success(&f.create("detached-work", &["--git-ref", "HEAD"]));
    assert_eq!(detached["git_binding"]["commit_sha"], sha);
    assert!(detached["git_binding"]["resolved_ref"].is_null());
    let commit = Fixture::success(&f.create("commit-work", &["--git-ref", &sha]));
    assert_eq!(commit["git_binding"]["commit_sha"], sha);
    assert!(commit["git_binding"]["resolved_ref"].is_null());
    let r = Command::new(env!("CARGO_BIN_EXE_awr"))
        .arg("--project")
        .arg(&f.0)
        .arg("--json")
        .args([
            "branch",
            "create",
            "env-work",
            "--actor",
            "developer",
            "--reason",
            "Read project Git",
            "--git-ref",
            "HEAD",
            "--expected-revision",
            &f.revision(),
        ])
        .env("GIT_DIR", f.0.join("absent.git"))
        .output()
        .unwrap();
    assert_eq!(Fixture::success(&r)["git_binding"]["commit_sha"], sha);
}

#[test]
fn switching_keeps_existing_runtime_records_and_shared_source_claim_guards() {
    let f = Fixture::new();
    let main = f.start("main-worker", true);
    let ms = main["session"]["id"].as_str().unwrap();
    let me = f.event(ms, "MAIN_PROCESS_FACT");
    let branch = Fixture::success(&f.create("parallel", &[]));
    let bid = branch["branch"]["id"].as_str().unwrap();
    let changed = Fixture::success(&f.switch("parallel"));
    assert_eq!(changed["selection"]["retained_active_sessions"], 1);
    assert_eq!(changed["selection"]["retained_active_claims"], 1);
    Fixture::error(
        &f.run(&[
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            "branch-worker",
            "--provider",
            "fixture",
            "--model",
            "local",
            "--claim",
            "--expected-revision",
            &f.revision(),
        ]),
        "ClaimConflict",
    );
    let child = f.start("branch-worker", false);
    let cs = child["session"]["id"].as_str().unwrap();
    assert_eq!(child["session"]["branch_id"], bid);
    assert!(child["claim"].is_null());
    let ce = f.event(cs, "BRANCH_PROCESS_FACT");
    assert_eq!(ce.branch_id, Some(bid.parse().unwrap()));
    assert!(f.event(ms, "ANOTHER_MAIN_FACT").branch_id.is_none());
    assert!(f.ok(&["session", "show", ms])["session"]["branch_id"].is_null());
    let blocked = f.run(&[
        "work",
        "progress",
        "W",
        "--session",
        cs,
        "--reason",
        "Apply source progress",
        "--next-action",
        "Continue shared source work",
        "--expected-revision",
        &f.revision(),
    ]);
    Fixture::error(&blocked, "ClaimConflict");
    f.ok(&[
        "work",
        "release",
        "W",
        "--session",
        ms,
        "--claim",
        main["claim"]["id"].as_str().unwrap(),
        "--expected-revision",
        &f.revision(),
    ]);
    f.ok(&[
        "work",
        "claim",
        "W",
        "--session",
        cs,
        "--expected-revision",
        &f.revision(),
    ]);
    f.ok(&[
        "work",
        "progress",
        "W",
        "--session",
        cs,
        "--reason",
        "Apply after resolving shared occupancy",
        "--next-action",
        "Shared source update",
        "--expected-revision",
        &f.revision(),
    ]);
    Fixture::success(&f.switch("main"));
    assert_eq!(
        f.ok(&["object", "show", "work", "W", "--cached", "--full"])["object"]["next_action"],
        "Shared source update"
    );
    let (s, p) = f.store();
    for (scope, yes, no) in [
        (BranchFilter::Main, me.id, ce.id),
        (BranchFilter::Branch(bid.parse().unwrap()), ce.id, me.id),
    ] {
        let events = s
            .query_events(
                p,
                &EventQuery {
                    branch: scope,
                    limit: 100,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(events.events.iter().any(|e| e.id == yes));
        assert!(!events.events.iter().any(|e| e.id == no));
    }
}

#[test]
fn context_and_evidence_keep_branch_identity_and_explicit_main_evidence_is_preserved() {
    let f = Fixture::new();
    let main = f.start("main-worker", false);
    let ms = main["session"]["id"].as_str().unwrap();
    f.event(ms, "MAIN_ONLY_EVENT");
    let base = f.ok(&[
        "context",
        "compile",
        "--session",
        ms,
        "--after-revision",
        "0",
    ]);
    assert!(base["work_context"]["identity"]["branch_id"].is_null());
    let created = Fixture::success(&f.create("context-work", &[]));
    let bid = created["branch"]["id"].as_str().unwrap();
    Fixture::success(&f.switch("context-work"));
    let child = f.start("branch-worker", false);
    let cs = child["session"]["id"].as_str().unwrap();
    f.event(cs, "BRANCH_ONLY_EVENT");
    let proof = f.evidence("branch-proof", None);
    assert_eq!(proof["evidence"]["branch_id"], bid);
    let explicit_main = f.evidence("main-proof", Some(None));
    assert!(explicit_main["evidence"]["branch_id"].is_null());
    let pack = f.ok(&[
        "context",
        "compile",
        "--session",
        cs,
        "--after-revision",
        &created["branch"]["fork_project_revision"].to_string(),
    ]);
    assert_eq!(pack["completeness"]["complete"], true);
    assert_eq!(pack["work_context"]["identity"]["branch_id"], bid);
    assert_ne!(
        pack["work_context"]["context_hash"],
        base["work_context"]["context_hash"]
    );
    let rendered = pack["work_context"]["rendered_context"].as_str().unwrap();
    assert!(rendered.contains("BRANCH_ONLY_EVENT"));
    assert!(!rendered.contains("MAIN_ONLY_EVENT"));
    Fixture::error(
        &f.run(&["context", "compile", "--session", ms]),
        "InvalidInput",
    );
    Fixture::success(&f.switch("main"));
    let report = f.ok(&[
        "evidence",
        "show",
        "branch-proof",
        "--source-sha",
        &"a".repeat(40),
    ]);
    assert_eq!(report["currency"], "historical");
    assert_eq!(report["evidence"]["branch_id"], bid);
    let (s, p) = f.store();
    let events = s
        .query_events(
            p,
            &EventQuery {
                event_type: Some("evidence.recorded".into()),
                branch: BranchFilter::Branch(bid.parse().unwrap()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(events.events.len(), 1);
}

#[test]
fn event_failures_roll_back_branch_creation_and_current_selection_with_no_fake_receipts() {
    let f = Fixture::new();
    let r = f.revision();
    let db = rusqlite::Connection::open(f.0.join(".awr/state.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_branch_create BEFORE INSERT ON events WHEN NEW.event_type='branch.created' BEGIN SELECT RAISE(ABORT,'injected branch failure'); END;").unwrap();
    Fixture::error(&f.create("a", &[]), "Storage");
    assert_eq!(f.revision(), r);
    assert_eq!(f.ok(&["branch", "list"])["total"], 0);
    db.execute_batch("DROP TRIGGER fail_branch_create").unwrap();
    let branch = Fixture::success(&f.create("a", &[]));
    let r = f.revision();
    db.execute_batch("CREATE TRIGGER fail_branch_switch BEFORE INSERT ON events WHEN NEW.event_type='branch.switched' BEGIN SELECT RAISE(ABORT,'injected switch failure'); END;").unwrap();
    Fixture::error(&f.switch("a"), "Storage");
    assert_eq!(f.revision(), r);
    assert!(f.ok(&["branch", "show"])["branch_id"].is_null());
    let (mut s, p) = f.store();
    for kind in ["branch.created", "branch.switched"] {
        assert!(matches!(
            s.append_event(
                p,
                r.parse().unwrap(),
                EventDraft::new(kind, "Forged branch receipt")
            ),
            Err(Error::InvalidInput(_))
        ));
    }
    assert_eq!(
        s.branch(p, branch["branch"]["id"].as_str().unwrap().parse().unwrap())
            .unwrap()
            .creation_event_id
            .unwrap()
            .to_string(),
        branch["event"]["id"].as_str().unwrap()
    );
}

#[test]
fn closed_or_invalid_branches_cannot_receive_new_work_or_evidence_and_legacy_rows_remain_readable()
{
    let f = Fixture::new();
    let a = Fixture::success(&f.create("a", &[]));
    let aid = a["branch"]["id"].as_str().unwrap();
    Fixture::error(&f.create("a", &[]), "InvalidInput");
    let db = rusqlite::Connection::open(f.0.join(".awr/state.db")).unwrap();
    db.execute("UPDATE branches SET status='abandoned' WHERE id=?1", [aid])
        .unwrap();
    Fixture::error(&f.switch("a"), "NotFound");
    Fixture::error(&f.create("child", &["--parent", "a"]), "NotFound");
    assert_eq!(
        f.ok(&["branch", "list", "--status", "abandoned"])["total"],
        1
    );
    let (mut s, p) = f.store();
    let r = s.project(p).unwrap().project_revision;
    let mut event = EventDraft::new(
        "work.observed",
        "Cannot append new process events on a closed branch",
    );
    event.branch_id = Some(aid.parse().unwrap());
    assert!(matches!(
        s.append_event(p, r, event),
        Err(Error::NotFound(_))
    ));
    let invalid = Id::new();
    assert!(matches!(
        s.create_branch(
            p,
            r,
            BranchDraft {
                name: "foreign".into(),
                parent_branch_id: Some(invalid),
                git_binding: None,
                actor: "developer".into(),
                reason: "Review".into()
            }
        ),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        s.record_evidence(
            p,
            r,
            EvidenceDraft {
                external_key: "closed-proof".into(),
                work_item_key: Some("W".into()),
                evidence_type: "report".into(),
                level: EvidenceLevel::Implemented,
                summary: "Closed branch record".into(),
                locator: "report.json".into(),
                sha256: None,
                source_sha: None,
                command: None,
                scope: vec!["W".into()],
                branch_id: Some(aid.parse().unwrap()),
                verified_at: None
            }
        ),
        Err(Error::NotFound(_))
    ));
    let legacy = Id::new();
    db.execute("INSERT INTO branches(id,project_id,name,git_ref,fork_project_revision,status,revision) VALUES(?1,?2,'legacy','unverified/ref',0,'active',1)",rusqlite::params![legacy.to_string(),p.to_string()]).unwrap();
    let record = s.branch(p, legacy).unwrap();
    assert_eq!(record.branch.git_ref.as_deref(), Some("unverified/ref"));
    assert!(record.git_binding.is_none() && record.creation_event_id.is_none());
    db.execute(
        "UPDATE branches SET fork_project_revision=?1 WHERE id=?2",
        rusqlite::params![r as i64 + 100, legacy.to_string()],
    )
    .unwrap();
    Fixture::error(&f.switch("legacy"), "SourceConflict");
}

#[test]
fn name_id_ambiguity_and_cross_project_parent_are_rejected_without_fallback() {
    let f = Fixture::new();
    let a = Fixture::success(&f.create("a", &[]));
    let id = a["branch"]["id"].as_str().unwrap();
    Fixture::success(&f.create(id, &[]));
    Fixture::error(&f.switch(id), "InvalidInput");
    Fixture::success(&f.switch("a"));
    let other = Fixture::new();
    let (mut s, p) = f.store();
    let foreign = s
        .register_project(&other.0, "other-project", "Other project")
        .unwrap();
    let (other_branch, _) = s
        .create_branch(
            foreign.id,
            foreign.project_revision,
            BranchDraft {
                name: "other".into(),
                parent_branch_id: None,
                git_binding: None,
                actor: "developer".into(),
                reason: "Review".into(),
            },
        )
        .unwrap();
    let r = s.project(p).unwrap().project_revision;
    assert!(matches!(
        s.create_branch(
            p,
            r,
            BranchDraft {
                name: "foreign-parent".into(),
                parent_branch_id: Some(other_branch.id),
                git_binding: None,
                actor: "developer".into(),
                reason: "Review".into()
            }
        ),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn branch_reads_and_switches_work_without_git_or_source_access_and_do_not_reindex() {
    let f = Fixture::new();
    let created = Fixture::success(&f.create("offline", &[]));
    let source = f.0.join("work.yaml");
    fs::rename(&source, f.0.join("work-away.yaml")).unwrap();
    fs::rename(
        f.0.join(".awr/project.toml"),
        f.0.join("manifest-away.toml"),
    )
    .unwrap();
    let r = f.revision();
    let shown = f.ok(&["branch", "show", "offline"]);
    assert_eq!(shown["record"]["branch"]["id"], created["branch"]["id"]);
    assert_eq!(f.revision(), r);
    Fixture::success(&f.switch("offline"));
    assert!(!source.exists());
    Fixture::success(&f.switch("main"));
}
