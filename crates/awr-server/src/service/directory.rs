use super::*;
use awr_team::ProjectDirectoryPlan;
use awr_team_pg::DirectoryApplyRequest;

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Read,
    Preview,
    Apply,
    Outcome,
}

impl Operation {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "preview" => Some(Self::Preview),
            "apply" => Some(Self::Apply),
            "outcome" => Some(Self::Outcome),
            _ => None,
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Read => "directory.read",
            Self::Preview => "directory.preview",
            Self::Apply => "directory.apply",
            Self::Outcome => "directory.outcome",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadBody {
    protocol_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewBody {
    protocol_version: u32,
    plan: ProjectDirectoryPlan,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutcomeBody {
    protocol_version: u32,
    request_id: String,
}

fn invalid() -> PgError {
    PgError::Protocol("invalid project directory request".into())
}

pub(super) async fn operate(
    state: &StateData,
    project: &ProjectBinding,
    token: &str,
    args: Value,
    op: Operation,
) -> Result<Value, PgError> {
    if args.get("protocol_version").and_then(Value::as_u64) != Some(1) {
        return Err(invalid());
    }
    let store = state.store.directory();
    let tenant = &project.tenant_id;
    let project = &project.project_id;
    match op {
        Operation::Read => {
            let request: ReadBody = serde_json::from_value(args).map_err(|_| invalid())?;
            debug_assert_eq!(request.protocol_version, 1);
            store.read(tenant, project, token).await
        }
        Operation::Preview => {
            let request: PreviewBody = serde_json::from_value(args).map_err(|_| invalid())?;
            debug_assert_eq!(request.protocol_version, 1);
            store.preview(tenant, project, token, &request.plan).await
        }
        Operation::Apply => {
            let request: DirectoryApplyRequest =
                serde_json::from_value(args).map_err(|_| invalid())?;
            store.apply(tenant, project, token, &request).await
        }
        Operation::Outcome => {
            let request: OutcomeBody = serde_json::from_value(args).map_err(|_| invalid())?;
            debug_assert_eq!(request.protocol_version, 1);
            store
                .outcome(tenant, project, token, &request.request_id)
                .await
        }
    }
}

pub(super) async fn dispatch(
    State(state): State<Arc<StateData>>,
    Path((key, operation)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !allowed_request(&state, &headers) {
        return denied();
    }
    let Some(token) = bearer(&headers) else {
        return denied();
    };
    let Some(project) = state.projects.get(&key) else {
        return denied();
    };
    let Some(op) = Operation::parse(&operation) else {
        return error_response(invalid());
    };
    let Ok(args) = serde_json::from_slice::<Value>(&body) else {
        return error_response(invalid());
    };
    if reject_forged_authority_fields(&args).is_err() {
        return denied();
    }
    let Ok(_permit) = state.permits.try_acquire() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, json!({"code":"Busy"}));
    };
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        audited(
            &state,
            project,
            token,
            op.action(),
            None,
            operate(&state, project, token, args, op),
        ),
    )
    .await;
    match result {
        Ok(Ok(value)) => response(StatusCode::OK, value),
        Ok(Err(error)) => error_response(error),
        Err(_) => unavailable(),
    }
}

pub(super) fn schema(op: Operation) -> Value {
    let identity =
        json!({"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_-]+$"});
    let unit = json!({"type":"object","additionalProperties":false,"required":["id","name"],
        "properties":{"id":identity,"name":{"type":"string","minLength":1,"maxLength":512}}});
    let references = json!({"type":"array","maxItems":128,"uniqueItems":true,"items":identity});
    let plan = json!({"type":"object","additionalProperties":false,
        "required":["organization","departments","project_department_ids","members"],
        "properties":{"organization":{"anyOf":[unit,{"type":"null"}]},
            "departments":{"type":"array","maxItems":128,"items":unit},
            "project_department_ids":references,
            "members":{"type":"array","maxItems":256,"items":{"type":"object","additionalProperties":false,
                "required":["person_id","department_ids"],"properties":{"person_id":{"type":"string","minLength":1,"maxLength":128},"department_ids":references}}}}});
    let mut result = json!({"type":"object","additionalProperties":false,"required":["protocol_version"],
        "properties":{"protocol_version":{"type":"integer","const":1}}});
    let mut fields = serde_json::Map::new();
    match op {
        Operation::Read => {}
        Operation::Preview => {
            fields.insert("plan".into(), plan);
        }
        Operation::Apply => {
            fields.insert("plan".into(), plan);
            fields.insert("request_id".into(), identity.clone());
            fields.insert(
                "expected_version".into(),
                json!({"type":"string","pattern":"^(0|[1-9][0-9]*)$","maxLength":19}),
            );
            fields.insert(
                "expected_plan_digest".into(),
                json!({"type":"string","pattern":"^[0-9a-f]{64}$"}),
            );
        }
        Operation::Outcome => {
            fields.insert("request_id".into(), identity);
        }
    }
    for (name, schema) in fields {
        result["required"].as_array_mut().unwrap().push(json!(name));
        result["properties"][name] = schema;
    }
    result
}
