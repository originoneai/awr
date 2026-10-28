//! On-demand, operation-specific views of the actual MCP tool catalog.
//! This module never supplies project values or relaxes the catalog schema.
use super::*;

pub(super) const TOOL_NAME: &str = "awr_team_input_contract";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContractError {
    InvalidInput,
    UnknownOperation,
}

pub(super) fn tool() -> Tool {
    let schema = json!({
        "type":"object",
        "additionalProperties":false,
        "required":["protocol_version","tool","op"],
        "properties":{
            "protocol_version":{"type":"integer","const":1},
            "tool":{"type":"string","enum":["awr_team_query","awr_team_command"]},
            "op":{"type":"string","minLength":1,"maxLength":128,
                "description":"An operation advertised by the selected tool's current properties.op.enum."}
        }
    });
    Tool::new(
        TOOL_NAME,
        "Read one compact operation input contract from the current MCP tool catalog. Use this only when current guidance lacks a matching command_base or the operation shape is unclear; it is not an extra step for every call. The response never invents current IDs, versions, digests or material facts: follow its value_sources and refresh real state. It grants no permission, admission, replay or acceptance.",
        schema.as_object().unwrap().clone(),
    )
    .with_annotations(
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
}

pub(super) fn describe(input: &Value, catalog: &[Tool]) -> Result<Value, ContractError> {
    if input.get("protocol_version") != Some(&json!(1))
        || input.as_object().is_none_or(|object| object.len() != 3)
    {
        return Err(ContractError::InvalidInput);
    }
    let tool_name = input
        .get("tool")
        .and_then(Value::as_str)
        .filter(|name| matches!(*name, "awr_team_query" | "awr_team_command"))
        .ok_or(ContractError::InvalidInput)?;
    let op = input
        .get("op")
        .and_then(Value::as_str)
        .filter(|op| !op.is_empty() && op.len() <= 128 && !op.chars().any(char::is_control))
        .ok_or(ContractError::InvalidInput)?;
    let tool = catalog
        .iter()
        .find(|tool| tool.name == tool_name)
        .ok_or(ContractError::InvalidInput)?;
    let schema = Value::Object((*tool.input_schema).clone());
    if !schema["properties"]["op"]["enum"]
        .as_array()
        .is_some_and(|operations| operations.contains(&json!(op)))
    {
        return Err(ContractError::UnknownOperation);
    }
    let projected = project(&schema, op)?;
    let value_sources = value_sources(tool_name, op, &projected);
    Ok(json!({
        "protocol":"awr-team-mcp-input-contract-v1",
        "protocol_version":1,
        "tool":tool_name,
        "op":op,
        "schema_source":"current tools/list inputSchema",
        "schema_projection":{"validation_equivalent":true,"omitted_annotations":["description"]},
        "schema":projected,
        "value_sources":value_sources,
        "usage":{
            "when":"Use on demand when a matching fresh command_base is absent or the selected operation shape is unclear.",
            "prefer":"If fresh work.prepare guidance contains a matching action.command_base, keep it unchanged and fill only action.caller_slots.",
            "not_required_per_call":true,
            "dynamic_values_included":false,
            "authority_granted":false
        },
        "read_only":true,
        "execution_authorized":false
    }))
}

pub(super) fn public_error(input: &Value) -> Value {
    if describe(input, &super::catalog()) == Err(ContractError::UnknownOperation) {
        return json!({
            "code":"UnsupportedOperation",
            "message":"the operation is not advertised by the selected tool's current catalog",
            "invalid_field":"/op",
            "next_step":"Read tools/list or awr_team_query capabilities, select an advertised operation for this exact tool, then request its contract. Do not substitute a similarly named query or command."
        });
    }
    json!({
        "code":"InvalidInput",
        "message":"input-contract fields or bounds are invalid",
        "next_step":"Send exactly protocol_version=1, tool=awr_team_query or awr_team_command, and one advertised operation name in op."
    })
}

/// Resolve only catalog conditions selected by the fixed operation. The result
/// remains the same schema with inactive operation branches removed.
fn project(schema: &Value, op: &str) -> Result<Value, ContractError> {
    let mut result = schema.clone();
    let op_schema = result["properties"]["op"]
        .as_object_mut()
        .ok_or(ContractError::InvalidInput)?;
    op_schema.remove("enum");
    op_schema.insert("const".into(), json!(op));
    let branches = result
        .as_object_mut()
        .and_then(|object| object.remove("allOf"))
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut active = Vec::new();
    for branch in branches {
        match operation_condition(&branch, op) {
            Some(true) => {
                if let Some(selected) = branch.get("then") {
                    active.push(selected.clone());
                }
            }
            Some(false) => {
                if let Some(selected) = branch.get("else") {
                    active.push(selected.clone());
                }
            }
            None => active.push(branch),
        }
    }
    if !active.is_empty() {
        result["allOf"] = json!(active);
    }
    if schema["properties"]["op"]["enum"]
        .as_array()
        .is_none_or(|operations| !operations.contains(&json!(op)))
    {
        return Err(ContractError::UnknownOperation);
    }
    compact_closed_command_args(&mut result);
    remove_descriptions(&mut result);
    Ok(result)
}

fn operation_condition(branch: &Value, op: &str) -> Option<bool> {
    let condition = branch.get("if")?;
    if !condition["required"]
        .as_array()
        .is_some_and(|required| required.contains(&json!("op")))
    {
        return None;
    }
    let operation = &condition["properties"]["op"];
    if let Some(expected) = operation.get("const") {
        return Some(expected == op);
    }
    operation
        .get("enum")
        .and_then(Value::as_array)
        .map(|operations| operations.contains(&json!(op)))
}

/// A closed operation branch rejects every arg not declared in its own
/// `properties`. Root properties outside that set are therefore unreachable.
/// Keep the branch object constraints, intersect every retained field with its
/// root constraint, and carry over non-conflicting root object constraints.
/// Any shape that does not satisfy these proof conditions stays unmodified.
fn compact_closed_command_args(schema: &mut Value) -> bool {
    let Some(root_args) = schema["properties"].get("args").cloned() else {
        return false;
    };
    let Some(branches) = schema.get("allOf").and_then(Value::as_array) else {
        return false;
    };
    let candidates = branches
        .iter()
        .enumerate()
        .filter_map(|(index, branch)| {
            let object = branch.as_object()?;
            if object.len() != 1 {
                return None;
            }
            let properties = object.get("properties")?.as_object()?;
            if properties.len() != 1 {
                return None;
            }
            let args = properties.get("args")?;
            (args["type"] == "object"
                && args["additionalProperties"] == false
                && args["properties"].is_object())
            .then(|| (index, args.clone()))
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return false;
    }
    let (index, branch_args) = candidates.into_iter().next().unwrap();
    let Some(compact) = intersect_closed_args(&root_args, &branch_args) else {
        return false;
    };
    schema["properties"]["args"] = compact;
    let branches = schema["allOf"].as_array_mut().unwrap();
    branches.remove(index);
    if branches.is_empty() {
        schema.as_object_mut().unwrap().remove("allOf");
    }
    true
}

fn intersect_closed_args(root: &Value, branch: &Value) -> Option<Value> {
    let mut root = root.clone();
    let mut branch = branch.clone();
    remove_descriptions(&mut root);
    remove_descriptions(&mut branch);
    let root_object = root.as_object()?;
    let root_properties = root_object.get("properties")?.as_object()?;
    let branch_object = branch.as_object_mut()?;
    // These keywords depend on which sibling schema evaluated a property.
    // Moving them into the closed branch can change that evaluated-property
    // set even when the visible `properties` maps look compatible.
    for keyword in ["patternProperties", "unevaluatedProperties"] {
        if root_object.contains_key(keyword) || branch_object.contains_key(keyword) {
            return None;
        }
    }
    let declared_by_branch = branch_object.get("properties")?.as_object()?;
    let branch_only_field = declared_by_branch
        .keys()
        .any(|name| !root_properties.contains_key(name));
    if branch_only_field
        && root_object
            .get("additionalProperties")
            .is_some_and(|additional| additional != true)
    {
        // In the original intersection, the root rejects or constrains this
        // branch-declared field. Declaring it in a merged schema would exempt
        // it from the root's additionalProperties rule.
        return None;
    }
    let allowed = {
        let branch_properties = branch_object.get_mut("properties")?.as_object_mut()?;
        for (name, branch_field) in branch_properties.iter_mut() {
            if let Some(root_field) = root_properties.get(name)
                && root_field != branch_field
            {
                *branch_field = json!({"allOf":[root_field.clone(),branch_field.clone()]});
            }
        }
        branch_properties.keys().cloned().collect::<Vec<_>>()
    };
    for (key, root_value) in root_object {
        if matches!(key.as_str(), "properties" | "description") {
            continue;
        }
        match branch_object.get(key) {
            None => {
                branch_object.insert(key.clone(), root_value.clone());
            }
            Some(branch_value) if branch_value == root_value => {}
            Some(_) => {
                let mut restricted_root = root.clone();
                restricted_root["properties"] = Value::Object(
                    allowed
                        .iter()
                        .cloned()
                        .filter_map(|name| {
                            root_properties
                                .get(&name)
                                .cloned()
                                .map(|value| (name, value))
                        })
                        .collect(),
                );
                return Some(json!({"allOf":[restricted_root,branch]}));
            }
        }
    }
    Some(branch)
}

fn remove_descriptions(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("description");
            for child in object.values_mut() {
                remove_descriptions(child);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(remove_descriptions),
        _ => {}
    }
}

fn required_args(schema: &Value) -> std::collections::BTreeSet<String> {
    let mut fields = std::collections::BTreeSet::new();
    if let Some(required) = schema["properties"]["args"]["required"].as_array() {
        fields.extend(required.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    for branch in schema["allOf"].as_array().into_iter().flatten() {
        if let Some(required) = branch["properties"]["args"]["required"].as_array() {
            fields.extend(required.iter().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    fields
}

fn value_sources(tool: &str, op: &str, schema: &Value) -> Value {
    let fields = required_args(schema);
    let has = |names: &[&str]| names.iter().any(|name| fields.contains(*name));
    let mut sources = Vec::new();
    let mut classified = std::collections::BTreeSet::new();
    if tool == "awr_team_command" {
        sources.push(json!({
            "fields":["workstream_id","work_id","coordinator_epoch","expected_project_revision",
                "expected_authority_version","expected_ownership_version","expected_contract_hash"],
            "read_op":"work.prepare",
            "pointers":{
                "workstream_id":"/workstream_id","work_id":"/data/work_id",
                "coordinator_epoch":"/coordinator_epoch","expected_project_revision":"/project_revision",
                "expected_authority_version":"/authority_version",
                "expected_ownership_version":"/data/ownership_version",
                "expected_contract_hash":"/data/contract_hash"},
            "freshness":"Use one fresh response for the whole header; refresh after related state or authority changes."
        }));
    }
    if has(&["session_id", "expected_session_version"]) {
        classified.extend(["session_id", "expected_session_version"].map(str::to_owned));
        sources.push(json!({
            "fields":["session_id","expected_session_version"],
            "read_op":"session.inspect",
            "pointers":{"session_id":"/data/items/*/session_id","expected_session_version":"/data/items/*/session_version"},
            "navigation":"Select the actual owned session from work.next or a successful session receipt, then inspect that session.",
            "freshness":"A later successful receipt may advance session_version; reinspect instead of reusing an older value."
        }));
    }
    if fields.contains("context_hash") {
        classified.insert("context_hash".to_owned());
        sources.push(json!({
            "fields":["context_hash"],"read_op":"work.prepare","pointer":"/data/context_hash",
            "freshness":"Use the hash from the context actually consumed. session.inspect context_hash is historical."
        }));
    }
    if has(&["expected_responsibility_version", "expected_work_version"]) {
        classified.extend(
            ["expected_responsibility_version", "expected_work_version"].map(str::to_owned),
        );
        sources.push(json!({
            "fields":["expected_responsibility_version","expected_work_version"],
            "read_op":"work.prepare",
            "pointers":{"expected_responsibility_version":"/data/responsibility/version",
                "expected_work_version":"/data/runtime/work_version"},
            "freshness":"Use the current preparation. Only intake may use \"0\" when data.runtime is absent; never default an existing runtime."
        }));
    }
    if has(&[
        "claim_id",
        "expected_fence",
        "expected_lease_version",
        "fence",
        "lease_version",
    ]) {
        classified.extend(
            [
                "claim_id",
                "expected_fence",
                "expected_lease_version",
                "fence",
                "lease_version",
            ]
            .map(str::to_owned),
        );
        sources.push(json!({
            "fields":["claim_id","expected_fence or fence","expected_lease_version or lease_version"],
            "read_op":"claim.inspect",
            "pointers":{"claim_id":"/data/claim_id","fence":"/data/fence","lease_version":"/data/lease_version"},
            "freshness":"Inspect the exact current claim using claim_id plus work_id or session_id. A replayed acquire receipt is not current lease state."
        }));
    }
    if has(&["execution_id", "expected_execution_version"]) {
        classified.extend(["execution_id", "expected_execution_version"].map(str::to_owned));
        sources.push(json!({
            "fields":["execution_id","expected_execution_version"],
            "read_op":"execution.inspect",
            "pointers":{"execution_id":"/data/execution_id","expected_execution_version":"/data/execution_version"},
            "freshness":"Use the exact current execution or its latest successful receipt. Inspection never authorizes execution."
        }));
    }
    if fields.contains("source_snapshot_id") {
        classified.insert("source_snapshot_id".to_owned());
        sources.push(json!({
            "fields":["source_snapshot_id"],"read_op":"work.prepare","pointer":"/source_snapshot_id",
            "freshness":"Use the same fresh preparation as the command header."
        }));
    }
    if op == "delivery.candidate.select" {
        classified.extend(["expected_selected_digest", "candidate"].map(str::to_owned));
        sources.push(json!({
            "fields":["expected_selected_digest","candidate"],
            "read_op":"delivery.submission.describe",
            "pointers":{"expected_selected_digest":"/data/stored_selection/binding_digest or null",
                "binding_values":"/data/binding_values","args_schema":"/data/command/args_schema"},
            "material_inputs":"Measure the actual published immutable revision, target precondition, manifest bytes, SHA-256 values and byte lengths; descriptions do not supply those facts."
        }));
    }
    if op == "delivery.source.prepare" {
        classified.extend(
            [
                "candidate_digest",
                "expected_selection_version",
                "expected_metadata_revision",
                "expected_source_fingerprint",
            ]
            .map(str::to_owned),
        );
        sources.push(json!({
            "fields":["candidate_digest","expected_selection_version"],
            "read_op":"delivery.submission.describe",
            "pointers":{"candidate_digest":"/data/stored_selection/binding_digest",
                "expected_selection_version":"/data/stored_selection/selection_version"},
            "freshness":"The stored selection must still be current."
        }));
        sources.push(json!({
            "fields":["expected_metadata_revision","expected_source_fingerprint"],
            "read_op":"delivery.source.status",
            "pointers":{"expected_metadata_revision":"/data/metadata_revision",
                "expected_source_fingerprint":"/data/confirmed_fingerprint"},
            "freshness":"Use the current observed source status; unavailable or divergent source is not an inferred fingerprint."
        }));
    }
    if matches!(op, "evidence.submit" | "delivery.evidence.submit") {
        classified.extend(
            [
                "dirty_tree",
                "artifact_text",
                "artifact_hex",
                "input_digest",
                "execution_id",
            ]
            .map(str::to_owned),
        );
        sources.push(json!({
            "fields":["dirty_tree","artifact_text or artifact_hex","input_digest","execution_id"],
            "source":"caller-observed workspace plus the exact current execution",
            "freshness":"Submit actual bytes and state. Simulated-member evidence requires the actual execution_id; do not invent digests or artifact identifiers."
        }));
    }
    if op == "delivery.evidence.submit" {
        classified.insert("delivery_payload".to_owned());
        sources.push(json!({
            "fields":["delivery_payload.delivery_candidate_digest"],
            "source":"successful delivery.candidate.select receipt",
            "pointer":"/receipt/data/candidate_digest",
            "freshness":"Bind the exact selected candidate; reselect changed material before new evidence."
        }));
    }
    if matches!(op, "review.open" | "delivery.submit_and_request_review") {
        classified.insert("evidence_id".to_owned());
        sources.push(json!({
            "fields":["evidence_id"],"source":"successful evidence submission receipt",
            "pointer":"/receipt/data/evidence_id",
            "freshness":"Use the primary evidence under review, then refresh the session version. Do not resend the evidence payload as review args."
        }));
    }
    if tool == "awr_team_query" && op == "claim.inspect" {
        sources.push(json!({
            "fields":["claim_id","work_id or session_id"],
            "source":"the selected work/session navigation and the actual claim receipt",
            "freshness":"Both the record selector and a work-or-session selector are required; inspection grants no execution authority."
        }));
    }
    if op == "handoff.propose" {
        classified.insert("handoff_id".to_owned());
        sources.push(json!({
            "fields":["handoff_id"],
            "source_type":"new_request_identity",
            "rule":"Create one new stable handoff identifier for this proposal and retain it for its receipts. It is not a material fact and must not be copied from examples or another handoff."
        }));
    }
    if matches!(
        op,
        "handoff.inspect"
            | "handoff.accept"
            | "handoff.reject"
            | "handoff.cancel"
            | "handoff.timeout"
    ) {
        classified.extend(["handoff_id", "expected_handoff_version"].map(str::to_owned));
        sources.push(json!({
            "fields":["handoff_id","expected_handoff_version"],
            "source":"successful handoff.propose receipt, refreshed by the latest successful handoff.inspect receipt",
            "pointers":{"handoff_id":"/receipt/data/handoff_id","expected_handoff_version":"/receipt/data/version"},
            "freshness":"Use the latest receipt for this exact handoff. A proposal or inspection of another handoff is not a selector."
        }));
    }
    if op == "handoff.accept" {
        classified.extend(
            [
                "inspection_request_id",
                "acceptor_person_id",
                "successor_execution",
            ]
            .map(str::to_owned),
        );
        sources.push(json!({
            "fields":["inspection_request_id","acceptor_person_id","successor_execution"],
            "source":"the same authenticated member/client/session's successful handoff.inspect receipt",
            "pointers":{"inspection_request_id":"/receipt/data/inspection_request_id","acceptor_person_id":"/receipt/data/consumption/member_person_id","successor_execution":"/receipt/data/consumption/execution_instance"},
            "freshness":"Reinspect after package, session, authority, fence or related work changes; the read-only query catalog is not a consumption receipt."
        }));
        sources.push(json!({
            "fields":["expected_current_fence"],
            "read_op":"work.prepare",
            "pointer":"/data/runtime/last_fence",
            "freshness":"Required whenever runtime exists. Omit or use null only when the fresh preparation has no runtime."
        }));
    }
    if op == "delivery.integration.prepare" {
        classified.extend(
            [
                "connector_id",
                "connector_version",
                "candidate_digest",
                "selection_version",
                "evidence_id",
                "review_round_id",
                "review_decision_id",
                "operation",
            ]
            .map(str::to_owned),
        );
        sources.push(json!({
            "fields":["connector_id","connector_version"],
            "read_op":"delivery.neutral.inspect",
            "pointers":{"connector_id":"/data/connectors/*/connector_id","connector_version":"/data/connectors/*/connector_version"},
            "freshness":"Select the configured connector for this work whose current_epoch and enabled facts are true; do not combine values from different connector entries."
        }));
        sources.push(json!({
            "fields":["candidate_digest","selection_version"],
            "read_op":"delivery.submission.describe",
            "pointers":{"candidate_digest":"/data/stored_selection/binding_digest","selection_version":"/data/stored_selection/selection_version"},
            "freshness":"The stored selection must be current for this exact work."
        }));
        sources.push(json!({
            "fields":["evidence_id","review_round_id","review_decision_id"],
            "read_op":"review.inspect",
            "navigation":"Use the actual round_id from the successful review-opening receipt as review_round_id, then inspect that exact round.",
            "pointers":{"evidence_id":"/data/review/evidence_id","review_round_id":"/data/review/round_id","review_decision_id":"/data/review/decisions/*/decision_id"},
            "freshness":"Select the actual current approving decision bound to that evidence and round; never combine identifiers from different reviews."
        }));
        sources.push(json!({
            "fields":["operation"],
            "source":"selected operation contract literal",
            "pointer":"/schema/properties/args/properties/operation/enum",
            "rule":"Use only the advertised literal; it is not a provider fact."
        }));
    }
    if op == "delivery.integration.reject_prepared" {
        classified.insert("integration_id".to_owned());
        sources.push(json!({
            "fields":["integration_id"],
            "source":"successful delivery.integration.prepare receipt",
            "pointer":"/receipt/data/integration_id",
            "freshness":"Reject only the exact still-prepared integration intent; inspect it through delivery.integration.inspect when state is uncertain."
        }));
    }
    let unclassified_required_fields = fields.difference(&classified).cloned().collect::<Vec<_>>();
    json!({
        "values":sources,
        "unclassified_required_fields":unclassified_required_fields,
        "rule":"No current value is embedded in this static contract. Read the named current state and preserve JSON decimal strings for versions and fences. A required field without an explicit source above has source=unknown: consult the operation's current authoritative read or successful receipt and stop if its provenance cannot be established. Never infer that an unclassified field is caller-observed, material or literal, and never fabricate it."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(tool: &str, op: &str) -> Value {
        json!({"protocol_version":1,"tool":tool,"op":op})
    }

    fn contract(tool: &str, op: &str) -> Value {
        describe(&request(tool, op), &super::super::catalog()).unwrap()
    }

    fn required(contract: &Value) -> std::collections::BTreeSet<String> {
        required_args(&contract["schema"])
    }

    #[test]
    fn every_multiplexed_operation_has_one_bounded_catalog_projection() {
        let catalog = super::super::catalog();
        assert_eq!(
            WorkstreamQuery::OPERATIONS.len() + WorkstreamCommand::OPERATIONS.len(),
            83
        );
        for (tool, operations) in [
            ("awr_team_query", WorkstreamQuery::OPERATIONS),
            ("awr_team_command", WorkstreamCommand::OPERATIONS),
        ] {
            let full = catalog.iter().find(|entry| entry.name == tool).unwrap();
            let full_bytes = serde_json::to_vec(&full.input_schema).unwrap().len();
            for op in operations {
                let value = describe(&request(tool, op), &catalog).unwrap();
                assert_eq!(value["schema"]["properties"]["op"]["const"], *op);
                let bytes = serde_json::to_vec(&value).unwrap().len();
                assert!(bytes < full_bytes, "{tool}/{op}: {bytes} >= {full_bytes}");
                assert!(bytes < 49_152, "{tool}/{op}: {bytes}");
            }
        }
    }

    #[test]
    fn all_operation_results_fit_the_dual_content_mcp_budget() {
        let catalog = super::super::catalog();
        let mut largest = ("", "", 0, 0);
        for (tool, operations) in [
            ("awr_team_query", WorkstreamQuery::OPERATIONS),
            ("awr_team_command", WorkstreamCommand::OPERATIONS),
        ] {
            for op in operations {
                let value = describe(&request(tool, op), &catalog).unwrap();
                let value_bytes = serde_json::to_vec(&value).unwrap().len();
                let wire_bytes = serde_json::to_vec(&CallToolResult::structured(value))
                    .unwrap()
                    .len();
                if wire_bytes > largest.3 {
                    largest = (tool, op, value_bytes, wire_bytes);
                }
                assert!(wire_bytes < 49_152, "{tool}/{op}: {wire_bytes}");
                if matches!(*op, "session.checkpoint" | "execution.report") {
                    assert!(wire_bytes < 32_768, "{tool}/{op}: {wire_bytes}");
                    eprintln!("{op}: value={value_bytes} dual_content_wire={wire_bytes}");
                }
                let invalid_op = super::super::input_diagnostics::shape_issue(
                    &json!("not-an-advertised-operation"),
                    &describe(&request(tool, op), &catalog).unwrap()["schema"]["properties"]["op"],
                    "/op",
                )
                .unwrap();
                assert_eq!(invalid_op["constraint"], "const", "{tool}/{op}");
            }
        }
        eprintln!(
            "largest input contract: {}/{} value={} dual_content_wire={}",
            largest.0, largest.1, largest.2, largest.3
        );
    }

    #[test]
    fn every_query_projection_changes_only_selected_conditions_and_annotations() {
        let catalog = super::super::catalog();
        let query = catalog
            .iter()
            .find(|entry| entry.name == "awr_team_query")
            .unwrap();
        let source = Value::Object((*query.input_schema).clone());
        for op in WorkstreamQuery::OPERATIONS {
            let mut expected = source.clone();
            expected["properties"]["op"]
                .as_object_mut()
                .unwrap()
                .remove("enum");
            expected["properties"]["op"]["const"] = json!(op);
            let branches = expected.as_object_mut().unwrap().remove("allOf").unwrap();
            let active = branches
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|branch| match operation_condition(branch, op) {
                    Some(true) => branch.get("then").cloned(),
                    Some(false) => branch.get("else").cloned(),
                    None => Some(branch.clone()),
                })
                .collect::<Vec<_>>();
            if !active.is_empty() {
                expected["allOf"] = json!(active);
            }
            remove_descriptions(&mut expected);
            let actual = &describe(&request("awr_team_query", op), &catalog).unwrap()["schema"];
            assert_eq!(actual, &expected, "{op}: query validation changed");
        }
    }

    #[test]
    fn every_command_projection_preserves_root_and_branch_constraint_intersections() {
        let catalog = super::super::catalog();
        let command = catalog
            .iter()
            .find(|entry| entry.name == "awr_team_command")
            .unwrap();
        let mut root_args = command.input_schema["properties"]["args"].clone();
        remove_descriptions(&mut root_args);
        for op in WorkstreamCommand::OPERATIONS {
            let value = describe(&request("awr_team_command", op), &catalog).unwrap();
            let actual = &value["schema"]["properties"]["args"];
            let closed = command.input_schema["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|branch| operation_condition(branch, op) == Some(true))
                .filter_map(|branch| branch.get("then"))
                .filter_map(|then| then["properties"].get("args"))
                .filter(|args| {
                    args["type"] == "object"
                        && args["additionalProperties"] == false
                        && args["properties"].is_object()
                })
                .collect::<Vec<_>>();
            assert_eq!(
                closed.len(),
                1,
                "{op}: every advertised command must have one closed args schema"
            );
            let mut branch = closed[0].clone();
            remove_descriptions(&mut branch);
            assert_eq!(actual["type"], branch["type"], "{op}: type changed");
            assert_eq!(
                actual["additionalProperties"], branch["additionalProperties"],
                "{op}: closure changed"
            );
            assert_eq!(
                actual["required"], branch["required"],
                "{op}: required fields changed"
            );
            let actual_fields = actual["properties"].as_object().unwrap();
            let branch_fields = branch["properties"].as_object().unwrap();
            assert_eq!(
                actual_fields.keys().collect::<Vec<_>>(),
                branch_fields.keys().collect::<Vec<_>>(),
                "{op}: allowed fields changed"
            );
            for (name, branch_field) in branch_fields {
                let root_field = &root_args["properties"][name];
                let expected = if root_field == branch_field {
                    branch_field.clone()
                } else {
                    json!({"allOf":[root_field.clone(),branch_field.clone()]})
                };
                assert_eq!(
                    actual_fields[name], expected,
                    "{op}/{name}: intersection changed"
                );
            }
        }
    }

    fn synthetic_args_schema(root: Value, branch: Value) -> Value {
        json!({
            "type":"object",
            "properties":{"args":root},
            "allOf":[{"properties":{"args":branch}}]
        })
    }

    #[test]
    fn future_property_evaluation_shapes_fail_safe_without_compaction() {
        let root_closed = json!({
            "type":"object",
            "additionalProperties":false,
            "properties":{"root_field":{"type":"string"}}
        });
        let branch_with_extra = json!({
            "type":"object",
            "additionalProperties":false,
            "required":["branch_only"],
            "properties":{"branch_only":{"type":"string"}}
        });
        let mut additional = synthetic_args_schema(root_closed.clone(), branch_with_extra);
        let before = additional.clone();
        assert!(!compact_closed_command_args(&mut additional));
        assert_eq!(additional, before);
        let rejection = super::super::input_diagnostics::shape_issue(
            &json!({"branch_only":"would-be-accepted-after-unsafe-merge"}),
            &additional["properties"]["args"],
            "/args",
        )
        .unwrap();
        assert_eq!(rejection["constraint"], "unexpected_field");

        for (label, root, branch) in [
            (
                "root-pattern-properties",
                json!({"type":"object","properties":{"known":{"type":"string"}},
                    "patternProperties":{"^x-":{"type":"string"}}}),
                json!({"type":"object","additionalProperties":false,
                    "properties":{"known":{"type":"string"}}}),
            ),
            (
                "branch-pattern-properties",
                json!({"type":"object","properties":{"known":{"type":"string"}}}),
                json!({"type":"object","additionalProperties":false,
                    "properties":{"known":{"type":"string"}},
                    "patternProperties":{"^x-":{"type":"string"}}}),
            ),
            (
                "root-unevaluated-properties",
                json!({"type":"object","properties":{"known":{"type":"string"}},
                    "unevaluatedProperties":false}),
                json!({"type":"object","additionalProperties":false,
                    "properties":{"known":{"type":"string"}}}),
            ),
            (
                "branch-unevaluated-properties",
                json!({"type":"object","properties":{"known":{"type":"string"}}}),
                json!({"type":"object","additionalProperties":false,
                    "properties":{"known":{"type":"string"}},
                    "unevaluatedProperties":false}),
            ),
        ] {
            let mut schema = synthetic_args_schema(root, branch);
            let before = schema.clone();
            assert!(
                !compact_closed_command_args(&mut schema),
                "{label}: unsafe compaction"
            );
            assert_eq!(schema, before, "{label}: fail-safe schema changed");
        }
    }

    fn field_issue(value: &Value, schema: &Value) -> Option<Value> {
        if let Some(branches) = schema["allOf"].as_array() {
            return branches.iter().find_map(|branch| {
                super::super::input_diagnostics::shape_issue(value, branch, "/args/session_id")
            });
        }
        super::super::input_diagnostics::shape_issue(value, schema, "/args/session_id")
    }

    #[test]
    fn review_family_projections_retain_utf8_byte_and_control_rejections() {
        for op in [
            "review.open",
            "delivery.submit_and_request_review",
            "review.accept",
            "review.return",
            "review.decide",
            "work.rework",
            "work.complete",
            "delivery.finalize",
        ] {
            let value = contract("awr_team_command", op);
            let schema = &value["schema"]["properties"]["args"]["properties"]["session_id"];
            let oversized = field_issue(&json!("雪".repeat(43)), schema).unwrap();
            assert_eq!(
                oversized["constraint"], "x-identity-utf8.maxUtf8Bytes",
                "{op}: 129-byte identity was accepted"
            );
            let control = field_issue(&json!("session\n"), schema).unwrap();
            assert_eq!(
                control["constraint"], "x-identity-utf8.rejectControls",
                "{op}: control character was accepted"
            );
        }
    }

    #[test]
    fn representative_native_rejections_discover_required_nesting_and_shapes() {
        let cases = [
            (
                "task.claim_available",
                &[
                    "session_id",
                    "expected_session_version",
                    "expected_responsibility_version",
                    "expected_work_version",
                    "ttl_seconds",
                ][..],
            ),
            (
                "execution.start",
                &[
                    "session_id",
                    "expected_session_version",
                    "execution_id",
                    "expected_execution_version",
                    "expected_work_version",
                    "claim_id",
                    "expected_fence",
                    "expected_lease_version",
                    "execution_mode",
                ],
            ),
            (
                "execution.report",
                &[
                    "session_id",
                    "expected_session_version",
                    "execution_id",
                    "expected_execution_version",
                    "outcome",
                    "observed_paths",
                    "note",
                ],
            ),
            (
                "delivery.source.prepare",
                &[
                    "source_snapshot_id",
                    "candidate_digest",
                    "expected_selection_version",
                    "expected_metadata_revision",
                    "expected_source_fingerprint",
                    "lease_seconds",
                ],
            ),
            (
                "delivery.candidate.select",
                &[
                    "source_snapshot_id",
                    "session_id",
                    "claim_id",
                    "fence",
                    "lease_version",
                    "candidate",
                ],
            ),
            (
                "delivery.evidence.submit",
                &[
                    "session_id",
                    "expected_session_version",
                    "delivery_payload",
                    "dirty_tree",
                ],
            ),
            (
                "delivery.submit_and_request_review",
                &["session_id", "expected_session_version", "evidence_id"],
            ),
        ];
        for (op, expected) in cases {
            let value = contract("awr_team_command", op);
            let actual = required(&value);
            for &field in expected {
                assert!(actual.contains(field), "{op}: missing {field}");
            }
        }

        let checkpoint = contract("awr_team_command", "session.checkpoint");
        assert_eq!(
            checkpoint["schema"]["properties"]["args"]["properties"]["client_info"]["properties"]["product"]
                ["type"],
            "string"
        );
        assert_eq!(
            checkpoint["schema"]["properties"]["args"]["properties"]["progress"]["properties"]["tests"]
                ["items"]["properties"]["outcome"]["enum"],
            json!(["passed", "failed", "not_run"])
        );
        let report = contract("awr_team_command", "execution.report");
        assert_eq!(
            report["schema"]["properties"]["args"]["allOf"][0]["then"]["required"],
            json!(["output_digest"])
        );
        let candidate = contract("awr_team_command", "delivery.candidate.select");
        assert_eq!(
            candidate["schema"]["properties"]["args"]["properties"]["candidate"]["required"],
            json!(["binding", "manifest"])
        );
        assert!(
            candidate["schema"]["properties"]["args"]["properties"]
                .get("expected_selected_digest")
                .is_some()
        );

        let session_end = contract("awr_team_command", "session.end");
        let session_end_args = &session_end["schema"]["properties"]["args"];
        assert_eq!(session_end_args["additionalProperties"], false);
        assert_eq!(
            session_end_args["required"],
            json!(["session_id", "expected_session_version"])
        );
        assert_eq!(
            session_end_args["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "expected_session_version".to_owned(),
                "session_id".to_owned()
            ]
            .into_iter()
            .collect()
        );
        let session_schema = &session_end_args["properties"]["session_id"];
        assert_eq!(
            field_issue(&json!("雪".repeat(43)), session_schema).unwrap()["constraint"],
            "x-identity-utf8.maxUtf8Bytes"
        );
        assert_eq!(
            field_issue(&json!("session\n"), session_schema).unwrap()["constraint"],
            "x-identity-utf8.rejectControls"
        );
        assert!(!session_end_args.to_string().contains("source_locator"));
    }

    #[test]
    fn optional_nulls_and_operation_conditions_match_backend_inputs() {
        let facts = json!({
            "outcome":"failed","input_digest":"a".repeat(64),"output_digest":null,
            "environment_digest":"b".repeat(64),"observed_paths":[],"note":"failed",
            "executor_stopped":null
        });
        let accepted = [
            (
                "task.assign",
                json!({"assignee_person_id":"person","expected_responsibility_version":"0",
                    "session_id":null,"expected_session_version":null}),
            ),
            (
                "task.assign",
                json!({"assignee_person_id":"person","expected_responsibility_version":"0",
                    "session_id":null}),
            ),
            (
                "task.assign",
                json!({"assignee_person_id":"person","expected_responsibility_version":"0",
                    "expected_session_version":null}),
            ),
            (
                "execution.attest",
                json!({"session_id":"session","expected_session_version":"1",
                    "execution_id":"execution","expected_execution_version":"1",
                    "reviewed_receipt_id":null,"facts":facts}),
            ),
            (
                "execution.reconcile",
                json!({"session_id":"session","expected_session_version":"1",
                    "execution_id":"execution","expected_execution_version":"1",
                    "expected_work_version":"1","reviewed_receipt_id":null,
                    "clear_recovery_block":false,"previous_epoch_recovery":null,"facts":facts}),
            ),
            (
                "handoff.propose",
                json!({"session_id":"session","expected_session_version":"1",
                    "handoff_id":"handoff","kind":"execution","to_person_id":"person",
                    "package":null,"proposed_successor":null,"proposer_execution_id":null,
                    "proposer_fence":null,"expires_at_ms":null,"now_ms":null}),
            ),
            (
                "handoff.inspect",
                json!({"session_id":"session","expected_session_version":"1",
                    "handoff_id":"handoff","expected_handoff_version":"1",
                    "inspector_person_id":"person","now_ms":null}),
            ),
            (
                "handoff.accept",
                json!({"session_id":"session","expected_session_version":"1",
                    "handoff_id":"handoff","expected_handoff_version":"1",
                    "acceptor_person_id":"person",
                    "successor_execution":{"kind":"person","person_id":"person"},
                    "inspection_request_id":"inspect-request","prior_execution_stopped":null,
                    "prior_reconciled":null,"context_reprepared":null,
                    "expected_current_fence":null,"now_ms":null}),
            ),
            (
                "handoff.reject",
                json!({"session_id":"session","expected_session_version":"1",
                    "handoff_id":"handoff","expected_handoff_version":"1",
                    "by_person_id":"person","reason":"rejected","now_ms":null}),
            ),
            (
                "handoff.cancel",
                json!({"session_id":"session","expected_session_version":"1",
                    "handoff_id":"handoff","expected_handoff_version":"1",
                    "by_person_id":"person","reason":"cancelled","now_ms":null}),
            ),
            (
                "handoff.timeout",
                json!({"session_id":"session","expected_session_version":"1",
                    "handoff_id":"handoff","expected_handoff_version":"1","now_ms":null}),
            ),
            (
                "delivery.register_pr",
                json!({"session_id":"session","expected_session_version":"1",
                    "repository":"owner/repo","pr_number":1,"pr_url":"https://example.test/pr/1",
                    "head_sha":"a".repeat(40),"fact_source":"operator_recorded_observation",
                    "observed_at":"2026-01-01T00:00:00Z","merge_sha":null,
                    "test_evidence_id":null,"author_actor_id":null,"owner_person_id":null,
                    "executor_actor_id":null,"gh_submitted":null,"gh_approved":null,"gh_merged":null}),
            ),
            (
                "delivery.observe_pr",
                json!({"session_id":"session","expected_session_version":"1",
                    "delivery_id":"delivery","expected_head_sha":"a".repeat(40),
                    "fact_source":"operator_recorded_observation","observed_at":"x".repeat(65),
                    "gh_approved":null,"gh_merged":null,"merge_sha":null}),
            ),
            (
                "delivery.connector.configure",
                json!({"source_snapshot_id":"snapshot","expected_connector_version":"0",
                    "mapping":{"connector_id":"connector","provider":"git","resource":"repo",
                        "principal_actor_id":"actor","principal_client_id":"client",
                        "fact_source":"caller_declared","enabled":true,"source_locator":null}}),
            ),
            (
                "delivery.candidate.select",
                json!({"source_snapshot_id":"snapshot","session_id":"session","claim_id":"claim",
                    "fence":"1","lease_version":"1","expected_selected_digest":null,
                    "candidate":{"binding":{},"manifest":{}}}),
            ),
            (
                "delivery.source.prepare",
                json!({"source_snapshot_id":"snapshot","candidate_digest":"a".repeat(64),
                    "expected_selection_version":"1","expected_metadata_revision":"1",
                    "expected_source_fingerprint":format!("sha256:{}", "b".repeat(64)),
                    "completion_receipt_id":null,"lease_seconds":5}),
            ),
            (
                "evidence.submit",
                json!({"session_id":"session","expected_session_version":"1","payload":{},
                    "dirty_tree":false,"artifact_text":"report","artifact_hex":null}),
            ),
            (
                "delivery.evidence.submit",
                json!({"session_id":"session","expected_session_version":"1",
                    "delivery_payload":{"delivery_candidate_digest":"a".repeat(64)},
                    "dirty_tree":false,"artifact_text":null,"artifact_hex":"00"}),
            ),
        ];
        for (op, args) in accepted {
            let contract = contract("awr_team_command", op);
            let issue = super::super::input_diagnostics::shape_issue(
                &args,
                &contract["schema"]["properties"]["args"],
                "/args",
            );
            assert!(issue.is_none(), "{op}: {issue:?}");
        }

        for (op, args, field, constraint) in [
            (
                "task.assign",
                json!({"assignee_person_id":"person","expected_responsibility_version":"0",
                    "session_id":"session","expected_session_version":null}),
                "/args/expected_session_version",
                "type",
            ),
            (
                "handoff.accept",
                json!({"session_id":"session","expected_session_version":"1",
                    "handoff_id":"handoff","expected_handoff_version":"1",
                    "acceptor_person_id":"person",
                    "successor_execution":{"kind":"person","person_id":"person"},
                    "prior_execution_stopped":null,"prior_reconciled":null,
                    "context_reprepared":null,"expected_current_fence":null,"now_ms":null}),
                "/args/inspection_request_id",
                "required",
            ),
            (
                "handoff.accept",
                json!({"session_id":"session","expected_session_version":"1",
                    "handoff_id":"handoff","expected_handoff_version":"1",
                    "acceptor_person_id":"person",
                    "successor_execution":{"kind":"person","person_id":"person"},
                    "inspection_request_id":null,"prior_execution_stopped":null,
                    "prior_reconciled":null,"context_reprepared":null,
                    "expected_current_fence":null,"now_ms":null}),
                "/args/inspection_request_id",
                "type",
            ),
            (
                "execution.attest",
                json!({"session_id":"session","expected_session_version":"1",
                    "execution_id":"execution","expected_execution_version":"1",
                    "facts":{"outcome":"succeeded","input_digest":"a".repeat(64),
                        "environment_digest":"b".repeat(64),"observed_paths":[],"note":"done"}}),
                "/args/facts/output_digest",
                "required",
            ),
            (
                "delivery.register_pr",
                json!({"session_id":"session","expected_session_version":"1",
                    "repository":"owner/repo","pr_number":1,"pr_url":"https://example.test/pr/1",
                    "head_sha":"a".repeat(40),"fact_source":"operator_recorded_observation",
                    "observed_at":"x".repeat(65)}),
                "/args/observed_at",
                "maxLength",
            ),
            (
                "evidence.submit",
                json!({"session_id":"session","expected_session_version":"1","payload":{},
                    "dirty_tree":false,"artifact_text":"report","artifact_hex":"00"}),
                "/args/artifact_hex",
                "type",
            ),
            (
                "delivery.evidence.submit",
                json!({"session_id":"session","expected_session_version":"1",
                    "delivery_payload":{"delivery_candidate_digest":"a".repeat(64)},
                    "dirty_tree":false,"artifact_text":"report","artifact_hex":"00"}),
                "/args/artifact_hex",
                "type",
            ),
        ] {
            let contract = contract("awr_team_command", op);
            let issue = super::super::input_diagnostics::shape_issue(
                &args,
                &contract["schema"]["properties"]["args"],
                "/args",
            )
            .unwrap_or_else(|| panic!("{op}: expected {field} {constraint}"));
            assert_eq!(issue["invalid_field"], field, "{op}");
            assert_eq!(issue["constraint"], constraint, "{op}");
        }

        let register = contract("awr_team_command", "delivery.register_pr");
        let observed_at = &register["schema"]["properties"]["args"]["properties"]["observed_at"];
        let register_limit = &observed_at["allOf"][1];
        assert_eq!(register_limit["maxLength"], 64);
        assert_eq!(register_limit["x-max-utf8-bytes"], 64);
        let over_bytes = "雪".repeat(22);
        assert_eq!(over_bytes.chars().count(), 22);
        assert_eq!(over_bytes.len(), 66);
        let issue = super::super::input_diagnostics::shape_issue(
            &json!({"session_id":"session","expected_session_version":"1",
                "repository":"owner/repo","pr_number":1,
                "pr_url":"https://example.test/pr/1","head_sha":"a".repeat(40),
                "fact_source":"operator_recorded_observation","observed_at":over_bytes}),
            &register["schema"]["properties"]["args"],
            "/args",
        )
        .unwrap();
        assert_eq!(issue["invalid_field"], "/args/observed_at");
        assert_eq!(issue["constraint"], "x-max-utf8-bytes");
        assert_eq!(issue["maximum_bytes"], 64);

        let observe = contract("awr_team_command", "delivery.observe_pr");
        assert!(
            observe["schema"]["properties"]["args"]["properties"]["observed_at"]
                .get("x-max-utf8-bytes")
                .is_none()
        );
    }

    #[test]
    fn claim_inspect_exposes_record_and_work_or_session_selectors() {
        let value = contract("awr_team_query", "claim.inspect");
        let constraints = value["schema"]["allOf"].as_array().unwrap();
        assert!(
            constraints
                .iter()
                .any(|branch| branch["required"] == json!(["claim_id"]))
        );
        assert!(constraints.iter().any(|branch| {
            branch["anyOf"] == json!([{"required":["work_id"]},{"required":["session_id"]}])
        }));
        assert!(
            value["value_sources"]
                .to_string()
                .contains("actual claim receipt")
        );
    }

    #[test]
    fn query_projection_selects_both_then_and_else_constraints() {
        let artifact = contract("awr_team_query", "artifact.content");
        assert!(
            artifact["schema"]["allOf"]
                .as_array()
                .unwrap()
                .iter()
                .any(|branch| {
                    branch["properties"]["max_context_bytes"]["maximum"]
                        == WorkstreamQuery::MAX_ARTIFACT_BYTES
                })
        );
        let capabilities = contract("awr_team_query", "capabilities");
        let constraints = capabilities["schema"]["allOf"].as_array().unwrap();
        assert!(
            constraints
                .iter()
                .any(|branch| { branch["properties"]["max_context_bytes"]["type"] == "null" })
        );
        assert!(constraints.iter().all(|branch| branch.get("if").is_none()));
    }

    #[test]
    fn handoff_and_integration_sources_are_receipt_bound_or_explicitly_unknown() {
        let handoff = contract("awr_team_command", "handoff.accept");
        let handoff_sources = &handoff["value_sources"];
        let handoff_text = handoff_sources.to_string();
        assert!(handoff_text.contains("/receipt/data/handoff_id"));
        assert!(handoff_text.contains("/receipt/data/version"));
        assert!(handoff_text.contains("/receipt/data/inspection_request_id"));
        assert!(handoff_text.contains("/data/runtime/last_fence"));
        assert_eq!(handoff_sources["unclassified_required_fields"], json!([]));
        assert!(handoff_text.contains("/receipt/data/consumption/member_person_id"));
        assert!(handoff_text.contains("/receipt/data/consumption/execution_instance"));

        let integration = contract("awr_team_command", "delivery.integration.prepare");
        let integration_sources = &integration["value_sources"];
        let integration_text = integration_sources.to_string();
        for pointer in [
            "/data/connectors/*/connector_version",
            "/data/stored_selection/binding_digest",
            "/data/review/evidence_id",
            "/data/review/round_id",
            "/data/review/decisions/*/decision_id",
        ] {
            assert!(integration_text.contains(pointer), "missing {pointer}");
        }
        assert_eq!(
            integration_sources["unclassified_required_fields"],
            json!([])
        );
        assert!(!integration_text.contains("remain caller-observed"));
        assert!(integration_text.contains("source=unknown"));
    }

    #[test]
    fn unknown_and_malformed_requests_are_bounded_without_echoing_values() {
        let catalog = super::super::catalog();
        assert_eq!(
            describe(&request("awr_team_command", "private-unknown-op"), &catalog),
            Err(ContractError::UnknownOperation)
        );
        let error = public_error(&request("awr_team_command", "private-unknown-op"));
        assert_eq!(error["code"], "UnsupportedOperation");
        assert_eq!(error["invalid_field"], "/op");
        assert!(!error.to_string().contains("private-unknown-op"));
        assert_eq!(
            public_error(&json!({
                "protocol_version":0,
                "tool":"awr_team_command",
                "op":"private-unknown-op"
            }))["code"],
            "InvalidInput"
        );
        assert_eq!(
            describe(
                &json!({"protocol_version":1,"tool":"bad","op":"capabilities"}),
                &catalog
            ),
            Err(ContractError::InvalidInput)
        );
    }
}
