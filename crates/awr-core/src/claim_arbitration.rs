//! Deterministic cross-node claim arbitration — design XCLAIM-1 v1.1 §3.1.
//!
//! The rule is pure: it reads only arbiter-issued data (`fence`, arbiter-clock `expires_at_ms`)
//! and never a node's own wall clock, so two devices that disagree about the time still reach the
//! same verdict. `node_clock_ms` is carried for audit and must stay unread; see `tests/claim_arbitration.rs`.
//!
//! Time contract: every `expires_at_ms` handed to this module is already **arbiter clock**. A
//! claim is expired when `expires_at_ms <= arbiter_at_ms` — a strict comparison, with no tolerance
//! window, because a tolerance added to the arbiter clock would evict leases that are still live.

use crate::error::Error;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// One node's view of its claim on a work item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCandidate {
    /// Stable per-device identity, e.g. `sha256:9f2c…` of the host identity.
    pub node_id: String,
    pub session_id: String,
    /// Monotone, arbiter-issued, unique per work item.
    pub fence: u64,
    /// Arbiter-clock epoch milliseconds.
    pub expires_at_ms: i64,
    /// The claimant's own wall clock. Evidence only: the rule never reads it.
    pub node_clock_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateState {
    Live,
    Expired,
}

impl CandidateState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Expired => "expired",
        }
    }
}

/// The outcome for one work item. `candidates` is sorted by `fence` so the record is stable
/// regardless of the order the caller supplied them in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArbitrationVerdict {
    pub work_item: String,
    pub arbiter_at_ms: i64,
    pub winner_fence: Option<u64>,
    pub candidates: Vec<(ClaimCandidate, CandidateState)>,
    /// Fences that lost, ascending. Empty when there is no live claim.
    pub superseded: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArbitrationError {
    #[error(
        "claim fence {fence} is held by more than one live candidate on {work_item}: {node_ids:?}"
    )]
    AmbiguousFence {
        work_item: String,
        fence: u64,
        node_ids: Vec<String>,
    },
    #[error("no claim candidate was supplied for {work_item}")]
    EmptyCandidateSet { work_item: String },
    #[error("arbitration record could not be serialised: {0}")]
    Record(String),
}

impl From<ArbitrationError> for Error {
    fn from(value: ArbitrationError) -> Self {
        Error::InvalidInput(value.to_string())
    }
}

/// Decide which live claim owns a work item.
///
/// 1. `state(c) = Expired` iff `c.expires_at_ms <= arbiter_at_ms`, else `Live`.
/// 2. If no candidate is live, `winner_fence = None` and `superseded` is empty.
/// 3. Otherwise the winner is the live candidate with the greatest `fence`.
/// 4. Two live candidates sharing the greatest `fence` is `AmbiguousFence`, never a silent pick.
/// 5. `superseded` lists the losing live fences, ascending.
pub fn arbitrate_claims(
    work_item: &str,
    candidates: &[ClaimCandidate],
    arbiter_at_ms: i64,
) -> Result<ArbitrationVerdict, ArbitrationError> {
    if candidates.is_empty() {
        return Err(ArbitrationError::EmptyCandidateSet {
            work_item: work_item.to_string(),
        });
    }

    let mut ordered: Vec<(ClaimCandidate, CandidateState)> = candidates
        .iter()
        .cloned()
        .map(|candidate| {
            let state = if candidate.expires_at_ms <= arbiter_at_ms {
                CandidateState::Expired
            } else {
                CandidateState::Live
            };
            (candidate, state)
        })
        .collect();
    ordered.sort_by_key(|(candidate, _)| candidate.fence);

    let live: Vec<&ClaimCandidate> = ordered
        .iter()
        .filter(|(_, state)| *state == CandidateState::Live)
        .map(|(candidate, _)| candidate)
        .collect();

    let winner_fence = match live.iter().map(|candidate| candidate.fence).max() {
        None => None,
        Some(top) => {
            let holders: Vec<&ClaimCandidate> = live
                .iter()
                .copied()
                .filter(|candidate| candidate.fence == top)
                .collect();
            if holders.len() > 1 {
                let mut node_ids: Vec<String> = holders
                    .iter()
                    .map(|candidate| candidate.node_id.clone())
                    .collect();
                node_ids.sort();
                return Err(ArbitrationError::AmbiguousFence {
                    work_item: work_item.to_string(),
                    fence: top,
                    node_ids,
                });
            }
            Some(top)
        }
    };

    let superseded: Vec<u64> = match winner_fence {
        None => Vec::new(),
        Some(top) => live
            .iter()
            .map(|candidate| candidate.fence)
            .filter(|fence| *fence != top)
            .collect(),
    };

    Ok(ArbitrationVerdict {
        work_item: work_item.to_string(),
        arbiter_at_ms,
        winner_fence,
        candidates: ordered,
        superseded,
    })
}

