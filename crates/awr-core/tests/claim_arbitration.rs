//! Spec tests for deterministic cross-node claim arbitration — design v1.0.
//!
//! See `reports/feature-cross-node-claim-design-v1.0.md` §3.1 (normative rule) and §4 (A1–A6).
//!
//! TODO(design-approval): replace the throwaway `reference` module below with
//! `awr_core::claim_arbitration` once the production module lands. Until then this file pins the
//! *specification* so CI can validate the rule before the module exists.
//!
//! TODO(verify): these tests have **never been executed** on the authoring host — there is no
//! Rust linker there, so `cargo test` cannot link. A passing result may only be quoted together
//! with a CI run id; this file makes no claim to have passed.

use sha2::{Digest, Sha256};

/// Throwaway reference implementation of the normative rule. Not production code.
///
/// Differences from the intended production module are limited to: no `thiserror` derive, no
/// public API surface, no serde derives on the input/output types.
mod reference {
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    #[derive(Debug, Clone, PartialEq)]
    pub struct ClaimCandidate {
        pub node_id: String,
        pub session_id: String,
        pub fence: u64,
        pub expires_at_ms: i64,
        /// Audit field only. The rule must never read it (A3).
        pub node_clock_ms: i64,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CandidateState {
        Live,
        Expired,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub struct ArbitrationVerdict {
        pub work_item: String,
        pub arbiter_at_ms: i64,
        pub skew_tolerance_ms: i64,
        pub winner_fence: Option<u64>,
        /// Sorted by fence, so the record is permutation-invariant (A1).
        pub candidates: Vec<(ClaimCandidate, CandidateState)>,
        pub superseded: Vec<u64>,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum ArbitrationError {
        AmbiguousFence {
            work_item: String,
            fence: u64,
            node_ids: Vec<String>,
        },
        EmptyCandidateSet {
            work_item: String,
        },
    }

    pub fn arbitrate_claims(
        work_item: &str,
        candidates: &[ClaimCandidate],
        arbiter_at_ms: i64,
        skew_tolerance_ms: i64,
    ) -> Result<ArbitrationVerdict, ArbitrationError> {
        if candidates.is_empty() {
            return Err(ArbitrationError::EmptyCandidateSet {
                work_item: work_item.to_string(),
            });
        }
        // Rule step 1+2: expiry is arbiter-clock only, closed interval at the tolerance edge.
        let deadline = arbiter_at_ms.saturating_add(skew_tolerance_ms);
        let mut ordered: Vec<(ClaimCandidate, CandidateState)> = candidates
            .iter()
            .cloned()
            .map(|c| {
                let state = if c.expires_at_ms <= deadline {
                    CandidateState::Expired
                } else {
                    CandidateState::Live
                };
                (c, state)
            })
            .collect();
        ordered.sort_by_key(|(c, _)| c.fence);

        let live: Vec<&ClaimCandidate> = ordered
            .iter()
            .filter(|(_, s)| *s == CandidateState::Live)
            .map(|(c, _)| c)
            .collect();

        // Rule step 3+4: unique maximum fence, otherwise refuse to decide.
        let winner_fence = match live.iter().map(|c| c.fence).max() {
            None => None,
            Some(top) => {
                let holders: Vec<&ClaimCandidate> =
                    live.iter().copied().filter(|c| c.fence == top).collect();
                if holders.len() > 1 {
                    let mut node_ids: Vec<String> =
                        holders.iter().map(|c| c.node_id.clone()).collect();
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

        // Rule step 5: losers, ascending.
        let superseded: Vec<u64> = match winner_fence {
            None => Vec::new(),
            Some(top) => live.iter().map(|c| c.fence).filter(|f| *f != top).collect(),
        };

        Ok(ArbitrationVerdict {
            work_item: work_item.to_string(),
            arbiter_at_ms,
            skew_tolerance_ms,
            winner_fence,
            candidates: ordered,
            superseded,
        })
    }

    /// Canonical, byte-stable JSON (sorted keys, integers only, candidates sorted by fence).
    pub fn arbitration_record(v: &ArbitrationVerdict) -> Value {
        let candidates: Vec<Value> = v
            .candidates
            .iter()
            .map(|(c, state)| {
                let mut m = BTreeMap::new();
                m.insert("expires_at_ms", json!(c.expires_at_ms));
                m.insert("fence", json!(c.fence));
                m.insert("node_clock_ms", json!(c.node_clock_ms));
                m.insert("node_id", json!(c.node_id));
                m.insert("session_id", json!(c.session_id));
                m.insert(
                    "state",
                    json!(match state {
                        CandidateState::Live => "live",
                        CandidateState::Expired => "expired",
                    }),
                );
                Value::Object(m.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
            })
            .collect();
        let mut m = BTreeMap::new();
        m.insert("arbiter_at_ms", json!(v.arbiter_at_ms));
        m.insert("candidates", json!(candidates));
        m.insert("skew_tolerance_ms", json!(v.skew_tolerance_ms));
        m.insert("superseded", json!(v.superseded));
        m.insert("winner_fence", json!(v.winner_fence));
        m.insert("work_item", json!(v.work_item));
        Value::Object(m.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// The old, pre-feature rule: expiry against the claimant's own wall clock.
    /// Kept only to prove the A3 test has discriminating power (A3b).
    pub fn arbitrate_claims_legacy_node_clock(
        work_item: &str,
        candidates: &[ClaimCandidate],
    ) -> Result<ArbitrationVerdict, ArbitrationError> {
        let mut ordered: Vec<(ClaimCandidate, CandidateState)> = candidates
            .iter()
            .cloned()
            .map(|c| {
                let state = if c.expires_at_ms <= c.node_clock_ms {
                    CandidateState::Expired
                } else {
                    CandidateState::Live
                };
                (c, state)
            })
            .collect();
        ordered.sort_by_key(|(c, _)| c.fence);
        let live: Vec<&ClaimCandidate> = ordered
            .iter()
            .filter(|(_, s)| *s == CandidateState::Live)
            .map(|(c, _)| c)
            .collect();
        let winner_fence = live.iter().map(|c| c.fence).max();
        let superseded = match winner_fence {
            None => Vec::new(),
            Some(top) => live.iter().map(|c| c.fence).filter(|f| *f != top).collect(),
        };
        Ok(ArbitrationVerdict {
            work_item: work_item.to_string(),
            arbiter_at_ms: 0,
            skew_tolerance_ms: 0,
            winner_fence,
            candidates: ordered,
            superseded,
        })
    }
}

use reference::{
    arbitrate_claims, arbitrate_claims_legacy_node_clock, arbitration_record,
    ArbitrationError, CandidateState, ClaimCandidate,
};
use serde_json::Value;

fn cand(node: &str, session: &str, fence: u64, expires_at_ms: i64, node_clock_ms: i64) -> ClaimCandidate {
    ClaimCandidate {
        node_id: node.to_string(),
        session_id: session.to_string(),
        fence,
        expires_at_ms,
        node_clock_ms,
    }
}

fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.is_empty() {
        return vec![vec![]];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(i);
        for mut tail in permutations(&rest) {
            let mut perm = vec![head.clone()];
            perm.append(&mut tail);
            out.push(perm);
        }
    }
    out
}

fn sha256_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A1 — permutation invariance of both the winner and the emitted record.
#[test]
fn a1_verdict_is_permutation_invariant() {
    let base = vec![
        cand("node-c", "sess-3", 9, 10_000, 0),
        cand("node-a", "sess-1", 4, 10_000, 0),
        cand("node-b", "sess-2", 7, 10_000, 0),
    ];
    let expected = arbitrate_claims("WORK-1", &base, 0, 0).expect("unambiguous");
    assert_eq!(expected.winner_fence, Some(9));
    assert_eq!(expected.superseded, vec![4, 7]);

    for perm in permutations(&base) {
        let got = arbitrate_claims("WORK-1", &perm, 0, 0).expect("unambiguous");
        assert_eq!(got.winner_fence, expected.winner_fence, "winner drifted under permutation");
        assert_eq!(
            arbitration_record(&got).to_string(),
            arbitration_record(&expected).to_string(),
            "record is not byte-stable under permutation"
        );
    }
}

/// A2 — exactly one winner, or an explicit ambiguity error; never a silent pick.
#[test]
fn a2_duplicate_max_fence_is_refused_not_guessed() {
    let distinct = vec![
        cand("node-a", "sess-1", 4, 10_000, 0),
        cand("node-b", "sess-2", 7, 10_000, 0),
    ];
    let v = arbitrate_claims("WORK-1", &distinct, 0, 0).expect("unambiguous");
    assert_eq!(v.winner_fence, Some(7));
    assert_eq!(v.superseded, vec![4]);
    assert_eq!(v.candidates.iter().filter(|(_, s)| *s == CandidateState::Live).count(), 2);

    let clashing = vec![
        cand("node-b", "sess-2", 7, 10_000, 0),
        cand("node-a", "sess-1", 7, 10_000, 0),
    ];
    match arbitrate_claims("WORK-1", &clashing, 0, 0) {
        Err(ArbitrationError::AmbiguousFence { fence, node_ids, .. }) => {
            assert_eq!(fence, 7);
            assert_eq!(node_ids, vec!["node-a".to_string(), "node-b".to_string()]);
        }
        other => panic!("expected AmbiguousFence, got {other:?}"),
    }
}

/// A3 / A3b — the verdict must not read any node clock, and the test must be able to notice if it did.
#[test]
fn a3_verdict_ignores_node_clock_skew_while_the_legacy_rule_does_not() {
    let base = vec![
        cand("node-a", "sess-1", 2, 1_000, 500),
        cand("node-b", "sess-2", 1, 900, 100),
    ];
    let expected = arbitrate_claims("WORK-1", &base, 0, 0).expect("unambiguous");

    for shift in [-10_000_000i64, 10_000_000] {
        let skewed: Vec<ClaimCandidate> = base
            .iter()
            .cloned()
            .map(|mut c| {
                c.node_clock_ms += shift;
                c
            })
            .collect();
        let got = arbitrate_claims("WORK-1", &skewed, 0, 0).expect("unambiguous");
        assert_eq!(got.winner_fence, expected.winner_fence, "winner moved with node clock");
        assert_eq!(
            got.candidates.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
            expected.candidates.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
            "candidate states moved with node clock"
        );
        // Only the audit-only field may differ: zero it out on both sides and demand equality.
        let a = arbitration_record(&expected);
        let b = arbitration_record(&got);
        assert_ne!(a, b, "record should differ exactly in the node_clock_ms audit field");
        assert_eq!(
            zero_node_clocks(&a),
            zero_node_clocks(&b),
            "record differs outside the node_clock_ms audit field"
        );
    }

    // A3b: the legacy rule reads the node clock and reaches a different winner on the same data,
    // which is what makes the assertion above discriminating rather than vacuous.
    let legacy = arbitrate_claims_legacy_node_clock("WORK-1", &base).expect("legacy ok");
    assert_ne!(
        legacy.winner_fence, expected.winner_fence,
        "legacy rule should disagree; otherwise this test proves nothing"
    );
}

/// A4 — the expired set only grows as the arbiter clock advances.
#[test]
fn a4_expiry_is_monotone_in_arbiter_time() {
    let candidates = vec![
        cand("node-a", "sess-1", 1, 2_000_000, 0),
        cand("node-b", "sess-2", 2, 5_000_000, 0),
    ];
    let mut previous = 0usize;
    for step in 0..=100i64 {
        let at = step * 100_000;
        let v = arbitrate_claims("WORK-1", &candidates, at, 0).expect("unambiguous");
        let expired = v
            .candidates
            .iter()
            .filter(|(_, s)| *s == CandidateState::Expired)
            .count();
        assert!(expired >= previous, "expired count regressed at arbiter_at_ms={at}");
        previous = expired;
    }
    assert_eq!(previous, 2, "both candidates must expire by arbiter_at_ms = 10^7");
}

/// A5 — the tolerance edge is a closed interval.
#[test]
fn a5_skew_tolerance_boundary_is_closed() {
    let at = 1_000_000i64;
    let skew = 30_000i64;

    let on_edge = vec![cand("node-a", "sess-1", 1, at + skew, 0)];
    let v = arbitrate_claims("WORK-1", &on_edge, at, skew).expect("unambiguous");
    assert_eq!(v.winner_fence, None);
    assert_eq!(v.candidates[0].1, CandidateState::Expired);

    let just_inside = vec![cand("node-a", "sess-1", 1, at + skew + 1, 0)];
    let v = arbitrate_claims("WORK-1", &just_inside, at, skew).expect("unambiguous");
    assert_eq!(v.winner_fence, Some(1));
    assert_eq!(v.candidates[0].1, CandidateState::Live);
}

/// A6 — the evidence record is reproducible and contains no floating point.
#[test]
fn a6_record_is_reproducible_and_hashable() {
    let candidates = vec![
        cand("node-a", "sess-1", 4, 10_000, 12),
        cand("node-b", "sess-2", 7, 10_000, 34),
    ];
    let first = arbitrate_claims("WORK-1", &candidates, 0, 30_000).expect("unambiguous");
    let second = arbitrate_claims("WORK-1", &candidates, 0, 30_000).expect("unambiguous");
    let a = arbitration_record(&first).to_string();
    let b = arbitration_record(&second).to_string();
    assert_eq!(a, b, "record is not reproducible for an identical input tuple");
    assert!(!a.contains('.'), "record must contain no floating point: {a}");
    let hash = sha256_hex(&a);
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
}

/// Guard: an empty candidate set is an explicit error, not a `winner = None` verdict.
#[test]
fn empty_candidate_set_is_an_error() {
    let empty: Vec<ClaimCandidate> = Vec::new();
    match arbitrate_claims("WORK-1", &empty, 0, 0) {
        Err(ArbitrationError::EmptyCandidateSet { work_item }) => assert_eq!(work_item, "WORK-1"),
        other => panic!("expected EmptyCandidateSet, got {other:?}"),
    }
}

/// A7 is a storage-layer criterion (design §4): after a claim is reclaimed, an execution request
/// carrying the former holder's fence must be refused, not applied. It cannot be expressed as a
/// pure rule, so it is only *declared* here and its real body is TODO until migration 015 and the
/// `session.rs` arbiter-clock change are approved (design §3.2). The store-level test will live in
/// `crates/awr-store/tests/claim_conflict_cross_node.rs`.
#[test]
#[ignore = "TODO: A7 needs migration 015 + session.rs arbiter-clock change (design §3.2); not implemented"]
fn a7_reclaimed_claim_cannot_be_used_by_its_former_holder() {
    todo!("see design §3.2 / §4 A7; requires the storage layer change, not the pure rule")
}

/// Replaces every `node_clock_ms` with 0 so two records can be compared for equality outside the
/// audit-only field.
fn zero_node_clocks(record: &Value) -> Value {
    let mut copy = record.clone();
    if let Some(candidates) = copy.get_mut("candidates").and_then(|c| c.as_array_mut()) {
        for candidate in candidates {
            if let Some(object) = candidate.as_object_mut() {
                object.insert("node_clock_ms".to_string(), serde_json::json!(0));
            }
        }
    }
    copy
}

/// Keeps the unused-import lint honest for the audit-only field in the fixtures above.
#[test]
fn record_carries_node_clock_as_audit_only() {
    let v = arbitrate_claims("WORK-1", &[cand("node-a", "sess-1", 1, 10, 4_242)], 0, 0)
        .expect("unambiguous");
    let record: Value = arbitration_record(&v);
    assert_eq!(record["candidates"][0]["node_clock_ms"], serde_json::json!(4_242));
    assert_eq!(record["winner_fence"], serde_json::json!(1));
}
