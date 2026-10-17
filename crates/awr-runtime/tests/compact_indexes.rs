use awr_runtime::compact_prepared_response;
use serde_json::{Value, json};
#[path = "../../../tests/common/compact_indexes.rs"]
mod contract;

fn prepared(count: usize) -> Value {
    let gaps = ["evidence_currency_unknown", "evidence_stale", "evidence_level_unverified", "missing_evidence_bindings"]
        .iter().enumerate().map(|(n, code)| json!({"code":code,"reference":format!("E-{n}"),"reason":"Retain the exact binding diagnostic", "scope":["W","public acceptance"], "source_sha":"a".repeat(40), "locator":format!("reports/check-{n}.json")})).collect::<Vec<_>>();
    let omitted = (0..count).map(|n| json!({"key":format!("optional-history-{n}"),"section":"delta","reason":"insufficient_budget_for_whole_chunk","entities":[{"kind":"event","id":format!("event-{n}"),"revision":n}]})).collect::<Vec<_>>();
    json!({"ok":true,"stage":"prepared","ready":true,"work":{"source_ref":{"locator":"work.yaml","pointer":"/work_items/0"}},
        "context":{"completeness":{"complete":true,"assessment_scope":"Context facts only; not verification or completion", "evidence_gaps":gaps},
            "work_context":{"rendered_context":format!("HARD RULE: {}", "Preserve every source fact. ".repeat(500)),
                "context_hash":"same-required-context-hash","identity":{"source_versions":[{"revision":7,"fingerprint":"source-hash"}]},"omitted_chunks":omitted}},
        "response_view":{"view":"summary","omitted_fields":[],"full_result":{"tool":"awr_work_prepare","basis":"fresh query; compare project_revision and context_hash"}},
        "diagnostics":[{"code":"active_claim","session_id":"current-session"}],"active_claims":[{"session_id":"current-session"}],"blocker":"Review the input", "management":{"required":true},"continuity":{"session":"current-session"},"completion_claimed":false,"context_consumed":false})
}

#[test]
fn optional_indexes_are_bounded_without_changing_required_facts_or_small_responses() {
    let mut sizes = Vec::new();
    for count in [0, 1, 20, 200] {
        for view in ["summary", "action"] {
            let mut legacy = prepared(count);
            legacy["response_view"]["view"] = json!(view);
            let compact = compact_prepared_response(legacy.clone());
            contract::assert_projection(&legacy, &compact, count);
            if view == "summary" && count > 0 {
                sizes.push(serde_json::to_vec(&compact).unwrap().len());
            }
            assert_eq!(
                compact_prepared_response(compact.clone()),
                compact,
                "projection is idempotent"
            );
        }
    }
    assert!(
        sizes[2] - sizes[0] < 10,
        "summary grows only with the count's digits"
    );
}

#[test]
fn full_incomplete_unknown_reasons_and_missing_detail_queries_remain_verbatim() {
    let original = prepared(20);
    for (path, value) in [
        ("/ok", json!(false)),
        ("/stage", json!("other")),
        ("/response_view/view", json!("full")),
        ("/context/completeness/complete", json!(false)),
        ("/response_view/full_result", Value::Null),
        ("/response_view/omitted_fields", Value::Null),
        (
            "/context/work_context/omitted_chunks/0/reason",
            json!("required_source_missing"),
        ),
    ] {
        let mut value_with_guard = original.clone();
        *value_with_guard.pointer_mut(path).unwrap() = value;
        assert_eq!(
            compact_prepared_response(value_with_guard.clone()),
            value_with_guard,
            "{path}"
        );
    }
}

#[test]
fn partial_writes_and_unknown_recovery_receipts_remain_verbatim() {
    for outcome in ["partial", "unknown"] {
        let failure = json!({"ok":false,"write_outcome":outcome,
            "error":{"code":"SourceWriteFailed","message":"Inspect interrupted write before retry"},
            "proposal":{"patch":{"changes":{"status":"in_progress"}}},
            "operation":{"request_id":"stable-request","recovery":{"tool":"awr_operation_get"}},
            "event":{"payload":{"released_claim_ids":[],"source_revision":7}},
            "diagnostics":[{"scope":["W"],"locator":"work.yaml","code":"write_receipt_required"}]});
        assert_eq!(compact_prepared_response(failure.clone()), failure);
        assert_eq!(
            awr_runtime::summarize_work_response(failure.clone()),
            failure
        );
    }
}
