use std::path::{Path, PathBuf};
use std::process::Command;

fn short_sha(value: &str) -> Option<String> {
    (value.len() >= 8 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| value[..8].to_ascii_lowercase())
}

pub(crate) fn format_version(version: &str, sha: Option<&str>, dirty: bool) -> String {
    match sha.and_then(short_sha) {
        Some(sha) => format!("{version} ({sha}{})", if dirty { "-dirty" } else { "" }),
        None => version.to_owned(),
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())?
}

fn watch(lines: &mut Vec<String>, path: PathBuf) {
    if path.exists()
        && let Some(path) = path.to_str()
        && !path.contains(['\n', '\r'])
    {
        lines.push(format!("cargo:rerun-if-changed={path}"));
    }
}

pub(crate) fn build_directives(
    manifest: &Path,
    version: &str,
    injected_sha: Option<&str>,
) -> Vec<String> {
    let mut lines = vec!["cargo:rerun-if-env-changed=AWR_BUILD_SHA".to_owned()];
    watch(&mut lines, manifest.join("build.rs"));
    watch(&mut lines, manifest.join("../build_identity.rs"));
    let root = manifest.join("../..");
    let mut sha = injected_sha.and_then(short_sha);
    let mut dirty = false;
    // A source archive inside another repository must not inherit that repository's identity.
    if root.join(".git").exists() {
        for name in ["HEAD", "index", "packed-refs"]
            .into_iter()
            .map(str::to_owned)
            .chain(git(&root, &["symbolic-ref", "-q", "HEAD"]).map(|s| s.trim().to_owned()))
        {
            if let Some(path) = git(
                &root,
                &["rev-parse", "--path-format=absolute", "--git-path", &name],
            ) {
                watch(&mut lines, PathBuf::from(path.trim()));
            }
        }
        // Watch source files rather than a directory containing ignored build/cache outputs.
        if let Some(paths) = git(
            &root,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
        ) {
            for path in paths.split('\0').filter(|s| !s.is_empty()) {
                watch(&mut lines, root.join(path));
            }
        }
        if sha.is_none() {
            sha = git(&root, &["rev-parse", "--short=8", "HEAD"]).and_then(|s| short_sha(s.trim()));
        }
        dirty = git(&root, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    }
    lines.push(format!(
        "cargo:rustc-env=AWR_BUILD_VERSION={}",
        format_version(version, sha.as_deref(), dirty)
    ));
    lines
}

pub(crate) fn emit() {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default();
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let sha = std::env::var("AWR_BUILD_SHA").ok();
    for line in build_directives(Path::new(&manifest), &version, sha.as_deref()) {
        println!("{line}");
    }
}
