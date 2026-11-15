//! Project-bound access to the existing local usage ledger.
use awr_core::{Error, Result};
use awr_runtime::{AttestedUsageProject, UsageRuntimeError};
use awr_store::Store;
use rmcp::model::{CallToolResult, Tool, ToolAnnotations};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) const NAMES: &[&str] = &["awr_usage_record", "awr_usage_query"];

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
}

fn text() -> Value {
    json!({"type":"string","minLength":1})
}

fn count() -> Value {
    json!({"type":"integer","minimum":0})
}

fn optional(value: Value) -> Value {
    json!({"anyOf":[value,{"type":"null"}]})
}

fn money() -> Value {
    object(
        json!({"currency":text(),"micros":count()}),
        &["currency", "micros"],
    )
}

fn cost() -> Value {
    json!({"oneOf":[
        {"const":"unknown"},
        object(json!({"actual":money()}), &["actual"]),
        object(json!({"api_equivalent_estimate":object(
            json!({"amount":money(),"pricing_version":text()}), &["amount","pricing_version"]
        )}), &["api_equivalent_estimate"])
    ]})
}

fn tokens() -> Value {
    object(
        json!({"input":count(),"output":count(),"cached_input":count()}),
        &["input", "output", "cached_input"],
    )
}

fn record_schema() -> Value {
    let receipt = object(
        json!({
            "receipt_id":text(),"project_id":text(),"provider_namespace":text(),"provider":text(),
            "call_id":text(),"model":text(),"session_id":text(),"occurred_at_ms":count(),
            "channel":{"enum":["model","compaction"]},
            "attribution":object(json!({"work_id":text(),"execution_id":text(),"workstream_id":optional(text())}),
                &["work_id","execution_id"]),"tokens":optional(tokens()),"cost":cost()
        }),
        &[
            "receipt_id",
            "project_id",
            "provider_namespace",
            "provider",
            "call_id",
            "model",
            "session_id",
            "occurred_at_ms",
            "channel",
            "attribution",
            "cost",
        ],
    );
    let correction = object(
        json!({
            "correction_id":text(),"project_id":text(),"target_receipt_id":text(),"corrected_at_ms":count(),
            "reason":text(),"prior_cost":cost(),"new_cost":cost(),"actor":text()
        }),
        &[
            "correction_id",
            "project_id",
            "target_receipt_id",
            "corrected_at_ms",
            "reason",
            "prior_cost",
            "new_cost",
            "actor",
        ],
    );
    let allocation = object(
        json!({
            "allocation_id":text(),"project_id":text(),"receipt_id":text(),"rule":optional(text()),
            "shares":{"type":"array","items":object(json!({"workstream_id":optional(text()),"micros":count()}), &["micros"])},
            "recorded_at_ms":count()
        }),
        &[
            "allocation_id",
            "project_id",
            "receipt_id",
            "shares",
            "recorded_at_ms",
        ],
    );
    let counter = object(
        json!({
            "scope":object(json!({"project_id":text(),"provider_namespace":text(),"provider":text(),
                "model":text(),"session_id":text(),"counter_epoch":text()}),
                &["project_id","provider_namespace","provider","model","session_id","counter_epoch"]),
            "observed_at_ms":count(),"tokens":tokens()
        }),
        &["scope", "observed_at_ms", "tokens"],
    );
    let interval = object(
        json!({"execution_id":text(),
            "interval":object(json!({"start_ms":count(),"end_ms":count()}), &["start_ms","end_ms"])
        }),
        &["execution_id", "interval"],
    );
    let variants: Vec<_> = [
        ("receipt", receipt),
        ("correction", correction),
        ("allocation", allocation),
        ("counter", counter),
        ("interval", interval),
    ]
    .into_iter()
    .map(|(kind, data)| {
        object(
            json!({"kind":{"const":kind},"data":data}),
            &["kind", "data"],
        )
    })
    .collect();
    json!({"oneOf":variants})
}

fn query_schema() -> Value {
    let mut schema = object(
        json!({
            "kind":{"enum":["receipts","corrections","allocations","counters","intervals","bindings","summary","observation"]},
            "offset":{"type":"integer","minimum":0,"default":0},
            "limit":{"type":"integer","minimum":1,"maximum":100,"default":20},
            "apply_corrections":{"type":"boolean","default":true},
            "expected_calls":optional(count()),"expected_time_ms":optional(count())
        }),
        &["kind"],
    );
    schema["allOf"] = json!([
        {"if":{"properties":{"kind":{"enum":["summary","observation"]}}},
         "then":{"not":{"anyOf":[{"required":["offset"]},{"required":["limit"]}]}}},
        {"if":{"properties":{"kind":{"not":{"enum":["summary","receipts"]}}}},
         "then":{"not":{"required":["apply_corrections"]}}},
        {"if":{"properties":{"kind":{"not":{"const":"observation"}}}},
         "then":{"not":{"anyOf":[{"required":["expected_calls"]},{"required":["expected_time_ms"]}]}}}
    ]);
    schema
}

pub(crate) fn tools() -> Vec<Tool> {
    [
        (NAMES[0],
            "Record local usage with expected_revision, a stable request_key and record {kind,data}. Returns the saved record and intake receipt.",
            object(json!({"expected_revision":count(),"request_key":{"type":"string","minLength":1,"maxLength":256},
                "record":record_schema()}), &["expected_revision","request_key","record"]), false),
        (NAMES[1],
            "Read local usage details, costs, time or coverage. Returns bounded records or separate actual, estimated and unknown totals.",
            query_schema(), true),
    ].into_iter().map(|(name, description, schema, read_only)| {
        let mut tool = Tool::new(name, description, schema.as_object().unwrap().clone());
        tool.annotations = Some(ToolAnnotations::new().read_only(read_only).destructive(false).idempotent(true).open_world(false));
        tool
    }).collect()
}

fn error(error: UsageRuntimeError) -> Error {
    match error {
        UsageRuntimeError::Store(error) => error,
        error => Error::InvalidInput(error.to_string()),
    }
}

pub(crate) fn call(root: &Path, name: &str, input: Value) -> Result<CallToolResult> {
    let database = crate::project::database(root)?;
    let mut store = if name == NAMES[0] {
        Store::open_existing(&database)?
    } else {
        Store::read_snapshot(&database, 256 * 1024 * 1024)?
    };
    let project = store.project_by_root(root)?;
    let binding = AttestedUsageProject {
        project_id: project.id.to_string(),
        project: project.id,
        attested: true,
    };
    let value = match name {
        "awr_usage_record" => binding.record(&mut store, input).map_err(error)?,
        "awr_usage_query" => binding.query(&store, input).map_err(error)?,
        _ => return Err(Error::Unsupported("usage operation".into())),
    };
    Ok(CallToolResult::structured(value))
}
