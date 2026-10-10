//! Discovery for the existing planning wire types; domain checks remain authoritative.
use serde_json::{Value, json};

pub(super) fn add(schema: &mut Value) {
    let strings = json!({"type":"array","items":{"type":"string"}});
    schema["$defs"] = json!({"TaskDraft":{
        "type":"object","additionalProperties":false,
        "required":["work_id","external_key","title","goals","scope_paths","acceptance",
                    "required_dependencies","completion_policy","definition_state"],
        "properties":{
            "work_id":{"type":"string","description":"Preserve the existing identity when editing."},
            "external_key":{"type":"string"},
            "title":{"type":"string","description":"Nonblank; at most 512 UTF-8 bytes."},
            "goals":strings,
            "scope_paths":strings,
            "acceptance":strings,
            "required_dependencies":strings,
            "completion_policy":{"type":"string","description":"Existing supported policy label; cannot forge completion."},
            "definition_state":{"type":"string","enum":["draft","enabled","archived","cancelled"]},
            "dependency_acceptance":{"type":"object","additionalProperties":{
                "oneOf":[
                    {"type":"string","enum":["agent_reviewed_caller_asserted_reconciled","simulated_member_independent"]},
                    {"type":"object","additionalProperties":false,"required":["cross_workstream"],
                     "properties":{"cross_workstream":{
                        "type":"object","additionalProperties":false,
                        "required":["review_assurance","version_policy"],
                        "properties":{
                            "review_assurance":{"type":"string","enum":["team_independent","simulated_member_independent"]},
                            "version_policy":{"type":"string","enum":["current_contract","fixed_delivery"]}
                        }
                     }}}
                ]},"description":"Nonempty map of required predecessors. Omission retains source policy; null is rejected."},
            "hard_rules":{"type":"array","items":{"type":"string"},
                "description":"Omission retains source values; a list replaces them. Null is rejected."},
            "verification_requirements":{"type":"array","items":{"type":"string"},
                "description":"Omission retains source values; a list replaces them. Null is rejected."},
            "execution_settlement":{"type":"object","additionalProperties":false,
                "required":["mode","workspace_id"],
                "properties":{
                    "mode":{"type":"string","enum":["independent_workspace_v1","independent_workspace_v2"]},
                    "workspace_id":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9][A-Za-z0-9_.:-]*$"}
                },"description":"Omission retains source values; null is rejected. V2 only permits the original current claim's late terminal report, not execution or renewal."},
            "workstream":{"type":["string","null"],"description":"Owning workstream external key; required for creating source-backed tasks."},
            "split_from":{"type":["string","null"]},
            "split_children":strings
        }
    }});
    schema["properties"]["changes"]["description"] = json!(
        "Full task definitions, not field patches. On edits, omitted dependency_acceptance, hard_rules, verification_requirements and execution_settlement retain source values; replacements require exact prior values in before. New workspace tasks require scope_paths and verification_requirements."
    );
    schema["properties"]["changes"]["items"] = json!({
        "type":"object","additionalProperties":false,"required":["op","after"],
        "properties":{
            "op":{"type":"string","enum":["create_task","edit_fields","split","cancel","archive"]},
            "before":{"anyOf":[{"$ref":"#/$defs/TaskDraft"},{"type":"null"}],
                "description":"Exact prior task for existing-work changes; omitted or null for creation."},
            "after":{"$ref":"#/$defs/TaskDraft"}
        }
    });
    schema["properties"]["self_approve_policy"] = json!({
        "type":["object","null"],"additionalProperties":false,
        "required":["allow_self_approve_ordinary","delivery_completion_policy"],
        "properties":{
            "allow_self_approve_ordinary":{"type":"boolean"},
            "delivery_completion_policy":{"type":"string"}
        },"description":"Optional ordinary planning policy; grants no authority and cannot downgrade independent delivery review."
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_team::{
        CrossWorkstreamReviewAssurance, DependencyAcceptanceMode, DraftChange,
        DraftDefinitionState, DraftOpKind, ExecutionSettlementMode,
        OrdinaryPlanningSelfApprovePolicy, TaskDraft,
    };
    use awr_team_pg::PlanningDraftRequest;
    use std::collections::BTreeSet;

    fn advertised() -> Value {
        super::super::catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_planning_draft")
            .unwrap()
            .input_schema
            .as_ref()
            .clone()
            .into()
    }

    fn task() -> Value {
        json!({
            "work_id":"task-api","external_key":"API-001","title":"Implement the API",
            "goals":["delivery"],"scope_paths":["src/api.rs"],"acceptance":["Tests pass"],
            "required_dependencies":["upstream"],
            "completion_policy":"caller_managed_execution_and_simulated_member_review",
            "definition_state":"enabled","workstream":"app","split_from":"parent",
            "split_children":["child"],"hard_rules":["Preserve existing behavior"],
            "verification_requirements":["Run the API tests"],
            "dependency_acceptance":{"upstream":"simulated_member_independent"},
            "execution_settlement":{"mode":"independent_workspace_v2","workspace_id":"workspace.api-1"}
        })
    }

    fn keys(value: &Value) -> BTreeSet<&str> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn discovery_shares_resolvable_task_definitions_and_stays_bounded() {
        let schema = advertised();
        let change = &schema["properties"]["changes"]["items"];
        assert_eq!(change["additionalProperties"], false);
        assert_eq!(change["required"], json!(["op", "after"]));
        for reference in [
            &change["properties"]["after"],
            &change["properties"]["before"]["anyOf"][0],
        ] {
            let pointer = reference["$ref"]
                .as_str()
                .unwrap()
                .strip_prefix('#')
                .unwrap();
            assert_eq!(
                schema.pointer(pointer).unwrap(),
                &schema["$defs"]["TaskDraft"]
            );
        }
        assert_eq!(change["properties"]["before"]["anyOf"][1]["type"], "null");
        assert!(
            schema.to_string().len() < 6500,
            "Planning discovery must remain bounded"
        );
    }

    #[test]
    fn advertised_task_fields_cover_actual_serialization_and_required_fields() {
        let schema = advertised();
        let definition = &schema["$defs"]["TaskDraft"];
        let parsed: TaskDraft = serde_json::from_value(task()).unwrap();
        parsed.validate_for_create().unwrap();
        let wire = serde_json::to_value(parsed).unwrap();
        assert_eq!(keys(&definition["properties"]), keys(&wire));
        assert_eq!(definition["additionalProperties"], false);
        let required = definition["required"].as_array().unwrap();
        assert_eq!(required.len(), 9);
        for field in required {
            let mut missing = wire.clone();
            missing
                .as_object_mut()
                .unwrap()
                .remove(field.as_str().unwrap());
            assert!(
                serde_json::from_value::<TaskDraft>(missing).is_err(),
                "{field}"
            );
        }
        let mut unknown = wire;
        unknown["unexpected"] = json!(true);
        assert!(serde_json::from_value::<TaskDraft>(unknown).is_err());
    }

    #[test]
    fn operation_and_definition_enums_match_real_change_parsing() {
        let schema = advertised();
        let change = &schema["properties"]["changes"]["items"];
        let states = &schema["$defs"]["TaskDraft"]["properties"]["definition_state"]["enum"];
        assert_eq!(
            change["properties"]["op"]["enum"],
            json!([
                DraftOpKind::CreateTask,
                DraftOpKind::EditFields,
                DraftOpKind::Split,
                DraftOpKind::Cancel,
                DraftOpKind::Archive
            ])
        );
        assert_eq!(
            states,
            &json!([
                DraftDefinitionState::Draft,
                DraftDefinitionState::Enabled,
                DraftDefinitionState::Archived,
                DraftDefinitionState::Cancelled
            ])
        );
        for (op, state) in [
            ("create_task", "draft"),
            ("edit_fields", "enabled"),
            ("split", "enabled"),
            ("cancel", "cancelled"),
            ("archive", "archived"),
        ] {
            assert!(
                change["properties"]["op"]["enum"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(op))
            );
            assert!(states.as_array().unwrap().contains(&json!(state)));
            let mut after = task();
            after["definition_state"] = json!(state);
            let before = if op == "create_task" {
                Value::Null
            } else {
                task()
            };
            let mut wire = json!({"op":op,"before":before,"after":after});
            let parsed: DraftChange = serde_json::from_value(wire.clone()).unwrap();
            parsed.after.validate().unwrap();
            if op == "create_task" {
                wire.as_object_mut().unwrap().remove("before");
                assert!(
                    serde_json::from_value::<DraftChange>(wire.clone())
                        .unwrap()
                        .before
                        .is_none()
                );
            }
            wire["unexpected"] = json!(true);
            assert!(serde_json::from_value::<DraftChange>(wire).is_err());
        }
        for (field, value) in [("op", json!("remove")), ("after", Value::Null)] {
            let mut wire = json!({"op":"create_task","after":task()});
            wire[field] = value;
            assert!(serde_json::from_value::<DraftChange>(wire).is_err());
        }
        let mut invalid = task();
        invalid["definition_state"] = json!("completed");
        assert!(!states.as_array().unwrap().contains(&json!("completed")));
        assert!(serde_json::from_value::<TaskDraft>(invalid).is_err());
    }

    #[test]
    fn legacy_omissions_and_present_nulls_preserve_parser_semantics() {
        let schema = advertised();
        let fields = &schema["$defs"]["TaskDraft"]["properties"];
        let mut legacy = task();
        for field in [
            "dependency_acceptance",
            "hard_rules",
            "verification_requirements",
            "execution_settlement",
            "workstream",
            "split_from",
            "split_children",
        ] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        legacy["completion_policy"] = json!("legacy_project_review");
        let parsed: TaskDraft = serde_json::from_value(legacy).unwrap();
        parsed.validate().unwrap();
        assert!(parsed.dependency_acceptance.is_none() && parsed.execution_settlement.is_none());
        assert!(parsed.hard_rules.is_none() && parsed.verification_requirements.is_none());
        assert!(parsed.split_children.is_empty());
        for (field, kind) in [
            ("dependency_acceptance", "object"),
            ("execution_settlement", "object"),
            ("hard_rules", "array"),
            ("verification_requirements", "array"),
            ("split_children", "array"),
        ] {
            assert_eq!(fields[field]["type"], kind);
            let mut wire = task();
            wire[field] = Value::Null;
            assert!(
                serde_json::from_value::<TaskDraft>(wire).is_err(),
                "{field}"
            );
        }
        for field in ["workstream", "split_from"] {
            assert_eq!(fields[field]["type"], json!(["string", "null"]));
            let mut wire = task();
            wire[field] = Value::Null;
            assert!(serde_json::from_value::<TaskDraft>(wire).is_ok());
        }
    }

    #[test]
    fn scalar_and_list_types_match_actual_task_parsers() {
        let schema = advertised();
        let fields = &schema["$defs"]["TaskDraft"]["properties"];
        for field in [
            "work_id",
            "external_key",
            "title",
            "completion_policy",
            "definition_state",
        ] {
            assert_eq!(fields[field]["type"], "string");
            let mut wire = task();
            wire[field] = json!(17);
            assert!(
                serde_json::from_value::<TaskDraft>(wire).is_err(),
                "{field}"
            );
        }
        for field in [
            "goals",
            "scope_paths",
            "acceptance",
            "required_dependencies",
            "hard_rules",
            "verification_requirements",
            "split_children",
        ] {
            assert_eq!(fields[field]["type"], "array");
            assert_eq!(fields[field]["items"]["type"], "string");
            for bad in [json!("not-a-list"), json!([17])] {
                let mut wire = task();
                wire[field] = bad;
                assert!(
                    serde_json::from_value::<TaskDraft>(wire).is_err(),
                    "{field}"
                );
            }
        }
    }

    #[test]
    fn dependency_modes_expose_both_string_and_closed_cross_stream_policies() {
        let schema = advertised();
        let choices = &schema["$defs"]["TaskDraft"]["properties"]["dependency_acceptance"]["additionalProperties"]
            ["oneOf"];
        let cross = &choices[1]["properties"]["cross_workstream"];
        assert_eq!(
            choices[0]["enum"],
            json!([
                DependencyAcceptanceMode::AgentReviewedCallerAssertedReconciled,
                DependencyAcceptanceMode::SimulatedMemberIndependent
            ])
        );
        assert_eq!(
            cross["properties"]["review_assurance"]["enum"],
            json!([
                CrossWorkstreamReviewAssurance::TeamIndependent,
                CrossWorkstreamReviewAssurance::SimulatedMemberIndependent
            ])
        );
        assert_eq!(
            cross["properties"]["version_policy"]["enum"],
            json!([
                awr_core::DeliveryVersionPolicy::CurrentContract,
                awr_core::DeliveryVersionPolicy::FixedDelivery
            ])
        );
        assert_eq!(choices[1]["additionalProperties"], false);
        assert_eq!(choices[1]["required"], json!(["cross_workstream"]));
        assert_eq!(cross["additionalProperties"], false);
        assert_eq!(
            cross["required"],
            json!(["review_assurance", "version_policy"])
        );
        let mut modes = choices[0]["enum"].as_array().unwrap().clone();
        for assurance in cross["properties"]["review_assurance"]["enum"]
            .as_array()
            .unwrap()
        {
            for version in cross["properties"]["version_policy"]["enum"]
                .as_array()
                .unwrap()
            {
                modes.push(json!({"cross_workstream":{"review_assurance":assurance,"version_policy":version}}));
            }
        }
        assert_eq!(modes.len(), 6);
        for mode in modes {
            let mut wire = task();
            wire["dependency_acceptance"] = json!({"upstream":mode});
            serde_json::from_value::<TaskDraft>(wire)
                .unwrap()
                .validate()
                .unwrap();
        }
        for bad in [
            json!("unknown"),
            json!({"cross_workstream":{"review_assurance":"unknown","version_policy":"current_contract"}}),
            json!({"cross_workstream":{"review_assurance":"team_independent","version_policy":"current_contract","extra":true}}),
        ] {
            let mut wire = task();
            wire["dependency_acceptance"] = json!({"upstream":bad});
            assert!(serde_json::from_value::<TaskDraft>(wire).is_err());
        }
    }

    #[test]
    fn settlement_modes_and_workspace_bounds_keep_domain_validation() {
        let schema = advertised();
        let settlement = &schema["$defs"]["TaskDraft"]["properties"]["execution_settlement"];
        assert_eq!(settlement["additionalProperties"], false);
        assert_eq!(settlement["required"], json!(["mode", "workspace_id"]));
        let identity = &settlement["properties"]["workspace_id"];
        assert_eq!(
            settlement["properties"]["mode"]["enum"],
            json!([
                ExecutionSettlementMode::IndependentWorkspaceV1,
                ExecutionSettlementMode::IndependentWorkspaceV2
            ])
        );
        assert_eq!(identity["pattern"], "^[A-Za-z0-9][A-Za-z0-9_.:-]*$");
        assert_eq!(identity["minLength"], 1);
        assert_eq!(identity["maxLength"], 128);
        for mode in settlement["properties"]["mode"]["enum"].as_array().unwrap() {
            let mut wire = task();
            wire["execution_settlement"]["mode"] = mode.clone();
            serde_json::from_value::<TaskDraft>(wire)
                .unwrap()
                .validate_for_create()
                .unwrap();
        }
        for id in [
            String::new(),
            "_leading".into(),
            "path/to/work".into(),
            "é".into(),
            "a".repeat(129),
        ] {
            let mut wire = task();
            wire["execution_settlement"]["workspace_id"] = json!(id);
            assert!(
                serde_json::from_value::<TaskDraft>(wire)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut incomplete = task();
        incomplete
            .as_object_mut()
            .unwrap()
            .remove("verification_requirements");
        let parsed: TaskDraft = serde_json::from_value(incomplete).unwrap();
        parsed.validate().unwrap();
        assert!(
            parsed.validate_for_create().is_err(),
            "Edits may retain fields; creates must declare them"
        );
    }

    #[test]
    fn planning_request_policy_is_optional_nullable_closed_and_never_downgrades_review() {
        let schema = advertised();
        let policy = &schema["properties"]["self_approve_policy"];
        assert_eq!(policy["type"], json!(["object", "null"]));
        assert_eq!(policy["additionalProperties"], false);
        let ordinary = OrdinaryPlanningSelfApprovePolicy::ordinary_default();
        let wire_policy = serde_json::to_value(&ordinary).unwrap();
        assert_eq!(keys(&policy["properties"]), keys(&wire_policy));
        assert_eq!(
            policy["properties"]["allow_self_approve_ordinary"]["type"],
            "boolean"
        );
        assert_eq!(
            policy["properties"]["delivery_completion_policy"]["type"],
            "string"
        );
        for value in [None, Some(Value::Null), Some(wire_policy.clone())] {
            let mut request = json!({"protocol_version":1,"request_id":"planning-1","mode":"create",
                                     "changes":[{"op":"create_task","after":task()}]});
            if let Some(value) = value {
                request["self_approve_policy"] = value;
            }
            let parsed: PlanningDraftRequest = serde_json::from_value(request).unwrap();
            assert_eq!(parsed.changes.len(), 1);
        }
        for field in policy["required"].as_array().unwrap() {
            let mut missing = wire_policy.clone();
            missing
                .as_object_mut()
                .unwrap()
                .remove(field.as_str().unwrap());
            assert!(serde_json::from_value::<OrdinaryPlanningSelfApprovePolicy>(missing).is_err());
        }
        let mut unknown = wire_policy;
        unknown["grant_review"] = json!(true);
        assert!(serde_json::from_value::<OrdinaryPlanningSelfApprovePolicy>(unknown).is_err());
        ordinary
            .validate_no_delivery_downgrade("independent_review")
            .unwrap();
        let weaker = OrdinaryPlanningSelfApprovePolicy {
            allow_self_approve_ordinary: true,
            delivery_completion_policy: "source_asserted".into(),
        };
        assert!(
            weaker
                .validate_no_delivery_downgrade("independent_review")
                .is_err()
        );
    }

    #[test]
    fn schema_preserves_legacy_labels_without_relaxing_completion_or_dependency_checks() {
        let schema = advertised();
        assert!(
            schema["$defs"]["TaskDraft"]["properties"]["completion_policy"]
                .get("enum")
                .is_none()
        );
        let mut forged = task();
        forged["completion_policy"] = json!("completed");
        assert!(
            serde_json::from_value::<TaskDraft>(forged)
                .unwrap()
                .validate()
                .is_err()
        );
        for map in [json!({}), json!({"missing":"simulated_member_independent"})] {
            let mut invalid = task();
            invalid["dependency_acceptance"] = map;
            assert!(
                serde_json::from_value::<TaskDraft>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
}
