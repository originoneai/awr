#[path = "fixtures/github.rs"]
mod github;
use awr_server::delivery_adapter::{
    GitHubAdapter, GitHubError, github::MANIFEST_CHECK, local_git::ArtifactState,
};
use awr_team::{TenantId, delivery::*};
use github::*;
use serde_json::json;
use std::fs;

#[tokio::test]
async fn mapped_artifacts_and_checks_are_inspectable_neutral_facts_without_approval() {
    let f = Fixture::new();
    let adapter = f.adapter();
    assert_eq!(f.api.calls(), 0);
    assert!(!adapter.capabilities().integration_requests);
    let snapshot = adapter.inspect(&f.candidate(), "one").await.unwrap();
    assert_eq!(
        snapshot.report.manifest_outcome,
        VerificationOutcome::Passed
    );
    assert_eq!(
        snapshot.report.checks[0].outcome,
        VerificationOutcome::Passed
    );
    assert_eq!(
        snapshot.report.integration_outcome,
        IntegrationOutcome::Pending
    );
    assert_eq!(
        snapshot.report.source_artifacts[0]
            .observed_sha256
            .as_deref(),
        Some(hash(BYTES).as_str())
    );
    assert_eq!(
        hash(
            &adapter
                .report_bytes(&snapshot.report_artifact.sha256)
                .unwrap()
        ),
        snapshot.report_artifact.sha256
    );
    assert_eq!(snapshot.records.len(), 4);
    for record in &snapshot.records {
        record.validate().unwrap();
        assert!(!matches!(
            record.record,
            DeliveryRecord::ReviewDecision(_) | DeliveryRecord::IntegrationRequest(_)
        ));
    }
}

#[tokio::test]
async fn exact_target_with_actual_bytes_proves_integration_independently_of_pr_state() {
    let mut f = Fixture::new();
    f.config.pull_number = None;
    f.api.set("/git/ref/heads/main", reference(SOURCE));
    let snapshot = f
        .adapter()
        .inspect(&f.candidate(), "applied")
        .await
        .unwrap();
    assert_eq!(
        snapshot.report.integration_outcome,
        IntegrationOutcome::Applied
    );
    assert!(!snapshot.report.target_precondition_matches);
    assert!(snapshot.report.pull.is_none());
    assert_eq!(
        snapshot.report.target_artifacts[0].state,
        ArtifactState::Passed
    );
    let DeliveryRecord::IntegrationObservation(observation) =
        &snapshot.records.last().unwrap().record
    else {
        panic!("expected target observation")
    };
    assert_eq!(
        observation.contains_manifest_digest.as_deref(),
        Some(f.candidate().binding.manifest_digest.as_str())
    );
}

#[tokio::test]
async fn merged_pr_alone_does_not_prove_target_contains_the_candidate() {
    let f = Fixture::new();
    let mut p = pull();
    p["merged"] = json!(true);
    p["state"] = json!("closed");
    f.api.set("/pulls/11", p);
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "merged-pr")
            .await
            .unwrap()
            .report
            .integration_outcome,
        IntegrationOutcome::Pending
    );
}

#[tokio::test]
async fn source_ancestry_still_requires_matching_target_artifact_bytes() {
    let f = Fixture::new();
    f.api.set(
        &format!("/compare/{SOURCE}...{TARGET}"),
        json!({"status":"ahead","base_commit":{"sha":SOURCE},"merge_base_commit":{"sha":SOURCE}}),
    );
    // Reusing the immutable blob is valid when the target's tree retains the artifact.
    let good = f
        .adapter()
        .inspect(&f.candidate(), "ancestor")
        .await
        .unwrap();
    assert_eq!(good.report.integration_outcome, IntegrationOutcome::Applied);
    let new_tree = "e".repeat(40);
    let new_blob = "f".repeat(40);
    f.api.set(
        &format!("/git/commits/{TARGET}"),
        json!({"sha":TARGET,"tree":{"sha":new_tree}}),
    );
    f.api.set(&format!("/git/trees/{new_tree}"), json!({"sha":new_tree,"truncated":false,"tree":[{"path":"result.txt","type":"blob","mode":"100644","sha":new_blob,"size":16}]}));
    f.api.set(
        &format!("/git/blobs/{new_blob}"),
        json!({"sha":new_blob,"size":16,"encoding":"base64","content":"Y2hhbmdlZCBvdXRwdXQhCg=="}),
    );
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "changed-bytes")
            .await
            .unwrap()
            .report
            .integration_outcome,
        IntegrationOutcome::Unknown
    );
}

