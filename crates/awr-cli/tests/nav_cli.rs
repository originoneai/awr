//! `awr nav` is the project owner's view of every workstream. The grant-based narrowing of
//! the shared MCP service must not leak into it.
use awr_core::Id;
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

const WORK: &str = include_str!("../../../tests/fixtures/workstreams/context.yaml");
const MANIFEST: &str = include_str!("../../../tests/fixtures/workstreams/context.toml");
const RULE: &str = "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve all hard source facts exactly.\n";
const API_SCOPE: &str = "01K00000000000000000000001";
const CLIENT_SCOPE: &str = "01K00000000000000000000002";

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-nav-cli-{}", Id::new()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(root.join("rules.md"), RULE).unwrap();
        fs::write(root.join("sources.toml"), MANIFEST).unwrap();
        let project = Self(root);
        project.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        project
    }
    fn ok(&self, args: &[&str]) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.0)
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn nodes(nav: &Value) -> Vec<&str> {
    let mut keys = nav["mainline_graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["work_key"].as_str().unwrap())
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

#[test]
fn cli_nav_stays_the_owner_view_across_workstreams() {
    let project = Project::new();
    let nav = project.ok(&["nav"]);
    assert_eq!(nav["protocol"], "awr-mainline-nav");
    assert_eq!(nav["schema_version"], 1);
    assert_eq!(nodes(&nav), ["API-1", "CLIENT-1"]);
    assert!(nav.to_string().contains("PRIVATE_CLIENT_TASK"));
    // The cross-workstream edge keeps both ends and both stream ids; nothing is an
    // opaque boundary for the owner.
    let cross = nav["cross_dependencies"].as_array().unwrap();
    assert_eq!(cross.len(), 1);
    assert_eq!(cross[0]["from"], "API-1");
    assert_eq!(cross[0]["to"], "CLIENT-1");
    assert_eq!(cross[0]["from_workstream_id"], API_SCOPE);
    assert_eq!(cross[0]["to_workstream_id"], CLIENT_SCOPE);
    let client = nav["mainline_graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["work_key"] == "CLIENT-1")
        .unwrap();
    assert_eq!(client["explainable_waits"][0]["kind"], "dependency_outcome");
    assert!(!nav.to_string().contains("dependency_outside_scope"));
    // An explicit stream narrows the selection only; the edge to the other stream stays.
    let api = project.ok(&["nav", "--workstream", API_SCOPE]);
    assert_eq!(nodes(&api), ["API-1"]);
    assert_eq!(api["cross_dependencies"].as_array().unwrap().len(), 1);
}