impl ArbitrationVerdict {
    /// Canonical JSON: sorted keys, integers only, candidates in ascending `fence` order. Equal
    /// fences are not tie-broken: the sort is stable, so candidates that share a `fence` keep the
    /// caller's input order. The same input therefore reproduces the same bytes, and `digest()` is
    /// stable for it, but a permutation of equal-`fence` candidates is not guaranteed to reproduce
    /// the same record. That reproducibility is what makes the verdict auditable; giving ties a
    /// total order would be a behaviour change and is deliberately left out of this change.
    pub fn record(&self) -> Result<String, ArbitrationError> {
        let candidates: Vec<Value> = self
            .candidates
            .iter()
            .map(|(candidate, state)| {
                let mut fields: BTreeMap<&str, Value> = BTreeMap::new();
                fields.insert("expires_at_ms", json!(candidate.expires_at_ms));
                fields.insert("fence", json!(candidate.fence));
                fields.insert("node_clock_ms", json!(candidate.node_clock_ms));
                fields.insert("node_id", json!(candidate.node_id));
                fields.insert("session_id", json!(candidate.session_id));
                fields.insert("state", json!(state.as_str()));
                Value::Object(
                    fields
                        .into_iter()
                        .map(|(key, value)| (key.to_string(), value))
                        .collect(),
                )
            })
            .collect();

        let mut fields: BTreeMap<&str, Value> = BTreeMap::new();
        fields.insert("arbiter_at_ms", json!(self.arbiter_at_ms));
        fields.insert("candidates", json!(candidates));
        fields.insert("superseded", json!(self.superseded));
        fields.insert("winner_fence", json!(self.winner_fence));
        fields.insert("work_item", json!(self.work_item));
        let value = Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        );
        serde_json::to_string(&value).map_err(|error| ArbitrationError::Record(error.to_string()))
    }

    /// Lowercase hex sha256 of [`Self::record`], so a verdict can be quoted by digest.
    pub fn digest(&self) -> Result<String, ArbitrationError> {
        let record = self.record()?;
        Ok(Sha256::digest(record.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
}

/// The pre-feature rule, kept only as a counterexample: it compares `expires_at_ms` against the
/// claimant's **own** wall clock. Used by `tests/claim_arbitration.rs` to prove the clock
/// independence test can actually fail; never used in the store or the CLI.
pub fn arbitrate_claims_legacy_node_clock(
    work_item: &str,
    candidates: &[ClaimCandidate],
) -> Result<ArbitrationVerdict, ArbitrationError> {
    if candidates.is_empty() {
        return Err(ArbitrationError::EmptyCandidateSet {
            work_item: work_item.to_string(),
        });
    }
    let mut ordered: Vec<(ClaimCandidate, CandidateState)> = candidates
        .iter()
        .cloned()
        .map(|candidate| {
            let state = if candidate.expires_at_ms <= candidate.node_clock_ms {
                CandidateState::Expired
            } else {
                CandidateState::Live
            };
            (candidate, state)
        })
        .collect();
    ordered.sort_by_key(|(candidate, _)| candidate.fence);
    let live: Vec<&ClaimCandidate> = ordered
        .iter()
        .filter(|(_, state)| *state == CandidateState::Live)
        .map(|(candidate, _)| candidate)
        .collect();
    let winner_fence = live.iter().map(|candidate| candidate.fence).max();
    let superseded = match winner_fence {
        None => Vec::new(),
        Some(top) => live
            .iter()
            .map(|candidate| candidate.fence)
            .filter(|fence| *fence != top)
            .collect(),
    };
    Ok(ArbitrationVerdict {
        work_item: work_item.to_string(),
        arbiter_at_ms: 0,
        winner_fence,
        candidates: ordered,
        superseded,
    })
}
