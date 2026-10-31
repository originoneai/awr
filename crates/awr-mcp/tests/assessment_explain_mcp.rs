//! DEC-021: stdio MCP optional explain; CLI/MCP share the same assessment result.
use awr_core::*;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use rmcp::{
    RoleClient, ServiceExt, model::*, service::RunningService, transport::TokioChildProcess,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tokio::{process::Command as TokioCommand, time::timeout};

const WORK: &str = "work_items:\n- id: W\n  title: Prepare customer analysis\n  status: ready\n  owner: business-coordinator\n  next_action: Draft the analysis\n  depends_on: [D]\n  acceptance: [Deliver the reviewed analysis]\n  verification:\n    evidence_level: none\n  evidence: []\n- id: D\n  title: Required input\n  status: completed\n";
const MANIFEST: &str = "[project]\nname='MCP DEC021'\nexternal_key='mcp-dec021'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("awr-dec021-mcp-{}", Id::new()));
        fs::create_dir_all(path.join(".awr")).unwrap();
        let root = path.canonicalize().unwrap();
        fs::create_dir(root.join("decisions")).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(
            root.join("rules.md"),
            "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve exact acceptance and source facts.\n",
        )
        .unwrap();
        fs::write(
            root.join("goal.md"),
            "# Deliver useful analysis\n\nPersist work and deliver reviewed customer results.\n",
        )
        .unwrap();
        fs::write(root.join(".awr/project.toml"), MANIFEST).unwrap();
        let f = Self { root };
        let mut store = Store::open(&f.root.join(".awr/state.db")).unwrap();
        assert!(
            index_project(
                &mut store,
                &f.root,
                &Manifest::load(&f.root).unwrap(),
                false
            )
            .unwrap()
            .ok
        );
        f
    }
    async fn client(&self) -> RunningService<RoleClient, ()> {
        let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_awr-mcp"));
        command.arg("--project").arg(&self.root).kill_on_drop(true);
        timeout(
            Duration::from_secs(20),
            ().serve(TokioChildProcess::new(command).unwrap()),
        )
        .await
        .unwrap()
        .unwrap()
    }
    fn cli(&self, args: &[&str]) -> Value {
        let awr = sibling_awr_binary(Path::new(env!("CARGO_BIN_EXE_awr-mcp")));
        assert!(
            awr.exists(),
            "sibling awr binary required for CLI/MCP parity: {}",
            awr.display()
        );
        let out = Command::new(&awr)
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
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
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn call(client: &RunningService<RoleClient, ()>, name: &str, args: Value) -> CallToolResult {
    timeout(
        Duration::from_secs(30),
        client.call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap()
}

/// Cargo's Windows test binary is `awr-mcp.exe`. Replacing the file name with
/// `awr` drops `.exe` and the CLI parity check looks for a file that is not there.
fn sibling_awr_binary(mcp_exe: &Path) -> PathBuf {
    let awr_name = if mcp_exe.extension().is_some_and(|ext| ext == "exe") {
        "awr.exe"
    } else {
        "awr"
    };
    mcp_exe.with_file_name(awr_name)
}

fn success(result: CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    result.structured_content.expect("structured")
}

#[test]
fn windows_mcp_exe_resolves_to_awr_exe() {
    let mcp = Path::new(r"D:\a\awr\awr\target\debug\awr-mcp.exe");
    let awr = sibling_awr_binary(mcp);
    assert_eq!(
        awr.file_name().and_then(|name| name.to_str()),
        Some("awr.exe")
    );
    assert_eq!(
        sibling_awr_binary(Path::new("/tmp/target/debug/awr-mcp"))
            .file_name()
            .and_then(|name| name.to_str()),
        Some("awr")
    );
}

#[tokio::test]
async fn mcp_explain_off_preserves_views_and_on_matches_cli_hash() {
    let f = Fixture::new();
    let client = f.client().await;

    let full = success(call(&client, "awr_work_prepare", json!({"work":"W"})).await);
    assert!(full.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD).is_none());

    let summary = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","response_view":"summary"}),
        )
        .await,
    );
    assert!(summary.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD).is_none());
    assert_eq!(summary["response_view"]["view"], "summary");

    let action = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","response_view":"action"}),
        )
        .await,
    );
    assert!(action.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD).is_none());
    assert_eq!(action["response_view"]["view"], "action");

    let mcp_on = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","explain":true}),
        )
        .await,
    );
    assert!(mcp_on.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD).is_some());
    assert_eq!(
        mcp_on[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["side_effects"]["claimed_work"],
        false
    );
    assert_eq!(
        mcp_on[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["side_effects"]["updated_completion"],
        false
    );
    assert_eq!(
        mcp_on[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["side_effects"]["auto_invoked_tools"],
        false
    );
    assert_eq!(
        mcp_on[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["metrics"]["prepare_context_duplicated"],
        false
    );

    // Default path: one prepare tool call (no extra round-trip to build explanation).
    // Proven by attach happening inside the same awr_work_prepare response.

    let cli_on = f.cli(&["work", "prepare", "W", "--explain"]);
    assert_eq!(
        mcp_on[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessment_hash"],
        cli_on[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessment_hash"],
        "CLI and MCP must share the same assessment result for the same project snapshot"
    );
    assert_eq!(
        mcp_on["management"]["decision"]["mode"],
        cli_on["management"]["decision"]["mode"]
    );
    assert_eq!(
        mcp_on["context"]["work_context"]["context_hash"],
        cli_on["context"]["work_context"]["context_hash"]
    );

    // Assess path.
    let assess_off = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    assert!(
        assess_off
            .get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD)
            .is_none()
    );
    let assess_on = success(
        call(
            &client,
            "awr_work_assess",
            json!({"work":"W","explain":true}),
        )
        .await,
    );
    assert!(
        assess_on
            .get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD)
            .is_some()
    );
    assert_eq!(
        assess_on["decision"]["mode"],
        assess_off["decision"]["mode"]
    );

    // Invalid explain on unrelated tool.
    let bad = call(&client, "awr_work_ready", json!({"explain":true})).await;
    assert_eq!(bad.is_error, Some(true));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn explain_errors_surface_without_breaking_legacy_prepare() {
    let f = Fixture::new();
    let client = f.client().await;
    // Budget=1 forces incompleteness; explain must not hide the error shape.
    let off = call(&client, "awr_work_prepare", json!({"work":"W","budget":1})).await;
    assert_eq!(off.is_error, Some(true));
    let off_value = off.structured_content.unwrap();
    assert_ne!(off_value.get("ok"), Some(&serde_json::json!(true)));
    assert!(
        off_value
            .get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD)
            .is_none()
    );

    let on = call(
        &client,
        "awr_work_prepare",
        json!({"work":"W","budget":1,"explain":true}),
    )
    .await;
    assert_eq!(on.is_error, Some(true));
    let on_value = on.structured_content.unwrap();
    assert_ne!(on_value.get("ok"), Some(&serde_json::json!(true)));
    // Legacy error fields remain comparable.
    assert_eq!(on_value.get("ready"), off_value.get("ready"));
    assert_eq!(
        on_value.pointer("/context/completeness/complete"),
        off_value.pointer("/context/completeness/complete")
    );
    if let Some(explanation) = on_value.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD) {
        assert_eq!(explanation["side_effects"]["claimed_work"], false);
        assert_eq!(explanation["metrics"]["prepare_context_duplicated"], false);
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn mcp_explain_reads_the_project_policy_like_the_cli() {
    let f = Fixture::new();
    let client = f.client().await;
    let plain = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","explain":true}),
        )
        .await,
    );
    let view = &plain[awr_runtime::ASSESSMENT_EXPLAIN_FIELD];
    assert!(
        view.get("policy").is_none(),
        "no file, nothing to say about a policy"
    );
    let optional = [
        "continue_prepare_current_work",
        "collect_user_reply",
        "refresh_sources",
        "repair_required_materials",
        "inspect_claim_conflict",
        "record_management_observations",
    ];
    let advice: Vec<String> = view["envelope"]["advisory_actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["code"].as_str().unwrap().to_string())
        .collect();
    let withheld = advice
        .iter()
        .find(|code| optional.contains(&code.as_str()))
        .expect("the fixture is offered some optional advice")
        .clone();
    fs::write(
        f.root.join(".awr/assessment-policy.toml"),
        format!("format = 1\npolicy_id = \"mcp-quiet\"\n\n[[version]]\nversion = 1\n\n[version.advice]\n{withheld} = false\n"),
    )
    .unwrap();
    let mcp = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","explain":true}),
        )
        .await,
    );
    let view = &mcp[awr_runtime::ASSESSMENT_EXPLAIN_FIELD];
    assert_eq!(view["policy"]["source"], "project");
    assert_eq!(view["policy"]["policy_id"], "mcp-quiet");
    let now: Vec<&str> = view["envelope"]["advisory_actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["code"].as_str().unwrap())
        .collect();
    assert!(
        !now.contains(&withheld.as_str()),
        "{withheld} should be withheld: {now:?}"
    );
    // The CLI over the same project says the same, byte for byte.
    let cli = f.cli(&["work", "prepare", "W", "--explain"]);
    assert_eq!(
        cli[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessment_hash"],
        view["envelope"]["assessment_hash"]
    );
    assert_eq!(
        cli[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["policy"],
        view["policy"]
    );
}
