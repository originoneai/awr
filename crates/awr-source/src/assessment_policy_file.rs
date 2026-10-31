//! The project's assessment policy file (DEC-040) and its one writer.
//!
//! The file is `.awr/assessment-policy.toml`. It is a source: the file is the authority, nothing in the runtime state
//! rewrites it, and it changes only through [`apply_assessment_policy`], which compares the exact bytes it saw with
//! the bytes it replaces. What a misread file contained is never echoed: a fault is described by its place.
use crate::{LockedSourceFile, fingerprint, read_under_root};
use awr_core::{Error, POLICY_FORMAT, PolicyDocument, Result};
use std::path::Path;

pub const ASSESSMENT_POLICY_FILE: &str = ".awr/assessment-policy.toml";
pub const ASSESSMENT_POLICY_READ_CAP: u64 = 64 * 1024;

/// The file as it is on disk: its exact bytes' fingerprint and the document they hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentPolicyFile {
    pub fingerprint: String,
    pub document: PolicyDocument,
}

fn too_large() -> Error {
    Error::InvalidInput(format!(
        "{ASSESSMENT_POLICY_FILE} is larger than {ASSESSMENT_POLICY_READ_CAP} bytes"
    ))
}

fn place(text: &str, error: &toml::de::Error) -> String {
    error
        .span()
        .map(|span| {
            let before = &text[..span.start.min(text.len())];
            let line = before.matches('\n').count() + 1;
            let column = before
                .rsplit('\n')
                .next()
                .map_or(0, |last| last.chars().count())
                + 1;
            format!(" at line {line}, column {column}")
        })
        .unwrap_or_default()
}

/// Parses the text of a policy file. A newer format is `Unsupported` (use a newer awr) and is told before any other
/// key is looked at; a key this format does not have, a value of the wrong kind and a malformed file are `InvalidInput`
/// described by place only.
pub fn parse_assessment_policy(text: &str) -> Result<PolicyDocument> {
    let table: toml::Table = text.parse().map_err(|error| {
        Error::InvalidInput(format!(
            "{ASSESSMENT_POLICY_FILE} is not valid TOML{}",
            place(text, &error)
        ))
    })?;
    match table.get("format").and_then(toml::Value::as_integer) {
        Some(format) if format == i64::from(POLICY_FORMAT) => {}
        Some(format) if format > i64::from(POLICY_FORMAT) => {
            return Err(Error::Unsupported(format!(
                "{ASSESSMENT_POLICY_FILE} has format {format}, and this awr reads format {POLICY_FORMAT}; use a newer awr"
            )));
        }
        _ => {
            return Err(Error::InvalidInput(format!(
                "{ASSESSMENT_POLICY_FILE} needs `format = {POLICY_FORMAT}`"
            )));
        }
    }
    toml::from_str::<PolicyDocument>(text).map_err(|error| {
        Error::InvalidInput(format!(
            "{ASSESSMENT_POLICY_FILE} is not a valid policy{}",
            place(text, &error)
        ))
    })
}

fn parse_bytes(bytes: &[u8]) -> Result<PolicyDocument> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::InvalidInput(format!("{ASSESSMENT_POLICY_FILE} is not UTF-8 text")))?;
    parse_assessment_policy(text)
}

/// Parses candidate bytes and reports their fingerprint, as a change would write them.
pub fn read_candidate(bytes: &[u8]) -> Result<AssessmentPolicyFile> {
    if bytes.len() as u64 > ASSESSMENT_POLICY_READ_CAP {
        return Err(too_large());
    }
    Ok(AssessmentPolicyFile {
        fingerprint: fingerprint(bytes),
        document: parse_bytes(bytes)?,
    })
}

/// The project's policy file, read through the root-confined handle: `None` when there is none. A link is refused, a
/// file over 64 KiB is refused, and a file this build cannot read is an error, never a default.
pub fn read_assessment_policy(root: &Path) -> Result<Option<AssessmentPolicyFile>> {
    match std::fs::symlink_metadata(root.join(ASSESSMENT_POLICY_FILE)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::Io(error)),
        Ok(metadata) if metadata.len() > ASSESSMENT_POLICY_READ_CAP => return Err(too_large()),
        Ok(_) => (),
    }
    let bytes = read_under_root(root, ASSESSMENT_POLICY_FILE)?;
    read_candidate(&bytes).map(Some)
}

/// The writer lock of the file. Several writers that create the lock file for the first time at the same moment can
/// see the file vanish under them for an instant, and the loser is told `NotFound`; the file exists by then, so a
/// few more attempts settle it. Any other fault, and a lock that is busy, is returned as it is.
fn open_lock(root: &Path) -> Result<LockedSourceFile> {
    let mut attempts = 0;
    loop {
        match LockedSourceFile::open(root, ASSESSMENT_POLICY_FILE) {
            Err(Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound && attempts < 5 =>
            {
                attempts += 1;
                std::thread::yield_now();
            }
            other => return other,
        }
    }
}

/// Writes `candidate` as the policy file if, and only if, the file still holds the bytes whose fingerprint the caller
/// saw (`None`: there was no file). The check and the write happen under the cooperative writer lock of the file; a
/// file that changed meanwhile is a `SourceConflict` and nothing is written.
pub fn apply_assessment_policy(
    root: &Path,
    expected_current: Option<&str>,
    candidate: &[u8],
) -> Result<()> {
    let root = std::fs::canonicalize(root).map_err(Error::Io)?;
    let candidate_file = read_candidate(candidate)?;
    let lock = open_lock(&root)?;
    let current = match lock.read() {
        Ok(bytes) => Some(bytes),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    match (current, expected_current) {
        (Some(bytes), Some(expected)) if fingerprint(&bytes) == expected => {
            if fingerprint(&bytes) == candidate_file.fingerprint {
                return Ok(());
            }
            lock.replace(expected, candidate)
        }
        (None, None) => lock.create_new(candidate),
        _ => Err(Error::SourceConflict(format!(
            "{ASSESSMENT_POLICY_FILE} changed since it was read"
        ))),
    }
}
