//! Bounded advice for an already rejected input, using only known schema fields.
//! This is not a JSON Schema validator, admission gate or request transformation.
use super::*;

pub(in crate::service) fn add_input_diagnostic(error: &mut Value, input: &Value, write: bool) {
    if error["code"] != "InvalidInput" {
        return;
    }
    let name = if write {
        "awr_team_command"
    } else {
        "awr_team_query"
    };
    let schema = catalog()
        .into_iter()
        .find(|tool| tool.name == name)
        .unwrap()
        .input_schema;
    let Some(detail) = shape_issue(input, &Value::Object((*schema).clone()), "") else {
        // The selected operation can add required fields or a closed args shape.
        // All domain checks and successful requests remain untouched.
        let op = input.get("op").and_then(Value::as_str);
        for branch in schema["allOf"].as_array().into_iter().flatten() {
            if branch["if"]["properties"]["op"]["const"].as_str() != op || op.is_none() {
                continue;
            }
            if let Some(detail) = shape_issue(input, &branch["then"], "") {
                attach(error, detail);
                return;
            }
        }
        return;
    };
    attach(error, detail);
}

fn attach(error: &mut Value, detail: Value) {
    error
        .as_object_mut()
        .unwrap()
        .extend(detail.as_object().unwrap().clone());
    // Preserve existing actionable guidance, including session-resume instructions
    // and the decimal-string wire format. Add advice only when it was absent.
    if error.get("next_step").is_none() {
        error["next_step"] = json!(
            "Correct this field using the selected operation's discovered input schema. After an uncertain command outcome, inspect the original request before retrying."
        );
    }
}

fn issue(path: &str, constraint: &str) -> Value {
    json!({"invalid_field":if path.is_empty() { "/" } else { path },"constraint":constraint})
}

fn shape_issue(input: &Value, schema: &Value, path: &str) -> Option<Value> {
    let required = schema["required"].as_array();
    if let Some(fields) = required {
        for field in fields.iter().filter_map(Value::as_str) {
            if input.is_object() && input.get(field).is_none() {
                return Some(issue(&format!("{path}/{field}"), "required"));
            }
        }
    }
    if schema["additionalProperties"] == false {
        if let (Some(values), Some(fields)) = (input.as_object(), schema["properties"].as_object())
        {
            if values.keys().any(|key| !fields.contains_key(key)) {
                let mut detail = issue(path, "unexpected_field");
                // Unknown names can themselves be secrets. Return the container
                // and its fixed, public field set instead of echoing any key.
                detail["expected_fields"] = json!(fields.keys().collect::<Vec<_>>());
                return Some(detail);
            }
        }
    }
    if let Some(kind) = schema.get("type") {
        let matches = |name: &str| match name {
            "object" => input.is_object(),
            "array" => input.is_array(),
            "string" => input.is_string(),
            "boolean" => input.is_boolean(),
            "integer" => input.is_i64() || input.is_u64(),
            "null" => input.is_null(),
            _ => true,
        };
        let valid = if let Some(name) = kind.as_str() {
            matches(name)
        } else {
            kind.as_array()
                .is_none_or(|kinds| kinds.iter().filter_map(Value::as_str).any(matches))
        };
        if !valid {
            return Some(issue(path, "type"));
        }
    }
    if let Some(expected) = schema.get("const") {
        if input != expected {
            return Some(issue(path, "const"));
        }
    }
    if let Some(choices) = schema["enum"].as_array() {
        if !choices.contains(input) {
            return Some(issue(path, "enum"));
        }
    }
    if let Some(text) = input.as_str() {
        let length = text.chars().count() as u64;
        for (key, bad) in [
            (
                "minLength",
                schema["minLength"].as_u64().is_some_and(|n| length < n),
            ),
            (
                "maxLength",
                schema["maxLength"].as_u64().is_some_and(|n| length > n),
            ),
        ] {
            if bad {
                return Some(issue(path, key));
            }
        }
        let bad_pattern = match schema["pattern"].as_str() {
            Some("^[0-9a-f]{64}$") => {
                text.len() != 64
                    || !text
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }
            Some("^(0|[1-9][0-9]*)$") => !decimal(text, true),
            Some("^[1-9][0-9]*$") => !decimal(text, false),
            _ => false,
        };
        if bad_pattern {
            return Some(issue(path, "pattern"));
        }
    }
    if let Some(number) = input.as_u64() {
        if schema["minimum"].as_u64().is_some_and(|n| number < n)
            || schema["maximum"].as_u64().is_some_and(|n| number > n)
        {
            return Some(issue(path, "bounds"));
        }
    }
    // Never walk payloads, maps, submitted artifacts or caller-controlled paths.
    // Only root properties and the known args object are diagnostic locations.
    if let Some(fields) = schema["properties"].as_object() {
        for (name, field_schema) in fields {
            if let Some(value) = input.get(name) {
                if value.is_null() && !required.is_some_and(|r| r.contains(&json!(name))) {
                    continue;
                }
                if path.is_empty() || path == "/args" {
                    if let Some(detail) =
                        shape_issue(value, field_schema, &format!("{path}/{name}"))
                    {
                        return Some(detail);
                    }
                }
            }
        }
    }
    None
}

