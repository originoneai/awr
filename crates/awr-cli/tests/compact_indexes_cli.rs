use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
#[path = "../../../tests/common/compact_indexes.rs"]
mod contract;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let f = Self(std::env::temp_dir().join(format!("awr-compact-indexes-{}", Id::new())));
        fs::create_dir_all(&f.0).unwrap();
        let mut ledger = String::from(
            "goals:\n- id: G\n  title: Deliver a guide\n  status: active\n  summary: Explain the public contract\nwork_items:\n- id: W\n  title: Write guide\n  status: ready\n  goal: G\n  acceptance: [Reviewed guide]\n  next_action: Draft the guide\n",
        );
        for n in 0..40 {
            ledger.push_str(&format!(
                "- id: HISTORY-{n}\n  title: Review support item {n}\n  status: completed\n"
            ));
        }
        fs::write(f.0.join("work.yaml"), ledger).unwrap();
        fs::write(f.0.join("rules.md"), "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve every required source fact exactly.\n").unwrap();
        fs::write(f.0.join("sources.toml"), "[project]\nname='Index projection fixture'\nexternal_key='index-projection'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n").unwrap();
        assert!(
            f.run(&["init", "--manifest", "sources.toml", "--accept"])
                .status
                .success()
        );
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
    fn read(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn opt_in_cli_projection_covers_zero_one_and_many_omissions_and_full_drilldown() {
    let f = Fixture::new();
    let full = f.read(&["work", "prepare", "W", "--budget", "100000"]);
    let pack = &full["context"]["work_context"];
    let budgets = [
        100000,
        pack["token_estimate"].as_u64().unwrap() - 1,
        pack["required_tokens"].as_u64().unwrap() + 50,
    ];
    for (case, budget) in budgets.into_iter().enumerate() {
        let budget = budget.to_string();
        for view in ["full", "summary", "action"] {
            let args = [
                "work",
                "prepare",
                "W",
                "--budget",
                &budget,
                "--response-view",
                view,
            ];
            let legacy = f.read(&args);
            let count = legacy["context"]["work_context"]["omitted_chunks"]
                .as_array()
                .unwrap()
                .len();
            match case {
                0 => assert_eq!(count, 0),
                1 => assert_eq!(count, 1),
                _ => assert!(count > 10),
            }
            let mut opted = args.to_vec();
            opted.push("--compact-indexes");
            let compact = f.read(&opted);
            if view == "full" {
                assert_eq!(compact, legacy);
            } else {
                contract::assert_projection(&legacy, &compact, count);
                let q = &compact["response_view"]["full_result"];
                assert_eq!(q["response_view"], "full");
                assert!(q.get("compact_indexes").is_none());
                let detail = f.read(&[
                    "work",
                    "prepare",
                    q["work"].as_str().unwrap(),
                    "--budget",
                    &budget,
                ]);
                assert_eq!(
                    detail["context"]["work_context"]["omitted_chunks"],
                    legacy["context"]["work_context"]["omitted_chunks"]
                );
                assert_eq!(detail["project_revision"], compact["project_revision"]);
                assert_eq!(
                    detail["context"]["work_context"]["context_hash"],
                    compact["context"]["work_context"]["context_hash"]
                );
            }
        }
    }
    let rev = f.read(&["session", "list"])["project_revision"].to_string();
    let started = f.read(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "fixture",
        "--provider",
        "fixture",
        "--model",
        "test",
        "--expected-revision",
        &rev,
    ]);
    let sid = started["session"]["id"].as_str().unwrap();
    let args = [
        "work",
        "prepare",
        "W",
        "--session",
        sid,
        "--response-view",
        "action",
        "--budget",
        "100000",
    ];
    let legacy = f.read(&args);
    let mut opted = args.to_vec();
    opted.push("--compact-indexes");
    assert_eq!(
        legacy["context"]["work_context"]["omitted_chunks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(f.read(&opted), legacy);
    let rev = f.read(&["session", "list"])["project_revision"].to_string();
    f.read(&[
        "session",
        "end",
        "--session",
        sid,
        "--expected-revision",
        &rev,
    ]);
}

#[test]
fn cli_projection_preserves_existing_failures_and_human_output() {
    let f = Fixture::new();
    for view in ["full", "summary", "action"] {
        let args = [
            "work",
            "prepare",
            "W",
            "--budget",
            "1",
            "--response-view",
            view,
        ];
        let original = f.run(&args);
        let mut opted = args.to_vec();
        opted.push("--compact-indexes");
        let compact = f.run(&opted);
        assert!(!original.status.success());
        assert_eq!(original.status.code(), compact.status.code());
        assert_eq!(original.stdout, compact.stdout);
        assert_eq!(original.stderr, compact.stderr);
    }
    fs::write(
        f.0.join("work.yaml"),
        fs::read_to_string(f.0.join("work.yaml"))
            .unwrap()
            .replace("acceptance: [Reviewed guide]", "acceptance: []"),
    )
    .unwrap();
    f.read(&["source", "reindex"]);
    for view in ["full", "summary", "action"] {
        let args = ["work", "prepare", "W", "--response-view", view];
        let original = f.run(&args);
        let mut opted = args.to_vec();
        opted.push("--compact-indexes");
        let compact = f.run(&opted);
        assert!(!original.status.success());
        assert_eq!(original.status.code(), compact.status.code());
        assert_eq!(original.stdout, compact.stdout);
        assert_eq!(original.stderr, compact.stderr);
    }
    let human = |opted: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_awr"));
        cmd.arg("--project")
            .arg(&f.0)
            .args(["work", "prepare", "W", "--response-view", "action"]);
        if opted {
            cmd.arg("--compact-indexes");
        }
        cmd.output().unwrap()
    };
    assert_eq!(human(false).stdout, human(true).stdout);
}
