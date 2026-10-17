//! Project-relative evidence locators and `#L` line selectors.

use awr_source::{Error, Locator, Manifest, SourceSnapshot, fingerprint};
use sha2::{Digest, Sha256};
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

fn git_fingerprint(locator: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(locator.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    format!("sha256:{:x}", hash.finalize())
}

#[test]
fn line_suffix_is_stripped_from_identity_and_fingerprint() {
    let root = project_root("strip");
    let manifest = manifest();
    let bytes = b"report bytes\n";

    // git:// hashes the canonical locator, so both spellings must reach one binding.
    let commit = "a".repeat(40);
    let plain = format!("git://{commit}:evidence/check.json");
    let suffixed = format!("{plain}#L12-L14");
    let plain_locator = Locator::parse(&root, &manifest, &plain).unwrap();
    let suffixed_locator = Locator::parse(&root, &manifest, &suffixed).unwrap();
    let identity = plain_locator.identity().unwrap();
    assert_eq!(identity, plain);
    assert_eq!(suffixed_locator.identity().unwrap(), identity);
    let snapshot = SourceSnapshot {
        locator: identity.clone(),
        fingerprint: git_fingerprint(&identity, bytes),
        bytes: bytes.to_vec(),
        content_review: None,
    };
    assert!(snapshot.verify_fingerprint());

    // file:// hashes bytes only; the suffix must not change either identity.
    let plain_file = file_identity(&root);
    let suffixed_file = format!("{plain_file}#L12-L14");
    let plain_file_locator = Locator::parse(&root, &manifest, &plain_file).unwrap();
    let suffixed_file_locator = Locator::parse(&root, &manifest, &suffixed_file).unwrap();
    assert_eq!(plain_file_locator.identity().unwrap(), plain_file);
    assert_eq!(
        suffixed_file_locator.identity().unwrap(),
        plain_file_locator.identity().unwrap()
    );
    let file_snapshot = SourceSnapshot {
        locator: plain_file_locator.identity().unwrap(),
        fingerprint: fingerprint(bytes),
        bytes: bytes.to_vec(),
        content_review: None,
    };
    assert!(file_snapshot.verify_fingerprint());
}

#[test]
fn file_locator_accepts_line_suffix_but_not_other_fragments() {
    let root = project_root("fragment");
    let manifest = manifest();
    let identity = file_identity(&root);

    let accepted = Locator::parse(&root, &manifest, &format!("{identity}#L12-L14")).unwrap();
    assert_eq!(accepted.identity().unwrap(), identity);
    assert_eq!(
        accepted.identity().unwrap(),
        Locator::parse(&root, &manifest, &identity)
            .unwrap()
            .identity()
            .unwrap()
    );

    // A URL fragment that is not a line selector still fails closed.
    let error = Locator::parse(&root, &manifest, &format!("{identity}#section-2")).unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error}");

    // A malformed line selector is a typed error, not a filename byte.
    for suffix in ["#L", "#L12-", "#Lfoo", "#L12-14"] {
        let error = Locator::parse(&root, &manifest, &format!("{identity}{suffix}")).unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)), "{suffix}: {error}");
    }
}

#[test]
fn git_line_suffix_is_stripped_before_the_git_object_is_built() {
    let root = project_root("git");
    let manifest = manifest();
    let commit = "b".repeat(40);

    let locator = Locator::parse(
        &root,
        &manifest,
        &format!("git://{commit}:docs/report.md#L3"),
    )
    .unwrap();
    match locator {
        Locator::Git { revision, path } => {
            assert_eq!(revision, commit);
            assert_eq!(path, Path::new("docs/report.md"));
            assert!(!path.to_string_lossy().contains('#'));
        }
        other => panic!("expected a Git locator, got {other:?}"),
    }
    assert_eq!(
        Locator::parse(&root, &manifest, &format!("git://{commit}:docs/report.md"))
            .unwrap()
            .identity()
            .unwrap(),
        format!("git://{commit}:docs/report.md")
    );
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
fn project_locator_rejects_an_authority_and_traversal() {
    let root = project_root("reject");
    let manifest = manifest();
    for spec in ["project://notes/a.md", "project:///../a.md", "project://"] {
        let error = Locator::parse(&root, &manifest, spec).unwrap_err();
        assert!(
            matches!(error, Error::InvalidInput(_) | Error::RuleViolation(_)),
            "{spec}: {error}"
        );
    }
}
