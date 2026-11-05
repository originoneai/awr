use awr_core::Id;
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

const DISTINCT: &str =
    include_str!("../../../tests/fixtures/team-mcp/publish-prep/distinct-identities.yaml");

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-yaml-identity-{}", Id::new()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("work-ledger.yaml"), DISTINCT).unwrap();
        fs::write(root.join("mapping.toml"), "[project]\nname='Shared YAML identity'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work-ledger.yaml'\nadapter='yaml-workstream-ledger-v1'\n").unwrap();
        Self(root)
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
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
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

#[test]
fn ready_resolves_internal_ids_from_the_shared_team_ledger() {
    let project = Project::new();
    project.ok(&["init", "--manifest", "mapping.toml", "--accept"]);
    let ready = project.ok(&["ready"]);
    assert_eq!(
        ready["diagnostic_counts"]["missing_dependency"]
            .as_u64()
            .unwrap_or(0),
        0,
        "{ready}"
    );
    assert!(
        ready["ready"]
            .as_array()
            .unwrap()
            .iter()
            .any(|work| work["external_key"] == "CLIENT-1"),
        "{ready}"
    );
    let work = project.ok(&["work", "show", "CLIENT-1"]);
    assert_eq!(work["required_dependencies"][0]["external_key"], "API-1");
    assert_eq!(
        fs::read_to_string(project.0.join("work-ledger.yaml")).unwrap(),
        DISTINCT
    );

    // A consistent authoritative identity edit refreshes the projection; it
    // must not rename local display selectors or create a dangling dependency.
    fs::write(
        project.0.join("work-ledger.yaml"),
        DISTINCT.replace("internal-api", "internal-api-v2"),
    )
    .unwrap();
    let refreshed = project.ok(&["ready"]);
    assert_eq!(
        refreshed["diagnostic_counts"]["missing_dependency"]
            .as_u64()
            .unwrap_or(0),
        0,
        "{refreshed}"
    );
    assert_eq!(
        project.ok(&["work", "show", "CLIENT-1"])["required_dependencies"][0]["external_key"],
        "API-1"
    );

    let missing = DISTINCT.replace(
        "depends_on: [internal-api]",
        "depends_on: [internal-api, genuinely-missing]",
    );
    fs::write(project.0.join("work-ledger.yaml"), missing).unwrap();
    let diagnosed = project.ok(&["ready"]);
    assert_eq!(
        diagnosed["diagnostic_counts"]["missing_dependency"], 1,
        "{diagnosed}"
    );
    assert!(
        !diagnosed["ready"]
            .as_array()
            .unwrap()
            .iter()
            .any(|work| work["external_key"] == "CLIENT-1")
    );
}
