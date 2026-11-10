use awr_source::{GatePolicy, WriteGate};
use std::fs;

#[test]
fn gates_missing_file_disables_all_hooks() {
    let root = std::env::temp_dir().join(format!("awr-gates-{}", awr_core::Id::new()));
    fs::create_dir_all(&root).unwrap();
    let policy = GatePolicy::load(&root).unwrap();
    assert_eq!(policy.hooks.write_gate, WriteGate::Off);
    assert!(!policy.hooks.write_gate_paths);
    assert!(!policy.hooks.completion_reminder);
    assert!(!policy.verify.require_work);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn gates_parses_hooks_and_verify_with_an_empty_check_table() {
    let policy = GatePolicy::parse("version = 1\n[checks]\n[hooks]\nwrite_gate = 'ask'\nwrite_gate_exempt = ['.local/**']\nwrite_gate_paths = true\ncompletion_reminder = true\n[verify]\nrequire_work = true\n").unwrap();
    assert_eq!(policy.hooks.write_gate, WriteGate::Ask);
    assert!(
        policy
            .hooks
            .is_exempt(std::path::Path::new(".local/nested/file"))
            .unwrap()
    );
    assert!(
        !policy
            .hooks
            .is_exempt(std::path::Path::new("src/file"))
            .unwrap()
    );
    assert!(policy.hooks.write_gate_paths);
    assert!(policy.hooks.completion_reminder);
    assert!(policy.verify.require_work);
}

#[test]
fn gates_rejects_unknown_tables_keys_versions_modes_and_globs() {
    for text in [
        "version = 2",
        "[hooks]",
        "version = 1\n[unknown]",
        "version = 1\n[hooks]\nunknown = true",
        "version = 1\n[checks]\nrequired = true",
        "version = 1\n[verify]\nrequire_wrok = true",
        "version = 1\n[verify]\nrequire_work = 'yes'",
        "version = 1\n[verify]\nrequire_work = 1",
        "version = 1\n[hooks]\nwrite_gate = 'maybe'",
        "version = 1\n[hooks]\nwrite_gate_exempt = ['[']",
    ] {
        assert!(
            GatePolicy::parse(text).is_err(),
            "accepted invalid policy: {text}"
        );
    }
}
