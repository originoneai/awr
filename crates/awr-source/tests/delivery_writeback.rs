//! Synthetic source mechanisms only; no native collaboration acceptance credit.
use awr_source::*;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const LEDGER: &str = r#"# Authoritative fixture; preserve this comment.
workstreams:
  version: 1
  definitions:
    - id: "00000000000000000000000001"
      external_key: app
      title: App
      state: active
      authority_version: 1
      goal_keys: [ship]
      acceptance_contracts: []
work_items:
  - id: a
    title: First
    status: planned # This is a source note, not domain acceptance.
    workstream: app
    goals: [ship]
    acceptance: [verified]
    custom: 'Keep exact quoting'
  - id: b
    title: Second
    status: planned
    workstream: app
    goals: [ship]
    acceptance: [verified]
    custom: 'Keep the other work untouched'
"#;

fn note() -> DeliverySourceNote {
    DeliverySourceNote {
        version: 1,
        publication_id: "publication-a".into(),
        work_external_key: "a".into(),
        contract_snapshot_id: "snapshot-a".into(),
        candidate_id: "candidate-a".into(),
        candidate_version: "1".into(),
        candidate_digest: "a".repeat(64),
        selection_version: "1".into(),
        metadata_revision: "1".into(),
        observation_receipt_ids: vec!["observation-a".into()],
        fact_ids: vec!["fact-a".into()],
        completion_reference: None,
    }
}

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-source-note-{}", awr_core::Id::new()));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        fs::write(root.join("ledger.yaml"), LEDGER).unwrap();
        Self { root }
    }
    fn open(&self) -> LockedSourceFile {
        LockedSourceFile::open(&self.root, "ledger.yaml").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn exact_note_preserves_status_comments_other_fields_and_other_work_bytes() {
    let patch = prepare_delivery_source_note(LEDGER.as_bytes(), &note()).unwrap();
    let text = String::from_utf8(patch.after_bytes.clone()).unwrap();
    assert!(text.starts_with("# Authoritative fixture; preserve this comment.\n"));
    assert!(text.contains("status: planned # This is a source note, not domain acceptance."));
    assert!(text.contains("custom: 'Keep exact quoting'"));
    assert!(text.ends_with(&LEDGER[LEDGER.find("  - id: b\n").unwrap()..]));
    let document: Value = serde_yaml_ng::from_str(&text).unwrap();
    assert_eq!(document["work_items"][0]["delivery_sync"], json!(note()));
    assert_eq!(patch.changed_external_keys, ["a"]);
    assert_eq!(patch.before_bytes, LEDGER.as_bytes());
    assert_ne!(patch.before_fingerprint, patch.after_fingerprint);
}

#[test]
fn reindex_proves_unchanged_contract_identity_and_graph() {
    let f = Fixture::new();
    let location = SoleSourceLocation::server_directory(&f.root, "ledger.yaml").unwrap();
    let before = prepare_publish_from_ledger_bytes(
        &location,
        &f.root,
        LEDGER.as_bytes(),
        "fixture-project",
        &PublishPrepOptions::default(),
    )
    .unwrap();
    let patch = prepare_delivery_source_note(LEDGER.as_bytes(), &note()).unwrap();
    let after = prepare_publish_from_ledger_bytes(
        &location,
        &f.root,
        &patch.after_bytes,
        "fixture-project",
        &PublishPrepOptions::default(),
    )
    .unwrap();
    assert_eq!(before.bundle_digest, after.bundle_digest);
    assert_eq!(before.graph_digest, after.graph_digest);
    assert_eq!(before.ledger_identity_digest, after.ledger_identity_digest);
    assert_eq!(before.source_status_notes, after.source_status_notes);
    assert_ne!(before.source_version_digest, after.source_version_digest);
}

#[test]
fn completion_reference_never_overwrites_status_or_finalizes_a_work() {
    let mut n = note();
    n.completion_reference = Some(DeliveryCompletionReference {
        receipt_id: "domain-receipt".into(),
        evidence_id: "evidence-a".into(),
        result_digest: "b".repeat(64),
        artifact_id: "artifact-a".into(),
        artifact_sha256: "c".repeat(64),
    });
    let patch = prepare_delivery_source_note(LEDGER.as_bytes(), &n).unwrap();
    let value: Value = serde_yaml_ng::from_slice(&patch.after_bytes).unwrap();
    assert_eq!(value["work_items"][0]["status"], "planned");
    assert!(
        value["work_items"][0]
            .get("completion_receipt_id")
            .is_none()
    );
    assert_eq!(
        value["work_items"][0]["delivery_sync"]["completion_reference"]["receipt_id"],
        "domain-receipt"
    );
    let mut forged = json!(n);
    forged["verified"] = json!(true);
    assert!(serde_json::from_value::<DeliverySourceNote>(forged).is_err());
}

#[test]
fn identical_note_is_byte_preserving_and_older_or_inconsistent_notes_conflict() {
    let first = prepare_delivery_source_note(LEDGER.as_bytes(), &note()).unwrap();
    let replay = prepare_delivery_source_note(&first.after_bytes, &note()).unwrap();
    assert_eq!(replay.before_bytes, replay.after_bytes);
    assert!(replay.changed_external_keys.is_empty());
    let mut stale = note();
    stale.publication_id = "other".into();
    assert!(matches!(
        prepare_delivery_source_note(&first.after_bytes, &stale),
        Err(Error::SourceConflict(_))
    ));
    stale.metadata_revision = "2".into();
    stale.candidate_digest = "b".repeat(64);
    assert!(prepare_delivery_source_note(&first.after_bytes, &stale).is_err());
    stale.selection_version = "2".into();
    assert!(prepare_delivery_source_note(&first.after_bytes, &stale).is_ok());
}

#[test]
fn malformed_duplicate_missing_and_unknown_source_records_are_refused() {
    for text in [
        "work_items: [{id: a, id: a}]",
        "work_items: [{id: a}, {id: a}]",
        "work_items: [{id: b}]",
        "work_items: [{id: a, delivery_sync: {manual: untouched}}]",
    ] {
        assert!(prepare_delivery_source_note(text.as_bytes(), &note()).is_err());
    }
}

#[test]
fn flow_mapping_and_crlf_remain_supported() {
    for text in [
        "work_items: [{id: a, status: planned}]\n",
        "work_items:\r\n  - id: a\r\n    status: planned\r\n",
    ] {
        let patch = prepare_delivery_source_note(text.as_bytes(), &note()).unwrap();
        let value: Value = serde_yaml_ng::from_slice(&patch.after_bytes).unwrap();
        assert_eq!(value["work_items"][0]["delivery_sync"], json!(note()));
        assert_eq!(value["work_items"][0]["status"], "planned");
        if text.contains("\r\n") {
            assert!(
                !String::from_utf8(patch.after_bytes)
                    .unwrap()
                    .replace("\r\n", "")
                    .contains('\n')
            );
        }
    }
}

#[test]
fn bounded_unique_references_and_explicit_versions_are_required() {
    let mut n = note();
    for value in [
        "0",
        "01",
        "-1",
        "9223372036854775808",
        "18446744073709551616",
    ] {
        n.metadata_revision = value.into();
        assert!(n.validate().is_err());
    }
    n = note();
    n.fact_ids.push("fact-a".into());
    assert!(n.validate().is_err());
    n.fact_ids = (0..33).map(|i| format!("fact-{i}")).collect();
    assert!(n.validate().is_err());
    n = note();
    n.fact_ids.clear();
    assert!(n.validate().is_err());
}

#[test]
fn confined_replace_checks_before_bytes_and_recovers_by_observing_after_bytes() {
    let f = Fixture::new();
    let guard = f.open();
    let identity = guard.identity().clone();
    let patch = prepare_delivery_source_note(&guard.read().unwrap(), &note()).unwrap();
    guard
        .replace(&patch.before_fingerprint, &patch.after_bytes)
        .unwrap();
    assert_eq!(guard.read().unwrap(), patch.after_bytes);
    assert!(
        guard
            .replace(&patch.before_fingerprint, LEDGER.as_bytes())
            .is_err()
    );
    drop(guard);
    let resumed = f.open();
    resumed.verify_identity(&identity).unwrap();
    assert_eq!(
        fingerprint(&resumed.read().unwrap()),
        patch.after_fingerprint
    );
    assert_eq!(
        fs::read_dir(&f.root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .count(),
        0
    );
}

#[test]
fn independent_guards_and_alternate_roots_share_the_same_lock() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("nested")).unwrap();
    fs::write(f.root.join("nested/ledger.yaml"), LEDGER).unwrap();
    let one = LockedSourceFile::open(&f.root, "nested/ledger.yaml").unwrap();
    assert!(matches!(
        LockedSourceFile::open(&f.root.join("nested"), "ledger.yaml"),
        Err(Error::SourceConflict(_))
    ));
    drop(one);
    assert!(LockedSourceFile::open(&f.root.join("nested"), "ledger.yaml").is_ok());
}

