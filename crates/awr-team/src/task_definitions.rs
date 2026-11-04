//! Source-owned task definitions, separate from execution and completion facts.
use crate::DraftDefinitionState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const TASK_DEFINITIONS_FILE: &str = "task_definitions.json";

/// Complete definition metadata for the contracts in a source snapshot.
/// Keys are internal work IDs; these fields never grant completion or acceptance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDefinitions {
    pub works: BTreeMap<String, TaskDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDefinition {
    pub title: String,
    pub definition_state: DraftDefinitionState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkDefinitionState;
    use serde_json::json;

    #[test]
    fn source_planning_and_public_definition_states_share_the_same_wire_values() {
        for state in ["draft", "enabled", "archived", "cancelled"] {
            let wire = json!(state);
            let public: WorkDefinitionState = serde_json::from_value(wire.clone()).unwrap();
            let planning: DraftDefinitionState = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(public).unwrap(), wire);
            assert_eq!(planning.as_str(), state);
            let definition: TaskDefinition = serde_json::from_value(json!({
                "title":"Synthetic task", "definition_state":state
            }))
            .unwrap();
            assert_eq!(definition.definition_state, planning);
        }
        assert!(serde_json::from_value::<WorkDefinitionState>(json!("completed")).is_err());
        assert!(
            serde_json::from_value::<TaskDefinition>(json!({
                "title":"Synthetic task", "definition_state":"completed"
            }))
            .is_err()
        );
    }
}
