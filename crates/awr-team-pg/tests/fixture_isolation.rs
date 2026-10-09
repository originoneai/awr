//! Needs no database, so the normal CI runs it. The PostgreSQL suites reach
//! their database only through `common`: it accepts a loopback target from
//! `AWR_TEAM_TEST_DATABASE_URL` and creates a process-private database. A suite
//! that read the runtime `AWR_TEAM_DATABASE_URL` instead would drop and rebuild
//! `awr_team` in whatever database a developer's shell has exported for the
//! real service. `pg_skeleton` did exactly that until it moved onto `common`.

use std::path::{Path, PathBuf};

fn rust_files(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn no_pg_suite_reads_the_runtime_database_url() {
    // The server's PostgreSQL suites include this crate's `common` by path.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&manifest.join("tests"), &mut files);
    rust_files(&manifest.join("../awr-server/tests"), &mut files);
    assert!(files.len() > 60, "the scan must see both crates' PG suites");
    let offenders: Vec<_> = files
        .iter()
        .filter(|path| !path.ends_with("fixture_isolation.rs"))
        .flat_map(|path| {
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .enumerate()
                .filter(|(_, line)| {
                    line.contains("AWR_TEAM_DATABASE_URL")
                        && (line.contains("env::var(") || line.contains("env::var_os("))
                })
                .map(|(number, _)| format!("{}:{}", path.display(), number + 1))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "these tests read the runtime database URL instead of using tests/common: {offenders:?}"
    );
}