fn decimal(text: &str, zero_allowed: bool) -> bool {
    !text.is_empty()
        && (text.len() == 1 || !text.starts_with('0'))
        && text.bytes().all(|b| b.is_ascii_digit())
        && text
            .parse::<i64>()
            .is_ok_and(|n| n > 0 || zero_allowed && n == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(input: Value, write: bool) -> Value {
        let mut error = json!({"code":"InvalidInput","message":"invalid input"});
        add_input_diagnostic(&mut error, &input, write);
        error
    }

    fn command(args: Value) -> Value {
        json!({"protocol_version":1,"request_id":"request","op":"evidence.submit",
            "workstream_id":"00000000000000000000000001","work_id":"work","coordinator_epoch":"epoch",
            "expected_project_revision":"1","expected_authority_version":"1","expected_ownership_version":"1",
            "expected_contract_hash":"a".repeat(64),"args":args})
    }

    #[test]
    fn evidence_rejection_points_to_required_fields_and_the_closed_container() {
        let args = json!({"session_id":"session","expected_session_version":"1","payload":null,"dirty_tree":false});
        let mut input = command(args.clone());
        input["args"].as_object_mut().unwrap().remove("dirty_tree");
        assert_eq!(rejected(input, true)["invalid_field"], "/args/dirty_tree");
        let mut input = command(args);
        input["args"]["synthetic-secret-key"] = json!("synthetic-secret-value");
        let error = rejected(input, true);
        assert_eq!(error["invalid_field"], "/args");
        assert_eq!(error["constraint"], "unexpected_field");
        assert!(
            error["expected_fields"]
                .as_array()
                .unwrap()
                .contains(&json!("artifact_text"))
        );
        assert!(!error.to_string().contains("synthetic-secret"));
        assert!(error.to_string().len() < 1024);
    }

    #[test]
    fn diagnostics_do_not_echo_values_or_malformed_operation_names() {
        let mut input = command(
            json!({"session_id":"session","expected_session_version":"1","payload":{},"dirty_tree":false}),
        );
        input["expected_project_revision"] = json!("synthetic-secret-value");
        let error = rejected(input.clone(), true);
        assert_eq!(error["invalid_field"], "/expected_project_revision");
        assert_eq!(error["constraint"], "pattern");
        assert!(!error.to_string().contains("synthetic-secret"));
        input["op"] = json!("synthetic-secret-operation");
        input["expected_project_revision"] = json!("1");
        let error = rejected(input, true);
        assert_eq!(error["invalid_field"], "/op");
        assert!(!error.to_string().contains("synthetic-secret"));
    }

    #[test]
    fn inspection_requires_its_record_selector_and_generic_payload_is_untouched() {
        for (op, field) in [
            ("review.inspect", "/review_round_id"),
            ("evidence.inspect", "/evidence_id"),
        ] {
            let error = rejected(
                json!({"protocol_version":1,"op":op,"work_id":"work"}),
                false,
            );
            assert_eq!(error["invalid_field"], field);
            assert_eq!(error["constraint"], "required");
        }
        for payload in [
            Value::Null,
            json!(true),
            json!("report"),
            json!([1, 2]),
            json!({"synthetic-secret-key":"synthetic-secret-value"}),
        ] {
            let error = rejected(
                command(
                    json!({"session_id":"session","expected_session_version":"1","payload":payload,"dirty_tree":false}),
                ),
                true,
            );
            assert!(error.get("invalid_field").is_none());
        }
        let mut forbidden = json!({"code":"Forbidden"});
        add_input_diagnostic(&mut forbidden, &json!({}), true);
        assert_eq!(forbidden, json!({"code":"Forbidden"}));
    }

    #[test]
    fn review_rejections_point_to_note_reason_and_current_session_version_without_values() {
        for (op, args, field, constraint) in [
            (
                "work.rework",
                json!({"session_id":"session","expected_session_version":"1","round_id":"round","reason":"private-value-sentinel"}),
                "/args/note",
                "required",
            ),
            (
                "review.return",
                json!({"session_id":"session","expected_session_version":"1","round_id":"round","note":"private-value-sentinel"}),
                "/args/reason",
                "required",
            ),
            (
                "review.decide",
                json!({"session_id":"session","expected_session_version":"1","round_id":"round","reason":"reviewed","decision":"private-value-sentinel"}),
                "/args/decision",
                "enum",
            ),
            (
                "work.complete",
                json!({"session_id":"session","expected_session_version":"1","evidence_id":"evidence","context_complete":"private-value-sentinel"}),
                "/args/context_complete",
                "type",
            ),
            (
                "work.rework",
                json!({"session_id":"session","expected_session_version":2,"round_id":"round","note":"revised"}),
                "/args/expected_session_version",
                "type",
            ),
        ] {
            let mut input = command(args);
            input["op"] = json!(op);
            let error = rejected(input, true);
            assert_eq!(error["invalid_field"], field, "{op}");
            assert_eq!(error["constraint"], constraint, "{op}");
            assert!(!error.to_string().contains("private-value-sentinel"));
            assert!(error.to_string().len() < 1024);
        }
        let mut input = command(
            json!({"session_id":"session","expected_session_version":"1","round_id":"round","note":"revised","private-key-sentinel":"private-value-sentinel"}),
        );
        input["op"] = json!("work.rework");
        let error = rejected(input.clone(), true);
        assert_eq!(error["invalid_field"], "/args");
        assert_eq!(error["constraint"], "unexpected_field");
        assert_eq!(error["expected_fields"].as_array().unwrap().len(), 4);
        assert!(!error.to_string().contains("private-"));
        let mut state = json!({"code":"ReworkRequiresReturnedReview","action_guidance":{"next_action":"Inspect review.inspect"}});
        let before = state.clone();
        add_input_diagnostic(&mut state, &input, true);
        assert_eq!(state, before);
    }
}
