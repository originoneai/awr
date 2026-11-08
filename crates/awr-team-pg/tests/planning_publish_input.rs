use awr_team_pg::PlanningPublishRequest;
use serde_json::{Value, json};

fn activation() -> Value {
    json!({
        "protocol_version": 1,
        "request_id": "publish-example",
        "candidate_id": "candidate-example",
        "candidate_digest": "a".repeat(64),
        "activate": true
    })
}

#[test]
fn activation_requires_an_explicit_impact_confirmation_before_dispatch() {
    let error = serde_json::from_value::<PlanningPublishRequest>(activation())
        .expect_err("missing confirmation must fail before reserving or publishing a request");
    assert!(error.to_string().contains("impact_proven"));
}

#[test]
fn publish_only_omission_keeps_its_existing_canonical_input() {
    let mut input = activation();
    input.as_object_mut().unwrap().remove("activate");
    let request: PlanningPublishRequest = serde_json::from_value(input).unwrap();
    assert!(!request.activate);
    assert!(!request.impact_proven);
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "protocol_version": 1,
            "request_id": "publish-example",
            "candidate_id": "candidate-example",
            "candidate_digest": "a".repeat(64),
            "activate": false,
            "impact_proven": false,
            "stopped_work_ids": []
        })
    );
}

#[test]
fn explicit_confirmation_keeps_true_and_intentional_refusal_distinct() {
    for confirmed in [true, false] {
        let mut input = activation();
        input["impact_proven"] = json!(confirmed);
        input["stopped_work_ids"] = json!([]);
        let request: PlanningPublishRequest = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(request.impact_proven, confirmed);
        assert_eq!(serde_json::to_value(request).unwrap(), input);
    }
}

#[test]
fn malformed_duplicate_and_unknown_fields_remain_invalid() {
    for bad in [Value::Null, json!("true"), json!(1), json!({})] {
        let mut input = activation();
        input["impact_proven"] = bad;
        assert!(serde_json::from_value::<PlanningPublishRequest>(input).is_err());
    }
    let mut input = activation();
    input["impact_proven"] = json!(true);
    input["unknown_field"] = json!(true);
    assert!(serde_json::from_value::<PlanningPublishRequest>(input).is_err());

    let duplicate = format!(
        r#"{{"request_id":"example","candidate_id":"example","candidate_digest":"{}","activate":true,"impact_proven":false,"impact_proven":true}}"#,
        "a".repeat(64)
    );
    assert!(serde_json::from_str::<PlanningPublishRequest>(&duplicate).is_err());
}

#[test]
fn decode_errors_expose_only_the_fixed_missing_field_diagnostic() {
    let error = serde_json::from_value::<PlanningPublishRequest>(activation()).unwrap_err();
    let diagnostic = PlanningPublishRequest::decode_error(error)
        .planning_input_diagnostic()
        .unwrap();
    assert_eq!(diagnostic["invalid_field"], "/impact_proven");
    assert_eq!(diagnostic["request_reserved"], false);
    assert_eq!(diagnostic["publication_performed"], false);
    assert_eq!(diagnostic["activation_performed"], false);

    let mut input = activation();
    input["impact_proven"] = json!("private-value-sentinel");
    let error = serde_json::from_value::<PlanningPublishRequest>(input).unwrap_err();
    let error = PlanningPublishRequest::decode_error(error);
    assert!(error.planning_input_diagnostic().is_none());
    assert_eq!(error.to_string(), "invalid planning publish");
    assert!(!error.to_string().contains("private-value-sentinel"));
}
