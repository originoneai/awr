//! Configuration conformance; actual effects and authorities are tested in PG fixtures.
#[path = "fixtures/local_git.rs"]
mod git;
#[path = "fixtures/github.rs"]
mod github;
#[path = "fixtures/github_receive.rs"]
mod receive;
use awr_server::delivery_adapter::*;

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
