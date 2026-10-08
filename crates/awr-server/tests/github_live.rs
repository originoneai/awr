//! The observation adapter against the live GitHub API. It needs the network, so it is behind
//! the `github-live` feature (not `#[ignore]`: the native platform contract lists every ignored
//! test and how it is executed). Unauthenticated and read-only, about fifteen GET requests
//! against the public `octocat/Hello-World` repository (the unauthenticated limit is 60 an hour).
//!
//!     cargo test -p awr-server --features github-live --test github_live
//!
//! It runs the very candidates of `github_real_payloads.rs`, through the production HTTPS
//! transport instead of the recording, so a change in GitHub's responses (or in the
//! transport's TLS, header, redirect or size handling) shows up as a failure here.
#![cfg(feature = "github-live")]

#[path = "fixtures/github_real.rs"]
mod real;

use awr_server::delivery_adapter::GitHubAdapter;
use real::*;

#[tokio::test]
async fn the_observer_agrees_with_github_about_a_public_repository() {
    let scratch = Scratch::new();
    let adapter = GitHubAdapter::open(config(scratch.0.clone(), repository_id()), None).unwrap();
    let tip = adapter
        .inspect(&candidate(TIP, PARENT), "live-tip")
        .await
        .unwrap();
    assert_applied(&tip.report, true);
    let parent = adapter
        .inspect(&candidate(PARENT, PARENT), "live-parent")
        .await
        .unwrap();
    assert_applied(&parent.report, false);
}