#[tokio::test]
async fn drifted_target_or_pr_cannot_become_a_terminal_passing_observation() {
    let f = Fixture::new();
    f.api.sequence(
        "/git/ref/heads/main",
        vec![reference(SOURCE), reference(TARGET)],
    );
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "target-drift")
            .await
            .unwrap()
            .report
            .integration_outcome,
        IntegrationOutcome::Unknown
    );
    let mut p = pull();
    p["state"] = json!("closed");
    f.api.sequence("/pulls/11", vec![pull(), p]);
    let report = f
        .adapter()
        .inspect(&f.candidate(), "pull-drift")
        .await
        .unwrap()
        .report;
    assert_eq!(report.manifest_outcome, VerificationOutcome::Unknown);
    assert_eq!(report.checks[0].outcome, VerificationOutcome::Unknown);
}

#[tokio::test]
async fn check_producer_status_and_complete_pagination_are_required() {
    let f = Fixture::new();
    let route = format!("/commits/{SOURCE}/check-runs?filter=latest&per_page=100&page=1");
    for (i, outcome) in ["skipped", "neutral", "stale", "unknown"]
        .into_iter()
        .enumerate()
    {
        let mut check = run();
        check["conclusion"] = json!(outcome);
        f.api.set(
            &route,
            json!({"total_count":1,"check_runs":[check.clone()]}),
        );
        f.api.set("/check-runs/1", check);
        assert_eq!(
            f.adapter()
                .inspect(&f.candidate(), &format!("skip-{i}"))
                .await
                .unwrap()
                .report
                .checks[0]
                .outcome,
            VerificationOutcome::Unknown
        );
    }
    let mut wrong = run();
    wrong["app"]["id"] = json!(10);
    f.api
        .set(&route, json!({"total_count":1,"check_runs":[wrong]}));
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "wrong-app")
            .await
            .unwrap()
            .report
            .checks[0]
            .outcome,
        VerificationOutcome::Unknown
    );
    f.api
        .set(&route, json!({"total_count":2,"check_runs":[run()]}));
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "missing-page")
            .await
            .unwrap_err(),
        GitHubError::InvalidResponse
    );
}

#[tokio::test]
async fn pagination_checks_head_duplicates_and_reobserves_a_current_run() {
    let f = Fixture::new();
    let route = format!("/commits/{SOURCE}/check-runs?filter=latest&per_page=100&page=1");
    let checks: Vec<_> = (1..=101)
        .map(|id| {
            let mut r = run();
            r["id"] = json!(id);
            r["name"] = json!(if id == 101 { "CI" } else { "other" });
            r
        })
        .collect();
    f.api.set(
        &route,
        json!({"total_count":101,"check_runs":checks[..100]}),
    );
    f.api.set(
        &format!("/commits/{SOURCE}/check-runs?filter=latest&per_page=100&page=2"),
        json!({"total_count":101,"check_runs":[checks[100]]}),
    );
    f.api.set("/check-runs/101", checks[100].clone());
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "pages")
            .await
            .unwrap()
            .report
            .checks[0]
            .outcome,
        VerificationOutcome::Passed
    );
    let mut r = run();
    r["head_sha"] = json!(TARGET);
    f.api.set(&route, json!({"total_count":1,"check_runs":[r]}));
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "wrong-head")
            .await
            .unwrap_err(),
        GitHubError::InvalidResponse
    );
    let mut second = run();
    second["id"] = json!(2);
    f.api
        .set(&route, json!({"total_count":2,"check_runs":[run(),second]}));
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "duplicate-name")
            .await
            .unwrap()
            .report
            .checks[0]
            .outcome,
        VerificationOutcome::Unknown
    );
    f.api
        .set(&route, json!({"total_count":1,"check_runs":[run()]}));
    let mut changed = run();
    changed["conclusion"] = json!("failure");
    f.api.set("/check-runs/1", changed);
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "changed-run")
            .await
            .unwrap()
            .report
            .checks[0]
            .outcome,
        VerificationOutcome::Unknown
    );
}

#[tokio::test]
async fn manifest_hash_length_and_regular_blob_structure_are_all_checked() {
    let f = Fixture::new();
    let mut c = f.candidate();
    c.manifest.entries[0].sha256 = "f".repeat(64);
    c.binding.manifest_digest = c.manifest.digest().unwrap();
    assert_eq!(
        f.adapter()
            .inspect(&c, "hash-mismatch")
            .await
            .unwrap()
            .report
            .manifest_outcome,
        VerificationOutcome::Failed
    );
    for (i, mode) in ["120000", "160000"].into_iter().enumerate() {
        f.api.set(&format!("/git/trees/{TREE}"),json!({"sha":TREE,"truncated":false,"tree":[{"path":"result.txt","type":"blob","mode":mode,"sha":BLOB,"size":BYTES.len()}]}));
        assert_eq!(
            f.adapter()
                .inspect(&f.candidate(), &format!("mode-{i}"))
                .await
                .unwrap()
                .report
                .manifest_outcome,
            VerificationOutcome::Unknown
        );
    }
    f.api.set(
        &format!("/git/trees/{TREE}"),
        json!({"sha":TREE,"truncated":true,"tree":[]}),
    );
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "truncated-tree")
            .await
            .unwrap_err(),
        GitHubError::InvalidResponse
    );
}

