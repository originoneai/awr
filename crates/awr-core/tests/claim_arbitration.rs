//! Cross-node claim arbitration — design XCLAIM-1 v1.0, acceptance A1–A6.
//! A7 (reclaimed claims) is storage-layer only — four entry points plus a migration — so it is out
//! of scope for this pure-rule target; see the note below.
//!
//! These tests call the **production** rule in `awr_core::claim_arbitration`, not a local copy:
//! a green run must mean the shipped function is correct. The only in-file implementation is the
//! pre-feature rule that A3b uses as a deliberate counterexample.
//!
//! Locally verified on a Windows/gnu host: `cargo test -p awr-core --test claim_arbitration`
//! reported 7 passed, 0 failed, 0 ignored, and `cargo +1.93.1-x86_64-pc-windows-gnu fmt --all
//! --check` reported no diff. That covers only this single `awr-core` test target and formatting;
//! the native platform gate in CI remains the authority, and a passing result may only be quoted
//! with a CI run id.
//!
//! Design decisions under review that these tests pin:
//! - expiry is a **strict** arbiter-clock comparison (`expires_at_ms <= arbiter_at_ms`); there is no
//!   tolerance window, because a tolerance added to the arbiter clock would evict live leases;
//! - `node_clock_ms` is evidence only and must not change any outcome (A3/A3b).
//!
//! A7 (a reclaimed claim must not be usable by its former holder) is a **storage-layer** criterion:
//! it needs fence validation at all four entry points (`session progress`, `work complete`,
//! `evidence add`, `release`) plus a migration, so it is not covered by this pure-rule target. The
//! earlier `#[ignore]`d stub that declared it here was removed rather than registered as an ignored
//! entry with no native execution route.

use awr_core::claim_arbitration::{
    ArbitrationError, ArbitrationVerdict, CandidateState, ClaimCandidate, arbitrate_claims,
    arbitrate_claims_legacy_node_clock,
};

fn cand(
    node: &str,
    session: &str,
    fence: u64,
    expires_at_ms: i64,
    node_clock_ms: i64,
) -> ClaimCandidate {
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
    for index in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(index);
        for mut tail in permutations(&rest) {
            let mut perm = vec![head.clone()];
            perm.append(&mut tail);
            out.push(perm);
        }
    }
    out
}

fn states(verdict: &ArbitrationVerdict) -> Vec<CandidateState> {
    verdict.candidates.iter().map(|(_, state)| *state).collect()
}

