//! DEC-041 through the runtime: the question is the caller's, and a question that is not one is refused as such.
use awr_core::*;
use awr_runtime::outcome_history;
use awr_store::Store;

#[test]
fn an_empty_project_has_an_empty_history_and_a_bad_question_is_refused() {
    let root = std::env::temp_dir().join(format!("awr-outcome-history-runtime-{}", Id::new()));
    std::fs::create_dir(&root).unwrap();
    let mut store = Store::open(&root.join("state.db")).unwrap();
    let project = store
        .register_project(&root, "fixture", "Fixture")
        .unwrap()
        .id;
    let ask = |as_of_ms: i64| {
        outcome_history(
            &store,
            project,
            None,
            &HistoryRequest {
                as_of_ms,
                cohort_by: CohortBy::ProviderModel,
                work_keys: None,
            },
        )
    };
    let empty = ask(1_000).unwrap();
    assert_eq!(empty.sample.works_in_scope, 0);
    assert!(empty.cohorts.is_empty());
    assert_eq!(empty.coverage.works_with_outcome, (0, 0));
    assert_eq!(empty, ask(1_000).unwrap());
    assert_ne!(empty.history_hash, ask(2_000).unwrap().history_hash);
    assert!(matches!(ask(-1), Err(Error::InvalidInput(_))));
    let _ = std::fs::remove_dir_all(&root);
}
