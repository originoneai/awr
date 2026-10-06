//! Mechanism conformance, never native business acceptance or approval.
#[path = "fixtures/local_git.rs"]
mod fixture;
use awr_server::delivery_adapter::{LocalGitAdapter, LocalGitError, local_git::ArtifactState};
use awr_team::delivery::*;
use fixture::*;

async fn observe(sha256: bool) {
    let f = GitFixture::new(sha256);
    let adapter = f.adapter().await;
    let candidate = f.candidate();
    let before = f.bare(&["for-each-ref", "--format=%(refname) %(objectname)"]);
    let first = adapter.inspect(&candidate, "first").await.unwrap();
    assert_eq!(first.report.manifest_outcome, VerificationOutcome::Passed);
    assert_eq!(
        first.report.integration_outcome,
        IntegrationOutcome::Pending
    );
    assert!(first.report.target_precondition_matches);
    assert_eq!(first.report.unsupported_required_checks, vec!["unit-tests"]);
    assert_eq!(
        before,
        f.bare(&["for-each-ref", "--format=%(refname) %(objectname)"])
    );
    assert_eq!(
        adapter
            .report_bytes(&first.report_artifact.sha256)
            .unwrap()
            .len()
            .to_string(),
        first.report_artifact.byte_length
    );
    f.push_main(); // Real fixture effect; the observation adapter has no push capability.
    let second = adapter.inspect(&candidate, "after-push").await.unwrap();
    assert_eq!(
        second.report.integration_outcome,
        IntegrationOutcome::Applied
    );
    assert_eq!(
        second.report.target_revision.as_ref().unwrap().value,
        f.source
    );
    assert!(!second.report.target_precondition_matches);
    assert_eq!(second.report.graph_contains_source, Some(true));
    for record in &second.records {
        assert_eq!(
            parse_delivery_record(&serde_json::to_vec(record).unwrap()).unwrap(),
            *record
        );
    }
    let reopened = f.adapter().await;
    let recovered = reopened.inspect(&candidate, "first").await.unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(first).unwrap()
    );
    assert!(!adapter.capabilities().integration_requests);
    assert!(!adapter.capabilities().change_requests);
    assert!(adapter.capabilities().polling);
}

#[tokio::test]
async fn sha1_query_push_observation_and_restart() {
    observe(false).await;
}
#[tokio::test]
async fn sha256_query_push_observation_and_restart() {
    observe(true).await;
}

#[tokio::test]
async fn manifest_mismatch_and_missing_revisions_do_not_pass() {
    let f = GitFixture::new(false);
    f.push_main();
    let adapter = f.adapter().await;
    let mut candidate = f.candidate();
    candidate.manifest.entries[0].sha256 = "0".repeat(64);
    rebind(&mut candidate);
    let wrong = adapter.inspect(&candidate, "wrong-hash").await.unwrap();
    assert_eq!(wrong.report.manifest_outcome, VerificationOutcome::Failed);
    assert_ne!(
        wrong.report.integration_outcome,
        IntegrationOutcome::Applied
    );
    candidate.binding.source_revision.as_mut().unwrap().value = "0".repeat(40);
    let absent = adapter.inspect(&candidate, "no-object").await.unwrap();
    assert_eq!(absent.report.manifest_outcome, VerificationOutcome::Unknown);
    assert_ne!(
        absent.report.integration_outcome,
        IntegrationOutcome::Applied
    );
    f.git(&["tag", "-a", "synthetic-tag", "-m", "tag"]);
    candidate.binding.source_revision.as_mut().unwrap().value =
        f.git(&["rev-parse", "synthetic-tag"]);
    f.git(&[
        "push",
        f.config.repository.to_str().unwrap(),
        "refs/tags/synthetic-tag",
    ]);
    assert_eq!(
        adapter
            .inspect(&candidate, "tag-object")
            .await
            .unwrap()
            .report
            .manifest_outcome,
        VerificationOutcome::Unknown
    );
}

#[tokio::test]
async fn ancestor_without_current_artifact_content_is_not_integrated() {
    let f = GitFixture::new(false);
    f.commit("deliverable.txt", b"replacement\n");
    f.push_main();
    let result = f
        .adapter()
        .await
        .inspect(&f.candidate(), "changed-artifact")
        .await
        .unwrap();
    assert_eq!(result.report.graph_contains_source, Some(true));
    assert_eq!(
        result.report.target_artifacts[0].state,
        ArtifactState::Mismatch
    );
    assert_eq!(
        result.report.integration_outcome,
        IntegrationOutcome::Unknown
    );
}

