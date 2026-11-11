#[allow(dead_code)]
#[path = "../../build_identity.rs"]
mod build_identity;

use build_identity::{build_directives, format_version};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "awr-build-identity-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("crates/awr-cli")).unwrap();
        std::fs::create_dir_all(root.join("crates/awr-mcp")).unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for path in [
            "crates/awr-cli/build.rs",
            "crates/awr-mcp/build.rs",
            "crates/build_identity.rs",
        ] {
            std::fs::copy(source.join(path), root.join(path)).unwrap();
        }
        Self(root)
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.0)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn init(&self) -> String {
        self.git(&["init", "-b", "build-identity"]);
        self.git(&["config", "user.name", "Build fixture"]);
        self.git(&["config", "user.email", "build@example.invalid"]);
        self.git(&["add", "."]);
        self.git(&["commit", "-m", "Create build fixture"]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn directives(&self, sha: Option<&str>) -> Vec<String> {
        build_directives(&self.0.join("crates/awr-cli"), "0.7.0", sha)
    }

    fn version(&self, sha: Option<&str>) -> String {
        self.directives(sha)
            .into_iter()
            .find_map(|s| {
                s.strip_prefix("cargo:rustc-env=AWR_BUILD_VERSION=")
                    .map(str::to_owned)
            })
            .expect("every build emits its package version")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn version_identity_ac1_formats_clean_and_dirty_short_commits() {
    assert_eq!(
        format_version("0.7.0", Some("fd0ea25c12345678"), false),
        "0.7.0 (fd0ea25c)"
    );
    assert_eq!(
        format_version("0.7.0", Some("fd0ea25c12345678"), true),
        "0.7.0 (fd0ea25c-dirty)"
    );
}

#[test]
fn version_identity_ac1_different_commits_cannot_share_a_version_line() {
    let f = Fixture::new();
    let first = f.init();
    let before = f.version(None);
    assert_eq!(before, format!("0.7.0 ({})", &first[..8]));
    std::fs::write(f.0.join("change.txt"), "another source revision\n").unwrap();
    f.git(&["add", "change.txt"]);
    f.git(&["commit", "-m", "Change build fixture"]);
    let second = f.git(&["rev-parse", "HEAD"]);
    let after = f.version(None);
    assert_eq!(after, format!("0.7.0 ({})", &second[..8]));
    assert_ne!(before, after);
}

#[test]
fn version_identity_ac1_detects_staged_unstaged_and_untracked_changes() {
    let f = Fixture::new();
    let sha = f.init();
    let dirty = format!("0.7.0 ({}-dirty)", &sha[..8]);
    let file = f.0.join("crates/build_identity.rs");
    std::fs::write(&file, "// modified source\n").unwrap();
    assert_eq!(f.version(None), dirty);
    f.git(&["add", "crates/build_identity.rs"]);
    assert_eq!(f.version(None), dirty);
    f.git(&[
        "restore",
        "--staged",
        "--worktree",
        "crates/build_identity.rs",
    ]);
    std::fs::write(f.0.join("new-source.txt"), "untracked source\n").unwrap();
    assert_eq!(f.version(None), dirty);
}

#[test]
fn version_identity_ac2_source_archive_uses_only_the_package_version() {
    let f = Fixture::new();
    assert_eq!(f.version(None), "0.7.0");
    assert_eq!(format_version("0.7.0", None, true), "0.7.0");
}

#[test]
fn version_identity_ac2_valid_override_precedes_git_and_is_shortened() {
    let f = Fixture::new();
    assert_eq!(f.version(Some("abcdef1234")), "0.7.0 (abcdef12)");
    f.init();
    assert_eq!(f.version(Some("ABCDEF1234")), "0.7.0 (abcdef12)");
    std::fs::write(f.0.join("new-source.txt"), "changed\n").unwrap();
    assert_eq!(f.version(Some("abcdef1234")), "0.7.0 (abcdef12-dirty)");
}

#[test]
fn version_identity_ac2_invalid_overrides_never_become_placeholders_or_text() {
    let f = Fixture::new();
    for invalid in [
        "",
        "unknown",
        "abc123",
        "abcdef12oops",
        "abcdef12\nwarning",
        "éabcdef1234",
    ] {
        assert_eq!(f.version(Some(invalid)), "0.7.0", "{invalid:?}");
        assert_eq!(
            format_version("0.7.0", Some(invalid), true),
            "0.7.0",
            "{invalid:?}"
        );
    }
    let sha = f.init();
    assert_eq!(
        f.version(Some("not-a-commit")),
        format!("0.7.0 ({})", &sha[..8])
    );
}

#[test]
fn version_identity_ac2_archive_does_not_inherit_an_enclosing_repository_commit() {
    let f = Fixture::new();
    f.init();
    let manifest = f.0.join("archive/crates/awr-cli");
    std::fs::create_dir_all(&manifest).unwrap();
    let lines = build_directives(&manifest, "0.7.0", None);
    assert!(
        lines
            .iter()
            .any(|s| s == "cargo:rustc-env=AWR_BUILD_VERSION=0.7.0")
    );
}

#[test]
fn version_identity_ac4_watches_existing_worktree_refs_index_and_source_files() {
    let f = Fixture::new();
    f.init();
    let worktree = f.0.join("linked");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "linked-build",
        worktree.to_str().unwrap(),
    ]);
    let manifest = worktree.join("crates/awr-cli");
    let lines = build_directives(&manifest, "0.7.0", None);
    assert!(
        lines
            .iter()
            .any(|s| s == "cargo:rerun-if-env-changed=AWR_BUILD_SHA")
    );
    let paths: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("cargo:rerun-if-changed="))
        .collect();
    for path in &paths {
        assert!(
            manifest.join(path).exists(),
            "missing rebuild input: {path}"
        );
    }
    for suffix in [
        "HEAD",
        "index",
        "refs/heads/linked-build",
        "crates/build_identity.rs",
    ] {
        assert!(
            paths.iter().any(|p| Path::new(p).ends_with(suffix)),
            "missing {suffix}: {paths:?}"
        );
    }
    let root = f.git(&["rev-parse", "HEAD"]);
    assert!(
        lines
            .iter()
            .any(|s| s == &format!("cargo:rustc-env=AWR_BUILD_VERSION=0.7.0 ({})", &root[..8]))
    );
    f.git(&["worktree", "remove", "linked"]);
}

