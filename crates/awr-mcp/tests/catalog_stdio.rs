//! `awr-mcp --catalog`: one stdio server that resolves projects from the per-user local
//! catalog on every call. Each test isolates the catalog with `AWR_LOCAL_HOME` and turns the
//! background hub off with `AWR_LOCAL_WEB=0`, so nothing touches the real user state.
use awr_source::{Manifest, index_project};
use awr_store::Store;
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{process::Command, time::timeout};

type Client = RunningService<RoleClient, ()>;

const WORK: &str = "work_items:\n- id: W\n  title: Prepare analysis\n  status: ready\n  next_action: Draft the analysis\n  acceptance: [Deliver the analysis]\n";
const MANIFEST: &str = "[project]\nname='MCP catalog fixture'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n";

struct Lab {
    base: PathBuf,
}
impl Lab {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("awr-mcp-catalog-{}", awr_core::Id::new()));
        fs::create_dir_all(&base).unwrap();
        Self {
            base: base.canonicalize().unwrap(),
        }
    }
    fn state(&self) -> PathBuf {
        self.base.join("user-state")
    }
    /// An indexed AWR project under `parent`/`dir`, whose database says its name is `name`.
    fn project_in(&self, parent: &Path, dir: &str, name: &str) -> PathBuf {
        let root = parent.join(dir);
        fs::create_dir_all(root.join(".awr")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(
            root.join("rules.md"),
            "# Authority {#authority severity=hard scope=project value=*}\n\nKeep exact facts.\n",
        )
        .unwrap();
        fs::write(
            root.join("goal.md"),
            "# Deliver analysis\n\nPersist work.\n",
        )
        .unwrap();
        let manifest = MANIFEST.replace("MCP catalog fixture", name);
        fs::write(root.join(".awr/project.toml"), manifest).unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        assert!(
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false)
                .unwrap()
                .ok
        );
        root
    }
    fn project(&self, name: &str) -> PathBuf {
        self.project_in(&self.base.clone(), name, name)
    }
    fn register(&self, root: &Path) -> awr_local::LocalProject {
        awr_local::register(&self.state(), root).unwrap()
    }
    /// Start `awr-mcp --catalog` with the given extra arguments and working directory.
    async fn client(&self, args: &[&str], cwd: &Path) -> Client {
        let mut command = Command::new(env!("CARGO_BIN_EXE_awr-mcp"));
        command
            .arg("--catalog")
            .args(args)
            .current_dir(cwd)
            .env("AWR_LOCAL_HOME", self.state())
            .env("AWR_LOCAL_WEB", "0")
            .kill_on_drop(true);
        timeout(
            Duration::from_secs(20),
            ().serve(TokioChildProcess::new(command).unwrap()),
        )
        .await
        .unwrap()
        .unwrap()
    }
    fn empty_dir(&self) -> PathBuf {
        let dir = self.base.join("not-a-project");
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

async fn call(client: &Client, name: &str, args: Value) -> CallToolResult {
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
    result
        .structured_content
        .clone()
        .expect("structured result")
}
fn ok(result: CallToolResult) -> Value {
    assert_ne!(
        result.is_error,
        Some(true),
        "{:?}",
        result.structured_content
    );
    body(&result)
}
fn refused(result: CallToolResult) -> Value {
    assert_eq!(
        result.is_error,
        Some(true),
        "{:?}",
        result.structured_content
    );
    body(&result)
}
/// The project a status call answered for.
fn who(status: &Value) -> String {
    status["project_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no project_id in {status}"))
        .to_owned()
}
fn names(list: &Value) -> Vec<String> {
    list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_project_registered_after_the_server_started_is_usable_without_a_restart() {
    let lab = Lab::new();
    let first = lab.project("alpha");
    lab.register(&first);
    let client = lab.client(&[], &lab.empty_dir()).await;
    assert_eq!(
        names(&ok(call(&client, "awr_projects_list", json!({})).await)),
        ["alpha"]
    );

    let later = lab.project("beta");
    let entry = lab.register(&later);
    let listed = ok(call(&client, "awr_projects_list", json!({})).await);
    assert_eq!(listed["total"], 2);
    let status = ok(call(&client, "awr_project_status", json!({"project": entry.key})).await);
    assert_eq!(who(&status), entry.project_id.unwrap());
}

#[tokio::test]
async fn a_project_is_selected_by_key_unique_name_or_root() {
    let lab = Lab::new();
    let root = lab.project("alpha");
    let entry = lab.register(&root);
    let client = lab.client(&[], &lab.empty_dir()).await;
    for selector in [
        entry.key.as_str(),
        &entry.key[..10],
        "alpha",
        root.to_str().unwrap(),
    ] {
        let status = ok(call(&client, "awr_project_status", json!({"project": selector})).await);
        assert_eq!(
            who(&status),
            entry.project_id.clone().unwrap(),
            "{selector}"
        );
    }
}

#[tokio::test]
async fn unknown_ambiguous_unregistered_and_hidden_projects_are_refused() {
    let lab = Lab::new();
    let one = lab.project_in(&lab.base.clone(), "one", "twin");
    let two = lab.project_in(&lab.base.clone(), "two", "twin");
    let hidden = lab.project("hidden");
    let entry_one = lab.register(&one);
    lab.register(&two);
    let hidden_key = lab.register(&hidden).key;
    // A project that only discovery or an earlier version recorded: in the catalog, never registered.
    let legacy = lab.project("legacy");
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(lab.state().join("projects.json")).unwrap()).unwrap();
    let mut entry = serde_json::to_value(awr_local::inspect(&legacy).unwrap()).unwrap();
    entry["key"] = json!("f".repeat(64));
    catalog["projects"].as_array_mut().unwrap().push(entry);
    fs::write(
        lab.state().join("projects.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    fs::write(
        lab.state().join("hidden-projects.json"),
        serde_json::to_vec(&json!({"version": 1, "keys": [hidden_key]})).unwrap(),
    )
    .unwrap();

    let client = lab.client(&[], &lab.empty_dir()).await;
    let listed = ok(call(&client, "awr_projects_list", json!({})).await);
    assert_eq!(names(&listed), ["twin", "twin"], "{listed}");
    for selector in ["nope", "legacy", "hidden", "/no/such/dir"] {
        let error =
            refused(call(&client, "awr_project_status", json!({"project": selector})).await);
        assert!(
            error
                .to_string()
                .contains("no registered local project matches"),
            "{selector}: {error}"
        );
    }
    let ambiguous = refused(call(&client, "awr_project_status", json!({"project": "twin"})).await);
    assert!(
        ambiguous
            .to_string()
            .contains("2 registered projects match"),
        "{ambiguous}"
    );
    assert_eq!(
        who(&ok(call(
            &client,
            "awr_project_status",
            json!({"project": one.to_str().unwrap()})
        )
        .await)),
        entry_one.project_id.unwrap()
    );
}

#[tokio::test]
async fn without_a_default_project_a_call_must_name_one_and_the_error_says_where_to_look() {
    let lab = Lab::new();
    lab.register(&lab.project("alpha"));
    let client = lab.client(&[], &lab.empty_dir()).await;
    let error = refused(call(&client, "awr_project_status", json!({})).await);
    assert!(error.to_string().contains("awr_projects_list"), "{error}");
}

#[tokio::test]
async fn the_default_project_is_used_when_none_is_named() {
    let lab = Lab::new();
    let alpha = lab.project("alpha");
    let beta = lab.project("beta");
    let alpha_id = lab.register(&alpha).project_id.unwrap();
    let beta_id = awr_local::inspect(&beta).unwrap().project_id.unwrap();
    // Default from the working directory, even though it is not registered.
    let client = lab.client(&[], &beta).await;
    assert_eq!(
        who(&ok(call(&client, "awr_project_status", json!({})).await)),
        beta_id
    );
    assert_eq!(
        who(&ok(call(
            &client,
            "awr_project_status",
            json!({"project": "alpha"})
        )
        .await)),
        alpha_id
    );
    // An explicit --project wins over the working directory.
    let client = lab
        .client(&["--project", alpha.to_str().unwrap()], &beta)
        .await;
    assert_eq!(
        who(&ok(call(&client, "awr_project_status", json!({})).await)),
        alpha_id
    );
}

#[tokio::test]
async fn allow_root_limits_which_projects_are_served() {
    let lab = Lab::new();
    let allowed_dir = lab.base.join("allowed");
    let inside = lab.project_in(&allowed_dir, "inside", "inside");
    let outside = lab.project("outside");
    let inside_id = lab.register(&inside).project_id.unwrap();
    lab.register(&outside);
    let client = lab
        .client(
            &["--allow-root", allowed_dir.to_str().unwrap()],
            &lab.empty_dir(),
        )
        .await;
    assert_eq!(
        names(&ok(call(&client, "awr_projects_list", json!({})).await)),
        ["inside"]
    );
    assert_eq!(
        who(&ok(call(
            &client,
            "awr_project_status",
            json!({"project": "inside"})
        )
        .await)),
        inside_id
    );
    let refused_outside =
        refused(call(&client, "awr_project_status", json!({"project": "outside"})).await);
    let unknown = refused(call(&client, "awr_project_status", json!({"project": "nope"})).await);
    assert_eq!(
        refused_outside, unknown,
        "a project outside the allowed roots looks like an unknown one"
    );
}

#[tokio::test]
async fn a_replaced_project_is_refused_on_the_next_call() {
    let lab = Lab::new();
    let root = lab.project("alpha");
    let alpha_id = lab.register(&root).project_id.unwrap();
    let client = lab.client(&[], &lab.empty_dir()).await;
    assert_eq!(
        who(&ok(call(
            &client,
            "awr_project_status",
            json!({"project": "alpha"})
        )
        .await)),
        alpha_id
    );
    fs::remove_dir_all(&root).unwrap();
    lab.project_in(&lab.base.clone(), "alpha", "replacement");
    let error = refused(call(&client, "awr_project_status", json!({"project": "alpha"})).await);
    assert!(error.to_string().contains("identity changed"), "{error}");
}

#[tokio::test]
async fn the_list_is_bounded_and_can_be_filtered() {
    let lab = Lab::new();
    for name in ["red-one", "red-two", "blue-one"] {
        lab.register(&lab.project(name));
    }
    let client = lab.client(&[], &lab.empty_dir()).await;
    let page = ok(call(&client, "awr_projects_list", json!({"limit": 2})).await);
    assert_eq!(
        (
            page["returned"].clone(),
            page["total"].clone(),
            page["truncated"].clone()
        ),
        (json!(2), json!(3), json!(true))
    );
    let red = ok(call(&client, "awr_projects_list", json!({"query": "RED"})).await);
    assert_eq!(red["total"], 2);
    assert!(
        refused(call(&client, "awr_projects_list", json!({"limit": 0})).await)
            .to_string()
            .contains("limit")
    );
    assert!(
        refused(call(&client, "awr_projects_list", json!({"surprise": 1})).await)
            .to_string()
            .contains("surprise")
    );
}

#[tokio::test]
async fn the_catalog_mode_tool_surface_stays_within_the_size_limits() {
    let lab = Lab::new();
    let client = lab.client(&[], &lab.empty_dir()).await;
    let tools = client.list_all_tools().await.unwrap();
    assert!(tools.iter().any(|t| t.name == "awr_projects_list"));
    assert!(
        serde_json::to_vec(&tools).unwrap().len() <= 6144,
        "catalog size"
    );
    for tool in &tools {
        assert!(
            tool.description.as_deref().unwrap().len() <= 200,
            "{}",
            tool.name
        );
        if tool.name != "awr_projects_list" {
            let schema = serde_json::to_value(&*tool.input_schema).unwrap();
            assert!(
                schema["properties"]["project"].is_object(),
                "{} must accept project",
                tool.name
            );
            assert!(
                !schema["required"]
                    .as_array()
                    .is_some_and(|r| r.iter().any(|v| v == "project")),
                "{}: project is optional because there can be a default",
                tool.name
            );
        }
    }
}
