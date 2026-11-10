use awr_context::{ContextRequest, DeltaBaseline, compile_context};
use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const WORK: &str = "work_items:\n- id: W\n  title: Resume current work\n  milestone: M1\n  status: in_progress\n  next_action: Read the original plan\n  acceptance: [Keep the exact acceptance]\n- id: OTHER\n  title: Unrelated work\n  status: ready\n  next_action: Wait for its own agent\n  acceptance: [Keep unrelated work separate]\n";
const RULES: &str = "# Common {#common severity=hard scope=project value=*}\n\nKeep exact ground truth.\n\n# Receiver {#receiver severity=hard scope=agent value=receiver}\n\nRECEIVER_HARD_RULE\n\n# Sender {#sender severity=hard scope=agent value=sender}\n\nSENDER_ONLY_HARD_RULE\n";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-resume-cli-{}", Id::new()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(root.join("rules.md"), RULES).unwrap();
        fs::write(
            root.join("goal.md"),
            "# Preserve work across sessions\n\nResume from current facts.\n",
        )
        .unwrap();
        fs::write(root.join("sources.toml"),"[project]\nname='Resume fixture'\nexternal_key='resume'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n").unwrap();
        let f = Self(root);
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
        let r = self.run(args);
        assert!(
            r.status.success(),
            "{args:?}: {} gaps={}",
            String::from_utf8_lossy(&r.stderr),
            serde_json::from_slice::<Value>(&r.stdout)
                .ok()
                .map(|v| v["context"]["gaps"].clone())
                .unwrap_or(Value::Null)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn revision(&self) -> String {
        self.ok(&["status"])["project_revision"].to_string()
    }
    fn start(&self, agent: &str, ttl: Option<&str>) -> Value {
        let rev = self.revision();
        let mut args = vec![
            "session",
            "start",
            "--work",
            "W",
            "--agent",
            agent,
            "--provider",
            "source-provider",
            "--model",
            "source-model",
            "--expected-revision",
            &rev,
        ];
        if let Some(ttl) = ttl {
            args.extend(["--claim", "--ttl-ms", ttl]);
        }
        self.ok(&args)
    }
    fn checkpoint(&self, sid: &str) -> Value {
        let context = self.ok(&["context", "compile", "--session", sid]);
        self.ok(&[
            "session",
            "checkpoint",
            "--session",
            sid,
            "--context-hash",
            context["work_context"]["context_hash"].as_str().unwrap(),
            "--digest",
            "Implemented the first part",
            "--next-action",
            "SAVED_NEXT_ACTION",
            "--open-loop",
            "SAVED_OPEN_LOOP_ONE",
            "--open-loop",
            "SAVED_OPEN_LOOP_TWO",
            "--expected-revision",
            &self.revision(),
        ])
    }
    fn resume(&self, from: Option<&str>, rev: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            "session",
            "resume",
            "--agent",
            "receiver",
            "--provider",
            "target-provider",
            "--model",
            "target-model",
            "--expected-revision",
            rev,
        ];
        if let Some(id) = from {
            args.extend(["--from-session", id]);
        }
        args.extend(extra);
        self.run(&args)
    }
    fn resumed(&self, from: Option<&str>, extra: &[&str]) -> Value {
        let r = self.resume(from, &self.revision(), extra);
        assert!(
            r.status.success(),
            "{} {}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        serde_json::from_slice(&r.stdout).unwrap()
    }
    fn assert_error(&self, r: Output, code: &str) {
        assert!(!r.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stderr).unwrap()["code"],
            code
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn claim_conflict_rolls_back_successor_creation_and_preserves_other_owners() {
    let f = Fixture::new();
    let old = f.start("sender", None);
    let sid = old["session"]["id"].as_str().unwrap();
    let owner = f.start("owner", Some("3600000"));
    let owner_id = owner["session"]["id"].as_str().unwrap();
    let revision = f.revision();
    f.assert_error(
        f.resume(Some(sid), &revision, &["--claim"]),
        "ClaimConflict",
    );
    assert_eq!(
        f.ok(&["session", "list"])["project_revision"].to_string(),
        revision
    );
    assert_eq!(
        f.ok(&["session", "list", "--active"])["sessions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let old_after = f.ok(&["session", "show", sid]);
    assert_eq!(old_after["session"], old["session"]);
    assert!(old_after["resumed_successor"].is_null());
    assert_eq!(
        f.ok(&["session", "show", owner_id])["claims"][0]["id"],
        owner["claim"]["id"]
    );
    let resumed = f.resumed(Some(sid), &["--no-claim"]);
    assert!(resumed["resumed"]["claim"].is_null());
    let still_owner = f.ok(&["session", "show", owner_id]);
    assert_eq!(still_owner["session"]["status"], "active");
    assert_eq!(still_owner["claims"][0]["status"], "active");
}

#[test]
fn interrupted_work_resumes_with_new_agent_rules_current_sources_and_saved_loops() {
    let f = Fixture::new();
    let old = f.start("sender", Some("3600000"));
    let sid = old["session"]["id"].as_str().unwrap();
    let cp = f.checkpoint(sid);
    let cpid = cp["checkpoint"]["id"].as_str().unwrap();
    let ended = f.ok(&[
        "session",
        "end",
        "--session",
        sid,
        "--outcome",
        "interrupted",
        "--expected-revision",
        &f.revision(),
    ]);
    fs::write(
        f.0.join("work.yaml"),
        WORK.replace("Read the original plan", "Read the revised source plan"),
    )
    .unwrap();
    fs::write(
        f.0.join("rules.md"),
        RULES.replace(
            "Keep exact ground truth.",
            "Keep changed ground truth exactly.",
        ),
    )
    .unwrap();
    f.revision();
    {
        let mut store = Store::open(&f.0.join(".awr/state.db")).unwrap();
        let p = store.project_by_root(&f.0.canonicalize().unwrap()).unwrap();
        let mut event = EventDraft::new("work.progress", "UNRELATED_HISTORY_SENTINEL");
        event.importance = "high".into();
        event.work_item_id = Some(store.work_item(p.id, "OTHER").unwrap().item.meta.id);
        event.payload = json!({"body":"RAW_UNRELATED_PAYLOAD".repeat(10000)});
        store.append_event(p.id, p.project_revision, event).unwrap();
    }
    let source = fs::read(f.0.join("work.yaml")).unwrap();
    let resumed = f.resumed(Some(sid), &["--claim"]);
    let next = &resumed["resumed"]["session"];
    let nid = next["id"].as_str().unwrap();
    assert_ne!(nid, sid);
    assert_eq!(next["agent_id"], "receiver");
    assert_eq!(next["provider"], "target-provider");
    assert_eq!(next["model"], "target-model");
    assert!(next["last_checkpoint_id"].is_null());
    assert_eq!(resumed["resumed"]["from_session"], ended["session"]);
    assert_eq!(resumed["checkpoint_id"], cpid);
    assert_eq!(resumed["checkpoint_save"]["delta_recorded"], true);
    assert_eq!(resumed["context_ready"], true);
    assert!(!resumed["resumed"]["claim"].is_null());
    let context = &resumed["context"];
    let pack = &context["work_context"];
    let text = pack["rendered_context"].as_str().unwrap();
    for fact in [
        "Read the revised source plan",
        "Keep changed ground truth exactly.",
        "Keep the exact acceptance",
        "RECEIVER_HARD_RULE",
        "SAVED_NEXT_ACTION",
        "SAVED_OPEN_LOOP_ONE",
        "SAVED_OPEN_LOOP_TWO",
        "source.projected",
    ] {
        assert!(text.contains(fact), "missing {fact}");
    }
    for hidden in [
        "SENDER_ONLY_HARD_RULE",
        "UNRELATED_HISTORY_SENTINEL",
        "RAW_UNRELATED_PAYLOAD",
    ] {
        assert!(!resumed.to_string().contains(hidden));
    }
    assert_eq!(context["session_id"], nid);
    assert_eq!(context["checkpoint_id"], cpid);
    assert!(pack["token_estimate"].as_u64().unwrap() <= 5000);
    let mut repeated = f.ok(&[
        "context",
        "compile",
        "--work",
        "W",
        "--session",
        nid,
        "--agent",
        "receiver",
        "--intent",
        "resume",
        "--checkpoint",
        cpid,
    ]);
    assert_eq!(
        repeated.as_object_mut().unwrap().remove("budget"),
        Some(json!({"applied": 5000, "basis": "default"}))
    );
    assert_eq!(repeated, *context);
    let shown = f.ok(&["session", "show", nid]);
    assert_eq!(shown["inherited_checkpoint"]["id"], cpid);
    assert!(shown["checkpoint"].is_null());
    assert_eq!(
        f.ok(&["session", "show", sid])["resumed_successor"]["id"],
        nid
    );
    let bootstrap = f.ok(&["context", "bootstrap", "--session", nid, "--budget", "3000"]);
    assert_eq!(
        bootstrap["context"]["checkpoint"]["open_loops"],
        cp["checkpoint"]["open_loops"]
    );
    assert!(
        bootstrap["rendered_context"]
            .as_str()
            .unwrap()
            .contains("RECEIVER_HARD_RULE")
    );
    assert_eq!(fs::read(f.0.join("work.yaml")).unwrap(), source);
    f.assert_error(f.resume(Some(sid), &f.revision(), &[]), "InvalidTransition");
}

#[test]
fn active_resume_inherits_live_claim_and_expired_claims_need_explicit_acquisition() {
    let f = Fixture::new();
    let old = f.start("sender", Some("3600000"));
    let sid = old["session"]["id"].as_str().unwrap();
    f.checkpoint(sid);
    let resumed = f.resumed(None, &["--work", "W"]);
    assert_eq!(resumed["selection_basis"], "active_session");
    assert_eq!(resumed["resumed"]["from_session"]["status"], "interrupted");
    assert_eq!(
        resumed["resumed"]["claim"]["expires_at"],
        old["claim"]["expires_at"]
    );
    assert_ne!(resumed["resumed"]["claim"]["id"], old["claim"]["id"]);
    assert_eq!(
        f.ok(&["session", "show", sid])["claims"][0]["status"],
        "released"
    );

    let f = Fixture::new();
    let old = f.start("sender", Some("1"));
    let sid = old["session"]["id"].as_str().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let resumed = f.resumed(Some(sid), &[]);
    assert_eq!(resumed["recovery_basis"], "source_and_session_start");
    assert!(resumed["checkpoint_id"].is_null());
    assert!(resumed["resumed"]["claim"].is_null());
    assert!(!resumed["recovery_gaps"].as_array().unwrap().is_empty());
    assert_eq!(
        f.ok(&["session", "show", sid])["claims"][0]["status"],
        "expired"
    );
    let reacquired = f.resumed(
        Some(resumed["resumed"]["session"]["id"].as_str().unwrap()),
        &["--claim"],
    );
    assert_eq!(reacquired["resumed"]["claim"]["status"], "active");
}

#[test]
fn checkpointless_resume_keeps_its_recovery_baseline_across_context_reads_and_successors() {
    let f = Fixture::new();
    let old = f.start("sender", None);
    let sid = old["session"]["id"].as_str().unwrap();
    let baseline = &old["session"]["start_project_revision"];
    fs::write(
        f.0.join("work.yaml"),
        WORK.replace("Read the original plan", "Continue after the source change"),
    )
    .unwrap();
    let resumed = f.resumed(Some(sid), &[]);
    let nid = resumed["resumed"]["session"]["id"].as_str().unwrap();
    assert_eq!(&resumed["context"]["delta_after_revision"], baseline);
    let repeated = f.ok(&["context", "compile", "--session", nid]);
    assert_eq!(&repeated["delta_after_revision"], baseline);
    let delta = f.ok(&["context", "delta", "--session", nid]);
    assert_eq!(&delta["delta"]["after_revision"], baseline);
    assert_eq!(delta["delta"]["baseline_origin"], "resumed_session_start");
    assert_eq!(
        delta["delta"]["events"]["source_changes"][0]["changed_entities"][0]["external_key"],
        "W"
    );
    let next = f.resumed(Some(nid), &[]);
    assert_eq!(&next["context"]["delta_after_revision"], baseline);
    let last = f.ok(&[
        "context",
        "compile",
        "--session",
        next["resumed"]["session"]["id"].as_str().unwrap(),
    ]);
    assert_eq!(&last["delta_after_revision"], baseline);
    assert!(
        last["work_context"]["rendered_context"]
            .as_str()
            .unwrap()
            .contains("source.projected")
    );
}

#[test]
fn stale_revisions_missing_sources_and_unknown_target_scope_do_not_create_a_successor() {
    let f = Fixture::new();
    let old = f.start("sender", Some("3600000"));
    let sid = old["session"]["id"].as_str().unwrap();
    f.checkpoint(sid);
    let old_rev = f.revision();
    fs::write(
        f.0.join("work.yaml"),
        WORK.replace("Read the original plan", "Read newer inputs"),
    )
    .unwrap();
    f.assert_error(f.resume(Some(sid), &old_rev, &[]), "RevisionConflict");
    let rev = f.revision();
    fs::remove_file(f.0.join("rules.md")).unwrap();
    f.assert_error(f.resume(Some(sid), &rev, &[]), "SourceStale");
    fs::write(f.0.join("rules.md"),format!("{RULES}\n# Path {{#path severity=hard scope=path value=src/**}}\n\nKeep the selected source path consistent.\n")).unwrap();
    let failed = f.resume(Some(sid), &f.revision(), &[]);
    assert!(!failed.status.success());
    let report: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(report["context_phase"], "preflight");
    assert!(report["resumed"].is_null());
    assert_eq!(report["context"]["completeness"]["rules_complete"], false);
    let shown = f.ok(&["session", "show", sid]);
    assert_eq!(shown["session"]["status"], "active");
    assert!(shown["resumed_successor"].is_null());
    assert_eq!(shown["claims"][0]["status"], "active");
    assert_eq!(
        f.resumed(Some(sid), &["--path", "src/lib.rs"])["context_ready"],
        true
    );

    let f = Fixture::new();
    f.start("sender", None);
    f.start("sender", None);
    f.assert_error(
        f.resume(None, &f.revision(), &["--work", "W"]),
        "InvalidInput",
    );
}

#[test]
fn post_commit_context_overflow_exposes_the_created_session_for_context_retry() {
    for save_checkpoint in [true, false] {
        let f = Fixture::new();
        let old = f.start("sender", None);
        let sid = old["session"]["id"].as_str().unwrap();
        let checkpoint = save_checkpoint.then(|| f.checkpoint(sid));
        let baseline = checkpoint
            .as_ref()
            .map(|cp| cp["checkpoint"]["project_revision"].as_u64().unwrap())
            .unwrap_or_else(|| old["session"]["start_project_revision"].as_u64().unwrap());
        let delta_baseline = checkpoint
            .as_ref()
            .map(|cp| DeltaBaseline::Checkpoint {
                id: cp["checkpoint"]["id"].as_str().unwrap().parse().unwrap(),
            })
            .unwrap_or(DeltaBaseline::Revision { revision: baseline });
        fs::write(
            f.0.join("work.yaml"),
            WORK.replace(
                "Read the original plan",
                "Resume with current source changes",
            ),
        )
        .unwrap();
        let required = {
            let mut store = Store::open(&f.0.join(".awr/state.db")).unwrap();
            let prepared = compile_context(
                &mut store,
                &f.0,
                &ContextRequest {
                    work_item_key: Some("W".into()),
                    agent_id: Some("receiver".into()),
                    detached: true,
                    intent: "resume".into(),
                    delta_baseline,
                    ..Default::default()
                },
            )
            .unwrap();
            prepared.work_context.unwrap().required_tokens.to_string()
        };
        let failed = f.resume(Some(sid), &f.revision(), &["--budget", &required]);
        assert!(!failed.status.success());
        let report: Value = serde_json::from_slice(&failed.stdout).unwrap();
        assert_eq!(report["context_phase"], "resumed_session");
        assert_eq!(report["context_ready"], false);
        assert_eq!(report["context_error"]["code"], "BudgetExceeded");
        let nid = report["resumed"]["session"]["id"].as_str().unwrap();
        assert_eq!(
            f.ok(&["session", "show", nid])["session"]["status"],
            "active"
        );
        let retry = f.ok(&["context", "compile", "--session", nid]);
        assert_eq!(retry["completeness"]["complete"], true);
        assert_eq!(retry["delta_after_revision"], baseline);
        let delta = f.ok(&["context", "delta", "--session", nid]);
        assert_eq!(delta["delta"]["after_revision"], baseline);
        assert_eq!(
            delta["delta"]["events"]["source_changes"][0]["changed_entities"][0]["external_key"],
            "W"
        );
    }
}