#[tokio::test]
async fn bound_mappings_reject_changes_before_observation_and_pin_repository_identity() {
    let f = Fixture::new();
    let adapter = f.adapter();
    let mut c = f.candidate();
    c.binding.tenant_id = TenantId::new("other").unwrap();
    assert_eq!(
        adapter.inspect(&c, "scope").await.unwrap_err(),
        GitHubError::BindingMismatch
    );
    assert_eq!(f.api.calls(), 0);
    let mut p = pull();
    p["head"]["repo"]["id"] = json!(8);
    f.api.set("/pulls/11", p);
    assert_eq!(
        adapter.inspect(&f.candidate(), "fork").await.unwrap_err(),
        GitHubError::BindingMismatch
    );
    f.api.set("/pulls/11", pull());
    f.api.sequence(
        "",
        vec![
            json!({"id":7,"full_name":"acme/demo"}),
            json!({"id":8,"full_name":"acme/demo"}),
        ],
    );
    assert_eq!(
        adapter
            .inspect(&f.candidate(), "repository-replaced")
            .await
            .unwrap_err(),
        GitHubError::BindingMismatch
    );
    assert_eq!(f.files(), 0);
}

#[tokio::test]
async fn provider_errors_limits_and_deadlines_never_publish_synthetic_terminal_facts() {
    for (status, error) in [
        (401, GitHubError::ProviderAuthorizationUnavailable),
        (403, GitHubError::ProviderAuthorizationUnavailable),
        (429, GitHubError::RateLimited),
        (500, GitHubError::ProviderUnavailable),
        (302, GitHubError::ProviderUnavailable),
    ] {
        let f = Fixture::new();
        f.api.raw("", status, b"synthetic-error-secret".to_vec());
        assert_eq!(
            f.adapter()
                .inspect(&f.candidate(), "failed")
                .await
                .unwrap_err(),
            error
        );
        assert_eq!(f.files(), 0);
    }
    let mut f = Fixture::new();
    f.config.max_requests = 1;
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "request-limit")
            .await
            .unwrap_err(),
        GitHubError::OutputLimit
    );
    let mut f = Fixture::new();
    f.config.max_response_bytes = 1024;
    f.api.raw("", 200, vec![b'a'; 1025]);
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "byte-limit")
            .await
            .unwrap_err(),
        GitHubError::OutputLimit
    );
    let mut f = Fixture::new();
    f.config.request_timeout_ms = 100;
    f.config.inspection_timeout_ms = 100;
    f.api.0.lock().unwrap().delay_ms = 120;
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "timeout")
            .await
            .unwrap_err(),
        GitHubError::TimedOut
    );
    assert_eq!(f.files(), 0);
}

#[tokio::test]
async fn original_inspections_survive_restart_and_concurrent_calls_without_rewriting_proof() {
    let f = Fixture::new();
    let adapter = f.adapter();
    let c = f.candidate();
    let (one, two) = tokio::join!(adapter.inspect(&c, "same"), adapter.inspect(&c, "same"));
    let first = one.unwrap();
    assert_eq!(first.report_artifact, two.unwrap().report_artifact);
    let calls = f.api.calls();
    f.api.set("", json!({"id":8,"full_name":"acme/demo"}));
    assert_eq!(
        f.adapter()
            .inspect(&c, "same")
            .await
            .unwrap()
            .report_artifact,
        first.report_artifact
    );
    assert_eq!(f.api.calls(), calls);
    let mut changed = c.clone();
    changed.binding.candidate_version = "2".into();
    assert_eq!(
        adapter.inspect(&changed, "same").await.unwrap_err(),
        GitHubError::ReportConflict
    );
    fs::write(
        f.root.join(format!(
            "github-report-{}.json",
            first.report_artifact.sha256
        )),
        b"corrupt",
    )
    .unwrap();
    assert_eq!(
        adapter.inspect(&c, "same").await.unwrap_err(),
        GitHubError::ReportConflict
    );
}

#[test]
fn configuration_and_unsupported_required_checks_are_explicit() {
    let f = Fixture::new();
    for base in [
        "http://github.example",
        "https://user:password@github.example",
        "https://github.example/?token=value",
        "https://github.example/other",
    ] {
        let mut config = f.config.clone();
        config.api_base_url = base.into();
        assert!(matches!(
            GitHubAdapter::from_transport(config, f.api.clone()),
            Err(GitHubError::InvalidConfiguration)
        ));
    }
    let config: awr_server::delivery_adapter::GitHubConfig = toml::from_str(include_str!(
        "../../../examples/github-delivery/adapter.toml"
    ))
    .unwrap();
    assert_eq!(config.version, 1);
    assert_eq!(config.checks[0].check, "ci");
    assert_ne!(MANIFEST_CHECK, "ci");
}

