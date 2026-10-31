//! The project's claim policy applied: the lifetime of a claim whose request gave none, and the renewal that
//! recorded work earns. Both the CLI and the MCP server get them here, so a default cannot differ by surface.
use awr_core::{ClaimRenewal, Error, Result};
use awr_source::ClaimPolicy;
use std::path::Path;

/// How a request names the lifetime of the claim it asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimTtl {
    /// The request gave none: the project's default, when it declares one.
    Policy,
    Millis(u64),
    /// Asked for explicitly: the claim has no expiry whatever the project declares.
    NoExpiry,
}

impl ClaimTtl {
    /// From the two request fields; asking for a lifetime and for none is a contradiction.
    pub fn from_request(ttl_ms: Option<u64>, no_expiry: bool) -> Result<Self> {
        match (ttl_ms, no_expiry) {
            (Some(_), true) => Err(Error::InvalidInput(
                "ttl_ms and no_expiry contradict each other; give one".into(),
            )),
            (Some(ms), false) => Ok(Self::Millis(ms)),
            (None, true) => Ok(Self::NoExpiry),
            (None, false) => Ok(Self::Policy),
        }
    }
}

/// The `ttl_ms` of an acquisition. A lifetime given in the request is unchanged and `NoExpiry` is none, neither
/// reading the policy, so an explicit request works whatever state the policy file is in; a request that gave
/// none gets the project's default lifetime, and without a declared default has no expiry, as it always had.
pub fn resolve_claim_ttl(root: &Path, ttl: ClaimTtl) -> Result<Option<u64>> {
    match ttl {
        ClaimTtl::Millis(ms) => Ok(Some(ms)),
        ClaimTtl::NoExpiry => Ok(None),
        ClaimTtl::Policy => Ok(ClaimPolicy::load(root)?.default_ttl_ms()),
    }
}

/// The renewal the project's policy asks recorded work to earn. A policy that cannot be read renews nothing: the
/// work being recorded does not fail for it, and `doctor` reports the policy.
pub(crate) fn renewal_for(root: &Path) -> Option<ClaimRenewal> {
    let ttl_ms = ClaimPolicy::load(root).ok()?.renewal_ttl_ms()?;
    Some(ClaimRenewal { ttl_ms })
}