#[test]
fn version_identity_ac4_packed_refs_and_detached_head_keep_the_correct_commit() {
    let f = Fixture::new();
    let sha = f.init();
    f.git(&["pack-refs", "--all", "--prune"]);
    assert_eq!(f.version(None), format!("0.7.0 ({})", &sha[..8]));
    assert!(
        f.directives(None)
            .iter()
            .any(|s| s.ends_with("packed-refs"))
    );
    f.git(&["checkout", "--detach"]);
    assert_eq!(f.version(None), format!("0.7.0 ({})", &sha[..8]));
}

#[test]
fn version_identity_ac4_archive_and_unborn_repo_never_watch_missing_git_paths() {
    let f = Fixture::new();
    for git in [false, true] {
        if git {
            f.git(&["init", "-b", "build-identity"]);
        }
        assert_eq!(f.version(None), "0.7.0");
        for line in f.directives(None) {
            if let Some(path) = line.strip_prefix("cargo:rerun-if-changed=") {
                assert!(f.0.join("crates/awr-cli").join(path).exists(), "{line}");
            }
        }
    }
}

#[test]
fn version_identity_ac4_both_build_scripts_run_without_git_or_an_override() {
    let f = Fixture::new();
    std::fs::create_dir(f.0.join(".git")).unwrap();
    let empty_path = f.0.join("empty-path");
    std::fs::create_dir(&empty_path).unwrap();
    for name in ["awr-cli", "awr-mcp"] {
        let manifest = f.0.join("crates").join(name);
        let binary =
            f.0.join(format!("build-{name}{}", std::env::consts::EXE_SUFFIX));
        let compiled = Command::new("rustc")
            .args(["--edition=2024", "--crate-name", "build_script"])
            .arg(manifest.join("build.rs"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
        let output = Command::new(&binary)
            .current_dir(&manifest)
            .env("CARGO_MANIFEST_DIR", &manifest)
            .env("CARGO_PKG_VERSION", "0.7.0")
            .env("PATH", &empty_path)
            .env_remove("AWR_BUILD_SHA")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .any(|s| s == "cargo:rustc-env=AWR_BUILD_VERSION=0.7.0")
        );
    }
}
