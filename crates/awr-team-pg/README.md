# awr-team-pg

PostgreSQL coordination store for Team V1. The personal SQLite runtime does
not depend on this crate.

This crate uses `tokio-postgres` rather than `sqlx`. sqlx 0.8 pulls
`sqlx-sqlite` and conflicts with the personal runtime's `rusqlite` /
`libsqlite3-sys` link.

```sh
docker compose -f docker/team-postgres.yml up -d
export AWR_TEAM_TEST_DATABASE_URL='postgres://postgres:awr-test@127.0.0.1:55432/postgres'
cargo test -p awr-team-pg --features pg-tests -- --test-threads=1
```

The tests read `AWR_TEAM_TEST_DATABASE_URL` only (the default is the URL above),
never the service's `AWR_TEAM_DATABASE_URL`. They accept a loopback host only,
create a uniquely named database per test process and create the cluster-wide
role `awr_app` with a fixed test password, so point them at a throwaway
PostgreSQL 17 and never at a shared or production server. Run them with one test
thread: the suites share one server and several tests exercise lock order and
concurrent writers. `awr-server migrate` is a no-op when `awr_team.schema_state`
already matches; it refuses to start when the version is missing or unexpected.

Default `cargo test --workspace` compiles this crate but does not run the
PostgreSQL tests. The `Team PostgreSQL verification` workflow
(`.github/workflows/team-postgres.yml`) runs them, for this crate and for
`awr-server`, against a PostgreSQL 17 service container on every change under
`crates/`.

Source publish is `ingest` → independent `approve` → `activate`. Unsafe paths
are rejected before any snapshot row is written. Default tests cover path
safety without Postgres; `--features pg-tests` covers activation on PostgreSQL 17.

Read APIs (`ReadStore`) use REPEATABLE READ snapshots and `(project_revision, event_index)`
cursors. They are not linked from the personal SQLite CLI.