#[tokio::test]
async fn matching_tree_without_ancestry_has_no_inclusion_proof() {
    let f = GitFixture::new(false);
    let tree = f.git(&["rev-parse", "HEAD^{tree}"]);
    let independent = f.git(&["commit-tree", &tree, "-m", "Independent tree"]);
    f.git(&[
        "push",
        "--force",
        f.config.repository.to_str().unwrap(),
        &format!("{independent}:refs/heads/main"),
    ]);
    let result = f
        .adapter()
        .await
        .inspect(&f.candidate(), "unproved-squash")
        .await
        .unwrap();
    assert_eq!(result.report.graph_contains_source, Some(false));
    assert_ne!(
        result.report.integration_outcome,
        IntegrationOutcome::Applied
    );
}

#[tokio::test]
async fn literal_paths_empty_blobs_symlinks_and_bounds() {
    let mut f = GitFixture::new(false);
    f.commit("报告 [draft].txt", b"");
    f.git(&[
        "push",
        f.config.repository.to_str().unwrap(),
        "HEAD:refs/heads/empty",
    ]);
    let mut candidate = f.candidate();
    candidate.binding.source_revision.as_mut().unwrap().value = f.git(&["rev-parse", "HEAD"]);
    candidate.manifest.entries[0] = ArtifactEntry {
        artifact_id: "empty".into(),
        sha256: sha(b""),
        byte_length: "0".into(),
        locator: "git-blob:报告 [draft].txt".into(),
    };
    rebind(&mut candidate);
    assert_eq!(
        f.adapter()
            .await
            .inspect(&candidate, "empty-literal")
            .await
            .unwrap()
            .report
            .manifest_outcome,
        VerificationOutcome::Passed
    );
    let mut traversal = candidate.clone();
    traversal.manifest.entries[0].locator = "git-blob:../outside".into();
    rebind(&mut traversal);
    assert_eq!(
        f.adapter()
            .await
            .inspect(&traversal, "traversal")
            .await
            .unwrap()
            .report
            .source_artifacts[0]
            .state,
        ArtifactState::Unsupported
    );
    let blob = f.git(&["rev-parse", "HEAD:deliverable.txt"]);
    f.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{blob},link"),
    ]);
    f.git(&["commit", "-m", "Synthetic symlink object"]);
    f.git(&[
        "push",
        f.config.repository.to_str().unwrap(),
        "HEAD:refs/heads/link",
    ]);
    candidate.binding.source_revision.as_mut().unwrap().value = f.git(&["rev-parse", "HEAD"]);
    candidate.manifest.entries[0].locator = "git-blob:link".into();
    rebind(&mut candidate);
    assert_eq!(
        f.adapter()
            .await
            .inspect(&candidate, "symlink")
            .await
            .unwrap()
            .report
            .source_artifacts[0]
            .state,
        ArtifactState::Unsupported
    );
    f.config.max_blob_bytes = 1;
    let result = f
        .adapter()
        .await
        .inspect(&f.candidate(), "limited")
        .await
        .unwrap();
    assert_eq!(
        result.report.source_artifacts[0].state,
        ArtifactState::TooLarge
    );
    assert_eq!(result.report.manifest_outcome, VerificationOutcome::Unknown);
}

#[tokio::test]
async fn configuration_scope_and_report_corruption_are_refused() {
    let f = GitFixture::new(false);
    let adapter = f.adapter().await;
    let mut candidate = f.candidate();
    candidate.binding.target.reference = Some("refs/heads/other".into());
    assert!(matches!(
        adapter.inspect(&candidate, "wrong-ref").await,
        Err(LocalGitError::BindingMismatch)
    ));
    candidate = f.candidate();
    let snapshot = adapter.inspect(&candidate, "recover").await.unwrap();
    let path = f
        .config
        .report_directory
        .join(format!("report-{}.json", snapshot.report_artifact.sha256));
    std::fs::write(path, b"corrupted").unwrap();
    assert!(matches!(
        adapter.inspect(&candidate, "recover").await,
        Err(LocalGitError::ReportConflict)
    ));
    assert!(matches!(
        adapter.report_bytes("../outside"),
        Err(LocalGitError::ReportUnavailable)
    ));
    let mut config = f.config.clone();
    config.report_directory = config.repository.clone();
    assert!(matches!(
        LocalGitAdapter::open(config).await,
        Err(LocalGitError::InvalidConfiguration)
    ));
    let mut config = f.config.clone();
    config.repository = f.work.clone();
    assert!(matches!(
        LocalGitAdapter::open(config).await,
        Err(LocalGitError::InvalidConfiguration)
    ));
    let mut config = f.config.clone();
    config.target_reference = "refs/heads/../other".into();
    assert!(matches!(
        LocalGitAdapter::open(config).await,
        Err(LocalGitError::InvalidConfiguration)
    ));
    let unknown = serde_json::json!({"version":1,"caller_verified":true});
    assert!(
        serde_json::from_value::<awr_server::delivery_adapter::LocalGitConfig>(unknown).is_err()
    );
}

