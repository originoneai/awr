use crate::{Manifest, SourceSpec};
use awr_core::{Error, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
};
use url::Url;

#[derive(Debug, Clone)]
pub enum Locator {
    /// Already-authorized canonical path. Use `from_spec` for configured sources;
    /// reads deliberately refuse links substituted after that authority check.
    File(PathBuf),
    Git {
        revision: String,
        path: PathBuf,
    },
    /// Project-root-relative path; identity is `project:///<relative-path>`.
    Project { path: PathBuf },
}

#[derive(Debug, Clone)]
pub struct SourceSnapshot {
    pub locator: String,
    pub fingerprint: String,
    pub bytes: Vec<u8>,
    /// Byte-bound source proof; every use rechecks the retained snapshot.
    pub content_review: Option<awr_core::VerifiedSourceContentReview>,
}
impl SourceSnapshot {
    /// Validate retained source bytes without reading the original file or Git repository.
    /// Git fingerprints bind the resolved locator as well as the blob content.
    pub fn verify_fingerprint(&self) -> bool {
        let expected = if let Some(git) = self.locator.strip_prefix("git://") {
            let Some((commit, path)) = git.split_once(':') else {
                return false;
            };
            if ![40, 64].contains(&commit.len())
                || !commit.bytes().all(|b| b.is_ascii_hexdigit())
                || relative(Path::new(path)).is_err()
            {
                return false;
            }
            let mut hash = Sha256::new();
            hash.update(self.locator.as_bytes());
            hash.update([0]);
            hash.update(&self.bytes);
            format!("sha256:{:x}", hash.finalize())
        } else if self.locator.starts_with("file://") || self.locator.starts_with("project://") {
            fingerprint(&self.bytes)
        } else {
            return false;
        };
        self.fingerprint == expected
    }

    pub fn text(&self) -> Result<&str> {
        self.validate_content()?;
        std::str::from_utf8(&self.bytes)
            .map_err(|e| Error::InvalidInput(format!("source must be UTF-8: {e}")))
    }

    pub fn validate_content(&self) -> Result<()> {
        match &self.content_review {
            Some(review) => {
                if !self.verify_fingerprint() {
                    return Err(Error::SourceConflict(
                        "reviewed snapshot fingerprint is inconsistent".into(),
                    ));
                }
                review.verify_bytes(&self.bytes, &self.locator)
            }
            None => awr_core::ensure_public_source(&self.bytes, &self.locator),
        }
    }
    pub fn ensure_value(&self, value: &serde_json::Value) -> Result<()> {
        self.validate_content()?;
        match &self.content_review {
            Some(review) => review.ensure_value(value),
            None => awr_core::ensure_public_value(value),
        }
    }

    /// One-based inclusive line range; preserve the original newline bytes.
    pub fn section_text(&self, start: usize, end: usize) -> Result<&str> {
        let text = self.text()?;
        let lines: Vec<_> = text.split_inclusive('\n').collect();
        if start == 0 || end < start || end > lines.len() {
            return Err(Error::InvalidInput(
                "section line range is outside the source".into(),
            ));
        }
        let offset: usize = lines[..start - 1].iter().map(|s| s.len()).sum();
        let size: usize = lines[start - 1..end].iter().map(|s| s.len()).sum();
        Ok(&text[offset..offset + size])
    }

    pub fn section_fingerprint(&self, start: usize, end: usize) -> Result<String> {
        Ok(fingerprint(self.section_text(start, end)?.as_bytes()))
    }
}

pub fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let file = File::open(path)
        .map_err(|e| Error::SourceUnavailable(format!("{}: {e}", path.display())))?;
    let bytes = read_file_capped(file, path, cap)?;
    awr_core::ensure_public_source(&bytes, &path.to_string_lossy())?;
    Ok(bytes)
}

/// Read a canonical source path whose authority was checked by Locator/Manifest.
/// Unlike caller-supplied input files, it must not be redirected by a replacement link.
pub fn read_source_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let bytes = read_source_for_review(path, cap)?;
    awr_core::ensure_public_source(&bytes, &path.to_string_lossy())?;
    Ok(bytes)
}

/// Bounded diagnostic read. Callers must not archive its bytes before review.
pub fn read_source_for_review(path: &Path, cap: u64) -> Result<Vec<u8>> {
    read_file_capped(crate::open_file_exact(path)?, path, cap)
}

fn read_file_capped(file: File, _path: &Path, cap: u64) -> Result<Vec<u8>> {
    if cap == 0 {
        return Err(Error::InvalidInput("read cap must be positive".into()));
    }
    let read_limit = cap
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("read cap is too large".into()))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Error::InvalidInput(
            "source reader needs a regular file".into(),
        ));
    }
    if metadata.len() > cap {
        return Err(Error::InvalidInput(format!(
            "source exceeds {cap} byte read cap"
        )));
    }
    let mut bytes = Vec::new();
    file.take(read_limit).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Err(Error::InvalidInput(format!(
            "source exceeds {cap} byte read cap"
        )));
    }
    Ok(bytes)
}
pub fn fingerprint(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(Error::RuleViolation(
            "Git source path must stay inside the project".into(),
        ));
    }
    Ok(())
}

