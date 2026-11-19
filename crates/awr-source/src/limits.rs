use awr_core::{Error, Result};

pub const MARKDOWN_READ_CAP: u64 = 2 * 1024 * 1024;
pub const YAML_READ_CAP: u64 = 4 * 1024 * 1024;
/// Server-generated planning and delivery writeback may hold a whole project ledger.
pub const SERVER_LEDGER_READ_CAP: u64 = 32 * 1024 * 1024;
const LEDGER_LIMIT_DETAIL: &str = "awr_source_ledger_bytes";

/// Inclusive byte limits for the supported source adapters, independent of transport.
pub fn source_read_cap(adapter: &str) -> Result<u64> {
    match adapter {
        "yaml-ledger-v1" | "yaml-workstream-ledger-v1" => Ok(YAML_READ_CAP),
        "markdown-ledger-v1"
        | "markdown-heading-v1"
        | "markdown-rules-v1"
        | "markdown-directory-v1" => Ok(MARKDOWN_READ_CAP),
        _ => Err(Error::Unsupported(format!("source adapter {adapter}"))),
    }
}

pub(crate) fn check_source_size(bytes: &[u8], cap: u64) -> Result<()> {
    check_source_size_only(bytes, cap)?;
    awr_core::ensure_public_bytes(bytes)
}

pub(crate) fn check_source_size_only(bytes: &[u8], cap: u64) -> Result<()> {
    if bytes.len() as u64 > cap {
        return Err(Error::InvalidInput(format!(
            "source exceeds {cap} byte read cap"
        )));
    }
    Ok(())
}

/// Preserve deterministic ledger-size facts across the source/PG crate boundary.
/// The detail contains numbers only, never source bytes or filesystem locations.
pub(crate) fn ledger_too_large(actual: u64, limit: u64) -> Error {
    Error::WithDetails {
        error: Box::new(Error::InvalidInput(
            "source ledger exceeds its byte limit".into(),
        )),
        details: serde_json::json!({
            LEDGER_LIMIT_DETAIL: {"actual":actual,"limit":limit}
        }),
    }
}

/// Recover only the closed numeric detail emitted by `ledger_too_large`.
pub fn ledger_size_limit_exceeded(error: &Error) -> Option<(u64, u64)> {
    let Error::WithDetails { details, .. } = error else {
        return None;
    };
    let detail = details.get(LEDGER_LIMIT_DETAIL)?;
    Some((
        detail.get("actual")?.as_u64()?,
        detail.get("limit")?.as_u64()?,
    ))
}
