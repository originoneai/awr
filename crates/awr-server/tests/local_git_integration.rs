//! Configuration conformance; real effects require the PG-issued sealed permit.
#[path = "fixtures/local_git.rs"]
mod git;
use awr_server::delivery_adapter::{LocalGitError, LocalGitIntegrationConfig, LocalGitIntegrator};

#[tokio::test]
async fn integration_is_explicitly_enabled_and_observer_remains_read_only() {
    let f = git::GitFixture::new(false);
    let before = f.bare(&["rev-parse", "refs/heads/main"]);
    assert!(matches!(
        LocalGitIntegrator::open(LocalGitIntegrationConfig {
            enabled: false,
            repository: f.config.clone(),
        })
        .await,
        Err(LocalGitError::InvalidConfiguration)
    ));
    assert!(!f.adapter().await.capabilities().integration_requests);
    let integrator = LocalGitIntegrator::open(LocalGitIntegrationConfig {
        enabled: true,
        repository: f.config.clone(),
    })
    .await
    .unwrap();
    assert!(integrator.capabilities().integration_requests);
    assert!(!integrator.capabilities().change_requests);
    assert_eq!(f.bare(&["rev-parse", "refs/heads/main"]), before);
    assert_eq!(
        std::fs::read_dir(&f.config.report_directory)
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn configured_paths_and_refs_cannot_expand_the_effect_boundary() {
    let f = git::GitFixture::new(false);
    for change in ["reference", "reports", "repository"] {
        let mut config = f.config.clone();
        match change {
            "reference" => config.target_reference = "refs/heads/main..other".into(),
            "reports" => config.report_directory = config.repository.clone(),
            _ => config.repository = f.work.clone(),
        }
        assert!(matches!(
            LocalGitIntegrator::open(LocalGitIntegrationConfig {
                enabled: true,
                repository: config,
            })
            .await,
            Err(LocalGitError::InvalidConfiguration)
        ));
    }
    assert_eq!(f.bare(&["rev-parse", "refs/heads/main"]), f.base);
}
