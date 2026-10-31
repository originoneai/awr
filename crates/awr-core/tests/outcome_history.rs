//! DEC-041: the outcome history counts what the receipts say and nothing else. Each fixture under
//! tests/fixtures/assessment/history/ is one pre-registered case (C10, C16, C17, P02 and P05 of the
//! assessment corpus among them) with the exact pointers it expects; the invariants below hold for all.
use awr_core::*;
use serde_json::Value;
use std::{fs, path::PathBuf};

fn fixtures() -> Vec<(String, Value)> {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/assessment/history");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            (name, value)
        })
        .collect()
}

fn run(case: &Value) -> std::result::Result<OutcomeHistory, OutcomeHistoryError> {
    let input: HistoryInput = serde_json::from_value(case["input"].clone()).unwrap();
    let request: HistoryRequest = serde_json::from_value(case["request"].clone()).unwrap();
    aggregate_outcome_history(&input, &request)
}

fn walk(value: &Value, visit: &mut impl FnMut(&str, &Value)) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                visit(key, inner);
                walk(inner, visit);
            }
        }
        Value::Array(items) => items.iter().for_each(|inner| walk(inner, visit)),
        _ => {}
    }
}

#[test]
fn every_fixture_gives_exactly_what_it_expects() {
    let cases = fixtures();
    assert!(cases.len() >= 20, "{}", cases.len());
    for (name, case) in &cases {
        let result = run(case);
        if let Some(error) = case["error"].as_str() {
            let got = format!("{:?}", result.as_ref().expect_err(name));
            assert!(
                got.starts_with(error),
                "{name}: expected {error}, got {got}"
            );
            continue;
        }
        let history = result.unwrap_or_else(|e| panic!("{name}: {e}"));
        let value = serde_json::to_value(&history).unwrap();
        let expect = case["expect"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: no expect"));
        assert!(!expect.is_empty(), "{name}");
        for (pointer, wanted) in expect {
            let got = match pointer.strip_suffix("#len") {
                Some(array) => value
                    .pointer(array)
                    .and_then(Value::as_array)
                    .map(|items| Value::from(items.len())),
                None => value.pointer(pointer).cloned(),
            };
            assert_eq!(got.as_ref(), Some(wanted), "{name}: {pointer} in {value:#}");
        }
    }
}

#[test]
fn no_report_holds_a_rate_a_score_or_a_float() {
    for (name, case) in fixtures() {
        let Ok(history) = run(&case) else { continue };
        let value = serde_json::to_value(&history).unwrap();
        walk(&value, &mut |key, inner| {
            for banned in ["rate", "percent", "score", "probab", "saving", "confidence"] {
                assert!(!key.contains(banned), "{name}: key {key}");
            }
            if let Value::Number(number) = inner {
                assert!(!number.is_f64(), "{name}: {key} is a float");
            }
        });
        // What is not derived is said, in the words the corpus pre-registered.
        assert_eq!(
            history.not_derived,
            [
                "model_success_rate",
                "dollar_savings",
                "semantic_understanding"
            ]
        );
        assert_eq!(history.version, OUTCOME_HISTORY_VERSION);
    }
}

#[test]
fn the_same_input_request_and_instant_give_the_same_report_and_hash() {
    for (name, case) in fixtures() {
        let Ok(first) = run(&case) else { continue };
        let second = run(&case).unwrap();
        assert_eq!(first, second, "{name}");
        assert!(first.history_hash.starts_with("sha256:"), "{name}");
        assert_eq!(first.history_hash.len(), "sha256:".len() + 64, "{name}");
    }
    // The hash moves with the question: another instant, another report.
    let (_, case) = fixtures()
        .into_iter()
        .find(|(name, _)| name.starts_with("h08-"))
        .unwrap();
    let mut later = case.clone();
    later["request"]["as_of_ms"] = Value::from(100_000);
    assert_ne!(
        run(&case).unwrap().history_hash,
        run(&later).unwrap().history_hash
    );
}

#[test]
fn the_order_the_records_arrive_in_changes_nothing() {
    for (name, case) in fixtures() {
        let Ok(first) = run(&case) else { continue };
        let mut shuffled = case.clone();
        for family in ["receipts", "executions", "usage", "works", "sessions"] {
            if let Some(items) = shuffled["input"][family].as_array_mut() {
                items.reverse();
            }
        }
        assert_eq!(first, run(&shuffled).unwrap(), "{name}");
    }
}

#[test]
fn a_count_never_exceeds_the_works_it_counts() {
    for (name, case) in fixtures() {
        let Ok(history) = run(&case) else { continue };
        let works: usize = history.cohorts.iter().map(|c| c.works).sum();
        assert_eq!(works, history.sample.works_in_scope, "{name}");
        for cohort in &history.cohorts {
            let s = &cohort.signals;
            assert!(
                s.completion_reported
                    + s.completion_under_earlier_standard
                    + s.completion_standard_unknown
                    + s.conflicting
                    + s.cancelled
                    + s.open
                    >= cohort.works.saturating_sub(
                        s.accepted_by_human + s.accepted_by_ai + s.accepted_by_delegate
                    ),
                "{name}"
            );
            assert_eq!(
                cohort.cost.works_with_usage + cohort.cost.works_without_usage,
                cohort.works,
                "{name}"
            );
        }
        let (seen, of) = history.coverage.works_with_outcome;
        assert!(seen <= of && of == history.sample.works_in_scope, "{name}");
    }
}

#[test]
fn the_acceptance_digest_does_not_depend_on_the_order_of_the_criteria() {
    let a = vec!["Review the report".to_string(), "Deliver it".to_string()];
    let b = vec!["Deliver it".to_string(), "Review the report".to_string()];
    assert_eq!(acceptance_digest(&a), acceptance_digest(&b));
    assert_ne!(acceptance_digest(&a), acceptance_digest(&a[..1]));
    assert!(acceptance_digest(&a).starts_with("sha256:"));
}

#[test]
fn a_request_that_is_not_a_question_is_refused() {
    let (_, case) = fixtures().into_iter().next().unwrap();
    let input: HistoryInput = serde_json::from_value(case["input"].clone()).unwrap();
    for request in [
        HistoryRequest {
            as_of_ms: -1,
            cohort_by: CohortBy::None,
            work_keys: None,
        },
        HistoryRequest {
            as_of_ms: 1,
            cohort_by: CohortBy::None,
            work_keys: Some(vec!["".into()]),
        },
    ] {
        assert_eq!(
            aggregate_outcome_history(&input, &request),
            Err(OutcomeHistoryError::InvalidRequest)
        );
    }
    let mut twice = input.clone();
    twice.works.push(twice.works[0].clone());
    assert_eq!(
        aggregate_outcome_history(
            &twice,
            &HistoryRequest {
                as_of_ms: 1,
                cohort_by: CohortBy::None,
                work_keys: None
            }
        ),
        Err(OutcomeHistoryError::DuplicateWork)
    );
}
