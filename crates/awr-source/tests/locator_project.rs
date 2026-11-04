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
    manifest_with_key(None)
}

fn manifest_with_key(key: Option<&str>) -> Manifest {
    let key_line = key
        .map(|k| format!("external_key = '{k}'\n"))
        .unwrap_or_default();
    Manifest::parse(&format!(
        "[project]\nname = 'P'\n{key_line}\n[[sources]]\ndomain = 'decisions'\nrole = 'supporting'\npath = 'decisions'\nadapter = 'markdown-directory-v1'\n",
    ))
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
    // Without an authored key, the host form is identity-bearing and fails closed.
    let error = Locator::parse(&root, &manifest, "project://notes/a.md").unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("project locator identity is missing"),
        "{error}"
    );
    for spec in ["project:///../a.md", "project://"] {
        let error = Locator::parse(&root, &manifest, spec).unwrap_err();
        assert!(
            matches!(error, Error::InvalidInput(_) | Error::RuleViolation(_)),
            "{spec}: {error}"
        );
    }
    // A line selector is refused here, like `file://`: a project locator names a
    // file, so `#L1` is neither a fragment to strip nor a literal file name to
    // keep — both readings would leave one suffix with two meanings.
    let error = Locator::parse(&root, &manifest, "project:///evidence/check.json#L1").unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("project locator does not support a line selector"),
        "{error}"
    );
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

#[test]
fn identity_bearing_project_locator_round_trips_when_keys_match() {
    let root = project_root("keyed");
    let manifest = manifest_with_key(Some("alpha"));
    let spec = "project://alpha/evidence/check.json";

    let locator = Locator::parse(&root, &manifest, spec).unwrap();
    match &locator {
        Locator::Project { path, project_key } => {
            assert_eq!(path.as_path(), Path::new("evidence/check.json"));
            assert_eq!(project_key.as_deref(), Some("alpha"));
        }
        other => panic!("expected a project locator, got {other:?}"),
    }
    assert_eq!(locator.identity().unwrap(), spec);
    let snapshot = locator.read(&root, 4096).unwrap();
    assert_eq!(snapshot.locator, spec);
    assert_eq!(snapshot.fingerprint, fingerprint(&snapshot.bytes));
    assert!(snapshot.verify_fingerprint());
}

#[test]
fn identity_bearing_project_locator_mismatch_does_not_read_bytes() {
    let root = project_root("mismatch");
    let manifest = manifest_with_key(Some("alpha"));
    // Missing file would be SourceUnavailable if parse opened it.
    let error =
        Locator::parse(&root, &manifest, "project://beta/evidence/missing.json").unwrap_err();
    assert!(matches!(error, Error::SourceConflict(_)), "{error}");
    let text = error.to_string();
    assert!(text.contains("locator_key beta"), "{error}");
    assert!(text.contains("current_key alpha"), "{error}");
}

#[test]
fn identity_bearing_project_locator_is_refused_when_the_authored_key_is_unset() {
    let root = project_root("unset");
    let error =
        Locator::parse(&root, &manifest(), "project://alpha/evidence/check.json").unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("project locator identity is missing: [project] external_key is unset"),
        "{error}"
    );
}

#[test]
fn legacy_project_locator_is_not_rewritten_on_read_when_a_key_is_set() {
    let root = project_root("legacy-read");
    let manifest = manifest_with_key(Some("alpha"));
    let spec = "project:///evidence/check.json";
    let locator = Locator::parse(&root, &manifest, spec).unwrap();
    match &locator {
        Locator::Project { project_key, .. } => assert_eq!(project_key, &None),
        other => panic!("expected a project locator, got {other:?}"),
    }
    let snapshot = locator.read(&root, 4096).unwrap();
    assert_eq!(snapshot.locator, spec);
}

#[test]
fn bind_authored_project_key_attaches_the_key_only_at_registration() {
    let root = project_root("bind");
    let manifest = manifest_with_key(Some("alpha"));
    let locator = Locator::parse(&root, &manifest, "project:///evidence/check.json")
        .unwrap()
        .bind_authored_project_key(&manifest)
        .unwrap();
    assert_eq!(
        locator.identity().unwrap(),
        "project://alpha/evidence/check.json"
    );
    let snapshot = locator.read(&root, 4096).unwrap();
    assert_eq!(snapshot.locator, "project://alpha/evidence/check.json");
}

#[test]
fn project_locator_key_charset_is_closed() {
    let root = project_root("charset");
    let manifest = manifest_with_key(Some("alpha"));
    for spec in [
        "project://alpha:beta/evidence/check.json",
        "project://alpha@beta/evidence/check.json",
        "project:// /evidence/check.json",
        &format!("project://{}/evidence/check.json", "a".repeat(201)),
    ] {
        let error = Locator::parse(&root, &manifest, spec).unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)), "{spec}: {error}");
    }
}

#[test]
fn bind_authored_project_key_refuses_a_key_outside_the_locator_charset() {
    let root = project_root("bind-charset");
    let manifest = manifest_with_key(Some("alpha@beta"));
    let error = Locator::parse(&root, &manifest, "project:///evidence/check.json")
        .unwrap()
        .bind_authored_project_key(&manifest)
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("project locator key must match [A-Za-z0-9._-]{1,200}"),
        "{error}"
    );
}
