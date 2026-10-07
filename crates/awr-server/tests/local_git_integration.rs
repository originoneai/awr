//! Configuration conformance; real effects require the PG-issued sealed permit.
#[path = "fixtures/local_git.rs"]
mod git;
use awr_server::delivery_adapter::{LocalGitError, LocalGitIntegrationConfig, LocalGitIntegrator};
use awr_team::delivery::{IntegrationContentWitness, IntegrationOutcome};

#[tokio::test]
async fn observed_history_rewrite_adds_no_repository_effect_capability() {
    let f = git::GitFixture::new(false);
    let target = f.rewrite_main(Some(&f.base));
    let observer = f.adapter().await;
    let observed = observer
        .inspect(&f.candidate(), "external-rewrite")
        .await
        .unwrap();
    assert_eq!(
        observed.report.integration_outcome,
        IntegrationOutcome::Applied
    );
    assert!(matches!(
        observed.report.content_witness,
        Some(IntegrationContentWitness::MatchingCompleteSnapshots { .. })
    ));
    assert!(!observer.capabilities().integration_requests);
    assert!(!observer.capabilities().change_requests);
    assert_eq!(f.bare(&["rev-parse", "refs/heads/main"]), target);
    assert!(!f.config.report_directory.join("attempts").exists());
}

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
