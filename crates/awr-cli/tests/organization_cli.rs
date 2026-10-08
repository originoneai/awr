use awr_core::*;
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GOAL: &str = "goals:\n- id: G\n  title: Search the document collection\n  status: active\n  summary: User requested document search in the existing portal\n  success_criteria: [Readers find the requested document]\n";
const WORK: &str = "work_items:\n- id: W\n  title: Add document search\n  goal: G\n  status: ready\n  acceptance: [Readers find the requested document]\n  next_action: Implement the search handler\n";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr organization 空间 {}", Id::new()));
        fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn write(&self, path: &str, body: &str) {
        fs::write(self.0.join(path), body).unwrap();
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
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn init(&self, body: &str) -> Value {
        self.write("work-ledger.yaml", body);
        self.ok(&["init", "--accept"])
    }
    fn inspect(&self) -> Value {
        self.ok(&["intake", "inspect"])["organization"].clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn gap(report: &Value, code: &str) -> bool {
    report["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["code"] == code)
}

#[test]
fn new_project_has_ordered_guidance_and_minimal_draft_without_false_readiness() {
    let f = Fixture::new();
    f.write(
        "README.md",
        "# Document portal\nExisting code needs organization.\n",
    );
    let preview = f.ok(&["init"]);
    assert_eq!(preview["organization"]["state"], "not_initialized");
    assert_eq!(
        preview["draft"]["generated_files"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert!(!f.0.join(".awr").exists());
    let ids: Vec<_> = preview["organization"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["sources", "goals", "work", "recheck"]);
    let unavailable = f.run(&["status"]);
    assert!(!unavailable.status.success());
    let value: Value = serde_json::from_slice(&unavailable.stdout).unwrap();
    assert_eq!(value["organization"]["state"], "not_initialized");
    assert!(value["total"].is_null());
    let installed = f.ok(&["init", "--accept"]);
    assert_eq!(installed["organization"]["state"], "needs_organization");
    let report = f.inspect();
    assert!(gap(&report, "goal_unconfirmed"));
    assert!(gap(&report, "intake_work_only"));
    assert_eq!(report["business_execution_ready"], false);
    assert!(!f.0.join(".awr/intake/PLAN.md").exists());
    assert!(!f.0.join(".awr/intake/RULES.md").exists());
}

#[test]
fn single_ledger_organization_loop_preserves_material_and_becomes_ready() {
    let f = Fixture::new();
    f.write("README.md", "# Portal\nUser wants document search.\n");
    f.write(
        "PLAN.md",
        "# Search plan {#search status=active}\nKeep the existing renderer.\n",
    );
    let draft = format!(
        "{}{}",
        GOAL.replace("status: active", "status: draft"),
        WORK.replace("  goal: G\n", "")
            .replace("  acceptance: [Readers find the requested document]\n", "")
    );
    f.init(&draft);
    let report = f.inspect();
    assert_eq!(report["state"], "needs_organization");
    assert!(gap(&report, "work_goal_missing"));
    assert!(gap(&report, "work_structure_incomplete"));
    assert!(!f.0.join(".awr/intake/GOALS.md").exists());
    f.write("work-ledger.yaml", &format!("{GOAL}{WORK}"));
    let ready = f.inspect();
    assert_eq!(ready["state"], "ready");
    assert_eq!(ready["executable_work"], json!(["W"]));
    assert!(
        ready["project_revision"].as_u64().unwrap() > report["project_revision"].as_u64().unwrap()
    );
    assert_eq!(f.ok(&["ready"])["organization"]["state"], "ready");
    let context = f.ok(&["context", "compile", "--work", "W", "--detached"]);
    assert_eq!(context["completeness"]["complete"], true);
    assert_eq!(context["completeness"]["goal_context_complete"], true);
    let bootstrap = f.ok(&["context", "bootstrap", "--work", "W"]);
    assert_eq!(bootstrap["context"]["complete"], true);
    assert_eq!(
        fs::read_to_string(f.0.join("README.md")).unwrap(),
        "# Portal\nUser wants document search.\n"
    );
    assert_eq!(
        fs::read_to_string(f.0.join("PLAN.md")).unwrap(),
        "# Search plan {#search status=active}\nKeep the existing renderer.\n"
    );
    f.write(
        "work-ledger.yaml",
        &format!(
            "{}{}",
            GOAL.replace("status: active", "status: needs_confirmation"),
            WORK
        ),
    );
    assert_eq!(f.inspect()["state"], "needs_organization");
}

#[test]
fn empty_cancelled_and_source_completed_are_distinct() {
    for (work, expected, missing) in [
        (
            "work_items: []\n".into(),
            "needs_organization",
            Some("ledger_empty"),
        ),
        (
            WORK.replace("status: ready", "status: cancelled"),
            "closed_without_completion",
            None,
        ),
        (
            WORK.replace("status: ready", "status: completed"),
            "awaiting_verification",
            Some("completion_not_checked"),
        ),
    ] {
        let f = Fixture::new();
        f.init(&format!("{GOAL}{work}"));
        let report = f.inspect();
        assert_eq!(report["state"], expected);
        if let Some(code) = missing {
            assert!(gap(&report, code));
        }
        assert_eq!(report["business_execution_ready"], false);
        assert_eq!(report["verified_completed"], 0);
    }
}

#[test]
fn malformed_source_keeps_error_and_cannot_turn_retained_work_into_ready() {
    let f = Fixture::new();
    f.init(&format!("{GOAL}{WORK}"));
    f.write("work-ledger.yaml", "work_items: [malformed");
    let out = f.run(&["intake", "inspect"]);
    assert!(!out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["organization"]["state"], "source_unreadable");
    assert_eq!(value["organization"]["business_execution_ready"], false);
    assert!(!value["source_issues"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(f.0.join("work-ledger.yaml")).unwrap(),
        "work_items: [malformed"
    );
    f.write("work-ledger.yaml", &format!("{GOAL}{WORK}"));
    assert_eq!(f.inspect()["state"], "ready");
    f.write(".awr/project.toml", "[malformed");
    let out = f.run(&["status"]);
    assert!(!out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["organization"]["state"], "source_unreadable");
}

#[test]
fn missing_dependency_cycle_and_blocker_remain_actionable() {
    for addition in [
        "  depends_on: [absent]\n",
        "  depends_on: [W]\n",
        "  blocker: Waiting for sample documents\n",
    ] {
        let f = Fixture::new();
        f.init(&format!("{GOAL}{WORK}{addition}"));
        let report = f.inspect();
        assert_eq!(report["state"], "blocked");
        assert!(
            report["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["id"] == "dependencies")
        );
        assert_eq!(report["executable_work_total"], 0);
    }
}

#[test]
fn uncertain_future_goal_does_not_block_unrelated_ready_work() {
    let f = Fixture::new();
    let goals = format!(
        "{GOAL}- id: FUTURE\n  title: Possible recommendations\n  status: candidate\n  summary: Not requested yet\n"
    );
    f.init(&format!("{goals}{WORK}"));
    let report = f.inspect();
    assert_eq!(report["state"], "ready");
    assert!(gap(&report, "goal_unconfirmed"));
    assert_eq!(report["executable_work"], json!(["W"]));
    f.write(
        "work-ledger.yaml",
        &format!(
            "{goals}{}",
            WORK.replace("status: ready", "status: completed")
        ),
    );
    assert_eq!(f.inspect()["state"], "needs_organization");
}

#[test]
fn markdown_ledger_can_be_organized_in_place_with_source_linked_goals() {
    let f = Fixture::new();
    f.write(
        "GOALS.md",
        "# Search documents {#G status=active}\nUser requested document search.\n",
    );
    let table = "| 编号 | 任务 | 状态 | 目标 | 验收 | 下一步 |\n| --- | --- | --- | --- | --- | --- |\n| W | 文档搜索 | 就绪 | G | 能找到目标文档 | 实现搜索接口 |\n";
    f.write("台账.md", table);
    let initial = f.ok(&["init", "--accept"]);
    let goal_key = initial["organization"]["goals"][0]["key"].as_str().unwrap();
    let table = table.replace("| G |", &format!("| {goal_key} |"));
    f.write("台账.md", &table);
    assert_eq!(f.inspect()["state"], "ready");
    assert_eq!(fs::read_to_string(f.0.join("台账.md")).unwrap(), table);
    f.write(
        "台账.md",
        &table.replace(&format!("| {goal_key} |"), "| missing-goal |"),
    );
    let report = f.inspect();
    assert!(gap(&report, "work_goal_unresolved"));
    assert!(report["gaps"][0]["source_refs"][0]["start_line"].is_number());
}

fn register_report(f: &Fixture, passed: bool) {
    let (mut store, project) = {
        let s = Store::open_existing(&f.0.join(".awr/state.db")).unwrap();
        let p = s.project_by_root(&f.0).unwrap();
        (s, p)
    };
    let at = now_millis().unwrap();
    let report = json!({"version":1,"work_item":"W","source_sha":SHA,"command":"verify document search","scope":["W"],"verified_at":at,"checks":[{"name":"search result","passed":passed,"details":"The expected document was returned","criteria":["Readers find the requested document"]}]});
    let bytes = serde_json::to_vec(&report).unwrap();
    fs::write(f.0.join("search-report.json"), &bytes).unwrap();
    store
        .record_evidence(
            project.id,
            project.project_revision,
            EvidenceDraft {
                external_key: "search-proof".into(),
                work_item_key: Some("W".into()),
                evidence_type: "completion_report".into(),
                level: EvidenceLevel::LocallyVerified,
                summary: "Search verification".into(),
                locator: "search-report.json".into(),
                sha256: Some(
                    awr_source::fingerprint(&bytes)
                        .trim_start_matches("sha256:")
                        .into(),
                ),
                source_sha: Some(SHA.into()),
                command: Some("verify document search".into()),
                scope: vec!["W".into()],
                branch_id: None,
                verified_at: Some(at),
            },
        )
        .unwrap();
}

#[test]
fn completed_requires_current_actual_passing_report_and_all_acceptance() {
    let f = Fixture::new();
    f.init(&format!(
        "{GOAL}{}",
        WORK.replace("status: ready", "status: completed")
    ));
    register_report(&f, true);
    assert_eq!(f.inspect()["state"], "awaiting_verification");
    let checked = f.ok(&["intake", "inspect", "--source-sha", SHA]);
    assert_eq!(checked["organization"]["state"], "completed");
    assert_eq!(checked["organization"]["verified_completed"], 1);
    let different = f.ok(&[
        "intake",
        "inspect",
        "--source-sha",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    ]);
    assert_eq!(different["organization"]["state"], "awaiting_verification");
    f.write("search-report.json", "{}");
    assert_eq!(
        f.ok(&["intake", "inspect", "--source-sha", SHA])["organization"]["state"],
        "awaiting_verification"
    );
}

#[test]
fn failing_reports_and_cancelled_required_scope_cannot_complete_project() {
    let f = Fixture::new();
    f.init(&format!(
        "{GOAL}{}",
        WORK.replace("status: ready", "status: completed")
    ));
    register_report(&f, false);
    assert_eq!(
        f.ok(&["status", "--source-sha", SHA])["organization"]["state"],
        "awaiting_verification"
    );
    let f = Fixture::new();
    f.init(&format!(
        "{GOAL}{}- id: REQUIRED\n  status: cancelled\n  required: true\n",
        WORK.replace("status: ready", "status: completed")
    ));
    register_report(&f, true);
    assert_eq!(
        f.ok(&["status", "--source-sha", SHA])["organization"]["state"],
        "closed_without_completion"
    );
}

#[test]
fn diagnostic_limit_never_hides_missing_work_as_ready() {
    let f = Fixture::new();
    let mut work = "work_items:\n".to_string();
    for i in 0..125 {
        work.push_str(&format!("- id: W{i}\n  status: ready\n"));
    }
    f.init(&format!("{GOAL}{work}"));
    let report = f.inspect();
    assert_eq!(report["state"], "needs_organization");
    assert_eq!(report["truncated"], true);
    assert_eq!(report["gaps"].as_array().unwrap().len(), 100);
    assert!(report["gap_total"].as_u64().unwrap() > 100);
    assert_eq!(report["executable_work_total"], 0);
}

#[test]
fn minimal_profile_is_revision_bound_and_preserves_configured_hard_rules() {
    let f = Fixture::new();
    f.init(&format!("{GOAL}{WORK}"));
    let before = f.inspect();
    assert_eq!(before["context_profile"], "minimal");
    let manifest = fs::read_to_string(f.0.join(".awr/project.toml")).unwrap();
    f.write(
        ".awr/project.toml",
        &manifest.replace(
            "context_profile = \"minimal\"",
            "context_profile = \"standard\"",
        ),
    );
    let standard = f.inspect();
    assert!(gap(&standard, "rules_source_missing"));
    assert_eq!(standard["business_execution_ready"], false);
    assert!(
        standard["project_revision"].as_u64().unwrap()
            > before["project_revision"].as_u64().unwrap()
    );
    let context = f.run(&["context", "compile", "--work", "W", "--detached"]);
    assert!(!context.status.success());
    f.write("rules.md", "# Preserve input {#keep severity=hard scope=project value=*}\nNever overwrite the imported source documents.\n");
    f.write(".awr/project.toml", &format!("{manifest}\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n"));
    let context = f.ok(&["context", "compile", "--work", "W", "--detached"]);
    assert!(
        context
            .to_string()
            .contains("Never overwrite the imported source documents.")
    );
    fs::remove_file(f.0.join("rules.md")).unwrap();
    let failed = f.run(&["context", "compile", "--work", "W", "--detached"]);
    assert!(!failed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&f.run(&["intake", "inspect"]).stdout).unwrap()["organization"]
            ["state"],
        "source_unreadable"
    );
}

/// The gap lines the rendered context prints for the hard subset (empty when it prints none).
fn hard_gap_lines(context: &Value) -> Vec<String> {
    let text = context["work_context"]["rendered_context"]
        .as_str()
        .unwrap();
    match text.split("[hard_context_gaps]\n").nth(1) {
        Some(rest) => rest
            .lines()
            .take_while(|line| !line.starts_with('['))
            .map(str::to_string)
            .collect(),
        None => vec![],
    }
}

#[test]
fn rendered_hard_gaps_agree_with_completeness_for_the_minimal_and_standard_profiles() {
    let f = Fixture::new();
    f.init(&format!("{GOAL}{WORK}"));
    // An explicit minimal profile may go without a rules source: neither the flags nor the text report a gap.
    let minimal = f.ok(&["context", "compile", "--work", "W", "--detached"]);
    assert_eq!(minimal["completeness"]["complete"], true);
    assert_eq!(minimal["completeness"]["rules_complete"], true);
    assert!(
        minimal["work_context"]["rendered_context"]
            .as_str()
            .unwrap()
            .contains("Context profile: minimal")
    );
    assert_eq!(hard_gap_lines(&minimal), Vec::<String>::new());

    // A real gap of the hard subset is reported on its own; the missing rules source is not blamed for it.
    f.write(
        "work-ledger.yaml",
        &format!(
            "{GOAL}{}",
            WORK.replace("  acceptance: [Readers find the requested document]\n", "")
        ),
    );
    let refused = f.run(&["context", "compile", "--work", "W", "--detached"]);
    assert!(!refused.status.success());
    let body: Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(body["completeness"]["rules_complete"], true);
    assert_eq!(
        hard_gap_lines(&body),
        [
            "CONTEXT INCOMPLETE (hard subset)",
            "acceptance criteria are missing or blank"
        ]
    );
    f.write("work-ledger.yaml", &format!("{GOAL}{WORK}"));

    // The standard profile refuses the same project, in the flags and in the text.
    let manifest = fs::read_to_string(f.0.join(".awr/project.toml")).unwrap();
    f.write(
        ".awr/project.toml",
        &manifest.replace(
            "context_profile = \"minimal\"",
            "context_profile = \"standard\"",
        ),
    );
    let refused = f.run(&["context", "compile", "--work", "W", "--detached"]);
    assert!(!refused.status.success());
    let body: Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(body["completeness"]["complete"], false);
    assert_eq!(body["completeness"]["rules_complete"], false);
    assert!(
        body["completeness"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "rules_source_missing")
    );
    assert_eq!(
        hard_gap_lines(&body),
        [
            "CONTEXT INCOMPLETE (hard subset)",
            "rules source is missing"
        ]
    );

    // With a rules source the standard profile is complete again and prints no gap.
    f.write("rules.md", "# Preserve input {#keep severity=hard scope=project value=*}\nNever overwrite the imported source documents.\n");
    f.write(".awr/project.toml", &format!("{manifest}\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n").replace("context_profile = \"minimal\"", "context_profile = \"standard\""));
    let standard = f.ok(&["context", "compile", "--work", "W", "--detached"]);
    assert_eq!(standard["completeness"]["rules_complete"], true);
    assert_eq!(hard_gap_lines(&standard), Vec::<String>::new());
}

#[test]
fn finished_goal_with_open_work_and_completed_intake_cannot_imply_project_completion() {
    let f = Fixture::new();
    f.init(&format!(
        "{}{WORK}",
        GOAL.replace("status: active", "status: completed")
    ));
    let report = f.inspect();
    assert_eq!(report["state"], "needs_organization");
    assert!(gap(&report, "work_goal_unresolved"));
    f.write(
        "work-ledger.yaml",
        &format!(
            "{GOAL}{}  kind: intake\n",
            WORK.replace("status: ready", "status: completed")
        ),
    );
    f.inspect();
    register_report(&f, true);
    let report = f.ok(&["status", "--source-sha", SHA]);
    assert_eq!(report["history"]["verified_completed"], 1);
    assert_eq!(report["organization"]["state"], "needs_organization");
    assert!(gap(&report["organization"], "business_work_missing"));
}

#[test]
fn organized_minimal_project_can_claim_and_write_progress_with_profile_checks() {
    let f = Fixture::new();
    f.init(&format!("{GOAL}{WORK}"));
    let revision = f.ok(&["status"])["project_revision"].to_string();
    let started = f.ok(&[
        "session",
        "start",
        "--work",
        "W",
        "--agent",
        "intake-worker",
        "--provider",
        "generic",
        "--model",
        "local",
        "--claim",
        "--expected-revision",
        &revision,
    ]);
    let session = started["session"]["id"].as_str().unwrap();
    let revision = f.ok(&["status"])["project_revision"].to_string();
    f.ok(&[
        "work",
        "progress",
        "W",
        "--session",
        session,
        "--expected-revision",
        &revision,
        "--reason",
        "The goal and delivery structure are established",
        "--next-action",
        "Review the implemented handler",
    ]);
    let work = f.ok(&["work", "show", "W"]);
    assert_eq!(work["work"]["status"], "in_progress");
    assert_eq!(
        work["work"]["next_action"],
        "Review the implemented handler"
    );
    let revision = work["project_revision"].to_string();
    let manifest = fs::read_to_string(f.0.join(".awr/project.toml")).unwrap();
    f.write(
        ".awr/project.toml",
        &manifest.replace(
            "context_profile = \"minimal\"",
            "context_profile = \"standard\"",
        ),
    );
    let before = fs::read(f.0.join("work-ledger.yaml")).unwrap();
    let stale = f.run(&[
        "work",
        "progress",
        "W",
        "--session",
        session,
        "--expected-revision",
        &revision,
        "--reason",
        "Try stale profile",
        "--next-action",
        "This must not be written",
    ]);
    assert!(!stale.status.success());
    assert_eq!(fs::read(f.0.join("work-ledger.yaml")).unwrap(), before);
}
