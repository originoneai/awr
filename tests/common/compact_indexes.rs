use serde_json::{Value, json};

/// Check the public projection contract, including every field outside the index.
pub fn assert_projection(legacy: &Value, compact: &Value, count: usize) {
    assert!(compact.get("response_budget").is_none());
    if count == 0 {
        assert_eq!(compact, legacy, "zero omissions must add no bookkeeping");
        return;
    }
    let path = "context.work_context.omitted_chunks";
    assert!(
        compact["context"]["work_context"]
            .get("omitted_chunks")
            .is_none()
    );
    assert_eq!(
        compact["response_view"]["detail_counts"],
        json!({path:count})
    );
    assert!(
        compact["response_view"]["omitted_fields"]
            .as_array()
            .unwrap()
            .contains(&json!(path))
    );
    assert!(compact["response_view"]["full_result"].is_object());
    let mut restored = compact.clone();
    restored["context"]["work_context"]["omitted_chunks"] =
        legacy["context"]["work_context"]["omitted_chunks"].clone();
    restored["response_view"]["omitted_fields"] = legacy["response_view"]["omitted_fields"].clone();
    restored["response_view"]
        .as_object_mut()
        .unwrap()
        .remove("detail_counts");
    assert_eq!(
        &restored, legacy,
        "only the optional omission index may change"
    );
    let tokens = |v: &Value| {
        awr_context::token_count(&format!("{}\n", serde_json::to_string_pretty(v).unwrap()))
    };
    assert!(
        tokens(compact) < tokens(legacy),
        "the concise projection must save serialized tokens"
    );
}