#[tokio::test]
async fn concurrent_inspection_publishes_one_recoverable_report() {
    let f = GitFixture::new(false);
    let adapter = f.adapter().await;
    let candidate = f.candidate();
    let (a, b) = tokio::join!(
        adapter.inspect(&candidate, "concurrent"),
        adapter.inspect(&candidate, "concurrent")
    );
    assert_eq!(
        serde_json::to_value(a.unwrap()).unwrap(),
        serde_json::to_value(b.unwrap()).unwrap()
    );
    let mut changed = candidate;
    changed.binding.candidate_version = "2".into();
    assert!(matches!(
        adapter.inspect(&changed, "concurrent").await,
        Err(LocalGitError::ReportConflict)
    ));
}

#[tokio::test]
async fn missing_target_is_pending_and_promisor_configuration_is_refused() {
    let f = GitFixture::new(false);
    f.bare(&["update-ref", "-d", "refs/heads/main"]);
    let mut candidate = f.candidate();
    candidate.binding.target.precondition = TargetPrecondition::Missing;
    let adapter = f.adapter().await;
    let result = adapter.inspect(&candidate, "missing-target").await.unwrap();
    assert!(result.report.target_precondition_matches);
    assert_eq!(
        result.report.integration_outcome,
        IntegrationOutcome::Pending
    );
    f.bare(&["config", "remote.private.promisor", "true"]);
    assert!(matches!(
        adapter.inspect(&candidate, "changed-config").await,
        Err(LocalGitError::RepositoryUnavailable)
    ));
    assert!(matches!(
        f.adapter_config_result().await,
        Err(LocalGitError::InvalidConfiguration)
    ));
}

impl GitFixture {
    async fn adapter_config_result(&self) -> Result<LocalGitAdapter, LocalGitError> {
        LocalGitAdapter::open(self.config.clone()).await
    }
}

#[tokio::test]
async fn corrupt_reference_is_an_error_and_cannot_prove_absence() {
    let f = GitFixture::new(false);
    let adapter = f.adapter().await;
    std::fs::write(f.config.repository.join("refs/heads/main"), b"not-a-ref\n").unwrap();
    assert!(matches!(
        adapter.inspect(&f.candidate(), "corrupt-target").await,
        Err(LocalGitError::RepositoryUnavailable)
    ));
    assert_eq!(
        std::fs::read_dir(&f.config.report_directory)
            .unwrap()
            .count(),
        0
    );
    assert!(matches!(
        f.adapter_config_result().await,
        Err(LocalGitError::InvalidConfiguration)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn unsupported_ref_existence_probe_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let f = GitFixture::new(false);
    let executable = f.root.join("unsupported-git");
    std::fs::write(
        &executable,
        "#!/bin/sh\ncase \"$*\" in\n*--is-bare-repository*) printf 'true\\n';;\n*) exit 129;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = f.config.clone();
    config.git_executable = executable;
    assert!(matches!(
        LocalGitAdapter::open(config).await,
        Err(LocalGitError::InvalidConfiguration)
    ));
}

#[tokio::test]
async fn explicit_scope_object_format_and_example_configuration() {
    let f = GitFixture::new(false);
    let adapter = f.adapter().await;
    let mut other = f.candidate();
    other.binding.work_id = serde_json::from_value(serde_json::json!("other")).unwrap();
    assert!(matches!(
        adapter.inspect(&other, "other-work").await,
        Err(LocalGitError::BindingMismatch)
    ));
    let mut unsupported = f.candidate();
    unsupported.binding.source_revision.as_mut().unwrap().format = RevisionFormat::GitSha256;
    unsupported.binding.source_revision.as_mut().unwrap().value = "0".repeat(64);
    assert_eq!(
        adapter
            .inspect(&unsupported, "unsupported-format")
            .await
            .unwrap()
            .report
            .manifest_outcome,
        VerificationOutcome::Unknown
    );
    let config: awr_server::delivery_adapter::LocalGitConfig = toml::from_str(include_str!(
        "../../../examples/local-git-delivery/adapter.toml"
    ))
    .unwrap();
    assert_eq!(config.version, 1);
    assert_eq!(config.target_reference, "refs/heads/main");
}

#[cfg(unix)]
#[tokio::test]
async fn report_symlink_is_not_followed() {
    let f = GitFixture::new(false);
    let adapter = f.adapter().await;
    let result = adapter
        .inspect(&f.candidate(), "report-symlink")
        .await
        .unwrap();
    let path = f
        .config
        .report_directory
        .join(format!("report-{}.json", result.report_artifact.sha256));
    let outside = f.root.join("outside-report");
    std::fs::write(
        &outside,
        adapter
            .report_bytes(&result.report_artifact.sha256)
            .unwrap(),
    )
    .unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(outside, path).unwrap();
    assert!(matches!(
        adapter.inspect(&f.candidate(), "report-symlink").await,
        Err(LocalGitError::ReportUnavailable)
    ));
}