#[test]
fn external_edit_is_preserved_and_read_only_source_refuses_replacement() {
    let f = Fixture::new();
    let guard = f.open();
    fs::write(f.root.join("ledger.yaml"), b"external edit\n").unwrap();
    assert!(
        guard
            .replace(&fingerprint(LEDGER.as_bytes()), b"overwritten")
            .is_err()
    );
    assert_eq!(guard.read().unwrap(), b"external edit\n");
    let path = f.root.join("ledger.yaml");
    let permissions = fs::metadata(&path).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&path, readonly).unwrap();
    let error = guard
        .replace(&fingerprint(b"external edit\n"), b"overwritten")
        .unwrap_err();
    assert!(matches!(error, Error::Io(ref e) if e.kind()==std::io::ErrorKind::PermissionDenied));
    fs::set_permissions(&path, permissions).unwrap();
}

#[test]
fn traversal_nonfiles_and_oversized_inputs_are_refused() {
    let f = Fixture::new();
    for path in [
        "",
        "../ledger.yaml",
        "./ledger.yaml",
        "nested/../ledger.yaml",
        "/ledger.yaml",
        "nested\\ledger.yaml",
    ] {
        assert!(LockedSourceFile::open(&f.root, path).is_err());
    }
    fs::create_dir(f.root.join("directory")).unwrap();
    assert!(
        LockedSourceFile::open(&f.root, "directory")
            .unwrap()
            .read()
            .is_err()
    );
    fs::hard_link(f.root.join("ledger.yaml"), f.root.join("other-link")).unwrap();
    assert!(f.open().read().is_err());
    fs::remove_file(f.root.join("other-link")).unwrap();
    fs::write(
        f.root.join("ledger.yaml"),
        vec![b'x'; YAML_READ_CAP as usize + 1],
    )
    .unwrap();
    assert!(f.open().read().is_err());
}

