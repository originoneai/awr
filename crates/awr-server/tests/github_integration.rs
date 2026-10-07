//! Configuration conformance; actual effects and authorities are tested in PG fixtures.
#[path = "fixtures/local_git.rs"]
mod git;
#[path = "fixtures/github.rs"]
mod github;
#[path = "fixtures/github_receive.rs"]
mod receive;
use awr_server::delivery_adapter::*;
use awr_team::delivery::*;

#[tokio::test]
async fn actual_git_tree_rewrites_are_read_only_and_include_nonmanifest_paths_and_modes() {
    for change in ["unchanged", "nonmanifest", "mode", "lost-base"] {
        let f = receive::Fixture::new();
        let candidate = f.candidate();
        match change {
            "nonmanifest" => f
                .repo
                .commit("outside.txt", b"not in the selected manifest\n"),
            "mode" => {
                f.repo
                    .git(&["update-index", "--chmod=+x", "deliverable.txt"]);
                f.repo.git(&["commit", "-m", "Change fixture mode"]);
            }
            _ => {}
        }
        let tree = f.repo.git(&["rev-parse", "HEAD^{tree}"]);
        let result = f.repo.publish_tree(
            &tree,
            (change != "lost-base").then_some(f.repo.base.as_str()),
        );
        let observer =
            GitHubAdapter::from_transport(f.config.repository.clone(), f.api.clone()).unwrap();
        let observed = observer.inspect(&candidate, "actual-tree").await;
        if change == "lost-base" {
            assert_eq!(observed.unwrap_err(), GitHubError::ProviderUnavailable);
            assert_eq!(f.receive.posts.load(std::sync::atomic::Ordering::SeqCst), 0);
            continue;
        }
        let snapshot = observed.unwrap();
        assert_eq!(snapshot.report.graph_contains_source, Some(false));
        if change == "unchanged" {
            assert_eq!(
                snapshot.report.integration_outcome,
                IntegrationOutcome::Applied
            );
            assert!(matches!(
                snapshot.report.content_witness,
                Some(IntegrationContentWitness::MatchingCompleteSnapshots { .. })
            ));
        } else {
            assert_ne!(
                snapshot.report.integration_outcome,
                IntegrationOutcome::Applied
            );
        }
        assert_eq!(f.repo.bare(&["rev-parse", "refs/heads/main"]), result);
        assert_eq!(f.receive.posts.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn integration_is_explicitly_opt_in_and_opening_does_not_query_or_push() {
    let mut f = receive::Fixture::new();
    f.config.enabled = false;
    assert!(matches!(
        GitHubIntegrator::from_transports(f.config.clone(), f.api.clone(), f.receive.clone()),
        Err(GitHubError::InvalidConfiguration)
    ));
    f.config.enabled = true;
    let integrator = f.integrator();
    assert!(integrator.capabilities().integration_requests);
    assert!(f.api.calls.lock().unwrap().is_empty());
    assert_eq!(f.receive.posts.load(std::sync::atomic::Ordering::SeqCst), 0);
    let config: GitHubIntegrationConfig = toml::from_str(include_str!(
        "../../../examples/github-delivery/integration.toml"
    ))
    .unwrap();
    assert!(config.enabled);
    let extra = format!(
        "{}\nunknown_operation = true\n",
        include_str!("../../../examples/github-delivery/integration.toml")
    );
    assert!(toml::from_str::<GitHubIntegrationConfig>(&extra).is_err());
}
