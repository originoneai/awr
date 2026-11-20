//! One shared rule for independent acceptance of a completion.
//!
//! AWR-PPD-011's period fact snapshots and AWR-PPD-060's portfolio overview
//! must answer "verified?" identically. The rule was defined on the branch
//! `kimi/ppd-011-fact-snapshots` (`workstream_command/updates.rs`,
//! `independently_accepted`); it lives here so both readers use one function
//! and cannot drift. The inputs are the payload fields exactly as the two
//! producers write them (see the tests, which seed both payload shapes).

/// Whether a completion event payload proves independent acceptance, read in
/// this order:
///
/// 1. `independence_kind` — `work.complete` (`workstream_command/reviews.rs`)
///    writes it from the review decision. `team_independent`, `agent_review`
///    and `simulated_member_independent` verify; `personal_self_review`,
///    `ordinary_confirm`, `unspecified` and any other value stay unverified.
///    An agent review counts: the service refuses a review by the author or
///    the executor, so by construction a different actor accepted on the
///    current evidence.
/// 2. else the stated `team_independent_acceptance` boolean.
/// 3. else the legacy receipt of `review.rs`, whose `policy` names one of the
///    two agent/simulated-member independent review policies.
///
/// A payload without any basis is recorded, never verified.
pub(crate) fn independently_accepted(
    independence_kind: Option<&str>,
    team_independent_acceptance: Option<bool>,
    policy: Option<&str>,
) -> bool {
    match independence_kind {
        Some(kind) => matches!(
            kind,
            "team_independent" | "agent_review" | "simulated_member_independent"
        ),
        None => match team_independent_acceptance {
            Some(independent) => independent,
            None => matches!(policy, Some(policy)
                if policy == crate::review::AGENT_REVIEW_POLICY
                    || policy == crate::review::simulated_member::POLICY),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::independently_accepted;

    #[test]
    fn verifies_only_independent_kinds_and_acceptance_and_legacy_policies() {
        // independence_kind wins whenever the current producer wrote it.
        for kind in [
            "team_independent",
            "agent_review",
            "simulated_member_independent",
        ] {
            assert!(
                independently_accepted(Some(kind), Some(false), None),
                "{kind}"
            );
        }
        for kind in [
            "personal_self_review",
            "ordinary_confirm",
            "unspecified",
            "synthetic-unknown-kind",
        ] {
            assert!(
                !independently_accepted(Some(kind), Some(true), None),
                "{kind}"
            );
        }
        // An agent-review payload carries a false acceptance flag on purpose:
        // step 1 already classified it, so the flag is never consulted.
        assert!(independently_accepted(
            Some("agent_review"),
            Some(false),
            None
        ));
        // Without independence_kind, the stated acceptance decides.
        assert!(independently_accepted(None, Some(true), None));
        assert!(!independently_accepted(None, Some(false), None));
        // Legacy payloads: only the two independent review policies verify.
        assert!(independently_accepted(
            None,
            None,
            Some(crate::review::AGENT_REVIEW_POLICY)
        ));
        assert!(independently_accepted(
            None,
            None,
            Some(crate::review::simulated_member::POLICY)
        ));
        assert!(!independently_accepted(None, None, Some("review")));
        assert!(!independently_accepted(
            None,
            None,
            Some("ordinary_confirm")
        ));
        // An empty basis is recorded, never verified.
        assert!(!independently_accepted(None, None, None));
    }

    #[test]
    fn matches_the_payloads_the_two_producers_actually_write() {
        // Current producer: workstream_command/reviews.rs builds the
        // work.completed event with independence_kind and the boolean
        // acceptance flag in the same payload.
        let current_agent_review = serde_json::json!({
            "receipt_id": "r1", "evidence_id": "e1", "execution_id": "x1",
            "independence_kind": "agent_review",
            "approval_basis": "agent_review", "execution_basis": "caller_asserted",
            "human_approval": false,
            "team_independent_acceptance": false,
            "approved_by_person_id": "lead"
        });
        assert!(independently_accepted(
            current_agent_review["independence_kind"].as_str(),
            current_agent_review["team_independent_acceptance"].as_bool(),
            current_agent_review["policy"].as_str(),
        ));
        let current_self_review = serde_json::json!({
            "receipt_id": "r2", "evidence_id": "e2",
            "independence_kind": "personal_self_review",
            "human_approval": true,
            "team_independent_acceptance": false
        });
        assert!(!independently_accepted(
            current_self_review["independence_kind"].as_str(),
            current_self_review["team_independent_acceptance"].as_bool(),
            current_self_review["policy"].as_str(),
        ));
        // Legacy producer: review.rs writes receipt, evidence and policy only.
        let legacy_agent_policy = serde_json::json!({
            "receipt_id": "r3", "evidence_id": "e3",
            "policy": crate::review::AGENT_REVIEW_POLICY
        });
        assert!(independently_accepted(
            legacy_agent_policy["independence_kind"].as_str(),
            legacy_agent_policy["team_independent_acceptance"].as_bool(),
            legacy_agent_policy["policy"].as_str(),
        ));
        let legacy_plain_review = serde_json::json!({
            "receipt_id": "r4", "evidence_id": "e4", "policy": "review"
        });
        assert!(!independently_accepted(
            legacy_plain_review["independence_kind"].as_str(),
            legacy_plain_review["team_independent_acceptance"].as_bool(),
            legacy_plain_review["policy"].as_str(),
        ));
    }
}
