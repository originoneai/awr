//! Compact Agent guidance for a directory's actual connection state.
use crate::{LocalProject, ProjectState, registry};
use awr_core::Result;
use serde_json::{Value, json};
use std::path::Path;

pub fn project_intake(root: &Path) -> Result<Value> {
    Ok(describe(registry::inspect(root)?))
}

pub(crate) fn describe(project: LocalProject) -> Value {
    let root = project.root.clone();
    let (condition, command, action) = match project.state {
        ProjectState::Connected => (
            "project_identity_is_connected",
            vec!["status", "--json"],
            "Read current organization actions, goal, work and checkpoints before claiming. A connection does not establish task readiness.",
        ),
        ProjectState::Discovered => (
            "project_has_no_awr_runtime",
            vec!["init", "--json"],
            "Preview existing sources. Use the actual user request as the goal when available; retain unknown goals. Review the returned source mapping and accept only its current preview, then inspect readiness.",
        ),
        ProjectState::Attention => (
            "project_runtime_requires_inspection",
            vec!["doctor", "--json"],
            "Inspect the exact diagnostic and existing project sources. Back up the matching runtime before a supported migration or repair; preserve existing identities and unresolved findings.",
        ),
    };
    let mut argv = vec![
        "awr".to_owned(),
        "--project".into(),
        root.to_string_lossy().into_owned(),
    ];
    argv.extend(command.into_iter().map(str::to_owned));
    json!({
        "ok":true,"read_only":true,"project":project,
        "guidance":{
            "condition":condition,
            "basis":{"manifest_present":root.join(".awr/project.toml").is_file(),"database_present":root.join(".awr/state.db").is_file(),"goal_inferred":false},
            "next_action":{"argv":argv,"instruction":action},
            "recheck":{"trigger":"after initialization, source organization, runtime repair or directory availability changes","argv":["awr","--project",root,"overview","inspect","--json"]}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_new_directory_gets_actionable_intake_without_a_fabricated_goal_or_runtime() {
        let root =
            std::env::temp_dir().join(format!("awr-intake-observation-{}", awr_core::Id::new()));
        std::fs::create_dir(&root).unwrap();
        let value = project_intake(&root).unwrap();
        assert_eq!(value["project"]["state"], "discovered");
        assert!(value["project"]["project_id"].is_null());
        assert_eq!(value["guidance"]["basis"]["goal_inferred"], false);
        assert_eq!(value["guidance"]["next_action"]["argv"][3], "init");
        assert!(!root.join(".awr").exists());
        std::fs::create_dir(root.join(".awr")).unwrap();
        let repair = project_intake(&root).unwrap();
        assert_eq!(repair["project"]["state"], "attention");
        assert_eq!(repair["guidance"]["next_action"]["argv"][3], "doctor");
        std::fs::remove_dir_all(root).unwrap();
    }
}
