use awr_core::*;
use awr_source::{Manifest, index_project};
use awr_store::Store;
use rmcp::{
    RoleClient, ServiceExt, model::*, service::RunningService, transport::TokioChildProcess,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};
use tokio::{process::Command, time::timeout};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CRITERION: &str = "Deliver the reviewed analysis";
const WORK: &str = "work_items:\n- id: W\n  title: Prepare customer analysis\n  status: ready\n  owner: business-coordinator\n  next_action: Draft the analysis\n  depends_on: [D]\n  acceptance: [Deliver the reviewed analysis]\n  verification:\n    evidence_level: none\n  evidence: []\n- id: D\n  title: Required input\n  status: completed\n- id: NEXT\n  title: Prepare follow-up\n  status: ready\n  next_action: Review next steps\n  acceptance: [Follow-up is available]\n";
const MANIFEST: &str = "[project]\nname='MCP fixture'\nexternal_key='mcp-fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n";

#[tokio::test]
async fn action_view_preserves_exact_context_and_compaction_stdio_matches_runtime() {
    let f = Fixture::new();
    let session = f.session("W", true, None).session.id;
    let client = f.client().await;
    let full = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","session":session}),
        )
        .await,
    );
    let action = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","session":session,"response_view":"action"}),
        )
        .await,
    );
    assert_eq!(
        full["context"]["work_context"]["rendered_context"],
        action["context"]["work_context"]["rendered_context"]
    );
    assert_eq!(
        full["context"]["work_context"]["context_hash"],
        action["context"]["work_context"]["context_hash"]
    );
    assert_eq!(
        full["management"]["decision"]["required_actions"],
        action["management"]["decision"]["required_actions"]
    );
    assert!(serde_json::to_vec(&action).unwrap().len() < serde_json::to_vec(&full).unwrap().len());
    assert_eq!(action["response_view"]["view"], "action");
    assert_eq!(
        action["response_view"]["full_result"]["tool"],
        "awr_work_prepare"
    );
    let observation = json!({"compaction_id":"stdio-compact","sequence":1,"observed_at":now_millis().unwrap(),"trigger":"automatic","model":"fixture-model","source":"fixture.full_request","measurement_scope":"full_request","measurement_basis":"host_reported","after_tokens":180000,"context_window_tokens":256000});
    success(
        call(
            &client,
            "awr_compaction_observe",
            json!({"session":session,"expected_revision":f.rev(),"observation":observation}),
        )
        .await,
    );
    let mcp = success(
        call(
            &client,
            "awr_compaction_get",
            json!({"session":session,"include_observation":true}),
        )
        .await,
    );
    let (store, _) = f.store();
    let runtime = awr_runtime::inspect_compaction(
        &store,
        &f.root,
        &awr_runtime::InspectCompactionRequest {
            session,
            include_observation: true,
        },
    )
    .unwrap();
    assert_eq!(runtime, mcp);
    let basic = call(&client, "awr_work_prepare", json!({"work":"W","budget":1})).await;
    let guided = call(
        &client,
        "awr_work_prepare",
        json!({"work":"W","budget":1,"response_view":"action"}),
    )
    .await;
    assert_eq!(body(&basic), body(&guided));
    client.cancel().await.unwrap();
}

struct Fixture {
    root: PathBuf,
}

#[tokio::test]
async fn scoped_context_transport_preserves_semantic_identity_and_opaque_dependency_gaps() {
    let root = std::env::temp_dir().join(format!("awr-scoped-context-mcp-{}", Id::new()));
    fs::create_dir_all(root.join(".awr")).unwrap();
    let work = include_str!("../../../tests/fixtures/workstreams/context.yaml");
    fs::write(root.join("work.yaml"), work).unwrap();
    fs::write(
        root.join("rules.md"),
        "# Shared {#shared severity=hard scope=project value=*}\n\nPreserve approved contracts.\n",
    )
    .unwrap();
    fs::write(
        root.join(".awr/project.toml"),
        include_str!("../../../tests/fixtures/workstreams/context.toml"),
    )
    .unwrap();
    let f = Fixture { root };
    let mut store = Store::open(&f.db()).unwrap();
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
    drop(store);
    let client = f.client().await;
    let before = success(call(&client, "awr_context_compile", json!({"work":"API-1"})).await);
    assert_eq!(before["work_context"]["policy"], "awr.workstream_chunks.v1");
    assert!(
        !serde_json::to_string(&before)
            .unwrap()
            .contains("PRIVATE_CLIENT")
    );
    fs::write(
        f.root.join("work.yaml"),
        work.replace("Implement the client.", "Review the client."),
    )
    .unwrap();
    error(
        call(&client, "awr_context_compile", json!({"work":"API-1"})).await,
        "SourceStale",
    );
    f.reindex();
    let after = success(call(&client, "awr_context_compile", json!({"work":"API-1"})).await);
    for field in ["context_hash", "rendered_context"] {
        assert_eq!(before["work_context"][field], after["work_context"][field]);
    }
    let blocked = call(&client, "awr_context_compile", json!({"work":"CLIENT-1"})).await;
    assert_eq!(blocked.is_error, Some(true));
    let report = body(&blocked);
    assert_eq!(report["completeness"]["dependencies_complete"], false);
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("Implement the interface.")
    );
    client.cancel().await.unwrap();
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("awr-mcp-{}", Id::new()));
        fs::create_dir_all(path.join(".awr")).unwrap();
        let root = path.canonicalize().unwrap();
        fs::create_dir(root.join("decisions")).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(root.join("rules.md"), "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve exact acceptance and source facts.\n").unwrap();
        fs::write(
            root.join("goal.md"),
            "# Deliver useful analysis\n\nPersist work and deliver reviewed customer results.\n",
        )
        .unwrap();
        fs::write(root.join(".awr/project.toml"), MANIFEST).unwrap();
        let f = Self { root };
        let mut store = Store::open(&f.db()).unwrap();
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
    fn db(&self) -> PathBuf {
        self.root.join(".awr/state.db")
    }
    fn store(&self) -> (Store, Project) {
        let store = Store::open_existing(&self.db()).unwrap();
        let project = store.project_by_root(&self.root).unwrap();
        (store, project)
    }
    fn rev(&self) -> Revision {
        self.store().1.project_revision
    }
    fn reindex(&self) {
        let (mut store, _) = self.store();
        assert!(
            index_project(
                &mut store,
                &self.root,
                &Manifest::load(&self.root).unwrap(),
                false
            )
            .unwrap()
            .ok
        );
    }
    fn session(&self, key: &str, claim: bool, branch: Option<Id>) -> SessionStarted {
        let (mut store, p) = self.store();
        store
            .start_session(
                p.id,
                p.project_revision,
                SessionDraft {
                    work_item_key: Some(key.into()),
                    agent_id: format!("executor-{}", Id::new()),
                    provider: "fixture".into(),
                    model: "test".into(),
                    branch_id: branch,
                    claim,
                    claim_ttl_ms: Some(600_000),
                },
            )
            .unwrap()
            .0
    }
    fn logical_state(&self) -> BTreeMap<String, Vec<String>> {
        let conn = rusqlite::Connection::open_with_flags(
            self.db(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let tables = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        tables
            .into_iter()
            .map(|table| {
                let mut stmt = conn
                    .prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))
                    .unwrap();
                let columns = stmt.column_count();
                let mut rows = stmt
                    .query_map([], |r| {
                        (0..columns)
                            .map(|i| r.get_ref(i).map(|v| format!("{v:?}")))
                            .collect::<std::result::Result<Vec<_>, _>>()
                            .map(|v| v.join("|"))
                    })
                    .unwrap()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap();
                rows.sort();
                (table, rows)
            })
            .collect()
    }
    async fn client(&self) -> RunningService<RoleClient, ()> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_awr-mcp"));
        command.arg("--project").arg(&self.root).kill_on_drop(true);
        timeout(
            Duration::from_secs(20),
            ().serve(TokioChildProcess::new(command).unwrap()),
        )
        .await
        .unwrap()
        .unwrap()
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
fn body(result: &CallToolResult) -> Value {
    let value = result
        .structured_content
        .clone()
        .expect("structured result");
    assert_eq!(
        serde_json::from_str::<Value>(&result.content[0].as_text().unwrap().text).unwrap(),
        value
    );
    value
}
fn success(result: CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    body(&result)
}
fn error(result: CallToolResult, expected: &str) -> Value {
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let value = body(&result);
    let code = value
        .get("code")
        .or_else(|| value.get("error").and_then(|e| e.get("code")))
        .unwrap();
    assert_eq!(code, expected, "{value}");
    value
}
fn action(f: &Fixture, session: Id, action: &str) -> Value {
    json!({"work":"W","session":session,"action":action,"expected_revision":f.rev(),"reason":"Apply the reviewed work change"})
}

