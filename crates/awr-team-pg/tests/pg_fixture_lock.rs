#![cfg(feature = "pg-tests")]

mod common;

/// One failing test must not take the rest of its binary down with a
/// `PoisonError` that hides which test failed first. Before the fixture lock
/// tolerated poisoning, a single failure in `pg_operator_agent` turned eleven
/// healthy tests of the same binary red.
#[tokio::test]
async fn a_panic_while_the_fixture_is_held_does_not_poison_it_for_the_next_test() {
    // The failing "test" runs on its own thread and runtime and panics while it
    // still holds the fixture lock, which is exactly how a failed assertion in
    // the middle of a test leaves it.
    let failing = std::thread::spawn(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (_guard, _admin, _db) = common::fresh_team_schema().await;
                panic!("a test failed while it held the fixture");
            });
    });
    assert!(
        failing.join().is_err(),
        "the simulated failing test must panic while holding the fixture"
    );

    // The next test still gets a rebuilt, fully migrated schema.
    let (_guard, admin, _db) = common::fresh_team_schema().await;
    let version: i32 = admin
        .query_one(
            "SELECT version FROM awr_team.schema_state WHERE component='awr_team'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(version, awr_team_pg::EXPECTED_SCHEMA_VERSION);
}