#[cfg(unix)]
#[test]
fn root_parent_leaf_and_lock_symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::create_dir(f.root.join("real")).unwrap();
    fs::write(f.root.join("real/ledger.yaml"), LEDGER).unwrap();
    symlink(f.root.join("real"), f.root.join("alias")).unwrap();
    assert!(LockedSourceFile::open(&f.root.join("alias"), "ledger.yaml").is_err());
    assert!(LockedSourceFile::open(&f.root, "alias/ledger.yaml").is_err());
    let guard = f.open();
    fs::remove_file(f.root.join("ledger.yaml")).unwrap();
    symlink(f.root.join("real/ledger.yaml"), f.root.join("ledger.yaml")).unwrap();
    assert!(guard.read().is_err());
    fs::remove_file(f.root.join("ledger.yaml")).unwrap();
    fs::write(f.root.join("ledger.yaml"), LEDGER).unwrap();
    let lock = fs::read_dir(&f.root)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().ends_with(".lock"))
        .unwrap()
        .path();
    fs::remove_file(&lock).unwrap();
    symlink(f.root.join("ledger.yaml"), &lock).unwrap();
    assert!(guard.read().is_err());
    drop(guard);
    assert!(LockedSourceFile::open(&f.root, "ledger.yaml").is_err());
}

#[cfg(unix)]
#[test]
fn directory_or_lock_replacement_invalidates_live_and_recovered_identity() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("nested")).unwrap();
    fs::write(f.root.join("nested/ledger.yaml"), LEDGER).unwrap();
    let guard = LockedSourceFile::open(&f.root, "nested/ledger.yaml").unwrap();
    let identity = guard.identity().clone();
    fs::rename(f.root.join("nested"), f.root.join("old")).unwrap();
    fs::create_dir(f.root.join("nested")).unwrap();
    fs::write(f.root.join("nested/ledger.yaml"), LEDGER).unwrap();
    assert!(
        guard
            .replace(&fingerprint(LEDGER.as_bytes()), b"wrong directory")
            .is_err()
    );
    drop(guard);
    let recovered = LockedSourceFile::open(&f.root, "nested/ledger.yaml").unwrap();
    assert!(recovered.verify_identity(&identity).is_err());
    assert_eq!(
        fs::read(f.root.join("old/ledger.yaml")).unwrap(),
        LEDGER.as_bytes()
    );
    assert_eq!(
        fs::read(f.root.join("nested/ledger.yaml")).unwrap(),
        LEDGER.as_bytes()
    );
    drop(recovered);
    let guard = f.open();
    let identity = guard.identity().clone();
    let lock = fs::read_dir(&f.root)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().ends_with(".lock"))
        .unwrap()
        .path();
    fs::remove_file(&lock).unwrap();
    fs::write(lock, b"").unwrap();
    assert!(guard.read().is_err());
    drop(guard);
    assert!(f.open().verify_identity(&identity).is_err());
}

#[cfg(unix)]
#[test]
fn changed_root_is_refused_even_when_the_leaf_parent_and_lock_keep_their_identity() {
    let f = Fixture::new();
    let root = f.root.join("authorized");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/ledger.yaml"), LEDGER).unwrap();
    let guard = LockedSourceFile::open(&root, "nested/ledger.yaml").unwrap();
    let identity = guard.identity().clone();
    fs::rename(&root, f.root.join("old-root")).unwrap();
    fs::create_dir(&root).unwrap();
    // Preserve the actual leaf parent and stable lock, changing only the bound
    // root. Comparing the leaf fingerprint/parent alone would miss this change.
    fs::rename(f.root.join("old-root/nested"), root.join("nested")).unwrap();
    assert!(guard.read().is_err());
    drop(guard);
    let recovered = LockedSourceFile::open(&root, "nested/ledger.yaml").unwrap();
    assert!(recovered.verify_identity(&identity).is_err());
    assert_eq!(
        fs::read(root.join("nested/ledger.yaml")).unwrap(),
        LEDGER.as_bytes()
    );
}
