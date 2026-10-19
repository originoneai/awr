//! Project-relative evidence locators.

use awr_source::{Error, Locator, Manifest, fingerprint};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn project_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "awr-project-locator-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("evidence")).unwrap();
    fs::write(dir.join("evidence/check.json"), b"{\"check\":true}\n").unwrap();
    dir.canonicalize().unwrap()
}

fn manifest() -> Manifest {
    Manifest::parse(
        "[project]\nname = 'P'\n\n[[sources]]\ndomain = 'decisions'\nrole = 'supporting'\npath = 'decisions'\nadapter = 'markdown-directory-v1'\n",
    )
    .unwrap()
}

fn file_identity(root: &Path) -> String {
    let path = root.join("evidence/check.json");
    let text = path.to_string_lossy().replace('\\', "/");
    // canonicalize() adds the Windows verbatim prefix, which is not URL syntax.
    let text = text.strip_prefix("//?/").unwrap_or(&text).to_owned();
    format!("file:///{text}")
}

#[test]
fn file_and_git_locators_keep_their_pre_project_behaviour() {
    let root = project_root("parity");
    let manifest = manifest();
    let identity = file_identity(&root);

    // A file locator never accepts a fragment, so every line selector is refused.
    for suffix in ["#L1", "#L12-L14", "#section-2"] {
        let error = Locator::parse(&root, &manifest, &format!("{identity}{suffix}")).unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)), "{suffix}: {error}");
    }

    // A Git locator never had a fragment guard: `#L1` stays ordinary path bytes,
    // and only resolution fails. This is the pre-project behaviour, unchanged.
    let commit = "c".repeat(40);
    let locator = Locator::parse(
        &root,
        &manifest,
        &format!("git://{commit}:evidence/check.json#L1"),
    )
    .unwrap();
    match &locator {
        Locator::Git { revision, path } => {
            assert_eq!(revision, &commit);
            assert_eq!(path.as_path(), Path::new("evidence/check.json#L1"));
        }
        other => panic!("expected a Git locator, got {other:?}"),
    }
    assert_eq!(
        locator.identity().unwrap(),
        format!("git://{commit}:evidence/check.json#L1")
    );
    let error = locator.read(&root, 4096).unwrap_err();
    assert!(matches!(error, Error::SourceUnavailable(_)), "{error}");
}

#[test]
fn project_locator_keeps_identity_and_fingerprint_across_checkouts() {
    let first = project_root("first");
    let second = project_root("second");
    let manifest = manifest();
    let spec = "project:///evidence/check.json";

    let locator = Locator::parse(&first, &manifest, spec).unwrap();
    assert_eq!(locator.identity().unwrap(), spec);
    let snapshot = locator.read(&first, 4096).unwrap();
    assert_eq!(snapshot.locator, spec);
    assert_eq!(snapshot.fingerprint, fingerprint(&snapshot.bytes));
    assert!(snapshot.verify_fingerprint());

    // The same stored string resolves against a different checkout and verifies.
    let moved = Locator::parse(&second, &manifest, spec).unwrap();
    let moved_snapshot = moved.read(&second, 4096).unwrap();
    assert_eq!(moved_snapshot.locator, snapshot.locator);
    assert_eq!(moved_snapshot.fingerprint, snapshot.fingerprint);
    assert!(moved_snapshot.verify_fingerprint());
}

#[test]
fn project_locator_rejects_an_authority_traversal_and_a_selector() {
    let root = project_root("reject");
    let manifest = manifest();
    for spec in ["project://notes/a.md", "project:///../a.md", "project://"] {
        let error = Locator::parse(&root, &manifest, spec).unwrap_err();
        assert!(
            matches!(error, Error::InvalidInput(_) | Error::RuleViolation(_)),
            "{spec}: {error}"
        );
    }
    // A line selector is deferred: `#L1` stays path bytes, so resolution fails
    // with a typed error instead of being read as a selector.
    let error = Locator::parse(&root, &manifest, "project:///evidence/check.json#L1").unwrap_err();
    assert!(matches!(error, Error::SourceUnavailable(_)), "{error}");
}

#[test]
fn the_same_relative_path_with_different_bytes_never_falls_back() {
    let first = project_root("two-roots-first");
    let second = project_root("two-roots-second");
    fs::write(second.join("evidence/check.json"), b"{\"check\":false}\n").unwrap();
    let manifest = manifest();
    let spec = "project:///evidence/check.json";

    let a = Locator::parse(&first, &manifest, spec)
        .unwrap()
        .read(&first, 4096)
        .unwrap();
    let b = Locator::parse(&second, &manifest, spec)
        .unwrap()
        .read(&second, 4096)
        .unwrap();

    // One identity, two byte streams: each read uses its own root, and neither
    // falls back to the other checkout.
    assert_eq!(a.locator, b.locator);
    assert_eq!(a.bytes, b"{\"check\":true}\n");
    assert_eq!(b.bytes, b"{\"check\":false}\n");
    assert_ne!(a.fingerprint, b.fingerprint);
}

#[test]
fn a_project_locator_stays_inside_the_project_root() {
    let root = project_root("root-rule");
    let manifest = manifest();
    for spec in [
        "project:///../outside.json",
        "project:///evidence/../../outside.json",
    ] {
        let error = Locator::parse(&root, &manifest, spec).unwrap_err();
        assert!(matches!(error, Error::RuleViolation(_)), "{spec}: {error}");
    }
}