#[tokio::test]
async fn unsupported_and_missing_checks_do_not_gain_a_synthetic_pass() {
    let f = Fixture::new();
    let mut c = f.candidate();
    c.binding.required_checks.push("unmapped".into());
    let result = f.adapter().inspect(&c, "unsupported-check").await.unwrap();
    assert_eq!(result.report.unsupported_required_checks, vec!["unmapped"]);
    assert_eq!(result.report.checks.len(), 1);
    f.api.set(
        &format!("/commits/{SOURCE}/check-runs?filter=latest&per_page=100&page=1"),
        json!({"total_count":0,"check_runs":[]}),
    );
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "missing-check")
            .await
            .unwrap()
            .report
            .checks[0]
            .outcome,
        VerificationOutcome::Unknown
    );
}

#[tokio::test]
async fn immutable_nested_artifacts_are_resolved_from_trees_without_following_urls() {
    let f = Fixture::new();
    let sub = "e".repeat(40);
    f.api.set(&format!("/git/trees/{TREE}"),json!({"sha":TREE,"truncated":false,"tree":[{"path":"docs","type":"tree","mode":"040000","sha":sub}]}));
    f.api.set(&format!("/git/trees/{sub}"),json!({"sha":sub,"truncated":false,"tree":[{"path":"result.txt","type":"blob","mode":"100755","sha":BLOB,"size":BYTES.len()}]}));
    let mut c = f.candidate();
    c.manifest.entries[0].locator = "git-blob:docs/result.txt".into();
    c.binding.manifest_digest = c.manifest.digest().unwrap();
    assert_eq!(
        f.adapter()
            .inspect(&c, "nested")
            .await
            .unwrap()
            .report
            .manifest_outcome,
        VerificationOutcome::Passed
    );
    for (index, path) in [
        "git-blob:../result.txt",
        "https://elsewhere.invalid/secret",
        "git-blob:/etc/passwd",
    ]
    .into_iter()
    .enumerate()
    {
        c.manifest.entries[0].locator = path.into();
        c.binding.manifest_digest = c.manifest.digest().unwrap();
        assert_eq!(
            f.adapter()
                .inspect(&c, &format!("unsupported-locator-{index}"))
                .await
                .unwrap()
                .report
                .manifest_outcome,
            VerificationOutcome::Unknown
        );
    }
}

#[tokio::test]
async fn blob_and_cumulative_response_limits_preserve_unknown_or_failed_query() {
    let mut f = Fixture::new();
    f.config.max_blob_bytes = 1;
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "large-blob")
            .await
            .unwrap()
            .report
            .source_artifacts[0]
            .state,
        ArtifactState::TooLarge
    );
    let mut f = Fixture::new();
    f.config.max_response_bytes = 1024;
    f.config.max_total_response_bytes = 1024;
    f.api.set(
        "",
        json!({"id":7,"full_name":"acme/demo","unused":"x".repeat(600)}),
    );
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "total-response-limit")
            .await
            .unwrap_err(),
        GitHubError::OutputLimit
    );
    assert_eq!(f.files(), 0);
    let f = Fixture::new();
    f.api.set(
        &format!("/commits/{SOURCE}/check-runs?filter=latest&per_page=100&page=1"),
        json!({"total_count":301,"check_runs":[]}),
    );
    assert_eq!(
        f.adapter()
            .inspect(&f.candidate(), "check-limit")
            .await
            .unwrap_err(),
        GitHubError::OutputLimit
    );
}

#[tokio::test]
async fn missing_target_and_real_failed_checks_remain_separate_delivery_facts() {
    let f = Fixture::new();
    f.api.raw("/git/ref/heads/main", 404, Vec::new());
    let mut check = run();
    check["conclusion"] = json!("failure");
    f.api.set(
        &format!("/commits/{SOURCE}/check-runs?filter=latest&per_page=100&page=1"),
        json!({"total_count":1,"check_runs":[check.clone()]}),
    );
    f.api.set("/check-runs/1", check);
    let mut c = f.candidate();
    c.binding.target.precondition = TargetPrecondition::Missing;
    let result = f.adapter().inspect(&c, "missing-target").await.unwrap();
    assert_eq!(result.report.checks[0].outcome, VerificationOutcome::Failed);
    assert_eq!(
        result.report.integration_outcome,
        IntegrationOutcome::Pending
    );
    assert!(result.report.target_revision.is_none());
    assert!(result.report.target_precondition_matches);
}