/// Strip a `#L<start>[-L<end>]` selector; any other `#` stays part of the path.
fn strip_line_suffix(value: &str) -> Result<&str> {
    let Some(index) = value.find('#') else {
        return Ok(value);
    };
    let Some(range) = value[index + 1..].strip_prefix('L') else {
        return Ok(value);
    };
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let mut parts = range.split('-');
    let start = parts.next().unwrap_or_default();
    let end = parts.next();
    if !numeric(start) || parts.next().is_some() {
        return Err(Error::InvalidInput(
            "line suffix must be #L<start> or #L<start>-L<end>".into(),
        ));
    }
    if let Some(end) = end {
        let Some(end) = end.strip_prefix('L') else {
            return Err(Error::InvalidInput(
                "line suffix must be #L<start> or #L<start>-L<end>".into(),
            ));
        };
        if !numeric(end) {
            return Err(Error::InvalidInput(
                "line suffix must be #L<start> or #L<start>-L<end>".into(),
            ));
        }
    }
    Ok(&value[..index])
}

impl Locator {
    /// Registration identity keeps a Git ref stable while snapshot locators bind immutable commits.
    pub fn identity(&self) -> Result<String> {
        match self {
            Self::File(path) => Url::from_file_path(path)
                .map(|url| url.to_string())
                .map_err(|_| Error::InvalidInput("invalid file locator".into())),
            Self::Git { revision, path } => {
                relative(path)?;
                let path =
                    path.components()
                        .filter_map(|part| match part {
                            Component::Normal(value) => Some(value.to_str().ok_or_else(|| {
                                Error::InvalidInput("Git path must be UTF-8".into())
                            })),
                            _ => None,
                        })
                        .collect::<Result<Vec<_>>>()?
                        .join("/");
                Ok(format!("git://{revision}:{path}"))
            }
            Self::Project { path } => {
                relative(path)?;
                let path = path
                    .components()
                    .filter_map(|part| match part {
                        Component::Normal(value) => Some(value.to_str().ok_or_else(|| {
                            Error::InvalidInput("project path must be UTF-8".into())
                        })),
                        _ => None,
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join("/");
                Ok(format!("project:///{path}"))
            }
        }
    }
    pub fn from_spec(root: &Path, manifest: &Manifest, spec: &SourceSpec) -> Result<Self> {
        if let Some(path) = &spec.path {
            return Self::file(root, manifest, path);
        }
        let value = spec
            .locator
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("missing locator".into()))?;
        Self::parse(root, manifest, value)
    }
    /// Parse a locator string, stripping a `#L<start>[-L<end>]` line selector first.
    ///
    /// The suffix is a presentation selector: it never reaches `identity()` or a
    /// source fingerprint. Every caller that resolves a locator string goes through
    /// this entry point, so there is exactly one strip.
    pub fn parse(root: &Path, manifest: &Manifest, value: &str) -> Result<Self> {
        let value = strip_line_suffix(value)?;
        if value.starts_with("file://") {
            let url = Url::parse(value).map_err(|e| Error::InvalidInput(e.to_string()))?;
            if url.query().is_some() || url.fragment().is_some() {
                return Err(Error::InvalidInput(
                    "file locator must not contain query or fragment".into(),
                ));
            }
            let path = url.to_file_path().map_err(|_| {
                Error::InvalidInput("file locator must identify a local absolute path".into())
            })?;
            return Self::file(root, manifest, &path);
        }
        if let Some(value) = value.strip_prefix("git://") {
            let (revision, path) = value.split_once(':').ok_or_else(|| {
                Error::InvalidInput(
                    "Git locator syntax is git://<ref>:<project-relative-path>".into(),
                )
            })?;
            if revision.is_empty()
                || revision.starts_with('-')
                || revision.chars().any(char::is_whitespace)
            {
                return Err(Error::InvalidInput("invalid Git revision".into()));
            }
            let path = PathBuf::from(path);
            relative(&path)?;
            return Ok(Self::Git {
                revision: revision.into(),
                path,
            });
        }
        if let Some(value) = value.strip_prefix("project://") {
            let Some(path) = value.strip_prefix('/') else {
                return Err(Error::InvalidInput(
                    "project locator syntax is project:///<project-relative-path>".into(),
                ));
            };
            let path = PathBuf::from(path);
            relative(&path)?;
            // The same authority check as file:// sources: a project locator can
            // never point outside the project or its authorized roots.
            Self::file(root, manifest, &path)?;
            return Ok(Self::Project { path });
        }
        Err(Error::Unsupported(
            "V1 supports file://, git:// and project:// sources".into(),
        ))
    }
    fn file(root: &Path, manifest: &Manifest, path: &Path) -> Result<Self> {
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(Error::RuleViolation(
                "source path contains parent traversal".into(),
            ));
        }
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            root.join(path)
        }
        .canonicalize()
        .map_err(|e| Error::SourceUnavailable(e.to_string()))?;
        if !manifest
            .authorized_roots(root)?
            .iter()
            .any(|allowed| path.starts_with(allowed))
        {
            return Err(Error::RuleViolation(
                "source is outside authorized roots".into(),
            ));
        }
        Ok(Self::File(path))
    }
    pub fn read(&self, root: &Path, cap: u64) -> Result<SourceSnapshot> {
        match self {
            Self::File(path) => {
                let bytes = read_source_for_review(path, cap)?;
                let locator = Url::from_file_path(path)
                    .map_err(|_| Error::InvalidInput("invalid file locator".into()))?
                    .to_string();
                let content_review = crate::content_review::load_review(root, &locator, &bytes)?;
                let snapshot = SourceSnapshot {
                    locator,
                    fingerprint: fingerprint(&bytes),
                    bytes,
                    content_review,
                };
                snapshot.validate_content()?;
                Ok(snapshot)
            }
            Self::Project { path } => {
                let root = root.canonicalize()?;
                let resolved = root
                    .join(path)
                    .canonicalize()
                    .map_err(|e| Error::SourceUnavailable(e.to_string()))?;
                if !resolved.starts_with(&root) {
                    return Err(Error::RuleViolation(
                        "project source escapes the project root".into(),
                    ));
                }
                let bytes = read_source_for_review(&resolved, cap)?;
                let locator = self.identity()?;
                let content_review = crate::content_review::load_review(&root, &locator, &bytes)?;
                let snapshot = SourceSnapshot {
                    locator,
                    fingerprint: fingerprint(&bytes),
                    bytes,
                    content_review,
                };
                snapshot.validate_content()?;
                Ok(snapshot)
            }
            Self::Git { revision, path } => {
                let resolved = git(
                    root,
                    &[
                        "rev-parse",
                        "--verify",
                        "--end-of-options",
                        &format!("{revision}^{{commit}}"),
                    ],
                )?;
                let commit = std::str::from_utf8(&resolved)
                    .map_err(|e| Error::InvalidInput(e.to_string()))?
                    .trim()
                    .to_owned();
                if ![40, 64].contains(&commit.len())
                    || !commit.chars().all(|c| c.is_ascii_hexdigit())
                {
                    return Err(Error::SourceUnavailable(
                        "Git did not return an immutable commit".into(),
                    ));
                }
                let prefix = git(root, &["rev-parse", "--show-prefix"])?;
                let prefix = std::str::from_utf8(&prefix)
                    .map_err(|e| Error::InvalidInput(e.to_string()))?
                    .trim_end_matches('\n');
                let relative_path = path
                    .components()
                    .filter_map(|part| match part {
                        Component::Normal(value) => Some(value.to_str().ok_or_else(|| {
                            Error::InvalidInput("Git source path must be UTF-8".into())
                        })),
                        Component::CurDir => None,
                        _ => Some(Err(Error::RuleViolation(
                            "Git path escapes project root".into(),
                        ))),
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join("/");
                let object = format!("{commit}:{prefix}{relative_path}");
                let size = git(root, &["cat-file", "-s", &object])?;
                let size = std::str::from_utf8(&size)
                    .map_err(|e| Error::InvalidInput(e.to_string()))?
                    .trim()
                    .parse::<u64>()
                    .map_err(|e| Error::SourceUnavailable(e.to_string()))?;
                if size > cap {
                    return Err(Error::InvalidInput(format!(
                        "Git source exceeds {cap} byte read cap"
                    )));
                }
                let bytes = git(root, &["cat-file", "blob", &object])?;
                if bytes.len() as u64 > cap {
                    return Err(Error::InvalidInput("Git blob exceeds read cap".into()));
                }
                let locator = format!("git://{commit}:{relative_path}");
                awr_core::ensure_public_source(&bytes, &locator)?;
                let mut digest = Sha256::new();
                digest.update(locator.as_bytes());
                digest.update([0]);
                digest.update(&bytes);
                Ok(SourceSnapshot {
                    locator,
                    fingerprint: format!("sha256:{:x}", digest.finalize()),
                    content_review: None,
                    bytes,
                })
            }
        }
    }
}
pub(crate) fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| Error::SourceUnavailable(format!("Git: {e}")))?;
    if !output.status.success() {
        return Err(Error::SourceUnavailable(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    Ok(output.stdout)
}
