//! The optional project policy file and the claim defaults it declares.
//!
//! The file is `.awr/policy.toml`, apart from `.awr/project.toml` on purpose: that manifest refuses keys it
//! does not know, so a published `awr` would stop opening a project that grew a `[policy]` table, while it
//! never reads a file it has no name for.
use crate::read_under_root;
use awr_core::{Error, Result};
use serde::Deserialize;
use std::path::Path;

pub const POLICY_FILE: &str = ".awr/policy.toml";
/// The one layout of the file this build reads; a newer layout is refused, never guessed at.
pub const POLICY_VERSION: u64 = 1;
/// Environment override of `claims.default_ttl`; it alone also declares a policy.
pub const DEFAULT_CLAIM_TTL_ENV: &str = "AWR_DEFAULT_CLAIM_TTL";
pub const MIN_CLAIM_TTL_MS: u64 = 60_000;
pub const MAX_CLAIM_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const POLICY_READ_CAP: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    version: u64,
    #[serde(default)]
    claims: ClaimsTable,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimsTable {
    default_ttl: Option<String>,
    renew_on_activity: Option<bool>,
}

/// A duration setting: `none` turns the default off, whatever a lower layer says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultTtl {
    Off,
    Millis(u64),
}

/// Parses a whole number with a unit of `s`, `m`, `h` or `d` (`90s`, `30m`, `4h`, `2d`), or `none`.
/// The bounds are one minute to seven days; the text is never echoed.
pub fn parse_default_ttl(text: &str) -> Result<DefaultTtl> {
    let text = text.trim().to_ascii_lowercase();
    if text == "none" {
        return Ok(DefaultTtl::Off);
    }
    let invalid = || {
        Error::InvalidInput(
            "a claim TTL is a whole number with a unit of s, m, h or d (for example 4h) between 1m and 7d, or none"
                .into(),
        )
    };
    let Some((at, unit)) = text.char_indices().next_back() else {
        return Err(invalid());
    };
    let unit_ms: u64 = match unit {
        's' => 1_000,
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        _ => return Err(invalid()),
    };
    let digits = &text[..at];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let millis = digits
        .parse::<u64>()
        .ok()
        .and_then(|count| count.checked_mul(unit_ms))
        .filter(|millis| (MIN_CLAIM_TTL_MS..=MAX_CLAIM_TTL_MS).contains(millis))
        .ok_or_else(invalid)?;
    Ok(DefaultTtl::Millis(millis))
}

/// What the project declares about claims: a default lifetime for a claim whose request gave none, and whether
/// recorded work extends the lifetime of the claims a session holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimPolicy {
    /// True when the policy file exists or the variable is set; a project that declares nothing keeps the
    /// behaviour it had.
    pub declared: bool,
    default_ttl_ms: Option<u64>,
    renew_on_activity: bool,
}

impl ClaimPolicy {
    /// The policy of the project at `root` under this process's environment.
    pub fn load(root: &Path) -> Result<Self> {
        let variable = std::env::var_os(DEFAULT_CLAIM_TTL_ENV)
            .map(|raw| raw.to_str().map(str::to_owned).unwrap_or_default());
        Self::load_with(root, variable.as_deref())
    }

    /// As `load`, with the value of `AWR_DEFAULT_CLAIM_TTL` given instead of read.
    pub fn load_with(root: &Path, variable: Option<&str>) -> Result<Self> {
        let file = read_policy_file(root)?;
        let mut policy = Self {
            declared: file.is_some() || variable.is_some(),
            default_ttl_ms: None,
            // A default that is not renewed drops the claim of a task that outlasts it.
            renew_on_activity: true,
        };
        if let Some(file) = file {
            if let Some(text) = &file.claims.default_ttl {
                let ttl = parse_default_ttl(text).map_err(|error| {
                    Error::InvalidInput(format!("{POLICY_FILE}: claims.default_ttl: {error}"))
                })?;
                policy.default_ttl_ms = millis(ttl);
            }
            policy.renew_on_activity = file.claims.renew_on_activity.unwrap_or(true);
        }
        if let Some(text) = variable {
            let ttl = parse_default_ttl(text).map_err(|error| {
                Error::InvalidInput(format!("{DEFAULT_CLAIM_TTL_ENV}: {error}"))
            })?;
            policy.default_ttl_ms = millis(ttl);
        }
        Ok(policy)
    }

    /// The lifetime of a claim whose request gave none; `None` leaves such a claim without an expiry.
    pub fn default_ttl_ms(&self) -> Option<u64> {
        self.default_ttl_ms
    }

    /// The time from now to which recorded work extends a live claim; `None` when nothing is renewed.
    pub fn renewal_ttl_ms(&self) -> Option<u64> {
        self.default_ttl_ms.filter(|_| self.renew_on_activity)
    }
}

fn millis(ttl: DefaultTtl) -> Option<u64> {
    match ttl {
        DefaultTtl::Off => None,
        DefaultTtl::Millis(millis) => Some(millis),
    }
}

fn read_policy_file(root: &Path) -> Result<Option<PolicyFile>> {
    match std::fs::symlink_metadata(root.join(POLICY_FILE)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::Io(error)),
        Ok(metadata) if metadata.len() > POLICY_READ_CAP => return Err(too_large()),
        Ok(_) => (),
    }
    let bytes = read_under_root(root, POLICY_FILE)?;
    if bytes.len() as u64 > POLICY_READ_CAP {
        return Err(too_large());
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Error::InvalidInput(format!("{POLICY_FILE} is not UTF-8 text")))?;
    let file: PolicyFile = toml::from_str(text).map_err(|error| read_error(text, &error))?;
    if file.version != POLICY_VERSION {
        return Err(Error::Unsupported(format!(
            "{POLICY_FILE} is version {}, and this awr reads version {POLICY_VERSION}; use a newer awr",
            file.version
        )));
    }
    Ok(Some(file))
}

fn too_large() -> Error {
    Error::InvalidInput(format!(
        "{POLICY_FILE} is larger than {} bytes",
        POLICY_READ_CAP
    ))
}

/// A key this build does not know is a newer layout; any other fault is described by place, never by content.
fn read_error(text: &str, error: &toml::de::Error) -> Error {
    let message = error.message();
    if message.starts_with("unknown field") {
        return Error::Unsupported(format!(
            "{POLICY_FILE} has a key this awr does not know ({message}); a newer awr may be needed to read it"
        ));
    }
    if message.starts_with("missing field") {
        return Error::InvalidInput(format!("{POLICY_FILE} is incomplete ({message})"));
    }
    let place = error
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
        .unwrap_or_default();
    Error::InvalidInput(format!("{POLICY_FILE} is not a valid policy{place}"))
}