fn small_observation() -> Value {
    json!({"observed_at":now_millis().unwrap(),"note":"Synthetic bounded analysis with one executor and no deferred work",
        "single_outcome":true,"bounded_scope":true,"single_executor":true,"no_deferred_wait":true,
        "independently_schedulable_units":1,"plan_valid":true,"outcome_known":true})
}

#[tokio::test]
async fn summary_views_preserve_context_and_queryable_full_transition_receipts() {
    let f = Fixture::new();
    let client = f.client().await;
    let session = f.session("W", true, None).session;
    let args = json!({"work":"W","session":session.id});
    let full = success(call(&client, "awr_work_prepare", args.clone()).await);
    let mut concise_args = args.clone();
    concise_args["response_view"] = json!("summary");
    let concise = success(call(&client, "awr_work_prepare", concise_args).await);
    for field in [
        "rendered_context",
        "context_hash",
        "identity",
        "omitted_chunks",
    ] {
        assert_eq!(
            full["context"]["work_context"][field],
            concise["context"]["work_context"][field]
        );
    }
    for field in [
        "management",
        "diagnostics",
        "continuity",
        "active_claims",
        "ready",
        "project_revision",
    ] {
        assert_eq!(full[field], concise[field]);
    }
    assert_eq!(
        full["context"]["completeness"],
        concise["context"]["completeness"]
    );
    assert!(serde_json::to_vec(&concise).unwrap().len() < serde_json::to_vec(&full).unwrap().len());
    let mut transition = action(&f, session.id, "progress");
    transition["next_action"] = json!("Review the draft analysis");
    transition["response_view"] = json!("summary");
    error(
        call(&client, "awr_work_transition", transition.clone()).await,
        "InvalidInput",
    );
    transition["request_id"] = json!("concise-progress");
    let changed = success(call(&client, "awr_work_transition", transition.clone()).await);
    assert_eq!(changed["changes"]["status"], "in_progress");
    assert!(changed["proposal"].get("patch").is_none());
    let revision = f.rev();
    let receipt = success(
        call(
            &client,
            "awr_operation_get",
            json!({"request_id":"concise-progress"}),
        )
        .await,
    );
    assert_eq!(
        receipt["operation"]["result"]["proposal"]["patch"]["changes"]["status"],
        "in_progress"
    );
    transition["response_view"] = json!("full");
    let replay = success(call(&client, "awr_work_transition", transition).await);
    assert_eq!(replay["operation"]["replayed"], true);
    assert!(replay["proposal"]["patch"].is_object());
    assert_eq!(f.rev(), revision);
    let failure = call(&client, "awr_work_prepare", json!({"work":"W","budget":1})).await;
    let concise_failure = call(
        &client,
        "awr_work_prepare",
        json!({"work":"W","budget":1,"response_view":"summary"}),
    )
    .await;
    assert_eq!(failure.is_error, concise_failure.is_error);
    assert_eq!(body(&failure), body(&concise_failure));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn bounded_exploration_can_be_lightweight_with_explicit_observations_and_limits_as_acceptance()
 {
    let f = Fixture::new();
    fs::write(
        f.root.join("work.yaml"),
        WORK.replace(
            "title: Prepare customer analysis",
            "title: Explore source quality\n  kind: exploration",
        )
        .replace(
            "Deliver the reviewed analysis",
            "Explain observations and remaining unknowns",
        ),
    )
    .unwrap();
    f.reindex();
    let client = f.client().await;
    let session = f.session("W", true, None).session;
    let a = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    let saved=success(call(&client,"awr_work_manage",json!({"work":"W","session":session.id,"expected_revision":f.rev(),"request_key":"exploration","contract_fingerprint":a["contract_fingerprint"],"observation":small_observation()})).await);
    assert_eq!(saved["assessment"]["decision"]["mode"], "lightweight");
    let prepared = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","session":session.id}),
        )
        .await,
    );
    assert!(
        prepared["context"]["work_context"]["rendered_context"]
            .as_str()
            .unwrap()
            .contains("Explain observations and remaining unknowns")
    );
    assert_eq!(
        prepared["management"]["decision"]["completion_policy"],
        "unchanged_source_policy"
    );
    client.cancel().await.unwrap();
}
#[tokio::test]
async fn management_preserves_identity_replay_current_contract_and_completion_guards() {
    let f = Fixture::new();
    let client = f.client().await;
    let session = f.session("W", true, None).session;
    let before = fs::read(f.root.join("work.yaml")).unwrap();
    let assessment = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    assert_eq!(assessment["decision"]["mode"], "undetermined");
    let mut args = json!({"work":"W","session":session.id,"expected_revision":f.rev(),"request_key":"small",
        "contract_fingerprint":assessment["contract_fingerprint"],"observation":small_observation()});
    args["observation"]["active_elapsed_ms"] = json!(1800000);
    args["observation"]["completed_rework_cycles"] = json!(3);
    let saved = success(call(&client, "awr_work_manage", args.clone()).await);
    assert_eq!(saved["assessment"]["decision"]["mode"], "lightweight");
    assert_eq!(
        saved["assessment"]["decision"]["reevaluation_signals"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let state = f.logical_state();
    let replay = success(call(&client, "awr_work_manage", args.clone()).await);
    assert_eq!(replay["event"]["id"], saved["event"]["id"]);
    assert_eq!(state, f.logical_state());
    args["observation"]["note"] = json!("changed request content");
    error(
        call(&client, "awr_work_manage", args).await,
        "SourceConflict",
    );
    let current = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    assert_eq!(current["work_id"], assessment["work_id"]);
    assert_eq!(current["decision"]["mode"], "lightweight");
    assert_eq!(current["observer"], session.agent_id);
    error(
        call(
            &client,
            "awr_work_transition",
            action(&f, session.id, "complete"),
        )
        .await,
        "EvidenceMissing",
    );
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), before);
    error(call(&client,"awr_event_append",json!({"expected_revision":f.rev(),"work":"W","session":session.id,"event_type":"management.assessed","summary":"forge a mode","payload":{}})).await,"InvalidInput");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_real_wait_upgrades_the_same_task_and_reply_does_not_downgrade_it() {
    let f = Fixture::new();
    let client = f.client().await;
    let started=success(call(&client,"awr_session_start",json!({"work":"W","conversation":"waiting-management","agent":"coordinator","provider":"fixture","model":"fixture","claim":true,"expected_revision":f.rev()})).await);
    let session: Session = serde_json::from_value(started["session"].clone()).unwrap();
    let assessment = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    success(call(&client,"awr_work_manage",json!({"work":"W","session":session.id,"expected_revision":f.rev(),"request_key":"before-wait","contract_fingerprint":assessment["contract_fingerprint"],"observation":small_observation()})).await);
    let ctx = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","session":session.id,"source_sha":SHA}),
        )
        .await,
    );
    assert!(
        ctx["context"]["work_context"]["rendered_context"]
            .as_str()
            .unwrap()
            .contains(CRITERION)
    );
    let wait=success(call(&client,"awr_session_wait",json!({"session":session.id,"expected_revision":f.rev(),"question":"Which reporting period should this analysis cover?","context_hash":ctx["context"]["work_context"]["context_hash"],"digest":"Input period remains unresolved","next_action":"Continue after the period is supplied","open_loops":["reporting period"]})).await);
    let waiting = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    assert_eq!(waiting["decision"]["mode"], "continuous");
    assert_eq!(waiting["work_id"], assessment["work_id"]);
    success(call(&client,"awr_session_reply",json!({"wait":wait["wait"]["id"],"expected_revision":f.rev(),"reply":"Use the previous full quarter."})).await);
    success(call(&client,"awr_work_manage",json!({"work":"W","session":session.id,"expected_revision":f.rev(),"request_key":"after-wait","contract_fingerprint":waiting["contract_fingerprint"],"observation":small_observation()})).await);
    let after = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    assert_eq!(after["decision"]["mode"], "continuous");
    assert_eq!(
        after["decision"]["completion_policy"],
        "unchanged_source_policy"
    );
    assert!(
        after["decision"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["basis"] == "runtime_history")
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn changed_scope_unknown_results_and_missing_acceptance_remain_explicit() {
    let f = Fixture::new();
    let client = f.client().await;
    let session = f.session("W", true, None).session;
    let first = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    let mut input = json!({"work":"W","session":session.id,"expected_revision":f.rev(),"request_key":"initial","contract_fingerprint":first["contract_fingerprint"],"observation":small_observation()});
    success(call(&client, "awr_work_manage", input.clone()).await);
    let source = WORK
        .replace(
            "acceptance: [Deliver the reviewed analysis]",
            "acceptance: [Explain observations and remaining unknowns]",
        )
        .replace(
            "title: Prepare customer analysis",
            "title: Explore available data\n  kind: exploration",
        );
    fs::write(f.root.join("work.yaml"), &source).unwrap();
    f.reindex();
    let changed = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    assert_eq!(changed["decision"]["mode"], "continuous");
    assert!(changed["observation"].is_null());
    input["expected_revision"] = json!(f.rev());
    input["request_key"] = json!("stale-contract");
    error(
        call(&client, "awr_work_manage", input.clone()).await,
        "SourceConflict",
    );
    input["contract_fingerprint"] = changed["contract_fingerprint"].clone();
    input["observation"]["outcome_known"] = json!(false);
    input["request_key"] = json!("unknown-result");
    let saved = success(call(&client, "awr_work_manage", input.clone()).await);
    assert_eq!(saved["assessment"]["decision"]["mode"], "continuous");
    input["expected_revision"] = json!(f.rev());
    input["request_key"] = json!("known-again");
    input["observation"]["outcome_known"] = json!(true);
    success(call(&client, "awr_work_manage", input).await);
    let retained = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    assert_eq!(retained["decision"]["mode"], "continuous");
    assert_eq!(retained["observation"]["outcome_known"], true);
    fs::write(
        f.root.join("work.yaml"),
        source.replace(
            "acceptance: [Explain observations and remaining unknowns]",
            "acceptance: []",
        ),
    )
    .unwrap();
    f.reindex();
    let missing = success(call(&client, "awr_work_assess", json!({"work":"W"})).await);
    assert!(
        missing["admission_gaps"]
            .as_array()
            .unwrap()
            .contains(&json!("source_acceptance_missing_or_ambiguous"))
    );
    error(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","session":session.id}),
        )
        .await,
        "ContextIncomplete",
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn preparation_composes_the_same_required_context_without_runtime_writes() {
    let f = Fixture::new();
    let client = f.client().await;
    let before = f.logical_state();
    let old_work = success(
        call(
            &client,
            "awr_work_get",
            json!({"work":"W","source_sha":SHA}),
        )
        .await,
    );
    let old_context = success(
        call(
            &client,
            "awr_context_compile",
            json!({"work":"W","detached":true,"source_sha":SHA}),
        )
        .await,
    );
    let new = success(
        call(
            &client,
            "awr_work_prepare",
            json!({"work":"W","source_sha":SHA}),
        )
        .await,
    );
    assert_eq!(new["context"]["work_context"], old_context["work_context"]);
    assert_eq!(new["ready"], old_work["work"]["ready"]);
    assert_eq!(new["context_consumed"], false);
    assert_eq!(new["read_only"], true);
    assert_eq!(f.logical_state(), before);
    error(
        call(&client, "awr_work_prepare", json!({"work":"W","budget":1})).await,
        "BudgetExceeded",
    );
    assert_eq!(f.logical_state(), before);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn report_preflight_derives_bytes_and_coverage_without_recording_evidence() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new();
    let client = f.client().await;
    let before = f.logical_state();
    let mut report = json!({"version":1,"work_item":"W","source_sha":SHA,"command":"review fixture","scope":["W"],"verified_at":now_millis().unwrap(),
        "checks":[{"name":"review","passed":true,"details":"fixture reviewed","criteria":[CRITERION]}]});
    let bytes = serde_json::to_vec(&report).unwrap();
    fs::write(f.root.join("report.json"), &bytes).unwrap();
    let args = json!({"work":"W","report":"report.json","evidence_key":"E","source_sha":SHA,"level":"locally_verified"});
    let result = success(call(&client, "awr_completion_prepare", args.clone()).await);
    assert_eq!(
        result["report_sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    assert_eq!(
        result["completion"]["acceptance"][0]["criterion"],
        CRITERION
    );
    assert_eq!(result["completion_claimed"], false);
    assert_eq!(result["verification_executed"], false);
    report["checks"][0]["criteria"] = json!([]);
    fs::write(f.root.join("report.json"), report.to_string()).unwrap();
    error(
        call(&client, "awr_completion_prepare", args.clone()).await,
        "EvidenceMissing",
    );
    report["checks"][0].as_object_mut().unwrap().remove("name");
    fs::write(f.root.join("report.json"), report.to_string()).unwrap();
    let err = error(
        call(&client, "awr_completion_prepare", args).await,
        "InvalidInput",
    );
    assert_eq!(err["details"]["location"]["locator"], "report.json");
    assert_eq!(f.logical_state(), before);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn source_reindex_exposes_the_same_structured_diagnostic_over_mcp() {
    let f = Fixture::new();
    let client = f.client().await;
    fs::write(
        f.root.join("work.yaml"),
        "work_items:\n- id: W\n  title: [类型错误]\n",
    )
    .unwrap();
    let result = call(
        &client,
        "awr_source_reindex",
        json!({"expected_revision":f.rev()}),
    )
    .await;
    assert_eq!(result.is_error, Some(true));
    let value = body(&result);
    let report = value.get("index").unwrap_or(&value);
    assert_eq!(report["ok"], false);
    assert_eq!(report["projection_complete"], false);
    let details = &report["issues"][0]["details"];
    assert_eq!(details["rule"], "ledger.string");
    assert_eq!(details["location"]["pointer"], "/work_items/0/title");
    assert_eq!(details["location"]["line"], 3);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn project_organization_guides_repairs_and_preserves_readonly_mcp_state() {
    let f = Fixture::new();
    fs::write(
        f.root.join(".awr/project.toml"),
        MANIFEST.replace("[project]\n", "[project]\ncontext_profile='minimal'\n"),
    )
    .unwrap();
    f.reindex();
    let client = f.client().await;
    let before = f.logical_state();
    let initial = success(call(&client, "awr_project_status", json!({"view":"full"})).await);
    assert_eq!(initial["organization"]["state"], "needs_organization");
    assert!(
        initial["organization"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["code"] == "work_goal_missing")
    );
    assert_eq!(initial["organization"]["business_execution_ready"], false);
    assert_eq!(f.logical_state(), before);

    let goal = initial["organization"]["goals"][0]["key"].as_str().unwrap();
    fs::write(f.root.join("work.yaml"), format!("work_items:\n- id: W\n  title: Deliver customer analysis\n  status: ready\n  goal: {goal}\n  acceptance: [Deliver the reviewed analysis]\n  next_action: Draft the requested analysis\n")).unwrap();
    let unchanged = f.logical_state();
    let stale = error(
        call(&client, "awr_project_status", json!({})).await,
        "SourceStale",
    );
    assert_eq!(stale["organization"]["state"], "source_unreadable");
    assert_eq!(stale["organization"]["business_execution_ready"], false);
    assert_eq!(f.logical_state(), unchanged);
    f.reindex();
    let before = f.logical_state();
    let ready = success(call(&client, "awr_project_status", json!({"view":"full"})).await);
    assert_eq!(ready["organization"]["state"], "ready");
    assert_eq!(ready["organization"]["executable_work"], json!(["W"]));
    assert_eq!(ready["suggested_work"]["external_key"], "W");
    let work_ready = success(call(&client, "awr_work_ready", json!({})).await);
    assert_eq!(work_ready["organization"]["state"], "ready");
    assert_eq!(f.logical_state(), before);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn minimal_profile_context_text_agrees_with_completeness_over_stdio() {
    const RULES_SOURCE: &str = "[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n";
    let f = Fixture::new();
    let rendered_gaps = |context: &Value| -> Vec<String> {
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
    };
    // No rules source under an explicit minimal profile: legitimate, so flags and text agree that nothing is missing.
    let manifest = MANIFEST.replace(RULES_SOURCE, "");
    assert_ne!(manifest, MANIFEST);
    fs::write(
        f.root.join(".awr/project.toml"),
        manifest.replace("[project]\n", "[project]\ncontext_profile='minimal'\n"),
    )
    .unwrap();
    f.reindex();
    let client = f.client().await;
    let minimal = success(call(&client, "awr_context_compile", json!({"work":"W"})).await);
    assert_eq!(minimal["completeness"]["complete"], true);
    assert_eq!(minimal["completeness"]["rules_complete"], true);
    assert!(
        minimal["work_context"]["rendered_context"]
            .as_str()
            .unwrap()
            .contains("Context profile: minimal")
    );
    assert_eq!(rendered_gaps(&minimal), Vec::<String>::new());
    client.cancel().await.unwrap();

    // The same project under the standard profile is refused, in the flags and in the text.
    fs::write(f.root.join(".awr/project.toml"), &manifest).unwrap();
    f.reindex();
    let client = f.client().await;
    let refused = error(
        call(&client, "awr_context_compile", json!({"work":"W"})).await,
        "ContextIncomplete",
    );
    assert_eq!(refused["completeness"]["complete"], false);
    assert_eq!(refused["completeness"]["rules_complete"], false);
    assert!(
        refused["completeness"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "rules_source_missing")
    );
    assert_eq!(
        rendered_gaps(&refused),
        [
            "CONTEXT INCOMPLETE (hard subset)",
            "rules source is missing"
        ]
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_discovers_tools_and_survives_protocol_and_argument_errors() {
    let f = Fixture::new();
    let before = f.logical_state();
    let client = f.client().await;
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), awr_mcp::domains::DOMAINS.len());
    let names = tools
        .iter()
        .map(|t| t.name.as_ref())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names,
        awr_mcp::domains::DOMAINS
            .iter()
            .map(|d| d.name)
            .collect::<std::collections::BTreeSet<_>>()
    );
    assert!(
        serde_json::to_vec(&tools).unwrap().len() < 32_768,
        "catalog must stay compact: {} bytes",
        serde_json::to_vec(&tools).unwrap().len()
    );
    for tool in &tools {
        let annotations = tool.annotations.as_ref().unwrap();
        // Domain entries can dispatch both reads and writes, so they are not
        // advertised as read-only; exact child semantics stay in the manifest.
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(tool.input_schema["additionalProperties"], false);
    }
    assert!(
        client
            .call_tool(CallToolRequestParams::new("awr_unlisted"))
            .await
            .is_err()
    );
    error(
        call(&client, "awr_work_ready", json!({"limit":0})).await,
        "InvalidInput",
    );
    error(
        call(
            &client,
            "awr_project_status",
            json!({"project":"elsewhere"}),
        )
        .await,
        "InvalidInput",
    );
    error(
        call(&client, "awr_work_get", json!({"work":"MISSING"})).await,
        "NotFound",
    );
    success(call(&client, "awr_project_status", json!({})).await);
    client.cancel().await.unwrap();
    assert_eq!(f.logical_state(), before);
}

#[tokio::test]
async fn work_edit_shortcut_keeps_readonly_preview_review_and_original_journal() {
    let f = Fixture::new();
    let client = f.client().await;
    let before = f.logical_state();
    let bytes = fs::read(f.root.join("work.yaml")).unwrap();
    let input = json!({"request_id":"small-edit","reason":"Clarify the task title","change":{"kind":"work_edit","work":"W","fields":{"title":"Reviewed analysis"}}});
    let preview = success(call(&client, "awr_change_preview", input.clone()).await);
    assert_eq!(preview["view"], "work_edit");
    assert_eq!(preview["read_only"], true);
    assert!(preview.get("preview").is_none());
    assert_eq!(f.logical_state(), before);
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), bytes);
    let mut apply = input.clone();
    apply["expected_revision"] = preview["project_revision"].clone();
    apply["expected_preview"] = preview["preview_fingerprint"].clone();
    // Applying an unbound shortcut is rejected even if its preview hash is known.
    error(
        call(&client, "awr_change_apply", apply.clone()).await,
        "InvalidInput",
    );
    apply["change"] = preview["change"].clone();
    let saved = success(call(&client, "awr_change_apply", apply.clone()).await);
    assert_eq!(saved["write_outcome"], "applied");
    assert_eq!(
        success(call(&client, "awr_change_apply", apply).await)["already_recorded"],
        true
    );
    let status = success(
        call(
            &client,
            "awr_change_status",
            json!({"kind":"work_edit","request_id":"small-edit"}),
        )
        .await,
    );
    assert_eq!(status["found"], true);
    assert_eq!(status["proposal_id"], saved["proposal_id"]);
    let (store, project) = f.store();
    let work = store.work_item(project.id, "W").unwrap();
    assert_eq!(work.item.title, "Reviewed analysis");
    assert_eq!(work.item.status, awr_core::WorkStatus::Ready);
    for fields in [
        json!({"status":"completed"}),
        json!({"verification":{"level":"verified"}}),
        json!({"owner":"other-agent"}),
    ] {
        error(call(&client,"awr_change_preview",json!({"request_id":"bad-edit","reason":"Attempt unsupported field","change":{"kind":"work_edit","work":"W","fields":fields}})).await,"MutationUnsupported");
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn source_preview_create_edit_and_graph_use_one_durable_domain_identity() {
    let f = Fixture::new();
    let client = f.client().await;
    let before = f.logical_state();
    let source_before = fs::read(f.root.join("work.yaml")).unwrap();
    let input = json!({"request_id":"reviewed-follow-up","reason":"Add a bounded follow-up draft","change":{"kind":"create","title":"Review the customer follow-up","fields":{"depends_on":["W"],"next_action":"Review the draft","acceptance":["The follow-up has a reviewed conclusion"]}}});
    let preview = success(call(&client, "awr_change_preview", input.clone()).await);
    assert_eq!(preview["read_only"], true);
    assert_eq!(preview["runtime_write_performed"], false);
    assert_eq!(f.logical_state(), before);
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), source_before);
    let mut apply = input.clone();
    apply["expected_revision"] = preview["project_revision"].clone();
    apply["expected_preview"] = preview["preview"]["fingerprint"].clone();
    let created = success(call(&client, "awr_change_apply", apply.clone()).await);
    assert_eq!(created["status"], "completed");
    let revision = f.rev();
    assert_eq!(
        success(call(&client, "awr_change_apply", apply.clone()).await)["already_recorded"],
        true
    );
    assert_eq!(f.rev(), revision);
    let key = created["external_key"].as_str().unwrap();
    let graph = success(call(&client, "awr_work_graph", json!({"roots":["W"]})).await);
    let nodes = graph["nodes"].as_array().unwrap();
    assert!(
        nodes
            .iter()
            .any(|n| n["key"] == key && n["status"] == "draft" && n["ready"] == false)
    );
    assert!(nodes.iter().any(|n| n["key"] == "D"));
    assert!(!nodes.iter().any(|n| n["key"] == "NEXT"));
    assert!(
        graph["affected"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == key)
    );
    error(
        call(&client, "awr_work_graph", json!({"roots":["W"],"limit":1})).await,
        "BudgetExceeded",
    );
    apply["change"]["title"] = json!("Different request body");
    error(
        call(&client, "awr_change_apply", apply).await,
        "SourceConflict",
    );
    let status = success(
        call(
            &client,
            "awr_change_status",
            json!({"kind":"create","request_id":"reviewed-follow-up"}),
        )
        .await,
    );
    assert_eq!(status["external_key"], key);
    assert_eq!(status["journal"], "source_change");
    let work = f.store().0.work_item(f.store().1.id, key).unwrap();
    let edit = json!({"request_id":"refine-follow-up","reason":"Clarify the next review action","change":{"kind":"edit","change":{"operation":"fields","kind":"work_item","target":key,"source_fingerprint":work.source.fingerprint,"fields":{"next_action":"Review the conclusion with the customer"}}}});
    let p = success(call(&client, "awr_change_preview", edit.clone()).await);
    let mut edit_apply = edit;
    edit_apply["expected_revision"] = p["project_revision"].clone();
    edit_apply["expected_preview"] = p["preview"]["fingerprint"].clone();
    success(call(&client, "awr_change_apply", edit_apply).await);
    assert_eq!(
        f.store()
            .0
            .work_item(f.store().1.id, key)
            .unwrap()
            .item
            .next_action,
        "Review the conclusion with the customer"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn whole_graph_preflight_rejects_missing_cycles_and_claimed_contract_changes() {
    let f = Fixture::new();
    let client = f.client().await;
    let source = f.store().0.work_item(f.store().1.id, "W").unwrap().source;
    let request = |id: &str, operations: Value| json!({"request_id":id,"reason":"Review the dependency plan","change":{"kind":"batch","change":{"kind":"ledger","source_id":source.id,"source_fingerprint":source.fingerprint,"operations":operations}}});
    let before = f.logical_state();
    let source_before = fs::read(f.root.join("work.yaml")).unwrap();
    let missing = error(
        call(
            &client,
            "awr_change_preview",
            request(
                "missing",
                json!([{"operation":"fields","target":"W","fields":{"depends_on":["absent"]}}]),
            ),
        )
        .await,
        "InvalidInput",
    );
    assert_eq!(missing["details"]["rule"], "work_graph.required_reference");
    assert!(
        missing["details"]["location"]["locator"]
            .as_str()
            .unwrap()
            .contains("work.yaml")
    );
    let cycle = error(
        call(
            &client,
            "awr_change_preview",
            request(
                "cycle",
                json!([{"operation":"fields","target":"D","fields":{"depends_on":["W"]}}]),
            ),
        )
        .await,
        "InvalidInput",
    );
    assert_eq!(cycle["details"]["rule"], "work_graph.acyclic");
    assert_eq!(f.logical_state(), before);
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), source_before);
    let input = request(
        "ordered-plan",
        json!([
            {"operation":"import","external_key":"FOLLOW","title":"Review follow-up","duplicate":"fail","fields":{"depends_on":["RESEARCH"]}},
            {"operation":"import","external_key":"RESEARCH","title":"Collect new observations","duplicate":"fail","fields":{}}
        ]),
    );
    let p = success(call(&client, "awr_change_preview", input.clone()).await);
    let mut apply = input;
    apply["expected_revision"] = p["project_revision"].clone();
    apply["expected_preview"] = p["preview"]["fingerprint"].clone();
    success(call(&client, "awr_change_apply", apply).await);
    let session = f.session("W", true, None);
    let current = f.store().0.work_item(f.store().1.id, "W").unwrap();
    let edit = json!({"request_id":"claimed-plan","reason":"Change the executing contract","change":{"kind":"edit","change":{"operation":"fields","kind":"work_item","target":"W","source_fingerprint":current.source.fingerprint,"fields":{"acceptance":["A changed delivery promise"]}}}});
    error(
        call(&client, "awr_change_preview", edit).await,
        "ClaimConflict",
    );
    assert!(
        f.store()
            .0
            .work_readiness(f.store().1.id, "W", None, now_millis().unwrap())
            .unwrap()
            .active_claims
            .iter()
            .any(|c| c.session_id == session.session.id)
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn all_five_reads_preserve_sources_and_every_runtime_and_search_table() {
    let f = Fixture::new();
    let session = f.session("W", true, None);
    let before = f.logical_state();
    let file = fs::read(f.root.join("work.yaml")).unwrap();
    let client = f.client().await;
    let status = success(call(&client, "awr_project_status", json!({})).await);
    assert_eq!(status["total"], 3);
    let ready = success(call(&client, "awr_work_ready", json!({})).await);
    assert_eq!(ready["ready_total"], 1);
    let work = success(
        call(
            &client,
            "awr_work_get",
            json!({"work":"W","source_sha":SHA}),
        )
        .await,
    );
    assert_eq!(work["acceptance"], json!([CRITERION]));
    assert_eq!(
        work["work"]["active_claims"][0]["session_id"],
        json!(session.session.id)
    );
    let context = success(
        call(
            &client,
            "awr_context_compile",
            json!({"work":"W","session":session.session.id,"source_sha":SHA,"budget":5000}),
        )
        .await,
    );
    assert_eq!(context["completeness"]["complete"], true);
    assert!(
        context["work_context"]["rendered_context"]
            .as_str()
            .unwrap()
            .contains("Preserve exact acceptance and source facts.")
    );
    assert!(context["work_context"]["token_estimate"].as_u64().unwrap() <= 5000);
    let again = success(
        call(
            &client,
            "awr_context_compile",
            json!({"work":"W","session":session.session.id,"source_sha":SHA,"budget":5000}),
        )
        .await,
    );
    assert_eq!(
        again["work_context"]["context_hash"],
        context["work_context"]["context_hash"]
    );
    let found = success(
        call(
            &client,
            "awr_search",
            json!({"text":"customer","kind":"work"}),
        )
        .await,
    );
    assert_eq!(found["hits"][0]["external_key"], "W");
    for value in [status, ready, work, context, found] {
        assert_eq!(value["project_revision"], f.rev());
        assert_eq!(value["read_only"], true);
        assert_eq!(value["source_refresh_performed"], false);
    }
    client.cancel().await.unwrap();
    assert_eq!(fs::read(f.root.join("work.yaml")).unwrap(), file);
    assert_eq!(f.logical_state(), before);
}

#[tokio::test]
async fn stale_files_new_directory_sources_and_changed_mappings_require_explicit_reindex() {
    let f = Fixture::new();
    let client = f.client().await;
    let before = f.logical_state();
    fs::write(
        f.root.join("work.yaml"),
        WORK.replace("Draft the analysis", "Review revised facts"),
    )
    .unwrap();
    for (name, args) in [
        ("awr_project_status", json!({})),
        ("awr_work_ready", json!({})),
        ("awr_work_get", json!({"work":"W"})),
        ("awr_context_compile", json!({"work":"W"})),
        ("awr_search", json!({"text":"analysis"})),
    ] {
        error(call(&client, name, args).await, "SourceStale");
        assert_eq!(f.logical_state(), before);
    }
    f.reindex();
    let work = success(call(&client, "awr_work_get", json!({"work":"W"})).await);
    assert_eq!(work["work"]["next_action"], "Review revised facts");
    let before = f.logical_state();
    fs::write(
        f.root.join("decisions/new.md"),
        "# A newly discovered decision\n\n## Decision\n\nKeep source authority.\n",
    )
    .unwrap();
    error(
        call(&client, "awr_project_status", json!({})).await,
        "SourceStale",
    );
    assert_eq!(f.logical_state(), before);
    f.reindex();
    let before = f.logical_state();
    fs::write(
        f.root.join(".awr/project.toml"),
        MANIFEST.replace("status='active'", "status='planned'"),
    )
    .unwrap();
    error(
        call(&client, "awr_context_compile", json!({"work":"W"})).await,
        "SourceStale",
    );
    assert_eq!(f.logical_state(), before);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn incomplete_context_keeps_diagnostics_and_budget_errors_are_tool_errors() {
    let f = Fixture::new();
    let client = f.client().await;
    let before = f.logical_state();
    let missing = error(
        call(&client, "awr_context_compile", json!({"work":"MISSING"})).await,
        "ContextIncomplete",
    );
    assert_eq!(missing["completeness"]["complete"], false);
    assert!(missing["work_context"].is_null());
    assert!(
        missing["diagnostic_text"]
            .as_str()
            .unwrap()
            .contains("CONTEXT INCOMPLETE")
    );
    error(
        call(
            &client,
            "awr_context_compile",
            json!({"work":"W","budget":1}),
        )
        .await,
        "BudgetExceeded",
    );
    error(
        call(
            &client,
            "awr_context_compile",
            json!({"work":"W","checkpoint":Id::new(),"after_revision":0}),
        )
        .await,
        "InvalidInput",
    );
    assert_eq!(f.logical_state(), before);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn source_actions_use_owned_claims_revision_guards_and_recovery_receipts() {
    let f = Fixture::new();
    let started = f.session("W", true, None);
    let sid = started.session.id;
    let client = f.client().await;
    let before = f.logical_state();
    let mut stale = action(&f, sid, "progress");
    stale["expected_revision"] = json!(f.rev() - 1);
    stale["next_action"] = json!("Review the draft");
    error(
        call(&client, "awr_work_transition", stale).await,
        "RevisionConflict",
    );
    let mut wrong = action(&f, Id::new(), "progress");
    wrong["next_action"] = json!("Review the draft");
    error(
        call(&client, "awr_work_transition", wrong).await,
        "NotFound",
    );
    assert_eq!(f.logical_state(), before);
    for (kind, field, text, status) in [
        ("progress", "next_action", "Review the draft", "in_progress"),
        ("block", "blocker", "Await customer input", "blocked"),
        (
            "unblock",
            "next_action",
            "Review restored input",
            "in_progress",
        ),
        ("cancel", "next_action", "Record cancellation", "cancelled"),
        ("reopen", "next_action", "Replan analysis", "planned"),
    ] {
        let mut args = action(&f, sid, kind);
        args[field] = json!(text);
        let result = success(call(&client, "awr_work_transition", args).await);
        assert_eq!(result["proposal"]["status"], "applied");
        assert!(
            f.root
                .join(result["recovery_directory"].as_str().unwrap())
                .join("before.yaml")
                .is_file()
        );
        let work = success(call(&client, "awr_work_get", json!({"work":"W"})).await);
        assert_eq!(work["work"]["status"], status);
        assert_eq!(work["work"]["owner"], "business-coordinator");
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn completion_requires_actual_bound_evidence_and_releases_only_the_owned_claim() {
    let f = Fixture::new();
    let started = f.session("W", true, None);
    let sid = started.session.id;
    let other = f.session("NEXT", true, None);
    let client = f.client().await;
    let mut progress = action(&f, sid, "progress");
    progress["next_action"] = json!("Review completed analysis");
    success(call(&client, "awr_work_transition", progress).await);
    let before = f.logical_state();
    error(
        call(&client, "awr_work_transition", action(&f, sid, "complete")).await,
        "EvidenceMissing",
    );
    let mut args = action(&f, sid, "complete");
    args["completion"] = json!({"version":1,"source_sha":SHA,"acceptance":[{"criterion":CRITERION,"evidence":["E"]}]});
    error(
        call(&client, "awr_work_transition", args.clone()).await,
        "NotFound",
    );
    assert_eq!(f.logical_state(), before);
    let report = json!({"version":1,"work_item":"W","source_sha":SHA,"command":"review customer analysis","scope":["W"],"verified_at":now_millis().unwrap(),"checks":[{"name":"review and delivery","passed":true,"details":"Reviewed the fixture analysis and receipt","criteria":[CRITERION]}]});
    let bytes = serde_json::to_vec(&report).unwrap();
    fs::write(f.root.join("report.json"), &bytes).unwrap();
    let digest = awr_source::fingerprint(&bytes)
        .trim_start_matches("sha256:")
        .to_owned();
    let recorded=success(call(&client,"awr_evidence_record",json!({"expected_revision":f.rev(),"external_key":"E","work":"W","evidence_type":"completion_report","level":"locally_verified","summary":"Reviewed the fixture analysis","locator":"report.json","sha256":digest,"source_sha":SHA,"command":"review customer analysis","scope":["W"],"verified_at":report["verified_at"]})).await);
    assert_eq!(recorded["validation_basis"], "caller_supplied_bindings");
    let before = f.logical_state();
    fs::write(f.root.join("report.json"), "changed report").unwrap();
    args["expected_revision"] = json!(f.rev());
    let failed = call(&client, "awr_work_transition", args.clone()).await;
    assert_eq!(failed.is_error, Some(true));
    assert_eq!(f.logical_state(), before);
    fs::write(f.root.join("report.json"), &bytes).unwrap();
    args["expected_revision"] = json!(f.rev());
    let result = success(call(&client, "awr_work_transition", args).await);
    assert_eq!(result["event"]["event_type"], "work.completed");
    assert_eq!(
        result["event"]["payload"]["released_claim_ids"],
        json!([started.claim.unwrap().id])
    );
    let (store, p) = f.store();
    assert_eq!(
        store.work_item(p.id, "W").unwrap().item.status,
        WorkStatus::Completed
    );
    assert_eq!(
        store.session(p.id, other.session.id).unwrap().status,
        "active"
    );
    assert_eq!(
        store
            .work_readiness(p.id, "NEXT", None, now_millis().unwrap())
            .unwrap()
            .active_claims[0]
            .id,
        other.claim.unwrap().id
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn generic_events_are_bounded_reserved_and_branch_bound_and_commands_are_inert() {
    let f = Fixture::new();
    let (mut store, p) = f.store();
    let created = awr_runtime::create_branch(
        &mut store,
        &f.root,
        &awr_runtime::CreateBranchRequest {
            name: "review".into(),
            parent: Some("main".into()),
            git_ref: None,
            expected_revision: p.project_revision,
            actor: "fixture".into(),
            reason: "Review isolated work".into(),
        },
    )
    .unwrap();
    drop(store);
    let bid = created.0.id;
    let client = f.client().await;
    let before = f.logical_state();
    error(call(&client,"awr_event_append",json!({"expected_revision":f.rev(),"event_type":"work.completed","summary":"Bypass completion"})).await,"InvalidInput");
    assert_eq!(f.logical_state(), before);
    let recorded=success(call(&client,"awr_event_append",json!({"expected_revision":f.rev(),"work":"W","branch":"review","event_type":"work.observed","importance":"critical","summary":"Review found a useful next step","payload":{"body":"UNINDEXED_PAYLOAD_SENTINEL"}})).await);
    assert_eq!(recorded["event"]["branch_id"], json!(bid));
    let before = f.logical_state();
    let context = success(
        call(
            &client,
            "awr_context_compile",
            json!({"work":"W","branch":"review","detached":true}),
        )
        .await,
    );
    assert!(
        context["work_context"]["rendered_context"]
            .as_str()
            .unwrap()
            .contains("Review found a useful next step")
    );
    let main = success(
        call(
            &client,
            "awr_context_compile",
            json!({"work":"W","branch":"main","detached":true}),
        )
        .await,
    );
    assert!(
        !serde_json::to_string(&main)
            .unwrap()
            .contains("Review found a useful next step")
    );
    let search = success(
        call(
            &client,
            "awr_search",
            json!({"text":"UNINDEXED_PAYLOAD_SENTINEL"}),
        )
        .await,
    );
    assert!(search["hits"].as_array().unwrap().is_empty());
    assert_eq!(f.logical_state(), before);
    let output = f.root.join("must-not-execute");
    success(call(&client,"awr_evidence_record",json!({"expected_revision":f.rev(),"external_key":"observation","work":"W","branch":"review","evidence_type":"note","level":"implemented","summary":"Record an observation","locator":"unread-report.json","scope":["W"],"command":format!("touch {}",output.display())})).await);
    assert!(!output.exists());
    let before = f.logical_state();
    error(call(&client,"awr_evidence_record",json!({"expected_revision":f.rev(),"external_key":"observation","work":"W","branch":"review","evidence_type":"note","level":"implemented","summary":"Duplicate","locator":"unread-report.json","scope":["W"]})).await,"SourceConflict");
    assert_eq!(f.logical_state(), before);
    client.cancel().await.unwrap();
}

#[test]
fn snapshot_limit_and_startup_do_not_initialize_or_modify_a_project() {
    let f = Fixture::new();
    let before = f.logical_state();
    let (store, _) = f.store();
    assert!(store.memory_snapshot(1).is_err());
    let snapshot = store.memory_snapshot(256 * 1024 * 1024).unwrap();
    assert_eq!(
        snapshot.project_by_root(&f.root).unwrap().project_revision,
        f.rev()
    );
    assert_eq!(f.logical_state(), before);
    let empty = f.root.join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(awr_mcp::AwrServer::open(&empty).is_ok());
    assert!(!empty.join(".awr").exists());
}

#[tokio::test]
async fn uninitialized_mcp_project_returns_intake_guidance_without_creating_state() {
    let root = std::env::temp_dir().join(format!("awr-mcp-empty-{}", Id::new()));
    fs::create_dir(&root).unwrap();
    let f = Fixture {
        root: root.canonicalize().unwrap(),
    };
    let client = f.client().await;
    let status = error(
        call(&client, "awr_project_status", json!({})).await,
        "NotFound",
    );
    assert_eq!(status["organization"]["state"], "not_initialized");
    assert_eq!(status["organization"]["business_execution_ready"], false);
    assert!(status["total"].is_null());
    assert!(
        status["organization"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == "sources")
    );
    assert!(!f.root.join(".awr").exists());
    error(call(&client,"awr_work_transition",json!({"action":"progress","work":"W","session":Id::new(),"expected_revision":0,"reason":"Record current progress","next_action":"Continue the task"})).await,"NotFound");
    assert!(!f.root.join(".awr").exists());
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn mutation_source_refresh_preserves_human_edits_and_requires_a_reviewed_revision() {
    for name in [
        "awr_event_append",
        "awr_evidence_record",
        "awr_work_transition",
    ] {
        let f = Fixture::new();
        let started = f.session("W", true, None);
        let client = f.client().await;
        let revision = f.rev();
        let human = WORK.replace("Draft the analysis", "Preserve this human source edit");
        fs::write(f.root.join("work.yaml"), &human).unwrap();
        let args = match name {
            "awr_event_append" => {
                json!({"expected_revision":revision,"event_type":"work.observed","summary":"Candidate observation"})
            }
            "awr_evidence_record" => {
                json!({"expected_revision":revision,"external_key":"candidate","work":"W","evidence_type":"note","level":"implemented","summary":"Candidate note","locator":"report.json","scope":["W"]})
            }
            _ => {
                json!({"expected_revision":revision,"work":"W","session":started.session.id,"action":"progress","reason":"Advance reviewed work","next_action":"Review the draft"})
            }
        };
        if name == "awr_work_transition" {
            error(call(&client, name, args).await, "SourceConflict");
            assert_eq!(f.rev(), revision);
            error(
                call(&client, "awr_project_status", json!({})).await,
                "SourceStale",
            );
            f.reindex();
        } else {
            error(call(&client, name, args).await, "RevisionConflict");
        }
        assert_eq!(fs::read_to_string(f.root.join("work.yaml")).unwrap(), human);
        assert!(f.rev() > revision);
        let (store, p) = f.store();
        let work = store.work_item(p.id, "W").unwrap();
        assert_eq!(work.item.status, WorkStatus::Ready);
        assert_eq!(work.item.next_action, "Preserve this human source edit");
        assert!(store.evidence(p.id, "candidate").is_err());
        success(call(&client, "awr_project_status", json!({})).await);
        client.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn legacy_stdio_negotiation_keeps_protocol_stdout_and_clean_eof() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let f = Fixture::new();
    let before = f.logical_state();
    let mut child = Command::new(env!("CARGO_BIN_EXE_awr-mcp"))
        .arg("--project")
        .arg(&f.root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"legacy-fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"awr_project_status","arguments":{}}}),
    ];
    for request in requests {
        input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        input.flush().await.unwrap();
        let mut line = String::new();
        timeout(Duration::from_secs(20), output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], request["id"]);
        assert!(reply.get("error").is_none(), "{reply}");
        match request["id"].as_i64().unwrap() {
            1 => {
                assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
                input
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
                    .await
                    .unwrap();
                input.flush().await.unwrap();
            }
            2 => assert_eq!(
                reply["result"]["tools"].as_array().unwrap().len(),
                awr_mcp::domains::DOMAINS.len()
            ),
            _ => assert_eq!(reply["result"]["structuredContent"]["read_only"], true),
        }
    }
    drop(input);
    assert!(
        timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert_eq!(f.logical_state(), before);
}

#[tokio::test]
async fn two_level_progressive_disclosure_shrinks_startup_and_routes_children() {
    let f = Fixture::new();
    let client = f.client().await;
    // Level 1: only bounded domains are advertised by default.
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), awr_mcp::domains::DOMAINS.len());
    for tool in &tools {
        assert!(awr_mcp::domains::is_public_domain(&tool.name));
    }
    let advertised = serde_json::to_vec(&tools).unwrap().len();
    assert!(
        advertised < 8192,
        "level-1 catalog must stay bounded: {advertised}"
    );
    // Level 2: empty discovery returns exact child names and schemas.
    let manifest = success(call(&client, "awr_query", json!({})).await);
    assert_eq!(manifest["mode"], "manifest");
    assert_eq!(manifest["domain"], "awr_query");
    assert_eq!(manifest["child_total"], 4);
    let names: Vec<&str> = manifest["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "awr_project_status",
            "awr_work_ready",
            "awr_work_get",
            "awr_search"
        ]
    );
    assert!(manifest["children"][0]["input_schema"]["properties"].is_object());
    // Level 3: child execution routes through the domain entry.
    let ready = success(
        call(
            &client,
            "awr_query",
            json!({"child_tool":"awr_work_ready","arguments":{}}),
        )
        .await,
    );
    assert_eq!(ready["ready_total"], 2);
    // Integrated hosts may still call the flat child name directly.
    let direct = success(call(&client, "awr_work_ready", json!({})).await);
    assert_eq!(direct["ready_total"], 2);
    // Child names outside the domain are rejected without dispatch.
    let error = timeout(
        Duration::from_secs(30),
        client.call_tool(
            CallToolRequestParams::new("awr_query").with_arguments(
                json!({"child_tool":"awr_session_start","arguments":{}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ),
    )
    .await
    .unwrap();
    assert!(error.is_err());
    assert!(
        error
            .unwrap_err()
            .to_string()
            .contains("child_tool is not a member"),
        "expected membership rejection"
    );
}
