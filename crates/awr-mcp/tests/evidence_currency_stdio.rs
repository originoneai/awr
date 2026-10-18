//! Issue #345 regression coverage over the MCP JSON-lines wire protocol.
//!
//! This intentionally drives the real `awr-mcp` child with initialize,
//! tools/list, and tools/call messages. It does not use rmcp's native client;
//! native-client/E4 coverage belongs to the separate client integration tests.

use awr_core::{Id, now_millis};
use awr_source::{Manifest, fingerprint, index_project};
use awr_store::Store;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

const C1: &str = "The primary analysis is reviewed";
const C2: &str = "The primary analysis is delivered";
const OTHER_C1: &str = "The independent analysis is reviewed";

const MANIFEST: &str = "[project]\nname='MCP evidence currency fixture'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n";

const WORK: &str = "goals:\n- id: G\n  title: Deliver reviewed analyses\n  status: active\n  summary: Deliver reviewed primary and independent analyses\n  success_criteria: [The analyses are reviewed and delivered]\nwork_items:\n- id: W\n  title: Deliver the primary analysis\n  goal: G\n  status: in_progress\n  owner: worker-a\n  next_action: Verify the primary analysis\n  depends_on: [D]\n  acceptance: [The primary analysis is reviewed, The primary analysis is delivered]\n  verification:\n    evidence_level: none\n  evidence: []\n- id: OTHER\n  title: Deliver the independent analysis\n  goal: G\n  status: in_progress\n  owner: worker-b\n  next_action: Verify the independent analysis\n  acceptance: [The independent analysis is reviewed]\n  verification:\n    evidence_level: none\n  evidence: []\n- id: D\n  title: Required input\n  status: completed\n";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-mcp-evidence-currency-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::create_dir(root.join("decisions")).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(
            root.join("rules.md"),
            "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve exact acceptance and source facts.\n",
        )
        .unwrap();
        fs::write(
            root.join("goal.md"),
            "# Deliver reviewed analyses\n\nDeliver the reviewed primary and independent analyses.\n",
        )
        .unwrap();
        fs::write(root.join(".awr/project.toml"), MANIFEST).unwrap();
        let root = root.canonicalize().unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let report =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(report.ok, "{report:?}");
        Self { root }
    }

    fn revision(&self) -> u64 {
        Store::open_readonly(&self.root.join(".awr/state.db"))
            .unwrap()
            .project_by_root(&self.root)
            .unwrap()
            .project_revision
    }

    fn ledger_sha(&self) -> String {
        fingerprint(&fs::read(self.root.join("work.yaml")).unwrap())
            .trim_start_matches("sha256:")
            .to_owned()
    }

    fn write_report(&self, path: &str, report: &Value) -> Vec<u8> {
        let bytes = serde_json::to_vec_pretty(report).unwrap();
        fs::write(self.root.join(path), &bytes).unwrap();
        bytes
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct WireClient {
    mode: &'static str,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
}

impl WireClient {
    async fn spawn(root: &Path, mode: &'static str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_awr-mcp"))
            .arg("--project")
            .arg(root)
            .env("AWR_MCP_TOOL_EXPOSURE_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            mode,
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            next_id: 1,
        };
        let initialize = client
            .rpc(
                "initialize",
                json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "issue-345-wire-probe", "version": "1"}
                }),
            )
            .await;
        assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");
        client.notify("notifications/initialized").await;
        client
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let line = format!("{request}\n");
        self.stdin
            .as_mut()
            .unwrap()
            .write_all(line.as_bytes())
            .await
            .unwrap();
        self.stdin.as_mut().unwrap().flush().await.unwrap();
        loop {
            let mut response_line = String::new();
            let read = self.stdout.as_mut().unwrap().read_line(&mut response_line);
            let read = timeout(Duration::from_secs(30), read)
                .await
                .unwrap()
                .unwrap();
            assert!(
                read > 0,
                "MCP child closed stdout while waiting for {method}"
            );
            let response: Value = serde_json::from_str(response_line.trim_end()).unwrap();
            if response["id"] == json!(id) {
                return response;
            }
        }
    }

    async fn notify(&mut self, method: &str) {
        let line = format!("{}\n", json!({"jsonrpc":"2.0","method":method}));
        self.stdin
            .as_mut()
            .unwrap()
            .write_all(line.as_bytes())
            .await
            .unwrap();
        self.stdin.as_mut().unwrap().flush().await.unwrap();
    }

    async fn tool_result(&mut self, name: &str, arguments: Value) -> Value {
        let (wire_name, wire_arguments) = if self.mode == "flat" {
            (name, arguments)
        } else {
            let domain = match name {
                "awr_project_status" | "awr_work_get" => "awr_query",
                "awr_context_compile" | "awr_completion_prepare" => "awr_context",
                "awr_work_transition" => "awr_work",
                "awr_evidence_record" => "awr_evidence",
                "awr_session_start" => "awr_session",
                "awr_source_reindex" => "awr_continuity",
                _ => panic!("missing hierarchical mapping for {name}"),
            };
            (domain, json!({"child_tool":name,"arguments":arguments}))
        };
        let response = self
            .rpc(
                "tools/call",
                json!({"name":wire_name,"arguments":wire_arguments}),
            )
            .await;
        assert!(
            response.get("error").is_none(),
            "wire MCP error: {response}"
        );
        response["result"].clone()
    }

    async fn call_ok(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.tool_result(name, arguments).await;
        assert_ne!(result["isError"], true, "MCP tool error: {result}");
        body(&result)
    }

    async fn call_error(&mut self, name: &str, arguments: Value, code: &str) -> Value {
        let result = self.tool_result(name, arguments).await;
        assert_eq!(result["isError"], true, "expected MCP tool error: {result}");
        let value = body(&result);
        assert_eq!(value["code"], code, "{value}");
        value
    }

    async fn domain_manifest(&mut self, domain: &str) -> Value {
        let response = self
            .rpc("tools/call", json!({"name":domain,"arguments":{}}))
            .await;
        assert!(
            response.get("error").is_none(),
            "wire MCP error: {response}"
        );
        let result = response["result"].clone();
        assert_ne!(result["isError"], true, "MCP tool error: {result}");
        body(&result)
    }

    async fn close(mut self) {
        self.stdin.take();
        self.stdout.take();
        let exited = timeout(Duration::from_secs(5), self.child.wait()).await;
        if !matches!(exited, Ok(Ok(status)) if status.success()) {
            let _ = self.child.kill().await;
            let _ = timeout(Duration::from_secs(5), self.child.wait()).await;
        }
    }
}

