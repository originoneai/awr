//! Bounded advice for an already rejected input, using only known schema fields.
//! This is not a JSON Schema validator, admission gate or request transformation.
use super::*;

pub(in crate::service) fn add_input_diagnostic(error: &mut Value, input: &Value, write: bool) {
    if error["code"] != "InvalidInput" || error.get("invalid_field").is_some() {
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
        // The selected operation can add required fields, a closed args shape,
        // or a const-conditioned args constraint. Domain checks stay authoritative.
        let op = input.get("op").and_then(Value::as_str);
        for branch in schema["allOf"].as_array().into_iter().flatten() {
            if branch["if"]["properties"]["op"]["const"].as_str() != op || op.is_none() {
                continue;
            }
            if let Some(detail) = shape_issue(input, &branch["then"], "") {
                attach(error, detail, write);
                return;
            }
            if let Some(args) = input.get("args") {
                if let Some(detail) =
                    conditional_args_issue(args, &branch["then"]["properties"]["args"])
                {
                    attach(error, detail, write);
                    return;
                }
            }
        }
        return;
    };
    attach(error, detail, write);
}

fn attach(error: &mut Value, detail: Value, write: bool) {
    error
        .as_object_mut()
        .unwrap()
        .extend(detail.as_object().unwrap().clone());
    if !write {
        error["action_guidance"] = json!({
            "condition":"The read query's input shape was rejected.",
            "basis":detail,
            "next_action":"Correct the indicated field using the discovered query schema, then resubmit the read query.",
            "recheck":"After correcting inputs. Current permission and state checks still apply; no execution or command replay is authorized."
        });
        return;
    }
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
    let below_minimum =
        input.as_i64().is_some_and(|n| n < 0) && schema["minimum"].as_u64().is_some();
    let outside_unsigned_bounds = input.as_u64().is_some_and(|number| {
        schema["minimum"].as_u64().is_some_and(|n| number < n)
            || schema["maximum"].as_u64().is_some_and(|n| number > n)
    });
    if below_minimum || outside_unsigned_bounds {
        let mut detail = issue(path, "bounds");
        for bound in ["minimum", "maximum"] {
            if let Some(value) = schema.get(bound) {
                detail[bound] = value.clone();
            }
        }
        return Some(detail);
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

/// Apply a const-conditioned constraint published on one operation's args object.
/// This reads the same catalog schema; it does not parse or admit the command.
fn conditional_args_issue(args: &Value, args_schema: &Value) -> Option<Value> {
    for branch in args_schema["allOf"].as_array()? {
        let Some(properties) = branch["if"]["properties"].as_object() else {
            continue;
        };
        let Some(required) = branch["if"]["required"].as_array() else {
            continue;
        };
        let names = required
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if names.is_empty() || names.len() != required.len() {
            continue;
        }
        let matched = names.iter().all(|field| {
            properties
                .get(*field)
                .and_then(|schema| schema.get("const"))
                .is_some_and(|expected| args.get(*field) == Some(expected))
        });
        if matched {
            if let Some(detail) = shape_issue(args, &branch["then"], "/args") {
                return Some(detail);
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

    fn checkpoint(args: Value) -> Value {
        let mut input = command(args);
        input["op"] = json!("session.checkpoint");
        input
    }

    fn checkpoint_args() -> Value {
        json!({
            "session_id":"session",
            "expected_session_version":"1",
            "context_hash":"a".repeat(64),
            "next_action":"Review the focused change",
            "open_loops":["Independent review pending"]
        })
    }

    fn prepare(args: Value) -> Value {
        let mut input = command(args);
        input["op"] = json!("execution.prepare");
        input
    }

    fn prepare_args() -> Value {
        json!({
            "session_id":"session",
            "expected_session_version":"1",
            "claim_id":"claim",
            "expected_fence":"1",
            "expected_lease_version":"1",
            "expected_work_version":"1",
            "input_digest":"a".repeat(64),
            "declared_scope":["src/filter.js", "test/filter.test.js"]
        })
    }

    #[test]
    fn execution_prepare_rejections_guide_missing_fields_and_foreign_args() {
        let mut args = prepare_args();
        args.as_object_mut().unwrap().remove("claim_id");
        args.as_object_mut().unwrap().remove("declared_scope");
        args["claimed_trust"] = Value::Null;
        args["execution_id"] = Value::Null;

        let error = rejected(prepare(args.clone()), true);
        assert_eq!(error["invalid_field"], "/args/claim_id");
        assert_eq!(error["constraint"], "required");

        args["claim_id"] = json!("claim");
        let error = rejected(prepare(args.clone()), true);
        assert_eq!(error["invalid_field"], "/args/declared_scope");
        assert_eq!(error["constraint"], "required");

        args["declared_scope"] = json!(["src/filter.js"]);
        args["private-key-sentinel"] = json!("private-value-sentinel");
        let error = rejected(prepare(args), true);
        assert_eq!(error["invalid_field"], "/args");
        assert_eq!(error["constraint"], "unexpected_field");
        let fields = error["expected_fields"].as_array().unwrap();
        assert_eq!(fields.len(), 8);
        assert!(fields.contains(&json!("claim_id")));
        assert!(fields.contains(&json!("declared_scope")));
        assert!(!fields.contains(&json!("claimed_trust")));
        assert!(!fields.contains(&json!("execution_id")));
        assert!(!error.to_string().contains("private-"));
        assert!(error.to_string().len() < 1024);
    }

    #[test]
    fn execution_prepare_required_fields_are_diagnosed_without_affecting_valid_shapes() {
        for field in prepare_args().as_object().unwrap().keys() {
            let mut args = prepare_args();
            args.as_object_mut().unwrap().remove(field);
            let error = rejected(prepare(args), true);
            assert_eq!(error["invalid_field"], format!("/args/{field}"));
            assert_eq!(error["constraint"], "required");
        }
        for scope in [json!([]), json!(["src/filter.js"])] {
            let mut args = prepare_args();
            args["declared_scope"] = scope;
            assert!(rejected(prepare(args), true).get("invalid_field").is_none());
        }
        for (field, value, constraint) in [
            ("claim_id", Value::Null, "type"),
            ("claim_id", json!(""), "minLength"),
            ("declared_scope", Value::Null, "type"),
            ("declared_scope", json!("private-value-sentinel"), "type"),
            ("input_digest", Value::Null, "type"),
            ("input_digest", json!("private-value-sentinel"), "pattern"),
            ("expected_work_version", json!("0"), "pattern"),
        ] {
            let mut args = prepare_args();
            args[field] = value;
            let error = rejected(prepare(args), true);
            assert_eq!(error["invalid_field"], format!("/args/{field}"));
            assert_eq!(error["constraint"], constraint);
            assert!(!error.to_string().contains("private-"));
        }
    }

    #[test]
    fn checkpoint_rejections_name_missing_fields_and_hide_unknown_args() {
        for field in ["session_id", "context_hash"] {
            let mut args = checkpoint_args();
            args.as_object_mut().unwrap().remove(field);
            let error = rejected(checkpoint(args), true);
            assert_eq!(error["invalid_field"], format!("/args/{field}"));
            assert_eq!(error["constraint"], "required");
        }

        let mut args = checkpoint_args();
        args["private-key-sentinel"] = json!("private-value-sentinel");
        let error = rejected(checkpoint(args), true);
        assert_eq!(error["invalid_field"], "/args");
        assert_eq!(error["constraint"], "unexpected_field");
        assert_eq!(error["expected_fields"].as_array().unwrap().len(), 8);
        assert!(!error.to_string().contains("private-"));
        assert!(error.to_string().len() < 1024);
    }

    #[test]
    fn checkpoint_schema_accepts_required_shape_and_optional_feedback() {
        let schema = catalog()
            .into_iter()
            .find(|tool| tool.name == "awr_team_command")
            .unwrap()
            .input_schema;
        let branch = schema["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|branch| branch["if"]["properties"]["op"]["const"] == "session.checkpoint")
            .unwrap();
        let args_schema = &branch["then"]["properties"]["args"];
        assert_eq!(args_schema["additionalProperties"], false);
        assert_eq!(
            args_schema["required"],
            json!([
                "session_id",
                "expected_session_version",
                "context_hash",
                "next_action",
                "open_loops"
            ])
        );
        let fields = args_schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            fields,
            std::collections::BTreeSet::from([
                "client_info",
                "context_hash",
                "expected_session_version",
                "next_action",
                "open_loops",
                "progress",
                "session_id",
                "usage",
            ])
        );

        let required_only = checkpoint(checkpoint_args());
        assert!(rejected(required_only, true).get("invalid_field").is_none());

        let mut optional = checkpoint_args();
        optional["client_info"] = json!({"product":"Synthetic Agent"});
        optional["progress"] = json!({"phase":"testing","summary":"Focused checks are running."});
        optional["usage"] = json!({
            "source":"synthetic-test",
            "source_ref":"unit",
            "counter_id":"counter",
            "scope":"host_session",
            "coverage":"complete",
            "observed_at_unix_ms":1,
            "input_tokens":1
        });
        assert!(
            rejected(checkpoint(optional), true)
                .get("invalid_field")
                .is_none()
        );
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
    fn malformed_query_bounds_are_precise_and_do_not_replace_validator_diagnostics() {
        for value in [json!(-1), json!(65536)] {
            let error = rejected(
                json!({"protocol_version":1,"op":"work.next","limit":value}),
                false,
            );
            assert_eq!(error["invalid_field"], "/limit");
            assert_eq!(error["constraint"], "bounds");
            assert_eq!(error["minimum"], 1);
            assert_eq!(error["maximum"], 100);
            assert!(
                error["action_guidance"]["next_action"]
                    .as_str()
                    .unwrap()
                    .contains("read query")
            );
            assert!(error.get("next_step").is_none());
        }
        let input = json!({"protocol_version":1,"op":"work.next","max_context_bytes":65536});
        let query: WorkstreamQuery = serde_json::from_value(input.clone()).unwrap();
        let mut error = query
            .validate()
            .unwrap_err()
            .query_input_diagnostic()
            .unwrap();
        let original = error.clone();
        add_input_diagnostic(&mut error, &input, false);
        assert_eq!(error, original);
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

    fn start(args: Value) -> Value {
        let mut input = command(args);
        input["op"] = json!("execution.start");
        input
    }

    fn start_args() -> Value {
        json!({
            "session_id":"session",
            "expected_session_version":"1",
            "execution_id":"execution",
            "expected_execution_version":"1",
            "expected_work_version":"1",
            "claim_id":"claim",
            "expected_fence":"1",
            "expected_lease_version":"1",
            "execution_mode":"caller_managed"
        })
    }

    #[test]
    fn execution_start_missing_mode_names_the_required_field() {
        let mut args = start_args();
        args.as_object_mut().unwrap().remove("execution_mode");
        let error = rejected(start(args), true);
        assert_eq!(error["invalid_field"], "/args/execution_mode");
        assert_eq!(error["constraint"], "required");
        assert!(!error.to_string().contains("private-"));
        assert!(error.to_string().len() < 1024);
    }

    #[test]
    fn execution_start_rejections_name_bounds_types_and_conditional_digests() {
        for field in [
            "session_id",
            "expected_session_version",
            "execution_id",
            "expected_execution_version",
            "expected_work_version",
            "claim_id",
            "expected_fence",
            "expected_lease_version",
            "execution_mode",
        ] {
            let mut args = start_args();
            args.as_object_mut().unwrap().remove(field);
            let error = rejected(start(args), true);
            assert_eq!(error["invalid_field"], format!("/args/{field}"), "{field}");
            assert_eq!(error["constraint"], "required", "{field}");
        }
        for (field, value, constraint) in [
            ("expected_work_version", json!("0"), "pattern"),
            ("expected_fence", json!("0"), "pattern"),
            ("expected_lease_version", json!("0"), "pattern"),
            ("expected_execution_version", json!("0"), "pattern"),
            ("expected_session_version", json!("0"), "pattern"),
            ("execution_id", Value::Null, "type"),
            ("execution_mode", Value::Null, "type"),
            ("claim_id", json!(1), "type"),
            ("expected_fence", json!(1), "type"),
            ("execution_mode", json!("private-value-sentinel"), "enum"),
            (
                "expected_input_digest",
                json!("private-value-sentinel"),
                "pattern",
            ),
        ] {
            let mut args = start_args();
            args[field] = value;
            let error = rejected(start(args), true);
            assert_eq!(error["invalid_field"], format!("/args/{field}"), "{field}");
            assert_eq!(error["constraint"], constraint, "{field}");
            assert!(!error.to_string().contains("private-"));
            assert!(error.to_string().len() < 1024);
        }

        let mut unknown = start_args();
        unknown["private-key-sentinel"] = json!("private-value-sentinel");
        let error = rejected(start(unknown), true);
        assert_eq!(error["invalid_field"], "/args");
        assert_eq!(error["constraint"], "unexpected_field");
        let fields = error["expected_fields"].as_array().unwrap();
        assert_eq!(fields.len(), 10);
        assert!(fields.contains(&json!("execution_mode")));
        assert!(fields.contains(&json!("expected_input_digest")));
        assert!(!error.to_string().contains("private-"));

        let digest = "a".repeat(64);
        for mode in ["caller_managed", "reference_write_v1"] {
            let mut args = start_args();
            args["execution_mode"] = json!(mode);
            let omitted = rejected(start(args.clone()), true);
            args["expected_input_digest"] = Value::Null;
            let null_digest = rejected(start(args.clone()), true);
            args["expected_input_digest"] = json!(digest);
            let supplied = rejected(start(args), true);
            if mode == "caller_managed" {
                assert!(omitted.get("invalid_field").is_none(), "{mode} omitted");
                assert!(null_digest.get("invalid_field").is_none(), "{mode} null");
                assert!(supplied.get("invalid_field").is_none(), "{mode} digest");
            } else {
                assert_eq!(omitted["invalid_field"], "/args/expected_input_digest");
                assert_eq!(omitted["constraint"], "required");
                assert_eq!(null_digest["invalid_field"], "/args/expected_input_digest");
                assert_eq!(null_digest["constraint"], "type");
                assert!(supplied.get("invalid_field").is_none(), "{mode} digest");
            }
        }

        let mut acquire = command(json!({"expected_work_version":"0"}));
        acquire["op"] = json!("claim.acquire");
        assert!(rejected(acquire, true).get("invalid_field").is_none());
        let evidence = command(json!({
            "session_id":"session",
            "expected_session_version":"1",
            "payload":{"note":"measured"},
            "dirty_tree":true,
            "input_digest":null
        }));
        assert!(rejected(evidence, true).get("invalid_field").is_none());
        let mut prepare_zero = prepare_args();
        prepare_zero["expected_work_version"] = json!("0");
        assert_eq!(
            rejected(prepare(prepare_zero), true)["constraint"],
            "pattern"
        );
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
