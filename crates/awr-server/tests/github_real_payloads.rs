//! The observation adapter against real api.github.com responses (recorded, replayed offline).
#[path = "fixtures/github_real.rs"]
mod real;

use real::*;

#[tokio::test]
async fn a_candidate_whose_source_is_the_branch_tip_is_applied_on_real_payloads() {
    let scratch = Scratch::new();
    let (adapter, api) = recorded_adapter(&scratch);
    let snapshot = adapter
        .inspect(&candidate(TIP, PARENT), "real-tip")
        .await
        .unwrap();
    assert_applied(&snapshot.report, true);
    // Every route the adapter used was recorded, and the real README blob (base64 with a
    // trailing newline) was decoded to its 13 bytes.
    let calls = api.calls.lock().unwrap();
    assert!(calls.iter().any(|c| c.starts_with("/git/blobs/")));
    assert!(calls.iter().any(|c| c == "/git/ref/heads/master"));
}

#[tokio::test]
async fn a_candidate_whose_source_is_an_ancestor_with_the_same_tree_is_applied_on_real_payloads() {
    let scratch = Scratch::new();
    let (adapter, api) = recorded_adapter(&scratch);
    let snapshot = adapter
        .inspect(&candidate(PARENT, PARENT), "real-parent")
        .await
        .unwrap();
    assert_applied(&snapshot.report, false);
    // Ancestry comes from GitHub's real compare payload, whose url field the adapter checks
    // against the exact pair it asked for.
    let calls = api.calls.lock().unwrap();
    assert!(calls.iter().any(|c| c.starts_with("/compare/")));
}

#[tokio::test]
async fn a_changed_readme_is_a_mismatch_on_real_payloads() {
    let scratch = Scratch::new();
    let (adapter, _) = recorded_adapter(&scratch);
    let mut changed = candidate(TIP, PARENT);
    changed.manifest.entries[0].sha256 = sha256(b"something else\n");
    changed.binding.manifest_digest = changed.manifest.digest().unwrap();
    let report = adapter
        .inspect(&changed, "real-mismatch")
        .await
        .unwrap()
        .report;
    assert_eq!(
        report.manifest_outcome,
        awr_team::delivery::VerificationOutcome::Failed
    );
    assert_ne!(
        report.integration_outcome,
        awr_team::delivery::IntegrationOutcome::Applied
    );
}