fn body(result: &Value) -> Value {
    let structured = result["structuredContent"].clone();
    assert!(
        !structured.is_null(),
        "MCP response lacks structuredContent: {result}"
    );
    let text = result["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(text).unwrap(), structured);
    structured
}

fn report(work: &str, source_sha: &str, criteria: &[&str]) -> Value {
    json!({
        "version": 1,
        "work_item": work,
        "source_sha": source_sha,
        "command": format!("review {work} report"),
        "scope": [work],
        "verified_at": now_millis().unwrap(),
        "checks": [{"name":"reviewed result","passed":true,"details":"Checked the exact fixture result","criteria":criteria}],
    })
}

fn evidence_currency(value: &Value, key: &str) -> Value {
    // Flat and grouped evidence are the same work-scoped read path. Grouping
    // must preserve each assessment, including ledger reuse and strict Git SHA.
    for group in value["evidence_groups"].as_array().unwrap() {
        for record in group["records"].as_array().unwrap() {
            let flat = value["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["external_key"] == record["external_key"])
                .unwrap();
            assert_eq!(record["currency"], flat["currency"]);
            assert_eq!(record["missing_bindings"], flat["missing_bindings"]);
            assert_eq!(record["source_sha"], flat["source_sha"]);
            assert_eq!(record["reasons"], flat["reasons"]);
        }
    }
    value["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["external_key"] == key)
        .unwrap_or_else(|| panic!("missing evidence {key}: {value}"))["currency"]
        .clone()
}

#[tokio::test]
async fn issue_345_evidence_currency_survives_ledger_drift_over_wire_mcp() {
    for mode in ["flat", "hierarchical"] {
        let f = Fixture::new();
        let mut client = WireClient::spawn(&f.root, mode).await;
        let listed = client.rpc("tools/list", json!({})).await;
        let names = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        if mode == "flat" {
            for name in [
                "awr_project_status",
                "awr_work_get",
                "awr_context_compile",
                "awr_completion_prepare",
                "awr_evidence_record",
                "awr_work_transition",
                "awr_session_start",
                "awr_source_reindex",
            ] {
                assert!(
                    names.contains(name),
                    "flat catalog omitted {name}: {names:?}"
                );
            }
        } else {
            for name in [
                "awr_query",
                "awr_context",
                "awr_work",
                "awr_evidence",
                "awr_session",
                "awr_continuity",
            ] {
                assert!(
                    names.contains(name),
                    "hierarchical catalog omitted {name}: {names:?}"
                );
                let manifest = client.domain_manifest(name).await;
                assert_eq!(manifest["mode"], "manifest");
                assert!(manifest["child_total"].as_u64().unwrap() > 0);
            }
        }

        let initial_sha = f.ledger_sha();
        let started_w = client
            .call_ok(
                "awr_session_start",
                json!({
                    "work":"W",
                    "conversation":"issue-345-primary",
                    "agent":"worker-a",
                    "provider":"fixture",
                    "model":"wire",
                    "expected_revision":f.revision(),
                    "claim":true,
                    "ttl_ms":600000,
                }),
            )
            .await;
        let session_w = started_w["session"]["id"].as_str().unwrap().to_owned();
        let started_other = client
            .call_ok(
                "awr_session_start",
                json!({
                    "work":"OTHER",
                    "conversation":"issue-345-independent",
                    "agent":"worker-b",
                    "provider":"fixture",
                    "model":"wire",
                    "expected_revision":f.revision(),
                    "claim":true,
                    "ttl_ms":600000,
                }),
            )
            .await;
        let session_other = started_other["session"]["id"].as_str().unwrap().to_owned();
        assert_eq!(f.ledger_sha(), initial_sha);

        let context = client
            .call_ok(
                "awr_context_compile",
                json!({"work":"W","detached":true,"source_sha":initial_sha}),
            )
            .await;
        assert!(context["work_context"]["context_hash"].is_string());

        let primary_report = report("W", &initial_sha, &[C1, C2]);
        let primary_bytes = f.write_report("primary-report.json", &primary_report);
        let primary_prepare_args = json!({
            "work":"W",
            "report":"primary-report.json",
            "evidence_key":"primary-report",
            "source_sha":initial_sha,
            "level":"locally_verified",
        });
        let preflight = client
            .call_ok("awr_completion_prepare", primary_prepare_args.clone())
            .await;
        assert_eq!(preflight["completion_claimed"], false);
        assert_eq!(preflight["verification_executed"], false);
        assert_eq!(
            preflight["report_sha256"],
            fingerprint(&primary_bytes).trim_start_matches("sha256:")
        );

        let mut corrupt = primary_report.clone();
        corrupt["checks"][0]["passed"] = json!(false);
        f.write_report("primary-report.json", &corrupt);
        let invalid = client
            .call_error(
                "awr_completion_prepare",
                primary_prepare_args.clone(),
                "EvidenceMissing",
            )
            .await;
        assert_eq!(
            invalid["details"]["location"]["locator"],
            "primary-report.json"
        );
        f.write_report("primary-report.json", &primary_report);

        let mut primary_evidence = preflight["evidence"].clone();
        primary_evidence["expected_revision"] = json!(f.revision());
        let recorded_primary = client
            .call_ok("awr_evidence_record", primary_evidence)
            .await;
        assert_eq!(
            recorded_primary["evidence"]["external_key"],
            "primary-report"
        );
        assert!(recorded_primary["evidence"]["section_fingerprint"].is_string());

        let git_report = f.root.join("git-proof.txt");
        fs::write(&git_report, "strict revision proof\n").unwrap();
        let recorded_git = client
            .call_ok(
                "awr_evidence_record",
                json!({
                    "expected_revision":f.revision(),
                    "external_key":"git-proof",
                    "work":"W",
                    "evidence_type":"note",
                    "level":"implemented",
                    "summary":"Proof tied to a Git revision",
                    "locator":"git-proof.txt",
                    "sha256":fingerprint(b"strict revision proof\n").trim_start_matches("sha256:"),
                    "source_sha":"c".repeat(40),
                    "command":"inspect Git revision",
                    "scope":["W"],
                    "verified_at":now_millis().unwrap(),
                }),
            )
            .await;
        assert!(recorded_git["evidence"]["section_fingerprint"].is_null());

        let other_report = report("OTHER", &initial_sha, &[OTHER_C1]);
        f.write_report("other-report.json", &other_report);
        let other_preflight = client
            .call_ok(
                "awr_completion_prepare",
                json!({
                    "work":"OTHER",
                    "report":"other-report.json",
                    "evidence_key":"other-report",
                    "source_sha":initial_sha,
                    "level":"locally_verified",
                }),
            )
            .await;
        let mut other_evidence = other_preflight["evidence"].clone();
        other_evidence["expected_revision"] = json!(f.revision());
        let recorded_other = client.call_ok("awr_evidence_record", other_evidence).await;
        assert!(recorded_other["evidence"]["section_fingerprint"].is_string());

        let completed_w = client
            .call_ok(
                "awr_work_transition",
                json!({
                    "work":"W",
                    "action":"complete",
                    "session":session_w,
                    "expected_revision":f.revision(),
                    "reason":"Deliver the reviewed primary analysis",
                    "completion":preflight["completion"],
                }),
            )
            .await;
        assert_eq!(completed_w["event"]["event_type"], "work.completed");
        let after_w_sha = f.ledger_sha();
        assert_ne!(after_w_sha, initial_sha);

        let primary_query = client
            .call_ok("awr_work_get", json!({"work":"W","source_sha":after_w_sha}))
            .await;
        assert_eq!(
            evidence_currency(&primary_query, "primary-report"),
            "current"
        );
        assert_eq!(evidence_currency(&primary_query, "git-proof"), "historical");
        let post_w_context = client
            .call_ok(
                "awr_context_compile",
                json!({"work":"W","detached":true,"source_sha":after_w_sha}),
            )
            .await;
        assert!(
            post_w_context["work_context"]["rendered_context"]
                .as_str()
                .unwrap()
                .contains("Current")
        );

        // The report and completion receipt retain the source SHA that was
        // actually verified. Current-SHA work queries below use the unchanged
        // stamped section to reuse this receipt after the ledger drift.
        let other_completion = other_preflight["completion"].clone();
        let completed_other = client
            .call_ok(
                "awr_work_transition",
                json!({
                    "work":"OTHER",
                    "action":"complete",
                    "session":session_other,
                    "expected_revision":f.revision(),
                    "reason":"Deliver the independently reviewed analysis",
                    "completion":other_completion,
                }),
            )
            .await;
        assert_eq!(completed_other["event"]["event_type"], "work.completed");
        let current_sha = f.ledger_sha();
        let current_query = client
            .call_ok("awr_work_get", json!({"work":"W","source_sha":current_sha}))
            .await;
        assert_eq!(
            evidence_currency(&current_query, "primary-report"),
            "current"
        );
        assert_eq!(evidence_currency(&current_query, "git-proof"), "historical");
        let other_query = client
            .call_ok(
                "awr_work_get",
                json!({"work":"OTHER","source_sha":current_sha}),
            )
            .await;
        assert_eq!(evidence_currency(&other_query, "other-report"), "current");
        let status = client
            .call_ok(
                "awr_project_status",
                json!({"view":"full","source_sha":current_sha}),
            )
            .await;
        assert_eq!(status["organization"]["verified_completed"], 2);

        // Verification remains byte-exact and retains the applied receipt's
        // selected proof, so a passing replacement cannot hide a damaged one.
        // The JSON remains a valid passing report; re-parsing alone must not
        // accept bytes that differ from the registered digest.
        let mut changed_report = primary_report.clone();
        changed_report["checks"][0]["details"] = json!("A different passing observation");
        f.write_report("primary-report.json", &changed_report);
        let damaged = client
            .call_ok(
                "awr_project_status",
                json!({"view":"full","source_sha":current_sha}),
            )
            .await;
        assert_eq!(damaged["organization"]["verified_completed"], 1);
        f.write_report("primary-report.json", &primary_report);
        let restored = client
            .call_ok(
                "awr_project_status",
                json!({"view":"full","source_sha":current_sha}),
            )
            .await;
        assert_eq!(restored["organization"]["verified_completed"], 2);

        // A source edit makes read-only calls stale, and a stale expected
        // revision cannot be allowed to record evidence during refresh.
        let old_revision = f.revision();
        let changed = fs::read_to_string(f.root.join("work.yaml"))
            .unwrap()
            .replacen(C2, "A revised acceptance is required", 1);
        fs::write(f.root.join("work.yaml"), changed).unwrap();
        let stale_sha = f.ledger_sha();
        assert_ne!(stale_sha, current_sha);
        client
            .call_error(
                "awr_work_get",
                json!({"work":"W","source_sha":stale_sha}),
                "SourceStale",
            )
            .await;
        let stale_record = client
            .call_error(
                "awr_evidence_record",
                json!({
                    "expected_revision":old_revision,
                    "external_key":"stale-attempt",
                    "work":"W",
                    "evidence_type":"note",
                    "level":"implemented",
                    "summary":"Must not be recorded from a stale revision",
                    "locator":"git-proof.txt",
                    "sha256":fingerprint(b"strict revision proof\n").trim_start_matches("sha256:"),
                    "source_sha":current_sha,
                    "command":"inspect stale source",
                    "scope":["W"],
                    "verified_at":now_millis().unwrap(),
                }),
                "RevisionConflict",
            )
            .await;
        assert_eq!(stale_record["details"]["expected"], old_revision);
        assert!(stale_record["details"]["actual"].as_u64().unwrap() > old_revision);
        let refreshed_sha = f.ledger_sha();
        let changed_query = client
            .call_ok(
                "awr_work_get",
                json!({"work":"W","source_sha":refreshed_sha}),
            )
            .await;
        assert_eq!(
            evidence_currency(&changed_query, "primary-report"),
            "historical"
        );
        assert!(
            changed_query["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["external_key"] != "stale-attempt")
        );
        let changed_status = client
            .call_ok(
                "awr_project_status",
                json!({"view":"full","source_sha":refreshed_sha}),
            )
            .await;
        assert_eq!(changed_status["organization"]["verified_completed"], 1);
        let changed_context = client
            .call_ok(
                "awr_context_compile",
                json!({"work":"W","detached":true,"source_sha":refreshed_sha}),
            )
            .await;
        assert!(
            changed_context["work_context"]["rendered_context"]
                .as_str()
                .unwrap()
                .contains("A revised acceptance is required")
        );

        client.close().await;
    }
}
