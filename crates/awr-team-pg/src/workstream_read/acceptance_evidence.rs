//! Deterministic per-criterion evidence evaluation for `work.snapshot`.
//!
//! Computes each acceptance criterion's evidence state strictly from:
//! - the current `WorkContract` (`acceptance` and optional V7 `examples`),
//! - the current delivery candidate's verification runs (`collaboration.verification`), and
//! - the latest review round on the current contract and current candidate (`collaboration.review`).
//!
//! No model calls, no inference across unbound criteria, and no reuse of
//! stale candidates or returned reviews.
use awr_core::ExampleKind;
use awr_team::WorkContract;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Outcome precedence when multiple connectors report runs for the same check
/// on the current candidate. Fail-closed: `failed` beats `running`/`unknown`/`passed`.
fn outcome_rank(outcome: &str) -> u8 {
    match outcome {
        "failed" => 4,
        "running" => 3,
        "unknown" => 2,
        "passed" => 1,
        _ => 0,
    }
}

/// Evaluate per-criterion evidence status for `work.snapshot`.
///
/// Legacy contracts without `examples` (`contract.examples.is_none()`) return
/// `status: "not_provided"` and `items: null` rather than an empty list.
pub(super) fn evaluate(contract: &WorkContract, observation: &Value) -> Value {
    let collab = &observation["collaboration"];
    let candidate_obj = collab.get("candidate").filter(|v| v.is_object());
    let candidate_digest = candidate_obj
        .and_then(|c| c.get("digest"))
        .and_then(Value::as_str);
    let candidate_current = candidate_obj
        .and_then(|c| c.get("current"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let facts_truncated = collab
        .get("facts_truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let review_obj = collab.get("review").filter(|v| v.is_object());
    let review_state = review_obj
        .and_then(|r| r.get("state"))
        .and_then(Value::as_str);
    let review_current_candidate = candidate_current
        && review_obj
            .and_then(|r| r.get("current_candidate"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

    let Some(examples) = &contract.examples else {
        return json!({
            "status": "not_provided",
            "basis": "contract_examples_absent",
            "candidate_digest": candidate_digest,
            "candidate_current": candidate_current,
            "review_state": review_state,
            "review_current_candidate": review_current_candidate,
            "items": Value::Null,
        });
    };

    // Only verification runs belonging to the current selected candidate count.
    let mut check_results: BTreeMap<&str, (&str, Option<&str>)> = BTreeMap::new();
    if candidate_current {
        if let Some(runs) = collab.get("verification").and_then(Value::as_array) {
            for run in runs {
                let Some(check) = run.get("check").and_then(Value::as_str) else {
                    continue;
                };
                let outcome = run
                    .get("outcome")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let run_id = run.get("run_id").and_then(Value::as_str);
                check_results
                    .entry(check)
                    .and_modify(|existing| {
                        if outcome_rank(outcome) > outcome_rank(existing.0) {
                            *existing = (outcome, run_id);
                        }
                    })
                    .or_insert((outcome, run_id));
            }
        }
    }

    let active_review_state = if review_current_candidate {
        review_state
    } else {
        None
    };

    let runtime_state = observation
        .get("runtime")
        .and_then(|r| r.get("state"))
        .and_then(Value::as_str);
    let current_execution = observation
        .get("execution")
        .and_then(|e| e.get("contract_matches_current"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let current_progress = observation
        .get("progress")
        .and_then(|p| p.get("contract_matches_current"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let work_started = candidate_current
        || current_execution
        || current_progress
        || runtime_state.is_some_and(|s| s != "unclaimed");

    let mut items = Vec::with_capacity(contract.acceptance.len());
    for criterion in &contract.acceptance {
        let mut examples_count = 0usize;
        let mut positive_examples = 0usize;
        let mut negative_examples = 0usize;
        let mut bound_checks: Vec<&str> = Vec::new();
        for example in examples {
            if example.criterion != *criterion {
                continue;
            }
            examples_count += 1;
            match example.kind {
                ExampleKind::Positive => positive_examples += 1,
                ExampleKind::Negative => negative_examples += 1,
            }
            if let Some(check_id) = example.check_id.as_deref() {
                if !bound_checks.contains(&check_id) {
                    bound_checks.push(check_id);
                }
            }
        }

        let checks: Vec<Value> = bound_checks
            .iter()
            .map(|check| {
                let (outcome, run_id) = check_results
                    .get(check)
                    .copied()
                    .unwrap_or(("not_recorded", None));
                json!({
                    "check": check,
                    "outcome": outcome,
                    "run_id": run_id,
                })
            })
            .collect();

        let (status, reason) = if bound_checks.is_empty() {
            // Never infer check coverage for an unbound criterion.
            ("no_bound_check", "no_bound_check")
        } else if !candidate_current {
            if work_started {
                ("waiting", "awaiting_current_candidate")
            } else {
                ("not_started", "work_not_started")
            }
        } else {
            let any_failed = bound_checks.iter().any(|check| {
                check_results
                    .get(check)
                    .is_some_and(|(o, _)| *o == "failed")
            });
            let all_passed = !facts_truncated
                && bound_checks.iter().all(|check| {
                    check_results
                        .get(check)
                        .is_some_and(|(o, _)| *o == "passed")
                });

            if any_failed {
                ("failed", "bound_check_failed")
            } else if active_review_state == Some("rejected") {
                ("failed", "review_returned")
            } else if all_passed && active_review_state == Some("approved") {
                ("supported", "bound_checks_passed_and_review_approved")
            } else if all_passed {
                ("waiting", "awaiting_review_approval")
            } else {
                ("waiting", "awaiting_bound_checks")
            }
        };

        items.push(json!({
            "criterion": criterion,
            "status": status,
            "reason": reason,
            "bound_checks": bound_checks,
            "checks": checks,
            "examples_count": examples_count,
            "positive_examples": positive_examples,
            "negative_examples": negative_examples,
        }));
    }

    json!({
        "status": "provided",
        "basis": "contract_examples_and_current_candidate",
        "candidate_digest": candidate_digest,
        "candidate_current": candidate_current,
        "review_state": review_state,
        "review_current_candidate": review_current_candidate,
        "facts_truncated": facts_truncated,
        "items": items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_team::{ContractExample, WorkId};
    use std::collections::BTreeMap;

    fn example(
        id: &str,
        criterion: &str,
        kind: ExampleKind,
        check_id: Option<&str>,
    ) -> ContractExample {
        ContractExample {
            id: Some(id.into()),
            criterion: criterion.into(),
            kind,
            given: "valid input".into(),
            when: "operation runs".into(),
            then: "expected result holds".into(),
            check_id: check_id.map(str::to_owned),
        }
    }

    fn v7_contract(examples: Vec<ContractExample>) -> WorkContract {
        WorkContract {
            codec: WorkContract::CODEC_V7.into(),
            work_id: WorkId::new("work-1").unwrap(),
            external_key: "WORK-1".into(),
            goals: vec!["G-1".into()],
            hard_rules: vec!["Keep history".into()],
            scope_paths: vec!["src/lib.rs".into()],
            acceptance: vec![
                "Filter returns matching records".into(),
                "Empty query returns all records".into(),
            ],
            required_dependencies: vec![],
            completion_policy: "independent_review".into(),
            verification_requirements: vec!["unit.filter".into()],
            dependency_acceptance: BTreeMap::new(),
            execution_settlement: None,
            examples: Some(examples),
        }
    }

    fn legacy_contract() -> WorkContract {
        WorkContract {
            codec: WorkContract::CODEC.into(),
            work_id: WorkId::new("work-legacy").unwrap(),
            external_key: "WORK-LEGACY".into(),
            goals: vec!["G-1".into()],
            hard_rules: vec!["Keep history".into()],
            scope_paths: vec!["src/lib.rs".into()],
            acceptance: vec!["Legacy criterion".into()],
            required_dependencies: vec![],
            completion_policy: "independent_review".into(),
            verification_requirements: vec!["unit.legacy".into()],
            dependency_acceptance: BTreeMap::new(),
            execution_settlement: None,
            examples: None,
        }
    }

    #[test]
    fn legacy_contract_returns_not_provided_not_empty() {
        let obs = json!({
            "runtime": {"state": "completed"},
            "collaboration": {
                "candidate": {"digest": "cand-1", "current": true},
                "verification": [{"check": "unit.legacy", "run_id": "r1", "outcome": "passed"}],
                "review": {"id": "rev-1", "state": "approved", "current_candidate": true},
                "facts_truncated": false
            }
        });
        let result = evaluate(&legacy_contract(), &obs);
        assert_eq!(result["status"], "not_provided");
        assert!(
            result["items"].is_null(),
            "legacy tasks must return null items with status=not_provided, never an empty array"
        );
    }

    #[test]
    fn supported_requires_all_bound_checks_passed_and_current_approved_review() {
        let contract = v7_contract(vec![
            example(
                "filter-pos",
                "Filter returns matching records",
                ExampleKind::Positive,
                Some("unit.filter.pos"),
            ),
            example(
                "filter-neg",
                "Filter returns matching records",
                ExampleKind::Negative,
                Some("unit.filter.neg"),
            ),
            // Second criterion intentionally has no check_id binding.
            example(
                "empty-pos",
                "Empty query returns all records",
                ExampleKind::Positive,
                None,
            ),
        ]);

        // All bound checks passed + review approved on current candidate -> supported;
        // unbound criterion stays no_bound_check (never inferred!).
        let supported_obs = json!({
            "runtime": {"state": "claimed"},
            "collaboration": {
                "candidate": {"digest": "cand-v1", "current": true},
                "verification": [
                    {"check": "unit.filter.pos", "run_id": "r1", "outcome": "passed"},
                    {"check": "unit.filter.neg", "run_id": "r2", "outcome": "passed"}
                ],
                "review": {"id": "rev-1", "state": "approved", "current_candidate": true},
                "facts_truncated": false
            }
        });
        let res = evaluate(&contract, &supported_obs);
        assert_eq!(res["status"], "provided");
        assert_eq!(res["items"][0]["status"], "supported");
        assert_eq!(res["items"][1]["status"], "no_bound_check");

        // Waiting when review is still open.
        let open_review = json!({
            "runtime": {"state": "claimed"},
            "collaboration": {
                "candidate": {"digest": "cand-v1", "current": true},
                "verification": [
                    {"check": "unit.filter.pos", "run_id": "r1", "outcome": "passed"},
                    {"check": "unit.filter.neg", "run_id": "r2", "outcome": "passed"}
                ],
                "review": {"id": "rev-1", "state": "open", "current_candidate": true},
                "facts_truncated": false
            }
        });
        assert_eq!(
            evaluate(&contract, &open_review)["items"][0]["status"],
            "waiting"
        );

        // Waiting when only one of two bound checks passed.
        let partial_checks = json!({
            "runtime": {"state": "claimed"},
            "collaboration": {
                "candidate": {"digest": "cand-v1", "current": true},
                "verification": [
                    {"check": "unit.filter.pos", "run_id": "r1", "outcome": "passed"}
                ],
                "review": {"id": "rev-1", "state": "approved", "current_candidate": true},
                "facts_truncated": false
            }
        });
        assert_eq!(
            evaluate(&contract, &partial_checks)["items"][0]["status"],
            "waiting"
        );

        // Failed when a bound check fails.
        let failed_check = json!({
            "runtime": {"state": "claimed"},
            "collaboration": {
                "candidate": {"digest": "cand-v1", "current": true},
                "verification": [
                    {"check": "unit.filter.pos", "run_id": "r1", "outcome": "passed"},
                    {"check": "unit.filter.neg", "run_id": "r2", "outcome": "failed"}
                ],
                "review": {"id": "rev-1", "state": "open", "current_candidate": true},
                "facts_truncated": false
            }
        });
        assert_eq!(
            evaluate(&contract, &failed_check)["items"][0]["status"],
            "failed"
        );
    }

    #[test]
    fn stale_candidate_and_returned_review_invalidate_supported_status() {
        let contract = v7_contract(vec![example(
            "filter-pos",
            "Filter returns matching records",
            ExampleKind::Positive,
            Some("unit.filter.pos"),
        )]);

        // Returned (rejected) review on current candidate -> failed, never supported.
        let returned_review = json!({
            "runtime": {"state": "claimed"},
            "collaboration": {
                "candidate": {"digest": "cand-v1", "current": true},
                "verification": [
                    {"check": "unit.filter.pos", "run_id": "r1", "outcome": "passed"}
                ],
                "review": {"id": "rev-1", "state": "rejected", "current_candidate": true},
                "facts_truncated": false
            }
        });
        let res = evaluate(&contract, &returned_review);
        assert_eq!(res["items"][0]["status"], "failed");
        assert_eq!(res["items"][0]["reason"], "review_returned");

        // Stale candidate (candidate.current == false, e.g., after contract binding edit) ->
        // old checks and review are completely ignored.
        let stale_candidate = json!({
            "runtime": {"state": "unclaimed"},
            "collaboration": {
                "candidate": {"digest": "cand-v1", "current": false},
                "verification": [
                    {"check": "unit.filter.pos", "run_id": "r1", "outcome": "passed"}
                ],
                "review": {"id": "rev-1", "state": "approved", "current_candidate": false},
                "facts_truncated": false
            }
        });
        let res = evaluate(&contract, &stale_candidate);
        assert_eq!(res["items"][0]["status"], "not_started");
        assert_eq!(res["items"][0]["checks"][0]["outcome"], "not_recorded");

        // Candidate changed to v2: checks pass on v2, but review approval is still from v1
        // (review.current_candidate == false) -> waiting, NOT supported.
        let old_review_new_candidate = json!({
            "runtime": {"state": "claimed"},
            "collaboration": {
                "candidate": {"digest": "cand-v2", "current": true},
                "verification": [
                    {"check": "unit.filter.pos", "run_id": "r2", "outcome": "passed"}
                ],
                "review": {"id": "rev-1", "state": "approved", "current_candidate": false},
                "facts_truncated": false
            }
        });
        assert_eq!(
            evaluate(&contract, &old_review_new_candidate)["items"][0]["status"],
            "waiting"
        );
    }

    #[test]
    fn unclaimed_work_without_candidate_is_not_started_and_unbound_is_no_bound_check() {
        let contract = v7_contract(vec![example(
            "filter-pos",
            "Filter returns matching records",
            ExampleKind::Positive,
            Some("unit.filter.pos"),
        )]);
        let fresh_obs = json!({
            "runtime": null,
            "collaboration": {
                "candidate": null,
                "verification": [],
                "review": null,
                "facts_truncated": false
            }
        });
        let res = evaluate(&contract, &fresh_obs);
        assert_eq!(res["items"][0]["status"], "not_started");
        assert_eq!(res["items"][1]["status"], "no_bound_check");
    }
}