/// A1 — the verdict and its record are invariant under permutation of the candidate list.
#[test]
fn a1_verdict_is_permutation_invariant() {
    let base = vec![
        cand("node-c", "sess-3", 9, 10_000, 0),
        cand("node-a", "sess-1", 4, 10_000, 0),
        cand("node-b", "sess-2", 7, 10_000, 0),
    ];
    let expected = arbitrate_claims("WORK-1", &base, 0).expect("unambiguous");
    assert_eq!(expected.winner_fence, Some(9));
    assert_eq!(expected.superseded, vec![4, 7]);
    let expected_record = expected.record().expect("record");

    for perm in permutations(&base) {
        let got = arbitrate_claims("WORK-1", &perm, 0).expect("unambiguous");
        assert_eq!(
            got.winner_fence, expected.winner_fence,
            "winner drifted under permutation"
        );
        assert_eq!(
            got.record().expect("record"),
            expected_record,
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
    let verdict = arbitrate_claims("WORK-1", &distinct, 0).expect("unambiguous");
    assert_eq!(verdict.winner_fence, Some(7));
    assert_eq!(verdict.superseded, vec![4]);
    assert_eq!(
        states(&verdict)
            .iter()
            .filter(|state| **state == CandidateState::Live)
            .count(),
        2
    );

    let clashing = vec![
        cand("node-b", "sess-2", 7, 10_000, 0),
        cand("node-a", "sess-1", 7, 10_000, 0),
    ];
    match arbitrate_claims("WORK-1", &clashing, 0) {
        Err(ArbitrationError::AmbiguousFence {
            fence, node_ids, ..
        }) => {
            assert_eq!(fence, 7);
            assert_eq!(node_ids, vec!["node-a".to_string(), "node-b".to_string()]);
        }
        other => panic!("expected AmbiguousFence, got {other:?}"),
    }
}

/// A3 / A3b — the verdict must not read any node clock, and this test must be able to notice if it did.
#[test]
fn a3_verdict_ignores_node_clock_skew_while_the_legacy_rule_does_not() {
    // A3b needs the legacy node-clock rule to reach a *different* winner on this same data,
    // otherwise the negative control is vacuous. node-a's node_clock_ms is therefore >= its own
    // expires_at_ms: the legacy rule expires node-a (fence 2) while the arbiter clock (0) keeps it
    // live under the production rule, so the production rule wins with fence 2 and the legacy rule
    // with fence 1.
    let base = vec![
        cand("node-a", "sess-1", 2, 1_000, 1_000),
        cand("node-b", "sess-2", 1, 900, 100),
    ];
    let expected = arbitrate_claims("WORK-1", &base, 0).expect("unambiguous");
    let expected_record = expected.record().expect("record");

    for shift in [-10_000_000i64, 10_000_000] {
        let skewed: Vec<ClaimCandidate> = base
            .iter()
            .cloned()
            .map(|mut candidate| {
                candidate.node_clock_ms += shift;
                candidate
            })
            .collect();
        let got = arbitrate_claims("WORK-1", &skewed, 0).expect("unambiguous");
        assert_eq!(
            got.winner_fence, expected.winner_fence,
            "winner moved with the node clock"
        );
        assert_eq!(
            states(&got),
            states(&expected),
            "candidate states moved with the node clock"
        );
        // The record differs exactly in the audit-only field: zero it out and demand equality.
        assert_ne!(
            got.record().expect("record"),
            expected_record,
            "record should differ in the node_clock_ms audit field"
        );
        assert_eq!(
            zero_node_clocks(&got.record().expect("record")),
            zero_node_clocks(&expected_record),
            "record differs outside the node_clock_ms audit field"
        );
    }

    // A3b: the legacy rule reads the node clock and reaches a different winner on the same data.
    // Without this, the assertions above could pass vacuously.
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
        let verdict = arbitrate_claims("WORK-1", &candidates, at).expect("unambiguous");
        let expired = states(&verdict)
            .iter()
            .filter(|state| **state == CandidateState::Expired)
            .count();
        assert!(
            expired >= previous,
            "expired count regressed at arbiter_at_ms={at}"
        );
        previous = expired;
    }
    assert_eq!(
        previous, 2,
        "both candidates must expire by arbiter_at_ms = 10^7"
    );
}

/// A5 — the boundary is strict: expiry at equality, live one millisecond later. No tolerance window.
#[test]
fn a5_expiry_boundary_is_strict_with_no_tolerance() {
    let at = 1_000_000i64;

    let at_equality = vec![cand("node-a", "sess-1", 1, at, 0)];
    let verdict = arbitrate_claims("WORK-1", &at_equality, at).expect("unambiguous");
    assert_eq!(verdict.winner_fence, None);
    assert_eq!(states(&verdict), vec![CandidateState::Expired]);
    assert!(verdict.superseded.is_empty());

    let one_ms_later = vec![cand("node-a", "sess-1", 1, at + 1, 0)];
    let verdict = arbitrate_claims("WORK-1", &one_ms_later, at).expect("unambiguous");
    assert_eq!(verdict.winner_fence, Some(1));
    assert_eq!(states(&verdict), vec![CandidateState::Live]);
}

/// A6 — the evidence record is reproducible and carries no floating point.
#[test]
fn a6_record_is_reproducible_and_hashable() {
    let candidates = vec![
        cand("node-a", "sess-1", 4, 10_000, 12),
        cand("node-b", "sess-2", 7, 10_000, 34),
    ];
    let first = arbitrate_claims("WORK-1", &candidates, 0).expect("unambiguous");
    let second = arbitrate_claims("WORK-1", &candidates, 0).expect("unambiguous");
    let record = first.record().expect("record");
    assert_eq!(
        record,
        second.record().expect("record"),
        "record is not reproducible for an identical input tuple"
    );
    assert!(
        !record.contains('.'),
        "record must contain no floating point: {record}"
    );

    let digest = first.digest().expect("digest");
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    );
    assert_eq!(digest, second.digest().expect("digest"));
}

/// Guard: an empty candidate set is an explicit error, not a `winner = None` verdict.
#[test]
fn empty_candidate_set_is_an_error() {
    let empty: Vec<ClaimCandidate> = Vec::new();
    match arbitrate_claims("WORK-1", &empty, 0) {
        Err(ArbitrationError::EmptyCandidateSet { work_item }) => assert_eq!(work_item, "WORK-1"),
        other => panic!("expected EmptyCandidateSet, got {other:?}"),
    }
}

/// Zeros every `node_clock_ms` in a serialised record so two records can be compared outside the
/// audit-only field.
fn zero_node_clocks(record: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(record).expect("record is JSON");
    if let Some(candidates) = value.get_mut("candidates").and_then(|c| c.as_array_mut()) {
        for candidate in candidates {
            if let Some(object) = candidate.as_object_mut() {
                object.insert("node_clock_ms".to_string(), serde_json::json!(0));
            }
        }
    }
    value.to_string()
}
